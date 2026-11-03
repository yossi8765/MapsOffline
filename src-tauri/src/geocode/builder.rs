//! Geocoder index builder: OSM PBF -> SQLite (+FTS5).
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use osmpbf::{BlobDecode, BlobReader, Element, ElementReader};
use rstar::{PointDistance, RTree, RTreeObject, AABB};
use rusqlite::{params, Connection};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use super::norm::{canon, housenumber_key, index_tokens};
use crate::types::{ProgressFn, ProgressPhase};

pub const SCHEMA_VERSION: &str = "3";
/// Builder revision (same file format): 1 = data.gov.il points of interest, 2 = numbered roads
/// indexed as streets "כביש N". Part of the manifest version so a new revision shows up as an
/// available update.
pub const BUILD_REV: u32 = 2;
thread_local! {
    /// cos(reference latitude) for the flat-earth distances of one build (the build runs on one
    /// thread; countries are built concurrently on different threads). Israel: cos(31.5°).
    static COS_LAT_TL: std::cell::Cell<f64> = const { std::cell::Cell::new(0.8526) };
}
#[allow(non_snake_case)]
fn COS_LAT() -> f64 {
    COS_LAT_TL.with(|c| c.get())
}

// ------------------------------------------------------------------ tag classification

#[derive(Default, Clone)]
struct Names {
    he: String,
    en: String,
    ar: String,
}

impl Names {
    fn primary(&self) -> &str {
        if !self.he.is_empty() {
            &self.he
        } else if !self.en.is_empty() {
            &self.en
        } else {
            &self.ar
        }
    }
    fn all(&self) -> [&str; 3] {
        [&self.he, &self.en, &self.ar]
    }
}

struct Roles {
    names: Names,
    place: Option<(&'static str, i64)>,
    street: bool,
    addr: Option<(String, String)>, // (housenumber, street)
    addr_city: Option<String>,
    poi: Option<&'static str>, // category key index; value stored in poi_val
    poi_val: String,
    /// Route numbers of a numbered road ("6", "1;20" -> ["1", "20"]): indexed as streets named
    /// "כביש N" so "כביש 6" can be searched (most stretches of national roads have no name).
    road_refs: Vec<String>,
}

fn road_highway(v: &str) -> bool {
    matches!(v, "motorway" | "trunk" | "primary" | "secondary" | "tertiary")
}

/// The numeric route numbers in a `ref` tag ("1;20", "6", "443"); anything else (letters,
/// "כביש 6 (מנהרה)") is ignored.
fn numeric_refs(v: &str) -> Vec<String> {
    v.split(';')
        .map(str::trim)
        .filter(|r| !r.is_empty() && r.len() <= 4 && r.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

fn has_arabic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}
fn has_latin(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic())
}

const POI_KEYS: [&str; 11] = [
    "amenity",
    "shop",
    "tourism",
    "leisure",
    "railway",
    "public_transport",
    "aeroway",
    "historic",
    "natural",
    "office",
    "healthcare",
];

fn poi_ok(idx: usize, v: &str) -> bool {
    match idx {
        0 => !matches!(
            v,
            "bench" | "waste_basket" | "parking_space" | "bicycle_parking" | "shelter" | "post_box" | "recycling" | "vending_machine" | "parking_entrance" | "waste_disposal" | "bbq" | "drinking_water" | "clock" | "grave_yard_x"
        ),
        3 => !matches!(v, "pitch_x"),
        4 | 5 => v == "station" || v == "halt",
        6 => !matches!(v, "runway" | "taxiway" | "taxilane" | "apron" | "gate" | "holding_position" | "parking_position"),
        8 => !matches!(v, "tree" | "tree_row" | "wood" | "scrub" | "grassland" | "heath" | "bare_rock" | "scree" | "cliff" | "coastline" | "valley" | "sand" | "wetland" | "fell" | "shingle" | "mud"),
        _ => !v.is_empty() && v != "no" && v != "yes_x",
    }
}

fn place_kind(v: &str) -> Option<&'static str> {
    Some(match v {
        "city" => "city",
        "town" => "town",
        "village" => "village",
        "hamlet" => "hamlet",
        "suburb" => "suburb",
        "neighbourhood" => "neighbourhood",
        "quarter" => "quarter",
        _ => return None,
    })
}

