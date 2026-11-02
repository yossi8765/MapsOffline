//! Route assembly: waypoints → legs, penalty-based alternatives, time resolution.
use std::time::Instant;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use chrono_tz::Asia::Jerusalem;
use rustc_hash::FxHashSet;

use crate::routing::astar::{self, PathDetails, PathResult, SearchParams};
use crate::routing::graph::{Graph, Snap};
use crate::routing::instructions;
use crate::routing::profile::Profile;
use crate::types::{Leg, LegKind, Mode, Route, RouteRequest, RouteResponse, RouteTag, TimeKind, TimeSpec};

const PENALTY: f32 = 1.6;
const MAX_SHARE: f64 = 0.70;
const MAX_STRETCH: f64 = 1.4;
const MAX_SNAP_M: f64 = 5_000.0;

/// Reusable dense per-edge penalty arrays (all 1.0 when idle).
static PENALTY_POOL: parking_lot::Mutex<Vec<Vec<f32>>> = parking_lot::Mutex::new(Vec::new());

fn take_penalty(n: usize) -> Vec<f32> {
    let mut pool = PENALTY_POOL.lock();
    match pool.iter().position(|v| v.len() == n) {
        Some(i) => pool.swap_remove(i),
        None => vec![1.0; n],
    }
}

/// Return a penalty array, resetting the `touched` edges back to 1.0.
fn give_penalty(mut v: Vec<f32>, touched: &[u32]) {
    for &e in touched {
        v[e as usize] = 1.0;
    }
    let mut pool = PENALTY_POOL.lock();
    if pool.len() < 2 {
        pool.push(v);
    }
}

struct Candidate {
    paths: Vec<PathResult>,
    details: Vec<PathDetails>,
    duration: f64,
    distance: f64,
}

fn leg_kind(mode: Mode) -> LegKind {
    match mode {
        Mode::Car => LegKind::Drive,
        Mode::Bike => LegKind::Bike,
        _ => LegKind::Walk,
    }
}

fn fmt(t: DateTime<chrono_tz::Tz>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// (depart, arrive) ISO strings for a route of `duration` seconds.
pub fn resolve_times(time: &TimeSpec, duration: f64) -> (Option<String>, Option<String>) {
    let dur = Duration::seconds(duration.round() as i64);
    let now = Utc::now().with_timezone(&Jerusalem);
    let parsed = time.iso.as_deref().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Jerusalem));
    match (time.kind, parsed) {
        (TimeKind::Arrive, Some(arr)) => (Some(fmt(arr - dur)), Some(fmt(arr))),
        (TimeKind::Depart, Some(dep)) => (Some(fmt(dep)), Some(fmt(dep + dur))),
        _ => (Some(fmt(now)), Some(fmt(now + dur))),
    }
}

fn compute(g: &Graph, prof: &Profile, snaps: &[Snap], penalty: Option<&[f32]>) -> Option<Candidate> {
    let mut paths = Vec::new();
    let mut details = Vec::new();
    let (mut duration, mut distance) = (0.0, 0.0);
    for w in snaps.windows(2) {
        let params = SearchParams { profile: prof, penalty, max_cost: f32::INFINITY };
        let p = astar::search(g, &params, &w[0], &w[1])?;
        let d = astar::details(g, prof, &p);
        duration += d.duration_sec;
        distance += d.distance_m;
        paths.push(p);
        details.push(d);
    }
    Some(Candidate { paths, details, duration, distance })
}

