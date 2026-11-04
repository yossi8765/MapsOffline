//! MapsOffline – library crate (tauri app + reusable modules for the data CLI).
// Deliberate style in this codebase (algorithm code indexes parallel arrays; the Tauri commands
// and builders take many plain parameters; callback types are spelled out where used).
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::needless_range_loop)]
pub mod commands;
pub mod data;
pub mod geo;
pub mod geocode;
pub mod history;
pub mod location;
pub mod map_tiles;
pub mod nav;
pub mod voice;
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
    {
        // the data update closes the pooled index connections before swapping the file
        let weak = Arc::downgrade(&state);
        geocode::set_release_hook(move || {
            if let Some(st) = weak.upgrade() {
                st.release_geocoder();
            }
        });
    }
    tauri::Builder::default()
        // Must be the first plugin: a second start hands its arguments (a mapsoffline:// or geo:
        // link) to the running window and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            use tauri::{Emitter, Manager};
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            if let Some(url) = link_arg(&argv) {
                let _ = app.emit("open-link", url);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
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
        // The asset protocol (PMTiles opened by the webview) may read the active data folder
        // only, not the whole disk; settings_set adds a newly chosen folder.
        .setup(|app| {
            use tauri::Manager;
            let root = app.state::<Arc<AppState>>().paths().root;
            allow_asset_dir(app.handle(), &root);
            // Installed builds register the link schemes through the installer; a dev / portable
            // run registers them for the current user so links work there too.
            #[cfg(windows)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register_all();
            }
            // a link that started the app waits until the webview asks for it (take_pending_link)
            *PENDING_LINK.lock() = link_arg(&std::env::args().collect::<Vec<_>>());
            Ok(())
        })
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
            commands::transit_delays,
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
            commands::trails_path,
            commands::minyanim_for,
            commands::minyanim_next,
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
            commands::take_pending_link,
            commands::nav_start,
            commands::nav_simulate,
            commands::nav_set_route,
            commands::nav_set_voice,
            commands::nav_stop,
            commands::speak,
            commands::voices_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MapsOffline");
}

/// Let the webview read files under `dir` through the asset protocol (the static scope in
/// tauri.conf.json is empty; the data folder can be the default, a portable or a custom one).
pub fn allow_asset_dir(app: &tauri::AppHandle, dir: &std::path::Path) {
    use tauri::Manager;
    if let Err(e) = app.asset_protocol_scope().allow_directory(dir, true) {
        log::warn!("asset scope {}: {e}", dir.display());
    }
}

/// Link that started the app (first instance), handed to the webview once it is ready.
pub(crate) static PENDING_LINK: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// The first `mapsoffline:` or `geo:` argument of a command line.
fn link_arg(argv: &[String]) -> Option<String> {
    argv.iter()
        .skip(1)
        .find(|a| {
            let l = a.to_ascii_lowercase();
            l.starts_with("mapsoffline:") || l.starts_with("geo:")
        })
        .cloned()
}
