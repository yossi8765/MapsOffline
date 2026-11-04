//! Integration test on the real Israel extract. Skipped unless `MAPSOFFLINE_PBF` is set.
//! Run: MAPSOFFLINE_PBF=<path> cargo test --release --test routing_real -- --nocapture
use std::path::PathBuf;
use std::time::Instant;

use mapsoffline_lib::routing;
use mapsoffline_lib::types::*;

fn req(from: [f64; 2], to: [f64; 2], mode: Mode, alternatives: u32) -> RouteRequest {
    RouteRequest {
        waypoints: vec![Waypoint { lon: from[0], lat: from[1], name: None }, Waypoint { lon: to[0], lat: to[1], name: None }],
        mode,
        time: TimeSpec::default(),
        accessible: false,
        avoid: Avoid::default(),
        alternatives,
    }
}

#[test]
fn real_israel_routing() {
    let Ok(pbf) = std::env::var("MAPSOFFLINE_PBF") else {
        eprintln!("MAPSOFFLINE_PBF not set – skipping");
        return;
    };
    let pbf = PathBuf::from(pbf);
    assert!(pbf.exists(), "PBF not found: {}", pbf.display());
    let out_dir = std::env::temp_dir().join("mapsoffline-test");
    std::fs::create_dir_all(&out_dir).unwrap();
    let graph_file = out_dir.join("graph.bin");

    let reuse = std::env::var("MAPSOFFLINE_REUSE_GRAPH").is_ok() && graph_file.exists();
    if !reuse {
        let t = Instant::now();
        let progress = |ph: ProgressPhase, d: u64, tot: u64, msg: &str| {
            eprintln!("  [{ph:?}] {d}/{tot} {msg}");
            true
        };
        routing::build_graph(&pbf, &graph_file, &progress).expect("build graph");
        let secs = t.elapsed().as_secs_f64();
        let size = std::fs::metadata(&graph_file).unwrap().len();
        eprintln!("BUILD: {secs:.1}s, file {:.1} MB", size as f64 / 1e6);
        assert!(secs < 240.0, "graph build too slow: {secs}s");
        assert!(size < 150_000_000, "graph file too big: {size}");
    }

    let t = Instant::now();
    let g = routing::load_graph(&graph_file).expect("load graph");
    eprintln!("LOAD: {:?}; {} vertices, {} edges, {} restrictions", t.elapsed(), g.num_vertices(), g.num_edges(), g.restrictions.len());
    assert!(g.num_edges() > 300_000);
    let release = !cfg!(debug_assertions);

    let tlv = [34.7818, 32.0853];
    let jlm = [35.2137, 31.7683];
    let haifa = [34.9896, 32.7940];
    let eilat = [34.9519, 29.5577];
    let ks = [35.5695, 33.2075];

    // --- car TLV -> JLM with alternatives
    let t = Instant::now();
    let r = g.route(&req(tlv, jlm, Mode::Car, 3)).expect("tlv->jlm");
    let ms = t.elapsed().as_millis();
    eprintln!("TLV->JLM car: {} routes in {ms} ms (compute_ms={})", r.routes.len(), r.compute_ms);
    for rt in &r.routes {
        eprintln!("   {} {:.1} km {:.0} min tags={:?} legs={} steps={}", rt.id, rt.distance_m / 1000.0, rt.duration_sec / 60.0, rt.tags, rt.legs.len(), rt.legs[0].steps.len());
    }
    let best = &r.routes[0];
    assert!(best.distance_m > 50_000.0 && best.distance_m < 80_000.0, "unexpected distance {}", best.distance_m);
    assert!(best.duration_sec > 30.0 * 60.0 && best.duration_sec < 90.0 * 60.0);
    assert!(best.tags.contains(&RouteTag::Fastest));
    assert!(best.legs[0].steps.len() >= 3);
    assert_eq!(best.legs[0].steps.first().unwrap().maneuver, Maneuver::Depart);
    assert_eq!(best.legs[0].steps.last().unwrap().maneuver, Maneuver::Arrive);
    for s in &best.legs[0].steps {
        eprintln!("      {:>6.0}m  {}", s.distance_m, s.instruction);
    }
    assert!(best.geometry.len() > 100);
    // geometry starts/ends near the waypoints
    let g0 = best.geometry[0];
    let g1 = *best.geometry.last().unwrap();
    assert!((g0[0] - tlv[0]).abs() < 0.01 && (g0[1] - tlv[1]).abs() < 0.01);
    assert!((g1[0] - jlm[0]).abs() < 0.01 && (g1[1] - jlm[1]).abs() < 0.01);

    // --- car TLV -> Haifa
    let t = Instant::now();
    let r = g.route(&req(tlv, haifa, Mode::Car, 2)).expect("tlv->haifa");
    eprintln!("TLV->Haifa car: {} routes, {:.1} km, {:.0} min in {:?}", r.routes.len(), r.routes[0].distance_m / 1000.0, r.routes[0].duration_sec / 60.0, t.elapsed());
    assert!(r.routes[0].distance_m > 80_000.0 && r.routes[0].distance_m < 120_000.0);

    // --- long: Eilat -> Kiryat Shmona
    let t = Instant::now();
    let r = g.route(&req(eilat, ks, Mode::Car, 1)).expect("eilat->ks");
    let ms = t.elapsed().as_millis();
    eprintln!("Eilat->KS car: {:.1} km, {:.0} min in {ms} ms", r.routes[0].distance_m / 1000.0, r.routes[0].duration_sec / 60.0);
    assert!(r.routes[0].distance_m > 400_000.0 && r.routes[0].distance_m < 560_000.0);
    if release {
        assert!(ms < 300, "long route too slow: {ms} ms");
    }

    // --- urban walking in Tel Aviv (Dizengoff Center -> Rabin Square)
    let a = [34.7748, 32.0754];
    let b = [34.7806, 32.0804];
    let t = Instant::now();
    let r = g.route(&req(a, b, Mode::Foot, 2)).expect("walk");
    let ms = t.elapsed().as_millis();
    eprintln!("TLV walk: {} routes, {:.0} m, {:.0} min in {ms} ms", r.routes.len(), r.routes[0].distance_m, r.routes[0].duration_sec / 60.0);
    for s in &r.routes[0].legs[0].steps {
        eprintln!("      {:>6.0}m  {}", s.distance_m, s.instruction);
    }
    assert!(r.routes[0].distance_m > 600.0 && r.routes[0].distance_m < 1_600.0);
    assert_eq!(r.routes[0].legs[0].kind, LegKind::Walk);
    if release {
        assert!(ms < 50, "urban walk too slow: {ms} ms");
    }

    // --- urban car
    let t = Instant::now();
    let r = g.route(&req(a, [34.7925, 32.0612], Mode::Car, 3)).expect("urban car");
    let ms = t.elapsed().as_millis();
    eprintln!("TLV urban car: {} routes, {:.0} m in {ms} ms", r.routes.len(), r.routes[0].distance_m);
    if release {
        assert!(ms < 50, "urban car too slow: {ms} ms");
    }

    // --- bike
    let r = g.route(&req(tlv, [34.8, 32.1], Mode::Bike, 1)).expect("bike");
    eprintln!("bike: {:.0} m {:.0} min", r.routes[0].distance_m, r.routes[0].duration_sec / 60.0);
    assert_eq!(r.routes[0].legs[0].kind, LegKind::Bike);

    // --- via points
    let mut rq = req(tlv, jlm, Mode::Car, 3);
    rq.waypoints.insert(1, Waypoint { lon: 34.8, lat: 31.95, name: None });
    let r = g.route(&rq).expect("via");
    assert_eq!(r.routes.len(), 1);
    assert_eq!(r.routes[0].legs.len(), 2);

    // --- walk leg helper (transit access)
    let w = g.walk(a, b).expect("walk leg");
    assert!(w.distance_m > 600.0 && w.distance_m < 1_600.0);
    assert!(g.walk(tlv, jlm).is_none());

    // --- avoid toll changes nothing structurally
    let mut rq = req(tlv, haifa, Mode::Car, 1);
    rq.avoid.toll = true;
    let r = g.route(&rq).expect("avoid toll");
    eprintln!("TLV->Haifa avoid toll: {:.1} km {:.0} min", r.routes[0].distance_m / 1000.0, r.routes[0].duration_sec / 60.0);
}
