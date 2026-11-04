//! One-to-many searches on the road graph: isochrones ("how far in 10 / 20 / 30 minutes"),
//! walking times to many targets (transit access), and travel-time matrices (stop order).
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::geo::{douglas_peucker, haversine_m};
use crate::routing::astar::dijkstra_from;
use crate::routing::graph::{Graph, Snap};
use crate::routing::profile::{Profile, FOOT_SPEED};
use crate::types::{IsochroneBand, LonLat, Mode};

/// Farthest a point may be from the road network to start / end a search (m).
const MAX_SNAP_M: f64 = 1_000.0;

/// Walking time (seconds) from `origin` to each target over the foot network, None when farther
/// than `max_sec` or off the network. One Dijkstra for all targets (transit access legs).
pub fn walk_times_from(g: &Graph, origin: LonLat, targets: &[LonLat], max_sec: f32) -> Vec<Option<f32>> {
    let prof = Profile::new(Mode::Foot, Default::default());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let Some(os) = g.snap(origin, &allowed).filter(|s| s.dist_m <= 500.0) else { return vec![None; targets.len()] };
    let off0 = (os.dist_m / FOOT_SPEED as f64) as f32;
    let reach = dijkstra_from(g, &prof, &os, (max_sec - off0).max(0.0), true, None);
    let at: FxHashMap<u32, f32> = reach.states.into_iter().collect();
    targets
        .iter()
        .map(|&t| {
            let ts = g.snap(t, &allowed).filter(|s| s.dist_m <= 500.0)?;
            let off1 = (ts.dist_m / FOOT_SPEED as f64) as f32;
            let mut best = f32::INFINITY;
            for dir in 0..2u32 {
                let s = ts.edge * 2 + dir;
                if let Some(&head) = at.get(&s) {
                    let w = prof.traverse_time(g, s);
                    let f = if dir == 0 { ts.frac } else { 1.0 - ts.frac };
                    let t_point = head - w + f * w;
                    if t_point >= -0.5 {
                        best = best.min(t_point.max(0.0));
                    }
                }
            }
            // origin and target on the same edge, target behind: handled by the reverse state
            let total = off0 + best + off1;
            (total.is_finite() && total <= max_sec).then_some(total)
        })
        .collect()
}

/// Travel-time matrix (seconds, real time) between snapped points: one Dijkstra per row,
/// stopped once every target is settled; rows in parallel. Unreachable = infinity.
pub fn matrix(g: &Graph, prof: &Profile, snaps: &[Snap]) -> Vec<Vec<f64>> {
    let n = snaps.len();
    let target_states: FxHashSet<u32> = snaps.iter().flat_map(|s| [s.edge * 2, s.edge * 2 + 1]).filter(|&s| prof.allowed(g, s)).collect();
    (0..n)
        .into_par_iter()
        .map(|a| {
            let reach = dijkstra_from(g, prof, &snaps[a], f32::INFINITY, true, Some(&target_states));
            let at: FxHashMap<u32, f32> = reach.states.into_iter().collect();
            (0..n)
                .map(|b| {
                    if a == b {
                        return 0.0;
                    }
                    let t = &snaps[b];
                    let mut best = f64::INFINITY;
                    for dir in 0..2u32 {
                        let s = t.edge * 2 + dir;
                        if let Some(&head) = at.get(&s) {
                            let w = prof.traverse_time(g, s);
                            let f = if dir == 0 { t.frac } else { 1.0 - t.frac };
                            let v = head - w + f * w;
                            if v >= -0.5 {
                                best = best.min(v.max(0.0) as f64);
                            }
                        }
                    }
                    best
                })
                .collect()
        })
        .collect()
}

