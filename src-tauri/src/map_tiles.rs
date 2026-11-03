//! One vector-tile source over every installed detailed map: Israel (`tiles/israel.pmtiles`) and
//! the downloaded countries (`regions/<id>/map.pmtiles`). The frontend asks
//! `http://mo-map.localhost/{z}/{x}/{y}.mvt`; the first archive that has the tile answers.
//!
//! Extracts copy whole (unclipped) planet tiles, so a tile on a border is identical in every
//! archive that has it and "first hit wins" never cuts features. Files are opened per request
//! (no handle stays open, so updates can replace them); headers and directories are cached by
//! (path, length, mtime).
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use tauri::http::{header, Request, Response, StatusCode};

use crate::data::satellite::{deserialize_dir, find_entry, gunzip, tile_id, Entry};

struct Archive {
    stamp: (u64, SystemTime),
    root: Vec<Entry>,
    leaf_off: u64,
    data_off: u64,
    min_zoom: u8,
    max_zoom: u8,
    gzip_tiles: bool,
    gzip_dirs: bool,
    /// [min_lon, min_lat, max_lon, max_lat]
    bounds: [f64; 4],
    leaves: Mutex<HashMap<u64, Arc<Vec<Entry>>>>,
}

const MAX_LEAVES: usize = 512;

fn read_at(f: &mut File, off: u64, len: u64) -> anyhow::Result<Vec<u8>> {
    let mut b = vec![0u8; len as usize];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b)?;
    Ok(b)
}

impl Archive {
    fn open(path: &Path, stamp: (u64, SystemTime)) -> anyhow::Result<Self> {
        let mut f = File::open(path)?;
        let h = read_at(&mut f, 0, 127)?;
        if &h[..7] != b"PMTiles" || h[7] != 3 {
            anyhow::bail!("{} is not a PMTiles v3 archive", path.display());
        }
        let u = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
        let i = |o: usize| f64::from(i32::from_le_bytes(h[o..o + 4].try_into().unwrap())) / 1e7;
        let gzip_dirs = h[97] == 2;
        let raw = read_at(&mut f, u(8), u(16))?;
        let root = deserialize_dir(&if gzip_dirs { gunzip(&raw)? } else { raw })?;
        Ok(Self {
            stamp,
            root,
            leaf_off: u(40),
            data_off: u(56),
            gzip_dirs,
            gzip_tiles: h[98] == 2,
            min_zoom: h[100],
            max_zoom: h[101],
            bounds: [i(102), i(106), i(110), i(114)],
            leaves: Mutex::new(HashMap::new()),
        })
    }

    fn tile(&self, path: &Path, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
        if z < self.min_zoom || z > self.max_zoom || !self.covers(z, x, y) {
            return Ok(None);
        }
        let id = tile_id(z, x, y);
        let Some(mut e) = find_entry(&self.root, id) else { return Ok(None) };
        let mut f = File::open(path)?;
        if e.run_length == 0 {
            let leaf = self.leaf(&mut f, e.offset, e.length)?;
            match find_entry(&leaf, id) {
                Some(l) if l.run_length > 0 => e = l,
                _ => return Ok(None),
            }
        }
        let b = read_at(&mut f, self.data_off + e.offset, u64::from(e.length))?;
        Ok(Some(if self.gzip_tiles { gunzip(&b)? } else { b }))
    }

    fn leaf(&self, f: &mut File, off: u64, len: u32) -> anyhow::Result<Arc<Vec<Entry>>> {
        if let Some(l) = self.leaves.lock().unwrap().get(&off) {
            return Ok(l.clone());
        }
        let raw = read_at(f, self.leaf_off + off, u64::from(len))?;
        let l = Arc::new(deserialize_dir(&if self.gzip_dirs { gunzip(&raw)? } else { raw })?);
        let mut g = self.leaves.lock().unwrap();
        if g.len() >= MAX_LEAVES {
            g.clear();
        }
        g.insert(off, l.clone());
        Ok(l)
    }

    fn covers(&self, z: u8, x: u32, y: u32) -> bool {
        let n = f64::from(1u32 << z);
        let w = f64::from(x) / n * 360.0 - 180.0;
        let e = f64::from(x + 1) / n * 360.0 - 180.0;
        let lat = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).sinh().atan().to_degrees();
        let (north, south) = (lat(f64::from(y)), lat(f64::from(y + 1)));
        let b = self.bounds;
        !(e < b[0] || w > b[2] || north < b[1] || south > b[3])
    }
}

