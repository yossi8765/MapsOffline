//! Prayer times (מניינים) of synagogues from Nedarim Plus' national project ("פרויקט זמני תפילות
//! הארצי", matara.pro/nedarimplus/zmanim). Three public JSON endpoints of `Zmanim/Manage.aspx`
//! (format as used by github.com/rishumit/hebrew-maps, hmaps/etl/zmanim.py):
//!
//! * `Action=GetLocation` – every listed synagogue ("mosad"): `MosadId`, `Loc` = "lat*lon".
//! * `Action=GetMosadMinyan&MosadId=` – the weekly timetable: `T` prayer, `H` time, `D` day
//!   letters (א..ו, ש), `L` hall, `C` note. No name.
//! * `Action=GetNearestMinyan&lat=&lng=&radius=KM` – minyanim of the next 24 hours around a
//!   point, WITH the synagogue name `L`, address `A`, id `M`, location `W`. The only source of
//!   names, so the fetch sweeps it around every mosad; names not seen this time (e.g.
//!   Shabbat-only mosads) are kept from the previous snapshot.
//!
//! The snapshot (`minyanim/minyanim.json`, ~1 MB) is built weekly in CI (ready-made package) or
//! here in the update flow (~10–15 min at a polite request rate). Credit shown with the data:
//! "מקור: נדרים פלוס – פרויקט זמני תפילות הארצי".
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};

use super::download;
use super::Cancel;
use crate::paths::DataPaths;
use crate::types::{ProgressFn, ProgressPhase};

const BASE: &str = "https://www.matara.pro/nedarimplus/Zmanim/Manage.aspx";
pub const SOURCE_HE: &str = "נדרים פלוס – פרויקט זמני תפילות הארצי";
pub const SOURCE_URL: &str = "https://www.matara.pro/nedarimplus/zmanim/";
const DAY_LETTERS: [char; 7] = ['א', 'ב', 'ג', 'ד', 'ה', 'ו', 'ש'];
/// A snapshot older than this counts as stale in the data check.
pub const STALE_DAYS: i64 = 7;
/// Parallel requests and the minimum gap between two requests (be gentle with a charity's server).
const WORKERS: usize = 4;
const GAP: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Minyan {
    pub prayer: String,
    /// "HH:MM"
    pub time: String,
    /// day letters, Sunday..Shabbat: "אבגדהו", "ש"
    pub days: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hall: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mosad {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    /// [lon, lat] of every listed location (the first is the main one)
    pub locations: Vec<[f64; 2]>,
    #[serde(default)]
    pub minyanim: Vec<Minyan>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub source: String,
    pub url: String,
    /// local date of the fetch, YYYY-MM-DD
    pub fetched: String,
    pub mosads: Vec<Mosad>,
}

/// One synagogue's weekly timetable (IPC `minyanim_for`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinyanTable {
    pub mosad_id: String,
    pub name: Option<String>,
    pub address: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub distance_m: f64,
    pub minyanim: Vec<Minyan>,
    pub message: Option<String>,
    pub source: String,
    pub fetched: String,
    /// fetched from Nedarim Plus just now (online), not from the weekly snapshot
    #[serde(default)]
    pub live: bool,
}

/// An upcoming prayer nearby (IPC `minyanim_next`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextMinyan {
    pub mosad_id: String,
    pub name: Option<String>,
    pub address: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub distance_m: f64,
    pub prayer: String,
    pub time: String,
    /// minutes from now (0 = now)
    pub in_minutes: i64,
    pub hall: Option<String>,
    pub note: Option<String>,
}

// ---------------------------------------------------------------- parsing

/// "31.84*35.24" → [lon, lat] inside Israel's box.
fn parse_loc(s: &str) -> Option<[f64; 2]> {
    let (a, b) = s.split_once('*')?;
    let (lat, lon): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    ((29.0..=33.6).contains(&lat) && (34.0..=36.0).contains(&lon)).then(|| [(lon * 1e6).round() / 1e6, (lat * 1e6).round() / 1e6])
}

