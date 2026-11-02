//! Restricted territories (Oslo Area A incl. Hebron H1 and the Gaza Strip as kind A, Area B as
//! kind B) as polygon rings taken from OSM boundary relations.
//!
//! The builder stitches relation member ways into closed rings, classifies every graph edge
//! (flags `AREA_A` / `AREA_B`) and stores the rings in `graph.bin`; at runtime the rings are
//! used to check routes that came from an online service, which cannot avoid polygons itself.
use crate::geo::haversine_m;
use crate::types::LonLat;

pub const KIND_A: u8 = 1;
pub const KIND_B: u8 = 2;

/// Build-time tolerance: a point this close to a ring boundary counts as *outside*. The Oslo
/// lines often run along roads that stay under Israeli control, so a road on the line is usable.
pub const EDGE_TOL_M: f64 = 10.0;
/// Runtime tolerance for online route geometries (slightly looser: simplified polylines).
pub const ROUTE_TOL_M: f64 = 8.0;
/// Segments longer than this get their midpoint sampled too.
const SAMPLE_STEP_M: f64 = 300.0;
const BANDS: usize = 2048;

/// Area kind of a boundary relation, by its name tags. `None` when the relation is not one of
/// the restricted territories.
pub fn kind_for_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Option<u8> {
    let mut is_boundary = false;
    let mut has_boundary_tag = false;
    let mut kind: Option<u8> = None;
    for (k, v) in tags {
        match k {
            "type" => is_boundary = v == "boundary",
            "boundary" => has_boundary_tag = true,
            "name:en" => {
                kind = kind.or(match v.trim() {
                    "Area A" | "Area H1" | "Gaza Strip" => Some(KIND_A),
                    "Area B" => Some(KIND_B),
                    _ => None,
                })
            }
            "name:he" => {
                kind = kind.or(match v.trim() {
                    "שטח A" | "אזור H1" | "רצועת עזה" => Some(KIND_A),
                    "שטח B" => Some(KIND_B),
                    _ => None,
                })
            }
            _ => {}
        }
    }
    if is_boundary && has_boundary_tag {
        kind
    } else {
        None
    }
}

/// Stitch way segments (each a list of 1e-7° lon/lat points) into closed rings by matching
/// endpoints. Returns the rings and the number of chains that could not be closed.
pub fn stitch(mut segs: Vec<Vec<[i32; 2]>>) -> (Vec<Vec<[i32; 2]>>, usize) {
    segs.retain(|s| s.len() >= 2);
    let mut rings = Vec::new();
    let mut unclosed = 0;
    while let Some(mut cur) = segs.pop() {
        loop {
            if cur.len() >= 4 && cur.first() == cur.last() {
                rings.push(cur);
                break;
            }
            let end = *cur.last().unwrap();
            match segs.iter().position(|s| s[0] == end || *s.last().unwrap() == end) {
                Some(i) => {
                    let mut s = segs.swap_remove(i);
                    if *s.last().unwrap() == end {
                        s.reverse();
                    }
                    cur.extend_from_slice(&s[1..]);
                }
                None => {
                    unclosed += 1;
                    break;
                }
            }
        }
    }
    (rings, unclosed)
}

/// Rings plus a latitude-band index over their segments.
#[derive(Default, Clone)]
pub struct Areas {
    /// kind per ring (KIND_A / KIND_B)
    pub kind: Vec<u8>,
    /// ring `r` is points `off[r]..off[r+1]` (closed: first == last); len R+1
    pub off: Vec<u32>,
    /// interleaved lon,lat in 1e-7°
    pub pts: Vec<i32>,
    // ---- runtime index (derived, not persisted)
    bbox: [f64; 4],
    y0: f64,
    dy: f64,
    /// segment start indices per latitude band
    bands: Vec<Vec<u32>>,
    /// ring id of each point index
    seg_ring: Vec<u32>,
}

impl Areas {
    /// Rings as persisted (no index yet; call [`Areas::build_index`]).
    pub fn from_parts(kind: Vec<u8>, off: Vec<u32>, pts: Vec<i32>) -> Self {
        Self { kind, off, pts, ..Default::default() }
    }

