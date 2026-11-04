//! Routing: hybrid online/offline routes, stop-order optimisation, isochrones, track matching,
//! elevation profiles.
//!
//! Owner: R2 (routing + navigation stream).
#[allow(unused_imports)]
use super::prelude::*;

use crate::routing::profile::Profile;

/// Most stops `optimize_order` / `optimize_trip` accept.
const MAX_TRIP_STOPS: usize = 25;

/// A graph error with the reason the UI needs: an old graph file asks for a routing update.
fn graph_err(e: anyhow::Error) -> AppError {
    let a = err(e);
    if a.code == "NO_ROUTING_DATA" && a.message.contains("graph version") {
        a.with_details("graph version")
    } else {
        a
    }
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
/// Requests only the local engine can answer (truck, wheelchair, avoided roads / areas, hills)
/// go to it directly.
pub fn route_hybrid(st: &AppState, req: &RouteRequest, settings: &Settings) -> R<RouteResponse> {
    let mode = settings.routing_mode.as_str();
    if mode != "offline" && crate::online::supports(req) {
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
                        crate::transit::attach_notice(t, req, &mut resp);
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
            let g = st.graph_for(&pts).map_err(graph_err)?;
            g.route(req).map_err(err)
        }
    }
}

// ---------------------------------------------------------------- stop order optimisation

/// Travel-time matrix between the request's waypoints (local engine, real seconds).
fn trip_matrix(st: &AppState, req: &RouteRequest) -> R<Vec<Vec<f64>>> {
    let n = req.waypoints.len();
    if n > MAX_TRIP_STOPS {
        return Err("TOO_MANY_WAYPOINTS".into());
    }
    if req.mode == Mode::Transit {
        return Err("ORDER_NOT_FOR_TRANSIT".into());
    }
    let pts: Vec<[f64; 2]> = req.waypoints.iter().map(|w| [w.lon, w.lat]).collect();
    let g = st.graph_for(&pts).map_err(graph_err)?;
    let prof = Profile::for_request(&g, req);
    let allowed = |e: u32| prof.allowed_edge_any(&g, e);
    let mut snaps = Vec::with_capacity(n);
    for p in &pts {
        let s = g.snap(*p, &allowed).filter(|s| s.dist_m <= 5_000.0).ok_or_else(|| AppError::from("NO_ROAD_NEAR_WAYPOINT"))?;
        snaps.push(s);
    }
    let mut m = crate::routing::iso::matrix(&g, &prof, &snaps);
    // unreachable pairs: a large finite cost keeps the planners total
    for row in &mut m {
        for v in row.iter_mut() {
            if !v.is_finite() {
                *v = 1e7;
            }
        }
    }
    Ok(m)
}

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
        let cost = trip_matrix(&st, &req)?;
        Ok(crate::routing::order::plan_trip(&cost, fixed_end, 0.0, &[]).0)
    })
    .await
    .map_err(err)?
}

/// Departure time of a request (epoch seconds): its depart time, else now.
fn start_epoch(req: &RouteRequest) -> f64 {
    let now = chrono::Utc::now().timestamp() as f64;
    match (req.time.kind, req.time.iso.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())) {
        (TimeKind::Depart, Some(t)) => t.timestamp() as f64,
        _ => now,
    }
}

fn iso_at(epoch: f64) -> String {
    chrono::DateTime::from_timestamp(epoch.round() as i64, 0)
        .unwrap_or_default()
        .with_timezone(&chrono_tz::Asia::Jerusalem)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Multi-stop trip: visiting order for up to 25 stops with stay times and arrival windows
/// (`stops` aligned with `req.waypoints`), and the arrival time at each stop in that order.
#[tauri::command]
pub async fn optimize_trip(state: State<'_, Arc<AppState>>, req: RouteRequest, fixed_end: bool, stops: Vec<TripStop>) -> R<TripPlan> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<TripPlan> {
        let n = req.waypoints.len();
        let start = start_epoch(&req);
        let parse = |s: &Option<String>| s.as_deref().and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok()).map(|t| t.timestamp() as f64);
        let rules: Vec<crate::routing::order::StopRule> = (0..n)
            .map(|i| {
                let s = stops.get(i).cloned().unwrap_or_default();
                crate::routing::order::StopRule { stay_sec: s.stay_min.unwrap_or(0.0).max(0.0) * 60.0, window: (parse(&s.window_start_iso), parse(&s.window_end_iso)) }
            })
            .collect();
        let cost = if n < 2 { vec![vec![0.0; n]; n] } else { trip_matrix(&st, &req)? };
        let (order, arrivals) = crate::routing::order::plan_trip(&cost, fixed_end, start, &rules);
        Ok(TripPlan { order, arrivals: arrivals.into_iter().map(iso_at).collect() })
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------- isochrones / tracks

/// Areas reachable from a point within each of `minutes` (car / foot / bike / wheelchair).
#[tauri::command]
pub async fn isochrone(state: State<'_, Arc<AppState>>, lon: f64, lat: f64, mode: Mode, minutes: Vec<f64>) -> R<Vec<IsochroneBand>> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<Vec<IsochroneBand>> {
        if matches!(mode, Mode::Transit | Mode::Truck) {
            return Err(AppError::new("MODE_NOT_SUPPORTED", "isochrones: car, foot, bike or wheelchair"));
        }
        let g = st.graph_for(&[[lon, lat]]).map_err(graph_err)?;
        crate::routing::iso::isochrone(&g, [lon, lat], mode, &minutes).map_err(err)
    })
    .await
    .map_err(err)?
}

/// A route along an imported / recorded track (map matching), ready for navigation.
#[tauri::command]
pub async fn match_track(state: State<'_, Arc<AppState>>, points: Vec<LonLat>, mode: Mode) -> R<Route> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || -> R<Route> {
        if points.len() > 100_000 {
            return Err("TOO_MANY_POINTS".into());
        }
        let ends: Vec<[f64; 2]> = [points.first(), points.last()].into_iter().flatten().copied().collect();
        let g = st.graph_for(&ends).map_err(graph_err)?;
        crate::routing::matching::match_track(&g, &points, mode).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Elevation in metres at each point (None outside the installed DEM), for route profiles.
#[tauri::command]
pub async fn elevation_profile(state: State<'_, Arc<AppState>>, points: Vec<LonLat>) -> R<Vec<Option<f32>>> {
    let archive = state.paths().terrain_file_in_use();
    if !archive.exists() {
        // no elevation data installed: the public Terrarium tiles when the internet may be used
        if state.settings.read().routing_mode != "offline" {
            return tauri::async_runtime::spawn_blocking(move || crate::data::terrain::elevations_online(&points).map_err(err)).await.map_err(err)?;
        }
        return Err("NO_TERRAIN_DATA".into());
    }
    tauri::async_runtime::spawn_blocking(move || crate::data::terrain::elevations(&archive, &points).map_err(err)).await.map_err(err)?
}
