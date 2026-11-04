//! Offline geocoder: SQLite FTS5 index built from OSM PBF.
//!
//! * `build` creates the index (places / streets / addresses / pois).
//! * `open` opens it read-only; `search` and `reverse` answer queries.
mod builder;
pub mod datagov;
#[cfg(test)]
mod golden;
pub use builder::{BUILD_REV, SCHEMA_VERSION};
pub mod categories;
pub mod hours;
pub mod info;
pub mod norm;
mod thin;
pub mod wiki;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use rustc_hash::FxHashMap;

use crate::geo::haversine_m;
use crate::types::{GeoExtra, GeoKind, GeoPrecision, GeoResult, LonLat, ProgressFn, WikiSummary};
use info::PoiInfo;
use norm::{
    address_candidates, allowed_edits2, canon, canon_tokens, expand_abbreviations, from_latin_keyboard, is_type_word, levenshtein2, norm, prefix_variants, spell_fold,
    strip_he, tokens, tri_key, trigram_query, AddrCand,
};

/// Israel: OSM plus the open-data points of data.gov.il (fetched now; skipped when offline).
pub fn build(pbf: &Path, out: &Path, progress: ProgressFn) -> Result<()> {
    let report = |m: &str| {
        progress(crate::types::ProgressPhase::Processing, 0, 100, m);
    };
    let extra = datagov::fetch_all(&report);
    let syn = datagov::fetch_street_synonyms(&report);
    builder::build_at(pbf, out, 31.5, &extra, &syn, true, progress)
}

/// While true the app opens no connection to the Israel index (it is being replaced); bumping
/// [`EPOCH`] makes pooled connections be dropped instead of reused.
pub static REPLACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Closes the app's pooled connections to the Israel index right away (set by the app; the
/// data CLI has none). Pooled connections keep the file open, and Windows refuses to replace
/// an open file.
static RELEASE_HOOK: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

pub fn set_release_hook(f: impl Fn() + Send + Sync + 'static) {
    let _ = RELEASE_HOOK.set(Box::new(f));
}

/// Replace the installed index with a freshly built one while the app may be searching it:
/// stop new connections, let running searches finish, drop pooled ones, then swap the file.
pub fn replace_index(dest: &Path, tmp: &Path) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    struct Unblock;
    impl Drop for Unblock {
        fn drop(&mut self) {
            REPLACING.store(false, Ordering::SeqCst);
        }
    }
    REPLACING.store(true, Ordering::SeqCst);
    let _unblock = Unblock;
    EPOCH.fetch_add(1, Ordering::SeqCst);
    if let Some(release) = RELEASE_HOOK.get() {
        release();
    }
    // searches already running finish within milliseconds and drop their connection
    std::thread::sleep(std::time::Duration::from_millis(600));
    crate::paths::atomic_replace(dest, tmp)
}

/// Index of another country (`ref_lat` = middle latitude of its bbox).
pub fn build_region(pbf: &Path, out: &Path, ref_lat: f64, progress: ProgressFn) -> Result<()> {
    builder::build_at(pbf, out, ref_lat, &[], &[], false, progress)
}

struct PlaceMem {
    name: String,
    kind: String,
    lon: f64,
    lat: f64,
    pop: i64,
    city: String,
    canon: String,
    id: i64,
    /// landmarks: wikipedia link (builder revision 4)
    info: Option<Box<PoiInfo>>,
}

/// Settlement / neighbourhood table kept in memory (≈4k rows). Built lazily on first use so
/// that `open` only opens the SQLite file; shared by all connections of one index.
#[derive(Default)]
struct Places {
    list: Vec<PlaceMem>,
    /// settlement name -> population (for ranking streets/POIs by their city).
    city_pop: FxHashMap<String, i64>,
    /// settlements by English name: (canonical English name, canonical Hebrew name, population),
    /// so an address typed in English ("Herzl 12 Tel Aviv") finds the Hebrew town of the index.
    en_names: Vec<(String, String, i64)>,
}

/// One street of the index, kept in memory for typo matching (the trigram index finds no
/// candidate for a short name with a typo in the middle; then these are scanned).
struct StreetKey {
    id: i64,
    name: String,
    city: String,
    ckey: String,
    /// spelling-folded `ckey` and its length in characters
    fold: String,
    n: usize,
    lon: f64,
    lat: f64,
}

#[derive(Default)]
struct CacheInner {
    places: OnceLock<Places>,
    streets: OnceLock<Vec<StreetKey>>,
}

/// Lazily-filled caches (places, street keys) shared between connections to the same index file.
#[derive(Clone, Default)]
pub struct SharedCache(Arc<CacheInner>);

/// What an index file offers beyond the base schema (older builds lack these).
#[derive(Clone, Copy, Default, Debug)]
struct Caps {
    /// pois.hours / pois.flags / pois.minzoom
    poi_cols: bool,
    /// street_syn (official street synonyms)
    syn: bool,
    /// streets_tri (trigram index of street keys)
    tri: bool,
    /// pois.info / places.info (details: phone, website, brand, wikipedia …), revision 4
    poi_info: bool,
    place_info: bool,
    /// addresses.src (interpolation-way addresses), revision 4
    addr_src: bool,
    /// wiki / wiki_qid (offline Wikipedia summaries), revision 4
    wiki: bool,
}

fn caps_of(conn: &Connection) -> Caps {
    let table = |t: &str| conn.query_row("SELECT 1 FROM sqlite_master WHERE name = ?1", [t], |_| Ok(())).is_ok();
    let col = |sql: &str| conn.prepare(sql).is_ok();
    Caps {
        poi_cols: col("SELECT hours, flags, minzoom FROM pois LIMIT 0"),
        syn: table("street_syn"),
        tri: table("streets_tri"),
        poi_info: col("SELECT info FROM pois LIMIT 0"),
        place_info: col("SELECT info FROM places LIMIT 0"),
        addr_src: col("SELECT src FROM addresses LIMIT 0"),
        wiki: table("wiki") && table("wiki_qid"),
    }
}

pub struct Geocoder {
    conn: Connection,
    cache: SharedCache,
    caps: Caps,
}

pub fn open(path: &Path) -> Result<Geocoder> {
    open_with_cache(path, SharedCache::default())
}

/// Open another connection to the index, sharing an existing place cache (cheap: no table scan).
pub fn open_with_cache(path: &Path, cache: SharedCache) -> Result<Geocoder> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("open geocoder index {}", path.display()))?;
    conn.execute_batch("PRAGMA query_only=1; PRAGMA cache_size=-16384; PRAGMA mmap_size=268435456; PRAGMA temp_store=MEMORY;")?;
    conn.set_prepared_statement_cache_capacity(32);
    let ver: String = conn
        .query_row("SELECT value FROM meta WHERE key='version'", [], |r| r.get(0))
        .context("geocoder index is invalid")?;
    if ver != builder::SCHEMA_VERSION {
        anyhow::bail!("geocoder index version {ver} unsupported");
    }
    let caps = caps_of(&conn);
    Ok(Geocoder { conn, cache, caps })
}

fn load_places(conn: &Connection, caps: Caps) -> Result<Places> {
    let mut places = Vec::new();
    let mut city_pop: FxHashMap<String, i64> = FxHashMap::default();
    let mut en_names: Vec<(String, String, i64)> = Vec::new();
    let mut st = conn.prepare(&format!(
        "SELECT id,name_he,name_en,kind,lon,lat,population,city,{} FROM places",
        if caps.place_info { "info" } else { "NULL" }
    ))?;
    let rows = st.query_map([], |r| {
        let name: String = r.get(1)?;
        let en: String = r.get(2)?;
        let info: Option<String> = r.get(8)?;
        Ok((
            PlaceMem {
                id: r.get(0)?,
                canon: canon(if name.is_empty() { &en } else { &name }),
                name: if name.is_empty() { en.clone() } else { name },
                kind: r.get(3)?,
                lon: r.get(4)?,
                lat: r.get(5)?,
                pop: r.get(6)?,
                city: r.get(7)?,
                info: PoiInfo::from_json(info.as_deref()).map(Box::new),
            },
            en,
        ))
    })?;
    for p in rows {
        let (p, en) = p?;
        if matches!(p.kind.as_str(), "city" | "town" | "village" | "hamlet") {
            let e = city_pop.entry(p.name.clone()).or_insert(0);
            *e = (*e).max(p.pop);
            let ek = canon(&en);
            if !ek.is_empty() && ek != p.canon {
                en_names.push((ek, p.canon.clone(), p.pop));
            }
        }
        places.push(p);
    }
    Ok(Places { list: places, city_pop, en_names })
}

impl Geocoder {
    fn places(&self) -> &Places {
        self.cache.0.places.get_or_init(|| {
            load_places(&self.conn, self.caps).unwrap_or_else(|e| {
                log::warn!("geocoder: loading places failed: {e:#}");
                Places::default()
            })
        })
    }

    /// Every street key, spelling-folded once per index (typo fallback, see [`StreetKey`]).
    fn street_keys(&self) -> &[StreetKey] {
        self.cache.0.streets.get_or_init(|| {
            let load = || -> Result<Vec<StreetKey>> {
                let mut st = self.conn.prepare("SELECT id, name, city, ckey, lon, lat FROM streets")?;
                let rows = st.query_map([], |r| {
                    let ckey: String = r.get(3)?;
                    let fold = spell_fold(&ckey);
                    Ok(StreetKey { id: r.get(0)?, name: r.get(1)?, city: r.get(2)?, n: fold.chars().count(), fold, ckey, lon: r.get(4)?, lat: r.get(5)? })
                })?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            };
            load().unwrap_or_else(|e| {
                log::warn!("geocoder: loading street keys failed: {e:#}");
                Vec::new()
            })
        })
    }

