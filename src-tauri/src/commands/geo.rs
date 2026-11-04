//! Geocoding: address / place search, reverse geocoding, POI map layer.
//!
//! Owner: R4 (geocode stream).
#[allow(unused_imports)]
use super::prelude::*;

// ---------------------------------------------------------------- geocoding

#[tauri::command]
pub async fn geocode_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    near: Option<LonLat>,
    limit: Option<usize>,
) -> R<Vec<GeoResult>> {
    let st = state.inner().clone();
    let limit = limit.unwrap_or(10).clamp(1, 50);
    tauri::async_runtime::spawn_blocking(move || {
        let q = query.trim().to_string();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let mut out: Vec<GeoResult> = Vec::new();
        if let Some(c) = parse_coord(&q) {
            out.push(c);
        }
        // Israel and every downloaded country; the area around `near` comes first, the others
        // are interleaved after it.
        let regions = st.region_geocoders();
        let in_box = |b: &[f64; 4], p: &LonLat| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3];
        let mut groups: Vec<(bool, Vec<GeoResult>)> = Vec::new();
        match st.with_geocoder(|g| g.search(&q, near, limit)) {
            Ok(v) => groups.push((near.is_none_or(|p| in_box(&crate::paths::BBOX_ISRAEL, &p)), v)),
            Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {}
            Err(e) if regions.is_empty() => return Err(err(e)),
            Err(e) => log::warn!("geocode (Israel): {e:#}"),
        }
        for (id, path, bbox) in &regions {
            match st.with_region_geocoder(path, |g| g.search(&q, near, limit)) {
                Ok(v) => groups.push((near.is_some_and(|p| in_box(bbox, &p)), v)),
                Err(e) => log::warn!("geocode ({id}): {e:#}"),
            }
        }
        groups.sort_by_key(|(primary, _)| !*primary);
        let mut iters: Vec<std::vec::IntoIter<GeoResult>> = Vec::new();
        for (primary, v) in groups {
            if primary {
                out.extend(v);
            } else {
                iters.push(v.into_iter());
            }
        }
        while out.len() < limit * 2 && !iters.is_empty() {
            iters.retain_mut(|it| match it.next() {
                Some(r) => {
                    out.push(r);
                    true
                }
                None => false,
            });
        }
        // Transit stops by name / stop code. The model is loaded in the background on first
        // use so a search never waits for it; until then results simply have no stops.
        let stops = match st.transit.read().clone() {
            Some(t) => t.search_stops(&q, near, 5),
            None => {
                st.transit_load_background();
                Vec::new()
            }
        };
        Ok(crate::geocode::merge_with_stops(out, stops, limit))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn geocode_reverse(state: State<'_, Arc<AppState>>, lon: f64, lat: f64) -> R<Option<GeoResult>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let b = crate::paths::BBOX_ISRAEL;
        let in_israel = lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3];
        if in_israel {
            match st.with_geocoder(|g| g.reverse(lon, lat)) {
                Ok(Some(r)) => return Ok(Some(r)),
                Ok(None) => {}
                Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {}
                Err(e) => return Err(err(e)),
            }
        }
        for (_, path, b) in st.region_geocoders() {
            if lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3] {
                if let Ok(Some(r)) = st.with_region_geocoder(&path, |g| g.reverse(lon, lat)) {
                    return Ok(Some(r));
                }
            }
        }
        Ok(None)
    })
    .await
    .map_err(err)?
}

/// Points of interest of the given categories inside the viewport (map layer). The `category`
/// field carries the raw category id so the frontend can pick the marker icon. With `zoom`, only
/// the POIs the build-time thinning shows at that zoom (fewer, evenly spread, stable icons).
#[tauri::command]
pub async fn poi_layer(state: State<'_, Arc<AppState>>, cats: Vec<String>, bbox: [f64; 4], limit: Option<usize>, zoom: Option<f64>) -> R<Vec<GeoResult>> {
    let st = state.inner().clone();
    let limit = limit.unwrap_or(1500).clamp(1, 10000);
    tauri::async_runtime::spawn_blocking(move || {
        let c: Vec<&str> = cats.iter().map(String::as_str).collect();
        let overlaps = |b: &[f64; 4]| !(bbox[2] < b[0] || bbox[0] > b[2] || bbox[3] < b[1] || bbox[1] > b[3]);
        let mut out = Vec::new();
        if overlaps(&crate::paths::BBOX_ISRAEL) {
            match st.with_geocoder(|g| g.pois_in_bbox(&c, bbox, limit, zoom)) {
                Ok(v) => out = v,
                Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {}
                Err(e) => return Err(err(e)),
            }
        }
        for (_, path, b) in st.region_geocoders() {
            if out.len() >= limit {
                break;
            }
            if overlaps(&b) {
                if let Ok(mut v) = st.with_region_geocoder(&path, |g| g.pois_in_bbox(&c, bbox, limit - out.len(), zoom)) {
                    out.append(&mut v);
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(err)?
}

/// Parse "32.08, 34.78" / "34.78 32.08" (lat,lon preferred when ambiguous) into a coordinate result.
fn parse_coord(q: &str) -> Option<GeoResult> {
    let parts: Vec<&str> = q.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }
    let a: f64 = parts[0].parse().ok()?;
    let b: f64 = parts[1].parse().ok()?;
    // Israel: lat ~29..34, lon ~34..36. Decide by range.
    let (lat, lon) = if (29.0..=34.0).contains(&a) && (34.0..=36.0).contains(&b) {
        (a, b)
    } else if (29.0..=34.0).contains(&b) && (34.0..=36.0).contains(&a) {
        (b, a)
    } else if a.abs() <= 90.0 && b.abs() <= 180.0 {
        (a, b)
    } else {
        return None;
    };
    Some(GeoResult {
        id: format!("coord:{lon},{lat}"),
        name: format!("{:.5}, {:.5}", lat, lon),
        kind: GeoKind::Coord,
        precision: GeoPrecision::Exact,
        lon,
        lat,
        city: None,
        street: None,
        house_number: None,
        category: None,
        score: 100.0,
        extra: None,
    })
}
