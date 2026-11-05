//! Countries from OsmAnd `.obf` files: OsmAnd's map list (download.osmand.net), downloading a
//! file, and importing a downloaded or local file into an ordinary country folder
//! (`regions/osmand/<name>/`, `region.json` with `source: "osmand"`):
//!   OBF → PBF (`obf::to_pbf`) → the usual search index + routing graph builders,
//!   OBF map section → `map.pmtiles` (`obf::tiles`).
//!
//! Same area as an installed country (see `regions::same_area`): the caller's
//! [`ConflictPolicy`] decides – replace it, keep both (the smaller / more specific area is used
//! first), or skip the new one.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};

use super::{download, regions, Cancel};
use crate::paths::{atomic_replace, DataPaths};
use crate::types::{ConflictPolicy, ObfCatalogEntry, ProgressPhase, RegionConflict, RegionPart, RegionProgress};

pub const INDEX_URL: &str = "https://download.osmand.net/get_indexes";
const DOWNLOAD_URL: &str = "https://download.osmand.net/download?standard=yes&file=";
/// The list changes daily but a week is fresh enough for picking countries.
const CATALOG_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

fn catalog_file(paths: &DataPaths) -> PathBuf {
    paths.regions_dir().join("osmand-catalog.xml")
}

fn work_dir(paths: &DataPaths) -> PathBuf {
    paths.downloads_dir().join("obf")
}

/// "Burundi_africa_2.obf.zip" / "Burundi_africa_2.obf" → "Burundi_africa".
pub fn stem(file: &str) -> String {
    let f = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let f = f.strip_suffix(".zip").unwrap_or(f);
    let f = f.strip_suffix(".obf").unwrap_or(f);
    f.strip_suffix("_2").unwrap_or(f).to_string()
}

/// Region id of an OsmAnd file: "osmand/burundi_africa" (lower case, safe characters only).
pub fn region_id(file: &str) -> String {
    let s: String = stem(file).to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("osmand/{}", if s.is_empty() { "file".into() } else { s })
}

/// Readable name and continent from a file stem: "Us_california_san-diego_northamerica" →
/// ("Us california san-diego", "northamerica").
fn split_name(stem: &str) -> (String, String) {
    const CONTINENTS: [&str; 8] = ["africa", "asia", "europe", "northamerica", "southamerica", "centralamerica", "australia-oceania", "russia"];
    let mut parts: Vec<&str> = stem.split('_').collect();
    let continent = match parts.last() {
        Some(last) if parts.len() > 1 && CONTINENTS.contains(&last.to_lowercase().as_str()) => {
            let c = last.to_lowercase();
            parts.pop();
            c
        }
        _ => String::new(),
    };
    (parts.join(" "), continent)
}

