//! Integration test on the real Israel GTFS feed.
//! Skipped unless `MAPSOFFLINE_GTFS` points to `israel-public-transportation.zip`.
//!
//!   MAPSOFFLINE_GTFS=<zip> cargo test --release --test transit_real -- --nocapture
//!
//! Optional: `MAPSOFFLINE_TRANSIT_BIN=<path>` reuses an already built model (skips the build).

use std::time::Instant;

use chrono::{Datelike, Duration, Utc, Weekday};
use mapsoffline_lib::transit::{self, NoWalk};
use mapsoffline_lib::types::{Mode, Route, RouteRequest, TimeKind, TimeSpec, Waypoint};

fn summary(r: &Route) -> String {
    let lines: Vec<String> = r
        .legs
        .iter()
        .map(|l| match &l.transit {
            Some(t) => format!(
                "[{:?} {} {} → {} {}–{} ({} stops)]",
                t.vehicle,
                t.route_short_name,
                t.from_stop,
                t.to_stop,
                &t.depart_iso[11..16],
                &t.arrive_iso[11..16],
                t.num_stops
            ),
            None => format!("[walk {:.0} m {:.0} min]", l.distance_m, l.duration_sec / 60.0),
        })
        .collect();
    format!(
        "{} {}→{} {:.0} min, {} transfers, walk {:.0} m, tags {:?}\n      {}",
        r.id,
        r.depart_iso.as_deref().map(|s| &s[11..16]).unwrap_or("?"),
        r.arrive_iso.as_deref().map(|s| &s[11..16]).unwrap_or("?"),
        r.duration_sec / 60.0,
        r.transfers,
        r.walk_m,
        r.tags,
        lines.join(" ")
    )
}

#[test]
fn real_gtfs_tel_aviv_jerusalem() {
    let Ok(zip) = std::env::var("MAPSOFFLINE_GTFS") else {
        eprintln!("MAPSOFFLINE_GTFS not set – skipping");
        return;
    };
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let dir = std::env::temp_dir().join("mapsoffline_transit_real");
    std::fs::create_dir_all(&dir).unwrap();
    let out = match std::env::var("MAPSOFFLINE_TRANSIT_BIN") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            let out = dir.join("transit.bin");
            let t0 = Instant::now();
            let progress = |ph: mapsoffline_lib::types::ProgressPhase, d: u64, t: u64, m: &str| {
                if m.contains("M rows") && !m.ends_with("0 M rows") {
                    return true;
                }
                eprintln!("  [{:>6.1}s] {:?} {}/{} {}", t0.elapsed().as_secs_f64(), ph, d, t, m);
                true
            };
            transit::build(std::path::Path::new(&zip), &out, &progress).expect("build");
            println!("BUILD: {:.1} s", t0.elapsed().as_secs_f64());
            out
        }
    };
    println!("MODEL FILE: {:.1} MB", std::fs::metadata(&out).unwrap().len() as f64 / 1e6);
    let t0 = Instant::now();
    let t = transit::load(&out).expect("load");
    println!("LOAD: {:.2} s, heap ≈ {:.0} MB", t0.elapsed().as_secs_f64(), t.heap_bytes() as f64 / 1e6);
    println!(
        "MODEL: {} stops, {} routes, {} patterns, {} trips, {} stop-times, {} transfers, {} shape pts, {} days from {}",
        t.stops.len(),
        t.routes.len(),
        t.patterns.len(),
        t.trip_service.len(),
        t.arr.len(),
        t.tr_to.len(),
        t.shape_pts.len(),
        t.num_days,
        t.date_of_day(0)
    );

    // next Israeli weekday (Sun–Thu) after today, within the feed
    let mut date = Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).date_naive() + Duration::days(1);
    while matches!(date.weekday(), Weekday::Fri | Weekday::Sat) || t.day_index(date).is_none() {
        date += Duration::days(1);
        if date.num_days_from_ce() > t.feed_start + t.num_days as i32 + 10 {
            panic!("no weekday inside feed");
        }
    }
    let mk = |kind: TimeKind, hhmm: &str, from: [f64; 2], to: [f64; 2]| RouteRequest {
        waypoints: vec![
            Waypoint { lon: from[0], lat: from[1], name: None },
            Waypoint { lon: to[0], lat: to[1], name: None },
        ],
        mode: Mode::Transit,
        time: TimeSpec { kind, iso: Some(format!("{}T{}:00+02:00", date.format("%Y-%m-%d"), hhmm)) },
        avoid: Default::default(),
        alternatives: 5,
        ..Default::default()
    };
    let mk_via = |kind: TimeKind, hhmm: &str, pts: &[[f64; 2]]| {
        let mut r = mk(kind, hhmm, pts[0], pts[pts.len() - 1]);
        r.waypoints = pts.iter().map(|p| Waypoint { lon: p[0], lat: p[1], name: None }).collect();
        r
    };
    let ta = [34.7794, 32.0557];
    let hahagana = [34.7863, 32.0541];
    let modiin = [35.0070, 31.9076];
    let jlm = [35.2033, 31.7890];

    for (label, req) in [
        ("TLV→JLM depart 08:00", mk(TimeKind::Depart, "08:00", ta, jlm)),
        ("TLV→JLM arrive by 10:00", mk(TimeKind::Arrive, "10:00", ta, jlm)),
        ("JLM→TLV depart 17:30", mk(TimeKind::Depart, "17:30", jlm, ta)),
        ("TLV short hop depart 08:00", mk(TimeKind::Depart, "08:00", ta, [34.7818, 32.0853])),
        ("VIA HaHagana→Modiin→JLM depart 08:00", mk_via(TimeKind::Depart, "08:00", &[hahagana, modiin, jlm])),
        ("VIA HaHagana→Modiin→JLM arrive by 10:00", mk_via(TimeKind::Arrive, "10:00", &[hahagana, modiin, jlm])),
    ] {
        let t1 = Instant::now();
        let r = t.route(&req, &NoWalk).expect("route");
        println!("\n{label} on {date}: {} routes in {} ms", r.routes.len(), t1.elapsed().as_millis());
        for rt in &r.routes {
            println!("  {}", summary(rt));
        }
        if !label.contains("short") {
            assert!(r.routes.iter().any(|x| x.transfers <= 2 && x.legs.iter().any(|l| l.transit.is_some())));
        }
        if label.contains("arrive") {
            for rt in &r.routes {
                assert!(rt.arrive_iso.as_deref().unwrap()[11..16] <= *"10:00");
            }
        }
    }

    let t1 = Instant::now();
    let s = t.search_stops("\u{05EA}\u{05D7}\u{05E0}\u{05D4} \u{05DE}\u{05E8}\u{05DB}\u{05D6}\u{05D9}\u{05EA}", Some(jlm), 5);
    println!("\nsearch_stops in {} ms:", t1.elapsed().as_millis());
    for g in &s {
        println!("  {} {:?} {:?} {:.4},{:.4}", g.name, g.city, g.category, g.lon, g.lat);
    }
}