fn street_highway(v: &str) -> bool {
    matches!(
        v,
        "motorway" | "trunk" | "primary" | "secondary" | "tertiary" | "unclassified" | "residential" | "living_street" | "pedestrian" | "road"
    )
}

fn classify<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Option<Roles> {
    let (mut name, mut name_he, mut name_en, mut name_ar) = ("", "", "", "");
    let (mut place, mut highway, mut hn, mut street, mut addr_place, mut city) = ("", "", "", "", "", "");
    let mut religion = "";
    let mut road_ref = "";
    let mut pop = 0i64;
    let mut pc: [&str; 11] = [""; 11];
    for (k, v) in tags {
        match k {
            "name" => name = v,
            "name:he" => name_he = v,
            "name:en" => name_en = v,
            "name:ar" => name_ar = v,
            "place" => place = v,
            "highway" => highway = v,
            "ref" => road_ref = v,
            "addr:housenumber" => hn = v,
            "addr:street" => street = v,
            "addr:place" => addr_place = v,
            "addr:city" => city = v,
            "religion" => religion = v,
            "population" => pop = v.trim().replace(',', "").parse().unwrap_or(0),
            _ => {
                if let Some(i) = POI_KEYS.iter().position(|x| *x == k) {
                    pc[i] = v;
                }
            }
        }
    }
    let primary = if !name_he.is_empty() { name_he } else { name };
    let road_refs = if road_highway(highway) { numeric_refs(road_ref) } else { Vec::new() };
    if primary.is_empty() && hn.is_empty() && road_refs.is_empty() {
        return None;
    }
    let mut names = Names { he: primary.to_string(), en: name_en.to_string(), ar: name_ar.to_string() };
    if !name.is_empty() && name != primary {
        if names.en.is_empty() && has_latin(name) {
            names.en = name.to_string();
        } else if names.ar.is_empty() && has_arabic(name) {
            names.ar = name.to_string();
        }
    }
    let place = if primary.is_empty() { None } else { place_kind(place).map(|k| (k, pop)) };
    let street_role = !primary.is_empty() && street_highway(highway);
    let st = if !street.is_empty() { street } else { addr_place };
    let addr = if !hn.is_empty() && !st.is_empty() { Some((hn.to_string(), st.to_string())) } else { None };
    let mut poi = None;
    let mut poi_val = String::new();
    if !primary.is_empty() {
        for i in 0..POI_KEYS.len() {
            if !pc[i].is_empty() && poi_ok(i, pc[i]) {
                poi = Some(POI_KEYS[i]);
                poi_val = pc[i].to_string();
                if poi_val == "place_of_worship" && matches!(religion, "jewish" | "muslim" | "christian") {
                    poi_val = format!("place_of_worship:{religion}");
                }
                break;
            }
        }
    }
    if place.is_none() && !street_role && addr.is_none() && poi.is_none() && road_refs.is_empty() {
        return None;
    }
    Some(Roles {
        names,
        place,
        street: street_role,
        addr,
        addr_city: if city.is_empty() { None } else { Some(city.to_string()) },
        poi,
        poi_val,
        road_refs,
    })
}

// ------------------------------------------------------------------ pass 1 state

#[derive(Default)]
struct Interner {
    map: FxHashMap<String, u32>,
    vec: Vec<String>,
}
impl Interner {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.vec.len() as u32;
        self.vec.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }
}

type Pos = Option<(f64, f64)>;
type Span = (u32, u32);

struct PlaceRaw {
    names: Names,
    kind: &'static str,
    pop: i64,
    pos: Pos,
    refs: Span,
}
struct StreetRaw {
    names: Names,
    refs: Span,
}
struct AddrRaw {
    street: u32,
    hn: Box<str>,
    city: Option<u32>,
    pos: Pos,
    refs: Span,
}
struct PoiRaw {
    names: Names,
    cat: u32,
    pos: Pos,
    refs: Span,
}

#[derive(Default)]
struct State {
    arena: Vec<i64>,
    places: Vec<PlaceRaw>,
    streets: Vec<StreetRaw>,
    addrs: Vec<AddrRaw>,
    pois: Vec<PoiRaw>,
    strs: Interner,
    nodes_seen: u64,
}

