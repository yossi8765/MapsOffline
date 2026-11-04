//! Estimated fare of a public-transport route: Rav-Kav "ערך צבור" single-ride prices of the
//! distance-band reform ("רפורמת צדק תחבורתי").
//!
//! A ride costs by the air-line distance between where you board and where you get off. Inside
//! the 15 km band, buses / light rail / Carmelit / cable car include free transfers for 90 minutes
//! from the first validation (one ticket while the whole group stays within 15 km of the first
//! boarding). The train has its own table; consecutive train legs are one train ride. Monthly or
//! daily passes and discounts (students, seniors) are not considered – the app shows the price as
//! an estimate.
//!
//! Table: Ministry of Transport price list (static.hopon.co.il/mot/ravPassPrices.html), checked
//! 2026-11-04; identical to the table hebrew-maps (hmaps/transit/fares.py) ships as "2026-06".
//! Update `TABLE` / `UPDATED` when the ministry changes prices (once or twice a year).

use chrono::DateTime;

use crate::geo::haversine_m;
use crate::types::{LegKind, Route, TransitVehicle};

pub const UPDATED: &str = "2026-11";
/// (max km, bus / light rail ₪, train ₪ or None)
const TABLE: [(f64, f64, Option<f64>); 6] = [
    (15.0, 8.0, Some(11.5)),
    (40.0, 14.5, Some(21.0)),
    (75.0, 19.0, Some(27.0)),
    (120.0, 19.0, Some(30.5)),
    (225.0, 30.5, Some(52.5)),
    (f64::INFINITY, 87.32, None),
];
const TRANSFER_MINUTES: f64 = 90.0;
const FREE_TRANSFER_KM: f64 = 15.0;

/// Price of one ride of `km` air-line kilometres (None: no train fare that far).
pub fn band(km: f64, train: bool) -> Option<f64> {
    TABLE.iter().find(|(limit, _, _)| km <= *limit).and_then(|(_, bus, rail)| if train { *rail } else { Some(*bus) })
}

struct Ride {
    train: bool,
    from: [f64; 2],
    to: [f64; 2],
    depart_min: f64,
    line: String,
}

fn minutes(iso: &str) -> f64 {
    DateTime::parse_from_rfc3339(iso).map(|d| d.timestamp() as f64 / 60.0).unwrap_or(f64::NAN)
}

fn km(a: [f64; 2], b: [f64; 2]) -> f64 {
    haversine_m(a, b) / 1000.0
}

