//! Per-request forbidden edges: roads the user asked to avoid ("avoid this road"), areas drawn
//! on the map, and – for trucks – edges whose maxheight / maxweight / maxwidth the vehicle
//! exceeds. The result is a bitset over edge ids carried by the [`Profile`].
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::routing::graph::{flags, Graph};
use crate::routing::profile::Profile;
use crate::types::{LonLat, Mode, RouteRequest};

/// Longest stretch (m) of one road that "avoid this road" forbids around the chosen point.
const AVOID_ROAD_M: f64 = 3_000.0;

struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }
    fn set(&mut self, e: u32) {
        self.0[(e >> 6) as usize] |= 1u64 << (e & 63);
    }
    #[cfg(test)]
    fn get(&self, e: u32) -> bool {
        self.0[(e >> 6) as usize] & (1u64 << (e & 63)) != 0
    }
    fn any(&self) -> bool {
        self.0.iter().any(|&w| w != 0)
    }
}

impl Profile {
    /// The profile of a request: mode, avoid options, per-request forbidden edges, waypoints
    /// (for `access=destination`) and truck dimensions.
    pub fn for_request(g: &Graph, req: &RouteRequest) -> Profile {
        let mut p = Profile::new(req.mode, req.avoid.clone());
        p.vehicle = req.vehicle;
        p.free_points = req.waypoints.iter().map(|w| g.proj(w.lon, w.lat)).collect();
        p.forbid_edges = forbidden_edges(g, req).map(Arc::new);
        p
    }
}

/// Bitset of edges the request must not use, or None when there are none.
pub fn forbidden_edges(g: &Graph, req: &RouteRequest) -> Option<Vec<u64>> {
    let a = &req.avoid;
    let truck = req.mode == Mode::Truck && req.vehicle.is_some();
    if a.points.is_empty() && a.polygons.is_empty() && !truck {
        return None;
    }
    let mut bits = Bits::new(g.num_edges());
    for &p in &a.points {
        for e in road_around(g, p, req.mode) {
            bits.set(e);
        }
    }
    for ring in &a.polygons {
        for e in edges_in_polygon(g, ring) {
            bits.set(e);
        }
    }
    if truck {
        let v = req.vehicle.unwrap_or_default();
        for (i, &e) in g.limits.edge.iter().enumerate() {
            let h = g.limits.height_dm[i] as f64 * 0.1;
            let w = g.limits.weight_ht[i] as f64 * 0.5;
            let wd = g.limits.width_dm[i] as f64 * 0.1;
            let too_high = h > 0.0 && v.height_m.is_some_and(|x| x > h);
            let too_heavy = w > 0.0 && v.weight_t.is_some_and(|x| x > w);
            let too_wide = wd > 0.0 && v.width_m.is_some_and(|x| x > wd);
            if too_high || too_heavy || too_wide {
                bits.set(e);
            }
        }
    }
    // a waypoint on a forbidden edge must stay reachable: free the edges it snaps to
    if bits.any() {
        Some(bits.0)
    } else {
        None
    }
}

/// Edges of the road nearest `p`: the snapped edge plus the connected edges with the same name
/// (or, unnamed, the same class through simple junctions), up to [`AVOID_ROAD_M`] each way.
pub fn road_around(g: &Graph, p: LonLat, mode: Mode) -> Vec<u32> {
    let prof = Profile::new(mode, Default::default());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let Some(snap) = g.snap(p, &allowed) else { return Vec::new() };
    if snap.dist_m > 200.0 {
        return Vec::new();
    }
    let e0 = snap.edge;
    let name = g.e_name[e0 as usize];
    let class = g.e_class[e0 as usize];
    let same = |e: u32| -> bool {
        if name != crate::routing::graph::NO_NAME {
            g.e_name[e as usize] == name
        } else {
            g.e_class[e as usize] == class && g.e_flags[e as usize] & flags::ROUNDABOUT == 0
        }
    };
    let mut out = vec![e0];
    let mut seen: FxHashSet<u32> = FxHashSet::default();
    seen.insert(e0);
    // walk both directions from the snapped edge
    for start in [g.e_from[e0 as usize], g.e_to[e0 as usize]] {
        let mut frontier = vec![(start, 0.0f64)];
        while let Some((v, d)) = frontier.pop() {
            if d > AVOID_ROAD_M {
                continue;
            }
            let cand: Vec<u32> = g.incident(v).iter().copied().filter(|&e| !seen.contains(&e) && same(e)).collect();
            // unnamed roads: only follow simple continuations
            if name == crate::routing::graph::NO_NAME && cand.len() > 1 {
                continue;
            }
            for e in cand {
                seen.insert(e);
                out.push(e);
                let other = if g.e_from[e as usize] == v { g.e_to[e as usize] } else { g.e_from[e as usize] };
                frontier.push((other, d + g.e_len[e as usize] as f64));
            }
        }
    }
    out
}

/// Edges with a point inside the ring or a segment crossing it.
pub fn edges_in_polygon(g: &Graph, ring: &[LonLat]) -> Vec<u32> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let pr: Vec<[f32; 2]> = ring.iter().map(|p| g.proj(p[0], p[1])).collect();
    let (mut min, mut max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in &pr {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    let mut cand = Vec::new();
    g.grid.query(min, max, &mut cand);
    let n = pr.len();
    let inside = |q: [f32; 2]| -> bool {
        let mut odd = false;
        for i in 0..n {
            let (a, b) = (pr[i], pr[(i + 1) % n]);
            if (a[1] > q[1]) != (b[1] > q[1]) {
                let x = a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                if x > q[0] {
                    odd = !odd;
                }
            }
        }
        odd
    };
    let cross = |p1: [f32; 2], p2: [f32; 2]| -> bool {
        let o = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        (0..n).any(|i| {
            let (a, b) = (pr[i], pr[(i + 1) % n]);
            let (d1, d2) = (o(a, b, p1), o(a, b, p2));
            let (d3, d4) = (o(p1, p2, a), o(p1, p2, b));
            (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
        })
    };
    cand.into_iter()
        .filter(|&e| {
            let pts: Vec<[f32; 2]> = g.edge_points(e).iter().map(|p| g.proj(p[0], p[1])).collect();
            pts.iter().any(|&q| inside(q)) || pts.windows(2).any(|w| cross(w[0], w[1]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits() {
        let mut b = Bits::new(130);
        assert!(!b.any());
        b.set(129);
        assert!(b.get(129) && !b.get(128) && b.any());
    }
}
