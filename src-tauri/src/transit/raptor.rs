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
    star: Vec<i32>,
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
                    if key < self.star[s].min(self.dest_bound) {
                        let lab = Lab {
                            key,
                            kind: Kind::Transit { pattern: p, trip: tr, day_off: d, board_pos: bp, alight_pos: pos as u16 },
                        };
                        self.prim[k][s] = lab;
                        self.best[k][s] = lab;
                        self.star[s] = key;
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
                if key < self.star[to as usize].min(self.dest_bound) {
                    self.best[k][to as usize] = Lab { key, kind: Kind::Foot { from: s, dur: dur as i32 } };
                    self.star[to as usize] = key;
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
        star: vec![INF; nstops],
        dest_bound: INF,
        marked: Vec::new(),
        is_marked: vec![false; nstops],
    };
    let key_of = |time: i32| if fwd { time } else { -time };
    let mut min_key = INF;
    for &(s, time, walk) in q.sources {
        let key = key_of(time);
        min_key = min_key.min(key);
        if key < c.star[s as usize] {
            let lab = Lab { key, kind: Kind::Access { walk } };
            c.prim[0][s as usize] = lab;
            c.best[0][s as usize] = lab;
            c.star[s as usize] = key;
            c.mark(s);
        }
    }
    c.dest_bound = min_key.saturating_add(HORIZON);
    let src: Vec<u32> = c.marked.clone();
    c.relax_foot(0, &src);

    let mut found: Vec<(usize, u32, i32)> = Vec::new(); // (round, target stop, walk)
    let mut qpos = vec![NONE; t.patterns.len()];
    let mut queue: Vec<u32> = Vec::new();
    for k in 1..=max_trips {
        // collect patterns through marked stops
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
        // foot transfers from stops improved by transit in this round
        let by_transit: Vec<u32> = c.marked.clone();
        c.relax_foot(k, &by_transit);
        // destination
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
                c.dest_bound = cand;
                found.push((k, s, walk));
            }
        }
        if c.marked.is_empty() {
            break;
        }
    }
    found.into_iter().filter_map(|(k, s, w)| c.reconstruct(k, s, w)).collect()
}
