//! RAPTOR (Delling, Pajor, Werneck 2012) – round-based public transit routing.
//!
//! * Forward mode: earliest arrival from a set of source stops (access walks already added).
//! * Reverse mode: latest departure given an arrival deadline. Implemented as a *true reverse
//!   scan*: the same algorithm runs on the time-reversed timetable. Internally every label is a
//!   "key" where smaller is better: `key = time` (forward) or `key = -time` (reverse). In reverse
//!   mode patterns are scanned from the last stop backwards, "boarding" a trip means finding the
//!   latest trip whose *arrival* at that stop is ≤ the required time, and pickup/drop-off flags
//!   swap roles. This keeps both directions in one code path.
//!
//! Features: up to `max_trips` rounds (trips; transfers = trips − 1), marked-stop optimisation,
//! local + target pruning, foot transfers after each round (never chained), a 60 s minimum change
//! time when changing vehicles at the same stop, and service-day handling over the previous,
//! current and next service day (offset −86400 / 0 / +86400 s) so trips of yesterday that run
//! past midnight are found.
//!
//! The result is the Pareto set over (arrival, number of trips): one journey per round that
//! improves the destination key.
//!
//! Range RAPTOR (`run_range`, rRAPTOR in the paper): the profile over a departure window. Every
//! boarding time at the access stops inside the window becomes one iteration, processed from the
//! latest departure to the earliest (reverse mode: earliest deadline to latest). Labels are kept
//! between iterations – an arrival reachable when leaving later is reachable when leaving earlier
//! by waiting – so each iteration only explores what the earlier start improves. Pruning is per
//! round (`cum[k]` = best key with at most k trips), which keeps the Pareto set over
//! (departure, arrival, trips) exact across iterations.

use rustc_hash::FxHashSet;

use super::model::{Transit, NONE, NO_DROPOFF, NO_PICKUP};

pub const MAX_TRIPS: usize = 5;
pub const TRANSFER_SLACK: i32 = 60;
pub const DAY: i32 = 86_400;
const INF: i32 = i32::MAX / 4;
/// Search horizon (seconds) after the earliest source time.
pub const HORIZON: i32 = 12 * 3600;

#[derive(Clone, Copy, Debug)]
enum Kind {
    None,
    Access { walk: i32 },
    Transit { pattern: u32, trip: u32, day_off: i8, board_pos: u16, alight_pos: u16 },
    Foot { from: u32, dur: i32 },
}

#[derive(Clone, Copy, Debug)]
struct Lab {
    key: i32,
    kind: Kind,
}
const NOLAB: Lab = Lab { key: INF, kind: Kind::None };

pub struct Query<'a> {
    pub day: u32,
    pub reverse: bool,
    /// (stop, time at the stop in seconds since `day` midnight, walk seconds of the access/egress leg).
    /// Forward: earliest arrival at the stop. Reverse: latest departure from the stop.
    pub sources: &'a [(u32, i32, i32)],
    /// (stop, walk seconds) – forward: egress stops, reverse: access stops.
    pub targets: &'a [(u32, i32)],
    pub max_trips: usize,
    pub banned_routes: &'a FxHashSet<u32>,
}

/// Journey leg in chronological order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JLeg {
    Access { stop: u32, walk: i32 },
    /// `trip` is local to the pattern; `day_off` −1/0/+1 relative to the query day.
    Transit { pattern: u32, trip: u32, day_off: i32, from_pos: u32, to_pos: u32 },
    Foot { from: u32, to: u32, dur: i32 },
    Egress { stop: u32, walk: i32 },
}

#[derive(Clone, Debug)]
pub struct Journey {
    pub legs: Vec<JLeg>,
    pub trips: usize,
    /// Departure from the origin (seconds since query-day midnight).
    pub depart: i32,
    /// Arrival at the destination.
    pub arrive: i32,
}