/// Isochrone bands around `origin` for `minutes` (any order), using real travel times.
pub fn isochrone(g: &Graph, origin: LonLat, mode: Mode, minutes: &[f64]) -> anyhow::Result<Vec<IsochroneBand>> {
    let mut mins: Vec<f64> = minutes.iter().copied().filter(|m| m.is_finite() && *m > 0.0).collect();
    mins.sort_by(|a, b| a.total_cmp(b));
    mins.dedup();
    anyhow::ensure!(!mins.is_empty(), "NO_ISOCHRONE_MINUTES");
    let max_min = *mins.last().unwrap();
    anyhow::ensure!(max_min <= 180.0, "ISOCHRONE_TOO_LONG");
    let prof = Profile::new(mode, Default::default());
    let allowed = |e: u32| prof.allowed_edge_any(g, e);
    let os = g.snap(origin, &allowed).filter(|s| s.dist_m <= MAX_SNAP_M).ok_or_else(|| anyhow::anyhow!("NO_ROAD_NEAR_WAYPOINT"))?;
    let max_sec = (max_min * 60.0) as f32;
    let reach = dijkstra_from(g, &prof, &os, max_sec, true, None);

    // sample points along the reached edges with their arrival time
    let o = g.proj(origin[0], origin[1]);
    let mut samples: Vec<([f32; 2], f32)> = vec![(o, 0.0)];
    let mut radius = 1.0f32;
    for &(s, head) in &reach.states {
        let w = prof.traverse_time(g, s);
        let tail = head - w;
        let pts = g.state_points(s);
        let total: f64 = pts.windows(2).map(|x| haversine_m(x[0], x[1])).sum();
        if total <= 0.0 {
            continue;
        }
        let step = 40.0f64;
        let mut acc = 0.0;
        for win in pts.windows(2) {
            let seg = haversine_m(win[0], win[1]);
            let k = (seg / step).ceil().max(1.0) as usize;
            for j in 0..=k {
                let x = (acc + seg * j as f64 / k as f64) / total;
                let t = tail + x as f32 * w;
                if t < 0.0 || t > max_sec {
                    continue;
                }
                let p = [win[0][0] + (win[1][0] - win[0][0]) * j as f64 / k as f64, win[0][1] + (win[1][1] - win[0][1]) * j as f64 / k as f64];
                let q = g.proj(p[0], p[1]);
                radius = radius.max(((q[0] - o[0]).powi(2) + (q[1] - o[1]).powi(2)).sqrt());
                samples.push((q, t));
            }
            acc += seg;
        }
    }
    // raster: min arrival time per cell
    let cell = (radius / 150.0).clamp(40.0, 400.0);
    let (mut minx, mut miny, mut maxx, mut maxy) = (o[0], o[1], o[0], o[1]);
    for (q, _) in &samples {
        minx = minx.min(q[0]);
        miny = miny.min(q[1]);
        maxx = maxx.max(q[0]);
        maxy = maxy.max(q[1]);
    }
    let pad = 2.0 * cell;
    let (x0, y0) = (minx - pad, miny - pad);
    let nx = (((maxx + pad - x0) / cell).ceil() as usize).max(1);
    let ny = (((maxy + pad - y0) / cell).ceil() as usize).max(1);
    let mut t = vec![f32::INFINITY; nx * ny];
    for (q, tt) in &samples {
        let (i, j) = (((q[0] - x0) / cell) as usize, ((q[1] - y0) / cell) as usize);
        if i < nx && j < ny {
            let c = &mut t[j * nx + i];
            *c = c.min(*tt);
        }
    }
    // one cell of buffer around the reached roads (the band is "this close to a reached road")
    let mut tb = t.clone();
    for j in 0..ny {
        for i in 0..nx {
            let mut m = t[j * nx + i];
            for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                let (a, b) = (i as i64 + di, j as i64 + dj);
                if a >= 0 && b >= 0 && (a as usize) < nx && (b as usize) < ny {
                    m = m.min(t[b as usize * nx + a as usize]);
                }
            }
            tb[j * nx + i] = m;
        }
    }
    let mut bands = Vec::with_capacity(mins.len());
    for &mm in &mins {
        let lim = (mm * 60.0) as f32;
        let filled: Vec<bool> = tb.iter().map(|&x| x <= lim).collect();
        let rings = trace_rings(&filled, nx, ny);
        let mut out: Vec<Vec<LonLat>> = Vec::new();
        for r in order_rings(rings) {
            let pts: Vec<[f64; 2]> = r.iter().map(|&(i, j)| [(x0 + i as f32 * cell) as f64, (y0 + j as f32 * cell) as f64]).collect();
            let simple = douglas_peucker(&pts, cell as f64 * 0.5);
            if simple.len() < 4 {
                continue;
            }
            out.push(simple.iter().map(|p| g.unproj(p[0] as f32, p[1] as f32)).collect());
        }
        bands.push(IsochroneBand { minutes: mm, rings: out });
    }
    Ok(bands)
}

