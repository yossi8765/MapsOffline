//! Journey planning: time resolution, access/egress, RAPTOR runs, assembly into `types::Route`.
//!
//! Alternatives: the first RAPTOR run yields the Pareto set over (arrival, transfers). Further
//! runs shift the query time (depart-at: just after the earliest found departure; arrive-by: just
//! before the latest found arrival) and one run bans the routes of the best journey, giving
//! genuinely different options. Journeys are then de-duplicated by their sequence of routes,
//! dominated ones removed, and tagged.

use std::time::Instant;

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Asia::Jerusalem;
use chrono_tz::Tz;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use super::model::{Transit, MODE_BUS, NONE};
use super::raptor::{self, JLeg, Journey, Query, DAY};
use crate::geo::{haversine_m, polyline_length_m};
use crate::transit::WalkRouter;
use crate::types::{
    Leg, LegKind, LonLat, Mode, Route, RouteRequest, RouteResponse, RouteTag, TimeKind, TimeSpec, TransitLegInfo,
    TransitStopTime,
};

pub const ACCESS_RADIUS_M: f64 = 1200.0;
pub const ACCESS_MAX_STOPS: usize = 20;
/// Extra rail / light-rail stops within the radius beyond the nearest 20.
pub const ACCESS_EXTRA_RAIL: usize = 10;
pub const PURE_WALK_MAX_M: f64 = 2000.0;
pub const MAX_ALTERNATIVES: usize = 5;

/// Resolve the request time to (local service date, seconds since local midnight, arrive_by).
pub fn resolve_time(ts: &TimeSpec) -> Result<(NaiveDate, i32, bool)> {
    let arrive_by = ts.kind == TimeKind::Arrive;
    let local: DateTime<Tz> = match (&ts.kind, &ts.iso) {
        (TimeKind::Now, _) | (_, None) => Utc::now().with_timezone(&Jerusalem),
        (_, Some(iso)) => parse_iso(iso)?,
    };
    Ok((local.date_naive(), local.num_seconds_from_midnight() as i32, arrive_by))
}

pub fn parse_iso(iso: &str) -> Result<DateTime<Tz>> {
    let s = iso.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Ok(d.with_timezone(&Jerusalem));
    }
    for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, f) {
            if let Some(d) = Jerusalem.from_local_datetime(&n).earliest() {
                return Ok(d);
            }
            return Ok(Jerusalem.from_utc_datetime(&(n - Duration::hours(2))));
        }
    }
    bail!("bad time: {iso}")
}

