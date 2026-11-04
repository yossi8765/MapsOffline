//! Public transport: GTFS → compact model, RAPTOR (depart-at / arrive-by), journey assembly.

pub mod board;
pub mod fares;
pub mod journey;
pub mod model;
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

/// Build `transit.bin` from the GTFS zip (long running).
pub fn build(gtfs_zip: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<()> {
    model::build(gtfs_zip, out, progress)
}

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
}