    /// Street rows (id, name, city, ckey, lon, lat) sharing a trigram with `key` (spelling- and
    /// confusable-folded), at most 5000; empty when the index has no trigram table.
    fn trigram_streets(&self, key: &str) -> Result<Vec<(i64, String, String, String, f64, f64)>> {
        if !self.caps.tri {
            return Ok(vec![]);
        }
        let Some(m) = trigram_query(&tri_key(key)) else { return Ok(vec![]) };
        let mut st = self.conn.prepare_cached(
            "SELECT s.id, s.name, s.city, s.ckey, s.lon, s.lat FROM streets_tri JOIN streets s ON s.id = streets_tri.rowid
             WHERE streets_tri MATCH ?1 LIMIT 5000",
        )?;
        let rows = st.query_map(params![m], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The place cache, to share with further connections (see [`open_with_cache`]).
    pub fn shared_cache(&self) -> SharedCache {
        self.cache.clone()
    }

    /// Fill the place cache and fault in the hot index pages (call in the background at startup).
    pub fn warm(&self) {
        let _ = self.places();
        for t in ["places_fts_idx", "streets_fts_idx", "pois_fts_idx"] {
            let _ = self.conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get::<_, i64>(0));
        }
    }
}

// ------------------------------------------------------------------ helpers

fn category_he(cat: &str) -> String {
    categories::label(cat)
}

fn with_city(name: &str, city: &str) -> String {
    if city.is_empty() || city == name {
        name.to_string()
    } else {
        format!("{name}, {city}")
    }
}

/// Ranking cost of the distance from `near`: most of it within the first tens of km
/// (8 · (1 − e^(−d/15 km))), plus a gentle 1 per 100 km beyond, so nearby hits win among
/// equals while the large gaps between match kinds (exact 200 / interpolated 150 / street 100)
/// still decide first.
fn near_penalty(near: Option<LonLat>, lon: f64, lat: f64) -> f64 {
    match near {
        Some(n) => {
            let km = haversine_m(n, [lon, lat]) / 1000.0;
            8.0 * (1.0 - (-km / 15.0).exp()) + km / 100.0
        }
        None => 0.0,
    }
}

/// Mark how the query was read on the first result (UI: "מציג תוצאות עבור: X").
fn set_interpretation(v: &mut [GeoResult], text: &str) {
    if let Some(g) = v.first_mut() {
        let t = text.trim();
        if !t.is_empty() {
            g.extra.get_or_insert_with(Default::default).interpretation = Some(t.to_string());
        }
    }
}

/// Only the first result keeps an interpretation (others may carry one from a fuzzy path).
fn keep_first_interpretation(v: &mut [GeoResult]) {
    for g in v.iter_mut().skip(1) {
        if let Some(e) = g.extra.as_mut() {
            e.interpretation = None;
            if e.is_empty() {
                g.extra = None;
            }
        }
    }
}

/// Opening hours / kosher flag / stored details of a point of interest, evaluated now.
fn poi_extra(hours: Option<&str>, kosher: bool, info: Option<&str>, now: chrono::NaiveDateTime) -> Option<Box<GeoExtra>> {
    let mut e = GeoExtra::default();
    if let Some(i) = PoiInfo::from_json(info) {
        i.apply(&mut e);
    }
    if let Some(h) = hours.filter(|h| !h.trim().is_empty()) {
        let v = hours::eval(h, now);
        e.opening_hours = Some(h.to_string());
        e.open_now = v.open_now;
        e.closes_at = v.closes_at;
        e.opens_at = v.opens_at;
    }
    if kosher {
        e.kosher = Some(true);
    }
    (!e.is_empty()).then(|| Box::new(e))
}

/// A typed coordinate (any of the formats of [`crate::coords`]) as a search result.
pub fn coord_result(c: &crate::coords::ParsedCoord) -> GeoResult {
    let decimal = format!("{:.5}, {:.5}", c.lat, c.lon);
    let mut extra = None;
    let name = if c.format == crate::coords::CoordFormat::Dd {
        decimal
    } else {
        // "ITM 178500 663500", shown with the decimal reading
        extra = Some(Box::new(GeoExtra { interpretation: Some(decimal), ..Default::default() }));
        c.label.clone()
    };
    GeoResult {
        id: format!("coord:{:.6},{:.6}", c.lon, c.lat),
        name,
        kind: GeoKind::Coord,
        precision: GeoPrecision::Exact,
        lon: c.lon,
        lat: c.lat,
        city: None,
        street: None,
        house_number: None,
        category: None,
        score: 1000.0,
        extra,
    }
}

fn parse_coord(q: &str, near: Option<LonLat>) -> Option<GeoResult> {
    let israel = |lon: f64, lat: f64| {
        let b = crate::paths::BBOX_ISRAEL;
        lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3]
    };
    crate::coords::parse(q, near, &israel).map(|c| coord_result(&c))
}

fn city_matches(db_city: &str, q: &str, cache: &mut FxHashMap<String, bool>) -> bool {
    if let Some(v) = cache.get(db_city) {
        return *v;
    }
    let d = canon(db_city);
    let ok = d == q || d.starts_with(q) || q.starts_with(&format!("{d} "));
    cache.insert(db_city.to_string(), ok);
    ok
}

fn pop_boost_city(pop: i64) -> f64 {
    (pop.max(1) as f64).log10()
}

struct Cand {
    kind: GeoKind,
    id: String,
    display: String,
    names: [String; 3],
    city: String,
    category: Option<String>,
    street: Option<String>,
    lon: f64,
    lat: f64,
    /// Own population (places) or population of the containing city.
    pop: i64,
    /// Points of interest: OSM opening_hours and the kosher flag.
    hours: Option<String>,
    kosher: bool,
    /// stored details (JSON, `PoiInfo`) of a POI or landmark
    info: Option<String>,
}

fn name_quality(nt: &[String], city_t: &[String], qt: &[String]) -> f64 {
    let nj = nt.join(" ");
    let qj = qt.join(" ");
    if nj == qj {
        return 100.0;
    }
    if nj.starts_with(&qj) {
        return 75.0 - ((nj.chars().count() - qj.chars().count()) as f64 * 0.1).min(10.0);
    }
    let pref = |pool: &[&String]| qt.iter().all(|q| pool.iter().any(|t| t.starts_with(q.as_str())));
    let name_pool: Vec<&String> = nt.iter().collect();
    if pref(&name_pool) {
        return 60.0;
    }
    let all_pool: Vec<&String> = nt.iter().chain(city_t.iter()).collect();
    if pref(&all_pool) {
        return 45.0;
    }
    20.0
}

impl Geocoder {
    pub fn search(&self, query: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let limit = limit.clamp(1, 50);
        if let Some(c) = parse_coord(q, near) {
            return Ok(vec![c]);
        }
        // a phone number ("03-6123456", "+972 3 612 3456"): the places that have it
        if q.chars().all(|c| c.is_ascii_digit() || matches!(c, ' ' | '-' | '+' | '(' | ')')) {
            if let Some(k) = info::phone_key(q).filter(|k| k.len() >= 9) {
                let v = self.search_phone(&k, near, limit)?;
                if !v.is_empty() {
                    return Ok(v);
                }
            }
        }
        if let Some(n) = road_number(q) {
            let roads = self.search_road(&n, near, limit)?;
            if !roads.is_empty() {
                return Ok(roads);
            }
        }
        let r = dedupe_addresses(self.search_rewritten(q, near, limit)?);
        // Keyboard left in English ("shzbdu; 50" -> "דיזנגוף 50"): try the Hebrew reading too and
        // keep whichever answers better (a city-only hit for the Hebrew part must not win). Not
        // when the Latin reading already found the exact address ("Dizengoff 50").
        let exact = r.first().is_some_and(|g| g.precision == GeoPrecision::Exact && matches!(g.kind, GeoKind::Address | GeoKind::Poi));
        if let Some(h) = from_latin_keyboard(q).filter(|_| !exact) {
            let mut r2 = dedupe_addresses(self.search_rewritten(&h, near, limit)?);
            let best = |v: &[GeoResult]| v.first().map_or(f64::MIN, |g| g.score);
            if best(&r2) > best(&r) {
                // the Hebrew reading wins: say what was searched (unless a finer reading exists)
                if r2.first().and_then(|g| g.extra.as_ref()).and_then(|e| e.interpretation.as_ref()).is_none() {
                    set_interpretation(&mut r2, &h);
                }
                return Ok(r2);
            }
        }
        Ok(r)
    }

