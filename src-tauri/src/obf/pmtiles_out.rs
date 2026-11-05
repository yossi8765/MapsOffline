//! PMTiles v3 writer for vector tiles (tile type MVT, gzip tiles and directories).
//!
//! Tiles may be added in any order; their bytes go to a temp data file right away (identical
//! contents are stored once), and `finish` sorts the entries by Hilbert tile id, folds runs of
//! consecutive ids with the same content into run-length entries (all-ocean / all-land tiles),
//! builds the root directory (plus leaf directories when the root would not fit in the first
//! 16 KiB) and writes header + root + metadata + leaves + data. The archive is not "clustered"
//! (data is in arrival order), which the spec and every reader allow.
//!
//! Directory serialisation follows the spec (and the raster writer in `data::satellite`,
//! whose helpers are crate-private, so the few lines are repeated here).

use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::bail;

pub use crate::data::satellite::{tile_id, tile_zxy};
use crate::data::satellite::Entry;

pub const HEADER_LEN: usize = 127;
/// Header + root directory must fit in the first 16 KiB.
const ROOT_MAX: usize = 16384 - HEADER_LEN;

/// Archive-level metadata for [`Writer::finish`].
pub struct VectorMeta {
    pub name: String,
    pub description: String,
    pub attribution: String,
    /// [min_lon, min_lat, max_lon, max_lat]
    pub bbox: [f64; 4],
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// `vector_layers` of the TileJSON metadata: (layer id, fields as (name, type)) where type
    /// is "String" / "Number" / "Boolean".
    pub layers: Vec<(String, Vec<(String, String)>)>,
}

/// What `finish` wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveStats {
    pub addressed_tiles: u64,
    pub tile_entries: u64,
    pub tile_contents: u64,
    pub bytes: u64,
}

fn put_varint(buf: &mut Vec<u8>, v: u64) {
    super::mvt::put_varint(buf, v)
}

pub(crate) fn serialize_dir(entries: &[Entry]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 6 + 8);
    put_varint(&mut buf, entries.len() as u64);
    let mut last = 0u64;
    for e in entries {
        put_varint(&mut buf, e.tile_id - last);
        last = e.tile_id;
    }
    for e in entries {
        put_varint(&mut buf, u64::from(e.run_length));
    }
    for e in entries {
        put_varint(&mut buf, u64::from(e.length));
    }
    for (i, e) in entries.iter().enumerate() {
        if i > 0 && e.offset == entries[i - 1].offset + u64::from(entries[i - 1].length) {
            put_varint(&mut buf, 0);
        } else {
            put_varint(&mut buf, e.offset + 1);
        }
    }
    buf
}

pub(crate) fn gzip(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(data.len() / 3 + 64), flate2::Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

/// 128-bit content key (two independent 64-bit hashes + length): collisions are not a concern.
fn content_key(bytes: &[u8]) -> (u64, u64, usize) {
    let mut a = rustc_hash::FxHasher::default();
    bytes.hash(&mut a);
    let mut b = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut b);
    (a.finish(), b.finish(), bytes.len())
}

pub struct Writer {
    data_tmp: PathBuf,
    data: BufWriter<File>,
    data_len: u64,
    entries: Vec<Entry>,
    dedup: HashMap<(u64, u64, usize), (u64, u32)>,
    root_max: usize,
}

