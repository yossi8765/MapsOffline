//! MOTIS / Transitous `plan` API client → `Route`s with transit legs.
use std::time::Instant;

use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::geo::polyline_length_m;
use crate::online::{polyline, to_local_iso};
use crate::types::*;

#[derive(Deserialize)]
struct PlanResponse {
    #[serde(default)]
    itineraries: Vec<Itinerary>,
    #[serde(default)]
    direct: Vec<Itinerary>,
}

#[derive(Deserialize)]
struct Itinerary {
    duration: f64,
    #[serde(rename = "startTime")]
    start_time: String,
    #[serde(rename = "endTime")]
    end_time: String,
    #[serde(default)]
    transfers: u32,
    legs: Vec<MLeg>,
}

#[derive(Deserialize)]
struct Place {
    #[serde(default)]
    name: String,
    lat: f64,
    lon: f64,
    #[serde(default)]
    departure: Option<String>,
    #[serde(default)]
    arrival: Option<String>,
}

#[derive(Deserialize)]
struct EncodedPolyline {
    #[serde(default)]
    points: String,
    #[serde(default = "default_precision")]
    precision: u32,
}

fn default_precision() -> u32 {
    7
}

#[derive(Deserialize)]
struct MLeg {
    mode: String,
    from: Place,
    to: Place,
    duration: f64,
    #[serde(rename = "startTime")]
    start_time: String,
    #[serde(rename = "endTime")]
    end_time: String,
    #[serde(default)]
    distance: Option<f64>,
    #[serde(rename = "legGeometry")]
    leg_geometry: Option<EncodedPolyline>,
    #[serde(default)]
    headsign: Option<String>,
    #[serde(rename = "routeShortName", default)]
    route_short_name: Option<String>,
    #[serde(rename = "routeLongName", default)]
    route_long_name: Option<String>,
    #[serde(rename = "agencyName", default)]
    agency_name: Option<String>,
    #[serde(rename = "routeColor", default)]
    route_color: Option<String>,
    #[serde(rename = "routeType", default)]
    route_type: Option<i64>,
    #[serde(rename = "intermediateStops", default)]
    intermediate_stops: Vec<Place>,
}

fn vehicle(mode: &str, route_type: Option<i64>) -> TransitVehicle {
    match mode {
        "BUS" | "COACH" => TransitVehicle::Bus,
        "TRAM" => TransitVehicle::Tram,
        "SUBWAY" | "METRO" => TransitVehicle::Subway,
        "FERRY" => TransitVehicle::Ferry,
        m if m.contains("RAIL") || m == "LONG_DISTANCE" => TransitVehicle::Rail,
        _ => match route_type {
            Some(0) => TransitVehicle::LightRail,
            Some(1) => TransitVehicle::Subway,
            Some(2) => TransitVehicle::Rail,
            Some(3) => TransitVehicle::Bus,
            Some(4) => TransitVehicle::Ferry,
            _ => TransitVehicle::Other,
        },
    }
}

fn is_transit(mode: &str) -> bool {
    !matches!(mode, "WALK" | "BIKE" | "CAR" | "RENTAL" | "SCOOTER" | "ODM" | "FLEX")
}

fn request_time(time: &TimeSpec) -> (String, bool) {
    let parsed = time.iso.as_deref().and_then(|s| DateTime::parse_from_rfc3339(s).ok());
    match (time.kind, parsed) {
        (TimeKind::Arrive, Some(t)) => (t.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true), true),
        (TimeKind::Depart, Some(t)) => (t.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true), false),
        _ => (Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true), false),
    }
}

