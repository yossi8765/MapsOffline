//! Public transport: GTFS → compact model, RAPTOR (depart-at / arrive-by), journey assembly.

pub(crate) mod binfmt;
pub mod board;
pub mod fares;
pub mod holidays;
pub mod isochrone;
pub mod journey;
#[cfg(feature = "cli")]
pub mod legacy;
pub mod model;
pub mod notice;
pub mod raptor;
#[cfg(test)]
mod tests;

use std::path::Path;

pub use model::Transit;

use crate::types::{GeoResult, Leg, LonLat, ProgressFn, RouteRequest, RouteResponse};

/// Provides walking legs on the road network (implemented by `routing::Graph`).
pub trait WalkRouter: Sync + Send {
    fn walk(&self, from: LonLat, to: LonLat) -> Option<Leg>;
    /// Straight-line fallback used when no graph is loaded.
    fn walk_or_straight(&self, from: LonLat, to: LonLat) -> Leg {
        self.walk(from, to).unwrap_or_else(|| straight_walk(from, to))
    }
}

/// Straight-line walking leg at 5 km/h.
pub fn straight_walk(from: LonLat, to: LonLat) -> Leg {
    let d = crate::geo::haversine_m(from, to);
    Leg {
        kind: crate::types::LegKind::Walk,
        duration_sec: d / (5000.0 / 3600.0),
        distance_m: d,
        geometry: vec![from, to],
        depart_iso: None,
        arrive_iso: None,
        steps: vec![],
        transit: None,
    }
}

pub struct NoWalk;
impl WalkRouter for NoWalk {
    fn walk(&self, _from: LonLat, _to: LonLat) -> Option<Leg> {
        None
    }
}

/// Build `transit.bin` from the GTFS zip (long running), stop-to-stop transfers in straight lines.
pub fn build(gtfs_zip: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<()> {
    model::build(gtfs_zip, out, progress)
}

/// Build `transit.bin` with the stop-to-stop transfers walked on the road network of `graph`
/// (when given). Every served stop's ≤ 30 neighbours within 400 m are walked once, in parallel.
pub fn build_with_graph(gtfs_zip: &Path, out: &Path, progress: ProgressFn, graph: Option<&crate::routing::Graph>) -> anyhow::Result<()> {
    match graph {
        Some(g) => {
            let walk = move |o: LonLat, targets: &[LonLat]| walk_times_from(g, o, targets, TRANSFER_WALK_MAX_S);
            model::build_with(gtfs_zip, out, progress, Some(&walk))
        }
        None => model::build_with(gtfs_zip, out, progress, None),
    }
}

/// Longest stop-to-stop walk looked for on the road network (a 400 m transfer that needs more
/// than this keeps its straight-line estimate).
pub const TRANSFER_WALK_MAX_S: f32 = 1800.0;

/// Walking seconds from `origin` to each target on the road network, None when there is no way
/// within `max_sec`.
/// Walking seconds from `origin` to each target over the road network (one-to-many foot
/// Dijkstra in routing/); None = farther than `max_sec` or unreachable.
pub fn walk_times_from(graph: &crate::routing::Graph, origin: LonLat, targets: &[LonLat], max_sec: f32) -> Vec<Option<f32>> {
    crate::routing::walk_times_from(graph, origin, targets, max_sec)
}

pub use notice::attach_notice;

pub fn load(path: &Path) -> anyhow::Result<Transit> {
    model::load(path)
}

impl Transit {
    pub fn route(&self, req: &RouteRequest, walker: &dyn WalkRouter) -> anyhow::Result<RouteResponse> {
        journey::plan(self, req, walker)
    }

    /// Stops whose name matches `query` (for the search box).
    pub fn search_stops(&self, query: &str, near: Option<LonLat>, limit: usize) -> Vec<GeoResult> {
        model::search_stops(self, query, near, limit)
    }

    /// Everywhere reachable from `origin` by transit + walking within each of `minutes`.
    pub fn isochrone(&self, origin: LonLat, time: &crate::types::TimeSpec, minutes: &[u32], accessible: bool) -> anyhow::Result<Vec<crate::types::IsochroneBand>> {
        isochrone::transit_isochrone(self, origin, time, minutes, accessible)
    }
}
