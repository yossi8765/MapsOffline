//! Routing: hybrid online/offline routes, stop-order optimisation, elevation profiles.
//!
//! Owner: R2 (routing + navigation stream).
#[allow(unused_imports)]
use super::prelude::*;

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
                            if l.duration_sec < online * 0.7 {
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
                    return Err(AppError::new("ONLINE_FAILED", format!("ONLINE_FAILED: {e:#}")));
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
            let pts: Vec<[f64; 2]> = req.waypoints.iter().map(|w| [w.lon, w.lat]).collect();
            let g = st.graph_for(&pts).map_err(err)?;
            g.route(req).map_err(err)
        }
    }
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
        let pts: Vec<[f64; 2]> = req.waypoints.iter().map(|w| [w.lon, w.lat]).collect();
        let g = st.graph_for(&pts).map_err(err)?;
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
                        cost[a][b] = r.duration_sec;
                    }
                }
            }
        }
        Ok(crate::routing::order::best_order(&cost, fixed_end))
    })
    .await
    .map_err(err)?
}

/// Elevation in metres at each point (None outside the installed DEM), for route profiles.
#[tauri::command]
pub async fn elevation_profile(state: State<'_, Arc<AppState>>, points: Vec<LonLat>) -> R<Vec<Option<f32>>> {
    let archive = state.paths().terrain_file_in_use();
    if !archive.exists() {
        return Err("NO_TERRAIN_DATA".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::data::terrain::elevations(&archive, &points).map_err(err)).await.map_err(err)?
}