fn clean(s: Option<&str>) -> String {
    s.unwrap_or("").replace(['\u{200f}', '\u{200e}'], "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Campaign labels the source appends to names (" - נ.פ", " - אח לאח"…).
fn clean_name(s: Option<&str>) -> Option<String> {
    let mut n = clean(s);
    loop {
        let before = n.clone();
        for suf in [" - נ.פ", " - נ. פ", " - נפ", " - מ.ש.ג.ב", " - מ.ה", " - א.ח", " - אח לאח", "-נ.פ", "- נ.פ"] {
            if let Some(stripped) = n.strip_suffix(suf) {
                n = stripped.trim_end().to_string();
            }
        }
        if n == before {
            break;
        }
    }
    let n = n.trim_matches(|c: char| c == '-' || c.is_whitespace()).to_string();
    (!n.is_empty()).then_some(n)
}

fn norm_time(s: Option<&str>) -> Option<String> {
    let s = s?.trim();
    let (h, m) = s.split_once(':').or_else(|| s.split_once('.'))?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h <= 24 && m <= 59 && m.to_string().len() <= 2).then(|| format!("{:02}:{:02}", h % 24, m))
}

fn norm_days(s: Option<&str>) -> String {
    let s = s.unwrap_or("");
    DAY_LETTERS.iter().filter(|c| s.contains(**c)).collect()
}

fn parse_timetable(v: &serde_json::Value) -> (Vec<Minyan>, Option<String>) {
    let mut out = Vec::new();
    for r in v.get("data").and_then(|d| d.as_array()).into_iter().flatten() {
        let (Some(time), days) = (norm_time(r.get("H").and_then(|x| x.as_str())), norm_days(r.get("D").and_then(|x| x.as_str()))) else { continue };
        let prayer = clean(r.get("T").and_then(|x| x.as_str()));
        if days.is_empty() || prayer.is_empty() {
            continue;
        }
        let opt = |k: &str| Some(clean(r.get(k).and_then(|x| x.as_str()))).filter(|s| !s.is_empty());
        out.push(Minyan { prayer, time, days, hall: opt("L"), note: opt("C") });
    }
    out.sort_by(|a, b| (&a.time, &a.prayer).cmp(&(&b.time, &b.prayer)));
    let msg = Some(clean(v.get("Message").and_then(|x| x.as_str()))).filter(|s| !s.is_empty());
    (out, msg)
}

// ---------------------------------------------------------------- fetch

struct Fetcher {
    client: reqwest::blocking::Client,
    next: parking_lot::Mutex<Instant>,
    errors: AtomicUsize,
    requests: AtomicU64,
}

impl Fetcher {
    fn json(&self, params: &[(&str, String)], cancel: &Cancel) -> anyhow::Result<Option<serde_json::Value>> {
        for attempt in 0..3u64 {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            cancel.wait_if_paused();
            {
                // global pacing across the worker threads
                let mut next = self.next.lock();
                let now = Instant::now();
                if *next > now {
                    std::thread::sleep(*next - now);
                }
                *next = Instant::now() + GAP;
            }
            self.requests.fetch_add(1, Ordering::Relaxed);
            match self.client.get(BASE).query(params).send() {
                Ok(r) if r.status().as_u16() == 418 => bail!("{}", download::blocked_message(BASE)),
                Ok(r) if matches!(r.status().as_u16(), 403 | 451) => bail!("blocked ({}) – {BASE}", r.status()),
                Ok(r) if r.status().is_success() => {
                    let body = r.text().unwrap_or_default();
                    let body = body.trim_start_matches('\u{feff}').trim_start();
                    // "" = no data, "Wrong radius" …
                    if !body.starts_with('{') {
                        return Ok(None);
                    }
                    return Ok(serde_json::from_str(body).ok());
                }
                _ => std::thread::sleep(Duration::from_secs(2 + 3 * attempt)),
            }
        }
        self.errors.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
}

/// Download everything into a new snapshot (names missing now are kept from `previous`).
pub fn fetch(previous: Option<&Snapshot>, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Snapshot> {
    let f = Fetcher { client: download::http_client()?, next: parking_lot::Mutex::new(Instant::now()), errors: AtomicUsize::new(0), requests: AtomicU64::new(0) };
    progress(ProgressPhase::Downloading, 0, 0, "מוריד את רשימת בתי הכנסת (נדרים פלוס)");
    let loc = f.json(&[("Action", "GetLocation".into())], cancel)?.ok_or_else(|| anyhow!("GetLocation returned nothing – is matara.pro reachable?"))?;
    let mut mosads: Vec<Mosad> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for d in loc.get("Details").and_then(|d| d.as_array()).into_iter().flatten() {
        let id = d.get("MosadId").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        let Some(ll) = d.get("Loc").and_then(|x| x.as_str()).and_then(parse_loc) else { continue };
        // "0" is a test entry
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || id.trim_start_matches('0').is_empty() {
            continue;
        }
        let i = *index.entry(id.clone()).or_insert_with(|| {
            mosads.push(Mosad { id: id.clone(), name: None, address: None, locations: Vec::new(), minyanim: Vec::new(), message: None });
            mosads.len() - 1
        });
        if !mosads[i].locations.contains(&ll) {
            mosads[i].locations.push(ll);
        }
    }
    anyhow::ensure!(!mosads.is_empty(), "GetLocation listed no synagogues");
    let total = mosads.len() as u64;
    log::info!("minyanim: {total} mosads");

    // weekly timetables
    let done = AtomicU64::new(0);
    let tables: Vec<anyhow::Result<(Vec<Minyan>, Option<String>)>> = std::thread::scope(|s| {
        let chunks: Vec<&[Mosad]> = mosads.chunks(mosads.len().div_ceil(WORKERS)).collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let (f, done) = (&f, &done);
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|m| {
                            let v = f.json(&[("Action", "GetMosadMinyan".into()), ("MosadId", m.id.clone())], cancel)?;
                            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                            if n % 50 == 0 {
                                progress(ProgressPhase::Downloading, n, total * 2, &format!("זמני תפילות: {n} מתוך {total} בתי כנסת"));
                            }
                            Ok(v.as_ref().map(parse_timetable).unwrap_or_default())
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    for (m, t) in mosads.iter_mut().zip(tables) {
        let (list, msg) = t?;
        m.minyanim = list;
        m.message = msg;
    }

    // names: sweep "nearest minyan" around the mosads that have a timetable (first a 30 m circle,
    // which often names several neighbours at once, then a 1 m circle for the rest)
    let names: parking_lot::Mutex<HashMap<String, (String, Option<String>)>> = parking_lot::Mutex::new(HashMap::new());
    for (radius, all_locations) in [(0.03, false), (0.001, true)] {
        let todo: Vec<[f64; 2]> = mosads
            .iter()
            .filter(|m| !m.minyanim.is_empty() && !names.lock().contains_key(&m.id))
            .flat_map(|m| if all_locations { m.locations.clone() } else { m.locations.iter().take(1).copied().collect() })
            .collect();
        let n_todo = todo.len() as u64;
        let swept = AtomicU64::new(0);
        std::thread::scope(|s| -> anyhow::Result<()> {
            let handles: Vec<_> = todo
                .chunks(todo.len().div_ceil(WORKERS).max(1))
                .map(|chunk| {
                    let (f, names, swept) = (&f, &names, &swept);
                    s.spawn(move || -> anyhow::Result<()> {
                        for &[lon, lat] in chunk {
                            let v = f.json(&[("Action", "GetNearestMinyan".into()), ("lat", lat.to_string()), ("lng", lon.to_string()), ("radius", radius.to_string())], cancel)?;
                            for r in v.as_ref().and_then(|v| v.get("data")).and_then(|d| d.as_array()).into_iter().flatten() {
                                let id = r.get("M").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                                let Some(name) = clean_name(r.get("L").and_then(|x| x.as_str())) else { continue };
                                if id.is_empty() {
                                    continue;
                                }
                                let addr = Some(clean(r.get("A").and_then(|x| x.as_str()))).filter(|s| !s.is_empty());
                                names.lock().entry(id).or_insert((name, addr));
                            }
                            let k = swept.fetch_add(1, Ordering::Relaxed) + 1;
                            if k % 50 == 0 {
                                progress(ProgressPhase::Downloading, total + k * total / n_todo.max(1), total * 2, &format!("שמות בתי הכנסת: {k} מתוך {n_todo}"));
                            }
                        }
                        Ok(())
                    })
                })
                .collect();
            for h in handles {
                h.join().map_err(|_| anyhow!("fetch thread panicked"))??;
            }
            Ok(())
        })?;
    }
    let errors = f.errors.load(Ordering::Relaxed);
    if errors > 20.max(mosads.len() / 10) {
        bail!("{errors} requests to matara.pro failed – keeping the previous data");
    }
    let names = names.into_inner();
    let prev: HashMap<&str, &Mosad> = previous.map(|p| p.mosads.iter().map(|m| (m.id.as_str(), m)).collect()).unwrap_or_default();
    for m in &mut mosads {
        if let Some((n, a)) = names.get(&m.id) {
            m.name = Some(n.clone());
            m.address = a.clone();
        } else if let Some(p) = prev.get(m.id.as_str()) {
            m.name = p.name.clone();
            m.address = p.address.clone();
        }
    }
    log::info!(
        "minyanim: {} mosads, {} named, {} with a timetable, {} minyanim, {} requests, {errors} errors",
        mosads.len(),
        mosads.iter().filter(|m| m.name.is_some()).count(),
        mosads.iter().filter(|m| !m.minyanim.is_empty()).count(),
        mosads.iter().map(|m| m.minyanim.len()).sum::<usize>(),
        f.requests.load(Ordering::Relaxed)
    );
    Ok(Snapshot { source: SOURCE_HE.into(), url: SOURCE_URL.into(), fetched: now_jerusalem().format("%Y-%m-%d").to_string(), mosads })
}

pub fn load(path: &Path) -> anyhow::Result<Snapshot> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn save(path: &Path, s: &Snapshot) -> anyhow::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    crate::paths::atomic_replace(path, &tmp)?;
    Ok(())
}

/// Version string of a snapshot (manifest).
pub fn version_of(s: &Snapshot) -> String {
    format!("nedarimplus {}", s.fetched)
}

/// Is the installed snapshot older than [`STALE_DAYS`] (or missing / unreadable)?
pub fn is_stale(version: Option<&str>) -> bool {
    let Some(date) = version.and_then(|v| v.strip_prefix("nedarimplus ")).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) else { return true };
    (now_jerusalem().date_naive() - date).num_days() >= STALE_DAYS
}

/// Update-flow job: fetch and store the snapshot.
pub fn update_minyanim(paths: &DataPaths, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    let file = paths.minyanim_file();
    let previous = load(&file).ok();
    let m = super::manifest::Manifest::load(paths);
    let installed = m.entry(crate::types::DataComponent::Minyanim).and_then(|e| e.version.clone());
    if previous.is_some() && !is_stale(installed.as_deref()) {
        progress(ProgressPhase::Processing, 1, 1, "זמני התפילות כבר מעודכנים");
        return Ok(installed);
    }
    let snap = fetch(previous.as_ref(), progress, cancel)?;
    progress(ProgressPhase::Finalizing, 0, 0, "שומר את זמני התפילות");
    save(&file, &snap)?;
    invalidate_cache();
    Ok(Some(version_of(&snap)))
}

// ---------------------------------------------------------------- queries

static CACHE: parking_lot::Mutex<Option<(std::path::PathBuf, std::time::SystemTime, std::sync::Arc<Snapshot>)>> = parking_lot::Mutex::new(None);

fn invalidate_cache() {
    *CACHE.lock() = None;
}

/// The snapshot, re-read only when the file changed.
fn cached(path: &Path) -> anyhow::Result<std::sync::Arc<Snapshot>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|_| anyhow!("NO_MINYANIM_DATA"))?;
    let mut c = CACHE.lock();
    if let Some((p, t, s)) = c.as_ref() {
        if p == path && *t == mtime {
            return Ok(s.clone());
        }
    }
    let s = std::sync::Arc::new(load(path)?);
    *c = Some((path.to_path_buf(), mtime, s.clone()));
    Ok(s)
}

fn now_jerusalem() -> chrono::DateTime<chrono_tz::Tz> {
    chrono::Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem)
}

fn dist_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (la1, la2) = (a[1].to_radians(), b[1].to_radians());
    let dla = la2 - la1;
    let dlo = (b[0] - a[0]).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

/// Letters / words that do not tell synagogues apart ("בית כנסת", "ישיבת"…).
const GENERIC: &[&str] = &["בית", "כנסת", "הכנסת", "ביהכנ", "ביכנ", "בית-כנסת", "קהילת", "קהילה", "ישיבת", "ישיבה", "מדרש", "המדרש", "מנין", "מניין", "המרכזי", "מרכזי", "הגדול", "ספרדי", "אשכנזי", "תימני", "שכונתי", "synagogue", "beit", "knesset"];

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '"')
        .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_lowercase())
        .filter(|w| w.chars().count() >= 2 && !GENERIC.contains(&w.as_str()))
        .collect()
}

