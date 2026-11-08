//! MapsOffline – library crate (tauri app + reusable modules for the data CLI).
// Deliberate style in this codebase (algorithm code indexes parallel arrays; the Tauri commands
// and builders take many plain parameters; callback types are spelled out where used).
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::needless_range_loop)]
pub mod commands;
pub mod coords;
pub mod data;
pub mod error;
pub mod logging;
pub mod geo;
#[cfg(target_os = "linux")]
pub mod geoclue;
pub mod geocode;
pub mod history;
pub mod location;
pub mod map_tiles;
pub mod nav;
pub mod obf;
pub mod voice;
pub mod app_update;
pub mod online_tiles;
pub mod online_sat;
pub mod online;
pub mod paths;
pub mod routing;
pub mod state;
pub mod speech;
pub mod system;
pub mod transit;
pub mod types;
pub mod user_layers;
pub mod user_tiles;

use std::sync::Arc;

use state::AppState;

/// Maximise the main window when its restored rectangle sticks out of its monitor's work area
/// (above the top edge, or wider / taller than the screen): that is a maximised window saved
/// without the flag, and it would show with the title bar cut off.
fn fix_offscreen_window(w: &tauri::WebviewWindow) {
    if w.is_maximized().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size), Ok(Some(mon))) = (w.outer_position(), w.outer_size(), w.current_monitor()) else { return };
    let area = mon.work_area();
    let (ax, ay) = (area.position.x, area.position.y);
    let (aw, ah) = (area.size.width as i32, area.size.height as i32);
    let off_top = pos.y < ay;
    let too_big = size.width as i32 >= aw - 4 && size.height as i32 >= ah - 40;
    let off_side = pos.x < ax - 4 || pos.x + size.width as i32 > ax + aw + 4;
    if off_top || too_big || off_side {
        let _ = w.maximize();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(AppState::new());
    logging::init(&state.paths().root.join("logs"));
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
        // (every scheme answers on a bounded worker pool: local file reads apart from network waits)
        .register_asynchronous_uri_scheme_protocol("mo-tiles", |_ctx, request, responder| {
            system::net_pool().run(move || responder.respond(online_tiles::proxy(request)));
        })
        // Every installed detailed map (Israel + downloaded countries) as one vector-tile source.
        .register_asynchronous_uri_scheme_protocol("mo-map", |_ctx, request, responder| {
            system::local_pool().run(move || responder.respond(map_tiles::serve(request)));
        })
        // Satellite layer: downloaded archive, else tiles seen before, else live from govmap / EOX.
        .register_asynchronous_uri_scheme_protocol("mo-sat", |ctx, request, responder| {
            use tauri::Manager;
            let st = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
            system::net_pool().run(move || responder.respond(online_sat::serve(st, request)));
        })
        // The user's own tile layers (XYZ / WMS online with a disk cache, PMTiles / MBTiles files).
        .register_asynchronous_uri_scheme_protocol("mo-user", |ctx, request, responder| {
            use tauri::Manager;
            let st = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
            let pool = if user_tiles::is_local(&st, request.uri().path()) { system::local_pool() } else { system::net_pool() };
            pool.run(move || responder.respond(user_tiles::serve(st, request)));
        })
        // size, position and maximised state of the windows survive a restart
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        // The asset protocol (PMTiles opened by the webview) may read the active data folder
        // only, not the whole disk; settings_set adds a newly chosen folder.
        .setup(|app| {
            use tauri::Manager;
            let root = app.state::<Arc<AppState>>().paths().root;
            allow_asset_dir(app.handle(), &root);
            // A window restored by the window-state plugin with the size of a maximised window but
            // not flagged maximised sits partly off-screen (title bar cut at the top). Such a
            // geometry means "maximised": maximise it properly.
            if let Some(w) = app.get_webview_window("main") {
                fix_offscreen_window(&w);
            }
            // Installed builds register the link schemes through the installer; a dev / portable
            // run registers them for the current user so links work there too.
            #[cfg(windows)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register_all();
            }
            // a link that started the app waits until the webview asks for it (take_pending_link)
            *PENDING_LINK.lock() = link_arg(&std::env::args().collect::<Vec<_>>());
            system::watch_accent(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // ---- R1 data / infra / security (commands/data.rs, commands/app.rs)
            commands::data::data_status,
            commands::data::data_check,
            commands::data::data_update,
            commands::data::data_export,
            commands::data::data_import,
            commands::data::data_download_package,
            commands::data::data_cancel,
            commands::data::data_pause,
            commands::data::data_preload,
            commands::data::tiles_path,
            commands::data::tiles_online_key,
            commands::data::satellite_path,
            commands::data::world_path,
            commands::data::terrain_path,
            commands::data::trails_path,
            commands::data::minyanim_for,
            commands::data::minyanim_next,
            commands::data::regions_catalog,
            commands::data::regions_locate,
            commands::data::regions_installed,
            commands::data::region_size,
            commands::data::region_install,
            commands::data::region_cancel,
            commands::data::region_delete,
            commands::data::obf_catalog,
            commands::data::obf_install,
            commands::data::data_usage,
            commands::data::data_delete,
            commands::data::map_tiles_available,
            commands::data::save_text_file,
            commands::data::read_text_file,
            commands::app::settings_get,
            commands::app::settings_set,
            commands::app::app_info,
            commands::app::take_pending_link,
            commands::app::logs_dir,
            commands::app::history_list,
            commands::app::history_add,
            commands::app::history_delete,
            commands::app::history_clear,
            commands::app::favorites_list,
            commands::app::favorites_add,
            commands::app::favorites_update,
            commands::app::favorites_delete,
            commands::app::app_update_check,
            commands::app::app_update_download,
            commands::app::app_update_install,
            commands::app::app_update_cancel,
            commands::app::favorites_patch,
            commands::app::favorites_groups,
            commands::app::system_accent,
            commands::app::system_text_scale,
            commands::app::connectivity,
            commands::app::user_layers_list,
            commands::app::user_layer_get,
            commands::app::user_layer_save,
            commands::app::user_layer_update,
            commands::app::user_layer_delete,
            commands::app::custom_layer_clear_cache,
            commands::data::package_info,
            commands::data::satellite_area_install,
            commands::data::satellite_area_cancel,
            commands::data::satellite_area_pause,
            commands::data::satellite_area_delete,
            commands::data::satellite_areas_status,
            commands::data::satellite_area_estimate,
            // [R1] new commands above
            // ---- R2 routing + navigation (commands/route.rs, commands/nav.rs)
            commands::route::route,
            commands::route::kohanim_areas,
            commands::route::optimize_order,
            commands::route::elevation_profile,
            commands::nav::locate,
            commands::nav::nav_start,
            commands::nav::nav_simulate,
            commands::nav::nav_set_route,
            commands::nav::nav_set_voice,
            commands::nav::nav_stop,
            commands::nav::nav_set_sim_speed,
            commands::nav::nav_set_sim_paused,
            commands::nav::nav_sim_jump,
            commands::nav::voice_test,
            commands::nav::speak,
            commands::nav::voices_list,
            commands::route::optimize_trip,
            commands::route::isochrone,
            commands::route::match_track,
            commands::nav::track_start,
            commands::nav::track_stop,
            // [R2] new commands above
            // ---- R3 transit (commands/transit.rs)
            commands::transit::transit_stops,
            commands::transit::transit_line,
            commands::transit::transit_agencies,
            commands::transit::transit_lines,
            commands::transit::transit_line_times,
            commands::transit::transit_stop_board,
            commands::transit::transit_delays,
            commands::transit::transit_isochrone,
            commands::transit::transit_vehicles,
            // [R3] new commands above
            // ---- R4 geocode (commands/geo.rs)
            commands::geo::geocode_search,
            commands::geo::geocode_reverse,
            commands::geo::poi_layer,
            commands::geo::geocode_batch,
            commands::geo::coords_parse,
            commands::geo::wiki_summary,
            // voice input for the search boxes (commands/speech.rs)
            commands::speech::speech_status,
            commands::speech::speech_install,
            commands::speech::speech_cancel,
            commands::speech::speech_delete,
            commands::speech::speech_transcribe,
            commands::speech::speech_system,
            // [R4] new commands above
        ])
        .build(tauri::generate_context!())
        .expect("error while running MapsOffline")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                app.state::<Arc<AppState>>().on_exit();
            }
        });
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
