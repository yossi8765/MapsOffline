//! Small geo helpers shared by all modules.
use crate::types::LonLat;

pub const EARTH_R: f64 = 6_371_008.8;

pub fn haversine_m(a: LonLat, b: LonLat) -> f64 {
    let (lon1, lat1) = (a[0].to_radians(), a[1].to_radians());
    let (lon2, lat2) = (b[0].to_radians(), b[1].to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_R * h.sqrt().asin()
}

/// Initial bearing in degrees [0,360).
pub fn bearing_deg(a: LonLat, b: LonLat) -> f64 {
    let (lon1, lat1) = (a[0].to_radians(), a[1].to_radians());
    let (lon2, lat2) = (b[0].to_radians(), b[1].to_radians());
    let dlon = lon2 - lon1;
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Length of a polyline in meters.
pub fn polyline_length_m(pts: &[LonLat]) -> f64 {
    pts.windows(2).map(|w| haversine_m(w[0], w[1])).sum()
}

pub fn in_bbox(p: LonLat, bbox: [f64; 4]) -> bool {
    p[0] >= bbox[0] && p[0] <= bbox[2] && p[1] >= bbox[1] && p[1] <= bbox[3]
}

/// Local equirectangular projection around `o`: metres east / north of it.
#[inline]
pub fn local_proj(o: LonLat, p: LonLat) -> (f64, f64) {
    let k = 111_320.0;
    ((p[0] - o[0]) * k * o[1].to_radians().cos(), (p[1] - o[1]) * k)
}

/// Distance (m) from `p` to segment a-b, and the fraction along a-b of the closest point.
pub fn point_segment_m(p: LonLat, a: LonLat, b: LonLat) -> (f64, f64) {
    let (bx, by) = local_proj(a, b);
    let (px, py) = local_proj(a, p);
    let len2 = bx * bx + by * by;
    let t = if len2 > 0.0 { ((px * bx + py * by) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (px - bx * t, py - by * t);
    ((dx * dx + dy * dy).sqrt(), t)
}

/// Douglas–Peucker simplification of a planar polyline (same units as `tol`). Keeps the first
/// and last point, so a closed ring stays closed.
pub fn douglas_peucker(pts: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (pts[a], pts[b]);
        let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
        let l2 = dx * dx + dy * dy;
        let mut best = (0.0f64, a);
        for (i, r) in pts.iter().enumerate().take(b).skip(a + 1) {
            let t = if l2 > 0.0 { (((r[0] - p[0]) * dx + (r[1] - p[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = ((r[0] - p[0] - t * dx).powi(2) + (r[1] - p[1] - t * dy).powi(2)).sqrt();
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dp_and_segment() {
        let line = [[0.0, 0.0], [1.0, 0.1], [2.0, 0.0], [3.0, 5.0], [4.0, 0.0]];
        let s = douglas_peucker(&line, 0.5);
        assert_eq!(s, vec![[0.0, 0.0], [2.0, 0.0], [3.0, 5.0], [4.0, 0.0]]);
        let (d, t) = point_segment_m([34.0, 32.001], [34.0, 32.0], [34.01, 32.0]);
        assert!((d - 111.3).abs() < 1.0 && t.abs() < 1e-9, "{d} {t}");
    }
}
