//! Bidirectional A* on directed-edge states with turn costs / restrictions.
//!
//! Forward labels `df[s]` = cost from the origin having fully traversed state `s` (including the
//! turn into it). Backward labels `db[s]` = cost from the tail of `s` (traversing it first) to the
//! destination. A path through `s` costs `df[s] + db[s] - w(s)`. Keys are reduced by the
//! consistent potential `p(v) = (h_t(v) - h_s(v)) / 2`; the search stops when
//! `top_f + top_b >= best`. Meetings are checked whenever either side labels a state.
use std::cmp::Ordering;
use std::collections::BinaryHeap;

use parking_lot::Mutex;
use smallvec::SmallVec;

use crate::geo::haversine_m;
use crate::routing::graph::{proj, Graph, Snap, NONE};
use crate::routing::profile::Profile;
use crate::types::{Leg, LegKind, LonLat};

// ---------------------------------------------------------------- reusable buffers

/// Label of one state in one search direction. Stamp, distance and parent are packed so that
/// a relaxation touches a single cache line.
#[derive(Clone, Copy)]
#[repr(C)]
struct Label {
    stamp: u32,
    dist: f32,
    par: u32,
}

struct Bufs {
    n: usize,
    gen: u32,
    f: Vec<Label>,
    b: Vec<Label>,
}

/// `n` all-zero labels (stamp 0 = "not reached"). Uses `alloc_zeroed`, so the OS hands out
/// lazily-zeroed pages: no upfront 50 MB memset on the first search.
fn zeroed_labels(n: usize) -> Vec<Label> {
    if n == 0 {
        return Vec::new();
    }
    let layout = std::alloc::Layout::array::<Label>(n).expect("label buffer size");
    // SAFETY: `Label` is repr(C) of u32/f32/u32, for which all-zero bytes are a valid value;
    // the pointer comes from the global allocator with exactly the layout of a Vec<Label>
    // with capacity `n`, so `Vec::from_raw_parts` may own and free it.
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout) as *mut Label;
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Vec::from_raw_parts(ptr, n, n)
    }
}

impl Bufs {
    fn new(n: usize) -> Self {
        // `par` of an unreached label is never read: parents are only followed from labels
        // stamped with the current generation, and those always have `par` written.
        Self { n, gen: 0, f: zeroed_labels(n), b: zeroed_labels(n) }
    }
    fn next_gen(&mut self) -> u32 {
        if self.gen == u32::MAX {
            self.f.iter_mut().for_each(|l| l.stamp = 0);
            self.b.iter_mut().for_each(|l| l.stamp = 0);
            self.gen = 0;
        }
        self.gen += 1;
        self.gen
    }
}

static POOL: Mutex<Vec<Bufs>> = Mutex::new(Vec::new());

fn take_bufs(n: usize) -> Bufs {
    let mut pool = POOL.lock();
    if let Some(i) = pool.iter().position(|b| b.n == n) {
        return pool.swap_remove(i);
    }
    drop(pool);
    Bufs::new(n)
}

fn give_bufs(b: Bufs) {
    let mut pool = POOL.lock();
    if pool.len() < 4 {
        pool.push(b);
    }
}

// ---------------------------------------------------------------- heap item

#[derive(Clone, Copy)]
struct Item {
    key: f32,
    s: u32,
}
impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        self.key == o.key && self.s == o.s
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        // min-heap on key
        o.key.total_cmp(&self.key).then_with(|| o.s.cmp(&self.s))
    }
}

// ---------------------------------------------------------------- public API

/// Search parameters.
pub struct SearchParams<'a> {
    pub profile: &'a Profile,
    /// extra multiplicative penalty per edge, indexed by edge id (alternatives)
    pub penalty: Option<&'a [f32]>,
    /// give up when the cheapest open label exceeds this (seconds)
    pub max_cost: f32,
}

/// A found path as a sequence of directed states.
#[derive(Clone, Debug)]
pub struct PathResult {
    pub states: Vec<u32>,
    /// fraction along the first state's direction where the path starts
    pub start_frac: f32,
    /// fraction along the last state's direction where the path ends
    pub end_frac: f32,
    /// search cost (with preferences / penalties)
    pub cost: f32,
}

/// Geometry + per-state details of a path.
#[derive(Clone, Debug, Default)]
pub struct PathDetails {
    pub geometry: Vec<LonLat>,
    /// index into `geometry` where each state starts
    pub state_starts: Vec<usize>,
    pub state_dist: Vec<f64>,
    pub state_dur: Vec<f64>,
    pub distance_m: f64,
    pub duration_sec: f64,
}

