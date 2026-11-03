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
use std::sync::{Arc, Mutex};
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
    if refresh || age.map_or(true, |a| a > CATALOG_MAX_AGE) {
        match download_catalog(&file) {
            Ok(()) => {}
            Err(e) if file.exists() => log::warn!("region catalogue refresh failed, using the cached copy: {e:#}"),
            Err(e) => return Err(e),
        }
    }
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).context("region catalogue missing")?;
    let mut g = CATALOG.lock().unwrap();
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
        out.push(Feature {
            entry: RegionEntry {
                id: id.to_string(),
                parent: p["parent"].as_str().map(str::to_string),
                name: name.to_string(),
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

fn read_meta(dir: &Path) -> Option<RegionMeta> {
    serde_json::from_slice(&std::fs::read(dir.join("region.json")).ok()?).ok()
}
fn write_meta(dir: &Path, m: &RegionMeta) -> anyhow::Result<()> {
    let tmp = dir.join("region.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    atomic_replace(&dir.join("region.json"), &tmp)?;
    Ok(())
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
        out.push(InstalledRegion {
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
    if let Ok(rd) = std::fs::read_dir(paths.regions_dir()) {
        for e in rd.flatten() {
            let dir = e.path();
            if let Some(m) = read_meta(&dir) {
                let f = map_file(&dir);
                if f.exists() {
                    v.push((m.id, f, m.bbox));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

pub fn ready_geocoders(paths: &DataPaths) -> Vec<(String, PathBuf, [f64; 4])> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(paths.regions_dir()) {
        for e in rd.flatten() {
            let dir = e.path();
            if let Some(m) = read_meta(&dir) {
                let f = geocode_file(&dir);
                if f.exists() && m.search_version.is_some() {
                    v.push((m.id, f, m.bbox));
                }
            }
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Install or update `parts` of region `id`. `emit` gets every progress step; the parts run in
/// parallel (the map extract and the PBF download use different hosts).
pub fn install(paths: &DataPaths, id: &str, parts: &[RegionPart], emit: &(dyn Fn(RegionProgress) + Sync), cancel: &Cancel) -> anyhow::Result<()> {
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

    let (map_r, search_r) = std::thread::scope(|s| {
        let map = parts.contains(&RegionPart::Map).then(|| {
            s.spawn(|| -> anyhow::Result<()> {
                let p = progress_for(RegionPart::Map);
                p(ProgressPhase::Queued, 0, 0, "");
                let r = tiles::update_extract(paths, &map_extract(&dir, &f.entry), &p, cancel).map(|_| ());
                finish(RegionPart::Map, &r);
                r
            })
        });
        let search = parts.contains(&RegionPart::Search).then(|| {
            s.spawn(|| -> anyhow::Result<(Option<String>, ())> {
                let p = progress_for(RegionPart::Search);
                p(ProgressPhase::Queued, 0, 0, "");
                let r = (|| {
                    let pbf = paths.downloads_dir().join("regions").join(format!("{}.osm.pbf", id.replace(['/', '\\', ':'], "_")));
                    let v = download::ensure_fresh(&f.pbf_url, &pbf, &p, cancel)?;
                    cancel.check()?;
                    let tmp = geocode_file(&dir).with_extension("sqlite.tmp");
                    let _ = std::fs::remove_file(&tmp);
                    let mid_lat = (f.entry.bbox[1] + f.entry.bbox[3]) / 2.0;
                    crate::geocode::build_region(&pbf, &tmp, mid_lat, &p)?;
                    atomic_replace(&geocode_file(&dir), &tmp)?;
                    // the extract is only needed for the build; a later update downloads it fresh
                    let _ = std::fs::remove_file(&pbf);
                    let _ = std::fs::remove_file(download::meta_path(&pbf));
                    Ok((v, ()))
                })();
                finish(RegionPart::Search, &r.as_ref().map(|_| ()).map_err(|e| anyhow!("{e:#}")));
                r
            })
        });
        (map.map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("map job panicked")))), search.map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("search job panicked")))))
    });

    let mut meta = read_meta(&dir).unwrap_or(meta);
    if let Some(Ok((v, ()))) = &search_r {
        meta.search_version = Some(format!("{} s{}", v.clone().unwrap_or_else(|| "unknown".into()), crate::geocode::SCHEMA_VERSION));
    }
    if map_r.as_ref().is_some_and(|r| r.is_ok()) || search_r.as_ref().is_some_and(|r| r.is_ok()) {
        meta.updated_iso = Some(chrono::Local::now().to_rfc3339());
    }
    write_meta(&dir, &meta)?;
    // nothing usable left from a first install that failed: forget the folder
    if !map_file(&dir).exists() && !geocode_file(&dir).exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    if let Some(Err(e)) = map_r {
        return Err(e);
    }
    if let Some(Err(e)) = search_r {
        return Err(e);
    }
    Ok(())
}

pub fn delete(paths: &DataPaths, id: &str) -> anyhow::Result<()> {
    let dir = paths.region_dir(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
