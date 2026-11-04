//! "How late is this line here, usually?" from Open Bus Stride (הסדנא לידע ציבורי), the public
//! archive of the Ministry of Transport's SIRI real-time feed. Optional and online only.
//!
//! One query per boarding stop (`siri_ride_stops/list`): every ride recorded near the stop on the
//! last 7 days within ±1 h of the planned departure's time of day, all lines at once (the API
//! cannot filter by line there). For each ride the delay is the time the bus was nearest to the
//! stop minus the planned (GTFS) arrival. Only samples where the bus passed within 60 m and the
//! delay is within −10…+30 min count (the nearest SIRI position is sometimes from another pass);
//! a median is reported only with at least 6 samples of the line.
//!
//! The API answers in 10–15 s, so this never runs inside route planning: the UI asks for it
//! after the routes are shown (`transit_delays` command). Results are cached per (stop,
//! hour) for a day; failures (offline, timeout) are cached for 10 minutes and return None.
//! Data: Open Bus Stride, https://open-bus-stride-api.hasadna.org.il (public API, attribution
//! "נתוני דיוק: Open Bus Stride, הסדנא לידע ציבורי").

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Duration as CDuration, Utc};
use serde::Deserialize;

pub const ATTRIBUTION: &str = "נתוני דיוק: Open Bus Stride, הסדנא לידע ציבורי";
const BASE: &str = "https://open-bus-stride-api.hasadna.org.il";
const MIN_SAMPLES: usize = 6;
const MAX_NEAR_M: f64 = 60.0;

/// One boarding the UI asks about.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelayQuery {
    pub short_name: String,
    pub agency: String,
    pub lon: f64,
    pub lat: f64,
    /// Planned departure (RFC 3339).
    pub depart_iso: String,
}

#[derive(Debug, Deserialize)]
struct RideStop {
    #[serde(rename = "nearest_siri_vehicle_location__recorded_at_time")]
    seen_at: Option<String>,
    #[serde(rename = "nearest_siri_vehicle_location__distance_from_siri_ride_stop_meters")]
    seen_dist_m: Option<f64>,
    #[serde(rename = "gtfs_ride_stop__arrival_time")]
    planned: Option<String>,
    #[serde(rename = "gtfs_route__route_short_name")]
    line: Option<String>,
    #[serde(rename = "gtfs_route__agency_name")]
    agency: Option<String>,
}

/// (line, agency) -> delay samples in minutes, per (stop cell, UTC hour).
type Samples = HashMap<(String, String), Vec<f64>>;

struct Entry {
    at: Instant,
    ok: bool,
    samples: Samples,
}

static CACHE: Mutex<Option<HashMap<(i64, i64, u32), Entry>>> = Mutex::new(None);

fn client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(super::USER_AGENT)
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(25))
        .build()?)
}