#[inline]
fn pen(penalty: Option<&[f32]>, e: u32) -> f32 {
    match penalty {
        Some(m) => m[e as usize],
        None => 1.0,
    }
}

#[inline]
fn d2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// Fraction of a snap measured along direction `dir` of its edge.
#[inline]
fn frac_dir(snap: &Snap, dir: u32) -> f32 {
    if dir == 0 { snap.frac } else { 1.0 - snap.frac }
}

/// Straight-line distance (m) above which long-route pruning kicks in.
const PRUNE_MIN_STRAIGHT: f32 = 15_000.0;

pub fn search(g: &Graph, p: &SearchParams, from: &Snap, to: &Snap) -> Option<PathResult> {
    let r = search_impl(g, p, from, to, true);
    if r.is_none() && d2(proj(from.point[0], from.point[1]), proj(to.point[0], to.point[1])) > PRUNE_MIN_STRAIGHT * PRUNE_MIN_STRAIGHT {
        // the hierarchy pruning cut the only connection (e.g. a long private / rural access road)
        return search_impl(g, p, from, to, false);
    }
    r
}

fn search_impl(g: &Graph, p: &SearchParams, from: &Snap, to: &Snap, allow_prune: bool) -> Option<PathResult> {
    let n = g.num_states();
    let prof = p.profile;
    let penalty = p.penalty;
    let mut b = take_bufs(n);
    let gen = b.next_gen();

    let ps = proj(from.point[0], from.point[1]);
    let pt = proj(to.point[0], to.point[1]);
    let hfac = prof.heuristic_spm(g);
    let straight = d2(ps, pt).sqrt();
    // Hierarchy pruning for long routes: minor roads only near the endpoints, tertiary roads
    // (car) within a wider radius, the main network everywhere.
    let prune = allow_prune && straight > PRUNE_MIN_STRAIGHT;
    let r_local2 = {
        let r = (straight * 0.05).clamp(2_500.0, 10_000.0);
        r * r
    };
    let r_wide2 = {
        let r = (straight * 0.25).clamp(10_000.0, 40_000.0);
        r * r
    };
    let pot = |v: u32| -> f32 {
        let vp = g.vproj(v);
        (d2(vp, pt).sqrt() - d2(vp, ps).sqrt()) * hfac * 0.5
    };

    let mut qf: BinaryHeap<Item> = BinaryHeap::with_capacity(4096);
    let mut qb: BinaryHeap<Item> = BinaryHeap::with_capacity(4096);
    let mut best = f32::INFINITY;
    let mut meet: u32 = NONE;

    let origin_init: SmallVec<[u32; 2]> = (0..2u32).map(|d| from.edge * 2 + d).filter(|&s| prof.allowed(g, s)).collect();
    let dest_init: SmallVec<[u32; 2]> = (0..2u32).map(|d| to.edge * 2 + d).filter(|&s| prof.allowed(g, s)).collect();
    if origin_init.is_empty() || dest_init.is_empty() {
        give_bufs(b);
        return None;
    }

    // meeting check (a fn rather than a closure because of the split borrows)
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn check_meet(g: &Graph, prof: &Profile, penalty: Option<&[f32]>, b: &Bufs, from: &Snap, to: &Snap, s: u32, best: &mut f32, meet: &mut u32) {
        let si = s as usize;
        let (lf, lb) = (b.f[si], b.b[si]);
        if lf.par == NONE && lb.par == NONE {
            // same edge, both partial: valid only if the destination lies ahead of the origin
            let dir = s & 1;
            if frac_dir(to, dir) < frac_dir(from, dir) - 1e-6 {
                return;
            }
        }
        let e = s >> 1;
        let w = prof.cost(g, e) * pen(penalty, e);
        let cand = lf.dist + lb.dist - w;
        if cand < *best {
            *best = cand;
            *meet = s;
        }
    }

    for &s in &origin_init {
        let e = s >> 1;
        let cost = prof.cost(g, e) * pen(penalty, e) * (1.0 - frac_dir(from, s & 1));
        b.f[s as usize] = Label { stamp: gen, dist: cost, par: NONE };
        qf.push(Item { key: cost + pot(g.head(s)), s });
    }
    for &s in &dest_init {
        let e = s >> 1;
        let cost = prof.cost(g, e) * pen(penalty, e) * frac_dir(to, s & 1);
        b.b[s as usize] = Label { stamp: gen, dist: cost, par: NONE };
        qb.push(Item { key: cost - pot(g.tail(s)), s });
    }
    for &s in &origin_init {
        if b.b[s as usize].stamp == gen {
            check_meet(g, prof, penalty, &b, from, to, s, &mut best, &mut meet);
        }
    }

    let near_ok = |e: u32| -> bool {
        if !prune {
            return true;
        }
        let r2 = match prof.prune_tier(g, e) {
            0 => return true,
            1 => r_wide2,
            _ => r_local2,
        };
        let v = g.vproj(g.e_from[e as usize]);
        d2(v, ps) < r2 || d2(v, pt) < r2
    };

    let mut settled = 0u32;
    loop {
        let tf = qf.peek().map(|i| i.key).unwrap_or(f32::INFINITY);
        let tb = qb.peek().map(|i| i.key).unwrap_or(f32::INFINITY);
        if tf + tb >= best {
            break;
        }
        if tf.min(tb) > p.max_cost {
            break;
        }
        settled += 1;
        if tf <= tb {
            let Item { key, s } = qf.pop().unwrap();
            let v = g.head(s);
            let base = b.f[s as usize].dist;
            if key > base + pot(v) + 1e-3 {
                continue; // stale
            }
            for &e2 in g.incident(v) {
                let s2 = g.state_from(v, e2);
                if !prof.allowed(g, s2) || !near_ok(e2) {
                    continue;
                }
                let Some(turn) = prof.turn_cost(g, s, s2) else { continue };
                let nd = base + turn + prof.cost(g, e2) * pen(penalty, e2);
                let s2i = s2 as usize;
                let l = &mut b.f[s2i];
                if l.stamp != gen || nd < l.dist {
                    *l = Label { stamp: gen, dist: nd, par: s };
                    qf.push(Item { key: nd + pot(g.head(s2)), s: s2 });
                    if b.b[s2i].stamp == gen {
                        check_meet(g, prof, penalty, &b, from, to, s2, &mut best, &mut meet);
                    }
                }
            }
        } else {
            let Item { key, s } = qb.pop().unwrap();
            let u = g.tail(s);
            let base = b.b[s as usize].dist;
            if key > base - pot(u) + 1e-3 {
                continue;
            }
            for &e0 in g.incident(u) {
                // state arriving at u along e0
                let s0 = if g.e_to[e0 as usize] == u { e0 * 2 } else { e0 * 2 + 1 };
                if !prof.allowed(g, s0) || !near_ok(e0) {
                    continue;
                }
                let Some(turn) = prof.turn_cost(g, s0, s) else { continue };
                let nd = base + turn + prof.cost(g, e0) * pen(penalty, e0);
                let s0i = s0 as usize;
                let l = &mut b.b[s0i];
                if l.stamp != gen || nd < l.dist {
                    *l = Label { stamp: gen, dist: nd, par: s };
                    qb.push(Item { key: nd - pot(g.tail(s0)), s: s0 });
                    if b.f[s0i].stamp == gen {
                        check_meet(g, prof, penalty, &b, from, to, s0, &mut best, &mut meet);
                    }
                }
            }
        }
    }

    log::debug!("search: {settled} pops, best {best:.0}");
    let result = if meet == NONE {
        None
    } else {
        let mut states = Vec::new();
        let mut s = meet;
        loop {
            states.push(s);
            let p = b.f[s as usize].par;
            if p == NONE {
                break;
            }
            s = p;
        }
        states.reverse();
        let mut s = b.b[meet as usize].par;
        while s != NONE {
            states.push(s);
            s = b.b[s as usize].par;
        }
        let first = states[0];
        let last = *states.last().unwrap();
        Some(PathResult { states, start_frac: frac_dir(from, first & 1), end_frac: frac_dir(to, last & 1), cost: best })
    };
    give_bufs(b);
    result
}

