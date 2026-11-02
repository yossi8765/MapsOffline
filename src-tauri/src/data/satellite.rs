//! Satellite imagery: Sentinel-2 cloudless tiles (EOX, CC BY-NC-SA 4.0) for the Israel bbox,
//! zoom 0–14 (≈10 m/pixel, the sensor's native resolution), packed into one raster PMTiles v3
//! archive that the frontend opens with the same `pmtiles://` protocol as the vector map.
//!
//! The archive is written by [`Writer`] (no external tool): tiles are appended to a temp data
//! file as they arrive (identical tiles – open sea, desert – are stored once), then header,
//! gzip-compressed directories, metadata and the data section are assembled into the final file.
use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use reqwest::blocking::Client;

use crate::data::download::{self, http_client};
use crate::data::Cancel;
use crate::paths::{atomic_replace, DataPaths, BBOX_ISRAEL};
use crate::types::{ProgressFn, ProgressPhase};

const BASE: &str = "https://tiles.maps.eox.at/wmts/1.0.0/";
pub const MAX_ZOOM: u8 = 14;

/// Max zoom for the chosen quality setting (see `Settings::satellite_quality`).
pub fn quality_zoom(q: &str) -> u8 {
    match q {
        "low" => 12,
        "medium" => 13,
        _ => MAX_ZOOM,
    }
}

/// Manifest version of the satellite archive: mosaic year + detail level.
pub fn archive_version(layer: &str, quality: &str) -> String {
    format!("{layer} z{}", quality_zoom(quality))
}

/// Is the installed version (manifest) what the current setting expects? Archives written before
/// the quality setting existed are full-detail and carry the bare layer name.
pub fn version_matches(installed: Option<&str>, layer: &str, quality: &str) -> bool {
    match installed {
        Some(v) => v == archive_version(layer, quality) || (quality_zoom(quality) == MAX_ZOOM && v == layer),
        None => false,
    }
}
const WORKERS: usize = 12;
const FIRST_YEAR: i32 = 2018;
const ATTRIBUTION: &str = "Sentinel-2 cloudless by EOX IT Services GmbH (CC BY-NC-SA 4.0), contains modified Copernicus Sentinel data";
const MIN_SIZE: u64 = 1024 * 1024;

fn tile_url(layer: &str, z: u8, x: u32, y: u32) -> String {
    format!("{BASE}{layer}_3857/default/g/{z}/{y}/{x}.jpg")
}

/// Newest yearly mosaic the server offers ("s2cloudless-2025"), probing from the current year
/// backwards. A 418 means the content filter blocks the host.
pub(crate) fn latest_layer(client: &Client) -> anyhow::Result<String> {
    let this_year = chrono::Utc::now().format("%Y").to_string().parse::<i32>().unwrap_or(FIRST_YEAR);
    let mut last: Option<anyhow::Error> = None;
    for year in (FIRST_YEAR..=this_year).rev() {
        let layer = format!("s2cloudless-{year}");
        let url = tile_url(&layer, 0, 0, 0);
        match client.head(&url).send() {
            Ok(r) if r.status().is_success() => return Ok(layer),
            Ok(r) if r.status().as_u16() == 418 => bail!("{}", download::blocked_message(&url)),
            Ok(r) if r.status().as_u16() == 404 => {}
            Ok(r) => last = Some(anyhow!("HTTP {} for {url}", r.status())),
            Err(e) => {
                last = Some(e.into());
                break;
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no Sentinel-2 layer found")).context("איתור גרסת תצלום הלוויין נכשל"))
}

// ---------------------------------------------------------------- tile maths

/// PMTiles tile id: Hilbert index within the zoom level plus the count of all tiles above it.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let acc: u64 = ((1u64 << (2 * z as u64)) - 1) / 3;
    let n: u64 = 1 << z;
    let (mut tx, mut ty) = (x as u64, y as u64);
    let mut d = 0u64;
    let mut s = n / 2;
    while s > 0 {
        let rx = u64::from(tx & s > 0);
        let ry = u64::from(ty & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                tx = n - 1 - tx;
                ty = n - 1 - ty;
            }
            std::mem::swap(&mut tx, &mut ty);
        }
        s /= 2;
    }
    acc + d
}

fn lon_to_x(lon: f64, z: u8) -> u32 {
    let n = (1u64 << z) as f64;
    (((lon + 180.0) / 360.0 * n).floor() as i64).clamp(0, (1i64 << z) - 1) as u32
}

fn lat_to_y(lat: f64, z: u8) -> u32 {
    let n = (1u64 << z) as f64;
    let r = lat.to_radians();
    let y = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (y.floor() as i64).clamp(0, (1i64 << z) - 1) as u32
}

fn tile_lon(x: u32, z: u8) -> f64 {
    x as f64 / (1u64 << z) as f64 * 360.0 - 180.0
}

fn tile_lat(y: u32, z: u8) -> f64 {
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y as f64 / (1u64 << z) as f64;
    n.sinh().atan().to_degrees()
}

/// Mediterranean coastline (lat → lon), south to north, used to skip open-sea tiles.
const COAST: [(f64, f64); 14] = [
    (31.30, 34.23),
    (31.52, 34.43),
    (31.67, 34.54),
    (31.80, 34.64),
    (32.08, 34.75),
    (32.33, 34.85),
    (32.45, 34.87),
    (32.70, 34.93),
    (32.82, 34.95),
    (32.92, 35.07),
    (33.09, 35.10),
    (33.20, 35.13),
    (33.35, 35.20),
    (33.50, 35.25),
];
const SEA_MIN_ZOOM: u8 = 12;
const SEA_MARGIN_DEG: f64 = 0.03;

fn coast_lon(lat: f64) -> Option<f64> {
    if lat < COAST[0].0 {
        return None; // Sinai: land all the way to the bbox edge
    }
    for w in COAST.windows(2) {
        let ((la0, lo0), (la1, lo1)) = (w[0], w[1]);
        if lat <= la1 {
            return Some(lo0 + (lo1 - lo0) * (lat - la0) / (la1 - la0));
        }
    }
    Some(COAST[COAST.len() - 1].1)
}

/// Open Mediterranean at zoom ≥ 12: the whole tile lies west of the coast (with a 3 km margin).
pub fn is_open_sea(z: u8, x: u32, y: u32) -> bool {
    if z < SEA_MIN_ZOOM {
        return false;
    }
    let east = tile_lon(x + 1, z);
    let (lat_n, lat_s) = (tile_lat(y, z), tile_lat(y + 1, z));
    // the coast moves east as we go north, so the northern edge is the strictest check
    match (coast_lon(lat_n), coast_lon(lat_s)) {
        (Some(a), Some(b)) => east < a.min(b) - SEA_MARGIN_DEG,
        _ => false,
    }
}

/// Every (z, x, y) covering `bbox` for zoom 0..=max_zoom, optionally minus open-sea tiles.
pub fn tiles_for_bbox(bbox: [f64; 4], max_zoom: u8, skip_sea: bool) -> Vec<(u8, u32, u32)> {
    let mut v = Vec::new();
    for z in 0..=max_zoom {
        let (x0, x1) = (lon_to_x(bbox[0], z), lon_to_x(bbox[2], z));
        let (y0, y1) = (lat_to_y(bbox[3], z), lat_to_y(bbox[1], z));
        for y in y0..=y1 {
            for x in x0..=x1 {
                if !(skip_sea && is_open_sea(z, x, y)) {
                    v.push((z, x, y));
                }
            }
        }
    }
    v
}

// ---------------------------------------------------------------- PMTiles v3 writer

#[derive(Clone, Copy)]
struct Entry {
    tile_id: u64,
    offset: u64,
    length: u32,
    run_length: u32,
}

fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(b);
            return;
        }
        buf.push(b | 0x80);
    }
}