/// ISO-8601 (Asia/Jerusalem) of `secs` after local midnight of `date` (secs may be <0 or >86400).
pub fn iso_at(date: NaiveDate, secs: i32) -> String {
    let n = date.and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(secs as i64);
    let dt = match Jerusalem.from_local_datetime(&n).earliest() {
        Some(d) => d,
        None => Jerusalem.from_utc_datetime(&(n - Duration::hours(2))),
    };
    dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

fn color_hex(c: Option<u32>) -> Option<String> {
    c.map(|c| format!("#{c:06X}"))
}

/// Stops used for access/egress: nearest 20 within 1.2 km plus extra rail/light-rail stops.
fn candidate_stops(t: &Transit, p: LonLat) -> Vec<u32> {
    let near = t.stops_near(p, ACCESS_RADIUS_M);
    let mut out: Vec<u32> = near.iter().take(ACCESS_MAX_STOPS).map(|x| x.0).collect();
    let extra = near
        .iter()
        .skip(ACCESS_MAX_STOPS)
        .filter(|x| t.stop_modes.get(x.0 as usize).is_some_and(|m| m & !MODE_BUS != 0))
        .take(ACCESS_EXTRA_RAIL)
        .map(|x| x.0);
    out.extend(extra);
    out
}

#[derive(Clone)]
struct Cand {
    route: Route,
    /// Seconds relative to the query's base date (may be < 0 or >= 86400).
    dep: i32,
    arr: i32,
    seq: Vec<u32>,
    transit: bool,
}

/// Buffer at a via point between consecutive segments.
pub const VIA_BUFFER_S: i32 = 120;
pub const MAX_WAYPOINTS: usize = 6;

pub fn plan(t: &Transit, req: &RouteRequest, walker: &dyn WalkRouter) -> Result<RouteResponse> {
    let t0 = Instant::now();
    if req.waypoints.len() < 2 {
        bail!("route needs origin and destination");
    }
    if req.waypoints.len() > MAX_WAYPOINTS {
        bail!("too many waypoints (max {MAX_WAYPOINTS})");
    }
    let pts: Vec<LonLat> = req.waypoints.iter().map(|w| [w.lon, w.lat]).collect();
    let (date, secs, arrive_by) = resolve_time(&req.time)?;
    let want = (req.alternatives.max(1) as usize).min(MAX_ALTERNATIVES);
    let nseg = pts.len() - 1;

    let list: Vec<Cand> = if nseg == 1 {
        plan_segment(t, pts[0], pts[1], date, secs, arrive_by, want, true, walker)?
    } else {
        // Chain segments. The anchor segment (first for depart-at, last for arrive-by) gets the
        // full alternatives search; every alternative is then extended segment by segment with
        // the single best continuation (no combinatorial explosion). Results are cached per
        // (segment, time).
        let mut cache: FxHashMap<(usize, i32), Option<Cand>> = FxHashMap::default();
        let mut next_best = |seg: usize, tq: i32| -> Option<Cand> {
            cache
                .entry((seg, tq))
                .or_insert_with(|| {
                    plan_segment(t, pts[seg], pts[seg + 1], date, tq, arrive_by, 1, false, walker)
                        .ok()
                        .and_then(|v| v.into_iter().next())
                })
                .clone()
        };
        let anchor = if arrive_by { nseg - 1 } else { 0 };
        let first = plan_segment(t, pts[anchor], pts[anchor + 1], date, secs, arrive_by, want, true, walker)?;
        let mut combined = Vec::new();
        'alt: for c in first {
            let mut chain = vec![c];
            if !arrive_by {
                for seg in 1..nseg {
                    let tq = chain.last().unwrap().arr + VIA_BUFFER_S;
                    match next_best(seg, tq) {
                        Some(n) => chain.push(n),
                        None => continue 'alt,
                    }
                }
            } else {
                for seg in (0..nseg - 1).rev() {
                    let tq = chain.last().unwrap().dep - VIA_BUFFER_S;
                    match next_best(seg, tq) {
                        Some(n) => chain.push(n),
                        None => continue 'alt,
                    }
                }
                chain.reverse();
            }
            combined.push(combine(chain));
        }
        select(combined, arrive_by, want)
    };
    let mut routes: Vec<Route> = list.into_iter().map(|c| c.route).collect();
    for (i, r) in routes.iter_mut().enumerate() {
        r.id = format!("transit-{i}");
        r.tags.clear();
    }
    tag_routes(&mut routes);
    Ok(RouteResponse::offline(routes, t0.elapsed().as_millis() as u64))
}

/// Concatenate chronologically ordered segment candidates into one.
fn combine(chain: Vec<Cand>) -> Cand {
    let dep = chain.first().unwrap().dep;
    let arr = chain.last().unwrap().arr;
    let mut legs: Vec<Leg> = Vec::new();
    let mut seq = Vec::new();
    for (i, c) in chain.iter().enumerate() {
        if i > 0 {
            seq.push(u32::MAX);
        }
        seq.extend_from_slice(&c.seq);
        legs.extend(c.route.legs.iter().cloned());
    }
    let mut geometry: Vec<LonLat> = Vec::new();
    for l in &legs {
        let skip = usize::from(!geometry.is_empty() && geometry.last() == l.geometry.first());
        geometry.extend(l.geometry.iter().skip(skip).copied());
    }
    let ntransit = legs.iter().filter(|l| l.kind == LegKind::Transit).count() as u32;
    let route = Route {
        id: String::new(),
        mode: Mode::Transit,
        duration_sec: (arr - dep) as f64,
        distance_m: legs.iter().map(|l| l.distance_m).sum(),
        walk_m: legs.iter().filter(|l| l.kind == LegKind::Walk).map(|l| l.distance_m).sum(),
        geometry,
        legs,
        tags: vec![],
        depart_iso: chain.first().unwrap().route.depart_iso.clone(),
        arrive_iso: chain.last().unwrap().route.arrive_iso.clone(),
        transfers: ntransit.saturating_sub(1),
    };
    Cand { route, dep, arr, seq, transit: chain.iter().any(|c| c.transit) }
}

