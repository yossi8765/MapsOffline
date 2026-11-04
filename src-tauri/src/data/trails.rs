//! Marked hiking and cycling trails: OSM route relations (route=hiking|foot|bicycle|mtb) from the
//! Israel PBF, written as one GeoJSON file (`trails/trails.geojson`) that the map loads as a
//! geojson source and colours by the trail's marking (osmc:symbol / colour: red, blue, green,
//! black, orange = Israel National Trail, purple, yellow).
//!
//! Three passes over the PBF: route relations → their member ways → those ways' nodes. Member
//! ways stay separate lines of one MultiLineString (no stitching needed for drawing); geometry is
//! simplified (Douglas–Peucker, ~4 m) and rounded to 1e-5° (~1 m).
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use osmpbf::{Element, ElementReader, RelMemberType};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::types::{ProgressFn, ProgressPhase};

/// Bump when the file layout / properties change (part of the manifest version).
pub const BUILD_REV: u32 = 1;

#[derive(Debug, Clone)]
struct Route {
    id: i64,
    name: String,
    name_en: Option<String>,
    route: String,
    colour: &'static str,
    network: Option<String>,
    reference: Option<String>,
    ways: Vec<i64>,
}

/// Normalised marking colour from `osmc:symbol` ("red:white:red_bar" → red) or `colour`.
pub fn marking_colour(osmc: Option<&str>, colour: Option<&str>, name: &str, reference: Option<&str>) -> &'static str {
    let lname = name.to_lowercase();
    if name.contains("שביל ישראל") || lname.contains("israel national trail") || lname.contains("israel trail") || reference.is_some_and(|r| r.eq_ignore_ascii_case("INT")) {
        return "orange";
    }
    let pick = |s: &str| -> Option<&'static str> {
        let s = s.trim().to_lowercase();
        let first = s.split(':').next().unwrap_or("").trim();
        Some(match first {
            "red" => "red",
            "blue" => "blue",
            "green" => "green",
            "black" => "black",
            "orange" => "orange",
            "purple" | "violet" => "purple",
            "yellow" => "yellow",
            "#ff0000" | "#f00" => "red",
            "#0000ff" | "#00f" => "blue",
            "#008000" | "#00ff00" | "#0f0" => "green",
            "#000000" | "#000" => "black",
            _ => return None,
        })
    };
    // osmc:symbol "waycolor:background:foreground..." – the first field is the way colour; when it
    // is white/empty the foreground (e.g. "white:white:red_bar") carries the marking
    if let Some(sym) = osmc {
        let parts: Vec<&str> = sym.split(':').collect();
        if let Some(c) = parts.first().and_then(|p| pick(p)) {
            return c;
        }
        for p in parts.iter().skip(2) {
            if let Some(c) = pick(p.split('_').next().unwrap_or("")) {
                return c;
            }
        }
    }
    colour.and_then(pick).unwrap_or("other")
}

fn is_trail(route: &str) -> bool {
    matches!(route, "hiking" | "foot" | "bicycle" | "mtb")
}