    pub fn num_rings(&self) -> usize {
        self.kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    pub fn push_ring(&mut self, kind: u8, ring: &[[i32; 2]]) {
        if self.off.is_empty() {
            self.off.push(0);
        }
        for p in ring {
            self.pts.push(p[0]);
            self.pts.push(p[1]);
        }
        self.off.push((self.pts.len() / 2) as u32);
        self.kind.push(kind);
    }

    #[inline]
    fn pt(&self, i: usize) -> (f64, f64) {
        (self.pts[2 * i] as f64 * 1e-7, self.pts[2 * i + 1] as f64 * 1e-7)
    }

    /// Structural check used by the graph loader.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.kind.is_empty() {
            return if self.off.len() <= 1 && self.pts.is_empty() { Ok(()) } else { Err("areas") };
        }
        if self.off.len() != self.kind.len() + 1 || self.off[0] != 0 || self.pts.len() % 2 != 0 {
            return Err("areas");
        }
        if self.off.last().map_or(true, |&o| o as usize * 2 != self.pts.len()) || self.off.windows(2).any(|w| w[1] < w[0] + 4) {
            return Err("areas");
        }
        Ok(())
    }

    /// Build the band index (call after loading / pushing rings).
    pub fn build_index(&mut self) {
        self.bands.clear();
        self.seg_ring.clear();
        if self.kind.is_empty() {
            return;
        }
        let n = self.pts.len() / 2;
        let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for i in 0..n {
            let (x, y) = self.pt(i);
            bbox[0] = bbox[0].min(x);
            bbox[1] = bbox[1].min(y);
            bbox[2] = bbox[2].max(x);
            bbox[3] = bbox[3].max(y);
        }
        self.bbox = bbox;
        self.y0 = bbox[1];
        self.dy = ((bbox[3] - bbox[1]) / BANDS as f64).max(1e-6);
        self.bands = vec![Vec::new(); BANDS];
        self.seg_ring = vec![0; n];
        for r in 0..self.kind.len() {
            let (a, b) = (self.off[r] as usize, self.off[r + 1] as usize);
            for i in a..b - 1 {
                self.seg_ring[i] = r as u32;
                let (_, y1) = self.pt(i);
                let (_, y2) = self.pt(i + 1);
                let lo = self.band(y1.min(y2));
                let hi = self.band(y1.max(y2));
                for band in lo..=hi {
                    self.bands[band].push(i as u32);
                }
            }
        }
    }

    #[inline]
    fn band(&self, y: f64) -> usize {
        (((y - self.y0) / self.dy).floor().max(0.0) as usize).min(BANDS - 1)
    }

    /// Kind bits (KIND_A | KIND_B) of the rings containing `p`. Points within `tol_m` of a ring
    /// boundary count as outside.
    pub fn classify(&self, p: LonLat, tol_m: f64) -> u8 {
        if self.bands.is_empty() {
            return 0;
        }
        let (px, py) = (p[0], p[1]);
        if px < self.bbox[0] || px > self.bbox[2] || py < self.bbox[1] || py > self.bbox[3] {
            return 0;
        }
        let b = self.band(py);
        let mut odd = vec![false; self.kind.len()];
        for &s in &self.bands[b] {
            let (x1, y1) = self.pt(s as usize);
            let (x2, y2) = self.pt(s as usize + 1);
            if (y1 > py) != (y2 > py) {
                let xi = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
                if xi > px {
                    let r = self.seg_ring[s as usize] as usize;
                    odd[r] = !odd[r];
                }
            }
        }
        let mut bits = 0u8;
        for (r, &o) in odd.iter().enumerate() {
            if o {
                bits |= self.kind[r];
            }
        }
        if bits != 0 && tol_m > 0.0 && self.near_boundary(px, py, b, tol_m) {
            return 0;
        }
        bits
    }

    /// Is the point within `tol_m` of any ring segment in the neighbouring bands?
    fn near_boundary(&self, px: f64, py: f64, b: usize, tol_m: f64) -> bool {
        let kx = 111_320.0 * py.to_radians().cos();
        let ky = 110_574.0;
        let tol2 = tol_m * tol_m;
        let lo = b.saturating_sub(1);
        let hi = (b + 1).min(BANDS - 1);
        for band in lo..=hi {
            for &s in &self.bands[band] {
                let (x1, y1) = self.pt(s as usize);
                let (x2, y2) = self.pt(s as usize + 1);
                let (ax, ay) = ((x1 - px) * kx, (y1 - py) * ky);
                let (bx, by) = ((x2 - px) * kx, (y2 - py) * ky);
                let (dx, dy) = (bx - ax, by - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 < 1e-9 { 0.0 } else { (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0) };
                let (cx, cy) = (ax + t * dx, ay + t * dy);
                if cx * cx + cy * cy <= tol2 {
                    return true;
                }
            }
        }
        false
    }

    /// Sample points of a polyline: every vertex plus midpoints of long segments.
    fn samples(pts: &[LonLat]) -> Vec<LonLat> {
        let mut out = Vec::with_capacity(pts.len() + 4);
        for (i, p) in pts.iter().enumerate() {
            if i > 0 {
                let q = pts[i - 1];
                if haversine_m(q, *p) > SAMPLE_STEP_M {
                    out.push([(q[0] + p[0]) / 2.0, (q[1] + p[1]) / 2.0]);
                }
            }
            out.push(*p);
        }
        out
    }

    /// Edge classification: flagged with every kind that any sample point (further than
    /// `EDGE_TOL_M` from the boundary) lies in, so an edge that merely starts inside a territory
    /// is impassable too. Same sampling as [`Areas::route_hits`], so offline routes never trip
    /// the runtime check.
    pub fn classify_edge(&self, pts: &[LonLat]) -> u8 {
        if self.bands.is_empty() || pts.is_empty() {
            return 0;
        }
        let mut bits = 0u8;
        for p in Self::samples(pts) {
            bits |= self.classify(p, EDGE_TOL_M);
            if bits == KIND_A | KIND_B {
                break;
            }
        }
        bits
    }

    /// First sample point of a route geometry that lies inside a ring, with its kind bits.
    pub fn route_first_hit(&self, geometry: &[LonLat]) -> Option<(LonLat, u8)> {
        if self.bands.is_empty() {
            return None;
        }
        Self::samples(geometry).into_iter().find_map(|p| {
            let k = self.classify(p, ROUTE_TOL_M);
            if k != 0 { Some((p, k)) } else { None }
        })
    }

    /// Kinds touched by a route geometry (any sample point inside).
    pub fn route_hits(&self, geometry: &[LonLat]) -> u8 {
        if self.bands.is_empty() {
            return 0;
        }
        let mut bits = 0u8;
        for p in Self::samples(geometry) {
            bits |= self.classify(p, ROUTE_TOL_M);
            if bits == KIND_A | KIND_B {
                break;
            }
        }
        bits
    }
}

