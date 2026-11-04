//! `mo-user` URI scheme: the user's own tile layers (Settings.customLayers).
//!
//! `http://mo-user.localhost/<layerId>/{z}/{x}/{y}` answers with
//! - `xyz`: the tile from the template URL (`{z}` `{x}` `{y}` `{-y}` `{s}`), cached on disk;
//! - `wms`: a 256 px EPSG:3857 GetMap of the tile's bounds, cached on disk;
//! - `pmtiles`: the tile of a local PMTiles archive (the program's reader in `map_tiles`);
//! - `mbtiles`: the tile of a local MBTiles (SQLite, TMS rows).
//!
//! Cached tiles live in `<data>/usercache/<layerId>/z/x/y.tile`; they are served for 30 days and
//! after that refreshed when the network answers (else the old copy is served).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::http::{header, Request, Response, StatusCode};

use crate::state::AppState;
use crate::types::CustomLayer;

const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_TILE_BYTES: usize = 8 * 1024 * 1024;

pub fn valid_id(id: &str) -> bool {
    crate::user_layers::valid_id(id)
}

fn parse(path: &str) -> Option<(String, u8, u32, u32)> {
    let mut it = path.trim_start_matches('/').split('/');
    let id = it.next()?.to_string();
    let z: u8 = it.next()?.parse().ok()?;
    let x: u32 = it.next()?.parse().ok()?;
    let y: u32 = it.next()?.split('.').next()?.parse().ok()?;
    (valid_id(&id) && z <= 24 && x < (1 << z) && y < (1 << z) && it.next().is_none()).then_some((id, z, x, y))
}

fn content_type(b: &[u8]) -> &'static str {
    match b {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, ..] => "image/jpeg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "application/x-protobuf",
    }
}

fn respond(status: StatusCode, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut b = Response::builder().status(status).header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if !body.is_empty() {
        let ct = content_type(&body);
        b = b.header(header::CONTENT_TYPE, ct);
        if ct == "application/x-protobuf" && body.starts_with(&[0x1f, 0x8b]) {
            b = b.header(header::CONTENT_ENCODING, "gzip");
        }
    }
    b.body(body).unwrap()
}

/// Web Mercator bounds of a tile in metres.
fn merc_bounds(z: u8, x: u32, y: u32) -> [f64; 4] {
    const R: f64 = 20_037_508.342_789_244;
    let n = f64::from(1u32 << z);
    let size = 2.0 * R / n;
    let minx = -R + f64::from(x) * size;
    let maxy = R - f64::from(y) * size;
    [minx, maxy - size, minx + size, maxy]
}

/// The URL of one tile of an online layer.
pub fn tile_url(l: &CustomLayer, z: u8, x: u32, y: u32) -> Option<String> {
    match l.kind.as_str() {
        "xyz" => {
            let tms_y = (1u32 << z) - 1 - y;
            let sub = ["a", "b", "c"][((x + y) % 3) as usize];
            Some(l.url.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{-y}", &tms_y.to_string()).replace("{y}", &y.to_string()).replace("{s}", sub))
        }
        "wms" => {
            let b = merc_bounds(z, x, y);
            let sep = if l.url.contains('?') { if l.url.ends_with('?') || l.url.ends_with('&') { "" } else { "&" } } else { "?" };
            let layers = l.wms_layers.clone().unwrap_or_default();
            Some(format!(
                "{}{sep}SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS={}&STYLES=&CRS=EPSG:3857&BBOX={:.3},{:.3},{:.3},{:.3}&WIDTH=256&HEIGHT=256&FORMAT=image/png&TRANSPARENT=TRUE",
                l.url,
                urlencode(&layers),
                b[0],
                b[1],
                b[2],
                b[3]
            ))
        }
        _ => None,
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(crate::data::download::USER_AGENT)
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(12))
            .build()
            .expect("http client")
    })
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let r = client().get(url).send().ok()?;
    if !r.status().is_success() {
        return None;
    }
    let b = r.bytes().ok()?;
    (!b.is_empty() && b.len() <= MAX_TILE_BYTES).then(|| b.to_vec())
}

fn cache_file(root: &Path, id: &str, z: u8, x: u32, y: u32) -> PathBuf {
    root.join(id).join(z.to_string()).join(x.to_string()).join(format!("{y}.tile"))
}

fn online_tile(st: &AppState, l: &CustomLayer, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let file = cache_file(&st.paths().usercache_dir(), &l.id, z, x, y);
    let cached = std::fs::read(&file).ok();
    let fresh = std::fs::metadata(&file).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age < MAX_AGE);
    if cached.is_some() && fresh {
        return cached;
    }
    match tile_url(l, z, x, y).and_then(|u| fetch(&u)) {
        Some(b) => {
            if let Some(dir) = file.parent() {
                if std::fs::create_dir_all(dir).is_ok() {
                    let tmp = file.with_extension("tmp");
                    if std::fs::write(&tmp, &b).is_ok() {
                        let _ = std::fs::rename(&tmp, &file);
                    }
                }
            }
            Some(b)
        }
        None => cached, // offline / server error: the old copy is better than nothing
    }
}