impl State {
    fn add(&mut self, r: Roles, pos: Pos, refs: &[i64]) {
        let span = if refs.is_empty() {
            (0, 0)
        } else {
            let s = self.arena.len() as u32;
            self.arena.extend_from_slice(refs);
            (s, refs.len() as u32)
        };
        if let Some((kind, pop)) = r.place {
            self.places.push(PlaceRaw { names: r.names.clone(), kind, pop, pos, refs: span });
        }
        if r.street && pos.is_none() {
            self.streets.push(StreetRaw { names: r.names.clone(), refs: span });
        }
        if pos.is_none() {
            for n in &r.road_refs {
                let names = Names { he: format!("כביש {n}"), en: format!("Route {n}"), ar: String::new() };
                self.streets.push(StreetRaw { names, refs: span });
            }
        }
        if let Some((hn, st)) = r.addr {
            let street = self.strs.id(&st);
            let city = r.addr_city.as_deref().map(|c| self.strs.id(c));
            self.addrs.push(AddrRaw { street, hn: hn.into_boxed_str(), city, pos, refs: span });
        }
        if r.poi.is_some() {
            let cat = self.strs.id(&r.poi_val);
            self.pois.push(PoiRaw { names: r.names, cat, pos, refs: span });
        }
    }

    fn node<'a>(&mut self, tags: impl Iterator<Item = (&'a str, &'a str)>, lon: f64, lat: f64) {
        self.nodes_seen += 1;
        if let Some(r) = classify(tags) {
            self.add(r, Some((lon, lat)), &[]);
        }
    }
}

// ------------------------------------------------------------------ geometry helpers

const E7: f64 = 1e7;

fn centroid(pts: &[(i32, i32)]) -> Option<(f64, f64)> {
    if pts.is_empty() {
        return None;
    }
    let (mut sx, mut sy) = (0f64, 0f64);
    for p in pts {
        sx += p.0 as f64;
        sy += p.1 as f64;
    }
    let n = pts.len() as f64;
    Some((sx / n / E7, sy / n / E7))
}

fn seg_len_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    let dx = (b.0 - a.0) as f64 / E7 * COS_LAT() * 111_320.0;
    let dy = (b.1 - a.1) as f64 / E7 * 110_574.0;
    (dx * dx + dy * dy).sqrt()
}

fn simplify(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let len: f64 = pts.windows(2).map(|w| seg_len_m(w[0], w[1])).sum();
    let n = ((len / 80.0).ceil() as usize + 1).clamp(2, 40).min(pts.len());
    if n >= pts.len() {
        return pts.to_vec();
    }
    (0..n).map(|i| pts[i * (pts.len() - 1) / (n - 1)]).collect()
}

/// Blob: repeated [u16 count][count * (i32 lon_e7, i32 lat_e7)] little endian.
pub(crate) fn encode_geom(ways: &[Vec<(i32, i32)>]) -> Vec<u8> {
    let mut b = Vec::new();
    for w in ways {
        b.extend_from_slice(&(w.len() as u16).to_le_bytes());
        for p in w {
            b.extend_from_slice(&p.0.to_le_bytes());
            b.extend_from_slice(&p.1.to_le_bytes());
        }
    }
    b
}

pub(crate) fn decode_geom(b: &[u8]) -> Vec<Vec<[f64; 2]>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 <= b.len() {
        let n = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
        i += 2;
        let mut w = Vec::with_capacity(n);
        for _ in 0..n {
            if i + 8 > b.len() {
                break;
            }
            let lon = i32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            let lat = i32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]);
            w.push([lon as f64 / E7, lat as f64 / E7]);
            i += 8;
        }
        out.push(w);
    }
    out
}

// ------------------------------------------------------------------ city index

struct CityPt {
    xy: [f64; 2],
    idx: u32,
}
impl RTreeObject for CityPt {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_point(self.xy)
    }
}
impl PointDistance for CityPt {
    fn distance_2(&self, p: &[f64; 2]) -> f64 {
        let dx = self.xy[0] - p[0];
        let dy = self.xy[1] - p[1];
        dx * dx + dy * dy
    }
}

struct CityIndex {
    tree: RTree<CityPt>,
    names: Vec<String>,
    pops: Vec<i64>,
}