/// First (unpenalized) route: choose, per waypoint, the first snap candidate that is actually
/// connected (islands such as parking lots behind private roads are skipped).
fn compute_base(g: &Graph, prof: &Profile, cands: &[Vec<Snap>]) -> Option<(Candidate, Vec<Snap>)> {
    let mut chosen: Vec<Snap> = Vec::with_capacity(cands.len());
    let mut paths = Vec::new();
    let mut details = Vec::new();
    let (mut duration, mut distance) = (0.0, 0.0);
    let params = SearchParams { profile: prof, penalty: None, max_cost: f32::INFINITY };
    for i in 0..cands.len() - 1 {
        let starts: Vec<Snap> = if i == 0 { cands[0].clone() } else { vec![chosen[i]] };
        let mut found = None;
        'outer: for a in starts.iter() {
            for b in &cands[i + 1] {
                if let Some(p) = astar::search(g, &params, a, b) {
                    found = Some((*a, *b, p));
                    break 'outer;
                }
            }
        }
        let (a, b, p) = found?;
        if i == 0 {
            chosen.push(a);
        }
        chosen.push(b);
        let d = astar::details(g, prof, &p);
        duration += d.duration_sec;
        distance += d.distance_m;
        paths.push(p);
        details.push(d);
    }
    Some((Candidate { paths, details, duration, distance }, chosen))
}

fn edge_set(c: &Candidate) -> FxHashSet<u32> {
    c.paths.iter().flat_map(|p| p.states.iter().map(|s| s >> 1)).collect()
}

/// Fraction of `c`'s length that runs on edges of `other`.
fn shared_fraction(g: &Graph, c: &Candidate, other: &FxHashSet<u32>) -> f64 {
    let mut shared = 0.0f64;
    let mut total = 0.0f64;
    for p in &c.paths {
        for &s in &p.states {
            let e = s >> 1;
            let l = g.e_len[e as usize] as f64;
            total += l;
            if other.contains(&e) {
                shared += l;
            }
        }
    }
    if total <= 0.0 { 1.0 } else { shared / total }
}

