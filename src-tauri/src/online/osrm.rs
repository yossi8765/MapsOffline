//! OSRM HTTP API client (route service) with Hebrew instruction generation.
use std::time::Instant;

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;

use crate::geo::polyline_length_m;
use crate::routing::alternatives::resolve_times;
use crate::routing::instructions::{exit_text, instruction_text, merge_roundabout_exits};
use crate::types::*;

#[derive(Deserialize)]
struct OsrmResponse {
    code: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    routes: Vec<OsrmRoute>,
}

#[derive(Deserialize)]
struct OsrmRoute {
    distance: f64,
    duration: f64,
    geometry: Geometry,
    legs: Vec<OsrmLeg>,
}

#[derive(Deserialize)]
struct Geometry {
    coordinates: Vec<LonLat>,
}

#[derive(Deserialize)]
struct OsrmLeg {
    distance: f64,
    duration: f64,
    #[serde(default)]
    steps: Vec<OsrmStep>,
}

#[derive(Deserialize)]
struct OsrmStep {
    distance: f64,
    duration: f64,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    rotary_name: Option<String>,
    /// signposted destinations of the way taken ("Jerusalem, Tel Aviv")
    #[serde(default)]
    destinations: Option<String>,
    /// exit numbers of an off ramp
    #[serde(default)]
    exits: Option<String>,
    #[serde(default)]
    intersections: Vec<OsrmIntersection>,
    geometry: Geometry,
    maneuver: OsrmManeuver,
}

#[derive(Deserialize, Default)]
struct OsrmIntersection {
    #[serde(default)]
    lanes: Vec<OsrmLane>,
}

#[derive(Deserialize)]
struct OsrmLane {
    #[serde(default)]
    indications: Vec<String>,
    #[serde(default)]
    valid: bool,
}

#[derive(Deserialize)]
struct OsrmManeuver {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    modifier: Option<String>,
    #[serde(default)]
    exit: Option<u32>,
}

fn profile_path(mode: Mode) -> &'static str {
    match mode {
        Mode::Bike => "routed-bike",
        Mode::Foot => "routed-foot",
        _ => "routed-car",
    }
}

fn leg_kind(mode: Mode) -> LegKind {
    match mode {
        Mode::Bike => LegKind::Bike,
        Mode::Foot => LegKind::Walk,
        _ => LegKind::Drive,
    }
}

fn maneuver(m: &OsrmManeuver) -> Maneuver {
    let modifier = m.modifier.as_deref().unwrap_or("");
    let by_modifier = || match modifier {
        "uturn" => Maneuver::Uturn,
        "sharp right" => Maneuver::SharpRight,
        "right" => Maneuver::TurnRight,
        "slight right" => Maneuver::SlightRight,
        "sharp left" => Maneuver::SharpLeft,
        "left" => Maneuver::TurnLeft,
        "slight left" => Maneuver::SlightLeft,
        _ => Maneuver::Straight,
    };
    match m.kind.as_str() {
        "depart" => Maneuver::Depart,
        "arrive" => Maneuver::Arrive,
        "roundabout" | "rotary" | "roundabout turn" => Maneuver::Roundabout,
        "exit roundabout" | "exit rotary" => Maneuver::ExitRoundabout,
        "merge" | "on ramp" => Maneuver::Merge,
        "fork" | "off ramp" => {
            if modifier.is_empty() {
                Maneuver::Fork
            } else {
                by_modifier()
            }
        }
        "continue" | "new name" | "use lane" | "notification" => {
            if modifier == "uturn" {
                Maneuver::Uturn
            } else if modifier.is_empty() || modifier == "straight" {
                Maneuver::Straight
            } else {
                by_modifier()
            }
        }
        _ => by_modifier(),
    }
}

fn street_name(s: &OsrmStep) -> Option<String> {
    let name = s.name.trim();
    let r = s.reference.as_deref().map(str::trim).filter(|r| !r.is_empty());
    match (name.is_empty(), r) {
        (false, _) => Some(name.to_string()),
        (true, Some(r)) => Some(if r.chars().all(|c| c.is_ascii_digit()) { format!("כביש {r}") } else { r.to_string() }),
        (true, None) => s.rotary_name.clone(),
    }
}

/// "Jerusalem: Tel Aviv, Haifa" (ref: names) → the names part, first entries.
fn destinations_text(d: Option<&str>) -> Option<String> {
    let d = d?.trim();
    let names = d.split_once(':').map(|(_, n)| n).unwrap_or(d).trim();
    let v: Vec<&str> = names.split(',').map(str::trim).filter(|x| !x.is_empty()).take(2).collect();
    (!v.is_empty()).then(|| v.join(", "))
}

