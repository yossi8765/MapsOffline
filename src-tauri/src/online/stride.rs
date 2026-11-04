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

/// Typical delay in minutes (median, rounded to 0.5) of each boarding; None when unknown.
pub fn delays(queries: &[DelayQuery]) -> Vec<Option<f64>> {
    queries
        .iter()
        .map(|q| {
            let dep = DateTime::parse_from_rfc3339(&q.depart_iso).ok()?.with_timezone(&Utc);
            let hour: u32 = dep.format("%H").to_string().parse().ok()?;
            let key = ((q.lon * 2000.0).round() as i64, (q.lat * 2000.0).round() as i64, hour);
            let mut guard = CACHE.lock().ok()?;
            let cache = guard.get_or_insert_with(HashMap::new);
            let fresh = cache.get(&key).is_some_and(|e| e.at.elapsed() < if e.ok { Duration::from_secs(86_400) } else { Duration::from_secs(600) });
            if !fresh {
                drop(guard);
                let res = fetch(q.lon, q.lat, hour);
                if let Err(e) = &res {
                    log::info!("stride: {e:#}");
                }
                let entry = match res {
                    Ok(samples) => Entry { at: Instant::now(), ok: true, samples },
                    Err(_) => Entry { at: Instant::now(), ok: false, samples: HashMap::new() },
                };
                guard = CACHE.lock().ok()?;
                guard.get_or_insert_with(HashMap::new).insert(key, entry);
            }
            let cache = guard.as_ref()?;
            let e = cache.get(&key)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
