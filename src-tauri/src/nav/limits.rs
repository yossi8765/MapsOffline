//! Posted speed limit where the car is, for routes whose steps carry none (online OSRM routes:
//! the public servers do not return `maxspeed`). Looked up by position and heading in the local
//! road graph when it is installed; else in OpenStreetMap: Overpass requests by ~2 km cells along
//! the route bring every way with a speed limit (fetched in the background when the route is set).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::geo::{bearing_deg, haversine_m, point_segment_m};
use crate::routing::graph::{parse_maxspeed, Class, Graph};
use crate::state::AppState;
use crate::types::LonLat;

/// Public Overpass servers, tried in turn (the main one often answers "at capacity").
const OVERPASS: [&str; 2] = ["https://overpass.kumi.systems/api/interpreter", "https://overpass-api.de/api/interpreter"];
/// A road farther than this from the fix is not "the road being driven".
const MATCH_M: f64 = 25.0;
/// With a heading, the road must run along it (or against it) within this angle.
const ALONG_DEG: f64 = 40.0;

/// One road: polyline (way order) + limit in that order / against it.
struct Way {
    pts: Vec<LonLat>,
    fwd: Option<u8>,
    bwd: Option<u8>,
}

#[derive(Default)]
struct Along {
    /// Route generation the ways belong to.
    gen: Option<u64>,
    ways: Vec<Way>,
    /// Cells already fetched (a new route after a detour only asks for the new ones).
    done: std::collections::HashSet<Cell>,
    busy: bool,
    retry_at: Option<Instant>,
    /// Failures in a row: the wait before the next try doubles (30 s … 8 min).
    failures: u32,
}

pub struct Lookup {
    app: AppHandle,
    last: Option<(LonLat, Option<u32>)>,
    along: Arc<Mutex<Along>>,
    graph_load_started: bool,
    /// graph.bin is installed (checked once per session).
    has_graph: Option<bool>,
}

/// Nearest polyline segment to `p`: (distance m, bearing of the segment in polyline order).
fn nearest_segment(pts: &[LonLat], p: LonLat) -> Option<(f64, f64)> {
    pts.windows(2)
        .map(|w| (point_segment_m(p, w[0], w[1]).0, w))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(d, w)| (d, bearing_deg(w[0], w[1])))
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The limit of the road matched to `p`, in the driving direction: with no heading only when
/// both directions agree. None = no road matched.
fn pick(cands: impl Iterator<Item = (f64, f64, Option<u8>, Option<u8>)>, heading: Option<f64>) -> Option<Option<u32>> {
    let (_, bear, fwd, bwd) = cands
        .filter(|c| c.0 <= MATCH_M)
        // a crossing street at a junction is near too: it must run along the heading
        .filter(|c| heading.is_none_or(|h| angle_diff(h, c.1).min(180.0 - angle_diff(h, c.1)) <= ALONG_DEG))
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let lim = match heading {
        Some(h) if angle_diff(h, bear) <= 90.0 => fwd,
        Some(_) => bwd,
        None if fwd == bwd => fwd,
        None => None,
    };
    Some(lim.filter(|&l| l > 0).map(u32::from))
}

fn drivable(c: Class) -> bool {
    !matches!(c, Class::Track | Class::Pedestrian | Class::Footway | Class::Path | Class::Cycleway | Class::Steps | Class::Other)
}

fn from_graph(g: &Graph, p: LonLat, heading: Option<f64>) -> Option<Option<u32>> {
    if !crate::geo::in_bbox(p, g.bbox) {
        return None;
    }
    let cands = g.snap_candidates(p, &|e| drivable(g.class(e)), 4);
    pick(
        cands.into_iter().filter_map(|s| {
            let (d, b) = nearest_segment(&g.edge_points(s.edge), p)?;
            Some((d, b, g.limit_s(Graph::state(s.edge, 0)), g.limit_s(Graph::state(s.edge, 1))))
        }),
        heading,
    )
}

fn parse_overpass(body: &Value) -> Vec<Way> {
    let tag = |t: &Value, k: &str| t.get(k).and_then(Value::as_str).and_then(parse_maxspeed);
    body.get("elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|el| {
            let pts: Vec<LonLat> = el.get("geometry")?.as_array()?.iter().filter_map(|g| Some([g.get("lon")?.as_f64()?, g.get("lat")?.as_f64()?])).collect();
            if pts.len() < 2 {
                return None;
            }
            let t = el.get("tags").cloned().unwrap_or(Value::Null);
            let both = tag(&t, "maxspeed").or_else(|| tag(&t, "maxspeed:type")).or_else(|| tag(&t, "source:maxspeed")).or_else(|| tag(&t, "zone:maxspeed"));
            let fwd = tag(&t, "maxspeed:forward").or(both);
            let bwd = tag(&t, "maxspeed:backward").or(both);
            (fwd.is_some() || bwd.is_some()).then_some(Way { pts, fwd, bwd })
        })
        .collect()
}

