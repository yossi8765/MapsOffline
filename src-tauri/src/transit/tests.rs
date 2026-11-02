//! Unit tests on a tiny synthetic GTFS feed.
use std::io::Write;
use std::path::PathBuf;

use super::*;
use super::journey::plan;
use super::model::search_stops;
use crate::types::{LegKind, Mode, RouteRequest, TimeKind, TimeSpec, TransitVehicle, Waypoint};

// Stops (lon, lat). All > 1.2 km apart except D/E (≈220 m → foot transfer).
const STOPS: &str = "stop_id,stop_code,stop_name,stop_desc,stop_lat,stop_lon,location_type,parent_station
A,1001,Alpha,,32.000,34.800,0,
B,1002,Beta,,32.000,34.830,0,
C,1003,Gamma Central,,32.000,34.860,0,
D,1004,Delta,,32.000,34.890,0,
E,1005,Echo,,32.002,34.890,0,
F,1006,Foxtrot,,32.030,34.920,0,
G,1007,Golf,,32.100,34.800,0,
H,1008,Hotel,,32.100,34.850,0,
";

const AGENCY: &str = "agency_id,agency_name,agency_url,agency_timezone
1,Egged,http://x,Asia/Jerusalem
2,Rail,http://y,Asia/Jerusalem
";

const ROUTES: &str = "route_id,agency_id,route_short_name,route_long_name,route_desc,route_type,route_color
R1,1,1,A-C,,3,
R2,1,2,C-D,,3,FF8800
R3,2,R3,E-F,,2,
R4,1,4N,G-H night,,3,
";

const TRIPS: &str = "route_id,service_id,trip_id,trip_headsign,direction_id,shape_id
R1,WD,T1a_011024,To C,0,S1
R1,WD,T1b_011024,To C,0,S1
R2,WD,T2a,To D,0,
R2,WD,T2b,To D,0,
R3,WD,T3a,To F,0,
R3,WD,T3b,To F,0,
R4,MON,T4,To H,0,
";

// Deliberately not grouped by trip for T1b to exercise non-contiguous handling.
const STOP_TIMES: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type,drop_off_type
T1a_011024,08:00:00,08:00:00,A,1,0,0
T1a_011024,08:10:00,08:10:00,B,2,0,0
T1a_011024,08:20:00,08:20:00,C,3,0,0
T1b_011024,08:30:00,08:30:00,A,1,0,0
T2a,08:25:00,08:25:00,C,1,0,0
T2a,08:40:00,08:40:00,D,2,0,0
T1b_011024,08:50:00,08:50:00,C,3,0,0
T1b_011024,08:40:00,08:40:00,B,2,0,0
T2b,08:55:00,08:55:00,C,1,0,0
T2b,09:10:00,09:10:00,D,2,0,0
T3a,08:50:00,08:50:00,E,1,0,0
T3a,09:05:00,09:05:00,F,2,0,0
T3b,09:20:00,09:20:00,E,1,0,0
T3b,09:35:00,09:35:00,F,2,0,0
T4,24:30:00,24:30:00,G,1,0,0
T4,24:50:00,24:50:00,H,2,0,0
";

const CALENDAR: &str = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date
WD,1,1,1,1,1,0,0,20241001,20241031
MON,1,0,0,0,0,0,0,20241001,20241031
";

const CAL_DATES: &str = "service_id,date,exception_type
WD,20241016,2
";

const SHAPES: &str = "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence
S1,32.000,34.800,1
S1,32.001,34.815,2
S1,32.000,34.830,3
S1,32.001,34.845,4
S1,32.000,34.860,5
";

