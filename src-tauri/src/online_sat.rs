//! The satellite layer of the map: `http://mo-sat.localhost/{z}/{x}/{y}`.
//!
//! Each tile comes from, in order:
//! 0. a downloaded sharp area (`satellite/areas/<id>.pmtiles`, the smallest one holding it);
//! 1. the downloaded archive (`satellite/israel-s2.pmtiles`) when it covers that zoom;
//! 2. the live cache (`satellite/live/<z>/<x>/<y>.jpg|png|none`): tiles rendered earlier;
//! 3. when online: rendered now from the govmap orthophoto (zoom 10–19 inside Israel, ~0.13 m)
//!    or Sentinel-2 (zoom ≤ 14, anywhere) — and stored in the live cache.
//!
//! So with internet the map is as sharp as the orthophoto allows, without any download, and
//! what was seen once keeps working offline. Requests to govmap are paced (see
//! `orthophoto::Source::interactive`); after a network error the live part rests for 20 s so an
//! offline machine answers at once.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::http::{header, Request, Response, StatusCode};

use crate::data::orthophoto::{self, Source};
use crate::data::satellite;
use crate::data::Cancel;
use crate::state::AppState;

/// Deepest zoom served (the orthophoto's finest level is ~0.13 m/px ≈ zoom 19 in Israel).
pub const MAX_ZOOM: u8 = 19;
const EOX_MAX_ZOOM: u8 = 14;
const OFFLINE_REST_SECS: u64 = 20;
/// The live cache is trimmed (oldest tiles first) to `CACHE_KEEP` once it grows past `CACHE_MAX`.
const CACHE_MAX: u64 = 2 * 1024 * 1024 * 1024;
const CACHE_KEEP: u64 = 1600 * 1024 * 1024;
/// Check the cache size at the first store and then every this many stores.
const TRIM_EVERY: u64 = 2000;
static STORED: AtomicU64 = AtomicU64::new(0);
static TRIMMING: AtomicBool = AtomicBool::new(false);

static SOURCE: OnceLock<Option<Source>> = OnceLock::new();
static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
/// Latest Sentinel-2 layer name once known, else when to ask again (unix seconds): a first probe
/// made offline must not hide the layer for the whole session.
static EOX_LAYER: Mutex<(Option<String>, u64)> = Mutex::new((None, 0));
const EOX_RETRY_SECS: u64 = 300;
/// Live tiles are for a map on screen: give up quickly so an offline machine answers at once
/// (the download client waits 30 s for a connection).
const LIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const LIVE_TIMEOUT: Duration = Duration::from_secs(8);
/// Unix seconds until which the live part is skipped (after a network error).
static REST_UNTIL: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn live_client() -> Option<reqwest::blocking::Client> {
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .user_agent(crate::data::download::USER_AGENT)
                .connect_timeout(LIVE_CONNECT_TIMEOUT)
                .timeout(LIVE_TIMEOUT)
                .build()
                .ok()
        })
        .clone()
}

fn source() -> Option<&'static Source> {
    SOURCE.get_or_init(|| live_client().map(Source::interactive)).as_ref()
}

fn eox_layer() -> Option<String> {
    let mut g = EOX_LAYER.lock().unwrap_or_else(|e| e.into_inner());
    if g.0.is_none() && now_secs() >= g.1 {
        g.0 = live_client().and_then(|c| satellite::latest_layer(&c).ok());
        if g.0.is_none() {
            g.1 = now_secs() + EOX_RETRY_SECS;
        }
    }
    g.0.clone()
}

/// No network (refused, unreachable, timed out) or a content filter blocking the host: worth a
/// rest of the live part. A rate-limit pause is handled by the pacer and is not one of these.
fn is_offline_error(e: &anyhow::Error) -> bool {
    // the content-filter message names the host first; compare the text after it
    let blocked = crate::data::download::blocked_message("http://x/");
    let blocked_tail = blocked.split_once(" x ").map_or("", |t| t.1);
    e.chain().any(|c| c.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_connect() || r.is_timeout() || r.is_request()))
        || (!blocked_tail.is_empty() && format!("{e:#}").contains(blocked_tail))
}

