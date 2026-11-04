//! Sharp aerial imagery for chosen areas (Settings → satellite → sharp areas).
//!
//! The country-wide archive stays at a moderate zoom (quick to download); each area gets its own
//! PMTiles archive `satellite/areas/<id>.pmtiles` with the govmap orthophoto from just above the
//! country archive's zoom up to the area's zoom (16 sharp, 17 very sharp, 18 sharpest). The `mo-sat`
//! scheme looks in the area archives first (the smallest area containing the tile), then in the
//! country archive, the live cache and online (see `online_sat.rs`).
//!
//! The govmap CDN allows ~8–12 requests per second: only one govmap download runs at a time
//! (country archive or area, [`govmap_turn`]), the pacing in `orthophoto::Source` is never
//! exceeded, and the tiles are for personal use only (never part of a shared package).
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context};
use parking_lot::{Mutex, MutexGuard};

use super::orthophoto as ortho;
use super::satellite::{self, download_raster, RasterSpec};
use super::Cancel;
use crate::paths::DataPaths;
use crate::types::{ProgressPhase, SatelliteArea, SatelliteAreaEstimate, SatelliteAreaProgress, SatelliteAreaStatus};

/// Largest area (km²) of one sharp area; larger ones would take days at the govmap pacing.
pub const MAX_KM2: f64 = 2500.0;
/// Zooms an area may go up to.
pub const MIN_AREA_ZOOM: u8 = 16;
pub const MAX_AREA_ZOOM: u8 = 18;
/// Average size of one rendered orthophoto tile (JPEG q85, 256 px), measured on city tiles.
const AVG_TILE_BYTES: u64 = 24_000;
/// Requests per second the govmap pacing settles at (between its 8 and 12).
const PACE_PER_SEC: f64 = 10.0;
/// Without a country orthophoto archive an area starts here (zoom 10 = the first orthophoto zoom).
const DEFAULT_MIN_ZOOM: u8 = 10;

static GOVMAP: Mutex<()> = Mutex::new(());

/// Wait (cancellable) until no other govmap download runs; `on_wait` is called once when it must wait.
pub fn govmap_turn<'a>(on_wait: &dyn Fn(), cancel: &Cancel) -> anyhow::Result<MutexGuard<'a, ()>> {
    if let Some(g) = GOVMAP.try_lock() {
        return Ok(g);
    }
    on_wait();
    loop {
        if cancel.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        if let Some(g) = GOVMAP.try_lock_for(Duration::from_millis(500)) {
            return Ok(g);
        }
    }
}

/// Running area downloads (id → their cancel / pause handle).
fn jobs() -> &'static Mutex<HashMap<String, Cancel>> {
    static J: OnceLock<Mutex<HashMap<String, Cancel>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn is_busy(id: &str) -> bool {
    jobs().lock().contains_key(id)
}

pub fn cancel(id: &str) {
    if let Some(c) = jobs().lock().get(id) {
        c.cancel();
    }
}