fn convert_steps(leg: &OsrmLeg) -> Vec<Step> {
    let mut out = Vec::with_capacity(leg.steps.len());
    let mut geometry_start = 0usize;
    for s in &leg.steps {
        let m = maneuver(&s.maneuver);
        let street = street_name(s);
        let exit = if m == Maneuver::Roundabout { s.maneuver.exit } else { None };
        let off_ramp = s.maneuver.kind == "off ramp";
        let junction = if off_ramp { s.exits.as_deref().and_then(|e| e.split(';').next()).map(|e| e.trim().to_string()).filter(|e| !e.is_empty()) } else { None };
        let destination = if off_ramp || s.maneuver.kind == "fork" { destinations_text(s.destinations.as_deref()) } else { None };
        let instruction = exit_text(junction.as_deref(), destination.as_deref(), street.as_deref()).unwrap_or_else(|| instruction_text(m, street.as_deref(), exit));
        let mut st = Step::basic(instruction, m, exit, street, s.distance, s.duration, geometry_start);
        st.road_ref = s.reference.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
        st.exit = junction;
        st.destination = destination;
        // lanes at the maneuver: the first intersection of the step
        if let Some(i) = s.intersections.first() {
            st.lanes = i.lanes.iter().map(|l| Lane { directions: l.indications.iter().map(|d| d.replace(' ', "_")).collect(), active: l.valid }).collect();
            if st.lanes.iter().all(|l| l.active) {
                st.lanes.clear(); // no lane advice when every lane works
            }
        }
        out.push(st);
        // Consecutive step geometries share their boundary vertex.
        geometry_start += s.geometry.coordinates.len().saturating_sub(1);
    }
    merge_roundabout_exits(out)
}

pub fn route(req: &RouteRequest, base_url: &str) -> anyhow::Result<RouteResponse> {
    let t0 = Instant::now();
    let coords: Vec<String> = req.waypoints.iter().map(|w| format!("{:.6},{:.6}", w.lon, w.lat)).collect();
    let alternatives = if req.waypoints.len() == 2 { req.alternatives.clamp(1, 3) } else { 1 };
    let mut exclude = Vec::new();
    if req.mode == Mode::Car {
        if req.avoid.toll {
            exclude.push("toll");
        }
        if req.avoid.motorway {
            exclude.push("motorway");
        }
        if req.avoid.ferry {
            exclude.push("ferry");
        }
    }
    anyhow::ensure!(matches!(req.mode, Mode::Car | Mode::Bike | Mode::Foot), "OSRM has no {} profile", req.mode.as_str());
    let base = base_url.trim_end_matches('/');
    let mut url = format!(
        "{base}/{}/route/v1/driving/{}?overview=full&geometries=geojson&steps=true&alternatives={}",
        profile_path(req.mode),
        coords.join(";"),
        if alternatives > 1 { alternatives.to_string() } else { "false".to_string() }
    );
    if !exclude.is_empty() {
        url.push_str("&exclude=");
        url.push_str(&exclude.join(","));
    }
    let client = super::client()?;
    let mut resp: OsrmResponse = client.get(&url).send().context("OSRM request failed")?.json().context("OSRM: bad JSON")?;
    // "NoRoute" with exclusions means there is no route that avoids them: retrying without
    // `exclude` would silently hand back a toll / motorway route, so it is an error and the local
    // engine (which honours the avoid options) answers instead.
    if resp.code != "Ok" && !exclude.is_empty() && !matches!(resp.code.as_str(), "NoRoute" | "NoSegment") {
        // Some servers reject `exclude` (InvalidValue / InvalidQuery); retry without it.
        let url2 = url.split("&exclude=").next().unwrap_or(&url).to_string();
        resp = client.get(&url2).send().context("OSRM request failed")?.json().context("OSRM: bad JSON")?;
    }
    if resp.code != "Ok" {
        bail!("OSRM: {} {}", resp.code, resp.message.unwrap_or_default());
    }
    if resp.routes.is_empty() {
        return Err(anyhow!("OSRM: no route"));
    }

    let mut routes = Vec::new();
    for (i, r) in resp.routes.iter().enumerate() {
        let (depart_iso, arrive_iso) = resolve_times(&req.time, r.duration);
        let legs: Vec<Leg> = r
            .legs
            .iter()
            .map(|l| {
                let geometry: Vec<LonLat> = {
                    let mut g = Vec::new();
                    for s in &l.steps {
                        if g.is_empty() {
                            g.extend_from_slice(&s.geometry.coordinates);
                        } else {
                            g.extend_from_slice(&s.geometry.coordinates[1.min(s.geometry.coordinates.len())..]);
                        }
                    }
                    g
                };
                Leg {
                    kind: leg_kind(req.mode),
                    duration_sec: l.duration,
                    distance_m: if l.distance > 0.0 { l.distance } else { polyline_length_m(&geometry) },
                    geometry,
                    depart_iso: None,
                    arrive_iso: None,
                    steps: convert_steps(l),
                    transit: None,
                }
            })
            .collect();
        routes.push(Route {
            id: format!("osrm-{i}"),
            mode: req.mode,
            duration_sec: r.duration,
            distance_m: r.distance,
            geometry: r.geometry.coordinates.clone(),
            legs,
            tags: vec![],
            depart_iso,
            arrive_iso,
            transfers: 0,
            walk_m: if req.mode == Mode::Foot { r.distance } else { 0.0 },
            fare_estimate_ils: None,
            fare_note: None,
            avoid_violations: vec![],
            fare_breakdown: vec![],
        });
    }
    tag_routes(&mut routes);
    Ok(RouteResponse { routes, compute_ms: t0.elapsed().as_millis() as u64, source: RouteSource::Online, provider: Some("osrm".into()), notice: None })
}