impl Journey {
    pub fn route_seq(&self, t: &Transit) -> Vec<u32> {
        self.legs
            .iter()
            .filter_map(|l| match l {
                JLeg::Transit { pattern, .. } => Some(t.patterns[*pattern as usize].route),
                _ => None,
            })
            .collect()
    }
}

struct DaySvc {
    day_off: i8,
    active: Vec<u64>,
}

struct Ctx<'a> {
    t: &'a Transit,
    fwd: bool,
    days: Vec<DaySvc>,
    prim: Vec<Vec<Lab>>,
    best: Vec<Vec<Lab>>,
    /// cum[k][s] = best key at s with at most k trips (over every iteration so far).
    cum: Vec<Vec<i32>>,
    max_trips: usize,
    /// Target pruning bound for the round being scanned.
    dest_bound: i32,
    marked: Vec<u32>,
    is_marked: Vec<bool>,
}

impl<'a> Ctx<'a> {
    #[inline]
    fn active(&self, ds: &DaySvc, p: u32, trip: u32) -> bool {
        let g = self.t.patterns[p as usize].trips_start + trip;
        let s = self.t.trip_service[g as usize];
        (ds.active[(s / 64) as usize] >> (s % 64)) & 1 == 1
    }

    /// Key of boarding `trip` (with day offset) at `pos`.
    #[inline]
    fn board_key(&self, p: u32, trip: u32, d: i8, pos: u32) -> i32 {
        let off = d as i32 * DAY;
        if self.fwd {
            self.t.dep_at(p, trip, pos) + off
        } else {
            -(self.t.arr_at(p, trip, pos) + off)
        }
    }

    /// Key of leaving `trip` at `pos`.
    #[inline]
    fn alight_key(&self, p: u32, trip: u32, d: i8, pos: u32) -> i32 {
        let off = d as i32 * DAY;
        if self.fwd {
            self.t.arr_at(p, trip, pos) + off
        } else {
            -(self.t.dep_at(p, trip, pos) + off)
        }
    }

