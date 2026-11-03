//! The satellite layer of the map: `http://mo-sat.localhost/{z}/{x}/{y}`.
//!
//! Each tile comes from, in order:
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::http::{header, Request, Response, StatusCode};

use crate::data::orthophoto::{self, Source};
use crate::data::satellite;
use crate::data::Cancel;
use crate::state::AppState;

/// Deepest zoom served (the orthophoto's finest level is ~0.13 m/px ≈ zoom 19 in Israel).
pub const MAX_ZOOM: u8 = 19;
const EOX_MAX_ZOOM: u8 = 14;
const OFFLINE_REST_SECS: u64 = 20;

static SOURCE: OnceLock<Option<Source>> = OnceLock::new();
static EOX_LAYER: OnceLock<Option<String>> = OnceLock::new();
/// Unix seconds until which the live part is skipped (after a network error).
static REST_UNTIL: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn source() -> Option<&'static Source> {
    SOURCE.get_or_init(|| crate::data::download::http_client().ok().map(Source::interactive)).as_ref()
}

fn eox_layer() -> Option<&'static str> {
    EOX_LAYER
        .get_or_init(|| {
            let c = crate::data::download::http_client().ok()?;
            satellite::latest_layer(&c).ok()
        })
        .as_deref()
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
            let client = crate::data::download::http_client()?;
            return satellite::fetch_tile(&client, &satellite::tile_url(layer, z, x, y), true, &cancel);
        }
        if govmap && orthophoto::wanted(z, x, y) {
            return orthophoto::render(src, z, x, y, &cancel);
        }
    }
    Ok(None)
}

fn tile(state: &AppState, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let paths = state.paths();
    let local = paths.satellite_file();
    let govmap = state.settings.read().satellite_source != "sentinel";
    // a downloaded Sentinel-2 archive is blurry next to the orthophoto: with govmap chosen it
    // only serves the low zooms (and stands in when the live tile cannot be had)
    let local_ortho = crate::data::manifest::Manifest::load(&paths)
        .entry(crate::types::DataComponent::Satellite)
        .and_then(|e| e.version.as_deref())
        .is_some_and(orthophoto::is_govmap_version);
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
            let msg = format!("{e:#}");
            log::debug!("live satellite {z}/{x}/{y}: {msg}");
            // a rate-limit pause is handled by the pacer; anything else looks like no network
            if !msg.contains("govmap") {
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
            let ct = if t.first() == Some(&0x89) { "image/png" } else { "image/jpeg" };
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
    fn parses_paths() {
        assert_eq!(parse("/19/1/2"), Some((19, 1, 2)));
        assert_eq!(parse("/20/1/2"), None);
        assert_eq!(parse("/2/4/0"), None);
    }
}