pub fn pause(id: &str, paused: bool) {
    if let Some(c) = jobs().lock().get(id) {
        c.set_paused(paused);
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Area in km² of a lon/lat box (equirectangular, fine at this size).
pub fn km2(b: [f64; 4]) -> f64 {
    let lat = ((b[1] + b[3]) / 2.0).to_radians();
    let w = (b[2] - b[0]) * 111.32 * lat.cos();
    let h = (b[3] - b[1]) * 110.57;
    (w * h).abs()
}

/// Does the box touch the orthophoto coverage (a corner, the centre or a mid-edge point inside)?
fn touches_coverage(b: [f64; 4]) -> bool {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    [(b[0], b[1]), (b[0], b[3]), (b[2], b[1]), (b[2], b[3]), (cx, cy), (cx, b[1]), (cx, b[3]), (b[0], cy), (b[2], cy)]
        .iter()
        .any(|&(x, y)| ortho::covers(x, y))
}

/// Inside the limits: id, zoom 16–18, a real box inside Israel's box that touches the coverage,
/// at most [`MAX_KM2`].
pub fn valid(a: &SatelliteArea) -> bool {
    let b = a.bbox;
    let i = crate::paths::BBOX_ISRAEL;
    valid_id(&a.id)
        && a.name.chars().count() <= 80
        && (MIN_AREA_ZOOM..=MAX_AREA_ZOOM).contains(&a.zoom)
        && b.iter().all(|v| v.is_finite())
        && b[0] < b[2]
        && b[1] < b[3]
        && b[0] >= i[0] - 0.05
        && b[2] <= i[2] + 0.05
        && b[1] >= i[1] - 0.05
        && b[3] <= i[3] + 0.05
        && km2(b) <= MAX_KM2
        && touches_coverage(b)
}

pub fn dir(paths: &DataPaths) -> PathBuf {
    paths.satellite_dir().join("areas")
}

pub fn file(paths: &DataPaths, id: &str) -> PathBuf {
    dir(paths).join(format!("{id}.pmtiles"))
}

/// First zoom of an area archive: just above the country orthophoto archive, else zoom 10.
pub fn min_zoom(paths: &DataPaths, area_zoom: u8) -> u8 {
    let country = paths.satellite_file();
    let m = super::manifest::Manifest::load(paths);
    let govmap = m.entry(crate::types::DataComponent::Satellite).and_then(|e| e.version.as_deref()).is_some_and(ortho::is_govmap_version);
    let from = match crate::map_tiles::archive_max_zoom(&country) {
        Some(z) if govmap && country.exists() => z + 1,
        _ => DEFAULT_MIN_ZOOM,
    };
    from.clamp(DEFAULT_MIN_ZOOM, area_zoom)
}

/// Tiles (over the coverage), size and download time of an area from `min_z` to `max_z`.
pub fn estimate(b: [f64; 4], min_z: u8, max_z: u8) -> SatelliteAreaEstimate {
    let km2 = km2(b);
    let ok = km2 <= MAX_KM2 && b[0] < b[2] && b[1] < b[3] && touches_coverage(b);
    if !ok || km2 > MAX_KM2 * 4.0 {
        // far too large to count tile by tile: a rough figure is enough to say "too large"
        return SatelliteAreaEstimate { tiles: 0, bytes: 0, minutes: 0.0, km2, ok: false };
    }
    let mut tiles = 0u64;
    for (z, x, y) in satellite::tiles_for_bbox(b, max_z, false) {
        if z >= min_z && ortho::wanted(z, x, y) {
            tiles += 1;
        }
    }
    // govmap requests: the source tiles (256 px) over the box at each zoom's source level; zooms
    // that read the same level share them (the renderer caches source tiles)
    let (sw, ne) = (ortho::itm::from_wgs84(b[0], b[1]), ortho::itm::from_wgs84(b[2], b[3]));
    let (w_m, h_m) = ((ne.0 - sw.0).abs(), (ne.1 - sw.1).abs());
    let lat = (b[1] + b[3]) / 2.0;
    let mut requests = 0.0;
    let mut last_res = f64::NAN;
    for z in min_z..=max_z {
        let res = ortho::source_res(z, lat);
        if res == last_res {
            continue;
        }
        last_res = res;
        let side = 256.0 * res;
        requests += ((w_m / side).ceil() + 1.0) * ((h_m / side).ceil() + 1.0);
    }
    // only the covered part of the box is fetched
    let cover = coverage_fraction(b);
    SatelliteAreaEstimate { tiles, bytes: tiles * AVG_TILE_BYTES, minutes: requests * cover / PACE_PER_SEC / 60.0, km2, ok }
}

/// Share of the box inside the coverage (sampled on a 12 × 12 grid).
fn coverage_fraction(b: [f64; 4]) -> f64 {
    const K: usize = 12;
    let mut inside = 0;
    for i in 0..K {
        for j in 0..K {
            let x = b[0] + (b[2] - b[0]) * (i as f64 + 0.5) / K as f64;
            let y = b[1] + (b[3] - b[1]) * (j as f64 + 0.5) / K as f64;
            if ortho::covers(x, y) {
                inside += 1;
            }
        }
    }
    (inside as f64 / (K * K) as f64).max(0.05)
}

/// The areas that may hold tile z/x/y, best first: the tile overlaps the area's box and the area
/// goes that deep; the smallest area first (the one the user drew around this place), deeper first
/// on a tie.
pub fn order(areas: &[SatelliteArea], z: u8, x: u32, y: u32) -> Vec<&SatelliteArea> {
    let (w, e) = (satellite::tile_lon(x, z), satellite::tile_lon(x + 1, z));
    let (n, s) = (satellite::tile_lat(y, z), satellite::tile_lat(y + 1, z));
    let mut v: Vec<&SatelliteArea> = areas
        .iter()
        .filter(|a| z <= a.zoom && a.bbox[0] < e && a.bbox[2] > w && a.bbox[1] < n && a.bbox[3] > s)
        .collect();
    v.sort_by(|a, b| km2(a.bbox).total_cmp(&km2(b.bbox)).then(b.zoom.cmp(&a.zoom)));
    v
}

/// The tile from the best area archive that has it.
pub fn tile(paths: &DataPaths, areas: &[SatelliteArea], z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    for a in order(areas, z, x, y) {
        if is_busy(&a.id) {
            continue;
        }
        let f = file(paths, &a.id);
        if f.exists() {
            if let Some(t) = crate::map_tiles::archive_tile(&f, z, x, y) {
                return Some(t);
            }
        }
    }
    None
}

/// Any area archive installed?
pub fn any_installed(paths: &DataPaths, areas: &[SatelliteArea]) -> bool {
    areas.iter().any(|a| file(paths, &a.id).exists())
}

pub fn status(paths: &DataPaths, areas: &[SatelliteArea]) -> Vec<SatelliteAreaStatus> {
    areas
        .iter()
        .map(|a| {
            let f = file(paths, &a.id);
            let md = std::fs::metadata(&f).ok();
            let partial = f.with_extension("data.tmp").exists();
            SatelliteAreaStatus {
                id: a.id.clone(),
                present: md.is_some(),
                partial,
                size_bytes: md.as_ref().map_or(0, |m| m.len()),
                zoom: md.as_ref().and_then(|_| crate::map_tiles::archive_max_zoom(&f)),
                updated_iso: md.and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339()),
                busy: is_busy(&a.id),
            }
        })
        .collect()
}

/// Delete an area's archive and its download leftovers.
pub fn delete(paths: &DataPaths, id: &str) -> anyhow::Result<()> {
    if !valid_id(id) {
        anyhow::bail!("INVALID_PATH: bad area id");
    }
    cancel(id);
    let prefix = format!("{id}.");
    if let Ok(rd) = std::fs::read_dir(dir(paths)) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

/// Download (or resume / update) one area. Blocks; progress through `emit`.
pub fn install(paths: &DataPaths, area: &SatelliteArea, emit: &(dyn Fn(SatelliteAreaProgress) + Sync)) -> anyhow::Result<()> {
    if !valid(area) {
        anyhow::bail!("INVALID_AREA: {}", area.id);
    }
    let cancel = {
        let mut j = jobs().lock();
        if j.contains_key(&area.id) {
            anyhow::bail!("UPDATE_RUNNING");
        }
        let c = Cancel::default();
        j.insert(area.id.clone(), c.clone());
        c
    };
    let r = run(paths, area, emit, &cancel);
    jobs().lock().remove(&area.id);
    let (phase, msg) = match &r {
        Ok(()) => (ProgressPhase::Done, String::new()),
        Err(_) if cancel.is_cancelled() => (ProgressPhase::Cancelled, String::new()),
        Err(e) => (ProgressPhase::Failed, format!("{e:#}")),
    };
    emit(SatelliteAreaProgress { id: area.id.clone(), phase, done: 0, total: 0, message: msg });
    r
}

fn run(paths: &DataPaths, area: &SatelliteArea, emit: &(dyn Fn(SatelliteAreaProgress) + Sync), cancel: &Cancel) -> anyhow::Result<()> {
    let id = area.id.clone();
    let say = |phase, done, total, message: String| emit(SatelliteAreaProgress { id: id.clone(), phase, done, total, message });
    std::fs::create_dir_all(dir(paths))?;
    let min_z = min_zoom(paths, area.zoom);
    let est = estimate(area.bbox, min_z, area.zoom);
    // the temporary data plus the finished archive
    super::disk::ensure_free(&dir(paths), est.bytes.saturating_mul(2))?;
    let _turn = govmap_turn(&|| say(ProgressPhase::Queued, 0, 0, "ממתין להורדת תצלומי אוויר אחרת שתסתיים".into()), cancel)?;
    say(ProgressPhase::Processing, 0, 0, "בודק את שרת תצלומי האוויר (govmap)".into());
    let client = super::download::http_client()?;
    let src = ortho::Source::new(client.clone());
    src.probe(cancel)?;
    let fetch = |z: u8, x: u32, y: u32, c: &Cancel| ortho::render(&src, z, x, y, c);
    let url = |_: u8, _: u32, _: u32| String::new();
    let label = format!("תצלומי אוויר – {}", area.name);
    let spec = RasterSpec {
        label: &label,
        url: &url,
        fetch: Some(&fetch),
        filter: Some(&ortho::wanted),
        workers: ortho::WORKERS,
        min_zoom: min_z,
        max_zoom: area.zoom,
        skip_sea: false,
        tile_type: 3,
        placeholders: false,
        name: format!("Survey of Israel orthophoto {} – {}", ortho::CACHE, area.name),
        attribution: ortho::ATTRIBUTION.to_string(),
        bbox: area.bbox,
    };
    let b = area.bbox;
    let version = format!("govmap-{} area z{min_z}-{} {:.5},{:.5},{:.5},{:.5}", ortho::CACHE, area.zoom, b[0], b[1], b[2], b[3]);
    let progress = |phase: ProgressPhase, done: u64, total: u64, msg: &str| {
        say(phase, done, total, msg.to_string());
        cancel.wait_if_paused();
        !cancel.is_cancelled()
    };
    download_raster(&file(paths, &area.id), &version, &spec, &client, &progress, cancel).with_context(|| anyhow!("sharp area {}", area.id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(id: &str, bbox: [f64; 4], zoom: u8) -> SatelliteArea {
        SatelliteArea { id: id.into(), name: id.into(), bbox, zoom }
    }

    // Tel Aviv–Yafo, roughly
    const TLV: [f64; 4] = [34.74, 32.03, 34.85, 32.15];

    #[test]
    fn validates_areas() {
        assert!(valid(&area("tlv", TLV, 17)));
        assert!(!valid(&area("TLV", TLV, 17)), "id must be lower-case");
        assert!(!valid(&area("tlv", TLV, 19)), "zoom above 18");
        assert!(!valid(&area("tlv", TLV, 15)), "zoom below 16");
        assert!(!valid(&area("big", [34.3, 29.5, 35.9, 33.3], 16)), "the whole country is too large");
        assert!(!valid(&area("sea", [33.0, 32.0, 33.2, 32.1], 16)), "outside the coverage");
        assert!(!valid(&area("bad", [34.85, 32.03, 34.74, 32.15], 16)), "west > east");
    }

    #[test]
    fn km2_of_a_city() {
        let k = km2(TLV);
        assert!((100.0..200.0).contains(&k), "{k}");
    }

    #[test]
    fn estimate_grows_with_zoom() {
        let a = estimate(TLV, 15, 16);
        let b = estimate(TLV, 15, 17);
        let c = estimate(TLV, 15, 18);
        assert!(a.ok && b.ok && c.ok);
        assert!(a.tiles < b.tiles && b.tiles < c.tiles);
        // each zoom has about 4× the tiles of the one above it
        assert!((3.0..5.5).contains(&((c.tiles - b.tiles) as f64 / (b.tiles - a.tiles) as f64)));
        assert!(a.minutes < b.minutes && b.minutes < c.minutes);
        assert_eq!(c.bytes, c.tiles * AVG_TILE_BYTES);
        let far = estimate([34.0, 29.0, 36.0, 33.5], 15, 18);
        assert!(!far.ok && far.tiles == 0);
    }

    #[test]
    fn serving_order_prefers_the_smallest_deep_enough_area() {
        let city = area("city", TLV, 17);
        let quarter = area("quarter", [34.76, 32.06, 34.79, 32.09], 18);
        let other = area("other", [35.15, 31.70, 35.28, 31.82], 18);
        let areas = vec![city.clone(), quarter.clone(), other];
        // a z17 tile in the quarter: both Tel Aviv areas hold it, the quarter first
        let (x, y) = (satellite::lon_to_x(34.775, 17), satellite::lat_to_y(32.075, 17));
        let o: Vec<&str> = order(&areas, 17, x, y).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(o, ["quarter", "city"]);
        // z18 only in the quarter
        let (x, y) = (satellite::lon_to_x(34.775, 18), satellite::lat_to_y(32.075, 18));
        let o: Vec<&str> = order(&areas, 18, x, y).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(o, ["quarter"]);
        // a z17 tile in Tel Aviv outside the quarter: only the city
        let (x, y) = (satellite::lon_to_x(34.83, 17), satellite::lat_to_y(32.13, 17));
        let o: Vec<&str> = order(&areas, 17, x, y).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(o, ["city"]);
        // nowhere near: none
        let (x, y) = (satellite::lon_to_x(35.0, 17), satellite::lat_to_y(30.0, 17));
        assert!(order(&areas, 17, x, y).is_empty());
    }

    /// `cargo test --lib sat_areas::tests::print_estimates -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_estimates() {
        for (z0, z) in [(15, 16), (15, 17), (15, 18), (10, 17)] {
            let e = estimate(TLV, z0, z);
            eprintln!("Tel Aviv z{z0}-{z}: {} tiles, {:.0} MB, {:.0} min, {:.0} km²", e.tiles, e.bytes as f64 / 1e6, e.minutes, e.km2);
        }
    }

    #[test]
    fn ids_are_plain() {
        assert!(valid_id("a-1"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
    }

    #[test]
    fn delete_removes_leftovers() {
        let root = std::env::temp_dir().join(format!("mo-sat-areas-{}", std::process::id()));
        let paths = DataPaths::new(&root);
        let d = dir(&paths);
        std::fs::create_dir_all(&d).unwrap();
        for f in ["a1.pmtiles", "a1.data.tmp", "a1.data.index.tmp", "a10.pmtiles"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        delete(&paths, "a1").unwrap();
        assert!(!d.join("a1.pmtiles").exists() && !d.join("a1.data.tmp").exists());
        assert!(d.join("a10.pmtiles").exists(), "another area with a similar id stays");
        let _ = std::fs::remove_dir_all(&root);
    }
}
