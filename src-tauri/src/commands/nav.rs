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
        let (voice, niqqud) = {
            let s = st.settings.read();
            (s.nav_voice, s.nav_voice_niqqud)
        };
        let cams = cameras_near(&st, &route);
        crate::nav::start(app, &route, crate::nav::Source::Gps, voice, niqqud, &cams).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Like nav_start, but drives along the route at `speed_kmh` (no GPS needed); 0 = at the posted
/// speed limit of each stretch.
#[tauri::command]
pub async fn nav_simulate(app: AppHandle, state: State<'_, Arc<AppState>>, route: Route, speed_kmh: Option<f64>) -> R<()> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (voice, niqqud) = {
            let s = st.settings.read();
            (s.nav_voice, s.nav_voice_niqqud)
        };
        let speed_kmh = match speed_kmh.unwrap_or(50.0) {
            v if v <= 0.0 || !v.is_finite() => 0.0,
            v => v.clamp(5.0, 300.0),
        };
        let cams = cameras_near(&st, &route);
        crate::nav::start(app, &route, crate::nav::Source::Simulate { speed_kmh }, voice, niqqud, &cams).map_err(err)
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

/// Change the speed of a running simulated drive (km/h; 0 = as the road allows).
#[tauri::command]
pub fn nav_set_sim_speed(speed_kmh: f64) {
    let v = if speed_kmh <= 0.0 || !speed_kmh.is_finite() { 0.0 } else { speed_kmh.clamp(5.0, 300.0) };
    crate::nav::set_sim_speed(v);
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

/// Say `text` with the Hebrew system voice (when installed). `vocalize`: a navigation
/// instruction – add the hidden niqqud when the setting is on.
#[tauri::command]
pub fn speak(state: State<'_, Arc<AppState>>, text: String, vocalize: Option<bool>) {
    let niqqud = vocalize.unwrap_or(false) && state.settings.read().nav_voice_niqqud;
    crate::voice::say(&crate::nav::spoken(&text, niqqud));
}

/// Settings → "test the voice": the same sample instructions without / with the hidden niqqud.
#[tauri::command]
pub fn voice_test(vocalized: bool) {
    let text = crate::nav::speech::SAMPLES.join(". ");
    crate::voice::say(&crate::nav::spoken(&text, vocalized));
}

/// Installed text-to-speech voices: [displayName, language], the one used first.
#[tauri::command]
pub async fn voices_list() -> R<Vec<(String, String)>> {
    tauri::async_runtime::spawn_blocking(crate::voice::voices).await.map_err(err)
}