impl Writer {
    /// Start an archive whose tile data is staged in `data_tmp` (deleted by `finish` / drop).
    pub fn create(data_tmp: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            data_tmp: data_tmp.to_path_buf(),
            data: BufWriter::with_capacity(1 << 20, File::create(data_tmp)?),
            data_len: 0,
            entries: Vec::new(),
            dedup: HashMap::new(),
            root_max: ROOT_MAX,
        })
    }

    /// Add one tile; `bytes` is the stored (already gzip-compressed) tile. A tile id added twice
    /// keeps its first content.
    pub fn add(&mut self, z: u8, x: u32, y: u32, bytes: &[u8]) -> anyhow::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let key = content_key(bytes);
        let (offset, length) = match self.dedup.get(&key) {
            Some(&v) => v,
            None => {
                let v = (self.data_len, u32::try_from(bytes.len())?);
                self.data.write_all(bytes)?;
                self.data_len += bytes.len() as u64;
                self.dedup.insert(key, v);
                v
            }
        };
        self.entries.push(Entry { tile_id: tile_id(z, x, y), offset, length, run_length: 1 });
        Ok(())
    }

    pub fn tiles(&self) -> usize {
        self.entries.len()
    }

    /// Write the archive to `dest`.
    pub fn finish(mut self, dest: &Path, meta: &VectorMeta) -> anyhow::Result<ArchiveStats> {
        self.data.flush()?;
        if self.entries.is_empty() {
            bail!("no tiles");
        }
        self.entries.sort_by_key(|e| e.tile_id);
        self.entries.dedup_by_key(|e| e.tile_id);
        let addressed = self.entries.len() as u64;
        // run-length: consecutive ids with the same content become one entry
        let mut entries: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for e in self.entries.iter() {
            if let Some(last) = entries.last_mut() {
                if last.offset == e.offset && last.length == e.length && last.tile_id + u64::from(last.run_length) == e.tile_id {
                    last.run_length += 1;
                    continue;
                }
            }
            entries.push(*e);
        }
        let contents = self.dedup.len() as u64;

        let (root, leaves) = build_directories(&entries, self.root_max)?;
        let metadata = gzip(metadata_json(meta).as_bytes())?;

        let root_off = HEADER_LEN as u64;
        let meta_off = root_off + root.len() as u64;
        let leaf_off = meta_off + metadata.len() as u64;
        let data_off = leaf_off + leaves.len() as u64;

        let mut h = Vec::with_capacity(HEADER_LEN);
        h.extend_from_slice(b"PMTiles");
        h.push(3);
        for v in [root_off, root.len() as u64, meta_off, metadata.len() as u64, leaf_off, leaves.len() as u64, data_off, self.data_len, addressed, entries.len() as u64, contents] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.push(0); // not clustered
        h.push(2); // internal compression: gzip
        h.push(2); // tile compression: gzip
        h.push(1); // tile type: mvt
        h.push(meta.min_zoom);
        h.push(meta.max_zoom);
        let e7 = |v: f64| ((v.clamp(-180.0, 180.0) * 1e7).round() as i32).to_le_bytes();
        for v in meta.bbox {
            h.extend_from_slice(&e7(v));
        }
        let center_zoom = meta.min_zoom.max(meta.max_zoom.saturating_sub(4)).min(meta.max_zoom);
        h.push(center_zoom);
        h.extend_from_slice(&e7((meta.bbox[0] + meta.bbox[2]) / 2.0));
        h.extend_from_slice(&e7((meta.bbox[1] + meta.bbox[3]) / 2.0));
        debug_assert_eq!(h.len(), HEADER_LEN);

        let mut out = BufWriter::with_capacity(1 << 20, File::create(dest)?);
        out.write_all(&h)?;
        out.write_all(&root)?;
        out.write_all(&metadata)?;
        out.write_all(&leaves)?;
        let mut src = BufReader::with_capacity(1 << 20, File::open(&self.data_tmp)?);
        std::io::copy(&mut src, &mut out)?;
        out.flush()?;
        drop(src);
        let _ = std::fs::remove_file(&self.data_tmp);
        Ok(ArchiveStats { addressed_tiles: addressed, tile_entries: entries.len() as u64, tile_contents: contents, bytes: data_off + self.data_len })
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.data.flush();
        let _ = std::fs::remove_file(&self.data_tmp);
    }
}

/// Root directory (gzip) and the concatenated leaf directories (gzip each).
fn build_directories(entries: &[Entry], root_max: usize) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let root = gzip(&serialize_dir(entries))?;
    if root.len() <= root_max {
        return Ok((root, Vec::new()));
    }
    let mut leaf_size = 4096usize;
    loop {
        let mut leaves = Vec::new();
        let mut root_entries = Vec::new();
        for chunk in entries.chunks(leaf_size) {
            let bytes = gzip(&serialize_dir(chunk))?;
            root_entries.push(Entry { tile_id: chunk[0].tile_id, offset: leaves.len() as u64, length: u32::try_from(bytes.len())?, run_length: 0 });
            leaves.extend_from_slice(&bytes);
        }
        let root = gzip(&serialize_dir(&root_entries))?;
        if root.len() <= root_max {
            return Ok((root, leaves));
        }
        leaf_size *= 2;
        if leaf_size > 1 << 24 {
            bail!("root directory too large ({} bytes)", root.len());
        }
    }
}