    /// POIs whose phone number is `key` (national digits, indexed by builder revision 4).
    fn search_phone(&self, key: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        if !self.caps.poi_info {
            return Ok(vec![]);
        }
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT s.id,s.name,s.category,s.city,s.lon,s.lat,{}
             FROM pois_fts JOIN pois s ON s.id = pois_fts.rowid
             WHERE pois_fts MATCH ?1 ORDER BY pois_fts.rowid LIMIT 50",
            self.poi_cols("s")
        ))?;
        let now = hours::now_il();
        let mut out: Vec<GeoResult> = Vec::new();
        for row in st.query_map(params![format!("\"{key}\"")], poi_row)? {
            let r = row?;
            // the token matched; keep only places whose stored phone really is this number
            let phone_ok = PoiInfo::from_json(r.info.as_deref()).and_then(|i| i.phone).and_then(|p| info::phone_key(&p)).is_some_and(|p| p == key);
            if !phone_ok {
                continue;
            }
            let score = 200.0 - near_penalty(near, r.lon, r.lat);
            out.push(GeoResult {
                id: format!("poi:{}", r.id),
                name: with_city(&if r.name.is_empty() { category_he(&r.cat) } else { r.name.clone() }, &r.city),
                kind: GeoKind::Poi,
                precision: GeoPrecision::Exact,
                lon: r.lon,
                lat: r.lat,
                city: if r.city.is_empty() { None } else { Some(r.city.clone()) },
                street: None,
                house_number: None,
                category: Some(category_he(&r.cat)),
                score,
                extra: poi_extra(r.hours.as_deref(), r.kosher, r.info.as_deref(), now),
            });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(limit);
        Ok(out)
    }

    /// Offline Wikipedia summary of an OSM `wikipedia` tag ("he:Title") or a Wikidata id ("Q123").
    pub fn wiki(&self, tag: &str) -> Result<Option<WikiSummary>> {
        if !self.caps.wiki {
            return Ok(None);
        }
        let t = tag.trim();
        let key = if t.starts_with('Q') && t[1..].chars().all(|c| c.is_ascii_digit()) {
            match self.conn.query_row("SELECT tag FROM wiki_qid WHERE qid = ?1", [t], |r| r.get::<_, String>(0)) {
                Ok(k) => k,
                Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        } else {
            match wiki::key(t) {
                Some(k) => k,
                None => return Ok(None),
            }
        };
        let r = self.conn.query_row("SELECT title, extract, url, lang FROM wiki WHERE tag = ?1", [&key], |r| {
            Ok(WikiSummary { title: r.get(0)?, extract: r.get(1)?, url: r.get(2)?, lang: r.get(3)? })
        });
        match r {
            Ok(w) => Ok(Some(w)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// "כביש 6": the stretches of road N, one per nearby town (the builder indexes numbered roads
    /// as streets named "כביש N"), nearest to `near` first, else in town-size order.
    fn search_road(&self, n: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let name = format!("כביש {n}");
        let mut st = self.conn.prepare_cached("SELECT id,name,city,lon,lat FROM streets WHERE ckey = ?1 ORDER BY id LIMIT 200")?;
        let rows = st.query_map(params![canon(&name)], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?))
        })?;
        let mut out: Vec<(f64, GeoResult)> = Vec::new();
        for (i, row) in rows.enumerate() {
            let (id, sname, city, lon, lat) = row?;
            let d = near.map_or(i as f64, |p| haversine_m(p, [lon, lat]));
            out.push((
                d,
                GeoResult {
                    id: format!("street:{id}"),
                    name: sname.clone(),
                    kind: GeoKind::Street,
                    precision: GeoPrecision::Street,
                    lon,
                    lat,
                    city: if city.is_empty() { None } else { Some(city) },
                    street: Some(sname),
                    house_number: None,
                    category: None,
                    score: 100.0,
                    extra: None,
                },
            ));
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out.into_iter().take(limit).enumerate().map(|(i, (_, mut g))| {
            g.score = 100.0 - i as f64;
            g
        }).collect())
    }

    fn search_rewritten(&self, q: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let expanded = expand_abbreviations(q);
        let rewritten = expanded.split(' ').count() != norm(q).split(' ').count();
        let q = if rewritten { expanded.as_str() } else { q };
        let cands = address_candidates(q);
        let mut r = Vec::new();
        if !cands.is_empty() {
            r = self.search_address(&cands, near, limit)?;
        }
        if r.is_empty() {
            r = self.search_text(q, near, limit)?;
        }
        // "ת\"א" was read as "תל אביב"
        if rewritten && r.first().and_then(|g| g.extra.as_ref()).and_then(|e| e.interpretation.as_ref()).is_none() {
            set_interpretation(&mut r, q);
        }
        Ok(r)
    }

    // -------------------------------------------------------------- addresses

    fn city_boost(&self, city: &str) -> f64 {
        pop_boost_city(self.places().city_pop.get(city).copied().unwrap_or(0)) * 0.5
    }

    fn addr_result(&self, id: String, street: &str, hn: &str, city: &str, lon: f64, lat: f64, prec: GeoPrecision, base: f64, near: Option<LonLat>) -> GeoResult {
        GeoResult {
            id,
            name: with_city(&format!("{street} {hn}"), city),
            kind: GeoKind::Address,
            precision: prec,
            lon,
            lat,
            city: if city.is_empty() { None } else { Some(city.to_string()) },
            street: Some(street.to_string()),
            house_number: Some(hn.to_string()),
            category: None,
            score: base + self.city_boost(city) - near_penalty(near, lon, lat),
            extra: None,
        }
    }

    /// Address candidates whose city was typed in English get the Hebrew name of that town too
    /// ("tel aviv" → "תל אביב יפו"), largest town first when several start with the words.
    fn hebrew_city_cands(&self, cands: &[AddrCand]) -> Vec<AddrCand> {
        let en = &self.places().en_names;
        let mut out: Vec<AddrCand> = cands.to_vec();
        for c in cands {
            let Some(qc) = c.city.as_deref().filter(|q| q.chars().any(|ch| ch.is_ascii_alphabetic())) else { continue };
            let best = en
                .iter()
                .filter(|(e, _, _)| e == qc || e.starts_with(&format!("{qc} ")))
                .max_by_key(|(e, _, pop)| (e == qc, *pop));
            if let Some((_, he, _)) = best {
                out.push(AddrCand { street: c.street.clone(), city: Some(he.clone()), hn: c.hn.clone(), num: c.num });
            }
        }
        out
    }

    fn search_address(&self, cands: &[AddrCand], near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let translated;
        let cands = if self.places().en_names.is_empty() {
            cands
        } else {
            translated = self.hebrew_city_cands(cands);
            &translated[..]
        };
        // Readings that take the trailing words as a city come first; readings without a city
        // (the "city" words were part of the street name) rank below anything found in the
        // typed city, even a street-level hit there.
        let (with_city, no_city): (Vec<AddrCand>, Vec<AddrCand>) = cands.iter().cloned().partition(|c| c.city.is_some());
        let no_city_penalty = if with_city.is_empty() { 0.0 } else { 120.0 };
        let mut res = self.addr_lookup(&with_city, near, 0.0)?;
        if !no_city.is_empty() {
            res.extend(self.addr_lookup(&no_city, near, no_city_penalty)?);
        }
        let has_house = |r: &[GeoResult]| r.iter().any(|x| matches!(x.precision, GeoPrecision::Exact | GeoPrecision::Interpolated));
        // A house found only by a no-city reading (scored ≤ ~80) must not stop the search in the
        // typed city ("רחל 12 ירושלים" found "רחל 12, חיפה" and never tried "רחל אמנו").
        let typed = |r: &[GeoResult]| r.iter().any(|x| matches!(x.precision, GeoPrecision::Exact | GeoPrecision::Interpolated) && (with_city.is_empty() || x.score > 100.0));
        // Each fallback looks in the typed city first; the no-city readings keep their penalty and
        // are tried only when nothing at all was found (addr_lookup stops at the first step that
        // answers, so mixing them would cut the typed city's interpolation short).
        let split_lookup = |extra: Vec<AddrCand>, penalty: f64, res: &mut Vec<GeoResult>| -> Result<()> {
            let (w, n): (Vec<AddrCand>, Vec<AddrCand>) = extra.into_iter().partition(|c| c.city.is_some());
            res.extend(self.addr_lookup(&w, near, penalty)?);
            if !n.is_empty() && !has_house(res) {
                res.extend(self.addr_lookup(&n, near, penalty + no_city_penalty)?);
            }
            Ok(())
        };
        if !typed(&res) {
            // another official name of the same street in that town ("מנחם בגין" = "דרך בגין")
            split_lookup(self.synonym_cands(cands)?, 5.0, &mut res)?;
        }
        if !typed(&res) {
            // The street the user typed may be a part of the mapped name ("ז'ולטי 7" for
            // "הרב בצלאל ז'ולטי 7"), or spelled differently ("מימון" / "מיימון"): resolve candidate
            // street keys through the (spelling-folded) streets index.
            split_lookup(self.expand_street_keys(cands)?, 10.0, &mut res)?;
        }
        if !typed(&res) {
            // typos the token index cannot bridge ("הרצאל 5"): closest street names by edit distance
            let close = self.similar_street_keys(&if with_city.is_empty() { cands.to_vec() } else { with_city.clone() })?;
            if !close.is_empty() {
                // a corrected street name: tell the user what was searched instead
                for mut g in self.addr_lookup(&close, near, 15.0)? {
                    let shown = g.name.clone();
                    g.extra.get_or_insert_with(Default::default).interpretation = Some(shown);
                    res.push(g);
                }
            }
        }

        // 4. city only
        if res.is_empty() {
            for c in cands {
                let Some(qc) = &c.city else { continue };
                // The street was not found at all, so only a clearly typed city may answer: the
                // whole name, or a prefix of at least 3 letters. A 1-2 letter "city" left over from
                // gibberish ("ווטז 12 כ") used to return the biggest city starting with it.
                let prefix_ok = qc.chars().count() >= 3;
                let mut best: Option<&PlaceMem> = None;
                for p in &self.places().list {
                    if (p.canon == *qc || (prefix_ok && p.canon.starts_with(qc.as_str())))
                        && best.is_none_or(|b| p.pop > b.pop) {
                            best = Some(p);
                        }
                }
                if let Some(p) = best {
                    res.push(self.place_result(p, 60.0 - near_penalty(near, p.lon, p.lat)));
                    break;
                }
            }
        }

        res.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        res.truncate(limit);
        keep_first_interpretation(&mut res);
        Ok(res)
    }

    /// Street keys that are another registered name of the candidate's street in its town
    /// (data.gov.il synonyms stored by the builder in `street_syn`).
    fn synonym_cands(&self, cands: &[AddrCand]) -> Result<Vec<AddrCand>> {
        if !self.caps.syn {
            return Ok(vec![]);
        }
        let mut st = self.conn.prepare_cached("SELECT DISTINCT city_key, ckey FROM street_syn WHERE alias = ?1 LIMIT 400")?;
        let mut out: Vec<AddrCand> = Vec::new();
        for c in cands {
            let mut cache = FxHashMap::default();
            let rows = st.query_map(params![c.street], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (city, ckey) = row?;
                if let Some(qc) = &c.city {
                    if !city_matches(&city, qc, &mut cache) {
                        continue;
                    }
                }
                if ckey != c.street && !out.iter().any(|o| o.street == ckey && o.city == c.city && o.hn == c.hn) {
                    out.push(AddrCand { street: ckey, city: c.city.clone(), hn: c.hn.clone(), num: c.num });
                }
            }
        }
        Ok(out)
    }

    /// Streets and places whose name is within a small edit distance of the whole query (used
    /// when the token index finds nothing at all). Results are street-level / place hits.
    fn similar_names(&self, q: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let qf = spell_fold(q);
        let n = qf.chars().count();
        if n < 4 {
            return Ok(vec![]);
        }
        let max = allowed_edits2(n);
        let mut out: Vec<(u32, GeoResult)> = Vec::new();
        for p in &self.places().list {
            let k = spell_fold(&p.canon);
            if k.chars().count().abs_diff(n) > 2 {
                continue;
            }
            let d = levenshtein2(&k, &qf);
            if d <= max {
                out.push((d, self.place_result(p, 70.0 - d as f64 * 5.0 + pop_boost_city(p.pop) - near_penalty(near, p.lon, p.lat))));
            }
        }
        // streets: candidates sharing a trigram first, else every street key (cached in memory)
        let street_hit = |id: i64, name: &str, city: &str, k: &str, lon: f64, lat: f64, out: &mut Vec<(u32, GeoResult)>| {
            if k.chars().count().abs_diff(n) > 2 {
                return;
            }
            let d = levenshtein2(k, &qf);
            if d <= max {
                out.push((d, self.addr_result(format!("street:{id}"), name, "", city, lon, lat, GeoPrecision::Street, 60.0 - d as f64 * 5.0 + self.city_boost(city), near)));
            }
        };
        let before = out.len();
        for (id, name, city, ckey, lon, lat) in self.trigram_streets(&qf)? {
            street_hit(id, &name, &city, &spell_fold(&ckey), lon, lat, &mut out);
        }
        // the in-memory scan of every street key when no trigram candidate was close enough
        // (a typo in a short name shares no trigram with it)
        if out.len() == before {
            for k in self.street_keys() {
                street_hit(k.id, &k.name, &k.city, &k.fold, k.lon, k.lat, &mut out);
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal)));
        out.truncate(limit);
        let mut v: Vec<GeoResult> = out.into_iter().map(|(_, g)| g).collect();
        if let Some(name) = v.first().map(|g| g.name.clone()) {
            set_interpretation(&mut v, &name);
        }
        Ok(v)
    }

    /// Streets whose (spelling-folded) key is within a small edit distance of the typed one,
    /// restricted to the candidate's city when one was given. Insert/delete cost 2, substitution
    /// 2, confusable-letter substitution 1 (`levenshtein2`); one typo up to 7 letters, two beyond
    /// (`allowed_edits2`). Candidates come from the trigram index; when it has none (short name,
    /// typo in the middle) the street keys cached in memory are scanned.
    fn similar_street_keys(&self, cands: &[AddrCand]) -> Result<Vec<AddrCand>> {
        let mut out: Vec<AddrCand> = Vec::new();
        for c in cands {
            let q = spell_fold(&c.street);
            let n = q.chars().count();
            let max = allowed_edits2(n);
            if max == 0 {
                continue;
            }
            let mut cache = FxHashMap::default();
            let mut hits: Vec<(u32, usize, String)> = Vec::new();
            let mut consider = |ckey: &str, city: &str, k: &str, kn: usize, hits: &mut Vec<(u32, usize, String)>| {
                if kn.abs_diff(n) > 2 {
                    return;
                }
                if let Some(qc) = &c.city {
                    if !city_matches(city, qc, &mut cache) {
                        return;
                    }
                }
                let d = levenshtein2(k, &q);
                if d > 0 && d <= max && !hits.iter().any(|h| h.2 == ckey) {
                    hits.push((d, ckey.chars().count(), ckey.to_string()));
                }
            };
            for (_, _, city, ckey, _, _) in self.trigram_streets(&c.street)? {
                let k = spell_fold(&ckey);
                let kn = k.chars().count();
                consider(&ckey, &city, &k, kn, &mut hits);
            }
            if hits.is_empty() {
                for s in self.street_keys() {
                    consider(&s.ckey, &s.city, &s.fold, s.n, &mut hits);
                }
            }
            hits.sort();
            for (_, _, k) in hits.into_iter().take(5) {
                out.push(AddrCand { street: k, city: c.city.clone(), hn: c.hn.clone(), num: c.num });
            }
        }
        Ok(out)
    }

    /// Street keys whose tokens contain every token of the candidate's street, best first
    /// (closest token count, then shortest). Returns new candidates with those keys.
    fn expand_street_keys(&self, cands: &[AddrCand]) -> Result<Vec<AddrCand>> {
        let mut out: Vec<AddrCand> = Vec::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.ckey, s.city FROM streets_fts JOIN streets s ON s.id = streets_fts.rowid
             WHERE streets_fts MATCH ?1 ORDER BY streets_fts.rowid LIMIT 60",
        )?;
        for c in cands {
            let qt: Vec<&str> = c.street.split(' ').filter(|t| !t.is_empty()).collect();
            if qt.is_empty() {
                continue;
            }
            let mut parts: Vec<String> = qt.iter().map(|t| format!("\"{}\"", spell_fold(t).replace('"', ""))).collect();
            if let Some(l) = parts.last_mut() {
                l.push('*');
            }
            // The index also carries the city, so a known city narrows the prefix match to it
            // (a bare "מימונ*" hits dozens of streets and the 60-row cap could miss the right one).
            if let Some(qc) = &c.city {
                parts.extend(qc.split(' ').filter(|t| !t.is_empty()).map(|t| format!("\"{}\"", spell_fold(t).replace('"', ""))));
            }
            let (last, head) = qt.split_last().expect("non-empty");
            // a lone 1-2 letter street word is too short to stand for a longer street name
            // ("ום 12" must not become "חומה ומגדל 12")
            if head.is_empty() && spell_fold(last).chars().count() < 3 {
                continue;
            }
            let fts = parts.join(" ");
            let headf: Vec<String> = head.iter().map(|q| spell_fold(q)).collect();
            let lastf = spell_fold(last);
            let mut cache = FxHashMap::default();
            let rows = stmt.query_map(params![fts], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut keys: Vec<(usize, usize, String)> = Vec::new();
            for row in rows {
                let (ckey, city) = row?;
                if ckey == c.street {
                    continue;
                }
                if let Some(qc) = &c.city {
                    if !city_matches(&city, qc, &mut cache) {
                        continue;
                    }
                }
                let kt: Vec<String> = ckey.split(' ').map(spell_fold).collect();
                // every query token must be a whole token of the key; the last one may be a prefix
                // (both sides spelling-folded, so "מימון" finds "מיימון")
                if !headf.iter().all(|q| kt.contains(q)) || !kt.iter().any(|k| k.starts_with(lastf.as_str())) {
                    continue;
                }
                if keys.iter().any(|k| k.2 == ckey) {
                    continue;
                }
                keys.push((kt.len().abs_diff(qt.len()), ckey.chars().count(), ckey));
            }
            keys.sort();
            for (_, _, k) in keys.into_iter().take(8) {
                out.push(AddrCand { street: k, city: c.city.clone(), hn: c.hn.clone(), num: c.num });
            }
        }
        Ok(out)
    }

    /// Steps 1-3 of the address search (exact number, interpolation, street centroid) for the
    /// given exact street keys. `penalty` lowers the scores of inexact street matches.
    fn addr_lookup(&self, cands: &[AddrCand], near: Option<LonLat>, penalty: f64) -> Result<Vec<GeoResult>> {
        let mut res: Vec<GeoResult> = Vec::new();

        // 1. exact house number
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT id,street,housenumber,city,lon,lat,{} FROM addresses WHERE street_key=?1 AND hn_key=?2",
            if self.caps.addr_src { "src" } else { "0" }
        ))?;
        for c in cands {
            let mut cache = FxHashMap::default();
            let rows = stmt.query_map(params![c.street, c.hn], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                ))
            })?;
            let mut by_city: FxHashMap<String, GeoResult> = FxHashMap::default();
            for row in rows {
                let (id, street, hn, city, lon, lat, src) = row?;
                if let Some(qc) = &c.city {
                    if !city_matches(&city, qc, &mut cache) {
                        continue;
                    }
                }
                // numbers placed along an OSM interpolation way: right street side and order,
                // but not a mapped building
                let (prec, base) = if src == 1 { (GeoPrecision::Interpolated, 160.0) } else { (GeoPrecision::Exact, 200.0) };
                let g = self.addr_result(format!("addr:{id}"), &street, &hn, &city, lon, lat, prec, base - penalty, near);
                match by_city.get(&city) {
                    Some(old) if old.score >= g.score => {}
                    _ => {
                        by_city.insert(city, g);
                    }
                }
            }
            res.extend(by_city.into_values());
            if !res.is_empty() {
                break;
            }
        }

        // 2. interpolation
        if res.is_empty() {
            let mut stmt = self.conn.prepare_cached(
                "SELECT street,housenumber_num,city,lon,lat,id FROM addresses WHERE street_key=?1 AND housenumber_num IS NOT NULL",
            )?;
            for c in cands {
                let mut cache = FxHashMap::default();
                let rows = stmt.query_map(params![c.street], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?, r.get::<_, i64>(5)?))
                })?;
                type Pt = (i64, f64, f64, i64);
                #[derive(Default)]
                struct Group {
                    street: String,
                    lo_same: Option<Pt>,
                    lo_any: Option<Pt>,
                    hi_same: Option<Pt>,
                    hi_any: Option<Pt>,
                }
                let mut groups: FxHashMap<String, Group> = FxHashMap::default();
                for row in rows {
                    let (street, num, city, lon, lat, id) = row?;
                    if num == c.num {
                        continue;
                    }
                    if let Some(qc) = &c.city {
                        if !city_matches(&city, qc, &mut cache) {
                            continue;
                        }
                    }
                    let g = groups.entry(city).or_default();
                    g.street = street;
                    let same = (num - c.num) % 2 == 0;
                    let pt = (num, lon, lat, id);
                    if num < c.num {
                        if g.lo_any.is_none_or(|p| num > p.0) {
                            g.lo_any = Some(pt);
                        }
                        if same && g.lo_same.is_none_or(|p| num > p.0) {
                            g.lo_same = Some(pt);
                        }
                    } else {
                        if g.hi_any.is_none_or(|p| num < p.0) {
                            g.hi_any = Some(pt);
                        }
                        if same && g.hi_same.is_none_or(|p| num < p.0) {
                            g.hi_same = Some(pt);
                        }
                    }
                }
                for (city, g) in groups {
                    let pair = match (g.lo_same, g.hi_same, g.lo_any, g.hi_any) {
                        (Some(l), Some(h), _, _) => Some((l, h)),
                        (_, _, Some(l), Some(h)) => Some((l, h)),
                        _ => None,
                    }
                    .filter(|(lo, hi)| haversine_m([lo.1, lo.2], [hi.1, hi.2]) <= 1500.0);
                    let Some((lo, hi)) = pair else {
                        // The number is past the end of the mapped range (or the street has a
                        // gap): stand at the nearest existing number, same side of the street
                        // when there is one, and say so (UI: "מיקום משוער (ליד מס׳ N)").
                        let closest = |a: Option<Pt>, b: Option<Pt>| match (a, b) {
                            (Some(x), Some(y)) => Some(if (x.0 - c.num).abs() <= (y.0 - c.num).abs() { x } else { y }),
                            (x, y) => x.or(y),
                        };
                        let Some(pt) = closest(g.lo_same, g.hi_same).or_else(|| closest(g.lo_any, g.hi_any)) else { continue };
                        let mut r = self.addr_result(
                            format!("addr:n:{}:{}", pt.3, c.num),
                            &g.street,
                            &c.hn,
                            &city,
                            pt.1,
                            pt.2,
                            GeoPrecision::Interpolated,
                            140.0 - penalty,
                            near,
                        );
                        r.extra = Some(Box::new(GeoExtra { near_house_number: Some(pt.0.to_string()), ..Default::default() }));
                        res.push(r);
                        continue;
                    };
                    let f = (c.num - lo.0) as f64 / (hi.0 - lo.0) as f64;
                    let lon = lo.1 + (hi.1 - lo.1) * f;
                    let lat = lo.2 + (hi.2 - lo.2) * f;
                    res.push(self.addr_result(
                        format!("addr:i:{}:{}:{}", lo.3, hi.3, c.num),
                        &g.street,
                        &c.hn,
                        &city,
                        lon,
                        lat,
                        GeoPrecision::Interpolated,
                        150.0 - penalty,
                        near,
                    ));
                }
                if !res.is_empty() {
                    break;
                }
            }
        }

        // 3. street centroid
        if res.is_empty() {
            let mut stmt = self.conn.prepare_cached("SELECT id,name,city,lon,lat FROM streets WHERE ckey=?1")?;
            for c in cands {
                let mut cache = FxHashMap::default();
                let rows = stmt.query_map(params![c.street], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?))
                })?;
                for row in rows {
                    let (id, name, city, lon, lat) = row?;
                    if let Some(qc) = &c.city {
                        if !city_matches(&city, qc, &mut cache) {
                            continue;
                        }
                    }
                    res.push(GeoResult {
                        id: format!("street:{id}"),
                        name: with_city(&name, &city),
                        kind: GeoKind::Street,
                        precision: GeoPrecision::Street,
                        lon,
                        lat,
                        city: if city.is_empty() { None } else { Some(city.clone()) },
                        street: Some(name),
                        house_number: Some(c.hn.clone()),
                        category: None,
                        score: 100.0 - penalty + self.city_boost(&city) - near_penalty(near, lon, lat),
                        extra: None,
                    });
                }
                if !res.is_empty() {
                    break;
                }
            }
        }

        Ok(res)
    }

    fn place_result(&self, p: &PlaceMem, score: f64) -> GeoResult {
        GeoResult {
            id: format!("place:{}", p.id),
            name: with_city(&p.name, &p.city),
            kind: GeoKind::Place,
            precision: GeoPrecision::City,
            lon: p.lon,
            lat: p.lat,
            city: Some(if p.city.is_empty() { p.name.clone() } else { p.city.clone() }),
            street: None,
            house_number: None,
            category: Some(p.kind.clone()),
            score,
            extra: p.info.as_ref().map(|i| {
                let mut e = GeoExtra::default();
                i.apply(&mut e);
                Box::new(e)
            }),
        }
    }

    // -------------------------------------------------------------- free text

    fn fetch_cands(&self, fts: &str, per_table: [usize; 3]) -> Result<Vec<Cand>> {
        let mut out = Vec::new();
        {
            let mut st = self.conn.prepare_cached(&format!(
                "SELECT p.id,p.name_he,p.name_en,p.name_ar,p.kind,p.lon,p.lat,p.population,p.city,{}
                 FROM places_fts JOIN places p ON p.id = places_fts.rowid
                 WHERE places_fts MATCH ?1 ORDER BY places_fts.rowid LIMIT ?2",
                if self.caps.place_info { "p.info" } else { "NULL" }
            ))?;
            let rows = st.query_map(params![fts, per_table[0] as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            })?;
            for row in rows {
                let (id, he, en, ar, kind, lon, lat, pop, city, info) = row?;
                let name = if he.is_empty() { en.clone() } else { he.clone() };
                out.push(Cand {
                    kind: GeoKind::Place,
                    id: format!("place:{id}"),
                    display: with_city(&name, &city),
                    names: [he, en, ar],
                    city: if city.is_empty() { name } else { city },
                    category: Some(kind),
                    street: None,
                    lon,
                    lat,
                    pop,
                    hours: None,
                    kosher: false,
                    info,
                });
            }
        }
        {
            let mut st = self.conn.prepare_cached(
                "SELECT s.id,s.name,s.city,s.lon,s.lat
                 FROM streets_fts JOIN streets s ON s.id = streets_fts.rowid
                 WHERE streets_fts MATCH ?1 ORDER BY streets_fts.rowid LIMIT ?2",
            )?;
            let rows = st.query_map(params![fts, per_table[1] as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?))
            })?;
            for row in rows {
                let (id, name, city, lon, lat) = row?;
                out.push(Cand {
                    kind: GeoKind::Street,
                    id: format!("street:{id}"),
                    display: with_city(&name, &city),
                    names: [name.clone(), String::new(), String::new()],
                    pop: self.places().city_pop.get(&city).copied().unwrap_or(0),
                    city,
                    category: None,
                    street: Some(name),
                    lon,
                    lat,
                    hours: None,
                    kosher: false,
                    info: None,
                });
            }
        }
        {
            let mut st = self.conn.prepare_cached(&format!(
                "SELECT s.id,s.name,s.category,s.city,s.lon,s.lat,{}
                 FROM pois_fts JOIN pois s ON s.id = pois_fts.rowid
                 WHERE pois_fts MATCH ?1 ORDER BY pois_fts.rowid LIMIT ?2",
                self.poi_cols("s")
            ))?;
            let rows = st.query_map(params![fts, per_table[2] as i64], poi_row)?;
            for row in rows {
                let r = row?;
                out.push(self.poi_cand(r));
            }
        }
        Ok(out)
    }

    fn poi_cand(&self, r: PoiRow) -> Cand {
        let name = if r.name.is_empty() { category_he(&r.cat) } else { r.name.clone() };
        Cand {
            kind: GeoKind::Poi,
            id: format!("poi:{}", r.id),
            display: with_city(&name, &r.city),
            names: [r.name, String::new(), String::new()],
            pop: self.places().city_pop.get(&r.city).copied().unwrap_or(0),
            city: r.city,
            category: Some(category_he(&r.cat)),
            street: None,
            lon: r.lon,
            lat: r.lat,
            hours: r.hours,
            kosher: r.kosher,
            info: r.info,
        }
    }

    /// The extra POI columns of this index for a SELECT ("hours, flags, minzoom, info" or NULLs).
    fn poi_cols(&self, alias: &str) -> String {
        let base = if self.caps.poi_cols { format!("{alias}.hours,{alias}.flags,{alias}.minzoom") } else { "NULL,0,0".to_string() };
        let info = if self.caps.poi_info { format!("{alias}.info") } else { "NULL".to_string() };
        format!("{base},{info}")
    }

    /// POIs of the given categories inside `bbox` = [min_lon, min_lat, max_lon, max_lat] (map
    /// layer). `cats` entries are category ids as in `categories::LEXICON` ("fuel",
    /// "place_of_worship:jewish"); at most `limit` results, nearest to the bbox centre first.
    /// With `zoom`, only POIs whose thinning minzoom is at most that zoom (indexes built before
    /// the thinning return everything, as before).
    pub fn pois_in_bbox(&self, cats: &[&str], bbox: [f64; 4], limit: usize, zoom: Option<f64>) -> Result<Vec<GeoResult>> {
        let thin = self.caps.poi_cols && zoom.is_some();
        // The stored minzoom thins ALL categories together; the layer shows only the selected ones,
        // so a synagogue could lose its place to a café that is not even drawn. Thin the matching
        // POIs among themselves instead (stored minzoom = importance order), over a margin around
        // the view so the markers near its edge do not change as it moves.
        let q = if thin {
            let (w, h) = ((bbox[2] - bbox[0]) * 0.25, (bbox[3] - bbox[1]) * 0.25);
            [bbox[0] - w, bbox[1] - h, bbox[2] + w, bbox[3] + h]
        } else {
            bbox
        };
        // category prefilter in SQL (same rule as categories::matches) so wide views stay fast
        let esc = |c: &str| c.replace('\'', "''");
        let cat_sql = cats
            .iter()
            .map(|c| if c.contains(':') { format!("p.category = '{}'", esc(c)) } else { format!("p.category = '{0}' OR p.category LIKE '{0}:%'", esc(c)) })
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat,{} FROM pois_rt r JOIN pois p ON p.id = r.id
             WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3{}",
            self.poi_cols("p"),
            if cat_sql.is_empty() { String::new() } else { format!(" AND ({cat_sql})") }
        ))?;
        let centre = [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0];
        let mut rows: Vec<PoiRow> = st
            .query_map(params![q[0], q[2], q[1], q[3]], poi_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|r| categories::matches(&r.cat, cats))
            .collect();
        if thin {
            let z = zoom.unwrap_or(99.0);
            rows.sort_by_key(|r| (r.minzoom, r.id));
            let pts: Vec<thin::ThinPt> = rows.iter().map(|r| thin::ThinPt { lon: r.lon, lat: r.lat, importance: -(r.minzoom as f64) }).collect();
            let mz = thin::minzooms(&pts);
            let inside = |r: &PoiRow| r.lon >= bbox[0] && r.lon <= bbox[2] && r.lat >= bbox[1] && r.lat <= bbox[3];
            rows = rows.into_iter().zip(mz).filter(|(r, m)| f64::from(*m) <= z && inside(r)).map(|(mut r, m)| {
                r.minzoom = i64::from(m);
                r
            }).collect();
        }
        let now = hours::now_il();
        let mut found: Vec<(f64, GeoResult)> = Vec::new();
        for r in rows {
            found.push((
                haversine_m(centre, [r.lon, r.lat]),
                GeoResult {
                    id: format!("poi:{}", r.id),
                    name: if r.name.is_empty() { category_he(&r.cat) } else { r.name },
                    kind: GeoKind::Poi,
                    precision: GeoPrecision::Exact,
                    lon: r.lon,
                    lat: r.lat,
                    city: if r.city.is_empty() { None } else { Some(r.city) },
                    street: None,
                    house_number: None,
                    // the more important (lower minzoom) first when sorting on the client
                    score: -(r.minzoom as f64),
                    extra: {
                        let mut x = poi_extra(r.hours.as_deref(), r.kosher, r.info.as_deref(), now).unwrap_or_default();
                        x.category_label = Some(category_he(&r.cat));
                        Some(x)
                    },
                    category: Some(r.cat),
                },
            ));
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(found.into_iter().take(limit).map(|(_, g)| g).collect())
    }

    /// POIs matching `fts` whose category is in `cats`.
    fn fetch_pois_in(&self, fts: &str, cats: &[&str], limit: usize) -> Result<Vec<Cand>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT s.id,s.name,s.category,s.city,s.lon,s.lat,{}
             FROM pois_fts JOIN pois s ON s.id = pois_fts.rowid
             WHERE pois_fts MATCH ?1 ORDER BY pois_fts.rowid LIMIT 800",
            self.poi_cols("s")
        ))?;
        let rows = st.query_map(params![fts], poi_row)?;
        let mut out = Vec::new();
        for row in rows {
            let r = row?;
            if !categories::matches(&r.cat, cats) {
                continue;
            }
            out.push(self.poi_cand(r));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Nearest POIs of the given categories around `near` (category-only queries: "בית כנסת").
    fn nearest_in_category(&self, cats: &[&str], near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        self.nearest_filtered(cats, near, limit, PoiFilter::default())
    }

    /// Nearest POIs of the given categories around `centre` (default: the middle of Israel),
    /// growing the search radius until `limit` are found, with the kosher / open-now filters.
    /// "Open now" keeps the POIs known to be open; when none is, the ones with unknown hours
    /// (never the ones known to be closed).
    fn nearest_filtered(&self, cats: &[&str], centre: Option<LonLat>, limit: usize, f: PoiFilter) -> Result<Vec<GeoResult>> {
        let [lon, lat] = centre.unwrap_or([35.0, 31.6]);
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat,{} FROM pois_rt r JOIN pois p ON p.id = r.id
             WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3",
            self.poi_cols("p")
        ))?;
        let now = hours::now_il();
        let kosher_known = self.caps.poi_cols;
        let mut found: Vec<(f64, GeoResult)> = Vec::new();
        let mut unknown: Vec<(f64, GeoResult)> = Vec::new();
        for radius_m in [1500.0, 5000.0, 20000.0, 80000.0, 400000.0] {
            let dlat = radius_m / 111_000.0;
            let dlon = radius_m / (111_320.0 * lat.to_radians().cos().max(0.2));
            let rows = st.query_map(params![lon - dlon, lon + dlon, lat - dlat, lat + dlat], poi_row)?;
            found.clear();
            unknown.clear();
            for row in rows {
                let r = row?;
                if !categories::matches(&r.cat, cats) || (f.kosher && kosher_known && !r.kosher) {
                    continue;
                }
                let extra = poi_extra(r.hours.as_deref(), r.kosher, r.info.as_deref(), now);
                let open = extra.as_ref().and_then(|e| e.open_now);
                if f.open && open == Some(false) {
                    continue;
                }
                let d = haversine_m([lon, lat], [r.lon, r.lat]);
                let g = GeoResult {
                    id: format!("poi:{}", r.id),
                    name: with_city(&if r.name.is_empty() { category_he(&r.cat) } else { r.name.clone() }, &r.city),
                    kind: GeoKind::Poi,
                    precision: GeoPrecision::Exact,
                    lon: r.lon,
                    lat: r.lat,
                    city: if r.city.is_empty() { None } else { Some(r.city) },
                    street: None,
                    house_number: None,
                    category: Some(category_he(&r.cat)),
                    score: 80.0 - d / 1000.0,
                    extra,
                };
                if f.open && open.is_none() {
                    unknown.push((d, g));
                } else {
                    found.push((d, g));
                }
            }
            if found.len() >= limit {
                break;
            }
        }
        if f.open && found.is_empty() {
            found = unknown;
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(found.into_iter().take(limit).map(|(_, g)| g).collect())
    }

    /// The settlement or neighbourhood named by `toks` (a leading "ב" may be the preposition:
    /// "בתל אביב"); the most populous one when several share the name.
    fn find_place(&self, toks: &[String]) -> Option<&PlaceMem> {
        if toks.is_empty() {
            return None;
        }
        let mut keys = vec![canon(&toks.join(" "))];
        if let Some(first) = toks[0].strip_prefix('ב').filter(|r| r.chars().count() >= 2) {
            let mut t = toks.to_vec();
            t[0] = first.to_string();
            keys.push(canon(&t.join(" ")));
        }
        let folded: Vec<String> = keys.iter().map(|k| k.split(' ').map(spell_fold).collect::<Vec<_>>().join(" ")).collect();
        // exact name first; else a name that extends it by whole words ("תל אביב" → "תל אביב–יפו")
        let mut best: Option<(bool, &PlaceMem)> = None;
        for p in &self.places().list {
            let pf = p.canon.split(' ').map(spell_fold).collect::<Vec<_>>().join(" ");
            let exact = folded.contains(&pf);
            if !exact && !folded.iter().any(|k| pf.starts_with(&format!("{k} "))) {
                continue;
            }
            let better = match best {
                None => true,
                Some((be, b)) => (exact && !be) || (exact == be && p.pop > b.pop),
            };
            if better {
                best = Some((exact, p));
            }
        }
        best.map(|(_, p)| p)
    }

    fn search_text(&self, q: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let all = tokens(q);
        let mut qt: Vec<String> = all.iter().filter(|t| !is_type_word(t)).cloned().collect();
        if qt.is_empty() {
            qt = all.clone();
        }
        if qt.iter().any(|t| !t.chars().all(|c| c.is_ascii_digit())) {
            qt.retain(|t| !t.chars().all(|c| c.is_ascii_digit()));
        }
        if qt.is_empty() {
            return Ok(vec![]);
        }
        // "בית כנסת חניכי הישיבות": a category phrase plus a name. Alone, the phrase means
        // "nearest of that kind"; with a name, the name is searched within the category.
        let cat = categories::detect(&qt);
        let mut rest: Vec<String> = Vec::new();
        if let Some(m) = &cat {
            rest = qt.iter().enumerate().filter(|(i, _)| *i < m.start || *i >= m.end).map(|(_, t)| t.clone()).collect();
            if rest.is_empty() {
                return self.nearest_in_category(m.cats, near, limit);
            }
            // "מסעדות בתל אביב", "בית מרקחת פתוח ליד", "מסעדה כשרה באזור חיפה": the category plus
            // flags and where. A name ("קפה לנדוור") stays a name search below.
            let g = parse_rest(&rest);
            let phrase = qt[m.start..m.end].join(" ");
            let mut said: Vec<String> = vec![phrase];
            if g.kosher {
                said.push("כשר".into());
            }
            if g.open {
                said.push("פתוח עכשיו".into());
            }
            let f = PoiFilter { kosher: g.kosher, open: g.open };
            if g.place.is_empty() && (g.near || g.kosher || g.open) {
                said.push("בסביבה".into());
                let mut v = self.nearest_filtered(m.cats, near, limit, f)?;
                set_interpretation(&mut v, &said.join(" · "));
                return Ok(v);
            }
            if g.located && !g.place.is_empty() {
                if let Some(p) = self.find_place(&g.place) {
                    said.push(p.name.clone());
                    let mut v = self.nearest_filtered(m.cats, Some([p.lon, p.lat]), limit, f)?;
                    set_interpretation(&mut v, &said.join(" · "));
                    return Ok(v);
                }
            }
        }
        let fts_of = |toks: &[String]| {
            let mut parts: Vec<String> = toks.iter().map(|t| format!("\"{}\"", spell_fold(t).replace('"', ""))).collect();
            if let Some(l) = parts.last_mut() {
                l.push('*');
            }
            parts.join(" ")
        };
        let fts = fts_of(&qt);
        let qcanon = canon_tokens(&qt);
        let mut cands = self.fetch_cands(&fts, [80, 300, 150])?;
        let mut cat_cands: Vec<Cand> = Vec::new();
        let rest_canon = canon_tokens(&rest);
        if let Some(m) = &cat {
            cat_cands = self.fetch_pois_in(&fts_of(&rest), m.cats, 200)?;
        }
        let mut fuzzy = false;
        let now = hours::now_il();
        if cands.is_empty() && cat.is_none() {
            // typos anywhere in the word ("דיתנגוף"): closest street / place names by edit distance
            let similar = self.similar_names(&qcanon.join(" "), near, limit)?;
            if !similar.is_empty() {
                return Ok(similar);
            }
        }
        if cands.is_empty() {
            fuzzy = true;
            let groups: Vec<String> = qt
                .iter()
                .map(|t| {
                    let vs = prefix_variants(&spell_fold(t), 3);
                    let alts: Vec<String> = vs.iter().map(|v| format!("\"{}\"*", v.replace('"', ""))).collect();
                    format!("({})", alts.join(" OR "))
                })
                .collect();
            cands = self.fetch_cands(&groups.join(" AND "), [150, 500, 500])?;
        }
        let short = qt.len() <= 2;
        let mut scored: Vec<(f64, Cand)> = Vec::with_capacity(cands.len() + cat_cands.len());
        // (candidate, query tokens it is scored against, bonus, fuzzy): category hits are scored
        // on the remaining name tokens and get a small bonus over plain text hits.
        let pool = cands
            .into_iter()
            .map(|c| (c, &qcanon, 0.0, fuzzy))
            .chain(cat_cands.into_iter().map(|c| (c, &rest_canon, 8.0, false)));
        for (c, qcanon, bonus, fuzzy) in pool {
            let nts: Vec<Vec<String>> = c
                .names
                .iter()
                .filter(|n| !n.is_empty())
                .map(|n| canon_tokens(&tokens(n)))
                .collect();
            let city_t = if c.kind == GeoKind::Place && c.pop > 0 && c.category.as_deref().is_some_and(|k| matches!(k, "city" | "town" | "village" | "hamlet")) {
                vec![]
            } else {
                canon_tokens(&tokens(&c.city))
            };
            let type_bonus = match (c.kind, short) {
                (GeoKind::Place, true) => 12.0,
                (GeoKind::Street, true) => 6.0,
                (GeoKind::Place, false) => 4.0,
                (GeoKind::Street, false) => 2.0,
                _ => 0.0,
            };
            let boost = match c.kind {
                GeoKind::Place => 1.8 * pop_boost_city(c.pop),
                _ => pop_boost_city(c.pop),
            };
            let base = if !fuzzy {
                nts.iter().map(|nt| name_quality(nt, &city_t, qcanon)).fold(0.0, f64::max)
            } else {
                // typo tolerant: per query token, best (scaled) edit distance to any name/city token
                let mut best_total: Option<u32> = None;
                for nt in &nts {
                    let pool: Vec<&String> = nt.iter().chain(city_t.iter()).collect();
                    let mut total = 0u32;
                    let mut ok = true;
                    for q in qcanon.iter() {
                        let ql = q.chars().count();
                        let mut best = u32::MAX;
                        for t in &pool {
                            let d1 = levenshtein2(q, t);
                            let trunc: String = t.chars().take(ql).collect();
                            let d2 = levenshtein2(q, &trunc) + 1;
                            best = best.min(d1).min(d2);
                        }
                        // a word typed in full may also be a prefix (+1): allow that on top
                        let limit = allowed_edits2(ql) + 1;
                        if best > limit {
                            ok = false;
                            break;
                        }
                        total += best;
                    }
                    if ok && best_total.is_none_or(|b| total < b) {
                        best_total = Some(total);
                    }
                }
                match best_total {
                    Some(d) => 50.0 - 3.0 * d as f64,
                    None => continue,
                }
            };
            let s = base + type_bonus + boost + bonus - near_penalty(near, c.lon, c.lat);
            scored.push((s, c));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen = rustc_hash::FxHashSet::default();
        let mut out = Vec::new();
        for (s, c) in scored {
            // a landmark place and the same attraction as a POI are one result
            let k = if matches!(c.kind, GeoKind::Place | GeoKind::Poi) { 255 } else { c.kind as u8 };
            if !seen.insert((k, c.display.clone())) {
                continue;
            }
            let extra = match c.kind {
                GeoKind::Poi => poi_extra(c.hours.as_deref(), c.kosher, c.info.as_deref(), now),
                GeoKind::Place => PoiInfo::from_json(c.info.as_deref()).map(|i| {
                    let mut e = GeoExtra::default();
                    i.apply(&mut e);
                    Box::new(e)
                }),
                _ => None,
            };
            out.push(GeoResult {
                id: c.id,
                name: c.display,
                kind: c.kind,
                precision: if c.kind == GeoKind::Street { GeoPrecision::Street } else { GeoPrecision::City },
                lon: c.lon,
                lat: c.lat,
                city: if c.city.is_empty() { None } else { Some(c.city) },
                street: c.street,
                house_number: None,
                category: c.category,
                score: s,
                extra,
            });
            if out.len() >= limit {
                break;
            }
        }
        // typo-tolerant reading: say which name was found
        if fuzzy {
            if let Some(name) = out.first().map(|g| g.name.clone()) {
                set_interpretation(&mut out, &name);
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------- reverse

    pub fn reverse(&self, lon: f64, lat: f64) -> Result<Option<GeoResult>> {
        let here = [lon, lat];
        let dlat = |m: f64| m / 111_000.0;
        let dlon = |m: f64| m / (111_320.0 * lat.to_radians().cos().max(0.2));

        // a named place (synagogue, shop …) right at the point: that place, not its address
        {
            let r = 15.0;
            let mut st = self.conn.prepare_cached(&format!(
                "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat,{} FROM pois_rt t JOIN pois p ON p.id = t.id
                 WHERE t.minlon <= ?2 AND t.maxlon >= ?1 AND t.minlat <= ?4 AND t.maxlat >= ?3 AND p.name <> ''",
                self.poi_cols("p")
            ))?;
            let rows = st.query_map(params![lon - dlon(r), lon + dlon(r), lat - dlat(r), lat + dlat(r)], poi_row)?;
            let mut best: Option<(f64, PoiRow)> = None;
            for row in rows {
                let p = row?;
                let d = haversine_m(here, [p.lon, p.lat]);
                if d <= r && best.as_ref().is_none_or(|b| d < b.0) {
                    best = Some((d, p));
                }
            }
            if let Some((_, p)) = best {
                return Ok(Some(GeoResult {
                    id: format!("poi:{}", p.id),
                    name: p.name,
                    kind: GeoKind::Poi,
                    precision: GeoPrecision::Exact,
                    lon: p.lon,
                    lat: p.lat,
                    city: if p.city.is_empty() { None } else { Some(p.city) },
                    street: None,
                    house_number: None,
                    score: 0.0,
                    extra: poi_extra(p.hours.as_deref(), p.kosher, p.info.as_deref(), hours::now_il()),
                    category: Some(category_he(&p.cat)),
                }));
            }
        }

        // nearest address within 150 m
        {
            let (r, mut best) = (150.0, None::<(f64, GeoResult)>);
            let mut st = self.conn.prepare_cached(
                "SELECT id,street,housenumber,city,lon,lat FROM addresses WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4",
            )?;
            let rows = st.query_map(params![lat - dlat(r), lat + dlat(r), lon - dlon(r), lon + dlon(r)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            })?;
            for row in rows {
                let (id, street, hn, city, alon, alat) = row?;
                let d = haversine_m(here, [alon, alat]);
                if d <= r && best.as_ref().is_none_or(|b| d < b.0) {
                    let g = self.addr_result(format!("addr:{id}"), &street, &hn, &city, alon, alat, GeoPrecision::Exact, 200.0 - d, None);
                    best = Some((d, g));
                }
            }
            if let Some((_, g)) = best {
                return Ok(Some(g));
            }
        }

        // nearest street within 300 m
        {
            let r = 300.0;
            let mut st = self.conn.prepare_cached(
                "SELECT s.id,s.name,s.city,s.geom FROM streets_rt r JOIN streets s ON s.id = r.id
                 WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3",
            )?;
            let rows = st.query_map(params![lon - dlon(r), lon + dlon(r), lat - dlat(r), lat + dlat(r)], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Vec<u8>>(3)?))
            })?;
            let mut best: Option<(f64, i64, String, String, [f64; 2])> = None;
            for row in rows {
                let (id, name, city, geom) = row?;
                for way in builder::decode_geom(&geom) {
                    for w in way.windows(2) {
                        let (d, p) = point_segment_m(here, w[0], w[1]);
                        if d <= r && best.as_ref().is_none_or(|b| d < b.0) {
                            best = Some((d, id, name.clone(), city.clone(), p));
                        }
                    }
                }
            }
            if let Some((d, id, name, city, p)) = best {
                return Ok(Some(GeoResult {
                    id: format!("street:{id}"),
                    name: with_city(&name, &city),
                    kind: GeoKind::Street,
                    precision: GeoPrecision::Street,
                    lon: p[0],
                    lat: p[1],
                    city: if city.is_empty() { None } else { Some(city) },
                    street: Some(name),
                    house_number: None,
                    category: None,
                    score: 100.0 - d / 10.0,
                    extra: None,
                }));
            }
        }

        // nearest POI within 100 m
        {
            let r = 100.0;
            let mut st = self.conn.prepare_cached(
                "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat FROM pois_rt r JOIN pois p ON p.id = r.id
                 WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3",
            )?;
            let rows = st.query_map(params![lon - dlon(r), lon + dlon(r), lat - dlat(r), lat + dlat(r)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            })?;
            let mut best: Option<(f64, GeoResult)> = None;
            for row in rows {
                let (id, name, cat, city, plon, plat) = row?;
                let d = haversine_m(here, [plon, plat]);
                if d <= r && best.as_ref().is_none_or(|b| d < b.0) {
                    best = Some((
                        d,
                        GeoResult {
                            id: format!("poi:{id}"),
                            name: with_city(&name, &city),
                            kind: GeoKind::Poi,
                            precision: GeoPrecision::Exact,
                            lon: plon,
                            lat: plat,
                            city: if city.is_empty() { None } else { Some(city) },
                            street: None,
                            house_number: None,
                            category: Some(category_he(&cat)),
                            score: 50.0 - d / 10.0,
                            extra: None,
                        },
                    ));
                }
            }
            if let Some((_, g)) = best {
                return Ok(Some(g));
            }
        }

        // nearest place
        let mut best: Option<(f64, &PlaceMem)> = None;
        for p in &self.places().list {
            let d = haversine_m(here, [p.lon, p.lat]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, p));
            }
        }
        Ok(best.map(|(d, p)| self.place_result(p, 20.0 - d / 1000.0)))
    }
}

