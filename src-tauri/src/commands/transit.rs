//! Public transport: stops layer, lines, timetables, stop boards, delays.
//!
//! Owner: R3 (transit stream).
#[allow(unused_imports)]
use super::prelude::*;

/// Transit stops inside `bbox` = [min_lon, min_lat, max_lon, max_lat] (map layer). Loads the
/// transit model lazily; returns NO_TRANSIT_DATA when it is not installed.
#[tauri::command]
pub async fn transit_stops(state: State<'_, Arc<AppState>>, bbox: [f64; 4], limit: Option<usize>) -> R<Vec<StopInfo>> {
    let st = state.inner().clone();
    let limit = limit.unwrap_or(2000).clamp(1, 20000);
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<StopInfo>> {
        let t = st.transit().map_err(err)?;
        let mut out = Vec::new();
        for (i, s) in t.stops.iter().enumerate() {
            if s.lon >= bbox[0] && s.lon <= bbox[2] && s.lat >= bbox[1] && s.lat <= bbox[3] {
                let code = t.strings.get(s.code as usize).filter(|c| !c.is_empty()).cloned();
                out.push(StopInfo { id: i as u32, name: t.strings.get(s.name as usize).cloned().unwrap_or_default(), code, lon: s.lon, lat: s.lat });
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(err)?
}

/// The longest stop pattern of a transit line (by short name + agency, preferring the given
/// headsign), with its shape, for the favorites panel. LINE_NOT_FOUND when the feed has none.
#[tauri::command]
pub async fn transit_line(state: State<'_, Arc<AppState>>, short_name: String, agency: String, headsign: Option<String>, long_name: Option<String>) -> R<crate::types::LineShape> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::LineShape> {
        let t = st.transit().map_err(err)?;
        t.line_shape_of(&short_name, &agency, headsign.as_deref(), long_name.as_deref()).ok_or_else(|| "LINE_NOT_FOUND".into())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn transit_agencies(state: State<'_, Arc<AppState>>) -> R<Vec<String>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<String>> {
        let t = st.transit().map_err(err)?;
        Ok(crate::transit::board::agencies(&t))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn transit_lines(state: State<'_, Arc<AppState>>, agency: String) -> R<Vec<crate::types::LineSummary>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<crate::types::LineSummary>> {
        let t = st.transit().map_err(err)?;
        Ok(crate::transit::board::lines(&t, &agency))
    })
    .await
    .map_err(err)?
}

/// Departures of a line from now until the end of today's service (both directions).
#[tauri::command]
pub async fn transit_line_times(state: State<'_, Arc<AppState>>, short_name: String, agency: String, headsign: Option<String>, long_name: Option<String>) -> R<crate::types::LineTimes> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::LineTimes> {
        let t = st.transit().map_err(err)?;
        crate::transit::board::line_times_of(&t, &short_name, &agency, headsign.as_deref(), long_name.as_deref()).ok_or_else(|| "NO_SERVICE_TODAY".into())
    })
    .await
    .map_err(err)?
}

/// Typical delay (minutes, median of the last week's rides) of each boarding, from Open Bus
/// Stride. Slow (10–15 s) and online only: the UI calls it after showing the routes. None per
/// boarding when unknown, offline, or `settings.transitReliability` is off.
#[tauri::command]
pub async fn transit_delays(state: State<'_, Arc<AppState>>, legs: Vec<crate::online::stride::DelayQuery>) -> R<Vec<Option<f64>>> {
    if !state.settings.read().transit_reliability {
        return Ok(vec![None; legs.len()]);
    }
    tauri::async_runtime::spawn_blocking(move || crate::online::stride::delays(&legs)).await.map_err(err)
}

/// Departure board of a stop (by code, else nearest served stop within 40 m).
#[tauri::command]
pub async fn transit_stop_board(state: State<'_, Arc<AppState>>, code: Option<String>, lon: f64, lat: f64) -> R<crate::types::StopBoard> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::StopBoard> {
        let t = st.transit().map_err(err)?;
        crate::transit::board::stop_board(&t, code.as_deref(), lon, lat).ok_or_else(|| "STOP_NOT_FOUND".into())
    })
    .await
    .map_err(err)?
}
