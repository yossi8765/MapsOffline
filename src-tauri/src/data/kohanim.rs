//! "Route for kohanim": places a kohen must not pass (graves under or beside the road), from the
//! Tahara of Kohanim association's map (gps.hakohanim.org, `GET /api/areas`). Three levels, each
//! one stricter than the last: 1 = basic (א), 2 = the association's standard (ב), 3 = the strict
//! opinions (ג); a route at level N avoids every area of level ≤ N. Their file also carries the
//! Oslo Area A polygons (level "A"), which the app has on its own – those are left out.
//!
//! The list is kept in `kohanim/areas.json`, a data component like the others (Data panel row,
//! weekly ready-made package, `.mapsdata` transfer to offline computers). It is also refreshed
//! from the site when a route needs it and it is older than three days (only when going online is
//! allowed); without a connection the kept copy is used. Manifest version `hakohanim YYYY-MM-DD`.
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Serialize;

use super::Cancel;
use crate::paths::{atomic_replace, DataPaths};
use crate::types::{DataComponent, LonLat, ProgressFn, ProgressPhase};

pub const AREAS_URL: &str = "https://gps.hakohanim.org/api/areas";
const REFRESH_AFTER: Duration = Duration::from_secs(3 * 24 * 3600);
/// The Data panel offers an update once the installed list is this old.
const STALE_DAYS: i64 = 3;

/// One area to avoid.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Area {
    /// 1 = basic (א), 2 = association standard (ב), 3 = strict (ג)
    pub level: u8,
    pub name: String,
    pub city: String,
    pub street: String,
    pub description: String,
    /// outer ring, lon/lat
    pub ring: Vec<LonLat>,
}

pub fn file(paths: &DataPaths) -> PathBuf {
    paths.root.join("kohanim").join("areas.json")
}

fn level_of(s: &str) -> Option<u8> {
    match s.trim() {
        "א" => Some(1),
        "ב" => Some(2),
        "ג" => Some(3),
        _ => None, // "A" = Oslo Area A, handled by the app's own territory data
    }
}

/// Parse the site's answer: `{success, data: FeatureCollection}` (or a bare FeatureCollection).
pub fn parse(json: &str) -> anyhow::Result<Vec<Area>> {
    let v: serde_json::Value = serde_json::from_str(json).context("kohanim areas: not JSON")?;
    let fc = if v.get("data").is_some_and(|d| d.get("features").is_some()) { &v["data"] } else { &v };
    let Some(features) = fc.get("features").and_then(|f| f.as_array()) else { bail!("kohanim areas: no features") };
    let mut out = Vec::new();
    for f in features {
        let p = &f["properties"];
        let Some(level) = p["level"].as_str().and_then(level_of) else { continue };
        let text = |k: &str| p[k].as_str().unwrap_or("").to_string();
        let g = &f["geometry"];
        // Polygon: outer ring; MultiPolygon: the outer ring of each part
        let rings: Vec<&serde_json::Value> = match g["type"].as_str() {
            Some("Polygon") => g["coordinates"].as_array().and_then(|r| r.first()).into_iter().collect(),
            Some("MultiPolygon") => g["coordinates"].as_array().map(|ps| ps.iter().filter_map(|p| p.as_array().and_then(|r| r.first())).collect()).unwrap_or_default(),
            _ => vec![],
        };
        for r in rings {
            let ring: Vec<LonLat> = r
                .as_array()
                .map(|pts| pts.iter().filter_map(|c| Some([c.get(0)?.as_f64()?, c.get(1)?.as_f64()?])).filter(|c| c[0].is_finite() && c[1].is_finite()).collect())
                .unwrap_or_default();
            if ring.len() >= 3 {
                out.push(Area { level, name: text("name"), city: text("city"), street: text("street"), description: text("description"), ring });
            }
        }
    }
    if out.is_empty() {
        bail!("kohanim areas: the list is empty");
    }
    Ok(out)
}

