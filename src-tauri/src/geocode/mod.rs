//! Offline geocoder: SQLite FTS5 index built from OSM PBF.
//!
//! * `build` creates the index (places / streets / addresses / pois).
//! * `open` opens it read-only; `search` and `reverse` answer queries.
mod builder;
pub mod datagov;
pub use builder::{BUILD_REV, SCHEMA_VERSION};
pub mod categories;
pub mod norm;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use rustc_hash::FxHashMap;

use crate::geo::haversine_m;
use crate::types::{GeoKind, GeoPrecision, GeoResult, LonLat, ProgressFn};
use norm::{norm, AddrCand, address_candidates, canon, canon_tokens, expand_abbreviations, from_latin_keyboard, is_type_word, levenshtein2, prefix_variants, spell_fold, tokens};

/// Israel: OSM plus the open-data points of data.gov.il (fetched now; skipped when offline).
pub fn build(pbf: &Path, out: &Path, progress: ProgressFn) -> Result<()> {
    let extra = datagov::fetch_all(&|m| {
        progress(crate::types::ProgressPhase::Processing, 0, 100, m);
    });
    builder::build_at(pbf, out, 31.5, &extra, progress)
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
    builder::build_at(pbf, out, ref_lat, &[], progress)
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
}

/// Settlement / neighbourhood table kept in memory (≈4k rows). Built lazily on first use so
/// that `open` only opens the SQLite file; shared by all connections of one index.
#[derive(Default)]
struct Places {
    list: Vec<PlaceMem>,
    /// settlement name -> population (for ranking streets/POIs by their city).
    city_pop: FxHashMap<String, i64>,
}

/// Lazily-filled place cache shared between connections to the same index file.
#[derive(Clone, Default)]
pub struct SharedCache(Arc<OnceLock<Places>>);

pub struct Geocoder {
    conn: Connection,
    cache: SharedCache,
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
    Ok(Geocoder { conn, cache })
}

fn load_places(conn: &Connection) -> Result<Places> {
    let mut places = Vec::new();
    let mut city_pop: FxHashMap<String, i64> = FxHashMap::default();
    let mut st = conn.prepare("SELECT id,name_he,name_en,kind,lon,lat,population,city FROM places")?;
    let rows = st.query_map([], |r| {
        let name: String = r.get(1)?;
        let en: String = r.get(2)?;
        Ok(PlaceMem {
            id: r.get(0)?,
            canon: canon(if name.is_empty() { &en } else { &name }),
            name: if name.is_empty() { en } else { name },
            kind: r.get(3)?,
            lon: r.get(4)?,
            lat: r.get(5)?,
            pop: r.get(6)?,
            city: r.get(7)?,
        })
    })?;
    for p in rows {
        let p = p?;
        if matches!(p.kind.as_str(), "city" | "town" | "village" | "hamlet") {
            let e = city_pop.entry(p.name.clone()).or_insert(0);
            *e = (*e).max(p.pop);
        }
        places.push(p);
    }
    Ok(Places { list: places, city_pop })
}

