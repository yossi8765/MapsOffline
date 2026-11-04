//! Road-network routing: graph build from OSM PBF, A*, alternatives, turn instructions.
//! Public API used by `commands.rs` and `data/`. Implemented in submodules.

pub mod alternatives;
pub mod avoid;
pub mod iso;
pub mod matching;
pub mod order;
pub mod areas;
pub mod astar;
pub mod graph;
pub mod instructions;
pub mod profile;

pub use graph::Graph;

use std::path::Path;

use crate::types::{Leg, LonLat, ProgressFn, RouteRequest, RouteResponse};

/// Build `graph.bin` from an OSM PBF extract (long running, call from spawn_blocking).
pub fn build_graph(pbf: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<()> {
    graph::build(pbf, out, progress)
}

/// Load a previously built graph.
pub fn load_graph(path: &Path) -> anyhow::Result<Graph> {
    graph::load(path)
}

/// Walking time (seconds) from `origin` to each of `targets` over the foot network – one search
/// for all of them – or None when farther than `max_sec` / off the network (transit access).
pub fn walk_times_from(graph: &Graph, origin: LonLat, targets: &[LonLat], max_sec: f32) -> Vec<Option<f32>> {
    iso::walk_times_from(graph, origin, targets, max_sec)
}

impl Graph {
    /// Route for car/foot/bike with up to `req.alternatives` alternatives.
    pub fn route(&self, req: &RouteRequest) -> anyhow::Result<RouteResponse> {
        alternatives::route_with_alternatives(self, req)
    }

    /// Walking leg between two points (used for transit access/egress). None if unreachable.
    pub fn walk(&self, from: LonLat, to: LonLat) -> Option<Leg> {
        astar::walk_leg(self, from, to)
    }

    /// Kinds of restricted territory (areas::KIND_A | KIND_B) a route geometry passes through.
    pub fn route_area_hits(&self, geometry: &[LonLat]) -> u8 {
        self.areas.route_hits(geometry)
    }

    /// Remove routes from an online response that enter territories the request avoids (online
    /// services cannot avoid polygons). Returns how many routes were dropped. Transit responses
    /// are left alone.
    pub fn drop_routes_in_avoided_areas(&self, req: &RouteRequest, resp: &mut RouteResponse) -> usize {
        let want = areas::avoid_bits(&req.avoid);
        if want == 0 || req.mode == crate::types::Mode::Transit || self.areas.is_empty() {
            return 0;
        }
        let before = resp.routes.len();
        resp.routes.retain(|r| self.areas.route_hits(&r.geometry) & want == 0);
        before - resp.routes.len()
    }
}

impl crate::transit::WalkRouter for Graph {
    fn walk(&self, from: LonLat, to: LonLat) -> Option<Leg> {
        Graph::walk(self, from, to)
    }
}
