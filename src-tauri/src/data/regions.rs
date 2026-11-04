//! Other countries (OsmAnd-style downloads): the offline map and the search index of any
//! Geofabrik region, next to the built-in Israel data.
//!
//! - Catalogue: Geofabrik `index-v1.json` (continents → countries → sub-regions, with outlines),
//!   cached as `regions/catalog.json` and refreshed weekly.
//! - Map: `pmtiles extract --region=<outline>` of the latest Protomaps build → `regions/<id>/map.pmtiles`
//!   (the app serves every installed map through one merged tile source, see `map_tiles.rs`).
//! - Search: the region's OSM extract → the same geocoder builder as Israel →
//!   `regions/<id>/geocode.sqlite`; the PBF is deleted afterwards.
//!
//! Routing and transit stay Israel-only offline (online routing works everywhere).
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};

use super::download::{self, http_client};
use super::tiles::{self, Extract};
use super::Cancel;
use crate::paths::{atomic_replace, DataPaths};
use crate::types::{InstalledRegion, ProgressPhase, RegionEntry, RegionPart, RegionPartStatus, RegionProgress, RegionSize};

const INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";
const CATALOG_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Regions already covered by the built-in components.
const BUILTIN: [&str; 1] = ["israel-and-palestine"];

/// One catalogue feature with what installing needs.
#[derive(Clone)]
pub struct Feature {
    pub entry: RegionEntry,
    pub pbf_url: String,
    pub geometry: serde_json::Value,
}

/// Per-region facts kept next to the data (`region.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionMeta {
    id: String,
    name: String,
    iso: Option<String>,
    bbox: [f64; 4],
    search_version: Option<String>,
    #[serde(default)]
    routing_version: Option<String>,
    updated_iso: Option<String>,
}

fn catalog_file(paths: &DataPaths) -> PathBuf {
    paths.regions_dir().join("catalog.json")
}

static CATALOG: Mutex<Option<(SystemTime, Arc<Vec<Feature>>)>> = Mutex::new(None);

/// The catalogue (downloaded when missing, older than a week or `refresh`; an old copy is used
/// when the download fails).
pub fn catalog(paths: &DataPaths, refresh: bool) -> anyhow::Result<Arc<Vec<Feature>>> {
    let file = catalog_file(paths);
    let age = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if refresh || age.is_none_or(|a| a > CATALOG_MAX_AGE) {
        match download_catalog(&file) {
            Ok(()) => {}
            Err(e) if file.exists() => log::warn!("region catalogue refresh failed, using the cached copy: {e:#}"),
            Err(e) => return Err(e),
        }
    }
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).context("region catalogue missing")?;
    let mut g = CATALOG.lock();
    if let Some((t, c)) = g.as_ref() {
        if *t == mtime {
            return Ok(c.clone());
        }
    }
    let parsed = Arc::new(parse_catalog(&std::fs::read(&file)?)?);
    *g = Some((mtime, parsed.clone()));
    Ok(parsed)
}

fn download_catalog(file: &Path) -> anyhow::Result<()> {
    let client = http_client()?;
    let r = client.get(INDEX_URL).send().with_context(|| format!("GET {INDEX_URL}"))?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(INDEX_URL));
    }
    let body = r.error_for_status()?.bytes()?;
    parse_catalog(&body).context("region catalogue is not valid")?;
    std::fs::create_dir_all(file.parent().unwrap())?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, &body)?;
    atomic_replace(file, &tmp)?;
    Ok(())
}

fn bbox_of(geom: &serde_json::Value) -> Option<[f64; 4]> {
    fn walk(v: &serde_json::Value, b: &mut [f64; 4]) {
        if let Some(a) = v.as_array() {
            if a.len() >= 2 && a[0].is_number() && a[1].is_number() {
                let (x, y) = (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0));
                b[0] = b[0].min(x);
                b[1] = b[1].min(y);
                b[2] = b[2].max(x);
                b[3] = b[3].max(y);
            } else {
                for c in a {
                    walk(c, b);
                }
            }
        }
    }
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    walk(geom.get("coordinates")?, &mut b);
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

