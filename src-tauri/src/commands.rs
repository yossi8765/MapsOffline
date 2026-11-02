//! Tauri commands – thin wrappers over the modules. Errors are returned as strings
//! (well-known codes: NO_ROUTING_DATA, NO_TRANSIT_DATA, NO_GEOCODE_DATA, UPDATE_RUNNING).
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;
use crate::types::*;

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

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
        match st.with_geocoder(|g| g.search(&q, near, limit)) {
            Ok(mut v) => out.append(&mut v),
            Err(e) if e.to_string() == "NO_GEOCODE_DATA" => {}
            Err(e) => return Err(err(e)),
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
    tauri::async_runtime::spawn_blocking(move || st.with_geocoder(|g| g.reverse(lon, lat)).map_err(err))
        .await
        .map_err(err)?
}

/// Points of interest of the given categories inside the viewport (map layer). The `category`
/// field carries the raw category id so the frontend can pick the marker icon.
#[tauri::command]
pub async fn poi_layer(state: State<'_, Arc<AppState>>, cats: Vec<String>, bbox: [f64; 4], limit: Option<usize>) -> R<Vec<GeoResult>> {
    let st = state.inner().clone();
    let limit = limit.unwrap_or(1500).clamp(1, 10000);
    tauri::async_runtime::spawn_blocking(move || {
        let c: Vec<&str> = cats.iter().map(String::as_str).collect();
        st.with_geocoder(|g| g.pois_in_bbox(&c, bbox, limit)).map_err(err)
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
    })
}

// ---------------------------------------------------------------- routing

#[tauri::command]
pub async fn route(state: State<'_, Arc<AppState>>, req: RouteRequest) -> R<RouteResponse> {
    if req.waypoints.len() < 2 {
        return Err("NEED_TWO_WAYPOINTS".into());
    }
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<RouteResponse> {
        let settings = st.settings.read().clone();
        route_hybrid(&st, &req, &settings)
    })
    .await
    .map_err(err)?
}

/// Online first (unless routing_mode == "offline"), then the local engines (unless "online").
pub fn route_hybrid(st: &AppState, req: &RouteRequest, settings: &Settings) -> R<RouteResponse> {
    let mode = settings.routing_mode.as_str();
    if mode != "offline" {
        match crate::online::route(req, settings) {
            Ok(mut resp) => {
                // Online engines cannot avoid territories: drop offending routes, and if none is
                // left let the local engine (which enforces the avoid) compute the route.
                if crate::routing::areas::avoid_bits(&req.avoid) != 0 && req.mode != Mode::Transit {
                    match st.graph() {
                        Ok(g) => {
                            let dropped = g.drop_routes_in_avoided_areas(req, &mut resp);
                            if dropped > 0 {
                                log::info!("online routes entering avoided territories dropped: {dropped}");
                            }
                            if resp.routes.is_empty() {
                                log::info!("all online routes enter avoided territories; using local routing");
                                return route_offline(st, req);
                            }
                        }
                        Err(e) => log::warn!("cannot check online routes against territories (no graph): {e}"),
                    }
                }
                if req.mode == Mode::Transit {
                    // stop codes come from the local GTFS model when it is already in memory
                    if let Some(t) = st.transit.read().as_ref() {
                        t.fill_stop_codes(&mut resp.routes);
                    }
                }
                // OSRM's foot / bike profiles keep off Israeli urban trunk roads and detour for
                // hours; when the local engine is clearly shorter, trust it instead.
                if matches!(req.mode, Mode::Foot | Mode::Bike) {
                    if let (Some(online), Ok(local)) = (resp.routes.first().map(|r| r.duration_sec), route_offline(st, req)) {
                        if let Some(l) = local.routes.first() {
                            if (l.duration_sec as f64) < online as f64 * 0.7 {
                                log::info!("online {:?} route {}s vs local {}s: using local", req.mode, online, l.duration_sec);
                                return Ok(local);
                            }
                        }
                    }
                }
                return Ok(resp);
            }
            Err(e) => {
                log::warn!("online routing failed, falling back to offline: {e:#}");
                if mode == "online" {
                    return Err(format!("ONLINE_FAILED: {e}"));
                }
            }
        }
    }
    route_offline(st, req)
}

