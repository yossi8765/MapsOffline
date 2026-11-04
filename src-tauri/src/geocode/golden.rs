//! Golden search queries on real data (builder revision 4 index built from the installed OSM
//! extract into a temp file, no network): every query must keep answering the same way.
//!
//! `cargo test --release --features cli --lib geocode::golden -- --ignored --nocapture`
//! (MO_GEOCODE_INDEX=<file> reuses an index instead of building one; delete the temp file
//! `mo-geocode-golden.sqlite` to rebuild).
use std::time::Instant;

use super::*;
use crate::types::GeoKind;

const TLV: Option<LonLat> = Some([34.78, 32.08]);
const JLM: Option<LonLat> = Some([35.21, 31.78]);

fn index() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MO_GEOCODE_INDEX") {
        return p.into();
    }
    let out = std::env::temp_dir().join("mo-geocode-golden.sqlite");
    if !out.exists() {
        let root = crate::paths::DataPaths::default_root();
        let pbf = root.join("downloads").join("israel-and-palestine-latest.osm.pbf");
        let t = Instant::now();
        builder::build_at(&pbf, &out, 31.5, &[], &[], false, &|_, _, _, _| true).unwrap();
        println!("built {} in {:?}", out.display(), t.elapsed());
    }
    out
}

enum Want {
    /// first result of this kind whose name contains the text
    First(GeoKind, &'static str),
    /// first result is a coordinate within ~50 m of (lon, lat)
    Coord(f64, f64),
    /// first result within `km` of (lon, lat)
    Near(f64, f64, f64),
    /// at least one result of this kind
    Any(GeoKind),
}

#[test]
#[ignore]
fn golden_queries() {
    let g = open(&index()).unwrap();
    println!("caps {:?}", g.caps);
    assert!(g.caps.poi_info && g.caps.place_info && g.caps.addr_src && g.caps.wiki, "revision 4 index expected");
    let n_interp: i64 = g.conn.query_row("SELECT count(*) FROM addresses WHERE src = 1", [], |r| r.get(0)).unwrap();
    let n_nameless: i64 = g.conn.query_row("SELECT count(*) FROM pois WHERE name = ''", [], |r| r.get(0)).unwrap();
    let n_info: i64 = g.conn.query_row("SELECT count(*) FROM pois WHERE info IS NOT NULL", [], |r| r.get(0)).unwrap();
    let n_land: i64 = g.conn.query_row("SELECT count(*) FROM places WHERE kind = 'landmark'", [], |r| r.get(0)).unwrap();
    println!("interpolated addresses {n_interp}, unnamed POIs {n_nameless}, POIs with details {n_info}, landmarks {n_land}");
    assert!(n_nameless > 1000 && n_info > 1000 && n_land > 50);

    // a plus code for Tel Aviv, and its short form (relative to `near`)
    let full = format!("{}+22", crate::coords::olc::encode_prefix(32.08, 34.78, 8));
    let short = full[4..].to_string();
    let cases: Vec<(&str, Option<LonLat>, Want)> = vec![
        ("הרצל 12 תל אביב", None, Want::First(GeoKind::Address, "הרצל 12")),
        ("דיזנגוף 50 תל אביב", None, Want::First(GeoKind::Address, "דיזנגוף 50")),
        ("Herzl 12 Tel Aviv", None, Want::First(GeoKind::Address, "תל אביב")),
        ("Dizengoff 50", TLV, Want::First(GeoKind::Address, "50")),
        ("vrmk 12 ,k tchc", None, Want::First(GeoKind::Address, "הרצל 12")),
        ("הרצאל 5 תל אביב", None, Want::First(GeoKind::Address, "הרצל 5")),
        ("בן יהודה 20 ירושלים", None, Want::First(GeoKind::Address, "בן יהודה 20")),
        ("תל אביב", None, Want::First(GeoKind::Place, "תל אביב")),
        ("ירושלים", None, Want::First(GeoKind::Place, "ירושלים")),
        ("Jerusalem", None, Want::First(GeoKind::Place, "ירושלים")),
        ("Haifa", None, Want::First(GeoKind::Place, "חיפה")),
        ("כביש 6", TLV, Want::First(GeoKind::Street, "כביש 6")),
        // squares: the biggest town first (Tel Aviv's Rabin Square is an OSM relation – not read)
        ("כיכר ציון", None, Want::Near(35.2196, 31.7819, 0.5)),
        ("כיכר המדינה", None, Want::Near(34.7897, 32.0868, 0.5)),
        ("כיכר רבין תל אביב", None, Want::Near(34.7817, 32.0809, 1.0)),
        ("178824 665036", None, Want::Coord(34.774_082, 32.077_885)),
        ("665036/178824", None, Want::Coord(34.774_082, 32.077_885)),
        ("ITM 178824 665036", None, Want::Coord(34.774_082, 32.077_885)),
        ("32°04′48″N 34°46′48″E", None, Want::Coord(34.78, 32.08)),
        ("32 4 48 צ 34 46 48 מ", None, Want::Coord(34.78, 32.08)),
        ("32.08, 34.78", None, Want::Coord(34.78, 32.08)),
        ("34.78 32.08", None, Want::Coord(34.78, 32.08)),
        ("48.2082, 16.3738", None, Want::Coord(16.3738, 48.2082)),
        ("בית כנסת", JLM, Want::Any(GeoKind::Poi)),
        ("ברזייה", TLV, Want::Any(GeoKind::Poi)),
        ("שירותים ציבוריים", TLV, Want::Any(GeoKind::Poi)),
        ("מעיין", JLM, Want::Any(GeoKind::Poi)),
        ("מסגד", JLM, Want::Any(GeoKind::Poi)),
        ("תצפית", JLM, Want::Any(GeoKind::Poi)),
        ("רמי לוי", JLM, Want::Any(GeoKind::Poi)),
    ];
    let mut owned: Vec<(String, Option<LonLat>, Want)> = cases.into_iter().map(|(q, n, w)| (q.to_string(), n, w)).collect();
    // an 8-digit plus code cell is ~275 m across ("+22" picks a corner of it)
    owned.push((full, None, Want::Near(34.78, 32.08, 0.5)));
    owned.push((short, TLV, Want::Near(34.78, 32.08, 0.5)));
    let (_, _, e, n) = crate::coords::utm::from_wgs84(34.78, 32.08);
    owned.push((format!("36S {} {}", e.round(), n.round()), None, Want::Coord(34.78, 32.08)));
    // a phone number of an indexed place finds that place
    let phone: Option<(i64, String)> = g
        .conn
        .query_row("SELECT id, info FROM pois WHERE info LIKE '%\"p\":\"0%' LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok();
    let mut fails = Vec::new();
    let mut total_ms = 0.0;
    for (q, near, want) in &owned {
        let t = Instant::now();
        let r = g.search(q, *near, 5).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total_ms += ms;
        let first = r.first();
        let ok = match want {
            Want::First(kind, text) => first.is_some_and(|f| f.kind == *kind && f.name.contains(text)),
            Want::Coord(lon, lat) => first.is_some_and(|f| f.kind == GeoKind::Coord && haversine_m([f.lon, f.lat], [*lon, *lat]) < 50.0),
            Want::Near(lon, lat, km) => first.is_some_and(|f| haversine_m([f.lon, f.lat], [*lon, *lat]) < km * 1000.0),
            Want::Any(kind) => r.iter().any(|f| f.kind == *kind),
        };
        println!(
            "{} {ms:6.1} ms  {q}  ->  {}",
            if ok { "ok  " } else { "FAIL" },
            first.map(|f| format!("{:?} {} ({:.5},{:.5})", f.kind, f.name, f.lon, f.lat)).unwrap_or_else(|| "nothing".into())
        );
        if !ok {
            fails.push(q.clone());
        }
    }
    if let Some((id, info)) = phone {
        let p = info::PoiInfo::from_json(Some(&info)).and_then(|i| i.phone).unwrap();
        let r = g.search(&p, TLV, 5).unwrap();
        let ok = r.iter().any(|x| x.id == format!("poi:{id}"));
        println!("{} phone {p} -> {:?}", if ok { "ok  " } else { "FAIL" }, r.first().map(|x| &x.name));
        if !ok {
            fails.push(format!("phone {p}"));
        }
    }
    println!("{} queries, {:.1} ms on average", owned.len(), total_ms / owned.len() as f64);
    assert!(fails.is_empty(), "failed: {fails:?}");
}