/// Is the downloaded archive the govmap orthophoto? Read from manifest.json, which is parsed
/// again only when it changed (this runs for every tile on screen).
fn local_is_ortho(paths: &crate::paths::DataPaths) -> bool {
    static CACHE: Mutex<Option<(PathBuf, Option<SystemTime>, bool)>> = Mutex::new(None);
    let file = paths.manifest_file();
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((f, t, v)) = g.as_ref() {
        if *f == file && *t == mtime {
            return *v;
        }
    }
    let v = crate::data::manifest::Manifest::load(paths)
        .entry(crate::types::DataComponent::Satellite)
        .and_then(|e| e.version.as_deref())
        .is_some_and(orthophoto::is_govmap_version);
    *g = Some((file, mtime, v));
    v
}

fn cache_base(dir: &Path, z: u8, x: u32, y: u32) -> PathBuf {
    dir.join(z.to_string()).join(x.to_string()).join(y.to_string())
}

enum Hit {
    Tile(Vec<u8>),
    Empty,
}

fn cached(base: &Path) -> Option<Hit> {
    for ext in ["jpg", "png"] {
        if let Ok(b) = std::fs::read(base.with_extension(ext)) {
            return Some(Hit::Tile(b));
        }
    }
    base.with_extension("none").exists().then_some(Hit::Empty)
}

fn store(base: &Path, tile: Option<&[u8]>) {
    let Some(parent) = base.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let (path, bytes): (PathBuf, &[u8]) = match tile {
        Some(b) if b.first() == Some(&0x89) => (base.with_extension("png"), b),
        Some(b) => (base.with_extension("jpg"), b),
        None => (base.with_extension("none"), &[]),
    };
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Every [`TRIM_EVERY`] stores: in the background, delete the oldest cached tiles while the
/// cache is over [`CACHE_MAX`].
fn maybe_trim(dir: &Path) {
    if !STORED.fetch_add(1, Ordering::Relaxed).is_multiple_of(TRIM_EVERY) || TRIMMING.swap(true, Ordering::AcqRel) {
        return;
    }
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        let n = trim(&dir, CACHE_MAX, CACHE_KEEP);
        if n > 0 {
            log::info!("live satellite cache: removed {n} old tiles");
        }
        TRIMMING.store(false, Ordering::Release);
    });
}