/// The route number of a road query: "כביש 6", "כביש443", "דרך 1", "road 90", "route 6".
fn road_number(q: &str) -> Option<String> {
    let t = q.trim().to_lowercase();
    let rest = ["כביש", "דרך", "road", "route", "highway", "rd"].iter().find_map(|w| t.strip_prefix(w))?;
    let n = rest.trim();
    (!n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit())).then(|| n.to_string())
}

/// One `pois` row (the extra columns are NULL / 0 in indexes built before them).
struct PoiRow {
    id: i64,
    name: String,
    cat: String,
    city: String,
    lon: f64,
    lat: f64,
    hours: Option<String>,
    kosher: bool,
    minzoom: i64,
    info: Option<String>,
}

fn poi_row(r: &rusqlite::Row) -> rusqlite::Result<PoiRow> {
    Ok(PoiRow {
        id: r.get(0)?,
        name: r.get(1)?,
        cat: r.get(2)?,
        city: r.get(3)?,
        lon: r.get(4)?,
        lat: r.get(5)?,
        hours: r.get(6)?,
        kosher: r.get::<_, Option<i64>>(7)?.unwrap_or(0) & 1 == 1,
        minzoom: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
        info: r.get(9)?,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct PoiFilter {
    kosher: bool,
    open: bool,
}

/// The words of a category query besides the category phrase.
#[derive(Debug, Default, PartialEq)]
struct RestParse {
    kosher: bool,
    open: bool,
    /// "ליד" / "בסביבה": around the user
    near: bool,
    /// a locative marker: "ב…" prefix on the place, or "באזור" / "בעיר" / "ליד" + place
    located: bool,
    /// the remaining words (a place name, or part of a POI name)
    place: Vec<String>,
}

fn parse_rest(rest: &[String]) -> RestParse {
    const KOSHER: &[&str] = &["כשר", "כשרה", "כשרים", "כשרות", "kosher"];
    const OPEN: &[&str] = &["פתוח", "פתוחה", "פתוחים", "פתוחות", "עכשיו", "open", "now"];
    const NEAR: &[&str] = &["ליד", "לידי", "קרוב", "קרובה", "קרובים", "בסביבה", "בסביבתי", "סביבי", "near", "nearby", "around"];
    const AREA: &[&str] = &["באזור", "בעיר", "בקרבת", "בסביבת", "ליד", "in"];
    let mut g = RestParse::default();
    for t in rest {
        let w = strip_he(t);
        if KOSHER.contains(&t.as_str()) || KOSHER.contains(&w) {
            g.kosher = true;
        } else if OPEN.contains(&t.as_str()) {
            g.open = true;
        } else if AREA.contains(&t.as_str()) && !g.place.is_empty() {
            g.place.push(t.clone()); // "ליד" inside a name: keep it
        } else if AREA.contains(&t.as_str()) {
            g.located = true;
            if NEAR.contains(&t.as_str()) {
                g.near = true;
            }
        } else if NEAR.contains(&t.as_str()) {
            g.near = true;
        } else {
            g.place.push(t.clone());
        }
    }
    if let Some(first) = g.place.first() {
        // "בתל אביב", "בחיפה": the preposition glued to the place
        if first.starts_with('ב') && first.chars().count() >= 3 {
            g.located = true;
        }
    }
    if !g.place.is_empty() && g.located {
        g.near = false; // "ליד חיפה" = around Haifa, not around me
    }
    g
}

/// Same address from several sources (OSM with "תל אביב" and "תל אביב–יפו", data.gov.il, a
/// second street reading): equal normalised street and house number, the same city (one city
/// name may extend the other by whole words) and less than 50 m apart. Keeps the best score and
/// takes the most precise, then the longer (official) city name. Order is preserved.
pub fn dedupe_addresses(v: Vec<GeoResult>) -> Vec<GeoResult> {
    fn same_city(a: &str, b: &str) -> bool {
        let (a, b) = (canon(a), canon(b));
        a == b || a.starts_with(&format!("{b} ")) || b.starts_with(&format!("{a} "))
    }
    fn rank(p: &GeoPrecision) -> u8 {
        match p {
            GeoPrecision::Exact => 0,
            GeoPrecision::Interpolated => 1,
            GeoPrecision::Street => 2,
            GeoPrecision::City => 3,
        }
    }
    fn pref(g: &GeoResult) -> (u8, std::cmp::Reverse<usize>) {
        (rank(&g.precision), std::cmp::Reverse(g.city.as_deref().map_or(0, |c| c.chars().count())))
    }
    let key = |g: &GeoResult| (g.street.as_deref().map(canon), g.house_number.as_deref().map(norm));
    // "placed at the nearest existing number" results of one street in one town are one answer
    // however far apart their stand-in numbers are (keep the better scored)
    let near_no = |g: &GeoResult| g.extra.as_ref().is_some_and(|e| e.near_house_number.is_some());
    let mut out: Vec<GeoResult> = Vec::with_capacity(v.len());
    for g in v {
        let dup = if g.kind == GeoKind::Address {
            let k = key(&g);
            out.iter().position(|o| {
                o.kind == GeoKind::Address
                    && key(o) == k
                    && same_city(o.city.as_deref().unwrap_or(""), g.city.as_deref().unwrap_or(""))
                    && (haversine_m([o.lon, o.lat], [g.lon, g.lat]) < 50.0 || (near_no(o) && near_no(&g)))
            })
        } else {
            None
        };
        match dup {
            None => out.push(g),
            Some(i) => {
                let o = &mut out[i];
                let score = o.score.max(g.score);
                if near_no(o) && near_no(&g) {
                    if g.score > o.score {
                        *o = g;
                    }
                } else if pref(&g) < pref(o) {
                    *o = g;
                }
                o.score = score;
            }
        }
    }
    out
}

/// Merge geocoder results with transit-stop results by score (stops are never dropped just
/// because the geocoder filled the limit).
pub fn merge_with_stops(mut geo: Vec<GeoResult>, stops: Vec<GeoResult>, limit: usize) -> Vec<GeoResult> {
    let limit = limit.max(1);
    let by_score = |a: &GeoResult, b: &GeoResult| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal);
    geo.extend(stops);
    geo.sort_by(by_score);
    if geo.len() > limit {
        // keep up to two reasonable stop matches visible even when places fill the list
        let mut keep: Vec<GeoResult> = Vec::new();
        for s in geo.iter().skip(limit).filter(|r| r.kind == GeoKind::Stop && r.score >= 45.0).take(2) {
            keep.push(s.clone());
        }
        geo.truncate(limit);
        if !keep.is_empty() && !geo.iter().any(|r| r.kind == GeoKind::Stop) {
            let n = keep.len().min(geo.len());
            geo.truncate(geo.len() - n);
            geo.extend(keep);
        }
    }
    geo
}

/// Distance in meters from `p` to segment `a`-`b` (all [lon,lat]) and the closest point on it.
fn point_segment_m(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, [f64; 2]) {
    let kx = 111_320.0 * p[1].to_radians().cos();
    let ky = 110_574.0;
    let (ax, ay) = ((a[0] - p[0]) * kx, (a[1] - p[1]) * ky);
    let (bx, by) = ((b[0] - p[0]) * kx, (b[1] - p[1]) * ky);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-9 { 0.0 } else { (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((cx * cx + cy * cy).sqrt(), [p[0] + cx / kx, p[1] + cy / ky])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance() {
        let (d, p) = point_segment_m([34.78, 32.07], [34.77, 32.06], [34.79, 32.06]);
        assert!((d - 1105.0).abs() < 10.0, "{d}");
        assert!((p[0] - 34.78).abs() < 1e-4);
        let (d0, _) = point_segment_m([34.7, 32.0], [34.7, 32.0], [34.7, 32.0]);
        assert!(d0 < 1e-6);
    }

    #[test]
    fn coord_parse() {
        let r = parse_coord("32.0853, 34.7818", None).unwrap();
        assert_eq!(r.kind, GeoKind::Coord);
        assert!((r.lon - 34.7818).abs() < 1e-9);
        assert!(parse_coord("הרצל 12", None).is_none());
    }

    fn addr(city: &str, street: &str, hn: &str, lon: f64, lat: f64, score: f64) -> GeoResult {
        GeoResult {
            id: format!("addr:{city}:{score}"),
            name: format!("{street} {hn}, {city}"),
            kind: GeoKind::Address,
            precision: GeoPrecision::Exact,
            lon,
            lat,
            city: Some(city.to_string()),
            street: Some(street.to_string()),
            house_number: Some(hn.to_string()),
            category: None,
            score,
            extra: None,
        }
    }

    fn toks(s: &str) -> Vec<String> {
        tokens(s)
    }

    #[test]
    fn category_query_grammar() {
        // "מסעדות בתל אביב": the place, located by the glued preposition
        let g = parse_rest(&toks("בתל אביב"));
        assert!(g.located && !g.near && !g.kosher && !g.open);
        assert_eq!(g.place, toks("בתל אביב"));
        // "בית מרקחת פתוח ליד": open + around me
        let g = parse_rest(&toks("פתוח ליד"));
        assert!(g.open && g.near && g.place.is_empty());
        // "מסעדה כשרה באזור חיפה"
        let g = parse_rest(&toks("כשרה באזור חיפה"));
        assert!(g.kosher && g.located && !g.near);
        assert_eq!(g.place, toks("חיפה"));
        // "קפה לנדוור": a name, no flags, not located
        let g = parse_rest(&toks("לנדוור"));
        assert_eq!(g, RestParse { place: toks("לנדוור"), ..Default::default() });
        // "בית כנסת חניכי הישיבות": a name too
        assert!(!parse_rest(&toks("חניכי הישיבות")).located);
    }

    #[test]
    fn interpretation_on_first_only() {
        let mut v = vec![addr("א", "ב", "1", 34.0, 32.0, 1.0), addr("א", "ב", "2", 34.0, 32.0, 1.0)];
        v[1].extra = Some(Box::new(GeoExtra { interpretation: Some("x".into()), ..Default::default() }));
        set_interpretation(&mut v, "הרצל 12 תל אביב");
        keep_first_interpretation(&mut v);
        assert_eq!(v[0].extra.as_ref().unwrap().interpretation.as_deref(), Some("הרצל 12 תל אביב"));
        assert!(v[1].extra.is_none(), "an emptied extra is dropped");
        let json = serde_json::to_value(&v[0]).unwrap();
        assert_eq!(json["interpretation"], "הרצל 12 תל אביב", "flattened into the result object");
        assert!(json.get("openNow").is_none());
    }

    #[test]
    fn distance_decay() {
        let here = Some([34.78, 32.08]);
        let p = |lon: f64, lat: f64| near_penalty(here, lon, lat);
        assert!(p(34.78, 32.08) < 1e-9);
        let (tlv_haifa, tlv_jlm, tlv_eilat) = (p(34.99, 32.79), p(35.21, 31.77), p(34.95, 29.55));
        assert!(tlv_jlm < tlv_haifa && tlv_haifa < tlv_eilat);
        // never enough to turn an exact address (200) into less than an interpolated one (150)
        assert!(tlv_eilat < 15.0, "{tlv_eilat}");
        assert_eq!(near_penalty(None, 35.0, 31.0), 0.0);
    }

    /// Real-data measurement (installed index + a fresh build): `cargo test --release --features
    /// cli --lib measure_search -- --ignored --nocapture`. Writes a UTF-8 report to the temp dir.
    #[test]
    #[ignore]
    fn measure_search() {
        use std::fmt::Write as _;
        use std::time::Instant;
        let root = crate::paths::DataPaths::default_root();
        let installed = root.join("geocode").join("index.sqlite");
        let mut rep = String::new();
        let tlv = Some([34.78, 32.08]);
        let typos = ["הרצאל 5 תל אביב", "בן יהודא 20 תל אביב", "דיזנגופ 50", "ז׳בוטינסקה 3 רמת גן"];
        // ---- old algorithm (read + fold every street key per query) on the installed index
        {
            let g = open(&installed).unwrap();
            writeln!(rep, "installed index {} MB, caps {:?}", std::fs::metadata(&installed).unwrap().len() / 1_000_000, g.caps).unwrap();
            for q in typos {
                let cands = address_candidates(q);
                let t = Instant::now();
                let mut n = 0;
                for c in &cands {
                    let mut st = g.conn.prepare_cached("SELECT DISTINCT ckey, city FROM streets").unwrap();
                    let rows: Vec<(String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
                    let qf = spell_fold(&c.street);
                    for (k, _) in &rows {
                        if levenshtein2(&spell_fold(k), &qf) <= 4 {
                            n += 1;
                        }
                    }
                }
                writeln!(rep, "OLD similar_street_keys {q}: {:?} ({n} hits)", t.elapsed()).unwrap();
            }
            let _ = g.street_keys();
            for q in typos {
                let cands = address_candidates(q);
                let t = Instant::now();
                let k = g.similar_street_keys(&cands).unwrap();
                writeln!(rep, "NEW (no trigram table, cached keys) {q}: {:?} -> {:?}", t.elapsed(), k.iter().map(|c| &c.street).collect::<Vec<_>>()).unwrap();
            }
        }
        // ---- fresh build with synonyms, hours, thinning, trigrams
        let pbf = root.join("downloads").join("israel-and-palestine-latest.osm.pbf");
        let out = std::env::temp_dir().join("mo-geocode-measure.sqlite");
        let syn = datagov::fetch_street_synonyms(&|_| {});
        writeln!(rep, "synonym groups fetched: {}", syn.len()).unwrap();
        let t = Instant::now();
        builder::build_at(&pbf, &out, 31.5, &[], &syn, false, &|_, _, _, _| true).unwrap();
        writeln!(rep, "build: {:?}, size {} MB", t.elapsed(), std::fs::metadata(&out).unwrap().len() / 1_000_000).unwrap();
        let g = open(&out).unwrap();
        let tri_bytes: i64 = g.conn.query_row("SELECT coalesce(sum(length(block)),0) FROM streets_tri_data", [], |r| r.get(0)).unwrap_or(-1);
        let syn_rows: i64 = g.conn.query_row("SELECT count(*) FROM street_syn", [], |r| r.get(0)).unwrap();
        let hours_rows: i64 = g.conn.query_row("SELECT count(*) FROM pois WHERE hours IS NOT NULL", [], |r| r.get(0)).unwrap();
        let kosher_rows: i64 = g.conn.query_row("SELECT count(*) FROM pois WHERE flags & 1", [], |r| r.get(0)).unwrap();
        let pois: i64 = g.conn.query_row("SELECT count(*) FROM pois", [], |r| r.get(0)).unwrap();
        writeln!(rep, "caps {:?}; trigram data {} KB; synonym links {syn_rows}; pois {pois}, with hours {hours_rows}, kosher {kosher_rows}", g.caps, tri_bytes / 1000).unwrap();
        for z in [6, 8, 10, 12, 14, 16, 17, 18] {
            let n: i64 = g.conn.query_row("SELECT count(*) FROM pois WHERE minzoom <= ?1", [z], |r| r.get(0)).unwrap();
            write!(rep, "z{z}:{n} ").unwrap();
        }
        writeln!(rep).unwrap();
        for q in typos {
            let cands = address_candidates(q);
            let t = Instant::now();
            let k = g.similar_street_keys(&cands).unwrap();
            writeln!(rep, "NEW (trigram) {q}: {:?} -> {:?}", t.elapsed(), k.iter().map(|c| &c.street).collect::<Vec<_>>()).unwrap();
        }
        // map layer: all restaurants+cafes in central Tel Aviv, by zoom
        let bbox = [34.76, 32.06, 34.80, 32.09];
        for z in [None, Some(13.0), Some(15.0), Some(17.0)] {
            let t = Instant::now();
            let v = g.pois_in_bbox(&["restaurant", "cafe", "fast_food"], bbox, 1500, z).unwrap();
            let open_known = v.iter().filter(|r| r.extra.as_ref().and_then(|e| e.open_now).is_some()).count();
            writeln!(rep, "poi_layer zoom {z:?}: {} in {:?} ({open_known} with known hours)", v.len(), t.elapsed()).unwrap();
        }
        let queries = [
            "הרצל 12 תל אביב", "מנחם בגין 100 תל אביב", "דרך מנחם בגין 132 תל אביב", "בגין 20 תל אביב", "הרצל 999 תל אביב",
            "מסעדות בתל אביב", "בית מרקחת פתוח ליד", "מסעדה כשרה באזור חיפה", "קפה לנדוור", "בית כנסת חניכי הישיבות",
            "דיתנגוף", "vrmk 12 ,k tchc", "ת\"א", "כביש 6", "רחל 12 ירושלים", "ז'ולטי 7", "מימון 3 ירושלים", "הרצאל 5 תל אביב", "סופר פארם",
        ];
        for q in queries {
            let t = Instant::now();
            let r = g.search(q, tlv, 5).unwrap();
            writeln!(rep, "== {q}  ({:?})", t.elapsed()).unwrap();
            for x in r.iter().take(4) {
                writeln!(rep, "   {:?} {:?} {} | {:?} score {:.1} extra {:?}", x.kind, x.precision, x.name, x.category, x.score, x.extra).unwrap();
            }
        }
        let path = std::env::temp_dir().join("mo-measure-search.txt");
        std::fs::write(&path, rep).unwrap();
        println!("report: {}", path.display());
    }

    #[test]
    fn road_queries() {
        assert_eq!(road_number("כביש 6").as_deref(), Some("6"));
        assert_eq!(road_number("כביש443").as_deref(), Some("443"));
        assert_eq!(road_number("דרך 1").as_deref(), Some("1"));
        assert_eq!(road_number("Route 90").as_deref(), Some("90"));
        assert_eq!(road_number("דרך בגין 12"), None);
        assert_eq!(road_number("כביש"), None);
    }

    #[test]
    fn dedupe_same_address_from_several_sources() {
        let d = dedupe_addresses(vec![
            addr("תל אביב–יפו", "הרצל", "12", 34.76995, 32.06231, 202.8),
            addr("תל אביב", "הרצל", "12", 34.77000, 32.06223, 200.0),
            addr("ירושלים", "הרצל", "12", 35.19909, 31.78756, 82.9),
            addr("תל אביב–יפו", "הרצל", "12", 34.76995, 32.06231, 82.8),
            addr("תל אביב–יפו", "הרצל", "14", 34.76990, 32.06250, 80.0),
        ]);
        assert_eq!(d.len(), 3, "{:?}", d.iter().map(|g| &g.name).collect::<Vec<_>>());
        assert_eq!(d[0].city.as_deref(), Some("תל אביב–יפו"));
        assert!((d[0].score - 202.8).abs() < 1e-9);
        assert_eq!(d[1].city.as_deref(), Some("ירושלים"));
        assert_eq!(d[2].house_number.as_deref(), Some("14"));
        // the longer official city name wins even when the shorter one scored higher
        let d = dedupe_addresses(vec![
            addr("תל אביב", "הרצל", "12", 34.77000, 32.06223, 210.0),
            addr("תל אביב-יפו", "הרצל", "12", 34.76995, 32.06231, 200.0),
        ]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].city.as_deref(), Some("תל אביב-יפו"));
        assert!((d[0].score - 210.0).abs() < 1e-9);
        // same street and number far apart stay separate
        let d = dedupe_addresses(vec![
            addr("רמת גן", "הרצל", "12", 34.81, 32.08, 100.0),
            addr("רמת גן", "הרצל", "12", 34.83, 32.09, 90.0),
        ]);
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn geom_codec() {
        let ways = vec![vec![(347_800_000, 320_600_000), (347_900_000, 320_700_000)], vec![(1, 2)]];
        let b = builder::encode_geom(&ways);
        let d = builder::decode_geom(&b);
        assert_eq!(d.len(), 2);
        assert!((d[0][1][0] - 34.79).abs() < 1e-9);
    }
}

#[cfg(test)]
mod replace_tests {
    use std::sync::Mutex;

    static HELD: Mutex<Option<rusqlite::Connection>> = Mutex::new(None);

    /// An idle pooled connection keeps the file open: replacing must close it first (Windows).
    #[test]
    fn replace_closes_pooled_connections() {
        let dir = std::env::temp_dir().join(format!("mo-geo-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (dest, tmp) = (dir.join("geocode.sqlite"), dir.join("geocode.sqlite.tmp"));
        rusqlite::Connection::open(&dest).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);").unwrap();
        rusqlite::Connection::open(&tmp).unwrap().execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (2);").unwrap();
        let c = rusqlite::Connection::open_with_flags(&dest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let _: i64 = c.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        *HELD.lock().unwrap() = Some(c);
        #[cfg(windows)]
        assert!(std::fs::remove_file(&dest).is_err(), "Windows refuses to delete an open file");
        super::set_release_hook(|| {
            HELD.lock().unwrap().take();
        });
        super::replace_index(&dest, &tmp).expect("replaced");
        let x: i64 = rusqlite::Connection::open(&dest).unwrap().query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