/// Parse OsmAnd's `get_indexes` XML: the country maps only (no world base map, sea marks, voices…).
pub fn parse_catalog(xml: &str) -> anyhow::Result<Vec<ObfCatalogEntry>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) | Event::Start(e) if e.name().as_ref() == "region" => {
                let get = |k: &str| -> Option<String> {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == k)
                        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                        .map(|v| v.into_owned())
                };
                if get("type").as_deref() != Some("map") {
                    continue;
                }
                let Some(file) = get("name") else { continue };
                if !file.ends_with("_2.obf.zip") {
                    continue;
                }
                let st = stem(&file);
                let (name, continent) = split_name(&st);
                // the world overview maps are not countries
                if continent.is_empty() || name.to_lowercase().starts_with("world") {
                    continue;
                }
                let num = |k: &str| get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                out.push(ObfCatalogEntry {
                    region_id: region_id(&file),
                    country: name.split(' ').next().unwrap_or(&name).to_string(),
                    name,
                    continent,
                    size_bytes: num("containerSize"),
                    content_bytes: num("contentSize"),
                    date: get("date").unwrap_or_default(),
                    file,
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if out.len() < 50 {
        bail!("only {} maps in OsmAnd's list", out.len());
    }
    out.sort_by(|a, b| a.continent.cmp(&b.continent).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// OsmAnd's map list, cached for a week in `regions/osmand-catalog.xml` (`refresh` forces a fetch;
/// a failed fetch falls back to the cached copy).
pub fn catalog(paths: &DataPaths, refresh: bool) -> anyhow::Result<Vec<ObfCatalogEntry>> {
    let file = catalog_file(paths);
    let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < CATALOG_TTL);
    if !refresh && fresh {
        if let Ok(list) = std::fs::read_to_string(&file).map_err(anyhow::Error::from).and_then(|s| parse_catalog(&s)) {
            return Ok(list);
        }
    }
    let fetched = (|| -> anyhow::Result<String> {
        let r = download::http_client()?.get(INDEX_URL).timeout(Duration::from_secs(60)).send()?;
        if r.status().as_u16() == 418 {
            bail!("{}", download::blocked_message(INDEX_URL));
        }
        Ok(r.error_for_status()?.text()?)
    })();
    match fetched {
        Ok(xml) => {
            let list = parse_catalog(&xml)?;
            std::fs::create_dir_all(paths.regions_dir())?;
            let tmp = file.with_extension("xml.tmp");
            std::fs::write(&tmp, xml.as_bytes())?;
            atomic_replace(&file, &tmp)?;
            Ok(list)
        }
        Err(e) => match std::fs::read_to_string(&file) {
            Ok(s) => parse_catalog(&s),
            Err(_) => Err(e.context("רשימת המפות של OsmAnd")),
        },
    }
}

/// OsmAnd's file for Israel (used when Settings.mapSource = "osmand").
pub const ISRAEL_FILE: &str = "Israel_asia_2.obf.zip";

/// Version of a file in OsmAnd's list: "obf YYYY-MM-DD" from the list's date. `refresh` asks the
/// server (falling back to the cached list when it cannot be reached).
pub fn catalog_version(paths: &DataPaths, file: &str, refresh: bool) -> anyhow::Result<String> {
    let list = catalog(paths, refresh)?;
    let e = list.iter().find(|e| e.file == file).ok_or_else(|| anyhow!("{file} is not in OsmAnd's list"))?;
    let p: Vec<&str> = e.date.split('.').collect();
    match p.as_slice() {
        [d, m, y] => Ok(format!("obf {y}-{m}-{d}")),
        _ => Ok(format!("obf {}", e.date)),
    }
}

/// The `.obf` inside a downloaded `.obf.zip` (unpacked into `downloads/obf/`); `true` = a
/// temporary copy the caller deletes when done.
pub fn unpack(paths: &DataPaths, src: &Path, progress: &dyn Fn(u64, u64) -> bool) -> anyhow::Result<(PathBuf, bool)> {
    unpacked(paths, src, progress)
}

/// Work folder for OsmAnd downloads and conversions (`downloads/obf/`).
pub fn work_folder(paths: &DataPaths) -> PathBuf {
    work_dir(paths)
}

/// Download one file of the list into `downloads/obf/` (resumable; cancel stops it).
pub fn download(paths: &DataPaths, file: &str, progress: crate::types::ProgressFn, cancel: &Cancel) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(file.ends_with(".obf.zip") && file.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')), "invalid OsmAnd file name {file:?}");
    let dest = work_dir(paths).join(file);
    download::ensure_fresh(&format!("{DOWNLOAD_URL}{file}"), &dest, progress, cancel)?;
    Ok(dest)
}

/// The `.obf` inside `src` (a `.obf.zip` is unpacked into `downloads/obf/`). Returns the path and
/// whether it is a temporary copy to delete afterwards.
fn unpacked(paths: &DataPaths, src: &Path, progress: &dyn Fn(u64, u64) -> bool) -> anyhow::Result<(PathBuf, bool)> {
    let is_zip = src.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Ok((src.to_path_buf(), false));
    }
    let mut zip = zip::ZipArchive::new(std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?).context("הקובץ אינו ZIP תקין")?;
    let idx = (0..zip.len())
        .find(|&i| zip.by_index(i).map(|e| e.name().to_lowercase().ends_with(".obf")).unwrap_or(false))
        .ok_or_else(|| anyhow!("אין קובץ .obf בתוך {}", src.display()))?;
    let mut e = zip.by_index(idx)?;
    let total = e.size();
    let dir = work_dir(paths);
    std::fs::create_dir_all(&dir)?;
    super::disk::ensure_free(&dir, total)?;
    let out = dir.join(format!("{}.obf", stem(&src.to_string_lossy())));
    let tmp = out.with_extension("obf.tmp");
    {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut buf = vec![0u8; 4 << 20];
        let mut done = 0u64;
        loop {
            let n = e.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut w, &buf[..n])?;
            done += n as u64;
            if !progress(done, total) {
                drop(w);
                let _ = std::fs::remove_file(&tmp);
                bail!("cancelled");
            }
        }
        std::io::Write::flush(&mut w)?;
    }
    atomic_replace(&out, &tmp)?;
    Ok((out, true))
}