fn tag_routes(routes: &mut [Route]) {
    if routes.is_empty() {
        return;
    }
    let fastest = (0..routes.len()).min_by(|&a, &b| routes[a].duration_sec.total_cmp(&routes[b].duration_sec)).unwrap();
    let shortest = (0..routes.len()).min_by(|&a, &b| routes[a].distance_m.total_cmp(&routes[b].distance_m)).unwrap();
    routes[fastest].tags.push(RouteTag::Fastest);
    if shortest != fastest || routes.len() == 1 {
        routes[shortest].tags.push(RouteTag::Shortest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_maneuvers() {
        let m = |k: &str, md: Option<&str>| maneuver(&OsrmManeuver { kind: k.into(), modifier: md.map(String::from), exit: None });
        assert_eq!(m("turn", Some("left")), Maneuver::TurnLeft);
        assert_eq!(m("continue", Some("uturn")), Maneuver::Uturn);
        assert_eq!(m("roundabout", Some("right")), Maneuver::Roundabout);
        assert_eq!(m("depart", None), Maneuver::Depart);
        assert_eq!(m("new name", Some("straight")), Maneuver::Straight);
    }

    #[test]
    fn steps_with_exits_lanes_and_roundabouts() {
        let json = r#"{"distance":1000,"duration":100,"steps":[
          {"distance":100,"duration":10,"name":"הרצל","geometry":{"coordinates":[[34.0,32.0],[34.001,32.0]]},"maneuver":{"type":"depart"}},
          {"distance":60,"duration":6,"name":"","geometry":{"coordinates":[[34.001,32.0],[34.002,32.0]]},"maneuver":{"type":"roundabout","modifier":"right","exit":2}},
          {"distance":652,"duration":40,"name":"","ref":"45","geometry":{"coordinates":[[34.002,32.0],[34.003,32.0]]},"maneuver":{"type":"exit roundabout","modifier":"right"}},
          {"distance":100,"duration":5,"name":"","destinations":"1: ירושלים, תל אביב","exits":"5","geometry":{"coordinates":[[34.003,32.0],[34.004,32.0]]},"maneuver":{"type":"off ramp","modifier":"slight right"},
           "intersections":[{"lanes":[{"indications":["straight"],"valid":false},{"indications":["slight right"],"valid":true}]}]},
          {"distance":0,"duration":0,"name":"","geometry":{"coordinates":[[34.004,32.0]]},"maneuver":{"type":"arrive"}}]}"#;
        let leg: OsrmLeg = serde_json::from_str(json).unwrap();
        let steps = convert_steps(&leg);
        assert_eq!(steps.len(), 4, "{steps:#?}");
        assert_eq!(steps[1].maneuver, Maneuver::Roundabout);
        assert!((steps[1].distance_m - 712.0).abs() < 1e-9);
        assert_eq!(steps[1].instruction, "היכנס לכיכר וצא ביציאה השנייה לכביש 45");
        assert_eq!(steps[2].instruction, "צא ביציאה 5 לכיוון ירושלים, תל אביב");
        assert_eq!(steps[2].lanes.len(), 2);
        assert!(!steps[2].lanes[0].active && steps[2].lanes[1].active);
        assert_eq!(steps[2].lanes[1].directions, vec!["slight_right"]);
    }

    /// Hits the public FOSSGIS server – run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_tel_aviv_jerusalem() {
        let req = RouteRequest {
            waypoints: vec![Waypoint { lon: 34.7818, lat: 32.0853, name: None }, Waypoint { lon: 35.2137, lat: 31.7683, name: None }],
            mode: Mode::Car,
            time: TimeSpec::default(),
            accessible: false,
            avoid: Avoid::default(),
            alternatives: 3,
            ..Default::default()
        };
        let r = route(&req, "https://routing.openstreetmap.de").unwrap();
        assert!(!r.routes.is_empty());
        assert!(r.routes[0].distance_m > 50_000.0 && r.routes[0].distance_m < 90_000.0);
        assert!(!r.routes[0].legs[0].steps.is_empty());
    }
}
