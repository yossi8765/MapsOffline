//! Real-data geocoder test. Skipped unless MAPSOFFLINE_PBF points to an OSM PBF of Israel.
use std::time::Instant;

use mapsoffline_lib::geocode;
use mapsoffline_lib::geo::haversine_m;
use mapsoffline_lib::types::{GeoKind, GeoPrecision, ProgressPhase};

#[test]
fn geocode_real() {
    let Ok(pbf) = std::env::var("MAPSOFFLINE_PBF") else {
        eprintln!("MAPSOFFLINE_PBF not set - skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mapsoffline_geocode_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("geocode.db");

    let t = Instant::now();
    let cb = |ph: ProgressPhase, done: u64, total: u64, msg: &str| {
        eprintln!("[{ph:?}] {done}/{total} {msg} ({:.1}s)", t.elapsed().as_secs_f32());
        true
    };
    geocode::build(std::path::Path::new(&pbf), &db, &cb).expect("build");
    let build_t = t.elapsed();
    let size = std::fs::metadata(&db).unwrap().len();
    eprintln!("BUILD {build_t:?}, size {:.1} MB", size as f64 / 1e6);

    let t = Instant::now();
    let g = geocode::open(&db).expect("open");
    eprintln!("OPEN {:?}", t.elapsed());

    let mut worst = 0u128;
    let mut run = |q: &str, near: Option<[f64; 2]>| {
        let t = Instant::now();
        let r = g.search(q, near, 8).expect("search");
        let ms = t.elapsed().as_micros();
        worst = worst.max(ms);
        eprintln!("SEARCH {q:?} -> {} results in {:.1} ms", r.len(), ms as f64 / 1000.0);
        for x in r.iter().take(4) {
            eprintln!("   {:?}/{:?} {} ({:.5},{:.5}) score {:.1}", x.kind, x.precision, x.name, x.lon, x.lat, x.score);
        }
        r
    };

    // exact address
    let r = run("הרצל 12 תל אביב", None);
    let a = r.first().expect("address result");
    assert_eq!(a.kind, GeoKind::Address);
    assert!(matches!(a.precision, GeoPrecision::Exact | GeoPrecision::Interpolated), "{:?}", a.precision);
    assert!(haversine_m([a.lon, a.lat], [34.77, 32.06]) < 3000.0, "{} {}", a.lon, a.lat);

    run("הרצל 12, תל אביב", None);
    run("תל אביב הרצל 12", None);
    run("רחוב הרצל 12 ירושלים", None);
    run("דיזנגוף 100", Some([34.78, 32.08]));
    run("הרצל 9999 תל אביב", None);

    // street
    let r = run("דיזנגוף", None);
    let s = r.first().expect("street result");
    assert_eq!(s.kind, GeoKind::Street);
    assert!(s.name.contains("תל אביב"), "{}", s.name);

    // city
    let r = run("ירושלים", None);
    assert_eq!(r[0].kind, GeoKind::Place);
    assert!(r[0].name.starts_with("ירושלים"));

    run("תל אב", None);
    run("haifa", None);

    // poi via typo tolerance
    let r = run("אזריאלי", None);
    assert!(r.iter().any(|x| x.kind == GeoKind::Poi && x.name.contains("עזריאלי")), "azrieli not found");
    run("עזריאלי", Some([34.79, 32.07]));
    run("32.0853, 34.7818", None);

    // reverse
    let t = Instant::now();
    let rv = g.reverse(34.7818, 32.0853).expect("reverse");
    eprintln!("REVERSE {:?} in {:?}", rv.as_ref().map(|x| (&x.name, x.precision)), t.elapsed());
    assert!(rv.is_some());
    let t = Instant::now();
    let rv2 = g.reverse(35.0, 30.5).expect("reverse far");
    eprintln!("REVERSE(desert) {:?} in {:?}", rv2.as_ref().map(|x| (&x.name, x.precision)), t.elapsed());

    // timing of repeated searches (warm)
    let t = Instant::now();
    for _ in 0..20 {
        g.search("הרצל", None, 8).unwrap();
        g.search("הרצל 12 תל אביב", None, 8).unwrap();
    }
    eprintln!("WARM avg {:.2} ms/search", t.elapsed().as_secs_f64() * 1000.0 / 40.0);
    eprintln!("WORST search {:.1} ms", worst as f64 / 1000.0);
    assert!(worst < 200_000, "search too slow");
    let _ = std::fs::remove_dir_all(&dir);
}