/// Overpass is asked by small squares along the route (a bbox query is fast; `around` a long
/// line is not): ~2 km cells, in route order.
const CELL_DEG: f64 = 0.02;
/// Cells per request (sent from the start of the route on, so the first stretch comes first).
const CELLS_PER_QUERY: usize = 12;

type Cell = (i32, i32);

fn route_cells(pts: &[LonLat]) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    let mut add = |p: LonLat| {
        let c = ((p[0] / CELL_DEG).floor() as i32, (p[1] / CELL_DEG).floor() as i32);
        if !out.contains(&c) {
            out.push(c);
        }
    };
    for w in pts.windows(2) {
        // long straight segments: sample them so no cell they cross is skipped
        let n = (haversine_m(w[0], w[1]) / 500.0).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f64 / n as f64;
            add([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
        }
    }
    if let Some(&p) = pts.last() {
        add(p);
    }
    out
}

fn overpass_query(cells: &[Cell]) -> String {
    let parts: String = cells
        .iter()
        .map(|&(x, y)| {
            let (w, s) = (x as f64 * CELL_DEG, y as f64 * CELL_DEG);
            format!("way({s:.4},{w:.4},{:.4},{:.4})[highway][~\"^(maxspeed|maxspeed:forward|maxspeed:backward|maxspeed:type|source:maxspeed|zone:maxspeed)$\"~\".\"];", s + CELL_DEG, w + CELL_DEG)
        })
        .collect();
    format!("[out:json][timeout:25];({parts});out tags geom;")
}

fn fetch(cells: &[Cell]) -> anyhow::Result<Vec<Way>> {
    let q = overpass_query(cells);
    let client = reqwest::blocking::Client::builder().user_agent(crate::online::USER_AGENT).connect_timeout(Duration::from_secs(6)).timeout(Duration::from_secs(30)).build()?;
    let mut errors = Vec::new();
    for url in OVERPASS {
        let r = client.post(url).form(&[("data", q.as_str())]).send().and_then(|r| r.error_for_status()).and_then(|r| r.json::<Value>());
        match r {
            Ok(body) => return Ok(parse_overpass(&body)),
            // the whole cause chain: a filter's refusal and a timeout read differently
            Err(e) => errors.push(format!("{url}: {:#}", anyhow::Error::from(e))),
        }
    }
    anyhow::bail!("{}", errors.join(" | "))
}

impl Lookup {
    pub fn new(app: AppHandle) -> Self {
        Lookup { app, last: None, along: Arc::default(), graph_load_started: false, has_graph: None }
    }

    /// The route being driven (generation `gen` changes with every new route): the limits of
    /// the cells it crosses that are not loaded yet are fetched in the background.
    pub fn route(&mut self, gen: u64, pts: &[LonLat]) {
        // the installed road graph has the limits: nothing to fetch
        if self.has_graph.get_or_insert_with(|| self.app.state::<Arc<AppState>>().paths().graph_file().exists()) == &true {
            return;
        }
        let mut a = self.along.lock();
        if a.gen == Some(gen) || a.busy || a.retry_at.is_some_and(|t| Instant::now() < t) || pts.len() < 2 {
            return;
        }
        let cells: Vec<Cell> = route_cells(pts).into_iter().filter(|c| !a.done.contains(c)).collect();
        if cells.is_empty() {
            a.gen = Some(gen);
            return;
        }
        a.busy = true;
        let along = self.along.clone();
        std::thread::spawn(move || {
            for batch in cells.chunks(CELLS_PER_QUERY) {
                let r = fetch(batch);
                let mut a = along.lock();
                match r {
                    Ok(ways) => {
                        log::info!("speed limits: {} ways in {} cells", ways.len(), batch.len());
                        a.ways.extend(ways);
                        a.done.extend(batch.iter().copied());
                    }
                    Err(e) => {
                        // stderr: shows in the Android log too
                        eprintln!("speed limits (Overpass): {e:#}");
                        log::warn!("speed limits (Overpass): {e:#}");
                        a.failures += 1;
                        a.retry_at = Some(Instant::now() + Duration::from_secs(30 << (a.failures - 1).min(4)));
                        a.busy = false;
                        return;
                    }
                }
            }
            let mut a = along.lock();
            a.gen = Some(gen);
            a.retry_at = None;
            a.failures = 0;
            a.busy = false;
        });
    }