pub(crate) fn parse_catalog(bytes: &[u8]) -> anyhow::Result<Vec<Feature>> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let feats = v["features"].as_array().ok_or_else(|| anyhow!("no features"))?;
    let mut out = Vec::with_capacity(feats.len());
    for f in feats {
        let p = &f["properties"];
        let (Some(id), Some(name), Some(pbf)) = (p["id"].as_str(), p["name"].as_str(), p["urls"]["pbf"].as_str()) else { continue };
        let Some(bbox) = bbox_of(&f["geometry"]) else { continue };
        let iso = match p["iso3166-1:alpha2"].as_array() {
            Some(a) if a.len() == 1 => a[0].as_str().map(str::to_string),
            _ => None,
        };
        // US states come as "us/new-york" with the continent as parent: give them a readable
        // name and put them under the United States
        let (name, parent) = match id.split_once('/') {
            Some((country, rest)) if name == id => (
                rest.split('-').map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }).collect::<Vec<_>>().join(" "),
                Some(country.to_string()),
            ),
            _ => (name.to_string(), p["parent"].as_str().map(str::to_string)),
        };
        out.push(Feature {
            entry: RegionEntry {
                id: id.to_string(),
                parent,
                name,
                iso,
                bbox,
                builtin: BUILTIN.contains(&id),
            },
            pbf_url: pbf.to_string(),
            geometry: f["geometry"].clone(),
        });
    }
    if out.len() < 50 {
        bail!("only {} regions in the catalogue", out.len());
    }
    Ok(out)
}

pub fn find(paths: &DataPaths, id: &str) -> anyhow::Result<Feature> {
    catalog(paths, false)?
        .iter()
        .find(|f| f.entry.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("UNKNOWN_REGION: {id}"))
}

