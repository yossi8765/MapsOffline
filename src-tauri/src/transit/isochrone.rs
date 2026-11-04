//! Transit isochrones: everywhere reachable from a point within N minutes by walking and public
//! transport. One-to-all RAPTOR gives the arrival time at every stop; each reached stop then
//! contributes a walking circle with the time left, as does the origin itself. The union of the
//! circles is rasterised (≈100 m cells) and its outline traced into rings (outer boundaries
//! only – holes are filled, an isochrone shows where one can get).

use anyhow::{bail, Result};
use rustc_hash::FxHashMap;

use super::journey::resolve_time;
use super::model::Transit;
use super::raptor::{one_to_all, MAX_TRIPS};
use crate::types::{IsochroneBand, LonLat, TimeSpec};

/// Walking speed with a detour factor (5 km/h on roads ≈ 1.25 × the straight line).
const WALK_MPS: f64 = 5000.0 / 3600.0 / 1.25;
/// Longest walk to the first stop.
const ACCESS_M: f64 = 1200.0;
/// Longest walk from a stop counted as reachable around it.
const EGRESS_M: f64 = 1000.0;
/// Longest walk from the origin itself.
const ORIGIN_M: f64 = 3000.0;
const MAX_MINUTES: u32 = 180;
const MAX_CELLS: f64 = 2_500_000.0;

pub fn transit_isochrone(t: &Transit, origin: LonLat, time: &TimeSpec, minutes: &[u32], accessible: bool) -> Result<Vec<IsochroneBand>> {
    let mut mins: Vec<u32> = minutes.iter().copied().filter(|&m| m > 0).map(|m| m.min(MAX_MINUTES)).collect();
    mins.sort_unstable();
    mins.dedup();
    if mins.is_empty() {
        return Ok(vec![]);
    }
    let (date, secs, _) = resolve_time(time)?;
    let Some(day) = t.day_index(date) else { bail!("NO_TRANSIT_SERVICE_ON_DATE") };
    let max_s = *mins.last().unwrap() as i32 * 60;
    let reach = (max_s as f64 * WALK_MPS).min(ACCESS_M);
    let sources: Vec<(u32, i32)> = t
        .stops_near(origin, reach)
        .into_iter()
        .filter(|(s, _)| !accessible || t.stop_accessible(*s))
        .map(|(s, d)| (s, secs + (d / WALK_MPS).ceil() as i32))
        .collect();
    let arr = one_to_all(t, day, &sources, MAX_TRIPS, accessible, max_s);
    let mut out = Vec::with_capacity(mins.len());
    for m in mins {
        let budget = secs + m as i32 * 60;
        let mut circles: Vec<(LonLat, f64)> = vec![(origin, (m as f64 * 60.0 * WALK_MPS).min(ORIGIN_M))];
        for (s, &a) in arr.iter().enumerate() {
            if a < budget {
                let r = ((budget - a) as f64 * WALK_MPS).min(EGRESS_M);
                if r >= 30.0 {
                    circles.push((t.lonlat(s as u32), r));
                }
            }
        }
        out.push(IsochroneBand { minutes: m, rings: union_rings(&circles) });
    }
    Ok(out)
}

/// Outline of the union of circles (centre, radius m) as closed lon/lat rings.
pub fn union_rings(circles: &[(LonLat, f64)]) -> Vec<Vec<LonLat>> {
    if circles.is_empty() {
        return vec![];
    }
    let lat0 = circles[0].0[1];
    let mx = 111_320.0 * lat0.to_radians().cos().max(0.1);
    let my = 110_540.0;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let local: Vec<(f64, f64, f64)> = circles
        .iter()
        .map(|&(c, r)| {
            let (x, y) = (c[0] * mx, c[1] * my);
            x0 = x0.min(x - r);
            y0 = y0.min(y - r);
            x1 = x1.max(x + r);
            y1 = y1.max(y + r);
            (x, y, r)
        })
        .collect();
    let area = (x1 - x0) * (y1 - y0);
    let cell = (area / MAX_CELLS).sqrt().max(100.0);
    // one empty cell of margin on every side, so every outline is closed inside the grid
    let (gx0, gy0) = (x0 - cell, y0 - cell);
    let w = ((x1 - x0) / cell).ceil() as usize + 3;
    let h = ((y1 - y0) / cell).ceil() as usize + 3;
    let mut grid = vec![false; w * h];
    for &(cx, cy, r) in &local {
        let r2 = r * r;
        let ix0 = (((cx - r - gx0) / cell).floor().max(0.0)) as usize;
        let ix1 = (((cx + r - gx0) / cell).ceil() as usize).min(w - 1);
        let iy0 = (((cy - r - gy0) / cell).floor().max(0.0)) as usize;
        let iy1 = (((cy + r - gy0) / cell).ceil() as usize).min(h - 1);
        for iy in iy0..=iy1 {
            let py = gy0 + (iy as f64 + 0.5) * cell - cy;
            for ix in ix0..=ix1 {
                let px = gx0 + (ix as f64 + 0.5) * cell - cx;
                if px * px + py * py <= r2 {
                    grid[iy * w + ix] = true;
                }
            }
        }
    }
    trace(&grid, w, h)
        .into_iter()
        .map(|ring| ring.into_iter().map(|(x, y)| [(gx0 + x as f64 * cell) / mx, (gy0 + y as f64 * cell) / my]).collect())
        .collect()
}