fn fetch(lon: f64, lat: f64, hour_utc: u32) -> anyhow::Result<Samples> {
    let now = Utc::now();
    let (d0, d1) = ((now - CDuration::days(7)).date_naive(), (now - CDuration::days(1)).date_naive());
    let (dlat, dlon) = (0.0005, 0.0006); // ~55 m box around the stop
    let url = format!(
        "{BASE}/siri_ride_stops/list?limit=2000&gtfs_stop__lat__greater_or_equal={:.6}&gtfs_stop__lat__lower_or_equal={:.6}&gtfs_stop__lon__greater_or_equal={:.6}&gtfs_stop__lon__lower_or_equal={:.6}&gtfs_date_from={d0}&gtfs_date_to={d1}",
        lat - dlat,
        lat + dlat,
        lon - dlon,
        lon + dlon
    );
    let rows: Vec<RideStop> = client()?.get(&url).send()?.error_for_status()?.json()?;
    let mut out: Samples = HashMap::new();
    for r in rows {
        let (Some(at), Some(plan), Some(dist)) = (
            r.seen_at.as_deref(),
            r.planned.as_deref(),
            r.seen_dist_m,
        ) else {
            continue;
        };
        if dist > MAX_NEAR_M {
            continue;
        }
        let (Ok(at), Ok(plan)) = (DateTime::parse_from_rfc3339(at), DateTime::parse_from_rfc3339(plan)) else { continue };
        // same time of day (±1 h) as the planned departure
        let h = plan.with_timezone(&Utc).format("%H").to_string().parse::<i32>().unwrap_or(-9);
        if (h - hour_utc as i32).rem_euclid(24).min((hour_utc as i32 - h).rem_euclid(24)) > 1 {
            continue;
        }
        let delay = (at - plan).num_seconds() as f64 / 60.0;
        if !(-10.0..=30.0).contains(&delay) {
            continue;
        }
        let key = (r.line.unwrap_or_default(), r.agency.unwrap_or_default());
        out.entry(key).or_default().push(delay);
    }
    Ok(out)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

type CacheKey = (i64, i64, u32);

fn cache_key(q: &DelayQuery) -> Option<CacheKey> {
    let dep = DateTime::parse_from_rfc3339(&q.depart_iso).ok()?.with_timezone(&Utc);
    let hour: u32 = dep.format("%H").to_string().parse().ok()?;
    Some(((q.lon * 2000.0).round() as i64, (q.lat * 2000.0).round() as i64, hour))
}

/// At most this many Stride requests run at once.
const PARALLEL: usize = 4;

/// Typical delay in minutes (median, rounded to 0.5) of each boarding; None when unknown.
/// The stops not in the cache are fetched in parallel (≤ 4 at a time, each stop/hour once).
pub fn delays(queries: &[DelayQuery]) -> Vec<Option<f64>> {
    let keys: Vec<Option<CacheKey>> = queries.iter().map(cache_key).collect();
    let mut missing: Vec<(CacheKey, f64, f64)> = Vec::new();
    {
        let Ok(mut guard) = CACHE.lock() else { return vec![None; queries.len()] };
        let cache = guard.get_or_insert_with(HashMap::new);
        for (q, k) in queries.iter().zip(&keys) {
            let Some(k) = *k else { continue };
            let fresh = cache.get(&k).is_some_and(|e| e.at.elapsed() < if e.ok { Duration::from_secs(86_400) } else { Duration::from_secs(600) });
            if !fresh && !missing.iter().any(|m| m.0 == k) {
                missing.push((k, q.lon, q.lat));
            }
        }
    }
    for chunk in missing.chunks(PARALLEL) {
        let fetched: Vec<(CacheKey, Entry)> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|&(k, lon, lat)| {
                    s.spawn(move || {
                        let res = fetch(lon, lat, k.2);
                        if let Err(e) = &res {
                            log::info!("stride: {e:#}");
                        }
                        let entry = match res {
                            Ok(samples) => Entry { at: Instant::now(), ok: true, samples },
                            Err(_) => Entry { at: Instant::now(), ok: false, samples: HashMap::new() },
                        };
                        (k, entry)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        if let Ok(mut guard) = CACHE.lock() {
            let cache = guard.get_or_insert_with(HashMap::new);
            for (k, e) in fetched {
                cache.insert(k, e);
            }
        }
    }
    let Ok(guard) = CACHE.lock() else { return vec![None; queries.len()] };
    queries
        .iter()
        .zip(&keys)
        .map(|(q, k)| {
            let cache = guard.as_ref()?;
            let e = cache.get(&(*k)?)?;
            let mut v = e
                .samples
                .iter()
                .filter(|((line, agency), _)| *line == q.short_name && (q.agency.is_empty() || agency.contains(&q.agency) || q.agency.contains(agency.as_str())))
                .flat_map(|(_, v)| v.iter().copied())
                .collect::<Vec<f64>>();
            if v.len() < MIN_SAMPLES {
                return None;
            }
            Some((median(&mut v) * 2.0).round() / 2.0)
        })
        .collect()
}

// ---------------------------------------------------------------- vehicle positions

#[derive(Debug, Deserialize)]
struct GtfsRouteRow {
    line_ref: Option<i64>,
    agency_name: Option<String>,
    route_long_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VehicleRow {
    lon: Option<f64>,
    lat: Option<f64>,
    bearing: Option<f64>,
    recorded_at_time: Option<String>,
    #[serde(rename = "siri_ride__id")]
    ride: Option<i64>,
}

/// Recent positions are at most this old (Stride lags real time by ~10–15 minutes).
const VEHICLE_MAX_AGE_MIN: i64 = 25;

static VEHICLE_CACHE: Mutex<Option<HashMap<(String, String), (Instant, Vec<crate::types::TransitVehiclePos>)>>> = Mutex::new(None);

fn quick_client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(super::USER_AGENT)
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()?)
}

/// Destination of an Israeli route_long_name ("A-town<->B stop-B town-1#" → "B stop-B town").
fn destination_of(long_name: &str) -> Option<String> {
    let (_, b) = long_name.split_once("<->")?;
    let b = b.trim().trim_end_matches('#');
    let b = match b.rsplit_once('-') {
        Some((head, tail)) if tail.chars().next().is_some_and(|c| c.is_ascii_digit()) => head,
        _ => b,
    };
    Some(b.trim().to_string()).filter(|s| !s.is_empty())
}

fn fetch_vehicles(short_name: &str, agency: &str) -> anyhow::Result<Vec<crate::types::TransitVehiclePos>> {
    let c = quick_client()?;
    let today = Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).date_naive();
    let routes: Vec<GtfsRouteRow> = c
        .get(format!("{BASE}/gtfs_routes/list"))
        .query(&[("route_short_name", short_name), ("date_from", &today.to_string()), ("date_to", &today.to_string()), ("limit", "100")])
        .send()?
        .error_for_status()?
        .json()?;
    let lines: Vec<(i64, Option<String>)> = routes
        .into_iter()
        .filter(|r| agency.is_empty() || r.agency_name.as_deref().is_some_and(|a| a == agency || a.contains(agency) || agency.contains(a)))
        .filter_map(|r| Some((r.line_ref?, r.route_long_name.as_deref().and_then(destination_of))))
        .take(8)
        .collect();
    let now = Utc::now();
    let from = (now - CDuration::minutes(VEHICLE_MAX_AGE_MIN)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let to = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut out = Vec::new();
    for chunk in lines.chunks(PARALLEL) {
        let res: Vec<anyhow::Result<(Vec<VehicleRow>, Option<String>)>> = std::thread::scope(|s| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|(line, head)| {
                    let (c, from, to) = (&c, &from, &to);
                    s.spawn(move || -> anyhow::Result<(Vec<VehicleRow>, Option<String>)> {
                        let rows: Vec<VehicleRow> = c
                            .get(format!("{BASE}/siri_vehicle_locations/list"))
                            .query(&[
                                ("siri_routes__line_ref", line.to_string().as_str()),
                                ("recorded_at_time_from", from.as_str()),
                                ("recorded_at_time_to", to.as_str()),
                                ("order_by", "recorded_at_time desc"),
                                ("limit", "500"),
                            ])
                            .send()?
                            .error_for_status()?
                            .json()?;
                        Ok((rows, head.clone()))
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("stride thread failed")))).collect()
        });
        for r in res {
            let (rows, head) = r?;
            // newest position of every ride (rows come newest first)
            let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
            for v in rows {
                let (Some(lon), Some(lat), Some(at)) = (v.lon, v.lat, v.recorded_at_time) else { continue };
                if v.ride.is_some_and(|id| !seen.insert(id)) {
                    continue;
                }
                let Ok(t) = DateTime::parse_from_rfc3339(&at) else { continue };
                if (now - t.with_timezone(&Utc)).num_minutes() > VEHICLE_MAX_AGE_MIN {
                    continue;
                }
                out.push(crate::types::TransitVehiclePos {
                    lon,
                    lat,
                    bearing: v.bearing,
                    headsign: head.clone(),
                    updated_iso: t.with_timezone(&chrono_tz::Asia::Jerusalem).format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// Positions of the vehicles of a line (by short name and agency) from Open Bus Stride, cached
/// for 30 s; empty on any failure.
pub fn vehicles(short_name: &str, agency: &str) -> Vec<crate::types::TransitVehiclePos> {
    let key = (short_name.to_string(), agency.to_string());
    if let Ok(g) = VEHICLE_CACHE.lock() {
        if let Some((at, v)) = g.as_ref().and_then(|m| m.get(&key)) {
            if at.elapsed() < Duration::from_secs(30) {
                return v.clone();
            }
        }
    }
    let v = match fetch_vehicles(short_name, agency) {
        Ok(v) => v,
        Err(e) => {
            log::info!("stride vehicles {short_name}/{agency}: {e:#}");
            Vec::new()
        }
    };
    if let Ok(mut g) = VEHICLE_CACHE.lock() {
        g.get_or_insert_with(HashMap::new).insert(key, (Instant::now(), v.clone()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_from_long_name() {
        assert_eq!(destination_of("הכובשים/דניאל-תל אביב יפו<->מסוף עמידר-רמת גן-10").as_deref(), Some("מסוף עמידר-רמת גן"));
        assert_eq!(destination_of("עוקף דיר חנא ב-דייר חנא<->ת.רכבת כרמיאל/דרור-כרמיאל-1#").as_deref(), Some("ת.רכבת כרמיאל/דרור-כרמיאל"));
        assert_eq!(destination_of("Line 5"), None);
    }

    /// Live API (cargo test -- --ignored vehicles_live): Dan 61.
    #[test]
    #[ignore]
    fn vehicles_live() {
        let t = Instant::now();
        let v = vehicles("61", "דן");
        eprintln!("{} vehicles in {:?}: {:?}", v.len(), t.elapsed(), v.first());
    }

    #[test]
    fn median_even_odd() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
    }

    /// Live API (cargo test -- --ignored stride_live): line 61 of Dan at Ibn Gabirol/Arlozorov.
    #[test]
    #[ignore]
    fn stride_live() {
        let q = DelayQuery { short_name: "61".into(), agency: "דן".into(), lon: 34.781789, lat: 32.084898, depart_iso: "2026-11-05T08:30:00+02:00".into() };
        let t = Instant::now();
        let r = delays(&[q]);
        eprintln!("{r:?} in {:?}", t.elapsed());
    }
}