fn get_varint(buf: &[u8], pos: &mut usize) -> anyhow::Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *buf.get(*pos).ok_or_else(|| anyhow!("truncated varint"))?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
        if shift > 63 {
            bail!("varint too long");
        }
    }
}

fn serialize_dir(entries: &[Entry]) -> Vec<u8> {
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

fn deserialize_dir(buf: &[u8]) -> anyhow::Result<Vec<Entry>> {
    let mut pos = 0;
    let n = get_varint(buf, &mut pos)? as usize;
    let mut v: Vec<Entry> = Vec::with_capacity(n);
    let mut last = 0u64;
    for _ in 0..n {
        last += get_varint(buf, &mut pos)?;
        v.push(Entry { tile_id: last, offset: 0, length: 0, run_length: 0 });
    }
    for e in v.iter_mut() {
        e.run_length = get_varint(buf, &mut pos)? as u32;
    }
    for e in v.iter_mut() {
        e.length = get_varint(buf, &mut pos)? as u32;
    }
    for i in 0..n {
        let o = get_varint(buf, &mut pos)?;
        v[i].offset = if o == 0 && i > 0 { v[i - 1].offset + u64::from(v[i - 1].length) } else { o.saturating_sub(1) };
    }
    Ok(v)
}

fn gzip(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

fn gunzip(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

const HEADER_LEN: usize = 127;
const ROOT_MAX: usize = 16384;
const LEAF_ENTRIES: usize = 4096;

pub struct ArchiveMeta {
    pub name: String,
    pub attribution: String,
    pub bbox: [f64; 4],
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// PMTiles tile type: 2 = png, 3 = jpeg
    pub tile_type: u8,
}

/// Incremental raster PMTiles writer: `add` tiles in any order, then `finish`.
/// Tiles go to `data_tmp` as they arrive and an index sidecar records (z, x, y, offset, len) per
/// tile, so an interrupted download resumes where it stopped (`open` → `done` set).
pub struct Writer {
    data_tmp: PathBuf,
    index_tmp: PathBuf,
    data: BufWriter<File>,
    index: BufWriter<File>,
    data_len: u64,
    entries: Vec<Entry>,
    /// every tile already handled: (offset, len) of its bytes (len 0 = no tile on the server)
    done: HashMap<(u8, u32, u32), (u64, u32)>,
    dedup: HashMap<u64, (u64, u32)>,
    /// dedup key of a content-filter placeholder picture, once one was recognised: such tiles
    /// are skipped (not stored, not marked done) so a later run fetches them for real
    placeholder: Option<u64>,
    addressed: u64,
    root_max: usize,
}

const INDEX_REC: usize = 1 + 4 + 4 + 8 + 4;

impl Writer {
    /// Start a new archive (any previous temp files are discarded).
    pub fn create(data_tmp: &Path) -> anyhow::Result<Self> {
        let index_tmp = data_tmp.with_extension("index.tmp");
        let _ = std::fs::remove_file(&index_tmp);
        Ok(Self {
            data_tmp: data_tmp.to_path_buf(),
            data: BufWriter::with_capacity(1 << 20, File::create(data_tmp)?),
            index: BufWriter::new(File::create(&index_tmp)?),
            index_tmp,
            data_len: 0,
            entries: Vec::new(),
            done: HashMap::new(),
            dedup: HashMap::new(),
            placeholder: None,
            addressed: 0,
            root_max: ROOT_MAX,
        })
    }

    /// Continue a previous run if its temp files are intact, otherwise start fresh.
    pub fn open(data_tmp: &Path) -> anyhow::Result<Self> {
        let index_tmp = data_tmp.with_extension("index.tmp");
        let (Ok(idx), Ok(md)) = (std::fs::read(&index_tmp), std::fs::metadata(data_tmp)) else {
            return Self::create(data_tmp);
        };
        let mut entries = Vec::new();
        let mut done = HashMap::new();
        let mut data_len = 0u64;
        for rec in idx.chunks_exact(INDEX_REC) {
            let z = rec[0];
            let x = u32::from_le_bytes(rec[1..5].try_into().unwrap());
            let y = u32::from_le_bytes(rec[5..9].try_into().unwrap());
            let offset = u64::from_le_bytes(rec[9..17].try_into().unwrap());
            let len = u32::from_le_bytes(rec[17..21].try_into().unwrap());
            if offset + u64::from(len) > md.len() {
                break; // data file is shorter than the index claims: stop at the last good record
            }
            done.insert((z, x, y), (offset, len));
            if len > 0 {
                entries.push(Entry { tile_id: tile_id(z, x, y), offset, length: len, run_length: 1 });
                data_len = data_len.max(offset + u64::from(len));
            }
        }
        if done.is_empty() {
            return Self::create(data_tmp);
        }
        let data_file = std::fs::OpenOptions::new().write(true).open(data_tmp)?;
        data_file.set_len(data_len)?;
        let mut data = BufWriter::with_capacity(1 << 20, data_file);
        data.seek(SeekFrom::Start(data_len))?;
        // rewrite the index with only the records we accepted
        let mut index = BufWriter::new(File::create(&index_tmp)?);
        index.write_all(&idx[..done.len() * INDEX_REC])?;
        log::info!("resuming satellite download: {} tiles already stored", done.len());
        Ok(Self { data_tmp: data_tmp.to_path_buf(), index_tmp, data, index, data_len, addressed: entries.len() as u64, entries, done, dedup: HashMap::new(), placeholder: None, root_max: ROOT_MAX })
    }

    pub fn is_done(&self, z: u8, x: u32, y: u32) -> bool {
        self.done.contains_key(&(z, x, y))
    }

    fn record(&mut self, z: u8, x: u32, y: u32, offset: u64, len: u32) -> anyhow::Result<()> {
        let mut rec = [0u8; INDEX_REC];
        rec[0] = z;
        rec[1..5].copy_from_slice(&x.to_le_bytes());
        rec[5..9].copy_from_slice(&y.to_le_bytes());
        rec[9..17].copy_from_slice(&offset.to_le_bytes());
        rec[17..21].copy_from_slice(&len.to_le_bytes());
        self.index.write_all(&rec)?;
        self.done.insert((z, x, y), (offset, len));
        Ok(())
    }

    /// Recreate the index from `done` (after placeholder tiles were dropped).
    fn rewrite_index(&mut self) -> anyhow::Result<()> {
        self.index.flush()?;
        let mut f = BufWriter::new(File::create(&self.index_tmp)?);
        for (&(z, x, y), &(offset, len)) in &self.done {
            let mut rec = [0u8; INDEX_REC];
            rec[0] = z;
            rec[1..5].copy_from_slice(&x.to_le_bytes());
            rec[5..9].copy_from_slice(&y.to_le_bytes());
            rec[9..17].copy_from_slice(&offset.to_le_bytes());
            rec[17..21].copy_from_slice(&len.to_le_bytes());
            f.write_all(&rec)?;
        }
        f.flush()?;
        drop(f);
        self.index = BufWriter::new(std::fs::OpenOptions::new().append(true).open(&self.index_tmp)?);
        Ok(())
    }

    /// Forget every tile that carries the content-filter placeholder (the picture shared by
    /// several zoom <= 11 tiles) and skip that content from now on. Returns how many were dropped.
    pub fn drop_placeholders(&mut self) -> anyhow::Result<usize> {
        let mut counts: HashMap<u64, usize> = HashMap::new();
        for e in self.entries.iter().filter(|e| zoom_of(e.tile_id) <= PLACEHOLDER_MAX_ZOOM) {
            *counts.entry(e.offset).or_insert(0) += 1;
        }
        let Some((&offset, _)) = counts.iter().filter(|(_, &n)| n >= PLACEHOLDER_SHARED).max_by_key(|(_, &n)| n) else { return Ok(0) };
        let before = self.entries.len();
        self.entries.retain(|e| e.offset != offset);
        self.done.retain(|_, &mut (o, len)| !(o == offset && len > 0));
        self.addressed = self.entries.len() as u64;
        self.placeholder = self.dedup.iter().find(|(_, &(o, _))| o == offset).map(|(&k, _)| k);
        self.rewrite_index()?;
        Ok(before - self.entries.len())
    }

    /// Flush data before index so every index record points at bytes already on disk.
    pub fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.data.flush()?;
        self.index.flush()?;
        Ok(())
    }

    /// `bytes` empty = the server has no tile here (remembered so a resume skips it).
    /// Returns false when the tile is a known content-filter placeholder and was skipped.
    pub fn add(&mut self, z: u8, x: u32, y: u32, bytes: &[u8]) -> anyhow::Result<bool> {
        if bytes.is_empty() {
            self.record(z, x, y, self.data_len, 0)?;
            return Ok(true);
        }
        let mut h = rustc_hash::FxHasher::default();
        bytes.hash(&mut h);
        bytes.len().hash(&mut h);
        let key = h.finish();
        if self.placeholder == Some(key) {
            return Ok(false);
        }
        let (offset, length) = match self.dedup.get(&key) {
            Some(&v) => v,
            None => {
                let off = self.data_len;
                self.data.write_all(bytes)?;
                self.data_len += bytes.len() as u64;
                let v = (off, bytes.len() as u32);
                self.dedup.insert(key, v);
                v
            }
        };
        self.entries.push(Entry { tile_id: tile_id(z, x, y), offset, length, run_length: 1 });
        self.addressed += 1;
        self.record(z, x, y, offset, length)?;
        Ok(true)
    }

    pub fn tiles(&self) -> u64 {
        self.addressed
    }

    /// See [`has_placeholders`]; cheap enough to call per low-zoom tile (they number ~1,800).
    pub fn has_placeholders(&self) -> bool {
        has_placeholders(&self.entries)
    }

    /// Assemble the archive at `dest` (written in full, caller renames). The temp data + index
    /// are deleted unless `keep_temps` (a partial archive whose next run should resume).
    pub fn finish(mut self, dest: &Path, meta: &ArchiveMeta, keep_temps: bool) -> anyhow::Result<()> {
        self.data.flush()?;
        self.index.flush()?;
        drop(self.data);
        drop(self.index);
        if self.entries.is_empty() {
            bail!("no tiles");
        }
        self.entries.sort_by_key(|e| e.tile_id);
        self.entries.dedup_by_key(|e| e.tile_id);
        let contents = {
            let mut offs: Vec<u64> = self.entries.iter().map(|e| e.offset).collect();
            offs.sort_unstable();
            offs.dedup();
            offs.len() as u64
        };

        // Directories: one root if it fits, otherwise leaves of LEAF_ENTRIES.
        let mut root = gzip(&serialize_dir(&self.entries))?;
        let mut leaves: Vec<u8> = Vec::new();
        if root.len() > self.root_max {
            let mut root_entries = Vec::new();
            for chunk in self.entries.chunks(LEAF_ENTRIES) {
                let bytes = gzip(&serialize_dir(chunk))?;
                root_entries.push(Entry { tile_id: chunk[0].tile_id, offset: leaves.len() as u64, length: bytes.len() as u32, run_length: 0 });
                leaves.extend_from_slice(&bytes);
            }
            root = gzip(&serialize_dir(&root_entries))?;
            if root.len() > self.root_max {
                bail!("root directory too large ({} bytes)", root.len());
            }
        }
        let metadata = gzip(
            serde_json::json!({ "name": meta.name, "attribution": meta.attribution, "type": "baselayer", "format": if meta.tile_type == 2 { "png" } else { "jpg" } })
                .to_string()
                .as_bytes(),
        )?;
        let clustered = self.entries.windows(2).all(|w| w[1].offset >= w[0].offset);

        let root_off = HEADER_LEN as u64;
        let meta_off = root_off + root.len() as u64;
        let leaf_off = meta_off + metadata.len() as u64;
        let data_off = leaf_off + leaves.len() as u64;

        let mut h = Vec::with_capacity(HEADER_LEN);
        h.extend_from_slice(b"PMTiles");
        h.push(3);
        for v in [root_off, root.len() as u64, meta_off, metadata.len() as u64, leaf_off, leaves.len() as u64, data_off, self.data_len, self.addressed, self.entries.len() as u64, contents] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.push(u8::from(clustered));
        h.push(2); // internal compression: gzip
        h.push(1); // tile compression: none
        h.push(meta.tile_type); // tile type: 2 png / 3 jpeg
        h.push(meta.min_zoom);
        h.push(meta.max_zoom);
        let e7 = |v: f64| ((v * 1e7).round() as i32).to_le_bytes();
        for v in [meta.bbox[0], meta.bbox[1], meta.bbox[2], meta.bbox[3]] {
            h.extend_from_slice(&e7(v));
        }
        h.push(((meta.min_zoom + meta.max_zoom) / 2).max(meta.min_zoom));
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
        if !keep_temps {
            let _ = std::fs::remove_file(&self.data_tmp);
            let _ = std::fs::remove_file(&self.index_tmp);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- minimal reader (verification)

struct Header {
    root_off: u64,
    root_len: u64,
    leaf_off: u64,
    data_off: u64,
    max_zoom: u8,
    tile_type: u8,
}

fn read_header(f: &mut File) -> anyhow::Result<Header> {
    let mut h = [0u8; HEADER_LEN];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut h).context("short PMTiles header")?;
    if &h[..7] != b"PMTiles" || h[7] != 3 {
        bail!("not a PMTiles v3 archive");
    }
    let u = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    Ok(Header { root_off: u(8), root_len: u(16), leaf_off: u(40), data_off: u(56), max_zoom: h[101], tile_type: h[99] })
}

fn read_at(f: &mut File, off: u64, len: u64) -> anyhow::Result<Vec<u8>> {
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b)?;
    Ok(b)
}

fn find_entry(entries: &[Entry], id: u64) -> Option<Entry> {
    let i = entries.partition_point(|e| e.tile_id <= id);
    if i == 0 {
        return None;
    }
    let e = entries[i - 1];
    if e.run_length == 0 || id < e.tile_id + u64::from(e.run_length) {
        Some(e)
    } else {
        None
    }
}

/// Tile bytes for (z, x, y) or `None` when the archive has no such tile.
pub fn read_tile(path: &Path, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let mut f = File::open(path)?;
    let h = read_header(&mut f)?;
    if z > h.max_zoom {
        return Ok(None);
    }
    let id = tile_id(z, x, y);
    let root = deserialize_dir(&gunzip(&read_at(&mut f, h.root_off, h.root_len)?)?)?;
    let Some(e) = find_entry(&root, id) else { return Ok(None) };
    let e = if e.run_length == 0 {
        let leaf = deserialize_dir(&gunzip(&read_at(&mut f, h.leaf_off + e.offset, u64::from(e.length))?)?)?;
        match find_entry(&leaf, id) {
            Some(e) if e.run_length > 0 => e,
            _ => return Ok(None),
        }
    } else {
        e
    };
    Ok(Some(read_at(&mut f, h.data_off + e.offset, u64::from(e.length))?))
}

/// Content filters (NetFree) answer image requests they have not inspected yet with one fixed
/// "sent for checking" picture. Real tiles at zoom <= 11 are never byte-identical, so any
/// content shared by several of them is such a placeholder.
const PLACEHOLDER_MAX_ZOOM: u8 = 11;
const PLACEHOLDER_SHARED: usize = 4;
pub(crate) const PLACEHOLDER_MSG: &str = "חלק מתמונות הלוויין הוחלפו בתמונת ביניים של סינון התוכן (NetFree: \"התמונה נשלחה לבדיקה\"). יש לאשר את האתר tiles.maps.eox.at בסינון, או להוריד בלי הסינון, ואז ללחוץ \"עדכן\" שוב.";

fn zoom_of(tile_id: u64) -> u8 {
    let mut z = 0u8;
    while tile_id >= ((1u64 << (2 * (z as u64 + 1))) - 1) / 3 {
        z += 1;
    }
    z
}

/// Does one data offset serve `PLACEHOLDER_SHARED` or more tiles of zoom <= 11?
fn has_placeholders(entries: &[Entry]) -> bool {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for e in entries.iter().filter(|e| e.run_length > 0 && zoom_of(e.tile_id) <= PLACEHOLDER_MAX_ZOOM) {
        let n = counts.entry(e.offset).or_insert(0);
        *n += 1;
        if *n >= PLACEHOLDER_SHARED {
            return true;
        }
    }
    false
}

fn all_entries(f: &mut File, h: &Header) -> anyhow::Result<Vec<Entry>> {
    let root = deserialize_dir(&gunzip(&read_at(f, h.root_off, h.root_len)?)?)?;
    let mut out = Vec::new();
    for e in root {
        if e.run_length == 0 {
            out.extend(deserialize_dir(&gunzip(&read_at(f, h.leaf_off + e.offset, u64::from(e.length))?)?)?);
        } else {
            out.push(e);
        }
    }
    Ok(out)
}

/// Is `b` a JPEG (`jpeg` = true) or a PNG?
fn has_signature(b: &[u8], jpeg: bool) -> bool {
    if jpeg {
        b.len() >= 4 && b[0] == 0xFF && b[1] == 0xD8
    } else {
        b.len() >= 8 && b[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    }
}

/// Header sanity, one real tile (Tel Aviv at zoom 10) carries the archive's image signature,
/// and – when `placeholders` – no content-filter placeholders among the low-zoom tiles.
pub(crate) fn verify(path: &Path, placeholders: bool) -> anyhow::Result<()> {
    let md = std::fs::metadata(path).context("raster archive missing")?;
    if md.len() < MIN_SIZE {
        bail!("raster archive too small ({} bytes)", md.len());
    }
    let mut f = File::open(path)?;
    let h = read_header(&mut f)?;
    let z = 10.min(h.max_zoom);
    let entries = all_entries(&mut f, &h)?;
    let t = match read_tile(path, z, lon_to_x(34.78, z), lat_to_y(32.08, z))? {
        Some(t) => t,
        None => {
            let e = entries.iter().find(|e| e.run_length > 0 && e.length > 0).ok_or_else(|| anyhow!("archive has no tiles"))?;
            read_at(&mut f, h.data_off + e.offset, u64::from(e.length))?
        }
    };
    if !has_signature(&t, h.tile_type != 2) {
        bail!("sample tile has the wrong image signature");
    }
    if placeholders && has_placeholders(&entries) {
        bail!("{PLACEHOLDER_MSG}");
    }
    Ok(())
}

// ---------------------------------------------------------------- download

fn fetch_tile(client: &Client, url: &str, jpeg: bool, cancel: &Cancel) -> anyhow::Result<Option<Vec<u8>>> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..4u32 {
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500 << attempt));
        }
        match client.get(url).send() {
            Ok(r) if r.status().is_success() => match r.bytes() {
                Ok(b) if has_signature(&b, jpeg) => return Ok(Some(b.to_vec())),
                Ok(_) => last = Some(anyhow!("{url}: not a {} (a content filter may be answering instead of the server)", if jpeg { "JPEG" } else { "PNG" })),
                Err(e) => last = Some(e.into()),
            },
            Ok(r) if r.status().as_u16() == 404 || r.status().as_u16() == 204 => return Ok(None),
            Ok(r) if r.status().as_u16() == 418 => bail!("{}", download::blocked_message(url)),
            Ok(r) if r.status().is_client_error() => bail!("HTTP {} for {url}", r.status()),
            Ok(r) => last = Some(anyhow!("HTTP {} for {url}", r.status())),
            Err(e) => last = Some(e.into()),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("download failed: {url}")))
}