/// Cut a polyline by fractions of its length.
pub fn cut_polyline(pts: &[LonLat], from_frac: f64, to_frac: f64) -> Vec<LonLat> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let total: f64 = pts.windows(2).map(|w| haversine_m(w[0], w[1])).sum();
    if total <= 0.0 {
        return vec![pts[0], pts[pts.len() - 1]];
    }
    let (a, b) = (from_frac.clamp(0.0, 1.0) * total, to_frac.clamp(0.0, 1.0) * total);
    let mut out = Vec::new();
    let mut acc = 0.0;
    let interp = |p: LonLat, q: LonLat, t: f64| -> LonLat { [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t] };
    for w in pts.windows(2) {
        let seg = haversine_m(w[0], w[1]);
        let (s0, s1) = (acc, acc + seg);
        if s1 >= a && s0 <= b {
            if out.is_empty() {
                let t = if seg > 0.0 { ((a - s0) / seg).clamp(0.0, 1.0) } else { 0.0 };
                out.push(interp(w[0], w[1], t));
            }
            if s1 <= b {
                out.push(w[1]);
            } else {
                let t = if seg > 0.0 { ((b - s0) / seg).clamp(0.0, 1.0) } else { 1.0 };
                out.push(interp(w[0], w[1], t));
                break;
            }
        }
        acc = s1;
    }
    if out.len() < 2 {
        let p = out.first().copied().unwrap_or(pts[0]);
        out = vec![p, p];
    }
    out
}

