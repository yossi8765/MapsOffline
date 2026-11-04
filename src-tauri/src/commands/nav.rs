//! Live navigation, track recording, text-to-speech and the Windows location service.
//!
//! Owner: R2 (routing + navigation stream).
#[allow(unused_imports)]
use super::prelude::*;

/// Current position through the Windows location service (system consent dialog on first use).
#[tauri::command]
pub async fn locate() -> R<crate::types::MyLocation> {
    tauri::async_runtime::spawn_blocking(|| crate::location::locate().map(|(lon, lat, accuracy_m)| crate::types::MyLocation { lon, lat, accuracy_m }).map_err(err))
        .await
        .map_err(err)?
}

// ---------------------------------------------------------------- live navigation

/// Speed cameras of the local graph near a route (empty without a graph).
fn cameras_near(st: &AppState, route: &Route) -> Vec<LonLat> {
    let geom: Vec<LonLat> = if route.geometry.len() >= 2 { route.geometry.clone() } else { route.legs.iter().flat_map(|l| l.geometry.iter().copied()).collect() };
    let (Some(a), Some(b)) = (geom.first(), geom.last()) else { return Vec::new() };
    let Ok(g) = st.graph_for(&[*a, *b]) else { return Vec::new() };
    let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in &geom {
        bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
    }
    let m = 0.005;
    g.camera_points().filter(|c| c[0] >= bb[0] - m && c[0] <= bb[2] + m && c[1] >= bb[1] - m && c[1] <= bb[3] + m).collect()
}

/// Navigate `route` with the Windows location service. Progress arrives as "nav-update" events
/// (snapped position, current maneuver, distances, ETA, offRoute / rerouteNeeded, arrived,
/// speed, posted limit, lanes, speed cameras); "nav-error" when the location is denied or lost,
/// "nav-ended" when it stops.
#[tauri::command]
pub async fn nav_start(app: AppHandle, state: State<'_, Arc<AppState>>, route: Route) -> R<()> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let voice = st.settings.read().nav_voice;
        let cams = cameras_near(&st, &route);
        crate::nav::start(app, &route, crate::nav::Source::Gps, voice, &cams).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Like nav_start, but drives along the route at `speed_kmh` (no GPS needed).
#[tauri::command]
pub async fn nav_simulate(app: AppHandle, state: State<'_, Arc<AppState>>, route: Route, speed_kmh: Option<f64>) -> R<()> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let voice = st.settings.read().nav_voice;
        let speed_kmh = speed_kmh.unwrap_or(50.0).clamp(5.0, 300.0);
        let cams = cameras_near(&st, &route);
        crate::nav::start(app, &route, crate::nav::Source::Simulate { speed_kmh }, voice, &cams).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Replace the route of the running session (after a reroute).
#[tauri::command]
pub async fn nav_set_route(state: State<'_, Arc<AppState>>, route: Route) -> R<()> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cams = cameras_near(&st, &route);
        crate::nav::set_route(&route, &cams).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn nav_set_voice(on: bool) {
    crate::nav::set_voice(on);
}

#[tauri::command]
pub fn nav_stop() {
    crate::nav::stop();
}

// ---------------------------------------------------------------- track recording

/// Start recording the travelled track ("track-update" events with the points so far).
#[tauri::command]
pub fn track_start(app: AppHandle, state: State<'_, Arc<AppState>>) -> R<()> {
    crate::nav::track::start(app, state.paths()).map_err(err)
}

/// Stop recording; with `save` the track is stored as a user layer and returned.
#[tauri::command]
pub async fn track_stop(state: State<'_, Arc<AppState>>, save: bool, name: Option<String>) -> R<Option<crate::types::TrackLayer>> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || crate::nav::track::stop(&paths, save, name).map_err(err)).await.map_err(err)?
}

/// Say `text` with the Hebrew system voice (when installed).
#[tauri::command]
pub fn speak(text: String) {
    crate::voice::say(&text);
}

/// Installed text-to-speech voices: [displayName, language], the one used first.
#[tauri::command]
pub async fn voices_list() -> R<Vec<(String, String)>> {
    tauri::async_runtime::spawn_blocking(crate::voice::voices).await.map_err(err)
}