/// What to fetch and how to store it, for one raster archive.
pub struct RasterSpec<'a> {
    /// Hebrew name used in progress messages ("תצלום לוויין")
    pub label: &'a str,
    pub url: &'a (dyn Fn(u8, u32, u32) -> String + Sync),
    pub max_zoom: u8,
    pub skip_sea: bool,
    /// PMTiles tile type: 2 = png, 3 = jpeg
    pub tile_type: u8,
    /// Detect content-filter placeholder pictures (only valid for imagery: flat DEM tiles are
    /// legitimately identical)
    pub placeholders: bool,
    pub name: String,
    pub attribution: String,
    pub bbox: [f64; 4],
}

/// Download every tile of `spec` into the archive at `dest` (atomically replaced at the end).
/// Resumable: temp data + index next to `dest` survive a failure / cancel for the same `version`.
/// Returns the number of tiles skipped because a content filter answered with its placeholder
/// picture (they stay unfetched; the next run picks them up).
pub fn download_raster(dest: &Path, version: &str, spec: &RasterSpec, client: &Client, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<usize> {
    let label = spec.label;
    let data_tmp = dest.with_extension("data.tmp");
    let out_tmp = dest.with_extension("pmtiles.tmp");
    let _ = std::fs::remove_file(&out_tmp);
    // Resume a previous run only if it was for the same version.
    let layer_tmp = dest.with_extension("layer.tmp");
    if std::fs::read_to_string(&layer_tmp).ok().as_deref() != Some(version) {
        let _ = std::fs::remove_file(&data_tmp);
        let _ = std::fs::remove_file(data_tmp.with_extension("index.tmp"));
    }
    std::fs::write(&layer_tmp, version)?;
    let mut writer = Writer::open(&data_tmp)?;
    let all = tiles_for_bbox(spec.bbox, spec.max_zoom, spec.skip_sea);
    let total = all.len() as u64;
    let tiles: Vec<(u8, u32, u32)> = all.iter().copied().filter(|&(z, x, y)| !writer.is_done(z, x, y)).collect();
    let already = total - tiles.len() as u64;
    progress(ProgressPhase::Downloading, already, total, &format!("{label} – {already}/{total} אריחים"));
    let jpeg = spec.tile_type != 2;

    let mut skipped = 0usize;
    let result = (|| -> anyhow::Result<()> {
        let next = AtomicUsize::new(0);
        let (tx, rx) = mpsc::sync_channel::<anyhow::Result<((u8, u32, u32), Option<Vec<u8>>)>>(WORKERS * 4);
        std::thread::scope(|s| -> anyhow::Result<()> {
            for _ in 0..WORKERS {
                let tx = tx.clone();
                let client = client.clone();
                let (tiles, next, cancel) = (&tiles, &next, cancel);
                let url = spec.url;
                s.spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(z, x, y)) = tiles.get(i) else { break };
                    cancel.wait_if_paused();
                    if cancel.is_cancelled() {
                        break;
                    }
                    let r = fetch_tile(&client, &url(z, x, y), jpeg, cancel).map(|b| ((z, x, y), b));
                    let failed = r.is_err();
                    if tx.send(r).is_err() || failed {
                        break;
                    }
                });
            }
            drop(tx);
            let started = Instant::now();
            let mut fetched = 0u64;
            let mut last_emit = Instant::now();
            let mut last_checkpoint = Instant::now();
            let mut result = Ok(());
            for r in rx {
                if cancel.is_cancelled() {
                    result = Err(anyhow!("cancelled"));
                    break;
                }
                let ((z, x, y), bytes) = match r {
                    Ok(v) => v,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                };
                if !writer.add(z, x, y, bytes.as_deref().unwrap_or(&[]))? {
                    skipped += 1;
                }
                fetched += 1;
                if spec.placeholders && z <= PLACEHOLDER_MAX_ZOOM && writer.has_placeholders() {
                    // the filter's "sent for checking" picture: forget those tiles, skip the rest
                    let n = writer.drop_placeholders()?;
                    skipped += n;
                    log::warn!("{label}: content-filter placeholder recognised, {n} tiles dropped; continuing");
                }
                if last_checkpoint.elapsed() >= Duration::from_secs(5) {
                    last_checkpoint = Instant::now();
                    writer.checkpoint()?;
                }
                let done = already + fetched;
                if done == total || last_emit.elapsed() >= Duration::from_millis(400) {
                    last_emit = Instant::now();
                    let rate = fetched as f64 / started.elapsed().as_secs_f64().max(0.001);
                    let eta = if fetched >= 50 && rate > 0.0 { ((total - done) as f64 / rate / 60.0).ceil() as u64 } else { 0 };
                    let eta_s = if eta > 0 { format!(" · נותרו כ-{eta} דקות") } else { String::new() };
                    let skip_s = if skipped > 0 { format!(" · {skipped} דולגו (סינון)") } else { String::new() };
                    progress(ProgressPhase::Downloading, done, total, &format!("{label} – {done}/{total} אריחים{eta_s}{skip_s}"));
                }
            }
            // keep what we have for the next attempt, then surface the error
            writer.checkpoint()?;
            result?;
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            if already + fetched != total {
                bail!("downloaded {} of {total} tiles", already + fetched);
            }
            Ok(())
        })?;
        progress(ProgressPhase::Finalizing, 0, 0, &format!("בונה את קובץ {label}"));
        let meta = ArchiveMeta { name: spec.name.clone(), attribution: spec.attribution.clone(), bbox: spec.bbox, min_zoom: 0, max_zoom: spec.max_zoom, tile_type: spec.tile_type };
        if skipped > 0 {
            progress(ProgressPhase::Finalizing, 0, 0, &format!("בונה את קובץ {label} ({skipped} אריחים חסרים בגלל סינון – יושלמו בעדכון הבא)"));
        }
        writer.finish(&out_tmp, &meta, skipped > 0)?;
        verify(&out_tmp, spec.placeholders)?;
        Ok(())
    })();
    if let Err(e) = result {
        // temp data + index stay on disk so the next run resumes
        let _ = std::fs::remove_file(&out_tmp);
        return Err(e);
    }
    atomic_replace(dest, &out_tmp)?;
    if skipped == 0 {
        let _ = std::fs::remove_file(&layer_tmp);
    }
    Ok(skipped)
}