pub fn route_offline(st: &AppState, req: &RouteRequest) -> R<RouteResponse> {
    match req.mode {
        Mode::Transit => {
            let t = st.transit().map_err(err)?;
            match st.graph() {
                Ok(g) => t.route(req, g.as_ref()).map_err(err),
                Err(_) => t.route(req, &crate::transit::NoWalk).map_err(err),
            }
        }
        _ => {
            let g = st.graph().map_err(err)?;
            g.route(req).map_err(err)
        }
    }
}

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
pub async fn transit_line(state: State<'_, Arc<AppState>>, short_name: String, agency: String, headsign: Option<String>) -> R<crate::types::LineShape> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::LineShape> {
        let t = st.transit().map_err(err)?;
        t.line_shape(&short_name, &agency, headsign.as_deref()).ok_or_else(|| "LINE_NOT_FOUND".to_string())
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
pub async fn transit_line_times(state: State<'_, Arc<AppState>>, short_name: String, agency: String, headsign: Option<String>) -> R<crate::types::LineTimes> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::LineTimes> {
        let t = st.transit().map_err(err)?;
        crate::transit::board::line_times(&t, &short_name, &agency, headsign.as_deref()).ok_or_else(|| "NO_SERVICE_TODAY".to_string())
    })
    .await
    .map_err(err)?
}

