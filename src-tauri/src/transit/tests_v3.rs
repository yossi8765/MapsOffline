//! Tests of the v3 additions: file format and validation, DST, frequencies, accessibility,
//! transfers.txt, Shabbat / holiday notices, fare breakdown, isochrones, the stops layer.
//! A child of `transit::tests` (shares its synthetic feed and helpers).
use super::*;
use crate::types::{NoticeKind, TimeKind, TimeSpec};

#[test]
fn v3_roundtrip_and_validation() {
    let t = build_model("v3");
    assert!(t.validate().is_ok());
    let dir = std::env::temp_dir().join(format!("mo_transit_{}_v3rt", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("transit.bin");
    model::save(&t, &p).unwrap();
    let u = load(&p).unwrap();
    assert_eq!(u.stops.len(), t.stops.len());
    assert_eq!(u.arr, t.arr);
    assert_eq!(u.tr_dur, t.tr_dur);
    assert_eq!(u.search_stops("gamma", None, 5).len(), 1);
    // truncated file → TRANSIT_DATA_CORRUPT, not a panic
    let bytes = std::fs::read(&p).unwrap();
    for cut in [6, 40, bytes.len() / 2, bytes.len() - 2] {
        std::fs::write(&p, &bytes[..cut]).unwrap();
        let e = format!("{:#}", load(&p).err().expect("truncated file must fail"));
        assert!(e.contains("TRANSIT_DATA_CORRUPT"), "cut {cut}: {e}");
    }
    // flipped bytes in the middle of an index: still an error, never a panic
    let mut bad = bytes.clone();
    let mid = bad.len() / 3;
    for b in &mut bad[mid..mid + 64] {
        *b = 0xFF;
    }
    std::fs::write(&p, &bad).unwrap();
    assert!(load(&p).is_err());
    // an older version → NO_TRANSIT_DATA (the UI offers the update)
    let mut old = bytes.clone();
    old[4..8].copy_from_slice(&2u32.to_le_bytes());
    std::fs::write(&p, &old).unwrap();
    let e = format!("{:#}", load(&p).err().unwrap());
    assert!(e.starts_with("NO_TRANSIT_DATA") && e.contains("transit version"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validate_rejects_bad_indexes() {
    let mut t = build_model("badidx");
    t.pattern_stops[0] = 999;
    assert!(t.validate().unwrap_err().contains("pattern stop"));
    let mut t = build_model("badidx2");
    t.tr_to[0] = 77;
    assert!(t.validate().is_err());
    let mut t = build_model("badidx3");
    t.patterns[0].times_start = u32::MAX - 3;
    assert!(t.validate().unwrap_err().contains("times"));
    let mut t = build_model("badidx4");
    t.trip_service[0] = 50;
    assert!(t.validate().is_err());
}

#[test]
fn dst_day_times() {
    use crate::transit::journey::{iso_at, resolve_time};
    use chrono::NaiveDate;
    // Israel moves to summer time on Friday 27 March 2026 at 02:00
    let d = NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();
    assert_eq!(iso_at(d, 8 * 3600), "2026-03-27T08:00:00+03:00");
    // GTFS 01:00 counts from noon − 12 h = 23:00 of the day before (still winter time)
    assert_eq!(iso_at(d, 3600), "2026-03-27T00:00:00+02:00");
    let ts = TimeSpec { kind: TimeKind::Depart, iso: Some("2026-03-27T08:00:00+03:00".into()) };
    assert_eq!(resolve_time(&ts).unwrap().1, 8 * 3600);
    // and back on Sunday 25 October 2026 at 02:00
    let d = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
    assert_eq!(iso_at(d, 8 * 3600), "2026-10-25T08:00:00+02:00");
    let ts = TimeSpec { kind: TimeKind::Depart, iso: Some("2026-10-25T08:00:00+02:00".into()) };
    assert_eq!(resolve_time(&ts).unwrap().1, 8 * 3600);
    // an ordinary day is unchanged
    let d = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
    assert_eq!(iso_at(d, 25 * 3600), "2026-11-05T01:00:00+02:00");
}

const TRIPS_FREQ: &str = "route_id,service_id,trip_id,trip_headsign,direction_id,shape_id,wheelchair_accessible
R1,WD,T1a_011024,To C,0,S1,1
R1,WD,T1b_011024,To C,0,S1,2
R2,WD,T2a,To D,0,,1
R2,WD,T2b,To D,0,,1
R3,WD,T3a,To F,0,,1
R3,WD,T3b,To F,0,,1
R4,MON,T4,To H,0,,1
R4,WD,TF,To H,0,,1
";

const STOP_TIMES_FREQ: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type,drop_off_type
T1a_011024,08:00:00,08:00:00,A,1,0,0
T1a_011024,08:10:00,08:10:00,B,2,0,0
T1a_011024,08:20:00,08:20:00,C,3,0,0
T1b_011024,08:30:00,08:30:00,A,1,0,0
T1b_011024,08:40:00,08:40:00,B,2,0,0
T1b_011024,08:50:00,08:50:00,C,3,0,0
T2a,08:25:00,08:25:00,C,1,0,0
T2a,08:40:00,08:40:00,D,2,0,0
T2b,08:55:00,08:55:00,C,1,0,0
T2b,09:10:00,09:10:00,D,2,0,0
T3a,08:50:00,08:50:00,E,1,0,0
T3a,09:05:00,09:05:00,F,2,0,0
T3b,09:20:00,09:20:00,E,1,0,0
T3b,09:35:00,09:35:00,F,2,0,0
T4,24:30:00,24:30:00,G,1,0,0
T4,24:50:00,24:50:00,H,2,0,0
TF,00:00:00,00:00:00,G,1,0,0
TF,00:12:00,00:12:00,H,2,0,0
";

const FREQUENCIES: &str = "trip_id,start_time,end_time,headway_secs
TF,10:00:00,11:00:00,1200
";

#[test]
fn frequencies_expand_into_trips() {
    let t = build_model_with("freq", &[("trips.txt", TRIPS_FREQ), ("stop_times.txt", STOP_TIMES_FREQ), ("frequencies.txt", FREQUENCIES)]);
    // TF runs at 10:00, 10:20, 10:40
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Depart, "2024-10-08T10:05:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert_eq!(transit_lines(best), vec!["4N"]);
    assert!(best.depart_iso.as_deref().unwrap().starts_with("2024-10-08T10:20:00"), "{:?}", best.depart_iso);
    assert!(best.arrive_iso.as_deref().unwrap().starts_with("2024-10-08T10:32:00"), "{:?}", best.arrive_iso);
    let r = plan(&t, &req(&t, "1007", "1008", TimeKind::Depart, "2024-10-08T10:45:00+03:00"), &NoWalk).unwrap();
    assert!(
        r.routes.iter().all(|r| !r.depart_iso.as_deref().unwrap_or("").starts_with("2024-10-08T1")),
        "no trip at 11:00 (the window end is exclusive)"
    );
}

#[test]
fn accessible_filter() {
    let t = build_model_with("access", &[("trips.txt", TRIPS_FREQ), ("stop_times.txt", STOP_TIMES_FREQ)]);
    let mut q = req(&t, "1001", "1003", TimeKind::Depart, "2024-10-07T08:05:00+03:00");
    let r = plan(&t, &q, &NoWalk).unwrap();
    assert!(r.routes[0].depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:30:00"));
    // T1b (08:30) is marked not wheelchair accessible
    q.accessible = true;
    let r = plan(&t, &q, &NoWalk).unwrap();
    assert!(r.routes.iter().all(|r| transit_lines(r).is_empty()), "the 08:30 trip is not wheelchair accessible");
    let mut q = req(&t, "1001", "1003", TimeKind::Depart, "2024-10-07T07:55:00+03:00");
    q.accessible = true;
    let r = plan(&t, &q, &NoWalk).unwrap();
    assert!(r.routes[0].depart_iso.as_deref().unwrap().starts_with("2024-10-07T08:00:00"));
}

const STOPS_ACCESS: &str = "stop_id,stop_code,stop_name,stop_desc,stop_lat,stop_lon,location_type,parent_station,wheelchair_boarding
A,1001,Alpha,,32.000,34.800,0,,1
B,1002,Beta,,32.000,34.830,0,,1
C,1003,Gamma Central,,32.000,34.860,0,ST,0
D,1004,Delta,,32.000,34.890,0,,2
E,1005,Echo,,32.002,34.890,0,,1
F,1006,Foxtrot,,32.030,34.920,0,,1
G,1007,Golf,,32.100,34.800,0,,1
H,1008,Hotel,,32.100,34.850,0,,1
ST,,Gamma Station,,32.000,34.860,1,,1
";

#[test]
fn stop_wheelchair_and_parent() {
    let t = build_model_with("stopacc", &[("stops.txt", STOPS_ACCESS)]);
    let id = |code: &str| t.stops.iter().position(|s| t.s(s.code) == code).unwrap() as u32;
    assert!(t.stop_accessible(id("1001")));
    assert!(!t.stop_accessible(id("1004")));
    // C inherits the station's "1"
    assert_eq!(t.stop_wheelchair[id("1003") as usize], 1);
    assert_eq!(t.stops.len(), 8, "the station itself is not a stop");
    // A → D: D is not accessible
    let mut q = req(&t, "1001", "1004", TimeKind::Depart, "2024-10-07T07:55:00+03:00");
    assert!(plan(&t, &q, &NoWalk).unwrap().routes.iter().any(|r| !transit_lines(r).is_empty()));
    q.accessible = true;
    assert!(plan(&t, &q, &NoWalk).unwrap().routes.iter().all(|r| transit_lines(r).is_empty()));
}

const TRANSFERS: &str = "from_stop_id,to_stop_id,transfer_type,min_transfer_time
C,C,2,600
D,E,3,
";

#[test]
fn transfers_txt() {
    let t = build_model_with("transfers", &[("transfers.txt", TRANSFERS)]);
    // 10 minutes to change at C: T1a 08:20 misses T2a 08:25, takes T2b 08:55 → D 09:10
    let r = plan(&t, &req(&t, "1001", "1004", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes[0].arrive_iso.as_deref().unwrap().starts_with("2024-10-07T09:10:00"), "{:?}", r.routes[0].arrive_iso);
    // the D → E walk is forbidden
    let d = t.stops.iter().position(|s| t.s(s.code) == "1004").unwrap() as u32;
    assert_eq!(t.transfers(d).count(), 0);
}

#[test]
fn notice_on_shabbat_holiday_and_night() {
    let t = build_model("shabbat");
    // Saturday 19 October 2024 (no WD service on Shabbat)
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-19T10:00:00+03:00"), &NoWalk).unwrap();
    assert!(r.routes.iter().all(|r| transit_lines(r).is_empty()));
    let n = r.notice.expect("a notice");
    assert_eq!(n.kind, NoticeKind::Shabbat);
    assert!(n.next_service_iso.as_deref().unwrap().starts_with("2024-10-21T08:00:00"), "{:?}", n.next_service_iso);
    // Friday afternoon before Yom Kippur (Shabbat 12 October 2024) → a holiday notice
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-11T18:00:00+03:00"), &NoWalk).unwrap();
    let n = r.notice.expect("a notice");
    assert_eq!(n.kind, NoticeKind::Holiday);
    assert_eq!(n.holiday_name.as_deref(), Some("יום כיפור"));
    // late at night with the first ride hours later
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-07T02:00:00+03:00"), &NoWalk).unwrap();
    assert_eq!(r.notice.map(|n| n.kind), Some(NoticeKind::LateNight));
    // an ordinary morning: no notice
    let r = plan(&t, &req(&t, "1001", "1003", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    assert!(r.notice.is_none());
}

#[test]
fn fares_breakdown_on_routes() {
    let t = build_model("fares");
    let r = plan(&t, &req(&t, "1001", "1004", TimeKind::Depart, "2024-10-07T07:55:00+03:00"), &NoWalk).unwrap();
    let best = &r.routes[0];
    assert!(!best.fare_breakdown.is_empty());
    let sum: f64 = best.fare_breakdown.iter().map(|t| t.ils).sum();
    assert!((sum - best.fare_estimate_ils.unwrap()).abs() < 0.01);
}

#[test]
fn transit_isochrone_reaches_by_bus() {
    let t = build_model("iso");
    let ts = TimeSpec { kind: TimeKind::Depart, iso: Some("2024-10-07T07:55:00+03:00".into()) };
    let bands = t.isochrone(ll(&t, "1001"), &ts, &[10, 30], false).unwrap();
    assert_eq!(bands.len(), 2);
    let c = ll(&t, "1003");
    // C (5.7 km away) by line 1 at 08:20 = 25 minutes
    assert!(!crate::transit::isochrone::inside_any(&bands[0].rings, c));
    assert!(crate::transit::isochrone::inside_any(&bands[1].rings, c));
    assert!(crate::transit::isochrone::inside_any(&bands[0].rings, ll(&t, "1001")));
}

#[test]
fn stops_in_bbox_uses_the_grid() {
    let t = build_model("bbox");
    let v = t.stops_in_bbox([34.79, 31.99, 34.84, 32.01], 100);
    let codes: Vec<String> = v.iter().map(|&s| t.stop_code(s).unwrap()).collect();
    assert_eq!(codes.len(), 2, "{codes:?}");
    assert!(codes.contains(&"1001".to_string()) && codes.contains(&"1002".to_string()));
    // the whole world: falls back to the scan, every served stop
    assert_eq!(t.stops_in_bbox([-180.0, -90.0, 180.0, 90.0], 100).len(), 8);
}