/// What happened to one import.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub id: String,
    pub name: String,
    /// the same area was already installed and the policy was Skip
    pub skipped: bool,
    /// installed countries the caller must delete now (policy Replace)
    pub replace: Vec<String>,
    pub conflicts: Vec<RegionConflict>,
}

/// Import an OsmAnd file (`.obf` or `.obf.zip`) as country `id` (normally [`region_id`]).
/// Progress goes out as `RegionProgress` for the parts map / search / routing.
pub fn import(
    paths: &DataPaths,
    src: &Path,
    id: &str,
    policy: ConflictPolicy,
    emit: &(dyn Fn(RegionProgress) + Sync),
    cancel: &Cancel,
) -> anyhow::Result<ImportOutcome> {
    anyhow::ensure!(regions::valid_region_id(id), "invalid region id {id:?}");
    let send = |part: RegionPart, phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
        cancel.wait_if_paused();
        emit(RegionProgress { id: id.to_string(), part, phase, done, total, message: msg.to_string() });
        !cancel.is_cancelled()
    };
    let all = |phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
        let mut ok = true;
        for p in [RegionPart::Map, RegionPart::Search, RegionPart::Routing] {
            ok &= send(p, phase, done, total, msg);
        }
        ok
    };
    all(ProgressPhase::Processing, 0, 0, "פותח את הקובץ");
    let (obf, temp_obf) = unpacked(paths, src, &|d, t| all(ProgressPhase::Processing, d, t, "מחלץ את הקובץ"))?;
    let cleanup_obf = || {
        if temp_obf {
            let _ = std::fs::remove_file(&obf);
        }
    };
    let info = match adapter::info(&obf) {
        Ok(i) => i,
        Err(e) => {
            cleanup_obf();
            return Err(e.context("הקובץ אינו קובץ מפה תקין של OsmAnd"));
        }
    };
    let name = if info.name.trim().is_empty() { split_name(&stem(&src.to_string_lossy())).0 } else { info.name.clone() };
    let conflicts: Vec<RegionConflict> = regions::same_area_regions(paths, &info.bbox, id)
        .into_iter()
        .map(|(id, name, source)| RegionConflict { id, name, source })
        .collect();
    let builtin = conflicts.iter().any(|c| c.source.as_deref() == Some("builtin"));
    if !conflicts.is_empty() && (policy == ConflictPolicy::Skip || (builtin && policy == ConflictPolicy::Replace)) {
        cleanup_obf();
        for p in [RegionPart::Map, RegionPart::Search, RegionPart::Routing] {
            send(p, ProgressPhase::Done, 0, 0, "");
        }
        return Ok(ImportOutcome { id: id.into(), name, skipped: true, replace: vec![], conflicts });
    }

    let dir = paths.region_dir(id);
    std::fs::create_dir_all(&dir)?;
    let pbf = work_dir(paths).join(format!("{}.osm.pbf", id.replace('/', "_")));
    let result = (|| -> anyhow::Result<(bool, bool, bool)> {
        // 1. OBF → PBF (feeds search and routing)
        let both = |phase: ProgressPhase, done: u64, total: u64, msg: &str| send(RegionPart::Search, phase, done, total, msg) & send(RegionPart::Routing, phase, done, total, msg);
        // 2. map tiles in parallel with the PBF + its two builds
        let (map_r, osm_r) = std::thread::scope(|s| {
            let map = s.spawn(|| -> anyhow::Result<()> {
                let p = |phase: ProgressPhase, done: u64, total: u64, msg: &str| send(RegionPart::Map, phase, done, total, msg);
                let out = regions::map_file(&dir);
                let tmp = out.with_extension("pmtiles.tmp");
                let _ = std::fs::remove_file(&tmp);
                adapter::tiles(&obf, &tmp, &p)?;
                atomic_replace(&out, &tmp)?;
                send(RegionPart::Map, ProgressPhase::Done, 1, 1, "");
                Ok(())
            });
            let osm = s.spawn(|| -> (anyhow::Result<()>, anyhow::Result<()>) {
                if let Err(e) = adapter::to_pbf(&obf, &pbf, &both) {
                    let m = format!("{e:#}");
                    return (Err(anyhow!("{m}")), Err(anyhow!("{m}")));
                }
                let search = (|| -> anyhow::Result<()> {
                    let p = |phase: ProgressPhase, done: u64, total: u64, msg: &str| send(RegionPart::Search, phase, done, total, msg);
                    let out = regions::geocode_file(&dir);
                    let tmp = out.with_extension("sqlite.tmp");
                    let _ = std::fs::remove_file(&tmp);
                    crate::geocode::build_region(&pbf, &tmp, (info.bbox[1] + info.bbox[3]) / 2.0, &p)?;
                    atomic_replace(&out, &tmp)?;
                    send(RegionPart::Search, ProgressPhase::Done, 1, 1, "");
                    Ok(())
                })();
                let routing = (|| -> anyhow::Result<()> {
                    cancel.check()?;
                    let p = |phase: ProgressPhase, done: u64, total: u64, msg: &str| send(RegionPart::Routing, phase, done, total, msg);
                    let out = regions::graph_file(&dir);
                    let tmp = out.with_extension("bin.tmp");
                    let _ = std::fs::remove_file(&tmp);
                    crate::routing::build_graph(&pbf, &tmp, &p)?;
                    atomic_replace(&out, &tmp)?;
                    send(RegionPart::Routing, ProgressPhase::Done, 1, 1, "");
                    Ok(())
                })();
                (search, routing)
            });
            (
                map.join().unwrap_or_else(|_| Err(anyhow!("map job panicked"))),
                osm.join().unwrap_or_else(|_| (Err(anyhow!("search job panicked")), Err(anyhow!("routing job panicked")))),
            )
        });
        let (search_r, routing_r) = osm_r;
        let fail = |part: RegionPart, r: &anyhow::Result<()>| {
            if let Err(e) = r {
                let phase = if cancel.is_cancelled() { ProgressPhase::Cancelled } else { ProgressPhase::Failed };
                send(part, phase, 0, 0, &format!("{e:#}"));
            }
        };
        fail(RegionPart::Map, &map_r);
        fail(RegionPart::Search, &search_r);
        fail(RegionPart::Routing, &routing_r);
        let parts = (map_r.is_ok(), search_r.is_ok(), routing_r.is_ok());
        if !(parts.0 || parts.1 || parts.2) {
            return Err(map_r.err().or(search_r.err()).unwrap_or_else(|| anyhow!("import failed")));
        }
        Ok(parts)
    })();
    let _ = std::fs::remove_file(&pbf);
    let _ = std::fs::remove_file(download::meta_path(&pbf));
    cleanup_obf();
    let (map, search, routing) = match result {
        Ok(p) => p,
        Err(e) => {
            // a first import that produced nothing leaves no folder behind
            if !dir.join("region.json").exists() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    };
    regions::write_osmand_meta(paths, &regions::OsmandMeta { id, name: &name, bbox: info.bbox, version: &info.version, map, search, routing })?;
    let replace = if policy == ConflictPolicy::Replace { conflicts.iter().filter(|c| c.source.as_deref() != Some("builtin")).map(|c| c.id.clone()).collect() } else { vec![] };
    Ok(ImportOutcome { id: id.into(), name, skipped: false, replace, conflicts })
}