/// Departure board of a stop (by code, else nearest served stop within 40 m).
#[tauri::command]
pub async fn transit_stop_board(state: State<'_, Arc<AppState>>, code: Option<String>, lon: f64, lat: f64) -> R<crate::types::StopBoard> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<crate::types::StopBoard> {
        let t = st.transit().map_err(err)?;
        crate::transit::board::stop_board(&t, code.as_deref(), lon, lat).ok_or_else(|| "STOP_NOT_FOUND".to_string())
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------- history & favorites

#[tauri::command(async)]
pub fn history_list(
    state: State<'_, Arc<AppState>>,
    kind: Option<HistoryKind>,
    limit: Option<usize>,
    filter: Option<String>,
) -> R<Vec<HistoryItem>> {
    state.with_history(|h| h.list(kind, limit.unwrap_or(100), filter.as_deref())).map_err(err)
}

#[tauri::command(async)]
pub fn history_add(
    state: State<'_, Arc<AppState>>,
    kind: HistoryKind,
    label: String,
    payload: serde_json::Value,
) -> R<HistoryItem> {
    state.with_history(|h| h.add(kind, &label, payload)).map_err(err)
}

#[tauri::command(async)]
pub fn history_delete(state: State<'_, Arc<AppState>>, id: i64) -> R<()> {
    state.with_history(|h| h.delete(id)).map_err(err)
}

#[tauri::command(async)]
pub fn history_clear(state: State<'_, Arc<AppState>>, kind: Option<HistoryKind>) -> R<()> {
    state.with_history(|h| h.clear(kind)).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_list(state: State<'_, Arc<AppState>>) -> R<Vec<Favorite>> {
    state.with_history(|h| h.favorites()).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_add(
    state: State<'_, Arc<AppState>>,
    name: String,
    icon: String,
    lon: f64,
    lat: f64,
    address: Option<String>,
    kind: Option<String>,
    data: Option<serde_json::Value>,
) -> R<Favorite> {
    state.with_history(|h| h.favorite_add(&name, &icon, lon, lat, address.as_deref(), kind.as_deref().unwrap_or("address"), data.as_ref())).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_update(state: State<'_, Arc<AppState>>, id: i64, name: String, icon: String) -> R<()> {
    state.with_history(|h| h.favorite_update(id, &name, &icon)).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_delete(state: State<'_, Arc<AppState>>, id: i64) -> R<()> {
    state.with_history(|h| h.favorite_delete(id)).map_err(err)
}

// ---------------------------------------------------------------- data

#[tauri::command(async)]
pub fn data_status(state: State<'_, Arc<AppState>>) -> R<DataStatus> {
    let paths = state.paths();
    let mut s = crate::data::status(&paths);
    s.routing.loaded = state.graph.read().is_some();
    s.transit.loaded = state.transit.read().is_some();
    s.geocode.loaded = state.geocoder_loaded();
    s.tiles.loaded = s.tiles.present;
    s.world.loaded = s.world.present;
    s.satellite.loaded = s.satellite.present;
    s.terrain.loaded = s.terrain.present;
    s.updating = state.updating.load(Ordering::Relaxed);
    s.paused = s.updating && state.cancel.is_paused();
    Ok(s)
}

/// Ask the servers whether newer data is available (does not download anything).
#[tauri::command]
pub async fn data_check(state: State<'_, Arc<AppState>>) -> R<crate::types::DataCheck> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || crate::data::check(&paths)).await.map_err(err)
}

/// Absolute path of the pmtiles file (frontend opens it through the asset protocol).
#[tauri::command]
pub fn tiles_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().tiles_file();
    Ok(p.exists().then(|| p.display().to_string()))
}

/// Absolute path of the world basemap archive, if installed.
#[tauri::command]
pub fn world_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().world_file();
    Ok(p.exists().then(|| p.display().to_string()))
}

/// Absolute path of the satellite imagery archive, if installed.
#[tauri::command]
pub fn satellite_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().satellite_file();
    Ok(p.exists().then(|| p.display().to_string()))
}

// ---------------------------------------------------------------- program updates

/// Newest signed release on GitLab versus the running version (does not download).
#[tauri::command]
pub async fn app_update_check() -> R<crate::types::AppUpdate> {
    tauri::async_runtime::spawn_blocking(|| crate::app_update::check().map_err(err)).await.map_err(err)?
}

/// Download + verify the installer of the newest signed release. Progress on `app-update-progress`.
#[tauri::command]
pub async fn app_update_download(app: AppHandle, state: State<'_, Arc<AppState>>) -> R<String> {
    let st = state.inner().clone();
    if st.updating.swap(true, Ordering::SeqCst) {
        return Err("UPDATE_RUNNING".into());
    }
    st.cancel.reset();
    let st2 = st.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let progress = |phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
            let _ = app.emit("app-update-progress", DataProgress { component: DataComponent::Tiles, phase, done, total, message: msg.to_string() });
            !st2.cancel.is_cancelled()
        };
        let r = crate::app_update::download(&progress, &st2.cancel).map(|p| p.display().to_string()).map_err(err);
        st2.updating.store(false, Ordering::SeqCst);
        r
    })
    .await
    .map_err(err)?;
    res
}

/// Start the verified installer and quit, so the installer can replace the running program.
#[tauri::command]
pub fn app_update_install(app: AppHandle, path: String) -> R<()> {
    let p = std::path::PathBuf::from(&path);
    if !p.starts_with(crate::app_update::update_dir()) || !p.exists() {
        return Err("UPDATE_FILE_INVALID".into());
    }
    crate::app_update::launch_installer(&p).map_err(err)?;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        app.exit(0);
    });
    Ok(())
}

// ---------------------------------------------------------------- stop order optimisation