impl CityIndex {
    fn nearest(&self, lon: f64, lat: f64) -> Option<usize> {
        let q = [lon * COS_LAT(), lat];
        let mut best: Option<(f64, usize)> = None;
        for c in self.tree.nearest_neighbor_iter(&q).take(6) {
            let d = c.distance_2(&q).sqrt() * 111_320.0;
            let pop = self.pops[c.idx as usize] as f64;
            let radius = (200.0 * (pop / 1000.0).sqrt()).max(600.0);
            let eff = d / radius;
            if d > 30_000.0 {
                continue;
            }
            if best.is_none_or(|(b, _)| eff < b) {
                best = Some((eff, c.idx as usize));
            }
        }
        best.map(|b| b.1)
    }
}

fn default_pop(kind: &str) -> i64 {
    match kind {
        "city" => 50_000,
        "town" => 12_000,
        "village" => 1_500,
        "hamlet" => 300,
        "suburb" => 15_000,
        _ => 5_000,
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "city" => 0,
        "town" => 1,
        "village" => 2,
        "suburb" => 3,
        "hamlet" => 4,
        "neighbourhood" => 5,
        _ => 6,
    }
}

fn hash_key<T: Hash>(t: &T) -> u64 {
    let mut h = FxHasher::default();
    t.hash(&mut h);
    h.finish()
}

fn name_of(n: &Names) -> String {
    n.primary().to_string()
}

// ------------------------------------------------------------------ build

const SCHEMA: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE places(id INTEGER PRIMARY KEY, name_he TEXT, name_en TEXT, name_ar TEXT, kind TEXT,
    lon REAL, lat REAL, population INTEGER, city TEXT);
CREATE TABLE streets(id INTEGER PRIMARY KEY, name TEXT, city TEXT, ckey TEXT, city_key TEXT,
    lon REAL, lat REAL, minlon REAL, minlat REAL, maxlon REAL, maxlat REAL, geom BLOB);
CREATE TABLE addresses(id INTEGER PRIMARY KEY, street TEXT, housenumber TEXT, housenumber_num INTEGER,
    city TEXT, lon REAL, lat REAL, street_key TEXT, hn_key TEXT);
CREATE TABLE pois(id INTEGER PRIMARY KEY, name TEXT, category TEXT, lon REAL, lat REAL, city TEXT);
CREATE VIRTUAL TABLE places_fts USING fts5(search, content='', tokenize='unicode61 remove_diacritics 2', detail=none, prefix='2 3');
CREATE VIRTUAL TABLE streets_fts USING fts5(search, content='', tokenize='unicode61 remove_diacritics 2', detail=none, prefix='2 3');
CREATE VIRTUAL TABLE pois_fts USING fts5(search, content='', tokenize='unicode61 remove_diacritics 2', detail=none, prefix='2 3');
CREATE VIRTUAL TABLE streets_rt USING rtree(id, minlon, maxlon, minlat, maxlat);
CREATE VIRTUAL TABLE pois_rt USING rtree(id, minlon, maxlon, minlat, maxlat);
";

fn resolve(span: Span, arena: &[i64], needed: &[i64], coords: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let (s, n) = (span.0 as usize, span.1 as usize);
    let mut out = Vec::with_capacity(n);
    for id in &arena[s..s + n] {
        if let Ok(i) = needed.binary_search(id) {
            let c = coords[i];
            if c.0 != i32::MIN {
                out.push(c);
            }
        }
    }
    out
}