    /// Best catchable trip at `pos` with board key ≥ `ready`.
    fn earliest_trip(&self, p: u32, pos: u32, ready: i32) -> Option<(u32, i8, i32)> {
        let nt = self.t.patterns[p as usize].ntrips;
        let mut best: Option<(u32, i8, i32)> = None;
        for ds in &self.days {
            let off = ds.day_off as i32 * DAY;
            if self.fwd {
                // first trip with dep + off >= ready
                let target = ready - off;
                let (mut lo, mut hi) = (0u32, nt);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if self.t.dep_at(p, mid, pos) < target {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let mut tr = lo;
                while tr < nt {
                    let k = self.t.dep_at(p, tr, pos) + off;
                    if best.is_some_and(|b| k >= b.2) {
                        break;
                    }
                    if self.active(ds, p, tr) {
                        best = Some((tr, ds.day_off, k));
                        break;
                    }
                    tr += 1;
                }
            } else {
                // last trip with arr + off <= -ready
                let target = -ready - off;
                let (mut lo, mut hi) = (0u32, nt);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if self.t.arr_at(p, mid, pos) <= target {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let mut tr = lo;
                while tr > 0 {
                    tr -= 1;
                    let k = -(self.t.arr_at(p, tr, pos) + off);
                    if best.is_some_and(|b| k >= b.2) {
                        break;
                    }
                    if self.active(ds, p, tr) {
                        best = Some((tr, ds.day_off, k));
                        break;
                    }
                }
            }
        }
        best
    }

    /// Record `key` at stop `s` reached with `k` trips in the per-round bounds.
    #[inline]
    fn improve(&mut self, k: usize, s: usize, key: i32) {
        for j in k..=self.max_trips {
            if key < self.cum[j][s] {
                self.cum[j][s] = key;
            } else {
                break;
            }
        }
    }

    fn mark(&mut self, s: u32) {
        if !self.is_marked[s as usize] {
            self.is_marked[s as usize] = true;
            self.marked.push(s);
        }
    }

    fn scan(&mut self, p: u32, start: u32, k: usize) {
        let t = self.t;
        let n = t.patterns[p as usize].n;
        let stops = t.pstops(p);
        let flags = t.pflags(p);
        let count = if self.fwd { n - start } else { start + 1 };
        let mut cur: Option<(u32, i8, u16)> = None;
        for i in 0..count {
            let pos = if self.fwd { start + i } else { start - i };
            let s = stops[pos as usize] as usize;
            let fl = flags[pos as usize];
            if let Some((tr, d, bp)) = cur {
                let alight_ok = if self.fwd { fl & NO_DROPOFF == 0 } else { fl & NO_PICKUP == 0 };
                if alight_ok {
                    let key = self.alight_key(p, tr, d, pos);
                    if key < self.cum[k][s].min(self.dest_bound) {
                        let lab = Lab {
                            key,
                            kind: Kind::Transit { pattern: p, trip: tr, day_off: d, board_pos: bp, alight_pos: pos as u16 },
                        };
                        self.prim[k][s] = lab;
                        self.best[k][s] = lab;
                        self.improve(k, s, key);
                        self.mark(s as u32);
                    }
                }
            }
            let prev = self.best[k - 1][s];
            if prev.key < INF {
                let board_ok = if self.fwd { fl & NO_PICKUP == 0 } else { fl & NO_DROPOFF == 0 };
                if board_ok {
                    let ready = prev.key + if matches!(prev.kind, Kind::Transit { .. }) { TRANSFER_SLACK } else { 0 };
                    let cur_key = cur.map(|(tr, d, _)| self.board_key(p, tr, d, pos));
                    if cur_key.is_none_or(|ck| ready <= ck) {
                        if let Some((tr, d, bk)) = self.earliest_trip(p, pos, ready) {
                            if cur_key.is_none_or(|ck| bk < ck) {
                                cur = Some((tr, d, pos as u16));
                            }
                        }
                    }
                }
            }
        }
    }

    fn relax_foot(&mut self, k: usize, from: &[u32]) {
        let t = self.t;
        for &s in from {
            let base = self.prim[k][s as usize];
            if base.key >= INF {
                continue;
            }
            for (to, dur) in t.transfers(s) {
                let key = base.key + dur as i32;
                if key < self.cum[k][to as usize].min(self.dest_bound) {
                    self.best[k][to as usize] = Lab { key, kind: Kind::Foot { from: s, dur: dur as i32 } };
                    self.improve(k, to as usize, key);
                    self.mark(to);
                }
            }
        }
    }

    fn reconstruct(&self, mut k: usize, target: u32, walk: i32) -> Option<Journey> {
        let t = self.t;
        let trips = k;
        let mut legs = Vec::new();
        let mut s = target;
        let mut lab = self.prim[k][s as usize];
        legs.push(if self.fwd { JLeg::Egress { stop: s, walk } } else { JLeg::Access { stop: s, walk } });
        for _ in 0..4 * MAX_TRIPS + 8 {
            match lab.kind {
                Kind::Foot { from, dur } => {
                    legs.push(if self.fwd { JLeg::Foot { from, to: s, dur } } else { JLeg::Foot { from: s, to: from, dur } });
                    s = from;
                    lab = self.prim[k][s as usize];
                }
                Kind::Transit { pattern, trip, day_off, board_pos, alight_pos } => {
                    let (from_pos, to_pos) =
                        if self.fwd { (board_pos as u32, alight_pos as u32) } else { (alight_pos as u32, board_pos as u32) };
                    legs.push(JLeg::Transit { pattern, trip, day_off: day_off as i32, from_pos, to_pos });
                    s = t.pstops(pattern)[board_pos as usize];
                    if k == 0 {
                        return None;
                    }
                    k -= 1;
                    lab = self.best[k][s as usize];
                }
                Kind::Access { walk } => {
                    // the source label is the access (fwd) / egress (rev) leg
                    legs.push(if self.fwd { JLeg::Access { stop: s, walk } } else { JLeg::Egress { stop: s, walk } });
                    if self.fwd {
                        legs.reverse();
                    }
                    return Some(finish(t, legs, trips));
                }
                Kind::None => return None,
            }
        }
        None
    }
}

/// Compute depart/arrive of a chronological leg list.
fn finish(t: &Transit, legs: Vec<JLeg>, trips: usize) -> Journey {
    let first = legs.iter().position(|l| matches!(l, JLeg::Transit { .. })).unwrap();
    let last = legs.iter().rposition(|l| matches!(l, JLeg::Transit { .. })).unwrap();
    let walk_of = |l: &JLeg| match l {
        JLeg::Access { walk, .. } | JLeg::Egress { walk, .. } => *walk,
        JLeg::Foot { dur, .. } => *dur,
        _ => 0,
    };
    let pre: i32 = legs[..first].iter().map(walk_of).sum();
    let post: i32 = legs[last + 1..].iter().map(walk_of).sum();
    let (dep, arr) = match (&legs[first], &legs[last]) {
        (
            JLeg::Transit { pattern: p1, trip: t1, day_off: d1, from_pos, .. },
            JLeg::Transit { pattern: p2, trip: t2, day_off: d2, to_pos, .. },
        ) => (t.dep_at(*p1, *t1, *from_pos) + d1 * DAY, t.arr_at(*p2, *t2, *to_pos) + d2 * DAY),
        _ => unreachable!(),
    };
    Journey { legs, trips, depart: dep - pre, arrive: arr + post }
}

/// Run RAPTOR. Returns the Pareto set (one journey per improving round), fewest trips first.
pub fn run(t: &Transit, q: &Query) -> Vec<Journey> {
    let mut v = run_range(t, q, 0);
    v.sort_by_key(|j| j.trips);
    v
}

/// Range RAPTOR over `window` seconds after the query time (reverse: before the deadline).
/// Returns every journey that is Pareto-optimal over (later departure, earlier arrival, fewer
/// trips) in the window; `window == 0` is a plain RAPTOR run.
pub fn run_range(t: &Transit, q: &Query, window: i32) -> Vec<Journey> {
    let nstops = t.stops.len();
    if nstops == 0 || q.sources.is_empty() || q.targets.is_empty() {
        return vec![];
    }
    let fwd = !q.reverse;
    let max_trips = q.max_trips.clamp(1, MAX_TRIPS);
    let mut days = Vec::new();
    for d in [-1i32, 0, 1] {
        let day = q.day as i32 + d;
        if day >= 0 && (day as u32) < t.num_days {
            days.push(DaySvc { day_off: d as i8, active: t.active_services(day as u32) });
        }
    }
    let mut c = Ctx {
        t,
        fwd,
        days,
        prim: vec![vec![NOLAB; nstops]; max_trips + 1],
        best: vec![vec![NOLAB; nstops]; max_trips + 1],
        cum: vec![vec![INF; nstops]; max_trips + 1],
        max_trips,
        dest_bound: INF,
        marked: Vec::new(),
        is_marked: vec![false; nstops],
    };
    let key_of = |time: i32| if fwd { time } else { -time };

    // Iterations: shifts of the source keys, largest first (latest departure / earliest deadline).
    let mut shifts: Vec<i32> = vec![0];
    if window > 0 {
        for &(s, time, _) in q.sources {
            let base = key_of(time);
            for (p, pos) in t.stop_patterns(s) {
                if !q.banned_routes.is_empty() && q.banned_routes.contains(&t.patterns[p as usize].route) {
                    continue;
                }
                let fl = t.pflags(p)[pos as usize];
                if (fwd && fl & NO_PICKUP != 0) || (!fwd && fl & NO_DROPOFF != 0) {
                    continue;
                }
                for tr in 0..t.patterns[p as usize].ntrips {
                    for ds in &c.days {
                        let shift = c.board_key(p, tr, ds.day_off, pos) - base;
                        if (0..=window).contains(&shift) && c.active(ds, p, tr) {
                            shifts.push(shift);
                        }
                    }
                }
            }
        }
        shifts.sort_unstable();
        shifts.dedup();
        // every boarding time is one iteration: dropping some would lose the journeys that start
        // exactly then (later iterations are cheap, labels carry over)
    }

    let mut dest_cum = vec![INF; max_trips + 1];
    let mut out: Vec<Journey> = Vec::new();
    let mut qpos = vec![NONE; t.patterns.len()];
    let mut queue: Vec<u32> = Vec::new();
    for &shift in shifts.iter().rev() {
        let mut min_key = INF;
        for &(s, time, walk) in q.sources {
            let key = key_of(time) + shift;
            min_key = min_key.min(key);
            if key < c.cum[0][s as usize] {
                let lab = Lab { key, kind: Kind::Access { walk } };
                c.prim[0][s as usize] = lab;
                c.best[0][s as usize] = lab;
                c.improve(0, s as usize, key);
                c.mark(s);
            }
        }
        let horizon = min_key.saturating_add(HORIZON);
        c.dest_bound = horizon.min(dest_cum[0]);
        let src: Vec<u32> = c.marked.clone();
        c.relax_foot(0, &src);

        let mut found: Vec<(usize, u32, i32)> = Vec::new(); // (round, target stop, walk)
        for k in 1..=max_trips {
            c.dest_bound = horizon.min(dest_cum[k]);
            let marked = std::mem::take(&mut c.marked);
            for &s in &marked {
                c.is_marked[s as usize] = false;
                for (p, pos) in t.stop_patterns(s) {
                    if !q.banned_routes.is_empty() && q.banned_routes.contains(&t.patterns[p as usize].route) {
                        continue;
                    }
                    let e = &mut qpos[p as usize];
                    if *e == NONE {
                        queue.push(p);
                        *e = pos;
                    } else if (fwd && pos < *e) || (!fwd && pos > *e) {
                        *e = pos;
                    }
                }
            }
            if queue.is_empty() {
                break;
            }
            for &p in &queue {
                let start = qpos[p as usize];
                qpos[p as usize] = NONE;
                c.scan(p, start, k);
            }
            queue.clear();
            let by_transit: Vec<u32> = c.marked.clone();
            c.relax_foot(k, &by_transit);
            let mut best_here: Option<(u32, i32, i32)> = None;
            for &(s, walk) in q.targets {
                let l = c.prim[k][s as usize];
                if l.key < INF && matches!(l.kind, Kind::Transit { .. }) {
                    let cand = l.key + walk;
                    if best_here.is_none_or(|b| cand < b.1) {
                        best_here = Some((s, cand, walk));
                    }
                }
            }
            if let Some((s, cand, walk)) = best_here {
                if cand < c.dest_bound {
                    for d in dest_cum.iter_mut().skip(k) {
                        if cand < *d {
                            *d = cand;
                        }
                    }
                    found.push((k, s, walk));
                }
            }
            if c.marked.is_empty() {
                break;
            }
        }
        // later iterations overwrite labels: reconstruct now
        out.extend(found.into_iter().filter_map(|(k, s, w)| c.reconstruct(k, s, w)));
        for s in std::mem::take(&mut c.marked) {
            c.is_marked[s as usize] = false;
        }
    }
    pareto(out, fwd)
}

/// Drop duplicates and journeys dominated over (departure, arrival, trips); for arrive-by the
/// same order applies (later departure and earlier arrival are both better).
fn pareto(mut v: Vec<Journey>, fwd: bool) -> Vec<Journey> {
    v.sort_by_key(|j| (j.trips, j.arrive, -j.depart));
    v.dedup_by(|a, b| a.depart == b.depart && a.arrive == b.arrive && a.trips == b.trips);
    let dominated = |a: &Journey, b: &Journey| {
        // does b dominate a?
        b.depart >= a.depart && b.arrive <= a.arrive && b.trips <= a.trips && (b.depart > a.depart || b.arrive < a.arrive || b.trips < a.trips)
    };
    let keep: Vec<bool> = (0..v.len()).map(|i| !(0..v.len()).any(|j| j != i && dominated(&v[i], &v[j]))).collect();
    let mut out: Vec<Journey> = v.into_iter().zip(keep).filter(|(_, k)| *k).map(|(j, _)| j).collect();
    if fwd {
        out.sort_by_key(|j| (j.depart, j.trips));
    } else {
        out.sort_by_key(|j| (j.arrive, j.trips));
    }
    out
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::types::LonLat;

    fn endpoints(t: &Transit, p: LonLat) -> Vec<(u32, i32)> {
        t.stops_near(p, 1200.0).into_iter().take(20).map(|(s, d)| (s, (d / 1.25) as i32)).collect()
    }

    /// Range RAPTOR must contain (or dominate) every plain RAPTOR answer at each departure in its
    /// window. Uses the installed transit.bin (cargo test -- --ignored range_matches_plain_runs).
    #[test]
    #[ignore]
    fn range_matches_plain_runs() {
        let path = crate::paths::DataPaths::new(crate::paths::DataPaths::default_root()).transit_file();
        let t = crate::transit::load(&path).expect("transit.bin");
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let day = t.day_index(date).expect("date in feed");
        let none = FxHashSet::default();
        for (o, d) in [([34.7818, 32.0853], [35.2137, 31.7683]), ([34.77, 32.07], [34.89, 32.17]), ([35.0, 32.8], [34.99, 32.79])] {
            for reverse in [false, true] {
                let (acc, egr) = (endpoints(&t, o), endpoints(&t, d));
                let base = if reverse { 9 * 3600 } else { 8 * 3600 };
                let window = 3600;
                let mk = |tq: i32| -> (Vec<(u32, i32, i32)>, Vec<(u32, i32)>) {
                    if !reverse {
                        (acc.iter().map(|&(s, w)| (s, tq + w, w)).collect(), egr.clone())
                    } else {
                        (egr.iter().map(|&(s, w)| (s, tq - w, w)).collect(), acc.clone())
                    }
                };
                let (src, tgt) = mk(base);
                let q = Query { day, reverse, sources: &src, targets: &tgt, max_trips: MAX_TRIPS, banned_routes: &none };
                let t0 = std::time::Instant::now();
                let range = run_range(&t, &q, window);
                let t_range = t0.elapsed();
                let t0 = std::time::Instant::now();
                for step in 0..4 {
                    let (src, tgt) = mk(if reverse { base - step * 900 } else { base + step * 900 });
                    let q = Query { day, reverse, sources: &src, targets: &tgt, max_trips: MAX_TRIPS, banned_routes: &none };
                    let _ = run(&t, &q);
                }
                eprintln!("  range {:?} vs 4 plain runs {:?}", t_range, t0.elapsed());
                let mut checked = 0;
                for step in (0..=window).step_by(120) {
                    let tq = if reverse { base - step } else { base + step };
                    let (src, tgt) = mk(tq);
                    let q = Query { day, reverse, sources: &src, targets: &tgt, max_trips: MAX_TRIPS, banned_routes: &none };
                    for j in run(&t, &q) {
                        // only journeys that start inside the window are covered by the profile
                        let inside = if reverse { j.arrive >= base - window } else { j.depart <= base + window };
                        if !inside {
                            continue;
                        }
                        checked += 1;
                        let ok = range.iter().any(|r| r.depart >= j.depart && r.arrive <= j.arrive && r.trips <= j.trips);
                        assert!(ok, "{o:?}->{d:?} rev={reverse} t={tq}: plain dep {} arr {} trips {} not covered; range: {:?}", j.depart, j.arrive, j.trips,
                            range.iter().map(|r| (r.depart, r.arrive, r.trips)).collect::<Vec<_>>());
                    }
                }
                eprintln!("{o:?}->{d:?} rev={reverse}: {} range journeys, {checked} plain journeys covered", range.len());
            }
        }
    }
}
