//! Geocoding: address / place search, reverse geocoding, POI map layer.
//!
//! Owner: R4 (geocode stream).
#[allow(unused_imports)]
use super::prelude::*;

// ---------------------------------------------------------------- geocoding

/// Typed coordinates in any supported format; bare numbers that fit both ways are read in the
/// order that lands in Israel or a downloaded country.
fn parse_coords(st: &AppState, q: &str, near: Option<LonLat>) -> Option<crate::coords::ParsedCoord> {
    let regions = st.region_geocoders();
    let known = |lon: f64, lat: f64| {
        let inb = |b: &[f64; 4]| lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3];
        inb(&crate::paths::BBOX_ISRAEL) || regions.iter().any(|(_, _, b)| inb(b))
    };
    crate::coords::parse(q, near, &known)
}

/// The search behind `geocode_search` / `geocode_batch`: coordinates, Israel and every downloaded
/// country (the area around `near` first, the others interleaved after it), transit stops.
fn search_all(st: &Arc<AppState>, query: &str, near: Option<LonLat>, limit: usize) -> R<Vec<GeoResult>> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(vec![]);
    }
    if let Some(c) = parse_coords(st, &q, near) {
        return Ok(vec![crate::geocode::coord_result(&c)]);
    }
    let mut out: Vec<GeoResult> = Vec::new();
    let regions = st.region_geocoders();
    let in_box = |b: &[f64; 4], p: &LonLat| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3];
    let mut groups: Vec<(bool, Vec<GeoResult>)> = Vec::new();
    let mut no_index = false;
    match st.with_geocoder(|g| g.search(&q, near, limit)) {
        Ok(v) => groups.push((near.is_none_or(|p| in_box(&crate::paths::BBOX_ISRAEL, &p)), v)),
        Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {
            no_index = true;
            // no local address index: search online (Photon / OSM) unless the user chose
            // offline-only; a failure just leaves the online results out
            if st.settings.read().routing_mode != "offline" {
                match crate::online::photon::search(&q, near, limit) {
                    Ok(v) => {
                        no_index = v.is_empty();
                        groups.push((true, v));
                    }
                    Err(e) => log::info!("online search (no local index): {e:#}"),
                }
            }
        }
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
    let merged = crate::geocode::merge_with_stops(out, stops, limit);
    // nothing to search in at all: tell the UI so it can point at the Data panel
    if merged.is_empty() && no_index && regions.is_empty() {
        return Err(AppError::from("NO_GEOCODE_DATA"));
    }
    Ok(merged)
}

#[tauri::command]
pub async fn geocode_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    near: Option<LonLat>,
    limit: Option<usize>,
) -> R<Vec<GeoResult>> {
    let st = state.inner().clone();
    let limit = limit.unwrap_or(10).clamp(1, 50);
    tauri::async_runtime::spawn_blocking(move || search_all(&st, &query, near, limit)).await.map_err(err)?
}

/// Best result of each query (CSV import of an address list); None where nothing was found.
/// At most 5000 queries per call.
#[tauri::command]
pub async fn geocode_batch(state: State<'_, Arc<AppState>>, queries: Vec<String>, near: Option<LonLat>) -> R<Vec<Option<GeoResult>>> {
    let st = state.inner().clone();
    if queries.len() > 5000 {
        return Err(AppError::new("TOO_MANY_QUERIES", format!("TOO_MANY_QUERIES: {} (max 5000)", queries.len())));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::with_capacity(queries.len());
        for q in &queries {
            // one bad line must not fail the whole list
            out.push(search_all(&st, q, near, 1).ok().and_then(|v| v.into_iter().next()));
        }
        Ok(out)
    })
    .await
    .map_err(err)?
}

/// Coordinates typed in any supported format (decimal, DMS, ITM, UTM, plus codes) or None.
#[tauri::command]
pub async fn coords_parse(state: State<'_, Arc<AppState>>, text: String) -> R<Option<crate::coords::ParsedCoord>> {
    Ok(parse_coords(state.inner(), &text, None))
}

/// Offline Wikipedia summary of a `wikipedia` tag ("he:Title") or Wikidata id, from the Israel
/// address index (built with summaries since builder revision 4); None when not available.
#[tauri::command]
pub async fn wiki_summary(state: State<'_, Arc<AppState>>, tag: String) -> R<Option<WikiSummary>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match st.with_geocoder(|g| g.wiki(&tag)) {
        Ok(v) => Ok(v),
        Err(e) if e.to_string() == "NO_GEOCODE_DATA" => Ok(None),
        Err(e) => Err(err(e)),
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
                Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {
                    // no local index: the nearest address from Photon (OSM) when online is allowed
                    if st.settings.read().routing_mode != "offline" {
                        if let Ok(Some(r)) = crate::online::photon::reverse(lon, lat) {
                            return Ok(Some(r));
                        }
                    }
                }
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