/// Build for a region centred around latitude `ref_lat` (degrees); `extra` points of interest
/// (open-data sources) join the OSM ones unless OSM has the same category within ~100 m.
pub fn build_at(pbf: &Path, out: &Path, ref_lat: f64, extra: &[super::datagov::ExtPoi], progress: ProgressFn) -> Result<()> {
    COS_LAT_TL.with(|c| c.set(ref_lat.to_radians().cos().max(0.2)));
    let t0 = Instant::now();
    let p = |done: u64, msg: &str| progress(ProgressPhase::Processing, done, 100, msg);
    if !p(0, "קריאת נתוני OSM (שלב 1)") {
        bail!("cancelled");
    }

    // ---- pass 1: tagged objects
    let mut st = State::default();
    {
        let reader = ElementReader::from_path(pbf).with_context(|| format!("open {}", pbf.display()))?;
        reader.for_each(|el| match el {
            Element::Node(n) => st.node(n.tags(), n.lon(), n.lat()),
            Element::DenseNode(n) => st.node(n.tags(), n.lon(), n.lat()),
            Element::Way(w) => {
                if let Some(r) = classify(w.tags()) {
                    let refs: Vec<i64> = w.refs().collect();
                    st.add(r, None, &refs);
                }
            }
            Element::Relation(_) => {}
        })?;
    }
    log::info!(
        "geocode pass1 {:?}: places {} streets {} addrs {} pois {}",
        t0.elapsed(),
        st.places.len(),
        st.streets.len(),
        st.addrs.len(),
        st.pois.len()
    );
    if !p(30, "פענוח קואורדינטות (שלב 2)") {
        bail!("cancelled");
    }

    // ---- pass 2: node coordinates for the ways
    let mut needed = st.arena.clone();
    needed.sort_unstable();
    needed.dedup();
    let mut coords = vec![(i32::MIN, 0i32); needed.len()];
    if !needed.is_empty() {
        let (lo, hi) = (needed[0], *needed.last().unwrap());
        let mut put = |id: i64, lon: i32, lat: i32| {
            if id >= lo && id <= hi {
                if let Ok(i) = needed.binary_search(&id) {
                    coords[i] = (lon, lat);
                }
            }
        };
        for blob in BlobReader::from_path(pbf)? {
            let blob = blob?;
            if let BlobDecode::OsmData(block) = blob.decode()? {
                let mut saw_way = false;
                for el in block.elements() {
                    match el {
                        Element::Node(n) => put(n.id(), n.decimicro_lon(), n.decimicro_lat()),
                        Element::DenseNode(n) => put(n.id(), n.decimicro_lon(), n.decimicro_lat()),
                        Element::Way(_) | Element::Relation(_) => {
                            saw_way = true;
                            break;
                        }
                    }
                }
                if saw_way {
                    break;
                }
            }
        }
    }
    log::info!("geocode pass2 {:?}", t0.elapsed());
    if !p(50, "בניית אינדקס יישובים ומקומות") {
        bail!("cancelled");
    }

    // ---- places
    struct PlaceFinal {
        names: Names,
        kind: &'static str,
        pop: i64,
        lon: f64,
        lat: f64,
    }
    let mut pfs: Vec<PlaceFinal> = Vec::new();
    for pr in &st.places {
        let pos = match pr.pos {
            Some(p) => Some(p),
            None => centroid(&resolve(pr.refs, &st.arena, &needed, &coords)),
        };
        if let Some((lon, lat)) = pos {
            let pop = if pr.pop > 0 { pr.pop } else { default_pop(pr.kind) };
            pfs.push(PlaceFinal { names: pr.names.clone(), kind: pr.kind, pop, lon, lat });
        }
    }
    pfs.sort_by(|a, b| kind_rank(a.kind).cmp(&kind_rank(b.kind)).then(b.pop.cmp(&a.pop)));
    // dedupe same-named settlements closer than 3 km
    {
        let mut seen: FxHashMap<(String, &str), Vec<(f64, f64)>> = FxHashMap::default();
        pfs.retain(|p| {
            if matches!(p.kind, "city" | "town" | "village" | "hamlet") {
                let e = seen.entry((canon(p.names.primary()), p.kind)).or_default();
                if e.iter().any(|q| ((q.0 - p.lon) * COS_LAT()).hypot(q.1 - p.lat) * 111_320.0 < 3000.0) {
                    return false;
                }
                e.push((p.lon, p.lat));
            }
            true
        });
    }
    let mut cities = CityIndex { tree: RTree::new(), names: vec![], pops: vec![] };
    {
        let mut pts = Vec::new();
        for p in &pfs {
            if matches!(p.kind, "city" | "town" | "village" | "hamlet") {
                let idx = cities.names.len() as u32;
                cities.names.push(name_of(&p.names));
                cities.pops.push(p.pop);
                pts.push(CityPt { xy: [p.lon * COS_LAT(), p.lat], idx });
            }
        }
        cities.tree = RTree::bulk_load(pts);
    }
    let city_name = |c: Option<usize>| -> String { c.map(|i| cities.names[i].clone()).unwrap_or_default() };
    let city_pop = |c: Option<usize>| -> i64 { c.map(|i| cities.pops[i]).unwrap_or(0) };

    // ---- streets
    struct Acc {
        names: Names,
        city: Option<usize>,
        ways: Vec<Vec<(i32, i32)>>,
        sx: f64,
        sy: f64,
        n: u64,
        bbox: [i32; 4], // minlon minlat maxlon maxlat
    }
    let mut accs: FxHashMap<(String, Option<usize>), Acc> = FxHashMap::default();
    for sr in &st.streets {
        let pts = resolve(sr.refs, &st.arena, &needed, &coords);
        if pts.len() < 2 {
            continue;
        }
        let key = canon(sr.names.primary());
        if key.is_empty() {
            continue;
        }
        let mid = pts[pts.len() / 2];
        let city = cities.nearest(mid.0 as f64 / E7, mid.1 as f64 / E7);
        let simp = simplify(&pts);
        let acc = accs.entry((key, city)).or_insert_with(|| Acc {
            names: sr.names.clone(),
            city,
            ways: vec![],
            sx: 0.0,
            sy: 0.0,
            n: 0,
            bbox: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
        });
        for q in &simp {
            acc.sx += q.0 as f64;
            acc.sy += q.1 as f64;
            acc.n += 1;
            acc.bbox[0] = acc.bbox[0].min(q.0);
            acc.bbox[1] = acc.bbox[1].min(q.1);
            acc.bbox[2] = acc.bbox[2].max(q.0);
            acc.bbox[3] = acc.bbox[3].max(q.1);
        }
        if acc.names.en.is_empty() {
            acc.names.en = sr.names.en.clone();
        }
        acc.ways.push(simp);
    }
    let mut street_list: Vec<Acc> = accs.into_values().collect();
    street_list.sort_by(|a, b| {
        city_pop(b.city)
            .cmp(&city_pop(a.city))
            .then_with(|| a.names.primary().cmp(b.names.primary()))
    });
    if !p(60, "כתיבת מסד הנתונים") {
        bail!("cancelled");
    }

    // ---- database
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    for ext in ["", "-journal", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", out.display(), ext));
    }
    let mut conn = Connection::open(out).with_context(|| format!("create {}", out.display()))?;
    let _: String = conn.query_row("PRAGMA journal_mode=OFF", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA synchronous=OFF; PRAGMA cache_size=-300000; PRAGMA temp_store=MEMORY;")?;
    conn.execute_batch(SCHEMA)?;
    let tx = conn.transaction()?;

    // places
    let mut n_places = 0u64;
    {
        let mut ins = tx.prepare("INSERT INTO places(id,name_he,name_en,name_ar,kind,lon,lat,population,city) VALUES (?,?,?,?,?,?,?,?,?)")?;
        let mut fts = tx.prepare("INSERT INTO places_fts(rowid, search) VALUES (?,?)")?;
        for (i, pl) in pfs.iter().enumerate() {
            let id = i as i64 + 1;
            let city = if matches!(pl.kind, "city" | "town" | "village" | "hamlet") {
                String::new()
            } else {
                city_name(cities.nearest(pl.lon, pl.lat))
            };
            ins.execute(params![id, pl.names.he, pl.names.en, pl.names.ar, pl.kind, pl.lon, pl.lat, pl.pop, city])?;
            let [a, b, c] = pl.names.all();
            fts.execute(params![id, index_tokens(&[a, b, c, &city])])?;
            n_places += 1;
        }
    }

    // streets
    let mut n_streets = 0u64;
    {
        let mut ins = tx.prepare("INSERT INTO streets(id,name,city,ckey,city_key,lon,lat,minlon,minlat,maxlon,maxlat,geom) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")?;
        let mut fts = tx.prepare("INSERT INTO streets_fts(rowid, search) VALUES (?,?)")?;
        let mut rt = tx.prepare("INSERT INTO streets_rt(id,minlon,maxlon,minlat,maxlat) VALUES (?,?,?,?,?)")?;
        for (i, a) in street_list.iter().enumerate() {
            let id = i as i64 + 1;
            let (mx, my) = (a.sx / a.n as f64, a.sy / a.n as f64);
            // representative point = simplified vertex nearest to the mean (lies on the road)
            let mut best = (f64::MAX, (0i32, 0i32));
            for w in &a.ways {
                for q in w {
                    let d = (q.0 as f64 - mx).powi(2) + (q.1 as f64 - my).powi(2);
                    if d < best.0 {
                        best = (d, *q);
                    }
                }
            }
            let (lon, lat) = (best.1 .0 as f64 / E7, best.1 .1 as f64 / E7);
            let city = city_name(a.city);
            let name = name_of(&a.names);
            let [h, e, r] = a.names.all();
            let (minlon, minlat, maxlon, maxlat) =
                (a.bbox[0] as f64 / E7, a.bbox[1] as f64 / E7, a.bbox[2] as f64 / E7, a.bbox[3] as f64 / E7);
            ins.execute(params![
                id,
                name,
                city,
                canon(&name),
                canon(&city),
                lon,
                lat,
                minlon,
                minlat,
                maxlon,
                maxlat,
                encode_geom(&a.ways)
            ])?;
            fts.execute(params![id, index_tokens(&[h, e, r, &city])])?;
            rt.execute(params![id, minlon, maxlon, minlat, maxlat])?;
            n_streets += 1;
        }
    }
    if !p(70, "שמירת כתובות") {
        bail!("cancelled");
    }

    // addresses
    let mut n_addrs = 0u64;
    {
        let mut ins = tx.prepare("INSERT INTO addresses(street,housenumber,housenumber_num,city,lon,lat,street_key,hn_key) VALUES (?,?,?,?,?,?,?,?)")?;
        let mut seen: FxHashSet<u64> = FxHashSet::default();
        let mut canon_cache: FxHashMap<u32, String> = FxHashMap::default();
        for ar in &st.addrs {
            let pos = match ar.pos {
                Some(p) => Some(p),
                None => centroid(&resolve(ar.refs, &st.arena, &needed, &coords)),
            };
            let Some((lon, lat)) = pos else { continue };
            let street = &st.strs.vec[ar.street as usize];
            let skey = canon_cache.entry(ar.street).or_insert_with(|| canon(street)).clone();
            if skey.is_empty() {
                continue;
            }
            let city = match ar.city {
                Some(c) => st.strs.vec[c as usize].clone(),
                None => city_name(cities.nearest(lon, lat)),
            };
            let (hk, hnum) = housenumber_key(&ar.hn);
            if hk.is_empty() {
                continue;
            }
            let h = hash_key(&(&skey, &hk, canon(&city), (lon * 2000.0) as i64, (lat * 2000.0) as i64));
            if !seen.insert(h) {
                continue;
            }
            ins.execute(params![street, &*ar.hn, hnum, city, lon, lat, skey, hk])?;
            n_addrs += 1;
        }
    }
    if !p(80, "שמירת נקודות עניין") {
        bail!("cancelled");
    }

    // pois
    let mut n_pois = 0u64;
    {
        struct PoiFinal {
            names: Names,
            cat: u32,
            lon: f64,
            lat: f64,
            city: Option<usize>,
        }
        let mut list: Vec<PoiFinal> = Vec::new();
        let mut seen: FxHashSet<u64> = FxHashSet::default();
        for pr in &st.pois {
            let pos = match pr.pos {
                Some(p) => Some(p),
                None => centroid(&resolve(pr.refs, &st.arena, &needed, &coords)),
            };
            let Some((lon, lat)) = pos else { continue };
            let h = hash_key(&(canon(pr.names.primary()), pr.cat, (lon * 1000.0) as i64, (lat * 1000.0) as i64));
            if !seen.insert(h) {
                continue;
            }
            list.push(PoiFinal { names: pr.names.clone(), cat: pr.cat, lon, lat, city: cities.nearest(lon, lat) });
        }
        // open-data points: skip those OSM already has (same category, neighbouring ~100 m cell)
        let cell = |lon: f64, lat: f64| ((lon * 1000.0).floor() as i64, (lat * 1000.0).floor() as i64);
        let mut osm_cells: FxHashSet<(u32, i64, i64)> = FxHashSet::default();
        for f in &list {
            let (x, y) = cell(f.lon, f.lat);
            osm_cells.insert((f.cat, x, y));
        }
        let mut n_extra = 0usize;
        for e in extra {
            let cat = st.strs.id(e.cat);
            let (x, y) = cell(e.lon, e.lat);
            if (-1..=1).any(|dx| (-1..=1).any(|dy| osm_cells.contains(&(cat, x + dx, y + dy)))) {
                continue;
            }
            osm_cells.insert((cat, x, y));
            let names = Names { he: e.name.clone(), ..Default::default() };
            list.push(PoiFinal { names, cat, lon: e.lon, lat: e.lat, city: cities.nearest(e.lon, e.lat) });
            n_extra += 1;
        }
        if !extra.is_empty() {
            log::info!("geocode: {n_extra} of {} open-data points added (the rest are in OSM)", extra.len());
        }
        list.sort_by_key(|p| std::cmp::Reverse(city_pop(p.city)));
        let mut ins = tx.prepare("INSERT INTO pois(id,name,category,lon,lat,city) VALUES (?,?,?,?,?,?)")?;
        let mut fts = tx.prepare("INSERT INTO pois_fts(rowid, search) VALUES (?,?)")?;
        let mut rt = tx.prepare("INSERT INTO pois_rt(id,minlon,maxlon,minlat,maxlat) VALUES (?,?,?,?,?)")?;
        for (i, f) in list.iter().enumerate() {
            let id = i as i64 + 1;
            let city = city_name(f.city);
            let [h, e, r] = f.names.all();
            let cat = &st.strs.vec[f.cat as usize];
            ins.execute(params![id, name_of(&f.names), cat, f.lon, f.lat, city])?;
            // the Hebrew category label is searchable too ("בית כנסת הגדול" finds synagogues)
            let label = super::categories::label(cat);
            fts.execute(params![id, index_tokens(&[h, e, r, &city, &label])])?;
            rt.execute(params![id, f.lon, f.lon, f.lat, f.lat])?;
            n_pois += 1;
        }
    }
    if !p(88, "יצירת אינדקסים לחיפוש") {
        bail!("cancelled");
    }

    tx.execute(
        "INSERT INTO meta(key,value) VALUES ('version',?1),('built_iso',?2),('places',?3),('streets',?4),('addresses',?5),('pois',?6)",
        params![
            SCHEMA_VERSION,
            chrono::Utc::now().to_rfc3339(),
            n_places.to_string(),
            n_streets.to_string(),
            n_addrs.to_string(),
            n_pois.to_string()
        ],
    )?;
    tx.commit()?;
    conn.execute_batch(
        "CREATE INDEX idx_addr_street ON addresses(street_key, hn_key);
         CREATE INDEX idx_addr_ll ON addresses(lat, lon);
         CREATE INDEX idx_streets_ckey ON streets(ckey);
         INSERT INTO places_fts(places_fts) VALUES('optimize');
         INSERT INTO streets_fts(streets_fts) VALUES('optimize');
         INSERT INTO pois_fts(pois_fts) VALUES('optimize');",
    )?;
    if !p(94, "דחיסת מסד הנתונים") {
        bail!("cancelled");
    }
    conn.execute_batch("VACUUM;")?;
    drop(conn);
    log::info!(
        "geocode built in {:?}: places {n_places} streets {n_streets} addrs {n_addrs} pois {n_pois}",
        t0.elapsed()
    );
    progress(ProgressPhase::Finalizing, 100, 100, "אינדקס הכתובות מוכן");
    Ok(())
}

#[cfg(test)]
mod real_data {
    /// Build the address index from the installed OSM extract into a temp file (no network, no
    /// data.gov.il points) and query it: `cargo test --release --features cli --lib
    /// build_real_geocoder -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn build_real_geocoder() {
        let root = crate::paths::DataPaths::default_root();
        let pbf = root.join("downloads").join("israel-and-palestine-latest.osm.pbf");
        let out = std::env::temp_dir().join("mo-geocode-test.sqlite");
        super::build_at(&pbf, &out, 31.5, &[], &|_, _, _, _| true).unwrap();
        let g = crate::geocode::open(&out).unwrap();
        for q in ["כביש 6", "כביש 443", "דרך 1", "כביש 90", "הרצל 12 תל אביב"] {
            let r = g.search(q, Some([34.78, 32.08]), 5).unwrap();
            println!("== {q}");
            for x in &r {
                println!("  {:?} {} {:?} ({:.5},{:.5})", x.kind, x.name, x.city, x.lon, x.lat);
            }
        }
    }
}