fn convert_leg(l: &MLeg) -> Leg {
    let geometry: Vec<LonLat> = match &l.leg_geometry {
        Some(g) if !g.points.is_empty() => polyline::decode(&g.points, g.precision),
        _ => vec![[l.from.lon, l.from.lat], [l.to.lon, l.to.lat]],
    };
    let distance_m = l.distance.filter(|d| *d > 0.0).unwrap_or_else(|| polyline_length_m(&geometry));
    let depart_iso = to_local_iso(&l.start_time);
    let arrive_iso = to_local_iso(&l.end_time);
    if is_transit(&l.mode) {
        let mut stops: Vec<TransitStopTime> = Vec::with_capacity(l.intermediate_stops.len() + 2);
        stops.push(TransitStopTime {
            name: l.from.name.clone(),
            code: None,
            lon: l.from.lon,
            lat: l.from.lat,
            time_iso: l.from.departure.as_deref().and_then(to_local_iso).or_else(|| depart_iso.clone()).unwrap_or_default(),
        });
        for s in &l.intermediate_stops {
            stops.push(TransitStopTime {
                name: s.name.clone(),
                code: None,
                lon: s.lon,
                lat: s.lat,
                time_iso: s.arrival.as_deref().or(s.departure.as_deref()).and_then(to_local_iso).unwrap_or_default(),
            });
        }
        stops.push(TransitStopTime {
            name: l.to.name.clone(),
            code: None,
            lon: l.to.lon,
            lat: l.to.lat,
            time_iso: l.to.arrival.as_deref().and_then(to_local_iso).or_else(|| arrive_iso.clone()).unwrap_or_default(),
        });
        let short = l.route_short_name.clone().unwrap_or_default();
        let long = l.route_long_name.clone().unwrap_or_default();
        Leg {
            kind: LegKind::Transit,
            duration_sec: l.duration,
            distance_m,
            geometry,
            depart_iso: depart_iso.clone(),
            arrive_iso: arrive_iso.clone(),
            steps: vec![],
            transit: Some(TransitLegInfo {
                vehicle: vehicle(&l.mode, l.route_type),
                route_short_name: short,
                route_long_name: long,
                agency: l.agency_name.clone().unwrap_or_default(),
                headsign: l.headsign.clone().unwrap_or_default(),
                color: l.route_color.as_deref().filter(|c| !c.is_empty()).map(|c| if c.starts_with('#') { c.to_string() } else { format!("#{c}") }),
                from_stop: l.from.name.clone(),
                to_stop: l.to.name.clone(),
                depart_iso: depart_iso.unwrap_or_default(),
                arrive_iso: arrive_iso.unwrap_or_default(),
                num_stops: (l.intermediate_stops.len() + 1) as u32,
                stops,
                delay_p50_min: None,
            }),
        }
    } else {
        Leg {
            kind: match l.mode.as_str() {
                "BIKE" => LegKind::Bike,
                "CAR" | "ODM" => LegKind::Drive,
                _ => LegKind::Walk,
            },
            duration_sec: l.duration,
            distance_m,
            geometry,
            depart_iso,
            arrive_iso,
            steps: vec![],
            transit: None,
        }
    }
}

fn convert_itinerary(idx: usize, it: &Itinerary) -> Route {
    let legs: Vec<Leg> = it.legs.iter().map(convert_leg).collect();
    let mut geometry = Vec::new();
    for l in &legs {
        if geometry.is_empty() {
            geometry.extend_from_slice(&l.geometry);
        } else {
            geometry.extend_from_slice(&l.geometry[1.min(l.geometry.len())..]);
        }
    }
    let transit_legs = legs.iter().filter(|l| l.kind == LegKind::Transit).count() as u32;
    Route {
        id: format!("transitous-{idx}"),
        mode: Mode::Transit,
        duration_sec: it.duration,
        distance_m: legs.iter().map(|l| l.distance_m).sum(),
        geometry,
        walk_m: legs.iter().filter(|l| l.kind == LegKind::Walk).map(|l| l.distance_m).sum(),
        transfers: if transit_legs > 0 { it.transfers.max(transit_legs - 1) } else { 0 },
        fare_estimate_ils: None,
        fare_note: None,
        avoid_violations: vec![],
        legs,
        tags: vec![],
        depart_iso: to_local_iso(&it.start_time),
        arrive_iso: to_local_iso(&it.end_time),
    }
}

