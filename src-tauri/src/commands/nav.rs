//! Live navigation, text-to-speech and the Windows location service.
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

/// Navigate `route` with the Windows location service. Progress arrives as "nav-update" events
/// (snapped position, current maneuver, distances, ETA, offRoute / rerouteNeeded, arrived);
/// "nav-error" when the location is denied or lost, "nav-ended" when it stops.
#[tauri::command]
pub fn nav_start(app: AppHandle, state: State<'_, Arc<AppState>>, route: Route) -> R<()> {
    let voice = state.settings.read().nav_voice;
    crate::nav::start(app, &route, crate::nav::Source::Gps, voice).map_err(err)
}

/// Like nav_start, but drives along the route at `speed_kmh` (no GPS needed).
#[tauri::command]
pub fn nav_simulate(app: AppHandle, state: State<'_, Arc<AppState>>, route: Route, speed_kmh: Option<f64>) -> R<()> {
    let voice = state.settings.read().nav_voice;
    let speed_kmh = speed_kmh.unwrap_or(50.0).clamp(5.0, 300.0);
    crate::nav::start(app, &route, crate::nav::Source::Simulate { speed_kmh }, voice).map_err(err)
}

/// Replace the route of the running session (after a reroute).
#[tauri::command]
pub fn nav_set_route(route: Route) -> R<()> {
    crate::nav::set_route(&route).map_err(err)
}

#[tauri::command]
pub fn nav_set_voice(on: bool) {
    crate::nav::set_voice(on);
}

#[tauri::command]
pub fn nav_stop() {
    crate::nav::stop();
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