/// 0..1 overlap of the distinctive words of two names.
fn name_score(a: &str, b: &str) -> f64 {
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let common = wa.iter().filter(|w| wb.iter().any(|v| v == *w || (w.chars().count() >= 4 && (v.starts_with(w.as_str()) || w.starts_with(v.as_str()))))).count();
    common as f64 / wa.len().min(wb.len()) as f64
}

fn nearest_location(m: &Mosad, at: [f64; 2]) -> ([f64; 2], f64) {
    m.locations.iter().map(|&l| (l, dist_m(l, at))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or(([0.0, 0.0], f64::MAX))
}

/// The timetable of the synagogue at this point: within 60 m when the names agree, else the
/// nearest within 25 m. `poi_name` = the OSM name of the place the user clicked.
pub fn table_for(paths: &DataPaths, lon: f64, lat: f64, poi_name: Option<&str>) -> anyhow::Result<Option<MinyanTable>> {
    let snap = cached(&paths.minyanim_file())?;
    let at = [lon, lat];
    let mut best: Option<(f64, &Mosad, [f64; 2], f64)> = None;
    for m in &snap.mosads {
        if m.minyanim.is_empty() {
            continue;
        }
        let (loc, d) = nearest_location(m, at);
        if d > 60.0 {
            continue;
        }
        let ns = match (poi_name, m.name.as_deref()) {
            (Some(p), Some(n)) => name_score(p, n),
            _ => 0.0,
        };
        if ns < 0.5 && d > 25.0 {
            continue;
        }
        // name agreement outweighs a few metres
        let rank = d - ns * 40.0;
        if best.as_ref().is_none_or(|b| rank < b.0) {
            best = Some((rank, m, loc, d));
        }
    }
    Ok(best.map(|(_, m, loc, d)| MinyanTable {
        mosad_id: m.id.clone(),
        name: m.name.clone(),
        address: m.address.clone(),
        lon: loc[0],
        lat: loc[1],
        distance_m: d.round(),
        minyanim: m.minyanim.clone(),
        message: m.message.clone(),
        source: snap.source.clone(),
        fetched: snap.fetched.clone(),
        live: false,
    }))
}

/// Day letter of a chrono weekday (Sunday = א).
fn day_letter(wd: chrono::Weekday) -> char {
    DAY_LETTERS[wd.num_days_from_sunday() as usize]
}

/// Prayers starting in the next `within_min` minutes (default 180) within `radius_m`, soonest
/// first (then nearest). At most 40.
pub fn next_near(paths: &DataPaths, lon: f64, lat: f64, radius_m: f64, within_min: Option<i64>) -> anyhow::Result<Vec<NextMinyan>> {
    let snap = cached(&paths.minyanim_file())?;
    let now = now_jerusalem();
    Ok(next_from(&snap, [lon, lat], radius_m, within_min.unwrap_or(180), now.weekday(), (now.hour() * 60 + now.minute()) as i64))
}

fn next_from(snap: &Snapshot, at: [f64; 2], radius_m: f64, within: i64, wd: chrono::Weekday, now_min: i64) -> Vec<NextMinyan> {
    let today = day_letter(wd);
    let tomorrow = day_letter(wd.succ());
    let mut out = Vec::new();
    for m in &snap.mosads {
        let (loc, d) = nearest_location(m, at);
        if d > radius_m {
            continue;
        }
        for x in &m.minyanim {
            let Some((h, mi)) = x.time.split_once(':') else { continue };
            let t = h.parse::<i64>().unwrap_or(0) * 60 + mi.parse::<i64>().unwrap_or(0);
            // today later than now, or tomorrow early (across midnight)
            let delta = if x.days.contains(today) && t >= now_min { t - now_min } else if x.days.contains(tomorrow) && t + 1440 - now_min <= within { t + 1440 - now_min } else { continue };
            if delta > within {
                continue;
            }
            out.push(NextMinyan {
                mosad_id: m.id.clone(),
                name: m.name.clone(),
                address: m.address.clone(),
                lon: loc[0],
                lat: loc[1],
                distance_m: d.round(),
                prayer: x.prayer.clone(),
                time: x.time.clone(),
                in_minutes: delta,
                hall: x.hall.clone(),
                note: x.note.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.in_minutes.cmp(&b.in_minutes).then(a.distance_m.total_cmp(&b.distance_m)));
    out.truncate(40);
    out
}

// ---------------------------------------------------------------- live (online)

/// One request with a short timeout (the caller falls back to the snapshot on any error). The
/// same public endpoint KioskLock's prayer-times add-on reads (`GetMosadMinyan&MosadId=`).
fn live_json(params: &[(&str, String)]) -> anyhow::Result<Option<serde_json::Value>> {
    let client = download::http_client()?;
    let r = client.get(BASE).query(params).timeout(Duration::from_secs(8)).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(BASE));
    }
    let body = r.error_for_status()?.text()?;
    let body = body.trim_start_matches('\u{feff}').trim_start();
    Ok(if body.starts_with('{') { serde_json::from_str(body).ok() } else { None })
}

/// A row of `GetNearestMinyan` (prayers of the next 24 hours around a point).
struct NearRow {
    id: String,
    name: Option<String>,
    address: Option<String>,
    loc: [f64; 2],
    prayer: String,
    time: String,
    hall: Option<String>,
}

fn live_nearest(lon: f64, lat: f64, radius_km: f64) -> anyhow::Result<Vec<NearRow>> {
    let v = live_json(&[("Action", "GetNearestMinyan".into()), ("lat", format!("{lat:.6}")), ("lng", format!("{lon:.6}")), ("radius", format!("{radius_km:.3}"))])?;
    let mut out = Vec::new();
    for r in v.as_ref().and_then(|v| v.get("data")).and_then(|d| d.as_array()).into_iter().flatten() {
        let s = |k: &str| r.get(k).and_then(|x| x.as_str());
        let id = s("M").unwrap_or("").trim().to_string();
        let (Some(loc), Some(time)) = (s("W").and_then(parse_loc), norm_time(s("H"))) else { continue };
        let prayer = clean(s("T"));
        if id.is_empty() || prayer.is_empty() {
            continue;
        }
        out.push(NearRow {
            id,
            name: clean_name(s("L")),
            address: Some(clean(s("A"))).filter(|x| !x.is_empty()),
            loc,
            prayer,
            time,
            // in this answer C is the hall ("בית המדרש", "עזרת נשים")
            hall: Some(clean(s("C"))).filter(|x| !x.is_empty()),
        });
    }
    Ok(out)
}

/// The synagogue's current weekly timetable from Nedarim Plus. The synagogue is the one the
/// snapshot matches here, else the one a live search around the point finds (same rule as
/// [`table_for`]: within 60 m when the names agree, else the nearest within 25 m).
pub fn table_live(paths: &DataPaths, lon: f64, lat: f64, poi_name: Option<&str>) -> anyhow::Result<Option<MinyanTable>> {
    let at = [lon, lat];
    let known = table_for(paths, lon, lat, poi_name).ok().flatten();
    let (id, name, address, loc, d) = match known {
        Some(t) => (t.mosad_id, t.name, t.address, [t.lon, t.lat], t.distance_m),
        None => {
            let mut best: Option<(f64, NearRow, f64)> = None;
            for r in live_nearest(lon, lat, 0.1)? {
                let d = dist_m(r.loc, at);
                let ns = match (poi_name, r.name.as_deref()) {
                    (Some(p), Some(n)) => name_score(p, n),
                    _ => 0.0,
                };
                if d > 60.0 || (ns < 0.5 && d > 25.0) {
                    continue;
                }
                let rank = d - ns * 40.0;
                if best.as_ref().is_none_or(|b| rank < b.0) {
                    best = Some((rank, r, d));
                }
            }
            let Some((_, r, d)) = best else { return Ok(None) };
            (r.id, r.name, r.address, r.loc, d.round())
        }
    };
    let v = live_json(&[("Action", "GetMosadMinyan".into()), ("MosadId", id.clone())])?.ok_or_else(|| anyhow!("no timetable"))?;
    let (minyanim, message) = parse_timetable(&v);
    Ok(Some(MinyanTable {
        mosad_id: id,
        name,
        address,
        lon: loc[0],
        lat: loc[1],
        distance_m: d,
        minyanim,
        message,
        source: SOURCE_HE.into(),
        fetched: now_jerusalem().format("%Y-%m-%d %H:%M").to_string(),
        live: true,
    }))
}

/// Prayers starting within `within_min` minutes inside `radius_m`, live from Nedarim Plus.
pub fn next_live(lon: f64, lat: f64, radius_m: f64, within_min: Option<i64>) -> anyhow::Result<Vec<NextMinyan>> {
    let now = now_jerusalem();
    let now_min = (now.hour() * 60 + now.minute()) as i64;
    let within = within_min.unwrap_or(180);
    let mut out: Vec<NextMinyan> = live_nearest(lon, lat, radius_m / 1000.0)?
        .into_iter()
        .filter_map(|r| {
            let (h, m) = r.time.split_once(':')?;
            let t = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
            // the answer covers the next 24 hours: an earlier clock time is tomorrow
            let delta = if t >= now_min { t - now_min } else { t + 1440 - now_min };
            let d = dist_m(r.loc, [lon, lat]);
            (delta <= within && d <= radius_m).then(|| NextMinyan {
                mosad_id: r.id,
                name: r.name,
                address: r.address,
                lon: r.loc[0],
                lat: r.loc[1],
                distance_m: d.round(),
                prayer: r.prayer,
                time: r.time,
                in_minutes: delta,
                hall: r.hall,
                note: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.in_minutes.cmp(&b.in_minutes).then(a.distance_m.total_cmp(&b.distance_m)));
    out.truncate(40);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let m = |id: &str, name: &str, at: [f64; 2], list: Vec<(&str, &str, &str)>| Mosad {
            id: id.into(),
            name: Some(name.into()),
            address: None,
            locations: vec![at],
            minyanim: list.into_iter().map(|(p, t, d)| Minyan { prayer: p.into(), time: t.into(), days: d.into(), hall: None, note: None }).collect(),
            message: None,
        };
        Snapshot {
            source: SOURCE_HE.into(),
            url: SOURCE_URL.into(),
            fetched: "2026-11-04".into(),
            mosads: vec![
                m("1", "חניכי הישיבות נוה יעקב", [35.24326, 31.84180], vec![("שחרית", "06:15", "אבגדהו"), ("מנחה", "13:30", "אבגדה"), ("ערבית", "20:00", "אבגדהוש")]),
                m("2", "אוהל יצחק", [35.24400, 31.84200], vec![("מנחה", "13:20", "אבגדהו")]),
            ],
        }
    }

    /// Live Nedarim Plus (network): `cargo test --lib live_lookup -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_lookup() {
        let paths = DataPaths::new(std::env::temp_dir().join("mo-live-minyan-none"));
        let t = table_live(&paths, 34.89827, 31.93375, Some("בית המדרש הגדול חניכי הישיבות")).unwrap().expect("found");
        eprintln!("{:?} {} minyanim", t.name, t.minyanim.len());
        assert!(t.live && !t.minyanim.is_empty());
        let n = next_live(34.89827, 31.93375, 600.0, Some(1440)).unwrap();
        eprintln!("next: {:?}", n.iter().take(3).map(|m| (&m.time, &m.prayer, &m.name)).collect::<Vec<_>>());
        assert!(!n.is_empty());
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_loc("31.8417964*35.2432599"), Some([35.24326, 31.841796]));
        assert_eq!(parse_loc("0*0"), None);
        assert_eq!(norm_time(Some("5:13")).as_deref(), Some("05:13"));
        assert_eq!(norm_time(Some("24:10")).as_deref(), Some("00:10"));
        assert_eq!(norm_time(Some("x")), None);
        assert_eq!(norm_days(Some("שאבג")), "אבגש");
        assert_eq!(clean_name(Some("חניכי הישיבות - נ.פ")).as_deref(), Some("חניכי הישיבות"));
        let v: serde_json::Value = serde_json::from_str(r#"{"data":[{"T":"שחרית","L":"בית המדרש","H":"05:33","D":"אבגדהו","C":"25 דקות לפני הנץ"},{"T":"","H":"7:00","D":"א"}],"Message":""}"#).unwrap();
        let (list, msg) = parse_timetable(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].hall.as_deref(), Some("בית המדרש"));
        assert_eq!(msg, None);
    }

    #[test]
    fn next_prayers_sorted_and_across_midnight() {
        let s = snap();
        // Monday 13:00: מנחה 13:20 (2) before 13:30 (1)
        let n = next_from(&s, [35.2433, 31.8418], 500.0, 180, chrono::Weekday::Mon, 13 * 60);
        assert_eq!(n.iter().map(|x| (x.mosad_id.as_str(), x.time.as_str())).collect::<Vec<_>>(), vec![("2", "13:20"), ("1", "13:30")]);
        assert_eq!(n[0].in_minutes, 20);
        // Monday 23:00, window 8 h: Tuesday's 06:15 shacharit is 435 minutes away
        let n = next_from(&s, [35.2433, 31.8418], 500.0, 480, chrono::Weekday::Mon, 23 * 60);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].in_minutes, 435);
        // Saturday: only ערבית is on Shabbat
        let n = next_from(&s, [35.2433, 31.8418], 500.0, 600, chrono::Weekday::Sat, 12 * 60);
        assert_eq!(n.iter().map(|x| x.prayer.as_str()).collect::<Vec<_>>(), vec!["ערבית"]);
    }

    /// MO_DATA=<data dir with minyanim/minyanim.json> cargo test real_minyanim -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_minyanim() {
        let paths = DataPaths::new(std::env::var("MO_DATA").expect("MO_DATA"));
        // Chanichei HaYeshivot, Neve Yaakov (Jerusalem)
        let t = table_for(&paths, 35.24326, 31.84180, Some("חניכי הישיבות")).unwrap().expect("a timetable");
        eprintln!("{} {:?} {} minyanim, {} m", t.mosad_id, t.name, t.minyanim.len(), t.distance_m);
        assert_eq!(t.mosad_id, "5772698");
        let n = next_near(&paths, 35.2137, 31.7683, 1500.0, Some(24 * 60)).unwrap();
        eprintln!("{} upcoming near Jerusalem centre; first: {:?}", n.len(), n.first().map(|x| (&x.name, &x.prayer, &x.time, x.in_minutes, x.distance_m)));
        assert!(!n.is_empty());
        assert!(n.windows(2).all(|w| w[0].in_minutes <= w[1].in_minutes));
    }

    #[test]
    fn names_decide_between_neighbours() {
        assert!(name_score("בית כנסת אוהל יצחק", "אוהל יצחק") > 0.9);
        assert!(name_score("בית כנסת אוהל יצחק", "חניכי הישיבות נוה יעקב") < 0.1);
        assert!(is_stale(None));
        assert!(is_stale(Some("nedarimplus 2020-01-01")));
    }
}