pub fn update_satellite(
    paths: &DataPaths,
    progress: ProgressFn,
    cancel: &Cancel,
) -> anyhow::Result<Option<String>> {
    paths.ensure_dirs()?;
    progress(ProgressPhase::Processing, 0, 0, "מחפש את תצלום הלוויין העדכני (Sentinel-2)");
    let client = http_client()?;
    let layer = latest_layer(&client)?;

    let quality = crate::state::load_settings(paths).satellite_quality;
    let version = archive_version(&layer, &quality);
    let dest = paths.satellite_file();
    let m = super::manifest::Manifest::load(paths);
    if dest.exists() && version_matches(m.entry(crate::types::DataComponent::Satellite).and_then(|e| e.version.as_deref()), &layer, &quality) && verify(&dest, true).is_ok() {
        progress(ProgressPhase::Processing, 1, 1, "תצלום הלוויין כבר מעודכן");
        return Ok(Some(version));
    }
    let url = |z: u8, x: u32, y: u32| tile_url(&layer, z, x, y);
    let spec = RasterSpec {
        label: "תצלום לוויין",
        url: &url,
        max_zoom: quality_zoom(&quality),
        skip_sea: true,
        tile_type: 3,
        placeholders: true,
        name: format!("Sentinel-2 cloudless {}", &layer[layer.len() - 4..]),
        attribution: ATTRIBUTION.to_string(),
        bbox: BBOX_ISRAEL,
    };
    let skipped = download_raster(&dest, &version, &spec, &client, progress, cancel)?;
    // a partial archive gets a version that never equals the expected one, so "check updates"
    // keeps offering the download until every tile is in
    Ok(Some(if skipped > 0 { format!("{version} (חסרים {skipped} אריחים)") } else { version }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hilbert_ids_match_spec() {
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
        // all ids of a level are distinct and inside the level's range
        let mut ids: Vec<u64> = (0..8u32).flat_map(|x| (0..8u32).map(move |y| tile_id(3, x, y))).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 64);
        assert_eq!(ids[0], 21);
        assert_eq!(ids[63], 84);
    }

    #[test]
    fn bbox_tile_count() {
        let t = tiles_for_bbox(BBOX_ISRAEL, MAX_ZOOM, true);
        assert!(t.len() > 12_000 && t.len() < 22_782, "{}", t.len());
        assert_eq!(tiles_for_bbox(BBOX_ISRAEL, MAX_ZOOM, false).len(), 22_782);
        assert_eq!(t[0], (0, 0, 0));
        assert!(t.iter().all(|&(z, x, y)| x < (1 << z) && y < (1 << z)));
        // Tel Aviv beach, Haifa bay and Eilat are land; 25 km off Netanya is open sea
        let at = |lon: f64, lat: f64| (lon_to_x(lon, 14), lat_to_y(lat, 14));
        for (lon, lat) in [(34.76, 32.08), (35.02, 32.85), (34.95, 29.55), (34.27, 31.30)] {
            let (x, y) = at(lon, lat);
            assert!(!is_open_sea(14, x, y), "{lon},{lat}");
        }
        let (x, y) = at(34.55, 32.35);
        assert!(is_open_sea(14, x, y));
        assert!(!is_open_sea(11, x >> 3, y >> 3));
    }

    #[test]
    fn placeholder_tiles_are_detected() {
        let dir = std::env::temp_dir().join(format!("mo-sat-ph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data_tmp = dir.join("p.data.tmp");
        let mut w = Writer::create(&data_tmp).unwrap();
        let same = [0xFF, 0xD8, 1, 2, 3];
        for i in 0..3u32 {
            assert!(w.add(9, i, 0, &same).unwrap());
        }
        assert!(!w.has_placeholders(), "three shared low-zoom tiles are still tolerated");
        w.add(9, 3, 0, &same).unwrap();
        assert!(w.has_placeholders());
        // a real tile next to them survives the purge; the placeholder content is skipped afterwards
        w.add(9, 5, 0, &[0xFF, 0xD8, 9, 9, 9]).unwrap();
        assert_eq!(w.drop_placeholders().unwrap(), 4);
        assert!(!w.has_placeholders());
        assert!(!w.is_done(9, 0, 0) && w.is_done(9, 5, 0));
        assert!(!w.add(9, 6, 0, &same).unwrap(), "placeholder content is skipped");
        assert!(!w.is_done(9, 6, 0));
        w.checkpoint().unwrap();
        // the rewritten index forgets the dropped tiles
        let reopened = Writer::open(&data_tmp).unwrap();
        assert!(!reopened.is_done(9, 0, 0) && reopened.is_done(9, 5, 0));
        drop(reopened);
        let mut w = Writer::open(&data_tmp).unwrap();
        for i in 0..4u32 {
            w.add(9, i, 0, &same).unwrap();
        }
        assert!(w.has_placeholders());
        // shared content at high zoom (flat lake tiles) is fine
        let mut w2 = Writer::create(&dir.join("q.data.tmp")).unwrap();
        for i in 0..50u32 {
            w2.add(13, i, 0, &same).unwrap();
        }
        assert!(!w2.has_placeholders());
        assert_eq!(zoom_of(tile_id(9, 3, 0)), 9);
        assert_eq!(zoom_of(0), 0);
        let out = dir.join("p.pmtiles");
        let meta = ArchiveMeta { name: "p".into(), attribution: "a".into(), bbox: [0.0, 0.0, 1.0, 1.0], min_zoom: 9, max_zoom: 9, tile_type: 3 };
        w.finish(&out, &meta, false).unwrap();
        let mut f = File::open(&out).unwrap();
        let h = read_header(&mut f).unwrap();
        assert!(has_placeholders(&all_entries(&mut f, &h).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_keeps_stored_tiles() {
        let dir = std::env::temp_dir().join(format!("mo-sat-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data_tmp = dir.join("r.data.tmp");
        let mut w = Writer::create(&data_tmp).unwrap();
        w.add(5, 1, 2, &[0xFF, 0xD8, 1]).unwrap();
        w.add(5, 1, 3, &[]).unwrap();
        w.add(5, 2, 3, &[0xFF, 0xD8, 2, 2]).unwrap();
        w.checkpoint().unwrap();
        drop(w);
        // a torn write at the end of the data file is dropped on resume
        let idx = data_tmp.with_extension("index.tmp");
        let mut rec = [0u8; INDEX_REC];
        rec[0] = 5;
        rec[9..17].copy_from_slice(&7u64.to_le_bytes());
        rec[17..21].copy_from_slice(&9u32.to_le_bytes());
        std::fs::OpenOptions::new().append(true).open(&idx).unwrap().write_all(&rec).unwrap();
        let mut w = Writer::open(&data_tmp).unwrap();
        assert!(w.is_done(5, 1, 2) && w.is_done(5, 1, 3) && w.is_done(5, 2, 3));
        assert!(!w.is_done(5, 0, 0));
        assert_eq!(w.tiles(), 2);
        w.add(5, 0, 0, &[0xFF, 0xD8, 9]).unwrap();
        let out = dir.join("r.pmtiles");
        let meta = ArchiveMeta { name: "r".into(), attribution: "a".into(), bbox: [0.0, 0.0, 1.0, 1.0], min_zoom: 5, max_zoom: 5, tile_type: 3 };
        w.finish(&out, &meta, false).unwrap();
        assert_eq!(read_tile(&out, 5, 1, 2).unwrap(), Some(vec![0xFF, 0xD8, 1]));
        assert_eq!(read_tile(&out, 5, 2, 3).unwrap(), Some(vec![0xFF, 0xD8, 2, 2]));
        assert_eq!(read_tile(&out, 5, 0, 0).unwrap(), Some(vec![0xFF, 0xD8, 9]));
        assert_eq!(read_tile(&out, 5, 1, 3).unwrap(), None);
        assert!(!idx.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn varint_roundtrip() {
        for v in [0u64, 1, 127, 128, 300, 1 << 40, u64::MAX] {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            let mut p = 0;
            assert_eq!(get_varint(&b, &mut p).unwrap(), v);
            assert_eq!(p, b.len());
        }
    }

    #[test]
    fn archive_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mo-sat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data_tmp = dir.join("data.tmp");
        let out = dir.join("t.pmtiles");
        let mut w = Writer::create(&data_tmp).unwrap();
        w.root_max = 512; // force leaf directories
        // 5000 tiles so the directory needs leaves; every 7th tile shares bytes (dedup path)
        let tiles: Vec<(u8, u32, u32)> = tiles_for_bbox([34.5, 31.0, 35.5, 32.5], 13, true).into_iter().take(5000).collect();
        assert!(tiles.len() > 1000);
        let bytes_of = |i: usize, z: u8, x: u32, y: u32| -> Vec<u8> {
            if i % 7 == 0 {
                vec![0xFF, 0xD8, 7, 7, 7]
            } else {
                let mut b = vec![0xFF, 0xD8, z];
                b.extend_from_slice(&x.to_le_bytes());
                b.extend_from_slice(&y.to_le_bytes());
                b
            }
        };
        for (i, &(z, x, y)) in tiles.iter().enumerate().rev() {
            w.add(z, x, y, &bytes_of(i, z, x, y)).unwrap();
        }
        let meta = ArchiveMeta { name: "t".into(), attribution: "a".into(), bbox: [34.5, 31.0, 35.5, 32.5], min_zoom: 0, max_zoom: 13, tile_type: 3 };
        w.finish(&out, &meta, false).unwrap();
        assert!(!data_tmp.exists());
        for (i, &(z, x, y)) in tiles.iter().enumerate() {
            assert_eq!(read_tile(&out, z, x, y).unwrap().as_deref(), Some(bytes_of(i, z, x, y).as_slice()), "tile {z}/{x}/{y}");
        }
        assert_eq!(read_tile(&out, 13, 0, 0).unwrap(), None);
        assert_eq!(read_tile(&out, 14, 0, 0).unwrap(), None);
        let mut f = File::open(&out).unwrap();
        let h = read_header(&mut f).unwrap();
        assert_eq!(h.max_zoom, 13);
        assert!(h.leaf_off > h.root_off && h.data_off > h.leaf_off);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