fn metadata_json(meta: &VectorMeta) -> String {
    let layers: Vec<serde_json::Value> = meta
        .layers
        .iter()
        .map(|(id, fields)| {
            let f: serde_json::Map<String, serde_json::Value> = fields.iter().map(|(k, t)| (k.clone(), serde_json::Value::String(t.clone()))).collect();
            serde_json::json!({ "id": id, "fields": f, "minzoom": meta.min_zoom, "maxzoom": meta.max_zoom })
        })
        .collect();
    serde_json::json!({
        "name": meta.name,
        "description": meta.description,
        "attribution": meta.attribution,
        "type": "baselayer",
        "format": "pbf",
        "version": "4",
        "vector_layers": layers,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> VectorMeta {
        VectorMeta {
            name: "test".into(),
            description: String::new(),
            attribution: "© OpenStreetMap".into(),
            bbox: [29.0, -4.5, 30.9, -2.3],
            min_zoom: 0,
            max_zoom: 15,
            layers: vec![("earth".into(), vec![("kind".into(), "String".into())])],
        }
    }

    #[test]
    fn hilbert_ids_follow_the_spec() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
        assert_eq!(tile_id(3, 0, 0), 21);
        assert_eq!(tile_id(12, 3423, 1763), 19078479);
        for (z, x, y) in [(0u8, 0u32, 0u32), (5, 17, 9), (15, 19310, 17051), (15, 32767, 0)] {
            assert_eq!(tile_zxy(tile_id(z, x, y)), (z, x, y));
        }
    }

    #[test]
    fn dir_roundtrip() {
        let entries: Vec<Entry> = (0..1000u64).map(|i| Entry { tile_id: i * 3, offset: i * 10, length: 10, run_length: 1 + (i % 3) as u32 }).collect();
        let back = crate::data::satellite::deserialize_dir(&serialize_dir(&entries)).unwrap();
        assert_eq!(back.len(), entries.len());
        for (a, b) in back.iter().zip(&entries) {
            assert_eq!((a.tile_id, a.offset, a.length, a.run_length), (b.tile_id, b.offset, b.length, b.run_length));
        }
    }

    fn write_archive(dir: &Path, root_max: Option<usize>, tiles: &[(u8, u32, u32, Vec<u8>)]) -> (PathBuf, ArchiveStats) {
        let dest = dir.join("t.pmtiles");
        let mut w = Writer::create(&dir.join("t.data.tmp")).unwrap();
        if let Some(r) = root_max {
            w.root_max = r;
        }
        for (z, x, y, b) in tiles {
            w.add(*z, *x, *y, &gzip(b).unwrap()).unwrap();
        }
        let st = w.finish(&dest, &meta()).unwrap();
        assert!(!dir.join("t.data.tmp").exists());
        (dest, st)
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mo-pmtiles-out-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn archive_roundtrip_through_map_tiles_reader() {
        let d = tmpdir("rt");
        // a run of identical tiles (ocean) at z3 row + some distinct ones, added out of order
        let mut tiles = Vec::new();
        for x in 0..8u32 {
            tiles.push((3u8, x, 2u32, b"ocean".to_vec()));
        }
        tiles.push((0, 0, 0, b"zero".to_vec()));
        tiles.push((2, 1, 1, b"two".to_vec()));
        let (dest, st) = write_archive(&d, None, &tiles);
        assert_eq!(st.addressed_tiles, 10);
        assert_eq!(st.tile_contents, 3);
        assert!(st.tile_entries < 10, "runs folded: {st:?}");
        let f = std::fs::read(&dest).unwrap();
        assert_eq!(&f[..7], b"PMTiles");
        assert_eq!((f[7], f[97], f[98], f[99], f[100], f[101]), (3, 2, 2, 1, 0, 15));
        for (z, x, y, b) in &tiles {
            let t = crate::map_tiles::read_tile_uncached(&dest, *z, *x, *y).unwrap();
            assert_eq!(t.as_deref(), Some(&b[..]), "{z}/{x}/{y}");
        }
        assert_eq!(crate::map_tiles::read_tile_uncached(&dest, 3, 0, 3).unwrap(), None);
        // metadata is gzip JSON with vector_layers
        let mo = u64::from_le_bytes(f[24..32].try_into().unwrap()) as usize;
        let ml = u64::from_le_bytes(f[32..40].try_into().unwrap()) as usize;
        let js: serde_json::Value = serde_json::from_slice(&crate::data::satellite::gunzip(&f[mo..mo + ml]).unwrap()).unwrap();
        assert_eq!(js["vector_layers"][0]["id"], "earth");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leaf_directories() {
        let d = tmpdir("leaf");
        let mut tiles = Vec::new();
        for x in 0..64u32 {
            for y in 0..64u32 {
                tiles.push((6u8, x, y, format!("t{x}/{y}").into_bytes()));
            }
        }
        let (dest, st) = write_archive(&d, Some(256), &tiles);
        assert_eq!(st.addressed_tiles, 4096);
        let f = std::fs::read(&dest).unwrap();
        let leaves_len = u64::from_le_bytes(f[48..56].try_into().unwrap());
        assert!(leaves_len > 0);
        for (z, x, y, b) in tiles.iter().step_by(97) {
            assert_eq!(crate::map_tiles::read_tile_uncached(&dest, *z, *x, *y).unwrap().as_deref(), Some(&b[..]));
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
