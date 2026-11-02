//! Google encoded-polyline decoder with configurable precision (MOTIS uses 7, OSRM 5).
use crate::types::LonLat;

pub fn decode(s: &str, precision: u32) -> Vec<LonLat> {
    let factor = 10f64.powi(precision as i32);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4);
    let (mut i, mut lat, mut lon) = (0usize, 0i64, 0i64);
    while i < bytes.len() {
        let read = |i: &mut usize| -> Option<i64> {
            let (mut result, mut shift) = (0i64, 0u32);
            loop {
                if *i >= bytes.len() {
                    return None;
                }
                let b = (bytes[*i] as i64) - 63;
                *i += 1;
                result |= (b & 0x1f) << shift;
                shift += 5;
                if b < 0x20 {
                    break;
                }
            }
            Some(if result & 1 != 0 { !(result >> 1) } else { result >> 1 })
        };
        let Some(dlat) = read(&mut i) else { break };
        let Some(dlon) = read(&mut i) else { break };
        lat += dlat;
        lon += dlon;
        out.push([lon as f64 / factor, lat as f64 / factor]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_reference_polyline() {
        // Google's documented example (precision 5).
        let pts = decode("_p~iF~ps|U_ulLnnqC_mqNvxq`@", 5);
        assert_eq!(pts.len(), 3);
        assert!((pts[0][1] - 38.5).abs() < 1e-5 && (pts[0][0] + 120.2).abs() < 1e-5);
        assert!((pts[2][1] - 43.252).abs() < 1e-5 && (pts[2][0] + 126.453).abs() < 1e-5);
    }
}
