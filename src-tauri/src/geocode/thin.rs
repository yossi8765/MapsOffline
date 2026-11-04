//! Point-of-interest thinning "like Google": every POI gets the lowest zoom at which it is shown.
//! Greedy and hierarchical: zoom by zoom (6 → 17) the most important POIs are placed first, a POI
//! is placed only when no POI already shown at that zoom lies within the spacing (72 px at z6
//! shrinking to 34 px at z17), and everything placed at a zoom stays placed above it (so icons
//! never blink in and out while zooming). POIs never placed get [`NEVER`] (shown from z18).
use rustc_hash::FxHashMap;

pub const MIN_Z: u8 = 6;
pub const MAX_Z: u8 = 17;
/// Minzoom of the POIs that never found room up to z17.
pub const NEVER: u8 = 18;

pub struct ThinPt {
    pub lon: f64,
    pub lat: f64,
    /// Higher = placed first.
    pub importance: f64,
}

/// Spacing in screen pixels at zoom `z`.
pub fn spacing_px(z: u8) -> f64 {
    let t = (z.clamp(MIN_Z, MAX_Z) - MIN_Z) as f64 / (MAX_Z - MIN_Z) as f64;
    72.0 - (72.0 - 34.0) * t
}

/// Web-Mercator world pixel coordinates (256 px tiles) at zoom `z`.
fn world_px(lon: f64, lat: f64, z: u8) -> (f64, f64) {
    let size = 256.0 * (1u64 << z) as f64;
    let x = (lon + 180.0) / 360.0 * size;
    let s = lat.clamp(-85.0, 85.0).to_radians().sin();
    let y = (0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * std::f64::consts::PI)) * size;
    (x, y)
}

/// Minimum zoom of every point (same order as `pts`).
pub fn minzooms(pts: &[ThinPt]) -> Vec<u8> {
    let mut order: Vec<u32> = (0..pts.len() as u32).collect();
    order.sort_by(|&a, &b| pts[b as usize].importance.total_cmp(&pts[a as usize].importance).then(a.cmp(&b)));
    let mut mz = vec![NEVER; pts.len()];
    let mut placed: Vec<u32> = Vec::new();
    for z in MIN_Z..=MAX_Z {
        let s = spacing_px(z);
        let s2 = s * s;
        let cell = |x: f64, y: f64| ((x / s).floor() as i64, (y / s).floor() as i64);
        let mut grid: FxHashMap<(i64, i64), Vec<(f64, f64)>> = FxHashMap::default();
        for &i in &placed {
            let p = &pts[i as usize];
            let (x, y) = world_px(p.lon, p.lat, z);
            grid.entry(cell(x, y)).or_default().push((x, y));
        }
        for &i in &order {
            if mz[i as usize] != NEVER {
                continue;
            }
            let p = &pts[i as usize];
            let (x, y) = world_px(p.lon, p.lat, z);
            let (cx, cy) = cell(x, y);
            let mut free = true;
            'n: for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(v) = grid.get(&(cx + dx, cy + dy)) {
                        if v.iter().any(|&(qx, qy)| (qx - x).powi(2) + (qy - y).powi(2) < s2) {
                            free = false;
                            break 'n;
                        }
                    }
                }
            }
            if free {
                mz[i as usize] = z;
                placed.push(i);
                grid.entry((cx, cy)).or_default().push((x, y));
            }
        }
    }
    mz
}

/// Importance tier of a POI category (raw OSM value, "place_of_worship:jewish" style).
pub fn category_tier(cat: &str) -> f64 {
    let base = cat.split_once(':').map_or(cat, |(b, _)| b);
    match base {
        "hospital" | "university" | "aerodrome" | "station" | "mall" | "stadium" | "museum" | "zoo" | "theme_park" | "attraction" | "beach" => 5.0,
        "townhall" | "college" | "place_of_worship" | "police" | "fire_station" | "library" | "theatre" | "cinema" | "supermarket" | "fuel"
        | "bus_station" | "courthouse" | "viewpoint" | "park" | "nature_reserve" | "castle" | "archaeological_site" | "monument" | "ruins" => 4.0,
        "school" | "pharmacy" | "bank" | "clinic" | "doctors" | "restaurant" | "cafe" | "hotel" | "post_office" | "community_centre"
        | "sports_centre" | "swimming_pool" | "marketplace" | "department_store" | "hostel" | "guest_house" | "spring" | "peak" => 3.0,
        "atm" | "toilets" | "parking" | "kindergarten" | "childcare" | "telephone" | "vending_machine" | "pitch" | "playground" | "fitness_station" => 1.0,
        _ => 2.0,
    }
}

/// Importance of one POI: category tier, a name, and the size of its town.
pub fn importance(cat: &str, named: bool, city_pop: i64) -> f64 {
    category_tier(cat) * 10.0 + if named { 6.0 } else { 0.0 } + (city_pop.max(1) as f64).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_scale() {
        assert_eq!(spacing_px(6), 72.0);
        assert_eq!(spacing_px(17), 34.0);
        assert!(spacing_px(12) < 72.0 && spacing_px(12) > 34.0);
    }

    #[test]
    fn cluster_and_hierarchy() {
        // 50 POIs on one spot in Tel Aviv: the most important one first, the rest never fit
        let mut pts: Vec<ThinPt> = (0..50).map(|i| ThinPt { lon: 34.78, lat: 32.08, importance: i as f64 }).collect();
        // a far point (Eilat: ~120 px away at z6; Jerusalem would be only ~20 px) and one ~100 m away
        pts.push(ThinPt { lon: 34.95, lat: 29.55, importance: 0.0 });
        pts.push(ThinPt { lon: 34.781, lat: 32.0805, importance: 100.0 });
        let mz = minzooms(&pts);
        assert_eq!(mz[51], MIN_Z, "the most important point is shown from the start");
        assert_eq!(mz[50], MIN_Z, "Eilat is far from everything");
        assert!(mz[49] > MIN_Z && mz[49] <= MAX_Z, "second in the cluster appears when zoomed in: {}", mz[49]);
        assert!(mz[..49].iter().all(|&z| z == NEVER), "the same spot cannot hold two icons");
        // every pair shown together at a zoom keeps the spacing at that zoom
        for z in MIN_Z..=MAX_Z {
            let shown: Vec<usize> = (0..pts.len()).filter(|&i| mz[i] <= z).collect();
            for a in 0..shown.len() {
                for b in a + 1..shown.len() {
                    let (pa, pb) = (&pts[shown[a]], &pts[shown[b]]);
                    let (xa, ya) = world_px(pa.lon, pa.lat, z);
                    let (xb, yb) = world_px(pb.lon, pb.lat, z);
                    assert!(((xa - xb).powi(2) + (ya - yb).powi(2)).sqrt() >= spacing_px(z) - 1e-9, "z{z}");
                }
            }
        }
    }

    #[test]
    fn importance_order() {
        assert!(importance("hospital", true, 400_000) > importance("cafe", true, 400_000));
        assert!(importance("cafe", true, 1000) > importance("cafe", false, 1000));
        assert!(importance("atm", true, 900_000) < importance("pharmacy", true, 900_000));
    }
}