/// Is (lon, lat) inside a GeoJSON Polygon / MultiPolygon (even-odd over all rings, so holes work)?
fn geom_contains(geom: &serde_json::Value, lon: f64, lat: f64) -> bool {
    fn ring_hits(ring: &[serde_json::Value], lon: f64, lat: f64) -> bool {
        let pts: Vec<(f64, f64)> = ring
            .iter()
            .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
            .collect();
        let mut inside = false;
        let n = pts.len();
        if n < 3 {
            return false;
        }
        let mut j = n - 1;
        for i in 0..n {
            let ((xi, yi), (xj, yj)) = (pts[i], pts[j]);
            if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
    let polys: Vec<&serde_json::Value> = match geom["type"].as_str() {
        Some("Polygon") => vec![&geom["coordinates"]],
        Some("MultiPolygon") => geom["coordinates"].as_array().map(|a| a.iter().collect()).unwrap_or_default(),
        _ => return false,
    };
    let mut inside = false;
    for poly in polys {
        for ring in poly.as_array().into_iter().flatten() {
            if ring.as_array().is_some_and(|r| ring_hits(r, lon, lat)) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Regions whose outline holds the point, smallest (by bbox area) first.
pub fn containing(cat: &[Feature], lon: f64, lat: f64) -> Vec<String> {
    let mut v: Vec<&Feature> = cat
        .iter()
        .filter(|f| {
            let b = f.entry.bbox;
            lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3] && geom_contains(&f.geometry, lon, lat)
        })
        .collect();
    let area = |f: &Feature| (f.entry.bbox[2] - f.entry.bbox[0]) * (f.entry.bbox[3] - f.entry.bbox[1]);
    v.sort_by(|a, b| area(a).total_cmp(&area(b)));
    v.into_iter().map(|f| f.entry.id.clone()).collect()
}

/// Find a city / country / place by name (Hebrew, English, any language) with OpenStreetMap's
/// Nominatim and tell which downloadable regions contain each hit.
pub fn locate(paths: &DataPaths, query: &str, lang: &str) -> anyhow::Result<Vec<crate::types::RegionLocate>> {
    let q = query.trim();
    if q.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let cat = catalog(paths, false)?;
    // Nominatim's usage policy: at most one request per second; the same query is answered from
    // memory (typing back and forth repeats queries)
    static ASKED: Mutex<Option<(std::time::Instant, std::collections::HashMap<String, Vec<crate::types::RegionLocate>>)>> = Mutex::new(None);
    let key = format!("{lang}|{}", q.to_lowercase());
    {
        let mut g = ASKED.lock();
        let (last, memo) = g.get_or_insert_with(|| (std::time::Instant::now() - Duration::from_secs(2), Default::default()));
        if let Some(hit) = memo.get(&key) {
            return Ok(hit.clone());
        }
        let wait = Duration::from_millis(1100).saturating_sub(last.elapsed());
        *last = std::time::Instant::now() + wait;
        drop(g);
        std::thread::sleep(wait);
    }
    let out = locate_online(&cat, q, lang)?;
    let mut g = ASKED.lock();
    if let Some((_, memo)) = g.as_mut() {
        if memo.len() > 200 {
            memo.clear();
        }
        memo.insert(key, out.clone());
    }
    Ok(out)
}

fn locate_online(cat: &[Feature], q: &str, lang: &str) -> anyhow::Result<Vec<crate::types::RegionLocate>> {
    let url = reqwest::Url::parse_with_params(
        "https://nominatim.openstreetmap.org/search",
        &[("q", q), ("format", "jsonv2"), ("limit", "6"), ("addressdetails", "1"), ("accept-language", if lang == "en" { "en" } else { "he,en" })],
    )?;
    let r = http_client()?.get(url.clone()).timeout(Duration::from_secs(12)).send().context("search service unreachable")?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(url.as_str()));
    }
    let hits: Vec<serde_json::Value> = r.error_for_status()?.json()?;
    let mut out: Vec<crate::types::RegionLocate> = Vec::new();
    for h in hits {
        let (Some(lat), Some(lon)) = (h["lat"].as_str().and_then(|v| v.parse().ok()), h["lon"].as_str().and_then(|v| v.parse::<f64>().ok())) else { continue };
        let mut regions = containing(cat, lon, lat);
        // a whole country: start the chain at the country itself, not at the sub-region that
        // happens to hold its centre point ("צרפת" → France, not Centre)
        if h["addresstype"].as_str() == Some("country") {
            if let Some(cc) = h["address"]["country_code"].as_str().map(str::to_uppercase) {
                if let Some(i) = regions.iter().position(|id| cat.iter().any(|f| f.entry.id == *id && f.entry.iso.as_deref() == Some(cc.as_str()))) {
                    regions.drain(..i);
                }
            }
        }
        if regions.is_empty() || out.iter().any(|o| o.regions == regions) {
            continue;
        }
        let display = h["display_name"].as_str().unwrap_or("");
        let name = h["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| display.split(',').next().unwrap_or("").trim().to_string());
        let rest: Vec<&str> = display.split(',').map(str::trim).filter(|p| *p != name && !p.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '-')).collect();
        let detail = rest.iter().rev().take(2).rev().cloned().collect::<Vec<_>>().join(", ");
        out.push(crate::types::RegionLocate { name, detail, lon, lat, regions });
    }
    Ok(out)
}

/// Download size of the region's OSM extract (HEAD).
pub fn size(paths: &DataPaths, id: &str) -> anyhow::Result<RegionSize> {
    let f = find(paths, id)?;
    let r = http_client()?.head(&f.pbf_url).send()?;
    Ok(RegionSize { pbf_bytes: r.status().is_success().then(|| r.content_length()).flatten() })
}

fn map_extract(dir: &Path, f: &RegionEntry) -> Extract {
    Extract {
        file: dir.join("map.pmtiles"),
        bbox: Some(f.bbox),
        maxzoom: 15,
        label: format!("מפת {}", f.name),
        region: Some(dir.join("region.geojson")),
        min_size: 16 * 1024,
    }
}

pub fn map_file(dir: &Path) -> PathBuf {
    dir.join("map.pmtiles")
}
pub fn geocode_file(dir: &Path) -> PathBuf {
    dir.join("geocode.sqlite")
}
pub fn graph_file(dir: &Path) -> PathBuf {
    dir.join("graph.bin")
}

fn read_meta(dir: &Path) -> Option<RegionMeta> {
    serde_json::from_slice(&std::fs::read(dir.join("region.json")).ok()?).ok()
}
fn write_meta(dir: &Path, m: &RegionMeta) -> anyhow::Result<()> {
    let tmp = dir.join("region.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    atomic_replace(&dir.join("region.json"), &tmp)?;
    META_GEN.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Bumped whenever a region.json is written or a region folder removed.
static META_GEN: AtomicU64 = AtomicU64::new(0);
/// Re-read the folders at least this often (a folder copied in by hand shows up).
const META_TTL: Duration = Duration::from_secs(10);

/// (folder, meta) of every installed region. Every search and map refresh asked for this; it
/// used to list the folder and parse each region.json every time.
fn metas(paths: &DataPaths) -> Arc<Vec<(PathBuf, RegionMeta)>> {
    type Cached = (PathBuf, u64, std::time::Instant, Arc<Vec<(PathBuf, RegionMeta)>>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
    let root = paths.regions_dir();
    let gen = META_GEN.load(Ordering::SeqCst);
    let mut g = CACHE.lock();
    if let Some((r, gn, t, v)) = g.as_ref() {
        if *r == root && *gn == gen && t.elapsed() < META_TTL {
            return v.clone();
        }
    }
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let dir = e.path();
            if let Some(m) = read_meta(&dir) {
                v.push((dir, m));
            }
        }
    }
    let v = Arc::new(v);
    *g = Some((root, gen, std::time::Instant::now(), v.clone()));
    v
}

/// Installed regions (folders with a `region.json`), sorted by name.
pub fn installed(paths: &DataPaths, busy: &dyn Fn(&str) -> bool) -> Vec<InstalledRegion> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(paths.regions_dir()) else { return out };
    for e in rd.flatten() {
        let dir = e.path();
        let Some(m) = read_meta(&dir) else { continue };
        let entry = RegionEntry { id: m.id.clone(), parent: None, name: m.name.clone(), iso: m.iso.clone(), bbox: m.bbox, builtin: false };
        let ex = map_extract(&dir, &entry);
        let map_version = ex.installed_build();
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let gf = geocode_file(&dir);
        let rf = graph_file(&dir);
        out.push(InstalledRegion {
            routing: RegionPartStatus { present: rf.exists(), size_bytes: size(&rf), version: m.routing_version.clone().filter(|_| rf.exists()) },
            busy: busy(&m.id),
            map: RegionPartStatus { present: map_version.is_some(), size_bytes: size(&ex.file), version: map_version },
            search: RegionPartStatus { present: gf.exists(), size_bytes: size(&gf), version: m.search_version.clone().filter(|_| gf.exists()) },
            id: m.id,
            name: m.name,
            iso: m.iso,
            bbox: m.bbox,
            updated_iso: m.updated_iso,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Region folders whose map / search index is ready (for the tile server and the geocoder).
pub fn ready_maps(paths: &DataPaths) -> Vec<(String, PathBuf, [f64; 4])> {
    let mut v = Vec::new();
    for (dir, m) in metas(paths).iter() {
        let f = map_file(dir);
        if f.exists() {
            v.push((m.id.clone(), f, m.bbox));
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Countries with an offline routing graph: (id, file, bbox).
pub fn ready_graphs(paths: &DataPaths) -> Vec<(String, PathBuf, [f64; 4])> {
    let mut v = Vec::new();
    for (dir, m) in metas(paths).iter() {
        let f = graph_file(dir);
        if f.exists() && m.routing_version.is_some() {
            v.push((m.id.clone(), f, m.bbox));
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

pub fn ready_geocoders(paths: &DataPaths) -> Vec<(String, PathBuf, [f64; 4])> {
    let mut v = Vec::new();
    for (dir, m) in metas(paths).iter() {
        let f = geocode_file(dir);
        if f.exists() && m.search_version.is_some() {
            v.push((m.id.clone(), f, m.bbox));
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Install or update `parts` of region `id`. `emit` gets every progress step; the parts run in
/// parallel (the map extract and the PBF download use different hosts).
pub fn install(paths: &DataPaths, id: &str, parts: &[RegionPart], emit: &(dyn Fn(RegionProgress) + Sync), cancel: &Cancel) -> anyhow::Result<()> {
    anyhow::ensure!(valid_region_id(id), "invalid region id {id:?}");
    let f = find(paths, id)?;
    if f.entry.builtin {
        bail!("ישראל כבר כלולה בנתונים הבסיסיים");
    }
    let dir = paths.region_dir(id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("region.geojson"), serde_json::to_vec(&f.geometry)?)?;
    let mut meta = read_meta(&dir).unwrap_or_default();
    meta.id = f.entry.id.clone();
    meta.name = f.entry.name.clone();
    meta.iso = f.entry.iso.clone();
    meta.bbox = f.entry.bbox;
    write_meta(&dir, &meta)?;

    let progress_for = |part: RegionPart| {
        let id = id.to_string();
        move |phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
            cancel.wait_if_paused();
            emit(RegionProgress { id: id.clone(), part, phase, done, total, message: msg.to_string() });
            !cancel.is_cancelled()
        }
    };
    let finish = |part: RegionPart, r: &anyhow::Result<()>| {
        let (phase, msg) = match r {
            Ok(()) => (ProgressPhase::Done, String::new()),
            Err(_) if cancel.is_cancelled() => (ProgressPhase::Cancelled, String::new()),
            Err(e) => (ProgressPhase::Failed, format!("{e:#}")),
        };
        emit(RegionProgress { id: id.to_string(), part, phase, done: 0, total: 0, message: msg });
    };

    let want_search = parts.contains(&RegionPart::Search);
    let want_routing = parts.contains(&RegionPart::Routing);
    let (map_r, osm_r) = std::thread::scope(|s| {
        let map = parts.contains(&RegionPart::Map).then(|| {
            s.spawn(|| -> anyhow::Result<()> {
                let p = progress_for(RegionPart::Map);
                p(ProgressPhase::Queued, 0, 0, "");
                let r = tiles::update_extract(paths, &map_extract(&dir, &f.entry), &p, cancel).map(|_| ());
                finish(RegionPart::Map, &r);
                r
            })
        });
        // search and routing share one download of the region's OSM extract
        let osm = (want_search || want_routing).then(|| {
            s.spawn(|| -> (Option<anyhow::Result<String>>, Option<anyhow::Result<String>>) {
                let ps = progress_for(RegionPart::Search);
                let pr = progress_for(RegionPart::Routing);
                let both = |phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
                    let mut ok = true;
                    if want_search {
                        ok &= ps(phase, done, total, msg);
                    }
                    if want_routing {
                        ok &= pr(phase, done, total, msg);
                    }
                    ok
                };
                both(ProgressPhase::Queued, 0, 0, "");
                let pbf = paths.downloads_dir().join("regions").join(format!("{}.osm.pbf", id.replace(['/', '\\', ':'], "_")));
                let version = match download::ensure_fresh(&f.pbf_url, &pbf, &both, cancel).and_then(|v| cancel.check().map(|_| v)) {
                    Ok(v) => v.unwrap_or_else(|| "unknown".into()),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        let fail = |part: RegionPart| -> anyhow::Result<String> {
                            finish(part, &Err(anyhow!("{msg}")));
                            Err(anyhow!("{msg}"))
                        };
                        return (want_search.then(|| fail(RegionPart::Search)), want_routing.then(|| fail(RegionPart::Routing)));
                    }
                };
                let search = want_search.then(|| {
                    let r = (|| {
                        let tmp = geocode_file(&dir).with_extension("sqlite.tmp");
                        let _ = std::fs::remove_file(&tmp);
                        let mid_lat = (f.entry.bbox[1] + f.entry.bbox[3]) / 2.0;
                        crate::geocode::build_region(&pbf, &tmp, mid_lat, &ps)?;
                        atomic_replace(&geocode_file(&dir), &tmp)?;
                        Ok(format!("{version} s{}", crate::geocode::SCHEMA_VERSION))
                    })();
                    finish(RegionPart::Search, &r.as_ref().map(|_| ()).map_err(|e| anyhow!("{e:#}")));
                    r
                });
                let routing = want_routing.then(|| {
                    let r = (|| {
                        cancel.check()?;
                        let tmp = graph_file(&dir).with_extension("bin.tmp");
                        let _ = std::fs::remove_file(&tmp);
                        crate::routing::build_graph(&pbf, &tmp, &pr)?;
                        atomic_replace(&graph_file(&dir), &tmp)?;
                        Ok(format!("{version} g{}.{}", crate::routing::graph::VERSION, crate::routing::graph::BUILD_REV))
                    })();
                    finish(RegionPart::Routing, &r.as_ref().map(|_| ()).map_err(|e| anyhow!("{e:#}")));
                    r
                });
                // the extract is only needed for the builds; a later update downloads it fresh
                let _ = std::fs::remove_file(&pbf);
                let _ = std::fs::remove_file(download::meta_path(&pbf));
                (search, routing)
            })
        });
        (
            map.map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("map job panicked")))),
            osm.map(|h| h.join().unwrap_or_else(|_| (Some(Err(anyhow!("search job panicked"))), None))),
        )
    });
    let (search_r, routing_r) = osm_r.unwrap_or((None, None));

    let mut meta = read_meta(&dir).unwrap_or(meta);
    if let Some(Ok(v)) = &search_r {
        meta.search_version = Some(v.clone());
    }
    if let Some(Ok(v)) = &routing_r {
        meta.routing_version = Some(v.clone());
    }
    if map_r.as_ref().is_some_and(|r| r.is_ok()) || search_r.as_ref().is_some_and(|r| r.is_ok()) || routing_r.as_ref().is_some_and(|r| r.is_ok()) {
        meta.updated_iso = Some(chrono::Local::now().to_rfc3339());
    }
    write_meta(&dir, &meta)?;
    // nothing usable left from a first install that failed: forget the folder
    if !map_file(&dir).exists() && !geocode_file(&dir).exists() && !graph_file(&dir).exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    for r in [map_r, search_r.map(|r| r.map(|_| ())), routing_r.map(|r| r.map(|_| ()))].into_iter().flatten() {
        r?;
    }
    Ok(())
}

/// A Geofabrik region id ("europe/germany", "us-west") names exactly one folder under
/// `regions/`: letters, digits, '-', '_' and '/' only, and no empty / "." / ".." segment.
/// ("..": `regions/..` is the data root, which `delete` would have wiped.)
pub fn valid_region_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/'))
        && id.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

pub fn delete(paths: &DataPaths, id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(valid_region_id(id), "invalid region id {id:?}");
    let dir = paths.region_dir(id);
    // belt and braces: the folder must sit directly inside regions/
    anyhow::ensure!(dir.parent() == Some(paths.regions_dir().as_path()), "invalid region folder {}", dir.display());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    META_GEN.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_ids_cannot_escape_the_regions_folder() {
        for ok in ["europe/germany", "us-west", "asia/israel-and-palestine", "a_b"] {
            assert!(valid_region_id(ok), "{ok}");
        }
        for bad in ["", "..", ".", "../x", "europe/..", "a//b", "/abs", r"c:\x", r"a\..", "x y"] {
            assert!(!valid_region_id(bad), "{bad}");
        }
        let paths = DataPaths::new(std::env::temp_dir().join("mo-region-id-test"));
        assert!(delete(&paths, "..").is_err());
    }

    #[test]
    fn point_in_multipolygon_with_hole() {
        let g: serde_json::Value = serde_json::json!({"type": "MultiPolygon", "coordinates": [
            [[[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]], [[4, 4], [6, 4], [6, 6], [4, 6], [4, 4]]],
            [[[20, 20], [22, 20], [22, 22], [20, 22], [20, 20]]]
        ]});
        assert!(geom_contains(&g, 1.0, 1.0));
        assert!(!geom_contains(&g, 5.0, 5.0), "inside the hole");
        assert!(geom_contains(&g, 21.0, 21.0));
        assert!(!geom_contains(&g, 15.0, 15.0));
    }

    #[test]
    fn parses_catalogue_shape() {
        let mut feats = String::new();
        for i in 0..60 {
            if i > 0 {
                feats.push(',');
            }
            feats.push_str(&format!(
                r#"{{"type":"Feature","properties":{{"id":"r{i}","parent":"europe","name":"R {i}","iso3166-1:alpha2":["F{i}"],"urls":{{"pbf":"https://x/r{i}.osm.pbf"}}}},"geometry":{{"type":"MultiPolygon","coordinates":[[[[1,2],[3,2],[3,{i}.5],[1,2]]]]}}}}"#
            ));
        }
        let j = format!(r#"{{"type":"FeatureCollection","features":[{feats},{{"type":"Feature","properties":{{"id":"israel-and-palestine","name":"Israel","iso3166-1:alpha2":["PS","IL"],"urls":{{"pbf":"u"}}}},"geometry":{{"type":"Polygon","coordinates":[[[34,29],[36,29],[36,33],[34,29]]]}}}}]}}"#);
        let c = parse_catalog(j.as_bytes()).unwrap();
        assert_eq!(c.len(), 61);
        assert_eq!(c[5].entry.bbox, [1.0, 2.0, 3.0, 5.5]);
        assert_eq!(c[5].entry.iso.as_deref(), Some("F5"));
        let il = c.iter().find(|f| f.entry.id == "israel-and-palestine").unwrap();
        assert!(il.entry.builtin && il.entry.iso.is_none() && il.entry.parent.is_none());
    }
}
