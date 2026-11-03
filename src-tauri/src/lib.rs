//! MapsOffline – library crate (tauri app + reusable modules for the data CLI).
pub mod commands;
pub mod data;
pub mod geo;
pub mod geocode;
pub mod history;
pub mod location;
pub mod map_tiles;
pub mod app_update;
pub mod online_tiles;
pub mod online_sat;
pub mod online;
pub mod paths;
pub mod routing;
pub mod state;
pub mod transit;
pub mod types;

use std::sync::Arc;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).try_init();
    let state = Arc::new(AppState::new());
    state.refresh_map_files();
    tauri::Builder::default()
        // Online map without local data: the webview reads the Protomaps world build with HTTP
        // range requests through this proxy (the build host sends no CORS headers).
        .register_asynchronous_uri_scheme_protocol("mo-tiles", |_ctx, request, responder| {
            std::thread::spawn(move || responder.respond(online_tiles::proxy(request)));
        })
        // Every installed detailed map (Israel + downloaded countries) as one vector-tile source.
        .register_asynchronous_uri_scheme_protocol("mo-map", |_ctx, request, responder| {
            std::thread::spawn(move || responder.respond(map_tiles::serve(request)));
        })
        // Satellite layer: downloaded archive, else tiles seen before, else live from govmap / EOX.
        .register_asynchronous_uri_scheme_protocol("mo-sat", |ctx, request, responder| {
            use tauri::Manager;
            let st = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
            std::thread::spawn(move || responder.respond(online_sat::serve(st, request)));
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::geocode_search,
            commands::geocode_reverse,
            commands::regions_catalog,
            commands::regions_locate,
            commands::save_text_file,
            commands::regions_installed,
            commands::region_size,
            commands::region_install,
            commands::region_cancel,
            commands::region_delete,
            commands::map_tiles_available,
            commands::route,
            commands::transit_stops,
            commands::poi_layer,
            commands::transit_line,
            commands::transit_agencies,
            commands::transit_lines,
            commands::transit_line_times,
            commands::transit_stop_board,
            commands::history_list,
            commands::history_add,
            commands::history_delete,
            commands::history_clear,
            commands::favorites_list,
            commands::favorites_add,
            commands::favorites_update,
            commands::favorites_delete,
            commands::data_status,
            commands::tiles_path,
            commands::tiles_online_key,
            commands::satellite_path,
            commands::world_path,
            commands::terrain_path,
            commands::locate,
            commands::app_update_check,
            commands::app_update_download,
            commands::app_update_install,
            commands::optimize_order,
            commands::elevation_profile,
            commands::data_update,
            commands::data_check,
            commands::data_export,
            commands::data_import,
            commands::data_cancel,
            commands::data_pause,
            commands::data_preload,
            commands::settings_get,
            commands::settings_set,
            commands::app_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MapsOffline");
}