/// Expand a path into geometry and per-state distance / true duration.
pub fn details(g: &Graph, prof: &Profile, path: &PathResult) -> PathDetails {
    let n = path.states.len();
    let mut d = PathDetails::default();
    let mut prev_state: Option<u32> = None;
    for (i, &s) in path.states.iter().enumerate() {
        let e = s >> 1;
        let (f0, f1) = (if i == 0 { path.start_frac as f64 } else { 0.0 }, if i == n - 1 { path.end_frac as f64 } else { 1.0 });
        let pts = g.state_points(s);
        let part = if f0 > 0.0 || f1 < 1.0 { cut_polyline(&pts, f0, f1) } else { pts };
        let dist: f64 = part.windows(2).map(|w| haversine_m(w[0], w[1])).sum();
        let mut dur = prof.traverse_time(g, e) as f64 * (f1 - f0).max(0.0);
        if let Some(ps) = prev_state {
            dur += prof.turn_cost(g, ps, s).unwrap_or(0.0) as f64;
        }
        d.state_starts.push(if d.geometry.is_empty() { 0 } else { d.geometry.len() - 1 });
        if d.geometry.is_empty() {
            d.geometry.extend_from_slice(&part);
        } else {
            d.geometry.extend_from_slice(&part[1..]);
        }
        d.state_dist.push(dist);
        d.state_dur.push(dur);
        d.distance_m += dist;
        d.duration_sec += dur;
        prev_state = Some(s);
    }
    d
}

/// Walking leg between two points on the network; `None` if too far / unreachable.
pub fn walk_leg(g: &Graph, from: LonLat, to: LonLat) -> Option<Leg> {
    let straight = haversine_m(from, to);
    if straight > 3_000.0 {
        return None;
    }
    let prof = Profile::new(crate::types::Mode::Foot, Default::default());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let sf = g.snap(from, &allowed)?;
    let st = g.snap(to, &allowed)?;
    if sf.dist_m > 500.0 || st.dist_m > 500.0 {
        return None;
    }
    let params = SearchParams { profile: &prof, penalty: None, max_cost: 3_600.0 };
    let path = search(g, &params, &sf, &st)?;
    let det = details(g, &prof, &path);
    let mut geometry = Vec::with_capacity(det.geometry.len() + 2);
    geometry.push(from);
    geometry.extend_from_slice(&det.geometry);
    geometry.push(to);
    let extra = haversine_m(from, sf.point) + haversine_m(st.point, to);
    let distance_m = det.distance_m + extra;
    let duration_sec = det.duration_sec + extra / crate::routing::profile::FOOT_SPEED as f64;
    let steps = crate::routing::instructions::build_steps(g, &prof, &path.states, &det, 1);
    Some(Leg {
        kind: LegKind::Walk,
        duration_sec,
        distance_m,
        geometry,
        depart_iso: None,
        arrive_iso: None,
        steps,
        transit: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_middle() {
        let pts = vec![[34.0, 32.0], [34.001, 32.0], [34.002, 32.0]];
        let c = cut_polyline(&pts, 0.25, 0.75);
        assert_eq!(c.len(), 3);
        assert!((c[0][0] - 34.0005).abs() < 1e-6);
        assert!((c[2][0] - 34.0015).abs() < 1e-6);
        let full = cut_polyline(&pts, 0.0, 1.0);
        assert_eq!(full.len(), 3);
    }

    #[test]
    fn heap_is_min() {
        let mut h = BinaryHeap::new();
        h.push(Item { key: 5.0, s: 1 });
        h.push(Item { key: 1.0, s: 2 });
        h.push(Item { key: 3.0, s: 3 });
        assert_eq!(h.pop().unwrap().s, 2);
        assert_eq!(h.pop().unwrap().s, 3);
    }
}