/// Plan a single origin->destination segment. `secs` is relative to `base` (any value; it is
/// normalised to a service date). `full` = alternatives search, otherwise one RAPTOR run.
#[allow(clippy::too_many_arguments)]
fn plan_segment(
    t: &Transit,
    o: LonLat,
    d: LonLat,
    base: NaiveDate,
    secs: i32,
    arrive_by: bool,
    want: usize,
    full: bool,
    walker: &dyn WalkRouter,
) -> Result<Vec<Cand>> {
    let shift_days = secs.div_euclid(DAY);
    let date = base + Duration::days(shift_days as i64);
    let secs = secs.rem_euclid(DAY);
    let direct = haversine_m(o, d);

    let mut cands: Vec<Cand> = Vec::new();
    let day = t.day_index(date);
    if day.is_none() && direct > PURE_WALK_MAX_M {
        bail!("NO_TRANSIT_SERVICE_ON_DATE");
    }
    if let Some(day) = day {
        let acc_stops = candidate_stops(t, o);
        let egr_stops = candidate_stops(t, d);
        // Access/egress durations come from the walker (road network when available), so the
        // boarding times RAPTOR picks already account for real walking detours.
        let access: Vec<(u32, Leg)> =
            acc_stops.par_iter().map(|&s| (s, walker.walk_or_straight(o, t.lonlat(s)))).collect();
        let egress: Vec<(u32, Leg)> =
            egr_stops.par_iter().map(|&s| (s, walker.walk_or_straight(t.lonlat(s), d))).collect();
        let wsec = |l: &Leg| l.duration_sec.ceil().max(0.0) as i32;
        let acc_map: FxHashMap<u32, &Leg> = access.iter().map(|(s, l)| (*s, l)).collect();
        let egr_map: FxHashMap<u32, &Leg> = egress.iter().map(|(s, l)| (*s, l)).collect();

        if !access.is_empty() && !egress.is_empty() {
            let mut journeys: Vec<Journey> = Vec::new();
            let run_at = |tq: i32, banned: &FxHashSet<u32>| -> Vec<Journey> {
                let (sources, targets): (Vec<(u32, i32, i32)>, Vec<(u32, i32)>) = if !arrive_by {
                    (
                        access.iter().map(|(s, l)| (*s, tq + wsec(l), wsec(l))).collect(),
                        egress.iter().map(|(s, l)| (*s, wsec(l))).collect(),
                    )
                } else {
                    (
                        egress.iter().map(|(s, l)| (*s, tq - wsec(l), wsec(l))).collect(),
                        access.iter().map(|(s, l)| (*s, wsec(l))).collect(),
                    )
                };
                raptor::run(
                    t,
                    &Query {
                        day,
                        reverse: arrive_by,
                        sources: &sources,
                        targets: &targets,
                        max_trips: raptor::MAX_TRIPS,
                        banned_routes: banned,
                    },
                )
            };
            let none = FxHashSet::default();
            let mut tq = secs;
            for _ in 0..(if full { 4 } else { 1 }) {
                let js = run_at(tq, &none);
                if js.is_empty() {
                    break;
                }
                tq = if !arrive_by {
                    js.iter().map(|j| j.depart).min().unwrap() + 60
                } else {
                    js.iter().map(|j| j.arrive).max().unwrap() - 60
                };
                journeys.extend(js);
                let distinct: FxHashSet<Vec<u32>> = journeys.iter().map(|j| line_key(t, j)).collect();
                if distinct.len() > want {
                    break;
                }
            }
            // diversification: ban the lines of the best journey
            let best = if !arrive_by {
                journeys.iter().min_by_key(|j| (j.arrive, -j.depart))
            } else {
                journeys.iter().max_by_key(|j| (j.depart, -j.arrive))
            };
            if let (true, Some(b)) = (full, best) {
                let lines: FxHashSet<(u32, u32, u32)> = b
                    .route_seq(t)
                    .into_iter()
                    .map(|r| {
                        let ri = &t.routes[r as usize];
                        (ri.agency, ri.short_name, ri.long_name)
                    })
                    .collect();
                let banned: FxHashSet<u32> = (0..t.routes.len() as u32)
                    .filter(|&r| {
                        let ri = &t.routes[r as usize];
                        lines.contains(&(ri.agency, ri.short_name, ri.long_name))
                    })
                    .collect();
                let js = run_at(secs, &banned);
                journeys.extend(js);
            }
            for j in &journeys {
                if let Some(route) = build_route(t, j, date, &acc_map, &egr_map, walker) {
                    cands.push(Cand { dep: j.depart, arr: j.arrive, seq: line_key(t, j), transit: true, route });
                }
            }
        }
    }

    // pure walking alternative
    if direct <= PURE_WALK_MAX_M {
        let mut leg = walker.walk_or_straight(o, d);
        let dur = leg.duration_sec.ceil() as i32;
        let (dep, arr) = if arrive_by { (secs - dur, secs) } else { (secs, secs + dur) };
        leg.depart_iso = Some(iso_at(date, dep));
        leg.arrive_iso = Some(iso_at(date, arr));
        let route = Route {
            id: String::new(),
            mode: Mode::Transit,
            duration_sec: (arr - dep) as f64,
            distance_m: leg.distance_m,
            geometry: leg.geometry.clone(),
            walk_m: leg.distance_m,
            legs: vec![leg],
            tags: vec![],
            depart_iso: Some(iso_at(date, dep)),
            arrive_iso: Some(iso_at(date, arr)),
            transfers: 0,
        };
        cands.push(Cand { route, dep, arr, seq: vec![], transit: false });
    }
    let mut list = select(cands, arrive_by, want);
    for c in &mut list {
        c.dep += shift_days * DAY;
        c.arr += shift_days * DAY;
    }
    Ok(list)
}

