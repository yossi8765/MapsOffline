//! MapsOffline – library crate (tauri app + reusable modules for the data CLI).
pub mod commands;
pub mod data;
pub mod geo;
pub mod geocode;
pub mod history;
pub mod location;
pub mod app_update;
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::geocode_search,
            commands::geocode_reverse,
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
            commands::satellite_path,
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