/// Archives in lookup order. Set by the app whenever the installed maps change.
static FILES: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());
static CACHE: Mutex<Option<HashMap<PathBuf, Arc<Archive>>>> = Mutex::new(None);

/// Replace the list of archives (Israel first, then the countries).
pub fn set_files(files: Vec<PathBuf>) {
    *FILES.write().unwrap() = files;
}

pub fn has_files() -> bool {
    !FILES.read().unwrap().is_empty()
}

fn archive(path: &Path) -> Option<Arc<Archive>> {
    let md = std::fs::metadata(path).ok()?;
    let stamp = (md.len(), md.modified().ok()?);
    let mut g = CACHE.lock().unwrap();
    let cache = g.get_or_insert_with(HashMap::new);
    if let Some(a) = cache.get(path) {
        if a.stamp == stamp {
            return Some(a.clone());
        }
    }
    match Archive::open(path, stamp) {
        Ok(a) => {
            let a = Arc::new(a);
            cache.insert(path.to_path_buf(), a.clone());
            Some(a)
        }
        Err(e) => {
            log::warn!("map archive {}: {e:#}", path.display());
            cache.remove(path);
            None
        }
    }
}

/// One tile of any PMTiles archive (cached header / directories), e.g. the satellite archive.
pub fn archive_tile(path: &Path, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    archive(path)?.tile(path, z, x, y).ok().flatten()
}

/// Max zoom of an archive.
pub fn archive_max_zoom(path: &Path) -> Option<u8> {
    archive(path).map(|a| a.max_zoom)
}

/// The first installed archive's tile, if any.
pub fn tile(z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let files = FILES.read().unwrap().clone();
    for p in &files {
        let Some(a) = archive(p) else { continue };
        match a.tile(p, z, x, y) {
            Ok(Some(t)) => return Some(t),
            Ok(None) => {}
            Err(e) => log::debug!("tile {z}/{x}/{y} in {}: {e:#}", p.display()),
        }
    }
    None
}

fn parse(path: &str) -> Option<(u8, u32, u32)> {
    let mut it = path.trim_start_matches('/').split('/');
    let z: u8 = it.next()?.parse().ok()?;
    let x: u32 = it.next()?.parse().ok()?;
    let y: u32 = it.next()?.split('.').next()?.parse().ok()?;
    (z <= 24 && x < (1 << z) && y < (1 << z)).then_some((z, x, y))
}

/// Handler of the `mo-map` URI scheme.
pub fn serve(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let resp = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/x-protobuf")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(body)
            .unwrap()
    };
    match parse(request.uri().path()) {
        // a missing tile is an empty (valid) vector tile
        Some((z, x, y)) => resp(StatusCode::OK, tile(z, x, y).unwrap_or_default()),
        None => resp(StatusCode::NOT_FOUND, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paths() {
        assert_eq!(parse("/12/2446/1670.mvt"), Some((12, 2446, 1670)));
        assert_eq!(parse("/0/0/0"), Some((0, 0, 0)));
        assert_eq!(parse("/3/8/0.mvt"), None);
        assert_eq!(parse("/a/b/c"), None);
    }
}

#[cfg(test)]
mod live {
    /// `MAP_FILE=<pmtiles> cargo test --lib map_tiles::live -- --ignored` reads a few tiles.
    #[test]
    #[ignore]
    fn reads_installed_archive() {
        let f = std::path::PathBuf::from(std::env::var("MAP_FILE").expect("MAP_FILE"));
        super::set_files(vec![f]);
        for (z, lon, lat) in [(0u8, 33.0f64, 34.7f64), (6, 33.0, 34.7), (10, 33.03, 34.68), (14, 33.04, 34.67), (14, 2.35, 48.85)] {
            let n = f64::from(1u32 << z);
            let x = ((lon + 180.0) / 360.0 * n) as u32;
            let y = ((1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n) as u32;
            let t0 = std::time::Instant::now();
            let t = super::tile(z, x, y);
            eprintln!("{z}/{x}/{y}: {:?} bytes in {:?}", t.as_ref().map(|b| b.len()), t0.elapsed());
        }
    }
}