/// Perpendicular distance in metres-ish units (equirectangular around the segment).
fn simplify(points: &[[f64; 2]], tol_m: f64) -> Vec<[f64; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let lat0 = points[0][1].to_radians();
    let kx = 111_320.0 * lat0.cos();
    let ky = 110_540.0;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = (points[a][0] * kx, points[a][1] * ky);
        let (bx, by) = (points[b][0] * kx, points[b][1] * ky);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let mut best = (0.0f64, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let (px, py) = (p[0] * kx, p[1] * ky);
            let d = if len2 == 0.0 {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            } else {
                ((dy * px - dx * py + bx * ay - by * ax).abs()) / len2.sqrt()
            };
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol_m {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn haversine_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (la1, la2) = (a[1].to_radians(), b[1].to_radians());
    let dla = la2 - la1;
    let dlo = (b[0] - a[0]).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

/// Build `out` (GeoJSON) from the PBF. Returns the number of trails written.
pub fn build(pbf: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<usize> {
    progress(ProgressPhase::Processing, 0, 100, "קורא את השבילים המסומנים מ-OSM");
    // ---- pass 1: route relations
    let reader = ElementReader::from_path(pbf).with_context(|| format!("open {}", pbf.display()))?;
    let routes: Vec<Route> = reader.par_map_reduce(
        |el| match el {
            Element::Relation(r) => {
                let tags: FxHashMap<&str, &str> = r.tags().collect();
                if tags.get("type") != Some(&"route") || !tags.get("route").is_some_and(|v| is_trail(v)) {
                    return Vec::new();
                }
                let name = tags.get("name:he").or_else(|| tags.get("name")).map(|s| s.to_string()).unwrap_or_default();
                let reference = tags.get("ref").map(|s| s.to_string());
                let ways: Vec<i64> = r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
                if ways.is_empty() {
                    return Vec::new();
                }
                vec![Route {
                    id: r.id(),
                    colour: marking_colour(tags.get("osmc:symbol").copied(), tags.get("colour").copied(), &name, reference.as_deref()),
                    name,
                    name_en: tags.get("name:en").map(|s| s.to_string()),
                    route: tags["route"].to_string(),
                    network: tags.get("network").map(|s| s.to_string()),
                    reference,
                    ways,
                }]
            }
            _ => Vec::new(),
        },
        Vec::new,
        |mut a, mut b| {
            a.append(&mut b);
            a
        },
    )?;
    log::info!("trails: {} route relations", routes.len());
    progress(ProgressPhase::Processing, 30, 100, &format!("{} שבילים, קורא את הדרכים", routes.len()));
    let want_ways: FxHashSet<i64> = routes.iter().flat_map(|r| r.ways.iter().copied()).collect();

    // ---- pass 2: member ways
    let reader = ElementReader::from_path(pbf)?;
    let ways: FxHashMap<i64, Vec<i64>> = reader.par_map_reduce(
        |el| match el {
            Element::Way(w) if want_ways.contains(&w.id()) => {
                let mut m = FxHashMap::default();
                m.insert(w.id(), w.refs().collect::<Vec<i64>>());
                m
            }
            _ => FxHashMap::default(),
        },
        FxHashMap::default,
        |mut a, b| {
            a.extend(b);
            a
        },
    )?;
    let want_nodes: FxHashSet<i64> = ways.values().flatten().copied().collect();
    progress(ProgressPhase::Processing, 60, 100, "קורא את נקודות השבילים");

    // ---- pass 3: their nodes
    let reader = ElementReader::from_path(pbf)?;
    let nodes: FxHashMap<i64, [f64; 2]> = reader.par_map_reduce(
        |el| {
            let mut m = FxHashMap::default();
            match el {
                Element::DenseNode(n) if want_nodes.contains(&n.id()) => {
                    m.insert(n.id(), [n.lon(), n.lat()]);
                }
                Element::Node(n) if want_nodes.contains(&n.id()) => {
                    m.insert(n.id(), [n.lon(), n.lat()]);
                }
                _ => {}
            }
            m
        },
        FxHashMap::default,
        |mut a, b| {
            a.extend(b);
            a
        },
    )?;
    progress(ProgressPhase::Processing, 85, 100, "כותב את קובץ השבילים");

    // ---- GeoJSON
    let tmp = out.with_extension("geojson.tmp");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    f.write_all(br#"{"type":"FeatureCollection","features":["#)?;
    let mut written = 0usize;
    let round = |v: f64| (v * 1e5).round() / 1e5;
    for r in &routes {
        let mut lines: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut length = 0.0;
        for wid in &r.ways {
            let Some(refs) = ways.get(wid) else { continue };
            let pts: Vec<[f64; 2]> = refs.iter().filter_map(|n| nodes.get(n).copied()).collect();
            if pts.len() < 2 {
                continue;
            }
            length += pts.windows(2).map(|w| haversine_m(w[0], w[1])).sum::<f64>();
            let s: Vec<[f64; 2]> = simplify(&pts, 4.0).into_iter().map(|p| [round(p[0]), round(p[1])]).collect();
            lines.push(s);
        }
        if lines.is_empty() {
            continue;
        }
        let props = serde_json::json!({
            "id": r.id,
            "name": r.name,
            "nameEn": r.name_en,
            "route": r.route,
            "colour": r.colour,
            "network": r.network,
            "ref": r.reference,
            "lengthM": length.round(),
        });
        let feat = serde_json::json!({ "type": "Feature", "id": r.id, "properties": props, "geometry": { "type": "MultiLineString", "coordinates": lines } });
        if written > 0 {
            f.write_all(b",")?;
        }
        serde_json::to_writer(&mut f, &feat)?;
        written += 1;
    }
    f.write_all(b"]}")?;
    f.flush()?;
    drop(f);
    anyhow::ensure!(written > 0, "no marked trails found in the OSM extract");
    crate::paths::atomic_replace(out, &tmp)?;
    log::info!("trails: {written} written to {}", out.display());
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_from_markings() {
        assert_eq!(marking_colour(Some("red:white:red_bar"), None, "", None), "red");
        assert_eq!(marking_colour(Some("white:white:blue_bar"), None, "", None), "blue");
        assert_eq!(marking_colour(None, Some("green"), "", None), "green");
        assert_eq!(marking_colour(Some("red:white:red_bar"), None, "שביל ישראל", None), "orange");
        assert_eq!(marking_colour(None, None, "Israel National Trail", None), "orange");
        assert_eq!(marking_colour(None, Some("#123456"), "x", None), "other");
    }

    #[test]
    fn simplify_keeps_corners() {
        let pts = [[35.0, 31.0], [35.0001, 31.0], [35.0002, 31.0], [35.0002, 31.001]];
        let s = simplify(&pts, 1.0);
        assert_eq!(s.first(), pts.first());
        assert_eq!(s.last(), pts.last());
        assert!(s.contains(&[35.0002, 31.0]));
        assert!(!s.contains(&[35.0001, 31.0]));
    }
}