/// Delete the oldest files under `dir` until at most `keep` bytes remain, when over `max`.
fn trim(dir: &Path, max: u64, keep: u64) -> usize {
    fn walk(d: &Path, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(&e.path(), out);
            } else {
                out.push((md.modified().unwrap_or(UNIX_EPOCH), md.len().max(1), e.path()));
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max {
        return 0;
    }
    files.sort_by_key(|f| f.0);
    let mut removed = 0;
    for (_, len, p) in files {
        if total <= keep {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    removed
}

/// Fetch / render one tile now. `Err` = temporary (network, rate limit): nothing is cached.
fn live(govmap: bool, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let cancel = Cancel::default();
    let src = source().ok_or_else(|| anyhow::anyhow!("no http client"))?;
    if govmap && z >= 10 && orthophoto::wanted(z, x, y) {
        if let Some(t) = orthophoto::render(src, z, x, y, &cancel)? {
            return Ok(Some(t));
        }
    }
    if z <= EOX_MAX_ZOOM {
        if let Some(layer) = eox_layer() {
            let client = live_client().ok_or_else(|| anyhow::anyhow!("no http client"))?;
            // one attempt: the map asks again when the tile is needed again
            return satellite::fetch_tile_attempts(&client, &satellite::tile_url(&layer, z, x, y), true, &cancel, 1);
        }
        if govmap && orthophoto::wanted(z, x, y) {
            return orthophoto::render(src, z, x, y, &cancel);
        }
    }
    Ok(None)
}

fn tile(state: &AppState, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let paths = state.paths();
    // 0. a sharp area downloaded for this place (the smallest one that holds the tile)
    {
        let areas = state.settings.read().satellite_areas.clone();
        if !areas.is_empty() {
            if let Some(t) = crate::data::sat_areas::tile(&paths, &areas, z, x, y) {
                return Some(t);
            }
        }
    }
    let local = paths.satellite_file();
    let govmap = state.settings.read().satellite_source != "sentinel";
    // a downloaded Sentinel-2 archive is blurry next to the orthophoto: with govmap chosen it
    // only serves the low zooms (and stands in when the live tile cannot be had)
    let local_ortho = local_is_ortho(&paths);
    let local_first = !govmap || local_ortho || z <= orthophoto::EOX_MAX_ZOOM || !orthophoto::wanted(z, x, y);
    let from_local = || -> Option<Vec<u8>> {
        if local.exists() && crate::map_tiles::archive_max_zoom(&local).is_some_and(|m| z <= m) {
            crate::map_tiles::archive_tile(&local, z, x, y)
        } else {
            None
        }
    };
    if local_first {
        if let Some(t) = from_local() {
            return Some(t);
        }
    }
    let base = cache_base(&paths.satellite_dir().join("live"), z, x, y);
    match cached(&base) {
        Some(Hit::Tile(b)) => return Some(b),
        Some(Hit::Empty) => return None,
        None => {}
    }
    if now_secs() < REST_UNTIL.load(Ordering::Relaxed) {
        return if local_first { None } else { from_local() };
    }
    match live(govmap, z, x, y) {
        Ok(Some(t)) => {
            store(&base, Some(&t));
            maybe_trim(&paths.satellite_dir().join("live"));
            Some(t)
        }
        Ok(None) => {
            if local_first {
                store(&base, None);
                None
            } else {
                from_local()
            }
        }
        Err(e) => {
            log::debug!("live satellite {z}/{x}/{y}: {e:#}");
            // Rest only when the network is the problem. (The old test, "the message does not
            // mention govmap", never fired: every reqwest error carries the govmap URL.)
            if is_offline_error(&e) {
                REST_UNTIL.store(now_secs() + OFFLINE_REST_SECS, Ordering::Relaxed);
            }
            if local_first {
                None
            } else {
                from_local()
            }
        }
    }
}

fn parse(path: &str) -> Option<(u8, u32, u32)> {
    let mut it = path.trim_start_matches('/').split('/');
    let z: u8 = it.next()?.parse().ok()?;
    let x: u32 = it.next()?.parse().ok()?;
    let y: u32 = it.next()?.split('.').next()?.parse().ok()?;
    (z <= MAX_ZOOM && x < (1 << z) && y < (1 << z)).then_some((z, x, y))
}

/// Handler of the `mo-sat` URI scheme.
pub fn serve(state: Arc<AppState>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let found = parse(request.uri().path()).and_then(|(z, x, y)| tile(&state, z, x, y));
    let b = Response::builder().header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    match found {
        Some(t) => {
            // the CI satellite package is WebP; local and live tiles are JPEG (or PNG)
            let ct = match crate::data::satellite::image_kind(&t) {
                Some(2) => "image/png",
                Some(4) => "image/webp",
                _ => "image/jpeg",
            };
            b.status(StatusCode::OK).header(header::CONTENT_TYPE, ct).body(t).unwrap()
        }
        // no picture here (sea outside coverage, offline): the map shows what is underneath
        None => b.status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_errors_are_recognised() {
        // a refused connection on a closed local port is a real reqwest connect error
        let c = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(2)).build().unwrap();
        let e: anyhow::Error = c.get("http://127.0.0.1:9/x").send().unwrap_err().into();
        assert!(is_offline_error(&e.context("govmap tile")));
        let rate = anyhow::anyhow!("{}", orthophoto::RATE_LIMITED_MSG);
        assert!(!is_offline_error(&rate));
        let blocked = anyhow::anyhow!("{}", crate::data::download::blocked_message("https://cdn.govmap.gov.il/a.jpg"));
        assert!(is_offline_error(&blocked));
        assert!(!is_offline_error(&anyhow::anyhow!("HTTP 500 for https://cdn.govmap.gov.il/a.jpg")));
    }

    #[test]
    fn parses_paths() {
        assert_eq!(parse("/19/1/2"), Some((19, 1, 2)));
        assert_eq!(parse("/20/1/2"), None);
        assert_eq!(parse("/2/4/0"), None);
    }

    #[test]
    fn trims_oldest_first() {
        let d = std::env::temp_dir().join(format!("mo-sat-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for i in 0..5u32 {
            let p = d.join(i.to_string()).join("t.jpg");
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let t = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + u64::from(i));
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        assert_eq!(trim(&d, 1000, 300), 0, "under the limit: nothing removed");
        assert_eq!(trim(&d, 400, 300), 2);
        assert!(!d.join("0/t.jpg").exists() && !d.join("1/t.jpg").exists() && d.join("4/t.jpg").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