/// De-duplicate (same lines), drop dominated, sort and truncate to `want` (keeping a pure walk).
fn select(cands: Vec<Cand>, arrive_by: bool, want: usize) -> Vec<Cand> {
    let better = |a: &Cand, b: &Cand| -> bool {
        if !arrive_by {
            (a.arr, -a.dep) < (b.arr, -b.dep)
        } else {
            (-a.dep, a.arr) < (-b.dep, b.arr)
        }
    };
    let mut by_seq: FxHashMap<(bool, Vec<u32>), Cand> = FxHashMap::default();
    for c in cands {
        let k = (c.transit, c.seq.clone());
        match by_seq.get(&k) {
            Some(e) if !better(&c, e) => {}
            _ => {
                by_seq.insert(k, c);
            }
        }
    }
    let list: Vec<Cand> = by_seq.into_values().collect();
    let dominates = |a: &Cand, b: &Cand| -> bool {
        let ge = a.dep >= b.dep
            && a.arr <= b.arr
            && a.route.transfers <= b.route.transfers
            && a.route.walk_m <= b.route.walk_m + 1.0;
        let strict = a.dep > b.dep
            || a.arr < b.arr
            || a.route.transfers < b.route.transfers
            || a.route.walk_m + 1.0 < b.route.walk_m;
        ge && strict
    };
    let keep: Vec<bool> = (0..list.len())
        .map(|i| !(0..list.len()).any(|j| j != i && dominates(&list[j], &list[i])))
        .collect();
    let mut list: Vec<Cand> = list.into_iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| c).collect();
    if !arrive_by {
        list.sort_by_key(|c| (c.arr, c.arr - c.dep, -(c.transit as i32)));
    } else {
        list.sort_by_key(|c| (-c.dep, c.arr - c.dep, -(c.transit as i32)));
    }
    if list.len() > want {
        let walk_idx = list.iter().position(|c| !c.transit);
        let walk = walk_idx.filter(|&i| i >= want && want > 1).map(|i| list.remove(i));
        list.truncate(want);
        if let Some(w) = walk {
            list.pop();
            list.push(w);
        }
    }
    list
}