impl Geocoder {
    fn places(&self) -> &Places {
        self.cache.0.get_or_init(|| {
            load_places(&self.conn).unwrap_or_else(|e| {
                log::warn!("geocoder: loading places failed: {e:#}");
                Places::default()
            })
        })
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

fn near_penalty(near: Option<LonLat>, lon: f64, lat: f64) -> f64 {
    match near {
        Some(n) => haversine_m(n, [lon, lat]) / 1000.0 / 50.0,
        None => 0.0,
    }
}

fn parse_coord(q: &str) -> Option<GeoResult> {
    let parts: Vec<&str> = q.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }
    let a: f64 = parts[0].parse().ok()?;
    let b: f64 = parts[1].parse().ok()?;
    let in_lat = |v: f64| (29.0..=34.5).contains(&v);
    let in_lon = |v: f64| (33.5..=36.5).contains(&v);
    let (lat, lon) = if in_lat(a) && in_lon(b) {
        (a, b)
    } else if in_lat(b) && in_lon(a) {
        (b, a)
    } else {
        return None;
    };
    Some(GeoResult {
        id: format!("coord:{lon:.6},{lat:.6}"),
        name: format!("{lat:.5}, {lon:.5}"),
        kind: GeoKind::Coord,
        precision: GeoPrecision::Exact,
        lon,
        lat,
        city: None,
        street: None,
        house_number: None,
        category: None,
        score: 1000.0,
    })
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
        if let Some(c) = parse_coord(q) {
            return Ok(vec![c]);
        }
        if let Some(n) = road_number(q) {
            let roads = self.search_road(&n, near, limit)?;
            if !roads.is_empty() {
                return Ok(roads);
            }
        }
        let r = dedupe_addresses(self.search_rewritten(q, near, limit)?);
        // Keyboard left in English ("shzbdu; 50" -> "דיזנגוף 50"): try the Hebrew reading too and
        // keep whichever answers better (a city-only hit for the Hebrew part must not win).
        if let Some(h) = from_latin_keyboard(q) {
            let r2 = dedupe_addresses(self.search_rewritten(&h, near, limit)?);
            let best = |v: &[GeoResult]| v.first().map_or(f64::MIN, |g| g.score);
            if best(&r2) > best(&r) {
                return Ok(r2);
            }
        }
        Ok(r)
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
        let q = if expanded.split(' ').count() != norm(q).split(' ').count() { expanded.as_str() } else { q };
        let cands = address_candidates(q);
        if !cands.is_empty() {
            let r = self.search_address(&cands, near, limit)?;
            if !r.is_empty() {
                return Ok(r);
            }
        }
        self.search_text(q, near, limit)
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
        }
    }

    fn search_address(&self, cands: &[AddrCand], near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
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
        if !has_house(&res) {
            // The street the user typed may be a part of the mapped name ("ז'ולטי 7" for
            // "הרב בצלאל ז'ולטי 7"), or spelled differently ("מימון" / "מיימון"): resolve candidate
            // street keys through the (spelling-folded) streets index.
            let expanded = self.expand_street_keys(cands)?;
            if !expanded.is_empty() {
                res.extend(self.addr_lookup(&expanded, near, 10.0)?);
            }
        }
        if !has_house(&res) {
            // typos the token index cannot bridge ("הרצאל 5"): closest street names by edit distance
            let close = self.similar_street_keys(&if with_city.is_empty() { cands.to_vec() } else { with_city.clone() })?;
            if !close.is_empty() {
                res.extend(self.addr_lookup(&close, near, 15.0)?);
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
        Ok(res)
    }

    /// Streets and places whose name is within a small edit distance of the whole query (used
    /// when the token index finds nothing at all). Results are street-level / place hits.
    fn similar_names(&self, q: &str, near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let qf = spell_fold(q);
        let n = qf.chars().count();
        if n < 4 {
            return Ok(vec![]);
        }
        let max = if n >= 8 { 4 } else { 2 };
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
        let mut stmt = self.conn.prepare_cached("SELECT id, name, city, ckey, lon, lat FROM streets")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?, r.get::<_, f64>(5)?)))?;
        for row in rows {
            let (id, name, city, ckey, lon, lat) = row?;
            let k = spell_fold(&ckey);
            if k.chars().count().abs_diff(n) > 2 {
                continue;
            }
            let d = levenshtein2(&k, &qf);
            if d <= max {
                out.push((d, self.addr_result(format!("street:{id}"), &name, "", &city, lon, lat, GeoPrecision::Street, 60.0 - d as f64 * 5.0 + self.city_boost(&city), near)));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal)));
        out.truncate(limit);
        Ok(out.into_iter().map(|(_, g)| g).collect())
    }

    /// Streets whose (spelling-folded) key is within a small edit distance of the typed one,
    /// restricted to the candidate's city when one was given. Insert/delete cost 2, substitution
    /// 2, confusable-letter substitution 1 (`levenshtein2`); allowance grows with the name length.
    fn similar_street_keys(&self, cands: &[AddrCand]) -> Result<Vec<AddrCand>> {
        let mut out: Vec<AddrCand> = Vec::new();
        let mut stmt = self.conn.prepare_cached("SELECT DISTINCT ckey, city FROM streets")?;
        let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        for c in cands {
            let q = spell_fold(&c.street);
            let n = q.chars().count();
            let max = if n >= 8 { 4 } else if n >= 4 { 2 } else { continue };
            let mut cache = FxHashMap::default();
            let mut hits: Vec<(u32, usize, String)> = Vec::new();
            for (ckey, city) in &rows {
                if let Some(qc) = &c.city {
                    if !city_matches(city, qc, &mut cache) {
                        continue;
                    }
                }
                let k = spell_fold(ckey);
                if k.chars().count().abs_diff(n) > 2 {
                    continue;
                }
                let d = levenshtein2(&k, &q);
                if d > 0 && d <= max && !hits.iter().any(|h| h.2 == *ckey) {
                    hits.push((d, ckey.chars().count(), ckey.clone()));
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
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id,street,housenumber,city,lon,lat FROM addresses WHERE street_key=?1 AND hn_key=?2")?;
        for c in cands {
            let mut cache = FxHashMap::default();
            let rows = stmt.query_map(params![c.street, c.hn], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?, r.get::<_, f64>(5)?))
            })?;
            let mut by_city: FxHashMap<String, GeoResult> = FxHashMap::default();
            for row in rows {
                let (id, street, hn, city, lon, lat) = row?;
                if let Some(qc) = &c.city {
                    if !city_matches(&city, qc, &mut cache) {
                        continue;
                    }
                }
                let g = self.addr_result(format!("addr:{id}"), &street, &hn, &city, lon, lat, GeoPrecision::Exact, 200.0 - penalty, near);
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
                    };
                    let Some((lo, hi)) = pair else { continue };
                    if haversine_m([lo.1, lo.2], [hi.1, hi.2]) > 1500.0 {
                        continue;
                    }
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
        }
    }

    // -------------------------------------------------------------- free text

    fn fetch_cands(&self, fts: &str, per_table: [usize; 3]) -> Result<Vec<Cand>> {
        let mut out = Vec::new();
        {
            let mut st = self.conn.prepare_cached(
                "SELECT p.id,p.name_he,p.name_en,p.name_ar,p.kind,p.lon,p.lat,p.population,p.city
                 FROM places_fts JOIN places p ON p.id = places_fts.rowid
                 WHERE places_fts MATCH ?1 ORDER BY places_fts.rowid LIMIT ?2",
            )?;
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
                ))
            })?;
            for row in rows {
                let (id, he, en, ar, kind, lon, lat, pop, city) = row?;
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
                });
            }
        }
        {
            let mut st = self.conn.prepare_cached(
                "SELECT s.id,s.name,s.category,s.city,s.lon,s.lat
                 FROM pois_fts JOIN pois s ON s.id = pois_fts.rowid
                 WHERE pois_fts MATCH ?1 ORDER BY pois_fts.rowid LIMIT ?2",
            )?;
            let rows = st.query_map(params![fts, per_table[2] as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })?;
            for row in rows {
                let (id, name, cat, city, lon, lat) = row?;
                out.push(self.poi_cand(id, name, &cat, city, lon, lat));
            }
        }
        Ok(out)
    }

    fn poi_cand(&self, id: i64, name: String, cat: &str, city: String, lon: f64, lat: f64) -> Cand {
        Cand {
            kind: GeoKind::Poi,
            id: format!("poi:{id}"),
            display: with_city(&name, &city),
            names: [name, String::new(), String::new()],
            pop: self.places().city_pop.get(&city).copied().unwrap_or(0),
            city,
            category: Some(category_he(cat)),
            street: None,
            lon,
            lat,
        }
    }

    /// POIs of the given categories inside `bbox` = [min_lon, min_lat, max_lon, max_lat] (map
    /// layer). `cats` entries are category ids as in `categories::LEXICON` ("fuel",
    /// "place_of_worship:jewish"); at most `limit` results, nearest to the bbox centre first.
    pub fn pois_in_bbox(&self, cats: &[&str], bbox: [f64; 4], limit: usize) -> Result<Vec<GeoResult>> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat FROM pois_rt r JOIN pois p ON p.id = r.id
             WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3",
        )?;
        let centre = [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0];
        let rows = st.query_map(params![bbox[0], bbox[2], bbox[1], bbox[3]], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?, r.get::<_, f64>(5)?))
        })?;
        let mut found: Vec<(f64, GeoResult)> = Vec::new();
        for row in rows {
            let (id, name, cat, city, lon, lat) = row?;
            if !categories::matches(&cat, cats) {
                continue;
            }
            found.push((
                haversine_m(centre, [lon, lat]),
                GeoResult {
                    id: format!("poi:{id}"),
                    name: if name.is_empty() { category_he(&cat) } else { name },
                    kind: GeoKind::Poi,
                    precision: GeoPrecision::Exact,
                    lon,
                    lat,
                    city: if city.is_empty() { None } else { Some(city) },
                    street: None,
                    house_number: None,
                    category: Some(cat),
                    score: 0.0,
                },
            ));
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(found.into_iter().take(limit).map(|(_, g)| g).collect())
    }

    /// POIs matching `fts` whose category is in `cats`.
    fn fetch_pois_in(&self, fts: &str, cats: &[&str], limit: usize) -> Result<Vec<Cand>> {
        let mut st = self.conn.prepare_cached(
            "SELECT s.id,s.name,s.category,s.city,s.lon,s.lat
             FROM pois_fts JOIN pois s ON s.id = pois_fts.rowid
             WHERE pois_fts MATCH ?1 ORDER BY pois_fts.rowid LIMIT 800",
        )?;
        let rows = st.query_map(params![fts], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, cat, city, lon, lat) = row?;
            if !categories::matches(&cat, cats) {
                continue;
            }
            out.push(self.poi_cand(id, name, &cat, city, lon, lat));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Nearest POIs of the given categories around `near` (category-only queries: "בית כנסת").
    fn nearest_in_category(&self, cats: &[&str], near: Option<LonLat>, limit: usize) -> Result<Vec<GeoResult>> {
        let [lon, lat] = near.unwrap_or([35.0, 31.6]);
        let mut st = self.conn.prepare_cached(
            "SELECT p.id,p.name,p.category,p.city,p.lon,p.lat FROM pois_rt r JOIN pois p ON p.id = r.id
             WHERE r.minlon <= ?2 AND r.maxlon >= ?1 AND r.minlat <= ?4 AND r.maxlat >= ?3",
        )?;
        let mut found: Vec<(f64, GeoResult)> = Vec::new();
        for radius_m in [1500.0, 5000.0, 20000.0, 80000.0, 400000.0] {
            let dlat = radius_m / 111_000.0;
            let dlon = radius_m / (111_320.0 * lat.to_radians().cos().max(0.2));
            let rows = st.query_map(params![lon - dlon, lon + dlon, lat - dlat, lat + dlat], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })?;
            found.clear();
            for row in rows {
                let (id, name, cat, city, plon, plat) = row?;
                if !categories::matches(&cat, cats) {
                    continue;
                }
                let d = haversine_m([lon, lat], [plon, plat]);
                found.push((
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
                        score: 80.0 - d / 1000.0,
                    },
                ));
            }
            if found.len() >= limit {
                break;
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(found.into_iter().take(limit).map(|(_, g)| g).collect())
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
                        let limit = 2 * (ql as u32 / 3).max(1);
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
            if !seen.insert((c.kind as u8, c.display.clone())) {
                continue;
            }
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
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------- reverse

    pub fn reverse(&self, lon: f64, lat: f64) -> Result<Option<GeoResult>> {
        let here = [lon, lat];
        let dlat = |m: f64| m / 111_000.0;
        let dlon = |m: f64| m / (111_320.0 * lat.to_radians().cos().max(0.2));

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

/// Same address from several sources (OSM with "תל אביב" and "תל אביב–יפו", data.gov.il, a
/// second street reading): equal normalised street and house number, the same city (one city
/// name may extend the other by whole words) and less than 50 m apart. Keeps the best score and
/// takes the most precise, then the longer (official) city name. Order is preserved.
/// The route number of a road query: "כביש 6", "כביש443", "דרך 1", "road 90", "route 6".
fn road_number(q: &str) -> Option<String> {
    let t = q.trim().to_lowercase();
    let rest = ["כביש", "דרך", "road", "route", "highway", "rd"].iter().find_map(|w| t.strip_prefix(w))?;
    let n = rest.trim();
    (!n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit())).then(|| n.to_string())
}

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
    let mut out: Vec<GeoResult> = Vec::with_capacity(v.len());
    for g in v {
        let dup = if g.kind == GeoKind::Address {
            let k = key(&g);
            out.iter().position(|o| {
                o.kind == GeoKind::Address
                    && key(o) == k
                    && same_city(o.city.as_deref().unwrap_or(""), g.city.as_deref().unwrap_or(""))
                    && haversine_m([o.lon, o.lat], [g.lon, g.lat]) < 50.0
            })
        } else {
            None
        };
        match dup {
            None => out.push(g),
            Some(i) => {
                let o = &mut out[i];
                let score = o.score.max(g.score);
                if pref(&g) < pref(o) {
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
        let r = parse_coord("32.0853, 34.7818").unwrap();
        assert_eq!(r.kind, GeoKind::Coord);
        assert!((r.lon - 34.7818).abs() < 1e-9);
        assert!(parse_coord("הרצל 12").is_none());
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
        }
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