fn mbtiles_tile(path: &Path, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    static CONNS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<rusqlite::Connection>>>>> = OnceLock::new();
    let conns = CONNS.get_or_init(Default::default);
    let conn = {
        let mut g = conns.lock();
        if let Some(c) = g.get(path) {
            c.clone()
        } else {
            let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
            let c = Arc::new(Mutex::new(c));
            g.insert(path.to_path_buf(), c.clone());
            c
        }
    };
    let row = (1u32 << z) - 1 - y; // MBTiles rows are TMS (south up)
    let conn = conn.lock();
    conn.query_row("SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3", rusqlite::params![z, x, row], |r| r.get::<_, Vec<u8>>(0)).ok()
}

/// Handler of the `mo-user` URI scheme.
pub fn serve(st: Arc<AppState>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some((id, z, x, y)) = parse(request.uri().path()) else { return respond(StatusCode::NOT_FOUND, Vec::new()) };
    let layer = st.settings.read().custom_layers.iter().find(|l| l.id == id).cloned();
    let Some(l) = layer else { return respond(StatusCode::NOT_FOUND, Vec::new()) };
    if l.minzoom.is_some_and(|m| z < m) || l.maxzoom.is_some_and(|m| z > m) {
        return respond(StatusCode::NOT_FOUND, Vec::new());
    }
    let tile = match l.kind.as_str() {
        "xyz" | "wms" => online_tile(&st, &l, z, x, y),
        "pmtiles" => crate::map_tiles::archive_tile(Path::new(&l.url), z, x, y),
        "mbtiles" => mbtiles_tile(Path::new(&l.url), z, x, y),
        _ => None,
    };
    match tile {
        Some(b) => respond(StatusCode::OK, b),
        None => respond(StatusCode::NOT_FOUND, Vec::new()),
    }
}

/// Does this layer read local files (served from the local pool) or the network?
pub fn is_local(st: &AppState, path: &str) -> bool {
    parse(path).is_some_and(|(id, ..)| st.settings.read().custom_layers.iter().any(|l| l.id == id && matches!(l.kind.as_str(), "pmtiles" | "mbtiles")))
}

/// Delete the disk cache of a layer (after it was removed or its URL changed).
pub fn clear_cache(paths: &crate::paths::DataPaths, id: &str) {
    if valid_id(id) {
        let _ = std::fs::remove_dir_all(paths.usercache_dir().join(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(kind: &str, url: &str) -> CustomLayer {
        CustomLayer { id: "a1".into(), name: "x".into(), kind: kind.into(), url: url.into(), wms_layers: Some("roads,rivers".into()), opacity: 1.0, visible: true, minzoom: None, maxzoom: None, attribution: None }
    }

    #[test]
    fn parses_paths() {
        assert_eq!(parse("/a1/3/2/1.png"), Some(("a1".into(), 3, 2, 1)));
        assert_eq!(parse("/../3/2/1"), None);
        assert_eq!(parse("/a1/3/9/1"), None);
        assert_eq!(parse("/a1/3/2/1/extra"), None);
    }

    #[test]
    fn builds_urls() {
        let l = layer("xyz", "https://{s}.tile.example/{z}/{x}/{y}.png?tms={-y}");
        assert_eq!(tile_url(&l, 2, 1, 0).unwrap(), "https://b.tile.example/2/1/0.png?tms=3");
        let w = layer("wms", "https://srv/wms?map=x");
        let u = tile_url(&w, 0, 0, 0).unwrap();
        assert!(u.starts_with("https://srv/wms?map=x&SERVICE=WMS"), "{u}");
        assert!(u.contains("LAYERS=roads,rivers") && u.contains("CRS=EPSG:3857") && u.contains("BBOX=-20037508.343,-20037508.343,20037508.343,20037508.343"), "{u}");
    }

    #[test]
    fn mbtiles_rows_are_tms() {
        let dir = std::env::temp_dir().join(format!("mo-mbtiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.mbtiles");
        let _ = std::fs::remove_file(&f);
        let c = rusqlite::Connection::open(&f).unwrap();
        c.execute_batch("CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);").unwrap();
        c.execute("INSERT INTO tiles VALUES (1, 0, 1, x'89504e47')", []).unwrap();
        drop(c);
        // XYZ y = 0 at z1 is TMS row 1
        assert_eq!(mbtiles_tile(&f, 1, 0, 0), Some(vec![0x89, b'P', b'N', b'G']));
        assert_eq!(mbtiles_tile(&f, 1, 0, 1), None);
    }
}