/// Identity of the lines ridden (agency + short + long name per transit leg). Several GTFS
/// route_ids can describe the same line (e.g. Israel Railways), so route indices are not enough.
fn line_key(t: &Transit, j: &Journey) -> Vec<u32> {
    let mut k = Vec::new();
    for l in &j.legs {
        if let JLeg::Transit { pattern, .. } = l {
            let r = &t.routes[t.patterns[*pattern as usize].route as usize];
            k.extend([r.agency, r.short_name, r.long_name]);
        }
    }
    k
}

fn tag_routes(routes: &mut [Route]) {
    if routes.is_empty() {
        return;
    }
    let argmin = |f: &dyn Fn(&Route) -> Option<f64>| -> Option<usize> {
        routes
            .iter()
            .enumerate()
            .filter_map(|(i, r)| f(r).map(|v| (i, v)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    };
    let fastest = argmin(&|r| Some(r.duration_sec));
    let fewest = argmin(&|r| {
        if r.legs.iter().any(|l| l.kind == LegKind::Transit) {
            Some(r.transfers as f64 + r.duration_sec / 1e6)
        } else {
            None
        }
    });
    let least_walk = argmin(&|r| Some(r.walk_m));
    if let Some(i) = fastest {
        routes[i].tags.push(RouteTag::Fastest);
    }
    if let Some(i) = fewest {
        routes[i].tags.push(RouteTag::FewestTransfers);
    }
    if let Some(i) = least_walk {
        routes[i].tags.push(RouteTag::LeastWalking);
    }
}

fn walk_leg_at(mut leg: Leg, date: NaiveDate, start: i32) -> Leg {
    leg.kind = LegKind::Walk;
    leg.depart_iso = Some(iso_at(date, start));
    leg.arrive_iso = Some(iso_at(date, start + leg.duration_sec.round() as i32));
    leg
}

fn transit_leg(t: &Transit, date: NaiveDate, p: u32, trip: u32, day_off: i32, from: u32, to: u32) -> Leg {
    let pt = &t.patterns[p as usize];
    let route = &t.routes[pt.route as usize];
    let stops = t.pstops(p);
    let off = day_off * DAY;
    let dep = t.dep_at(p, trip, from) + off;
    let arr = t.arr_at(p, trip, to) + off;
    let geometry = t.leg_geometry(p, from, to);
    let st_list: Vec<TransitStopTime> = (from..=to)
        .map(|pos| {
            let s = stops[pos as usize];
            let ll = t.lonlat(s);
            let tm = if pos == from { dep } else { t.arr_at(p, trip, pos) + off };
            TransitStopTime { name: t.stop_name(s).to_string(), code: t.stop_code(s), lon: ll[0], lat: ll[1], time_iso: iso_at(date, tm) }
        })
        .collect();
    let mut headsign = t.s(t.trip_headsign[(pt.trips_start + trip) as usize]).to_string();
    if headsign.is_empty() {
        headsign = t.stop_name(*stops.last().unwrap()).to_string();
    }
    let agency = if route.agency == NONE { String::new() } else { t.s(t.agencies[route.agency as usize]).to_string() };
    Leg {
        kind: LegKind::Transit,
        duration_sec: (arr - dep) as f64,
        distance_m: polyline_length_m(&geometry),
        geometry,
        depart_iso: Some(iso_at(date, dep)),
        arrive_iso: Some(iso_at(date, arr)),
        steps: vec![],
        transit: Some(TransitLegInfo {
            vehicle: t.vehicle_of_route(pt.route),
            route_short_name: t.s(route.short_name).to_string(),
            route_long_name: t.s(route.long_name).to_string(),
            agency,
            headsign,
            color: color_hex(route.color),
            from_stop: t.stop_name(stops[from as usize]).to_string(),
            to_stop: t.stop_name(stops[to as usize]).to_string(),
            depart_iso: iso_at(date, dep),
            arrive_iso: iso_at(date, arr),
            num_stops: to - from,
            stops: st_list,
        }),
    }
}

fn build_route(
    t: &Transit,
    j: &Journey,
    date: NaiveDate,
    acc: &FxHashMap<u32, &Leg>,
    egr: &FxHashMap<u32, &Leg>,
    walker: &dyn WalkRouter,
) -> Option<Route> {
    let mut legs: Vec<Leg> = Vec::new();
    let mut cur = j.depart;
    // `ready` tracks when the traveller is really ready, using the walker's (road network)
    // durations for stop-to-stop transfers; RAPTOR used a straight-line estimate. A connection
    // that the real walk cannot make (beyond a 60 s tolerance) invalidates the journey.
    let mut ready = j.depart;
    for l in &j.legs {
        match l {
            JLeg::Access { stop, walk } => {
                let leg = acc.get(stop).map(|l| (*l).clone()).unwrap_or_else(|| {
                    super::straight_walk(t.lonlat(*stop), t.lonlat(*stop))
                });
                legs.push(walk_leg_at(leg, date, cur));
                cur += walk;
                ready += walk;
            }
            JLeg::Foot { from, to, dur } => {
                let leg = walker.walk_or_straight(t.lonlat(*from), t.lonlat(*to));
                ready = ready.max(cur) + leg.duration_sec.ceil() as i32;
                legs.push(walk_leg_at(leg, date, cur));
                cur += dur;
            }
            JLeg::Transit { pattern, trip, day_off, from_pos, to_pos } => {
                let dep = t.dep_at(*pattern, *trip, *from_pos) + day_off * DAY;
                if ready > dep + 60 {
                    return None;
                }
                let leg = transit_leg(t, date, *pattern, *trip, *day_off, *from_pos, *to_pos);
                cur = t.arr_at(*pattern, *trip, *to_pos) + day_off * DAY;
                ready = cur;
                legs.push(leg);
            }
            JLeg::Egress { stop, walk } => {
                let leg = egr.get(stop).map(|l| (*l).clone()).unwrap_or_else(|| {
                    super::straight_walk(t.lonlat(*stop), t.lonlat(*stop))
                });
                legs.push(walk_leg_at(leg, date, cur));
                cur += walk;
            }
        }
    }
    // drop empty walks and merge consecutive walks
    let mut merged: Vec<Leg> = Vec::new();
    for l in legs {
        if l.kind == LegKind::Walk && l.distance_m < 1.0 && l.geometry.len() <= 2 {
            continue;
        }
        if let Some(prev) = merged.last_mut() {
            if prev.kind == LegKind::Walk && l.kind == LegKind::Walk {
                prev.duration_sec += l.duration_sec;
                prev.distance_m += l.distance_m;
                let skip = usize::from(prev.geometry.last() == l.geometry.first());
                let base = prev.geometry.len() - skip;
                prev.geometry.extend(l.geometry.into_iter().skip(skip));
                prev.steps.extend(l.steps.into_iter().map(|mut s| {
                    s.geometry_start += base;
                    s
                }));
                prev.arrive_iso = l.arrive_iso;
                continue;
            }
        }
        merged.push(l);
    }
    let mut geometry: Vec<LonLat> = Vec::new();
    for l in &merged {
        let skip = usize::from(!geometry.is_empty() && geometry.last() == l.geometry.first());
        geometry.extend(l.geometry.iter().skip(skip).copied());
    }
    let ntransit = merged.iter().filter(|l| l.kind == LegKind::Transit).count() as u32;
    Some(Route {
        id: String::new(),
        mode: Mode::Transit,
        duration_sec: (j.arrive - j.depart) as f64,
        distance_m: merged.iter().map(|l| l.distance_m).sum(),
        walk_m: merged.iter().filter(|l| l.kind == LegKind::Walk).map(|l| l.distance_m).sum(),
        geometry,
        legs: merged,
        tags: vec![],
        depart_iso: Some(iso_at(date, j.depart)),
        arrive_iso: Some(iso_at(date, j.arrive)),
        transfers: ntransit.saturating_sub(1),
    })
}