/// (total ₪, note) for a transit route; None for a walk-only route or an unpriced ride.
pub fn estimate(r: &Route) -> Option<(f64, String)> {
    let rides: Vec<Ride> = r
        .legs
        .iter()
        .filter(|l| l.kind == LegKind::Transit)
        .filter_map(|l| {
            let t = l.transit.as_ref()?;
            let (first, last) = (t.stops.first()?, t.stops.last()?);
            Some(Ride {
                train: t.vehicle == TransitVehicle::Rail,
                from: [first.lon, first.lat],
                to: [last.lon, last.lat],
                depart_min: minutes(&t.depart_iso),
                line: if t.route_short_name.is_empty() { t.agency.clone() } else { t.route_short_name.clone() },
            })
        })
        .collect();
    if rides.is_empty() {
        return None;
    }
    // group rides that share one ticket
    let mut groups: Vec<Vec<&Ride>> = Vec::new();
    for ride in &rides {
        if let Some(g) = groups.last_mut() {
            let prev = g[g.len() - 1];
            if ride.train && prev.train {
                g.push(ride);
                continue;
            }
            if !ride.train && !prev.train {
                let first = g[0];
                let within = km(first.from, ride.to) <= FREE_TRANSFER_KM && km(first.from, prev.to) <= FREE_TRANSFER_KM;
                let in_time = ride.depart_min - first.depart_min <= TRANSFER_MINUTES;
                if within && in_time {
                    g.push(ride);
                    continue;
                }
            }
        }
        groups.push(vec![ride]);
    }
    let mut total = 0.0;
    let mut parts = Vec::new();
    for g in &groups {
        let train = g[0].train;
        let price = band(km(g[0].from, g[g.len() - 1].to), train)?;
        total += price;
        let mut lines: Vec<&str> = Vec::new();
        for r in g {
            if !lines.contains(&r.line.as_str()) {
                lines.push(&r.line);
            }
        }
        let free = if g.len() > 1 && !train { " (כולל מעבר)" } else { "" };
        parts.push(format!("{} · {} ₪{free}", lines.join(" + "), fmt(price)));
    }
    Some(((total * 100.0).round() / 100.0, parts.join("; ")))
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Fill `fare_estimate_ils` / `fare_note` on every transit route.
pub fn annotate(routes: &mut [Route]) {
    for r in routes {
        if let Some((total, note)) = estimate(r) {
            r.fare_estimate_ils = Some(total);
            r.fare_note = Some(note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Leg, Mode, TransitLegInfo, TransitStopTime};

    const TLV: [f64; 2] = [34.78, 32.08];
    const RG: [f64; 2] = [34.82, 32.07]; // ~4 km
    const JLM: [f64; 2] = [35.21, 31.78]; // ~55 km

    fn stop(p: [f64; 2]) -> TransitStopTime {
        TransitStopTime { name: String::new(), code: None, lon: p[0], lat: p[1], time_iso: String::new() }
    }

    fn ride(v: TransitVehicle, a: [f64; 2], b: [f64; 2], dep: &str, line: &str) -> Leg {
        Leg {
            kind: LegKind::Transit,
            duration_sec: 0.0,
            distance_m: 0.0,
            geometry: vec![a, b],
            depart_iso: None,
            arrive_iso: None,
            steps: vec![],
            transit: Some(TransitLegInfo {
                vehicle: v,
                route_short_name: line.into(),
                route_long_name: String::new(),
                agency: String::new(),
                headsign: String::new(),
                color: None,
                from_stop: String::new(),
                to_stop: String::new(),
                depart_iso: format!("2026-11-03T{dep}:00+02:00"),
                arrive_iso: String::new(),
                num_stops: 1,
                stops: vec![stop(a), stop(b)],
                delay_p50_min: None,
            }),
        }
    }

    fn walk() -> Leg {
        crate::transit::straight_walk(TLV, TLV)
    }

    fn route(legs: Vec<Leg>) -> Route {
        Route {
            id: String::new(),
            mode: Mode::Transit,
            duration_sec: 0.0,
            distance_m: 0.0,
            geometry: vec![],
            legs,
            tags: vec![],
            depart_iso: None,
            arrive_iso: None,
            transfers: 0,
            walk_m: 0.0,
            fare_estimate_ils: None,
            fare_note: None,
            avoid_violations: vec![],
        }
    }

    #[test]
    fn bands() {
        assert_eq!(band(3.0, false), Some(8.0));
        assert_eq!(band(30.0, false), Some(14.5));
        assert_eq!(band(30.0, true), Some(21.0));
        assert_eq!(band(100.0, true), Some(30.5));
        assert_eq!(band(500.0, false), Some(87.32));
        assert_eq!(band(500.0, true), None);
    }

    #[test]
    fn urban_transfer_is_free() {
        let r = route(vec![ride(TransitVehicle::Bus, TLV, RG, "08:00", "1"), walk(), ride(TransitVehicle::LightRail, RG, TLV, "08:40", "R")]);
        let (total, note) = estimate(&r).unwrap();
        assert_eq!(total, 8.0);
        assert!(note.contains("כולל מעבר"), "{note}");
    }

    #[test]
    fn after_90_minutes_a_new_ticket() {
        let r = route(vec![ride(TransitVehicle::Bus, TLV, RG, "08:00", "1"), ride(TransitVehicle::Bus, RG, TLV, "09:45", "2")]);
        assert_eq!(estimate(&r).unwrap().0, 16.0);
    }

    #[test]
    fn train_plus_bus() {
        let r = route(vec![ride(TransitVehicle::Rail, TLV, JLM, "08:00", ""), ride(TransitVehicle::Bus, JLM, [35.22, 31.77], "08:50", "19")]);
        assert_eq!(estimate(&r).unwrap().0, 27.0 + 8.0);
    }

    #[test]
    fn consecutive_trains_are_one_ride() {
        let mid = [34.95, 31.95];
        let r = route(vec![ride(TransitVehicle::Rail, TLV, mid, "08:00", ""), ride(TransitVehicle::Rail, mid, JLM, "08:30", "")]);
        assert_eq!(estimate(&r).unwrap().0, 27.0);
    }

    #[test]
    fn walk_only_has_no_fare() {
        assert!(estimate(&route(vec![walk()])).is_none());
    }
}
