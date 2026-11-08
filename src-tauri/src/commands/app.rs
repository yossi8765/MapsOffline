//! App shell: settings, history & favorites, program updates, app info, start-up link, logs.
//!
//! Owner: R1 (data / infra / security stream).
#[allow(unused_imports)]
use super::prelude::*;

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
    group: Option<String>,
    color: Option<String>,
    note: Option<String>,
) -> R<Favorite> {
    state
        .with_history(|h| {
            h.favorite_add_ext(&name, &icon, lon, lat, address.as_deref(), kind.as_deref().unwrap_or("address"), data.as_ref(), group.as_deref(), color.as_deref(), note.as_deref())
        })
        .map_err(err)
}

/// Change some fields of a favorite (absent = unchanged, null = cleared); returns the row.
#[tauri::command(async)]
pub fn favorites_patch(state: State<'_, Arc<AppState>>, id: i64, patch: FavoritePatch) -> R<Favorite> {
    state.with_history(|h| h.favorite_patch(id, &patch)).map_err(err)
}

/// Folder names in use (for the folder picker).
#[tauri::command(async)]
pub fn favorites_groups(state: State<'_, Arc<AppState>>) -> R<Vec<String>> {
    state.with_history(|h| h.favorite_groups()).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_update(state: State<'_, Arc<AppState>>, id: i64, name: String, icon: String) -> R<()> {
    state.with_history(|h| h.favorite_update(id, &name, &icon)).map_err(err)
}

#[tauri::command(async)]
pub fn favorites_delete(state: State<'_, Arc<AppState>>, id: i64) -> R<()> {
    state.with_history(|h| h.favorite_delete(id)).map_err(err)
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
    // its own flag and cancel handle: a program download never shows up as a data update
    if st.app_updating.swap(true, Ordering::SeqCst) {
        return Err("UPDATE_RUNNING".into());
    }
    st.app_cancel.reset();
    let st2 = st.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let progress = |phase: ProgressPhase, done: u64, total: u64, msg: &str| -> bool {
            let _ = app.emit("app-update-progress", AppUpdateProgress { phase, done, total, message: msg.to_string() });
            !st2.app_cancel.is_cancelled()
        };
        let r = crate::app_update::download(&progress, &st2.app_cancel).map(|p| p.display().to_string()).map_err(err);
        st2.app_updating.store(false, Ordering::SeqCst);
        r
    })
    .await
    .map_err(err)?;
    res
}

/// Progress of `app_update_download` (event `app-update-progress`). Same shape as before minus
/// the meaningless `component` field.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppUpdateProgress {
    phase: ProgressPhase,
    done: u64,
    total: u64,
    message: String,
}

/// Stop a running program download.
#[tauri::command]
pub fn app_update_cancel(state: State<'_, Arc<AppState>>) -> R<()> {
    state.app_cancel.cancel();
    Ok(())
}

/// Start the verified installer and quit, so the installer can replace the running program.
#[tauri::command]
pub fn app_update_install(app: AppHandle, path: String) -> R<()> {
    // launch_installer checks the canonical path and re-hashes the file against the signed list
    crate::app_update::launch_installer(std::path::Path::new(&path)).map_err(err)?;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        app.exit(0);
    });
    Ok(())
}

// ---------------------------------------------------------------- settings

#[tauri::command]
pub fn settings_get(state: State<'_, Arc<AppState>>) -> R<Settings> {
    Ok(state.settings.read().clone())
}

#[tauri::command(async)]
pub fn settings_set(app: AppHandle, state: State<'_, Arc<AppState>>, settings: Settings) -> R<Settings> {
    let dir_changed = settings.data_dir != state.settings.read().data_dir;
    let settings = sanitize_settings(settings);
    // a new data folder must exist (or be creatable) and be writable before anything switches
    let new_root = dir_changed.then(|| {
        settings
            .data_dir
            .clone()
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(crate::paths::DataPaths::default_root)
    });
    if let Some(root) = &new_root {
        crate::paths::check_data_dir(root).map_err(err)?;
    }
    *state.settings.write() = settings.clone();
    state.save_settings().map_err(err)?;
    crate::nav::set_niqqud(settings.nav_voice_niqqud);
    if let Some(new_root) = new_root {
        let p = crate::paths::DataPaths::new(new_root);
        p.ensure_dirs().map_err(|e| err(anyhow::Error::new(e).context("INVALID_DATA_DIR")))?;
        crate::allow_asset_dir(&app, &p.root);
        *state.paths.write() = p;
        state.unload_all();
        *state.history.lock() = None;
    }
    Ok(settings)
}