/// Fetch the list from the site and keep it (data flow: the only network use of this module).
pub fn refresh(paths: &DataPaths) -> anyhow::Result<Vec<Area>> {
    let r = super::download::http_client()?.get(AREAS_URL).timeout(Duration::from_secs(20)).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", super::download::blocked_message(AREAS_URL));
    }
    let body = r.error_for_status().context("gps.hakohanim.org")?.text()?;
    let areas = parse(&body)?;
    let f = file(paths);
    if let Some(d) = f.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = f.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes())?;
    atomic_replace(&f, &tmp)?;
    if let Err(e) = super::manifest::Manifest::record(paths, DataComponent::Kohanim, Some(version_today())) {
        log::warn!("kohanim areas: manifest not updated: {e:#}");
    }
    log::info!("kohanim areas: {} from the site", areas.len());
    Ok(areas)
}

fn today() -> chrono::NaiveDate {
    chrono::Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).date_naive()
}

fn version_today() -> String {
    format!("hakohanim {}", today().format("%Y-%m-%d"))
}

fn fetched_on(version: Option<&str>) -> Option<chrono::NaiveDate> {
    version.and_then(|v| v.strip_prefix("hakohanim ")).and_then(|d| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
}

/// Is the installed list older than [`STALE_DAYS`] (or missing / of unknown age)?
pub fn is_stale(version: Option<&str>) -> bool {
    fetched_on(version).is_none_or(|d| (today() - d).num_days() >= STALE_DAYS)
}

/// Update-flow job (Data panel, CLI `build --kohanim`, weekly CI): fetch the list unless it was
/// already fetched today.
pub fn update_kohanim(paths: &DataPaths, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    let installed = super::manifest::Manifest::load(paths).entry(DataComponent::Kohanim).and_then(|e| e.version.clone());
    if file(paths).exists() && fetched_on(installed.as_deref()) == Some(today()) {
        progress(ProgressPhase::Processing, 1, 1, "רשימת האזורים לכהנים כבר מעודכנת");
        return Ok(installed);
    }
    if cancel.is_cancelled() {
        bail!("cancelled");
    }
    progress(ProgressPhase::Downloading, 0, 0, "מוריד את רשימת האזורים לכהנים");
    let areas = refresh(paths)?;
    progress(ProgressPhase::Finalizing, 1, 1, &format!("{} אזורים", areas.len()));
    Ok(Some(version_today()))
}

/// The areas: the kept copy, refreshed first when it is old (or missing) and `online` allows.
/// Fails only when there is no copy at all and the site cannot be reached.
pub fn load(paths: &DataPaths, online: bool) -> anyhow::Result<Vec<Area>> {
    let f = file(paths);
    let age = std::fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if online && age.is_none_or(|a| a > REFRESH_AFTER) {
        match refresh(paths) {
            Ok(a) => return Ok(a),
            Err(e) => log::info!("kohanim areas: refresh failed ({e:#}), using the kept copy"),
        }
    }
    let body = std::fs::read_to_string(&f).map_err(|_| anyhow::anyhow!("KOHANIM_NO_DATA"))?;
    parse(&body)
}

/// Rings to avoid for a route at `level` (1..=3): every area of that level or a milder one.
pub fn rings_for(areas: &[Area], level: u8) -> Vec<Vec<LonLat>> {
    areas.iter().filter(|a| a.level <= level).map(|a| a.ring.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_oslo_left_out() {
        let json = r#"{"success":true,"data":{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"a","city":"טבריה","street":"x","description":"d","level":"א"},"geometry":{"type":"Polygon","coordinates":[[[35.5,32.7],[35.6,32.7],[35.6,32.8],[35.5,32.7]]]}},
            {"type":"Feature","properties":{"name":"b","city":"","street":"","description":"","level":"ב"},"geometry":{"type":"Polygon","coordinates":[[[35,32],[35.1,32],[35.1,32.1],[35,32]]]}},
            {"type":"Feature","properties":{"name":"c","city":"","street":"","description":"","level":"ג"},"geometry":{"type":"MultiPolygon","coordinates":[[[[34,31],[34.1,31],[34.1,31.1],[34,31]]],[[[34.2,31],[34.3,31],[34.3,31.1],[34.2,31]]]]}},
            {"type":"Feature","properties":{"name":"Oslo Area A #1","city":"","street":"","description":"","level":"A"},"geometry":{"type":"Polygon","coordinates":[[[35,31],[35.1,31],[35.1,31.1],[35,31]]]}}
        ]}}"#;
        let a = parse(json).unwrap();
        assert_eq!(a.len(), 4); // א, ב, and the two parts of ג
        assert_eq!(a[0].level, 1);
        assert_eq!(rings_for(&a, 1).len(), 1);
        assert_eq!(rings_for(&a, 2).len(), 2);
        assert_eq!(rings_for(&a, 3).len(), 4);
        assert!(parse(r#"{"data":{"features":[]}}"#).is_err());
    }

    /// Real areas + a real Israel graph: routes in Tiberias, with and without the kohanim levels.
    /// `MO_KOHANIM_AREAS=<areas.json> MO_OBF_ISRAEL=<Israel_asia_2.obf.zip> cargo test --release --features cli --lib kohanim::tests::real_tiberias -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_tiberias() {
        let areas = parse(&std::fs::read_to_string(std::env::var("MO_KOHANIM_AREAS").unwrap()).unwrap()).unwrap();
        eprintln!("areas: {} (level 1: {}, 2: {}, 3: {})", areas.len(), rings_for(&areas, 1).len(), rings_for(&areas, 2).len(), rings_for(&areas, 3).len());
        let root = std::env::temp_dir().join(format!("mo-kohanim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = DataPaths::new(root.clone());
        paths.ensure_dirs().unwrap();
        let quiet = |_: crate::types::ProgressPhase, _: u64, _: u64, _: &str| true;
        let (obf, _) = super::super::obf_import::unpack(&paths, std::path::Path::new(&std::env::var("MO_OBF_ISRAEL").unwrap()), &|_, _| true).unwrap();
        let pbf = root.join("il.pbf");
        super::super::obf_import::adapter::to_pbf(&obf, &pbf, &quiet).unwrap();
        crate::routing::build_graph(&pbf, &paths.graph_file(), &quiet).unwrap();
        let g = crate::routing::load_graph(&paths.graph_file()).unwrap();
        // inside a ring (even-odd)
        let inside = |p: LonLat, r: &[LonLat]| {
            let mut c = false;
            let mut j = r.len() - 1;
            for i in 0..r.len() {
                let (a, b) = (r[i], r[j]);
                if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
                    c = !c;
                }
                j = i;
            }
            c
        };
        // across old Tiberias (the Rambam area), and straight across the first level-א areas
        let mut pairs: Vec<(LonLat, LonLat)> = vec![([35.5300, 32.7930], [35.5420, 32.7780]), ([35.5275, 32.7955], [35.5360, 32.7845])];
        for a in areas.iter().filter(|a| a.level == 1).take(6) {
            let (mut w, mut e, mut s, mut n) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for p in &a.ring {
                w = w.min(p[0]);
                e = e.max(p[0]);
                s = s.min(p[1]);
                n = n.max(p[1]);
            }
            // the long side of the box, 60 m beyond each end
            let pad = 0.0006;
            pairs.push(if e - w > n - s { ([w - pad, (s + n) / 2.0], [e + pad, (s + n) / 2.0]) } else { ([(w + e) / 2.0, s - pad], [(w + e) / 2.0, n + pad]) });
        }
        for (from, to) in pairs {
            for level in 0u8..=3 {
                let mut req: crate::types::RouteRequest = serde_json::from_value(serde_json::json!({
                    "waypoints": [{"lon": from[0], "lat": from[1]}, {"lon": to[0], "lat": to[1]}],
                    "mode": "car", "time": {"kind": "now"}, "avoid": {}, "alternatives": 0
                }))
                .unwrap();
                req.avoid.polygons = rings_for(&areas, level);
                let r = g.route(&req);
                match r {
                    Ok(resp) => {
                        let route = &resp.routes[0];
                        let hits = rings_for(&areas, level.max(1)).iter().filter(|ring| route.geometry.iter().any(|p| inside(*p, ring))).count();
                        eprintln!("{from:?}→{to:?} level {level}: {:.0} m, {:.0} s, passes through {hits} areas of level ≤ {}", route.distance_m, route.duration_sec, level.max(1));
                        if level > 0 {
                            assert_eq!(hits, 0, "level {level} route enters a forbidden area");
                        }
                    }
                    Err(e) => eprintln!("level {level}: {e:#}"),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
