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
        crate::allow_asset_dir(&app, &p.root);
        *state.paths.write() = p;
        state.unload_all();
        *state.history.lock() = None;
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