    /// The limit at `p` (None = unknown). Cheap to call every second: it looks again only after
    /// ~20 m.
    pub fn get(&mut self, p: LonLat, heading: Option<f64>) -> Option<u32> {
        if let Some((q, lim)) = self.last {
            if haversine_m(p, q) < 20.0 {
                return lim;
            }
        }
        let st = self.app.state::<Arc<AppState>>();
        let graph = st.graph.read().clone();
        if graph.is_none() && !self.graph_load_started {
            // load it beside the drive (seconds), never on the navigation thread
            self.graph_load_started = true;
            let app = self.app.clone();
            std::thread::spawn(move || {
                // stderr: shows in the Android log too
                match app.state::<Arc<AppState>>().graph() {
                    Ok(_) => eprintln!("speed limits: road graph loaded"),
                    Err(e) => eprintln!("speed limits: no road graph ({e:#})"),
                }
            });
        }
        let found = graph.as_deref().and_then(|g| from_graph(g, p, heading)).or_else(|| {
            let a = self.along.lock();
            pick(a.ways.iter().filter_map(|w| nearest_segment(&w.pts, p).map(|(d, b)| (d, b, w.fwd, w.bwd))), heading)
        });
        let lim = found.flatten();
        self.last = Some((p, lim));
        lim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overpass_ways_and_direction() {
        let body: Value = serde_json::from_str(
            r#"{"elements":[
              {"type":"way","tags":{"highway":"primary","maxspeed:forward":"70","maxspeed:backward":"50"},"geometry":[{"lat":32.0,"lon":34.8},{"lat":32.0,"lon":34.81}]},
              {"type":"way","tags":{"highway":"residential","maxspeed":"IL:urban"},"geometry":[{"lat":32.01,"lon":34.8},{"lat":32.02,"lon":34.8}]},
              {"type":"way","tags":{"highway":"residential"},"geometry":[{"lat":32.03,"lon":34.8},{"lat":32.04,"lon":34.8}]}
            ]}"#,
        )
        .unwrap();
        let ways = parse_overpass(&body);
        assert_eq!(ways.len(), 2, "untagged ways are dropped");
        let near = |p: LonLat, h: Option<f64>| pick(ways.iter().filter_map(|w| nearest_segment(&w.pts, p).map(|(d, b)| (d, b, w.fwd, w.bwd))), h);
        // driving east along the first way, then west
        assert_eq!(near([34.805, 32.0001], Some(90.0)), Some(Some(70)));
        assert_eq!(near([34.805, 32.0001], Some(270.0)), Some(Some(50)));
        assert_eq!(near([34.805, 32.0001], None), Some(None));
        assert_eq!(near([34.8001, 32.015], Some(0.0)), Some(Some(50)));
        // crossing it (heading north over the east-west road): not this road
        assert_eq!(near([34.805, 32.0001], Some(0.0)), None);
        // nothing within reach
        assert_eq!(near([34.9, 32.5], Some(0.0)), None);
    }

    #[test]
    fn cells_follow_the_route() {
        // ~11 km due east: about 6 cells of 0.02°, in order, none skipped
        let cells = route_cells(&[[34.801, 32.001], [34.919, 32.001]]);
        assert_eq!(cells.first(), Some(&(1740, 1600)));
        assert_eq!(cells.len(), 6, "{cells:?}");
        assert!(cells.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        let q = overpass_query(&cells[..1]);
        assert!(q.starts_with("[out:json]") && q.contains("way(32.0000,34.8000,32.0200,34.8200)[highway]"), "{q}");
    }
}

#[cfg(test)]
mod graph_tests {
    /// Limits from the installed road graph (Ayalon / Road 20 northbound near HaShalom).
    /// `cargo test --lib nav::limits::graph_tests -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn installed_graph_limits() {
        let paths = crate::paths::DataPaths::new(crate::paths::DataPaths::default_root());
        let g = crate::routing::load_graph(&paths.graph_file()).expect("graph.bin installed");
        for (p, h) in [([34.7905, 32.0735], 15.0), ([34.7905, 32.0735], 195.0), ([34.7818, 32.0853], 0.0), ([35.0, 31.85], 90.0)] {
            println!("{p:?} heading {h}: {:?}", super::from_graph(&g, p, Some(h)));
        }
    }
}