pub fn plan(req: &RouteRequest, base_url: &str) -> anyhow::Result<RouteResponse> {
    let t0 = Instant::now();
    let from = req.waypoints.first().context("no origin")?;
    let to = req.waypoints.last().context("no destination")?;
    let (time, arrive_by) = request_time(&req.time);
    let n = req.alternatives.clamp(1, 6);
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{base}/api/v1/plan?fromPlace={:.6},{:.6}&toPlace={:.6},{:.6}&time={}&arriveBy={}&numItineraries={n}&detailedTransfers=false",
        from.lat,
        from.lon,
        to.lat,
        to.lon,
        urlencode(&time),
        arrive_by
    );
    let client = super::client()?;
    let resp = client.get(&url).send().context("transit API request failed")?;
    if !resp.status().is_success() {
        bail!("transit API: HTTP {}", resp.status());
    }
    let plan: PlanResponse = resp.json().context("transit API: bad JSON")?;
    let mut routes: Vec<Route> = plan.itineraries.iter().enumerate().map(|(i, it)| convert_itinerary(i, it)).collect();
    // Direct (walk-only) options are offered separately; include a short one as a fallback choice.
    for (i, it) in plan.direct.iter().enumerate() {
        if it.duration <= 30.0 * 60.0 {
            routes.push(convert_itinerary(100 + i, it));
        }
    }
    if routes.is_empty() {
        bail!("transit API: no itineraries");
    }
    routes.sort_by(|a, b| a.arrive_iso.cmp(&b.arrive_iso).then(a.duration_sec.total_cmp(&b.duration_sec)));
    routes.truncate(n as usize);
    tag(&mut routes);
    crate::transit::fares::annotate(&mut routes);
    Ok(RouteResponse { routes, compute_ms: t0.elapsed().as_millis() as u64, source: RouteSource::Online, provider: Some("transitous".into()) })
}

fn tag(routes: &mut [Route]) {
    if routes.is_empty() {
        return;
    }
    let idx = |f: &dyn Fn(&Route) -> f64| (0..routes.len()).min_by(|&a, &b| f(&routes[a]).total_cmp(&f(&routes[b]))).unwrap();
    let fastest = idx(&|r| r.duration_sec);
    let fewest = idx(&|r| r.transfers as f64);
    let least_walk = idx(&|r| r.walk_m);
    routes[fastest].tags.push(RouteTag::Fastest);
    if routes[fewest].transfers < routes[fastest].transfers {
        routes[fewest].tags.push(RouteTag::FewestTransfers);
    }
    if routes[least_walk].walk_m < routes[fastest].walk_m * 0.8 && least_walk != fewest {
        routes[least_walk].tags.push(RouteTag::LeastWalking);
    }
}

fn urlencode(s: &str) -> String {
    s.replace('+', "%2B").replace(':', "%3A")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vehicle_mapping() {
        assert_eq!(vehicle("REGIONAL_RAIL", None), TransitVehicle::Rail);
        assert_eq!(vehicle("BUS", Some(3)), TransitVehicle::Bus);
        assert_eq!(vehicle("TRAM", None), TransitVehicle::Tram);
        assert_eq!(vehicle("OTHER", Some(0)), TransitVehicle::LightRail);
    }

    /// Hits api.transitous.org – run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_tel_aviv_jerusalem() {
        let req = RouteRequest {
            waypoints: vec![Waypoint { lon: 34.7794, lat: 32.0557, name: None }, Waypoint { lon: 35.2033, lat: 31.7890, name: None }],
            mode: Mode::Transit,
            time: TimeSpec::default(),
            avoid: Avoid::default(),
            alternatives: 4,
            ..Default::default()
        };
        let r = plan(&req, "https://api.transitous.org").unwrap();
        assert!(!r.routes.is_empty());
        assert!(r.routes.iter().any(|r| r.legs.iter().any(|l| l.transit.is_some())));
    }
}