/// Shortest visiting order of the request's waypoints by local travel time (origin fixed;
/// destination fixed when `fixed_end`). Returns the new order as indices into `req.waypoints`.
#[tauri::command]
pub async fn optimize_order(state: State<'_, Arc<AppState>>, req: RouteRequest, fixed_end: bool) -> R<Vec<usize>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<usize>> {
        let n = req.waypoints.len();
        if n < 3 {
            return Ok((0..n).collect());
        }
        if n > 10 {
            return Err("TOO_MANY_WAYPOINTS".into());
        }
        if req.mode == Mode::Transit {
            return Err("ORDER_NOT_FOR_TRANSIT".into());
        }
        let g = st.graph().map_err(err)?;
        // pairwise travel times (seconds); unreachable = very large
        let mut cost = vec![vec![f64::INFINITY; n]; n];
        for a in 0..n {
            for b in 0..n {
                if a == b {
                    cost[a][b] = 0.0;
                    continue;
                }
                let sub = RouteRequest { waypoints: vec![req.waypoints[a].clone(), req.waypoints[b].clone()], alternatives: 0, ..req.clone() };
                if let Ok(resp) = g.route(&sub) {
                    if let Some(r) = resp.routes.first() {
                        cost[a][b] = r.duration_sec as f64;
                    }
                }
            }
        }
        Ok(crate::routing::order::best_order(&cost, fixed_end))
    })
    .await
    .map_err(err)?
}

/// Current position through the Windows location service (system consent dialog on first use).
#[tauri::command]
pub async fn locate() -> R<crate::types::MyLocation> {
    tauri::async_runtime::spawn_blocking(|| crate::location::locate().map(|(lon, lat, accuracy_m)| crate::types::MyLocation { lon, lat, accuracy_m }).map_err(err))
        .await
        .map_err(err)?
}

/// Absolute path of the elevation archive, if installed.
#[tauri::command]
pub fn terrain_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().terrain_file();
    Ok(p.exists().then(|| p.display().to_string()))
}

/// Elevation in metres at each point (None outside the installed DEM), for route profiles.
#[tauri::command]
pub async fn elevation_profile(state: State<'_, Arc<AppState>>, points: Vec<LonLat>) -> R<Vec<Option<f32>>> {
    let archive = state.paths().terrain_file();
    if !archive.exists() {
        return Err("NO_TERRAIN_DATA".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::data::terrain::elevations(&archive, &points).map_err(err)).await.map_err(err)?
}

#[tauri::command]
pub async fn data_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    components: Vec<DataComponent>,
) -> R<()> {
    let st = state.inner().clone();
    if st.updating.swap(true, Ordering::SeqCst) {
        return Err("UPDATE_RUNNING".into());
    }
    st.cancel.reset();
    let paths = st.paths();
    let comps = if components.is_empty() { DataComponent::ALL.to_vec() } else { components };
    let app2 = app.clone();
    let emit: crate::data::Emit = Arc::new(move |p: DataProgress| {
        let _ = app2.emit("data-progress", p);
    });
    let st2 = st.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let r = crate::data::update(&comps, &paths, emit, &st2.cancel);
        st2.unload_all();
        st2.updating.store(false, Ordering::SeqCst);
        r
    })
    .await
    .map_err(err)?;
    let _ = app.emit("data-updated", ());
    res.map_err(err)
}

/// Pack the installed components (all when `components` is empty / missing) into one `.mapsdata`
/// file (for machines without internet).
#[tauri::command]
pub async fn data_export(app: AppHandle, state: State<'_, Arc<AppState>>, dest: String, components: Option<Vec<DataComponent>>) -> R<Vec<DataComponent>> {
    let only = components.unwrap_or_default();
    run_package_job(app, state.inner().clone(), move |paths, progress, cancel| {
        crate::data::package::export(paths, std::path::Path::new(&dest), &only, progress, cancel)
    })
    .await
}

/// Install a `.mapsdata` package into the data directory and reload everything.
#[tauri::command]
pub async fn data_import(app: AppHandle, state: State<'_, Arc<AppState>>, src: String) -> R<Vec<DataComponent>> {
    let st = state.inner().clone();
    // Validate before the UI switches into "updating" mode.
    let src_path = std::path::PathBuf::from(&src);
    let p2 = src_path.clone();
    tauri::async_runtime::spawn_blocking(move || crate::data::package::inspect(&p2).map(|_| ()))
        .await
        .map_err(err)?
        .map_err(err)?;
    let res = run_package_job(app.clone(), st.clone(), move |paths, progress, cancel| {
        crate::data::package::import(paths, &src_path, progress, cancel)
    })
    .await;
    st.unload_all();
    let _ = app.emit("data-updated", ());
    res
}

