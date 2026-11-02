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