/// Outer outlines of the covered cells of a `w × h` grid (row-major, y up) as closed rings of
/// cell-corner coordinates, counter-clockwise. Holes are dropped.
pub fn trace(grid: &[bool], w: usize, h: usize) -> Vec<Vec<(i32, i32)>> {
    let cov = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && grid[y as usize * w + x as usize];
    // directed boundary edges with the covered cell on the left
    let mut edges: Vec<((i32, i32), (i32, i32))> = Vec::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !cov(x, y) {
                continue;
            }
            let (xi, yi) = (x as i32, y as i32);
            if !cov(x, y - 1) {
                edges.push(((xi, yi), (xi + 1, yi)));
            }
            if !cov(x + 1, y) {
                edges.push(((xi + 1, yi), (xi + 1, yi + 1)));
            }
            if !cov(x, y + 1) {
                edges.push(((xi + 1, yi + 1), (xi, yi + 1)));
            }
            if !cov(x - 1, y) {
                edges.push(((xi, yi + 1), (xi, yi)));
            }
        }
    }
    let mut from: FxHashMap<(i32, i32), Vec<usize>> = FxHashMap::default();
    for (i, e) in edges.iter().enumerate() {
        from.entry(e.0).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut rings = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        let mut ring = vec![edges[start].0];
        let mut cur = start;
        used[cur] = true;
        loop {
            let (a, b) = edges[cur];
            let dir = (b.0 - a.0, b.1 - a.1);
            if b == edges[start].0 {
                break;
            }
            ring.push(b);
            // at a corner shared by two diagonal cells take the left turn (keeps them apart)
            let left = (-dir.1, dir.0);
            let next = from.get(&b).and_then(|v| {
                let free: Vec<usize> = v.iter().copied().filter(|&i| !used[i]).collect();
                free.iter().copied().find(|&i| (edges[i].1 .0 - b.0, edges[i].1 .1 - b.1) == left).or_else(|| free.first().copied())
            });
            match next {
                Some(n) => {
                    used[n] = true;
                    cur = n;
                }
                None => break,
            }
        }
        // signed area (shoelace): outer outlines are counter-clockwise
        let area: i64 = ring.iter().zip(ring.iter().cycle().skip(1)).map(|(p, q)| p.0 as i64 * q.1 as i64 - q.0 as i64 * p.1 as i64).sum();
        if area <= 0 || ring.len() < 4 {
            continue;
        }
        // drop vertices in the middle of straight runs, close the ring
        let n = ring.len();
        let mut simple: Vec<(i32, i32)> = (0..n)
            .filter(|&i| {
                let (p, c, q) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
                (c.0 - p.0) * (q.1 - c.1) != (c.1 - p.1) * (q.0 - c.0)
            })
            .map(|i| ring[i])
            .collect();
        if simple.len() >= 3 {
            simple.push(simple[0]);
            rings.push(simple);
        }
    }
    rings
}

/// Test helper: is `p` inside any ring (even-odd per ring)?
#[cfg(test)]
pub fn inside_any(rings: &[Vec<LonLat>], p: LonLat) -> bool {
    rings.iter().any(|r| {
        let mut c = false;
        for i in 0..r.len().saturating_sub(1) {
            let (a, b) = (r[i], r[i + 1]);
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
                c = !c;
            }
        }
        c
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_and_hole() {
        // 4x4 block with the middle 2x2 empty, plus a separate cell
        let (w, h) = (8, 6);
        let mut g = vec![false; w * h];
        for y in 1..5 {
            for x in 1..5 {
                g[y * w + x] = !(2..4).contains(&x) || !(2..4).contains(&y);
            }
        }
        g[w + 6] = true;
        let rings = trace(&g, w, h);
        assert_eq!(rings.len(), 2, "{rings:?}");
        let big = rings.iter().find(|r| r.len() == 5).expect("square outline");
        assert!(big.contains(&(1, 1)) && big.contains(&(5, 5)));
    }

    #[test]
    fn circles_union() {
        let a = [34.78, 32.08];
        let b = [34.79, 32.08]; // ~940 m east
        let rings = union_rings(&[(a, 600.0), (b, 600.0)]);
        assert_eq!(rings.len(), 1, "overlapping circles make one area");
        assert!(inside_any(&rings, a) && inside_any(&rings, b));
        assert!(!inside_any(&rings, [34.80, 32.09]));
        let apart = union_rings(&[(a, 300.0), ([34.82, 32.08], 300.0)]);
        assert_eq!(apart.len(), 2);
    }
}