async fn run_package_job<F>(app: AppHandle, st: Arc<AppState>, job: F) -> R<Vec<DataComponent>>
where
    F: FnOnce(&crate::paths::DataPaths, &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync), &crate::data::Cancel) -> anyhow::Result<Vec<DataComponent>>
        + Send
        + 'static,
{
    if st.updating.swap(true, Ordering::SeqCst) {
        return Err("UPDATE_RUNNING".into());
    }
    st.cancel.reset();
    let paths = st.paths();
    let st2 = st.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let progress = |component: DataComponent, phase: ProgressPhase, done: u64, total: u64, message: &str| {
            let _ = app.emit("data-progress", DataProgress { component, phase, done, total, message: message.to_string() });
        };
        let r = job(&paths, &progress, &st2.cancel);
        if let Err(e) = &r {
            let phase = if st2.cancel.is_cancelled() { ProgressPhase::Cancelled } else { ProgressPhase::Failed };
            for c in DataComponent::ALL {
                let _ = app.emit("data-progress", DataProgress { component: c, phase, done: 0, total: 0, message: format!("{e:#}") });
            }
        }
        st2.updating.store(false, Ordering::SeqCst);
        r
    })
    .await
    .map_err(err)?;
    res.map_err(err)
}

/// Cancel the whole update, or only one component (skipped if queued, aborted if running).
#[tauri::command]
pub fn data_cancel(state: State<'_, Arc<AppState>>, component: Option<DataComponent>) -> R<()> {
    match component {
        Some(c) => state.cancel.cancel_component(c),
        None => state.cancel.cancel(),
    }
    Ok(())
}

/// Pause / resume the running update (downloads drop their connection and resume by Range;
/// builders block at their next progress report).
#[tauri::command]
pub fn data_pause(state: State<'_, Arc<AppState>>, paused: bool) -> R<()> {
    state.cancel.set_paused(paused);
    Ok(())
}

/// Preload heavy data in the background (called once at startup). The data sets load in
/// parallel on separate blocking threads; every command stays answerable meanwhile.
#[tauri::command]
pub async fn data_preload(state: State<'_, Arc<AppState>>, transit: bool) -> R<()> {
    let st = state.inner().clone();
    let geo = {
        let st = st.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = st.with_geocoder(|g| {
                g.warm();
                Ok(())
            });
        })
    };
    let graph = {
        let st = st.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = st.graph();
        })
    };
    let tr = transit.then(|| {
        tauri::async_runtime::spawn_blocking(move || {
            let _ = st.transit();
        })
    });
    geo.await.map_err(err)?;
    graph.await.map_err(err)?;
    if let Some(t) = tr {
        t.await.map_err(err)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub fn settings_get(state: State<'_, Arc<AppState>>) -> R<Settings> {
    Ok(state.settings.read().clone())
}

#[tauri::command(async)]
pub fn settings_set(state: State<'_, Arc<AppState>>, settings: Settings) -> R<Settings> {
    let dir_changed = settings.data_dir != state.settings.read().data_dir;
    *state.settings.write() = settings.clone();
    state.save_settings().map_err(err)?;
    if dir_changed {
        let new_root = settings
            .data_dir
            .clone()
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::paths::DataPaths::default_root);
        let p = crate::paths::DataPaths::new(new_root);
        let _ = p.ensure_dirs();
        *state.paths.write() = p;
        state.unload_all();
        *state.history.lock() = None;
        state.save_settings().map_err(err)?;
    }
    Ok(settings)
}

#[tauri::command]
pub fn app_info() -> R<serde_json::Value> {
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "bbox": crate::paths::BBOX_ISRAEL,
    }))
}