/// Closed boundary rings (grid vertex coordinates) of the filled cells: outer rings
/// counter-clockwise, holes clockwise (filled area on the left).
pub fn trace_rings(filled: &[bool], nx: usize, ny: usize) -> Vec<Vec<(i32, i32)>> {
    let at = |i: i64, j: i64| i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny && filled[j as usize * nx + i as usize];
    // directed boundary edges, keyed by start vertex
    let mut out: FxHashMap<(i32, i32), Vec<(i32, i32)>> = FxHashMap::default();
    let mut count = 0usize;
    for j in 0..ny as i64 {
        for i in 0..nx as i64 {
            if !at(i, j) {
                continue;
            }
            let (a, b) = (i as i32, j as i32);
            if !at(i, j - 1) {
                out.entry((a, b)).or_default().push((a + 1, b));
                count += 1;
            }
            if !at(i + 1, j) {
                out.entry((a + 1, b)).or_default().push((a + 1, b + 1));
                count += 1;
            }
            if !at(i, j + 1) {
                out.entry((a + 1, b + 1)).or_default().push((a, b + 1));
                count += 1;
            }
            if !at(i - 1, j) {
                out.entry((a, b + 1)).or_default().push((a, b));
                count += 1;
            }
        }
    }
    let mut rings = Vec::new();
    let mut used = 0usize;
    while used < count {
        // any remaining start
        let Some((&start, _)) = out.iter().find(|(_, v)| !v.is_empty()) else { break };
        let mut ring = vec![start];
        let mut cur = start;
        let mut prev_dir = (0i32, 0i32);
        loop {
            let nexts = out.get_mut(&cur).unwrap();
            // at a saddle (two ways out) turn left: keeps diagonal cells apart
            let k = if nexts.len() > 1 {
                let left = (-prev_dir.1, prev_dir.0);
                nexts.iter().position(|n| (n.0 - cur.0, n.1 - cur.1) == left).unwrap_or(0)
            } else {
                0
            };
            let nxt = nexts.swap_remove(k);
            used += 1;
            prev_dir = (nxt.0 - cur.0, nxt.1 - cur.1);
            cur = nxt;
            if cur == start {
                break;
            }
            ring.push(cur);
            if ring.len() > count + 1 {
                break; // defensive
            }
        }
        // drop collinear vertices
        let n = ring.len();
        let simple: Vec<(i32, i32)> = (0..n)
            .filter(|&k| {
                let (a, b, c) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
                (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0) != 0
            })
            .map(|k| ring[k])
            .collect();
        if simple.len() >= 3 {
            let mut closed = simple;
            closed.push(closed[0]);
            rings.push(closed);
        }
    }
    rings
}

fn signed_area(r: &[(i32, i32)]) -> f64 {
    r.windows(2).map(|w| (w[0].0 as f64) * (w[1].1 as f64) - (w[1].0 as f64) * (w[0].1 as f64)).sum::<f64>() / 2.0
}

fn contains(r: &[(i32, i32)], p: (f64, f64)) -> bool {
    let mut odd = false;
    for w in r.windows(2) {
        let (a, b) = ((w[0].0 as f64, w[0].1 as f64), (w[1].0 as f64, w[1].1 as f64));
        if (a.1 > p.1) != (b.1 > p.1) && a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) > p.0 {
            odd = !odd;
        }
    }
    odd
}

/// Outer rings (largest first), each followed by the holes inside it.
fn order_rings(rings: Vec<Vec<(i32, i32)>>) -> Vec<Vec<(i32, i32)>> {
    let (mut outers, holes): (Vec<_>, Vec<_>) = rings.into_iter().partition(|r| signed_area(r) > 0.0);
    outers.sort_by(|a, b| signed_area(b).total_cmp(&signed_area(a)));
    let mut out = Vec::new();
    let mut used = vec![false; holes.len()];
    for o in outers {
        out.push(o.clone());
        for (k, h) in holes.iter().enumerate() {
            if !used[k] {
                // a point just inside the hole edge (cell centre to its left side)
                let p = (h[0].0 as f64 + 0.01, h[0].1 as f64 + 0.01);
                if contains(&o, p) {
                    used[k] = true;
                    out.push(h.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_of_a_block_with_a_hole() {
        // 4x4 filled with the centre 2x2 empty -> one outer ring + one hole
        let (nx, ny) = (4, 4);
        let mut f = vec![true; nx * ny];
        for (i, j) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            f[j * nx + i] = false;
        }
        let r = order_rings(trace_rings(&f, nx, ny));
        assert_eq!(r.len(), 2);
        assert!(signed_area(&r[0]) > 0.0 && (signed_area(&r[0]) - 16.0).abs() < 1e-9);
        assert!(signed_area(&r[1]) < 0.0 && (signed_area(&r[1]) + 4.0).abs() < 1e-9);
    }

    #[test]
    fn diagonal_cells_stay_apart() {
        let (nx, ny) = (2, 2);
        let f = vec![true, false, false, true];
        let r = trace_rings(&f, nx, ny);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|x| (signed_area(x) - 1.0).abs() < 1e-9));
    }
}