/// Clamp values the webview sends to what the program understands.
fn sanitize_settings(mut s: Settings) -> Settings {
    s.kohanim_level = s.kohanim_level.min(3);
    if !matches!(s.map_source.as_str(), "standard" | "osmand") {
        s.map_source = "standard".into();
    }
    if !matches!(s.color_scheme.as_str(), "white" | "sky" | "mint" | "lavender" | "peach" | "rose" | "sand" | "aqua") {
        s.color_scheme = crate::types::default_color_scheme();
    }
    if !matches!(s.nav_vehicle.as_str(), "arrow" | "car" | "suv" | "van" | "truck" | "motorbike" | "bus") {
        s.nav_vehicle = crate::types::default_nav_vehicle();
    }
    if !matches!(s.nav_vehicle_color.as_str(), "accent" | "white" | "silver" | "black" | "gray" | "blue" | "red" | "yellow") {
        s.nav_vehicle_color = crate::types::default_nav_vehicle_color();
    }
    s.nav_sim_speed_kmh = s.nav_sim_speed_kmh.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 300.0));
    s.nav_sim_walk_kmh = s.nav_sim_walk_kmh.filter(|v| v.is_finite()).map(|v| v.clamp(1.0, 10.0));
    if !matches!(s.coord_format.as_str(), "dd" | "dms" | "itm" | "utm") {
        s.coord_format = crate::types::default_coord_format();
    }
    let ok_scale = match &s.ui_scale {
        serde_json::Value::String(v) => v == "system",
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| (1.0..=1.25).contains(&f)),
        _ => false,
    };
    if !ok_scale {
        s.ui_scale = crate::types::default_ui_scale();
    }
    s.custom_layers.retain(|l| matches!(l.kind.as_str(), "xyz" | "wms" | "pmtiles" | "mbtiles") && !l.id.is_empty() && crate::user_tiles::valid_id(&l.id));
    for l in &mut s.custom_layers {
        l.opacity = l.opacity.clamp(0.0, 1.0);
    }
    s.teaching_tips_seen.truncate(200);
    s.satellite_areas.retain(crate::data::sat_areas::valid);
    s.satellite_areas.truncate(100);
    s
}

#[tauri::command]
pub fn app_info() -> R<serde_json::Value> {
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "bbox": crate::paths::BBOX_ISRAEL,
    }))
}

/// The link the app was started with, once (later links arrive as the "open-link" event).
#[tauri::command]
pub fn take_pending_link() -> Option<String> {
    crate::PENDING_LINK.lock().take()
}

// ---------------------------------------------------------------- diagnostics

/// Folder of the log files (`<data root>\logs`); the UI opens it for bug reports.
#[tauri::command]
pub fn logs_dir(state: State<'_, Arc<AppState>>) -> R<String> {
    let dir = crate::logging::dir().unwrap_or_else(|| state.paths().root.join("logs"));
    Ok(dir.display().to_string())
}

// ---------------------------------------------------------------- Windows look, connectivity

/// Windows accent colours (`null` elsewhere); changes arrive as event `system-accent-changed`.
#[tauri::command(async)]
pub fn system_accent() -> R<Option<SystemAccent>> {
    Ok(crate::system::accent())
}

/// Windows text scale factor (Settings → Accessibility → Text size), 1.0 ..= 2.25.
#[tauri::command(async)]
pub fn system_text_scale() -> R<f64> {
    Ok(crate::system::text_scale())
}

/// Is the internet reachable right now (1.5 s probe, remembered for 30 s)?
#[tauri::command(async)]
pub fn connectivity() -> R<bool> {
    Ok(crate::system::connectivity())
}

// ---------------------------------------------------------------- user layers

#[tauri::command(async)]
pub fn user_layers_list(state: State<'_, Arc<AppState>>) -> R<Vec<UserLayer>> {
    Ok(crate::user_layers::list(&state.paths()))
}

#[tauri::command(async)]
pub fn user_layer_get(state: State<'_, Arc<AppState>>, id: String) -> R<String> {
    crate::user_layers::get(&state.paths(), &id).map_err(err)
}

#[tauri::command(async)]
pub fn user_layer_save(state: State<'_, Arc<AppState>>, name: String, kind: String, geojson: String, color: Option<String>) -> R<UserLayer> {
    crate::user_layers::save(&state.paths(), &name, &kind, &geojson, color.as_deref()).map_err(err)
}

#[tauri::command(async)]
pub fn user_layer_update(state: State<'_, Arc<AppState>>, id: String, patch: UserLayerPatch) -> R<UserLayer> {
    crate::user_layers::update(&state.paths(), &id, &patch).map_err(err)
}

#[tauri::command(async)]
pub fn user_layer_delete(state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    crate::user_layers::delete(&state.paths(), &id).map_err(err)
}

/// Forget the disk cache of an online custom layer (after its URL changed or it was removed).
#[tauri::command(async)]
pub fn custom_layer_clear_cache(state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    crate::user_tiles::clear_cache(&state.paths(), &id);
    Ok(())
}