fn build_model(name: &str) -> Transit {
    let dir = std::env::temp_dir().join(format!("mo_transit_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path: PathBuf = dir.join("gtfs.zip");
    {
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (n, c) in [
            ("agency.txt", AGENCY),
            ("routes.txt", ROUTES),
            ("trips.txt", TRIPS),
            ("stop_times.txt", STOP_TIMES),
            ("stops.txt", STOPS),
            ("calendar.txt", CALENDAR),
            ("calendar_dates.txt", CAL_DATES),
            ("shapes.txt", SHAPES),
        ] {
            z.start_file(n, opt).unwrap();
            // BOM on one file to make sure headers are cleaned
            if n == "stops.txt" {
                z.write_all(b"\xEF\xBB\xBF").unwrap();
            }
            z.write_all(c.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    let out = dir.join("transit.bin");
    let noop = |_: crate::types::ProgressPhase, _: u64, _: u64, _: &str| true;
    build(&zip_path, &out, &noop).unwrap();
    let t = load(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    t
}

fn ll(t: &Transit, code: &str) -> [f64; 2] {
    let s = t.stops.iter().position(|s| t.s(s.code) == code).unwrap();
    t.lonlat(s as u32)
}

fn req(t: &Transit, from: &str, to: &str, kind: TimeKind, iso: &str) -> RouteRequest {
    let a = ll(t, from);
    let b = ll(t, to);
    RouteRequest {
        waypoints: vec![
            Waypoint { lon: a[0], lat: a[1], name: None },
            Waypoint { lon: b[0], lat: b[1], name: None },
        ],
        mode: Mode::Transit,
        time: TimeSpec { kind, iso: Some(iso.to_string()) },
        avoid: Default::default(),
        alternatives: 3,
    }
}

fn transit_lines(r: &crate::types::Route) -> Vec<String> {
    r.legs.iter().filter_map(|l| l.transit.as_ref().map(|x| x.route_short_name.clone())).collect()
}

#[test]
fn model_basics() {
    let t = build_model("basics");
    assert_eq!(t.stops.len(), 8);
    assert_eq!(t.routes.len(), 4);
    assert_eq!(t.trip_service.len(), 7);
    // D <-> E transfer exists
    let d = t.stops.iter().position(|s| t.s(s.code) == "1004").unwrap() as u32;
    let e = t.stops.iter().position(|s| t.s(s.code) == "1005").unwrap() as u32;
    let tr: Vec<_> = t.transfers(d).collect();
    assert_eq!(tr.len(), 1);
    assert_eq!(tr[0].0, e);
    assert!(tr[0].1 > 200 && tr[0].1 < 300, "transfer time {}", tr[0].1);
    // shapes attached to route 1
    let p1 = t.patterns.iter().position(|p| t.s(t.routes[p.route as usize].short_name) == "1").unwrap();
    assert_ne!(t.patterns[p1].shape, model::NONE);
    assert_eq!(t.patterns[p1].ntrips, 2);
    let g = t.leg_geometry(p1 as u32, 0, 2);
    assert!(g.len() >= 4, "shape geometry {:?}", g);
}

#[test]
fn direct_trip() {
    let t = build_model("direct");
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    assert!(!r.routes.is_empty());
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1"]);
    assert_eq!(best.transfers, 0);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T08:20:00"), "{:?}", best.arrive_iso);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:00:00"));
    let leg = best.legs.iter().find(|l| l.kind == LegKind::Transit).unwrap();
    let ti = leg.transit.as_ref().unwrap();
    assert_eq!(ti.vehicle, TransitVehicle::Bus);
    assert_eq!(ti.num_stops, 2);
    assert_eq!(ti.stops.len(), 3);
    assert_eq!(ti.agency, "Egged");
    assert_eq!(ti.headsign, "To C");
    assert!(best.tags.contains(&crate::types::RouteTag::Fastest));
}

#[test]
fn one_transfer() {
    let t = build_model("transfer");
    let r = plan(&t, &req(&t, "1001", "1004", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1", "2"]);
    assert_eq!(best.transfers, 1);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T08:40:00"), "{:?}", best.arrive_iso);
    let l2 = best.legs.iter().filter(|l| l.kind == LegKind::Transit).nth(1).unwrap();
    assert_eq!(l2.transit.as_ref().unwrap().color.as_deref(), Some("#FF8800"));
}

#[test]
fn no_service_on_removed_date() {
    let t = build_model("caldates");
    // 2024-10-16 removed via calendar_dates → no transit route
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-16T07:55:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes.iter().all(|r| transit_lines(r).is_empty()));
}

#[test]
fn foot_transfer() {
    let t = build_model("foot");
    let r = plan(&t, &req(&t, "1001", "1006", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1", "2", "R3"]);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:05:00"), "{:?}", best.arrive_iso);
    // walk leg between line 2 and R3
    let kinds: Vec<LegKind> = best.legs.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, vec![LegKind::Transit, LegKind::Transit, LegKind::Walk, LegKind::Transit]);
    let rail = best.legs.last().unwrap().transit.as_ref().unwrap();
    assert_eq!(rail.vehicle, TransitVehicle::Rail);
    assert_eq!(best.transfers, 2);
    assert!(best.walk_m > 200.0 && best.walk_m < 260.0);
}

#[test]
fn arrive_by() {
    let t = build_model("arrive");
    let r = plan(&t, &req(&t, "1001", "1004", TimeKind::Arrive, "2024-10-07T09:15:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1", "2"]);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:30:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:10:00"), "{:?}", best.arrive_iso);
    // deadline before any possible arrival → nothing
    let r = plan(&t, &req(&t, "1001", "1004", TimeKind::Arrive, "2024-10-07T08:30:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes.is_empty());
}

#[test]
fn arrive_by_with_foot_transfer() {
    let t = build_model("arrive_foot");
    let r = plan(&t, &req(&t, "1001", "1006", TimeKind::Arrive, "2024-10-07T09:40:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    // latest: T1b 08:30 → C 08:50 → T2b 08:55 → D 09:10 → walk → E → T3b 09:20 → F 09:35
    assert_eq!(transit_lines(best), vec!["1", "2", "R3"]);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:30:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:35:00"), "{:?}", best.arrive_iso);
}

#[test]
fn midnight_wrap() {
    let t = build_model("midnight");
    // T4 runs on Monday's service day at 24:30 → Tuesday 00:30. Query Tuesday 00:10.
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Depart, "2024-10-08T00:10:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["4N"]);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-08T00:30:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-08T00:50:00"), "{:?}", best.arrive_iso);
    // ... and arrive-by across midnight
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Arrive, "2024-10-08T01:00:00+03:00"), &NoWalk).unwrap();
    assert_eq!(transit_lines(&r.routes[0]), vec!["4N"]);
    // Monday evening query also finds it (same service day, time > 24h)
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Depart, "2024-10-07T23:50:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes[0].arrive_iso.as_deref().unwrap().starts_with("2024-10-08T00:50:00"));
    // Tuesday's own service (MON only) does not run → Wednesday 00:10 finds nothing
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Depart, "2024-10-09T00:10:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes.is_empty());
}

#[test]
fn pure_walk_alternative() {
    let t = build_model("walk");
    // D → E is 220 m: walking only (plus nothing else)
    let r = plan(&t, &req(&t, "1004", "1005", TimeKind::Depart, "2024-10-07T08:00:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes.iter().any(|r| r.legs.len() == 1 && r.legs[0].kind == LegKind::Walk));
}

#[test]
fn stop_search() {
    let t = build_model("search");
    let r = search_stops(&t, "gamma", None, 5);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].category.as_deref(), Some("stop:1003"));
    assert_eq!(r[0].kind, crate::types::GeoKind::Stop);
    let r = search_stops(&t, "1005", None, 5);
    assert_eq!(r[0].name, "Echo");
    let r = search_stops(&t, "central", Some([34.86, 32.0]), 5);
    assert_eq!(r.len(), 1);
    // ordering by distance among equal scores
    let r = search_stops(&t, "a", Some([34.92, 32.03]), 10);
    assert!(!r.is_empty());
}

fn via_req(t: &Transit, codes: &[&str], kind: TimeKind, iso: &str) -> RouteRequest {
    RouteRequest {
        waypoints: codes
            .iter()
            .map(|c| {
                let p = ll(t, c);
                Waypoint { lon: p[0], lat: p[1], name: None }
            })
            .collect(),
        mode: Mode::Transit,
        time: TimeSpec { kind, iso: Some(iso.to_string()) },
        avoid: Default::default(),
        alternatives: 3,
    }
}

#[test]
fn via_depart_at() {
    let t = build_model("via_dep");
    // A → (via B) → D: line 1 to B (08:10), stay ≥ 2 min, line 1 again B→C, then line 2 C→D.
    let r = plan(&t, &via_req(&t, &["1001", "1002", "1004"], TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk)
        .unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1", "1", "2"]);
    // 08:10 at B + 120 s → next line-1 trip at B is 08:40 → C 08:50 → T2b 08:55 → D 09:10
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:00:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:10:00"), "{:?}", best.arrive_iso);
    assert_eq!(best.transfers, 2);
    assert!(!best.tags.is_empty());
}

#[test]
fn via_arrive_by() {
    let t = build_model("via_arr");
    // A → (via C) → D arrive by 09:15: T2b C 08:55 → D 09:10; previous segment must reach C by 08:53 → T1b (A 08:30 → C 08:50)
    let r = plan(&t, &via_req(&t, &["1001", "1003", "1004"], TimeKind::Arrive, "2024-10-07T09:15:00+03:00"), &NoWalk)
        .unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["1", "2"]);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:30:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:10:00"), "{:?}", best.arrive_iso);
    assert_eq!(best.transfers, 1);
}

/// Walker whose road walks between nearby stops are a long detour (20 min).
struct DetourWalk;
impl WalkRouter for DetourWalk {
    fn walk(&self, from: crate::types::LonLat, to: crate::types::LonLat) -> Option<crate::types::Leg> {
        let d = crate::geo::haversine_m(from, to);
        if d > 100.0 && d < 400.0 {
            let mut l = straight_walk(from, to);
            l.duration_sec = 1200.0;
            Some(l)
        } else {
            None
        }
    }
}

#[test]
fn infeasible_road_transfer_is_rejected() {
    let t = build_model("detour");
    // With NoWalk the D→E transfer works (foot_transfer test). With a 20-minute road detour
    // neither R3 connection can be made, so no journey to F may be returned.
    let r = plan(&t, &req(&t, "1001", "1006", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &DetourWalk).unwrap();
    for rt in &r.routes {
        assert!(!transit_lines(rt).contains(&"R3".to_string()), "infeasible connection returned");
    }
}