/// Kind bits requested by an avoid setting.
pub fn avoid_bits(avoid: &crate::types::Avoid) -> u8 {
    (if avoid.area_a { KIND_A } else { 0 }) | (if avoid.area_b { KIND_B } else { 0 })
}

pub fn kinds_label(bits: u8) -> String {
    let mut v = Vec::new();
    if bits & KIND_A != 0 {
        v.push("A");
    }
    if bits & KIND_B != 0 {
        v.push("B");
    }
    if v.is_empty() {
        "none".into()
    } else {
        v.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[i32; 2]> {
        let c = |x: f64, y: f64| [(x * 1e7).round() as i32, (y * 1e7).round() as i32];
        vec![c(x0, y0), c(x1, y0), c(x1, y1), c(x0, y1), c(x0, y0)]
    }

    #[test]
    fn stitch_closes_rings() {
        let c = |x: i32, y: i32| [x, y];
        // square split into three chains, one reversed
        let segs = vec![vec![c(0, 0), c(10, 0)], vec![c(10, 10), c(10, 0)], vec![c(10, 10), c(0, 10), c(0, 0)]];
        let (rings, unclosed) = stitch(segs);
        assert_eq!(rings.len(), 1);
        assert_eq!(unclosed, 0);
        assert_eq!(rings[0].first(), rings[0].last());
        assert_eq!(rings[0].len(), 5);
        let (rings, unclosed) = stitch(vec![vec![c(0, 0), c(5, 5)]]);
        assert!(rings.is_empty() && unclosed == 1);
    }

    #[test]
    fn point_in_polygon_and_tolerance() {
        let mut a = Areas::default();
        a.push_ring(KIND_A, &sq(35.0, 32.0, 35.1, 32.1));
        a.push_ring(KIND_B, &sq(35.2, 32.0, 35.3, 32.1));
        a.build_index();
        assert_eq!(a.classify([35.05, 32.05], 0.0), KIND_A);
        assert_eq!(a.classify([35.25, 32.05], 0.0), KIND_B);
        assert_eq!(a.classify([35.15, 32.05], 0.0), 0);
        assert_eq!(a.classify([34.0, 31.0], 0.0), 0);
        // 2 m inside the western edge: outside with tolerance, inside without
        let near = [35.0 + 2.0 / (111_320.0 * 32.05f64.to_radians().cos()), 32.05];
        assert_eq!(a.classify(near, 0.0), KIND_A);
        assert_eq!(a.classify(near, EDGE_TOL_M), 0);
        // edge + route helpers
        assert_eq!(a.classify_edge(&[[35.02, 32.05], [35.08, 32.05]]), KIND_A);
        assert_eq!(a.classify_edge(&[[34.9, 32.05], [35.35, 32.05]]), 0); // endpoints and midpoint all outside
        assert_eq!(a.classify_edge(&[[35.05, 32.05], [34.9, 32.05], [34.8, 32.05], [34.7, 32.05]]), KIND_A); // starts inside
        assert_eq!(a.route_hits(&[[34.9, 32.05], [35.05, 32.05], [35.25, 32.05]]), KIND_A | KIND_B);
        assert_eq!(a.route_hits(&[[34.9, 32.05], [34.95, 32.05]]), 0);
        assert!(a.validate().is_ok());
    }

    #[test]
    fn relation_kind() {
        let t = |v: Vec<(&str, &str)>| kind_for_tags(v.into_iter());
        assert_eq!(t(vec![("type", "boundary"), ("boundary", "administrative"), ("name:en", "Area A")]), Some(KIND_A));
        assert_eq!(t(vec![("type", "boundary"), ("boundary", "administrative"), ("name:he", "שטח B")]), Some(KIND_B));
        assert_eq!(t(vec![("type", "boundary"), ("boundary", "administrative"), ("name:en", "Area C")]), None);
        assert_eq!(t(vec![("type", "multipolygon"), ("name:en", "Area A")]), None);
    }
}
