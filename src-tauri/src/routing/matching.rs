//! Map matching of a recorded / imported track (GPX): the track is thinned, each kept point
//! is snapped to the network and consecutive points are joined by the shortest route, so the
//! result is a regular route with turn-by-turn steps that live navigation can follow.
use crate::geo::haversine_m;
use crate::routing::alternatives::{leg_kind, resolve_times};
use crate::routing::astar::{self, PathDetails, PathResult, SearchParams};
use crate::routing::graph::{Graph, Snap};
use crate::routing::instructions;
use crate::routing::profile::Profile;
use crate::types::{Leg, LonLat, Mode, Route, TimeSpec};

/// Spacing of the points that are matched (m), and their maximum number.
const SPACING_M: f64 = 40.0;
const MAX_POINTS: usize = 400;
/// Track points farther than this from any usable road are ignored.
const MAX_OFFSET_M: f64 = 150.0;

/// Thin a track to points at least `spacing` metres apart (first and last kept).
pub fn thin(points: &[LonLat], spacing: f64) -> Vec<LonLat> {
    let mut out: Vec<LonLat> = Vec::new();
    for &p in points {
        if out.last().is_none_or(|&q| haversine_m(q, p) >= spacing) {
            out.push(p);
        }
    }
    if let (Some(&last), Some(&kept)) = (points.last(), out.last()) {
        if kept != last {
            out.push(last);
        }
    }
    out
}

/// Route along an imported track.
pub fn match_track(g: &Graph, points: &[LonLat], mode: Mode) -> anyhow::Result<Route> {
    anyhow::ensure!(points.len() >= 2, "NEED_TWO_WAYPOINTS");
    anyhow::ensure!(matches!(mode, Mode::Car | Mode::Foot | Mode::Bike), "MODE_NOT_SUPPORTED");
    let total: f64 = points.windows(2).map(|w| haversine_m(w[0], w[1])).sum();
    let spacing = SPACING_M.max(total / MAX_POINTS as f64);
    let pts = thin(points, spacing);
    let prof = Profile::new(mode, Default::default());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let snaps: Vec<Snap> = pts.iter().filter_map(|&p| g.snap(p, &allowed).filter(|s| s.dist_m <= MAX_OFFSET_M)).collect();
    anyhow::ensure!(snaps.len() >= 2, "NO_ROAD_NEAR_WAYPOINT");

    // join consecutive snaps; a point that cannot be reached (wrong carriageway, island) is skipped
    let params = SearchParams::new(&prof);
    let mut paths: Vec<PathResult> = Vec::new();
    let mut i = 0;
    while i + 1 < snaps.len() {
        let mut j = i + 1;
        let mut found = None;
        while j < snaps.len() && j <= i + 4 {
            if let Some(p) = astar::search(g, &params, &snaps[i], &snaps[j]) {
                found = Some(p);
                break;
            }
            j += 1;
        }
        match found {
            Some(p) => {
                paths.push(p);
                i = j;
            }
            None => i += 1,
        }
    }
    anyhow::ensure!(!paths.is_empty(), "NO_ROUTE");
    let (states, det) = merge_paths(g, &prof, &paths);
    let steps = instructions::build_steps(g, &prof, &states, &det, if mode == Mode::Foot { 15 } else { 5 });
    let (depart_iso, arrive_iso) = resolve_times(&TimeSpec::default(), det.duration_sec);
    let leg = Leg {
        kind: leg_kind(mode),
        duration_sec: det.duration_sec,
        distance_m: det.distance_m,
        geometry: det.geometry.clone(),
        depart_iso: None,
        arrive_iso: None,
        steps,
        transit: None,
    };
    Ok(Route {
        id: "track-0".into(),
        mode,
        duration_sec: det.duration_sec,
        distance_m: det.distance_m,
        geometry: det.geometry,
        legs: vec![leg],
        tags: vec![],
        depart_iso,
        arrive_iso,
        transfers: 0,
        walk_m: if mode == Mode::Foot { det.distance_m } else { 0.0 },
        fare_estimate_ils: None,
        fare_note: None,
        avoid_violations: vec![],
        fare_breakdown: vec![],
    })
}

/// One path out of several consecutive ones: a state shared at a joint (both ends of the joint
/// on the same edge) is merged into one, with its distance and time added up.
fn merge_paths(g: &Graph, prof: &Profile, paths: &[PathResult]) -> (Vec<u32>, PathDetails) {
    let mut states: Vec<u32> = Vec::new();
    let mut det = PathDetails::default();
    for p in paths {
        let d = astar::details(g, prof, p);
        for (k, &s) in p.states.iter().enumerate() {
            let start = d.state_starts[k];
            let end = d.state_starts.get(k + 1).copied().unwrap_or(d.geometry.len() - 1);
            let piece = &d.geometry[start..=end.min(d.geometry.len() - 1)];
            if k == 0 && states.last() == Some(&s) {
                // continue the previous state
                let last = det.state_dist.len() - 1;
                det.state_dist[last] += d.state_dist[k];
                det.state_dur[last] += d.state_dur[k];
                det.geometry.extend_from_slice(&piece[1.min(piece.len())..]);
            } else {
                det.state_starts.push(det.geometry.len().saturating_sub(1));
                if det.geometry.is_empty() {
                    det.geometry.extend_from_slice(piece);
                } else {
                    det.geometry.extend_from_slice(&piece[1.min(piece.len())..]);
                }
                states.push(s);
                det.state_dist.push(d.state_dist[k]);
                det.state_dur.push(d.state_dur[k]);
            }
            det.distance_m += d.state_dist[k];
            det.duration_sec += d.state_dur[k];
        }
    }
    (states, det)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinning_keeps_ends() {
        let pts: Vec<LonLat> = (0..100).map(|i| [34.0 + i as f64 * 1e-4, 32.0]).collect();
        let t = thin(&pts, 40.0);
        assert_eq!(t.first(), pts.first());
        assert_eq!(t.last(), pts.last());
        assert!(t.len() < 30 && t.len() > 15, "{}", t.len());
    }
}