pub fn route_with_alternatives(g: &Graph, req: &RouteRequest) -> anyhow::Result<RouteResponse> {
    let t0 = Instant::now();
    if req.waypoints.len() < 2 {
        anyhow::bail!("NEED_TWO_WAYPOINTS");
    }
    if req.waypoints.len() > 7 {
        anyhow::bail!("TOO_MANY_WAYPOINTS");
    }
    let prof = Profile::new(req.mode, req.avoid.clone());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let mut snap_cands: Vec<Vec<Snap>> = Vec::with_capacity(req.waypoints.len());
    for w in &req.waypoints {
        if prof.avoids_areas() {
            // A waypoint whose nearest road lies in an avoided territory must not be silently
            // moved to the nearest permitted road (possibly kilometres away).
            let open = prof.without_areas();
            let any = |e: u32| open.allowed_edge_any(g, e);
            if let Some(n) = g.snap_candidates([w.lon, w.lat], &any, 1).first() {
                if n.dist_m <= MAX_SNAP_M && g.flags(n.edge) & prof.forbid != 0 {
                    anyhow::bail!("NO_ROUTE_AVOID_AREAS");
                }
            }
        }
        let mut c = g.snap_candidates([w.lon, w.lat], &allowed, 5);
        c.retain(|s| s.dist_m <= MAX_SNAP_M);
        if c.is_empty() {
            anyhow::bail!("NO_ROAD_NEAR_WAYPOINT");
        }
        snap_cands.push(c);
    }

    log::debug!("snap: {:?}", t0.elapsed());
    let k = if req.waypoints.len() == 2 { req.alternatives.clamp(1, 5) as usize } else { 1 };
    let mut cands: Vec<Candidate> = Vec::new();
    let (base, snaps) = compute_base(g, &prof, &snap_cands)
        .ok_or_else(|| anyhow::anyhow!(if prof.avoids_areas() { "NO_ROUTE_AVOID_AREAS" } else { "NO_ROUTE" }))?;
    let best_duration = base.duration;
    log::debug!("base: {:?}", t0.elapsed());
    cands.push(base);

    if k > 1 {
        let mut penalty = take_penalty(g.num_edges());
        let mut touched: Vec<u32> = Vec::new();
        let mut sets: Vec<FxHashSet<u32>> = vec![edge_set(&cands[0])];
        for e in &sets[0] {
            penalty[*e as usize] = PENALTY;
            touched.push(*e);
        }
        let mut attempts = 0;
        while cands.len() < k && attempts < k * 2 {
            attempts += 1;
            let Some(alt) = compute(g, &prof, &snaps, Some(&penalty)) else { break };
            let alt_set = edge_set(&alt);
            // penalize for next round regardless of acceptance (diversify)
            for e in &alt_set {
                penalty[*e as usize] *= PENALTY;
                touched.push(*e);
            }
            if alt.duration > best_duration * MAX_STRETCH {
                continue;
            }
            if sets.iter().any(|s| shared_fraction(g, &alt, s) > MAX_SHARE) {
                continue;
            }
            sets.push(alt_set);
            cands.push(alt);
        }
        give_penalty(penalty, &touched);
    }

    log::debug!("alternatives: {:?}", t0.elapsed());
    // tags
    let fastest = cands.iter().enumerate().min_by(|a, b| a.1.duration.total_cmp(&b.1.duration)).map(|x| x.0);
    let shortest = cands.iter().enumerate().min_by(|a, b| a.1.distance.total_cmp(&b.1.distance)).map(|x| x.0);

    let kind = leg_kind(req.mode);
    let mode_name = match req.mode {
        Mode::Car => "car",
        Mode::Bike => "bike",
        Mode::Foot => "foot",
        Mode::Transit => "transit",
    };
    let mut routes = Vec::with_capacity(cands.len());
    for (i, c) in cands.iter().enumerate() {
        let (depart_iso, arrive_iso) = resolve_times(&req.time, c.duration);
        let mut legs = Vec::with_capacity(c.paths.len());
        let mut geometry: Vec<[f64; 2]> = Vec::new();
        for (p, d) in c.paths.iter().zip(c.details.iter()) {
            let steps = instructions::build_steps(g, &prof, &p.states, d, if req.mode == Mode::Foot { 15 } else { 5 });
            if geometry.is_empty() {
                geometry.extend_from_slice(&d.geometry);
            } else {
                geometry.extend_from_slice(&d.geometry[1.min(d.geometry.len())..]);
            }
            legs.push(Leg {
                kind,
                duration_sec: d.duration_sec,
                distance_m: d.distance_m,
                geometry: d.geometry.clone(),
                depart_iso: None,
                arrive_iso: None,
                steps,
                transit: None,
            });
        }
        let mut tags = Vec::new();
        if Some(i) == fastest {
            tags.push(RouteTag::Fastest);
        }
        if Some(i) == shortest {
            tags.push(RouteTag::Shortest);
        }
        routes.push(Route {
            id: format!("{mode_name}-{i}"),
            mode: req.mode,
            duration_sec: c.duration,
            distance_m: c.distance,
            geometry,
            legs,
            tags,
            depart_iso,
            arrive_iso,
            transfers: 0,
            walk_m: if req.mode == Mode::Foot { c.distance } else { 0.0 },
        });
    }
    log::debug!("assembled: {:?}", t0.elapsed());
    Ok(RouteResponse::offline(routes, t0.elapsed().as_millis() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_depart_arrive() {
        let t = TimeSpec { kind: TimeKind::Depart, iso: Some("2026-11-01T10:00:00+02:00".into()) };
        let (d, a) = resolve_times(&t, 3600.0);
        assert_eq!(d.as_deref(), Some("2026-11-01T10:00:00+02:00"));
        assert_eq!(a.as_deref(), Some("2026-11-01T11:00:00+02:00"));
        let t = TimeSpec { kind: TimeKind::Arrive, iso: Some("2026-11-01T10:00:00+02:00".into()) };
        let (d, a) = resolve_times(&t, 1800.0);
        assert_eq!(d.as_deref(), Some("2026-11-01T09:30:00+02:00"));
        assert_eq!(a.as_deref(), Some("2026-11-01T10:00:00+02:00"));
        let (d, a) = resolve_times(&TimeSpec::default(), 60.0);
        assert!(d.is_some() && a.is_some());
    }
}
