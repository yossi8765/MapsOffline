//! Track recording: the Windows location service is polled every 2 s; each new point (≥ 5 m
//! from the last) gets its elevation from the installed DEM, and a "track-update" event carries
//! the whole track so far. Stopping can save it as a user layer (GeoJSON LineString).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::geo::haversine_m;
use crate::paths::DataPaths;
use crate::types::{TrackLayer, TrackUpdate};

/// Minimum distance (m) between recorded points.
const MIN_STEP_M: f64 = 5.0;
const TICK: Duration = Duration::from_secs(2);

struct Recording {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<TrackState>>,
}

#[derive(Default, Clone)]
pub struct TrackState {
    pub points: Vec<(f64, f64, Option<f32>)>,
    pub times: Vec<String>,
    pub distance_m: f64,
    pub ascent_m: f64,
    pub started: Option<std::time::Instant>,
    pub start_iso: String,
}

impl TrackState {
    fn update(&self) -> TrackUpdate {
        TrackUpdate {
            points: self.points.clone(),
            distance_m: self.distance_m,
            duration_sec: self.started.map_or(0.0, |s| s.elapsed().as_secs_f64()),
            ascent_m: self.ascent_m,
        }
    }

    /// Add a fix; returns false when it is too close to the last point.
    pub fn push(&mut self, lon: f64, lat: f64, ele: Option<f32>, iso: String) -> bool {
        if let Some(&(plon, plat, pele)) = self.points.last() {
            let d = haversine_m([plon, plat], [lon, lat]);
            if d < MIN_STEP_M {
                return false;
            }
            self.distance_m += d;
            if let (Some(a), Some(b)) = (pele, ele) {
                if b > a {
                    self.ascent_m += (b - a) as f64;
                }
            }
        }
        self.points.push((lon, lat, ele));
        self.times.push(iso);
        true
    }
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

fn now_iso() -> String {
    chrono::Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn recording() -> bool {
    RECORDING.lock().is_some()
}

/// Start recording (no-op when already recording).
pub fn start(app: AppHandle, paths: DataPaths) -> anyhow::Result<()> {
    if recording() {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(TrackState { started: Some(std::time::Instant::now()), start_iso: now_iso(), ..Default::default() }));
    *RECORDING.lock() = Some(Recording { stop: stop.clone(), state: state.clone() });
    let dem = paths.terrain_file_in_use();
    std::thread::Builder::new().name("track".into()).spawn(move || {
        let mut gps = match super::gps::Gps::open() {
            Ok(g) => g,
            Err(e) => {
                let _ = app.emit("nav-error", e.to_string());
                *RECORDING.lock() = None;
                return;
            }
        };
        while !stop.load(Ordering::SeqCst) {
            let t0 = std::time::Instant::now();
            if let Ok(f) = gps.fix() {
                // a coarse (Wi-Fi) fix would draw zig-zags
                if f.accuracy_m.is_none_or(|a| a <= 60.0) {
                    let ele = if dem.exists() { crate::data::terrain::elevations(&dem, &[[f.lon, f.lat]]).ok().and_then(|v| v.first().copied().flatten()) } else { None };
                    let mut st = state.lock();
                    if st.push(f.lon, f.lat, ele, now_iso()) {
                        let u = st.update();
                        drop(st);
                        let _ = app.emit("track-update", &u);
                    }
                }
            }
            let spent = t0.elapsed();
            if spent < TICK {
                std::thread::sleep(TICK - spent);
            }
        }
    })?;
    Ok(())
}

/// Stop recording; with `save` the track becomes a user layer.
pub fn stop(paths: &DataPaths, save: bool, name: Option<String>) -> anyhow::Result<Option<TrackLayer>> {
    let Some(rec) = RECORDING.lock().take() else { return Ok(None) };
    rec.stop.store(true, Ordering::SeqCst);
    let st = rec.state.lock().clone();
    if !save || st.points.len() < 2 {
        return Ok(None);
    }
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("מסלול מוקלט {}", st.start_iso.get(..16).unwrap_or(&st.start_iso).replace('T', " ")));
    let geojson = to_geojson(&st, &name);
    save_track_layer(paths, &name, &geojson, &st).map(Some)
}

/// FeatureCollection with one LineString ([lon, lat, ele]) and the times / totals as properties.
pub fn to_geojson(st: &TrackState, name: &str) -> String {
    let coords: Vec<serde_json::Value> = st
        .points
        .iter()
        .map(|&(lon, lat, ele)| match ele {
            Some(e) => serde_json::json!([lon, lat, (e * 10.0).round() / 10.0]),
            None => serde_json::json!([lon, lat]),
        })
        .collect();
    serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {
                "name": name,
                "distanceM": st.distance_m.round(),
                "durationSec": st.started.map_or(0.0, |s| s.elapsed().as_secs_f64().round()),
                "ascentM": st.ascent_m.round(),
                "startIso": st.start_iso,
                "coordTimes": st.times,
            },
            "geometry": { "type": "LineString", "coordinates": coords }
        }]
    })
    .to_string()
}



/// Save the recorded track as a user layer (kind "track") in the shared user-layer store.
fn save_track_layer(paths: &DataPaths, name: &str, geojson: &str, _st: &TrackState) -> anyhow::Result<TrackLayer> {
    crate::user_layers::save(paths, name, "track", geojson, Some("#e8590c"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_state_and_geojson() {
        let mut st = TrackState { start_iso: "2026-11-04T10:00:00+02:00".into(), ..Default::default() };
        assert!(st.push(34.78, 32.08, Some(10.0), "a".into()));
        assert!(!st.push(34.78001, 32.08, Some(10.0), "b".into())); // ~1 m
        assert!(st.push(34.781, 32.08, Some(14.5), "c".into()));
        assert!(st.push(34.782, 32.08, Some(12.0), "d".into()));
        assert_eq!(st.points.len(), 3);
        assert!((st.ascent_m - 4.5).abs() < 1e-6);
        assert!((st.distance_m - 188.6).abs() < 2.0, "{}", st.distance_m);
        let g: serde_json::Value = serde_json::from_str(&to_geojson(&st, "t")).unwrap();
        assert_eq!(g["features"][0]["geometry"]["coordinates"][1][2], 14.5);
        assert_eq!(g["features"][0]["properties"]["coordTimes"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn saves_layer_files() {
        let dir = std::env::temp_dir().join(format!("mo-track-test-{}", std::process::id()));
        let paths = DataPaths::new(&dir);
        let mut st = TrackState::default();
        st.push(34.78, 32.08, None, "a".into());
        st.push(34.79, 32.08, None, "b".into());
        let l = save_track_layer(&paths, "x", &to_geojson(&st, "x"), &st).unwrap();
        assert!(dir.join("userlayers").join(format!("{}.geojson", l.id)).exists());
        let list = crate::user_layers::list(&paths);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "track");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