/// The bridge to the OBF reader (`crate::obf`).
pub mod adapter {
    use std::path::Path;

    use crate::types::ProgressFn;

    pub struct Info {
        /// west, south, east, north
        pub bbox: [f64; 4],
        pub name: String,
        /// "obf YYYY-MM-DD"
        pub version: String,
    }

    use crate::obf::reader::ObfFile;

    pub fn info(obf: &Path) -> anyhow::Result<Info> {
        let f = ObfFile::open(obf)?;
        let date = chrono::DateTime::from_timestamp_millis(f.date_created).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "unknown".into());
        let bbox = f.bbox().ok_or_else(|| anyhow::anyhow!("no map data in the file"))?;
        Ok(Info { bbox, name: f.display_name(), version: format!("obf {date}") })
    }

    pub fn to_pbf(obf: &Path, pbf: &Path, progress: ProgressFn) -> anyhow::Result<()> {
        let f = ObfFile::open(obf)?;
        let s = crate::obf::to_pbf::convert(&f, pbf, progress)?;
        log::info!("OBF → PBF {}: {s:?}", obf.display());
        Ok(())
    }

    pub fn tiles(obf: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<()> {
        let f = ObfFile::open(obf)?;
        let opts = crate::obf::tiles::TileOptions { name: f.display_name(), ..Default::default() };
        let s = crate::obf::tiles::build(&f, out, opts, progress)?;
        log::info!("OBF tiles {}: {} tiles, {} bytes", obf.display(), s.tiles, s.bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ids() {
        assert_eq!(stem("Burundi_africa_2.obf.zip"), "Burundi_africa");
        assert_eq!(stem(r"C:\x\Burundi_africa_2.obf"), "Burundi_africa");
        assert_eq!(region_id("Us_california_san-diego_northamerica_2.obf.zip"), "osmand/us_california_san-diego_northamerica");
        assert!(regions::valid_region_id(&region_id("Weird name (1).obf")));
        assert_eq!(split_name("Us_california_san-diego_northamerica"), ("Us california san-diego".into(), "northamerica".into()));
        assert_eq!(split_name("Burundi_africa"), ("Burundi".into(), "africa".into()));
    }

    /// Whole import of a real OsmAnd file into a temporary data folder:
    /// `cargo test --release --features cli --lib obf_import::tests::real_burundi -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_burundi() {
        let src = dirs::download_dir().unwrap().join("Compressed").join("Burundi_africa_2.obf.zip");
        assert!(src.exists(), "{} missing", src.display());
        let root = std::env::temp_dir().join(format!("mo-obf-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = DataPaths::new(root.clone());
        paths.ensure_dirs().unwrap();
        let t0 = std::time::Instant::now();
        let last = parking_lot::Mutex::new(String::new());
        let emit = |p: RegionProgress| {
            let line = format!("{:?} {:?} {}", p.part, p.phase, p.message);
            let mut l = last.lock();
            if *l != line {
                eprintln!("{line}");
                *l = line;
            }
        };
        let id = region_id("Burundi_africa_2.obf.zip");
        let out = import(&paths, &src, &id, ConflictPolicy::Replace, &emit, &Cancel::default()).unwrap();
        eprintln!("import: {:?} in {:?}", out, t0.elapsed());
        let dir = paths.region_dir(&id);
        for f in ["map.pmtiles", "geocode.sqlite", "graph.bin", "region.json"] {
            let m = std::fs::metadata(dir.join(f)).unwrap_or_else(|_| panic!("{f} missing"));
            eprintln!("{f}: {} KB", m.len() / 1024);
        }
        let inst = regions::installed(&paths, &|_| false);
        assert_eq!(inst.len(), 1);
        let r = &inst[0];
        eprintln!("installed: {} {:?} map={:?} search={:?} routing={:?}", r.name, r.bbox, r.map.version, r.search.version, r.routing.version);
        assert!(r.map.present && r.search.present && r.routing.present);
        // a tile in central Bujumbura (29.36E, -3.38) at zoom 14 has roads
        let (z, lon, lat) = (14u8, 29.36f64, -3.38f64);
        let n = 2f64.powi(z as i32);
        let x = ((lon + 180.0) / 360.0 * n) as u32;
        let y = ((1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / std::f64::consts::PI) / 2.0 * n) as u32;
        let tile = crate::map_tiles::read_tile_uncached(&regions::map_file(&dir), z, x, y).unwrap().expect("tile in Bujumbura");
        eprintln!("tile {z}/{x}/{y}: {} bytes", tile.len());
        // search and routing through the app's own readers
        let g = crate::geocode::open(&regions::geocode_file(&dir)).unwrap();
        let hits = g.search("Bujumbura", Some([29.36, -3.38]), 5).unwrap();
        eprintln!("search: {:?}", hits.iter().map(|h| (&h.name, h.lon, h.lat)).collect::<Vec<_>>());
        assert!(!hits.is_empty());
        let graph = crate::routing::load_graph(&regions::graph_file(&dir)).unwrap();
        let req: crate::types::RouteRequest = serde_json::from_value(serde_json::json!({
            "waypoints": [{"lon": 29.3599, "lat": -3.3822}, {"lon": 29.3833, "lat": -3.3614}],
            "mode": "car", "time": {"kind": "now"}, "avoid": {}, "alternatives": 0
        }))
        .unwrap();
        let route = graph.route(&req).unwrap();
        eprintln!("route: {} m, {} s", route.routes[0].distance_m, route.routes[0].duration_sec);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Israel from OsmAnd through the FULL Israel builders (what Settings.mapSource = "osmand" does):
    /// `MO_OBF_ISRAEL=<Israel_asia_2.obf.zip> cargo test --release --features cli --lib obf_import::tests::real_israel -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_israel() {
        let src = PathBuf::from(std::env::var("MO_OBF_ISRAEL").expect("MO_OBF_ISRAEL"));
        let root = std::env::temp_dir().join(format!("mo-obf-israel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = DataPaths::new(root.clone());
        paths.ensure_dirs().unwrap();
        let quiet = |_: ProgressPhase, _: u64, _: u64, _: &str| true;
        let t0 = std::time::Instant::now();
        let (obf, _) = unpack(&paths, &src, &|_, _| true).unwrap();
        let pbf = root.join("israel.osm.pbf");
        adapter::to_pbf(&obf, &pbf, &quiet).unwrap();
        eprintln!("pbf {} MB in {:?}", std::fs::metadata(&pbf).unwrap().len() >> 20, t0.elapsed());
        let t1 = std::time::Instant::now();
        crate::geocode::build(&pbf, &paths.geocode_file(), &quiet).unwrap();
        eprintln!("geocode {} MB in {:?}", std::fs::metadata(paths.geocode_file()).unwrap().len() >> 20, t1.elapsed());
        let t2 = std::time::Instant::now();
        crate::routing::build_graph(&pbf, &paths.graph_file(), &quiet).unwrap();
        eprintln!("graph {} MB in {:?}", std::fs::metadata(paths.graph_file()).unwrap().len() >> 20, t2.elapsed());
        let t3 = std::time::Instant::now();
        adapter::tiles(&obf, &paths.tiles_file(), &quiet).unwrap();
        eprintln!("tiles {} MB in {:?}", std::fs::metadata(paths.tiles_file()).unwrap().len() >> 20, t3.elapsed());
        let g = crate::geocode::open(&paths.geocode_file()).unwrap();
        for q in ["הרצל 12 תל אביב", "יפו 97 ירושלים", "רחוב ביאליק רמת גן", "בית חולים", "תחנת דלק"] {
            let hits = g.search(q, Some([34.78, 32.08]), 3).unwrap();
            eprintln!("{q}: {:?}", hits.iter().map(|h| (&h.name, &h.city, h.lon, h.lat)).collect::<Vec<_>>());
            assert!(!hits.is_empty(), "{q}");
        }
        let graph = crate::routing::load_graph(&paths.graph_file()).unwrap();
        let req: crate::types::RouteRequest = serde_json::from_value(serde_json::json!({
            "waypoints": [{"lon": 34.7818, "lat": 32.0853}, {"lon": 35.2137, "lat": 31.7683}],
            "mode": "car", "time": {"kind": "now"}, "avoid": {"areaA": true, "areaB": true}, "alternatives": 0
        }))
        .unwrap();
        let r = graph.route(&req).unwrap();
        eprintln!("Tel Aviv → Jerusalem: {:.1} km, {:.0} min; areas known: {}", r.routes[0].distance_m / 1000.0, r.routes[0].duration_sec / 60.0, !graph.areas.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `cargo test --features cli --lib obf_import::tests::real_catalog -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_catalog() {
        let root = std::env::temp_dir().join(format!("mo-obf-cat-{}", std::process::id()));
        let paths = DataPaths::new(root.clone());
        paths.ensure_dirs().unwrap();
        let list = catalog(&paths, true).unwrap();
        eprintln!("{} maps; first {:?}", list.len(), list.first());
        assert!(list.iter().any(|e| e.region_id == "osmand/burundi_africa"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_the_list() {
        let mut xml = String::from(r#"<?xml version="1.0"?><osmand_regions mapversion="1">"#);
        for i in 0..60 {
            xml.push_str(&format!(r#"<region type="map" date="01.11.2026" containerSize="{}" contentSize="9" name="Land{i}_europe_2.obf.zip"/>"#, 1000 + i));
        }
        xml.push_str(r#"<region type="map" name="World_basemap_2.obf.zip" containerSize="1"/><region type="voice" name="he_0.voice.zip"/><region type="map" name="World_seamarks_2.obf.zip"/></osmand_regions>"#);
        let v = parse_catalog(&xml).unwrap();
        assert_eq!(v.len(), 60);
        assert_eq!(v[0].continent, "europe");
        assert!(v.iter().all(|e| e.size_bytes >= 1000 && e.region_id.starts_with("osmand/land")));
    }
}
