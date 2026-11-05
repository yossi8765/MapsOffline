//! Data components: status / update / check / package import-export, file paths of the
//! installed archives, prayer times, other countries (regions), file export.
//!
//! Owner: R1 (data / infra / security stream).
#[allow(unused_imports)]
use super::prelude::*;

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
    {
        let j = state.data_jobs.lock();
        s.updating = j.busy();
        s.active = j.running.clone();
        s.queued = j.queued.clone();
    }
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

/// Newest Protomaps build key ("YYYYMMDD.pmtiles") for the online map fallback.
#[tauri::command]
pub async fn tiles_online_key() -> R<String> {
    tauri::async_runtime::spawn_blocking(|| {
        let client = crate::data::download::http_client().map_err(err)?;
        crate::data::tiles::latest_build_key(&client).map_err(err)
    })
    .await
    .map_err(err)?
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
    let paths = state.paths();
    let p = paths.satellite_file();
    if p.exists() {
        return Ok(Some(p.display().to_string()));
    }
    // sharp areas alone also make an offline satellite layer
    let areas = state.settings.read().satellite_areas.clone();
    Ok(crate::data::sat_areas::any_installed(&paths, &areas).then(|| crate::data::sat_areas::dir(&paths).display().to_string()))
}

// ---------------------------------------------------------------- sharp imagery areas

fn find_area(state: &AppState, id: &str) -> R<SatelliteArea> {
    state.settings.read().satellite_areas.iter().find(|a| a.id == id).cloned().ok_or_else(|| AppError::from("INVALID_AREA: unknown area"))
}

/// Download (or resume / update) a sharp-imagery area from Settings.satelliteAreas. Long running;
/// progress: event `satellite-area-progress` {id, phase, done, total, message}; `data-updated`
/// when it finished.
#[tauri::command]
pub async fn satellite_area_install(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    let area = find_area(&state, &id)?;
    let paths = state.paths();
    let app2 = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |p: SatelliteAreaProgress| {
            let _ = app2.emit("satellite-area-progress", p);
        };
        crate::data::sat_areas::install(&paths, &area, &emit)
    })
    .await
    .map_err(err)?;
    let _ = app.emit("data-updated", ());
    res.map_err(err)
}

#[tauri::command]
pub fn satellite_area_cancel(id: String) -> R<()> {
    crate::data::sat_areas::cancel(&id);
    Ok(())
}

#[tauri::command]
pub fn satellite_area_pause(id: String, paused: bool) -> R<()> {
    crate::data::sat_areas::pause(&id, paused);
    Ok(())
}

/// Delete an area's archive (the frontend removes it from Settings).
#[tauri::command]
pub async fn satellite_area_delete(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || crate::data::sat_areas::delete(&paths, &id)).await.map_err(err)?.map_err(err)?;
    let _ = app.emit("data-updated", ());
    Ok(())
}

#[tauri::command]
pub fn satellite_areas_status(state: State<'_, Arc<AppState>>) -> R<Vec<SatelliteAreaStatus>> {
    let areas = state.settings.read().satellite_areas.clone();
    Ok(crate::data::sat_areas::status(&state.paths(), &areas))
}

/// Tiles, size and time of an area before downloading it.
#[tauri::command]
pub async fn satellite_area_estimate(state: State<'_, Arc<AppState>>, bbox: [f64; 4], zoom: u8) -> R<SatelliteAreaEstimate> {
    let paths = state.paths();
    let zoom = zoom.clamp(crate::data::sat_areas::MIN_AREA_ZOOM, crate::data::sat_areas::MAX_AREA_ZOOM);
    tauri::async_runtime::spawn_blocking(move || {
        let min = crate::data::sat_areas::min_zoom(&paths, zoom);
        crate::data::sat_areas::estimate(bbox, min, zoom)
    })
    .await
    .map_err(err)
}

/// Absolute path of the elevation archive, if installed.
#[tauri::command]
pub fn terrain_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().terrain_file_in_use();
    Ok(p.exists().then(|| p.display().to_string()))
}

/// Absolute path of the marked-trails GeoJSON, if installed (map: geojson source via the asset protocol).
#[tauri::command]
pub fn trails_path(state: State<'_, Arc<AppState>>) -> R<Option<String>> {
    let p = state.paths().trails_file();
    Ok(p.exists().then(|| p.display().to_string()))
}

/// Payload of event `minyanim-live`: the live timetable that replaces the snapshot.
#[derive(Clone, serde::Serialize)]
struct MinyanimLive {
    lon: f64,
    lat: f64,
    table: crate::data::minyanim::MinyanTable,
}

/// Weekly prayer timetable of the synagogue at this point (`poi_name` helps between neighbours).
/// Answers at once from the installed weekly snapshot; when online, the current timetable from
/// Nedarim Plus follows as event `minyanim-live` {lon, lat, table} (it used to make the place card
/// wait up to 16 s). Err "NO_MINYANIM_DATA" when offline and the component is not installed.
#[tauri::command]
pub async fn minyanim_for(app: AppHandle, state: State<'_, Arc<AppState>>, lon: f64, lat: f64, poi_name: Option<String>) -> R<Option<crate::data::minyanim::MinyanTable>> {
    let paths = state.paths();
    let online = state.settings.read().routing_mode != "offline";
    if online {
        let (paths, name) = (paths.clone(), poi_name.clone());
        std::thread::spawn(move || match crate::data::minyanim::table_live(&paths, lon, lat, name.as_deref()) {
            Ok(Some(table)) => {
                let _ = app.emit("minyanim-live", MinyanimLive { lon, lat, table });
            }
            Ok(None) => {}
            Err(e) => log::info!("live minyanim failed, the snapshot stays: {e:#}"),
        });
    }
    tauri::async_runtime::spawn_blocking(move || match crate::data::minyanim::table_for(&paths, lon, lat, poi_name.as_deref()) {
        Ok(t) => Ok(t),
        // not installed: the live answer may still come
        Err(_) if online => Ok(None),
        Err(e) => Err(err(e)),
    })
    .await
    .map_err(err)?
}

/// Prayers starting soon around a point (default: within 180 minutes), soonest first.
#[tauri::command]
pub async fn minyanim_next(state: State<'_, Arc<AppState>>, lon: f64, lat: f64, radius_m: f64, within_min: Option<i64>) -> R<Vec<crate::data::minyanim::NextMinyan>> {
    let paths = state.paths();
    let online = state.settings.read().routing_mode != "offline";
    tauri::async_runtime::spawn_blocking(move || {
        if online {
            match crate::data::minyanim::next_live(lon, lat, radius_m, within_min) {
                Ok(v) => return Ok(v),
                Err(e) => log::info!("live minyanim failed, using the snapshot: {e:#}"),
            }
        }
        match crate::data::minyanim::next_near(&paths, lon, lat, radius_m, within_min) {
            Ok(v) => Ok(v),
            Err(_) if online => Ok(Vec::new()),
            Err(e) => Err(err(e)),
        }
    })
    .await
    .map_err(err)?
}

/// Update data components. Several requests may run side by side (see `data::jobs`): components
/// with their own sources start at once, OSM-derived ones wait while another job builds from the
/// OSM extract, components already running are ignored. Resolves when this request's components
/// are done; `data-updated` follows every finished job.
#[tauri::command]
pub async fn data_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    components: Vec<DataComponent>,
) -> R<()> {
    let st = state.inner().clone();
    // "update everything" = everything the user wants (Settings.wantedComponents), else all
    let wanted = st.settings.read().wanted_components.clone();
    let comps = if components.is_empty() { wanted.unwrap_or_else(|| DataComponent::ALL.to_vec()) } else { components };
    let plan = st.begin_component_job(&comps).map_err(AppError::from)?;
    if !plan.ignored.is_empty() {
        log::info!("already updating: {:?}", plan.ignored);
    }
    let emit_fn = {
        let app = app.clone();
        move |p: DataProgress| {
            let _ = app.emit("data-progress", p);
        }
    };
    for &c in &plan.queue {
        emit_fn(DataProgress { component: c, phase: ProgressPhase::Queued, done: 0, total: 0, message: String::new() });
    }

    // one job: build, drop what it replaced (reloaded lazily), announce
    let run = |comps: Vec<DataComponent>| {
        let (st, app) = (st.clone(), app.clone());
        let emit: crate::data::Emit = Arc::new(emit_fn.clone());
        async move {
            if comps.is_empty() {
                return Ok(());
            }
            let st2 = st.clone();
            let paths = st.paths();
            let res = tauri::async_runtime::spawn_blocking(move || {
                let r = crate::data::update(&comps, &paths, emit, &st2.cancel);
                st2.unload(&comps);
                st2.end_component_job(&comps);
                r
            })
            .await
            .map_err(err)?;
            let _ = app.emit("data-updated", ());
            res.map_err(err)
        }
    };

    let now = run(plan.start.clone());
    let later = {
        let (st, queue, emit_fn) = (st.clone(), plan.queue.clone(), emit_fn.clone());
        let run = &run;
        async move {
            if queue.is_empty() {
                return Ok(());
            }
            // wait (off the async runtime) until no running job builds from the OSM extract
            let st2 = st.clone();
            let start = tauri::async_runtime::spawn_blocking(move || loop {
                let (start, cancelled) = st2.promote_queued(&queue);
                for c in cancelled {
                    emit_fn(DataProgress { component: c, phase: ProgressPhase::Cancelled, done: 0, total: 0, message: String::new() });
                }
                if let Some(s) = start {
                    return s;
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            })
            .await
            .map_err(err)?;
            run(start).await
        }
    };
    let (a, b) = futures_join(now, later).await;
    a.and(b)
}

/// Await two futures side by side (no extra crate: tauri's runtime spawns both).
async fn futures_join<A, B>(a: A, b: B) -> (R<()>, R<()>)
where
    A: std::future::Future<Output = R<()>>,
    B: std::future::Future<Output = R<()>>,
{
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(move |cx| {
        use std::task::Poll;
        if ra.is_none() {
            if let Poll::Ready(r) = a.as_mut().poll(cx) {
                ra = Some(r);
            }
        }
        if rb.is_none() {
            if let Poll::Ready(r) = b.as_mut().poll(cx) {
                rb = Some(r);
            }
        }
        if ra.is_some() && rb.is_some() {
            Poll::Ready((ra.take().unwrap_or(Ok(())), rb.take().unwrap_or(Ok(()))))
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Pack the installed components (all when `components` is empty / missing) into one `.mapsdata`
/// file (for machines without internet).
#[tauri::command]
pub async fn data_export(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    dest: String,
    components: Option<Vec<DataComponent>>,
    regions: Option<Vec<String>>,
    areas: Option<Vec<String>>,
) -> R<Vec<DataComponent>> {
    check_user_path(&state, &dest, &["mapsdata"]).map_err(err)?;
    let regions = regions.unwrap_or_default();
    let area_ids = areas.unwrap_or_default();
    // missing = every component; an explicit empty list = none (only countries / areas)
    let want: Vec<DataComponent> = components.unwrap_or_else(|| DataComponent::ALL.to_vec());
    // countries being installed and areas being downloaded have half-written files
    let regions: Vec<String> = regions.into_iter().filter(|id| !state.region_busy(id)).collect();
    let areas: Vec<crate::types::SatelliteArea> =
        state.settings.read().satellite_areas.iter().filter(|a| area_ids.contains(&a.id) && !crate::data::sat_areas::is_busy(&a.id)).cloned().collect();
    let extras = crate::data::package::Extras { regions, areas };
    run_package_job(app, state.inner().clone(), move |paths, progress, cancel| {
        crate::data::package::export_with(paths, std::path::Path::new(&dest), &want, &extras, progress, cancel)
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
    let info = tauri::async_runtime::spawn_blocking(move || crate::data::package::inspect(&p2)).await.map_err(err)?.map_err(err)?;
    // countries in the package: hidden from the map and the search while their files are
    // replaced (as during a country install); one already being installed stops the import
    let ids: Vec<String> = info.regions.iter().filter(|id| crate::data::regions::valid_region_id(id)).cloned().collect();
    {
        let mut jobs = st.region_jobs.lock();
        if ids.iter().any(|id| jobs.contains_key(id)) {
            return Err("REGION_BUSY".into());
        }
        for id in &ids {
            jobs.insert(id.clone(), crate::data::Cancel::default());
        }
    }
    if !ids.is_empty() {
        st.refresh_map_files();
        st.unload_regions();
    }
    let extras = Arc::new(parking_lot::Mutex::new(crate::data::package::Extras::default()));
    let ex2 = extras.clone();
    let res = run_package_job(app.clone(), st.clone(), move |paths, progress, cancel| {
        let (installed, ex) = crate::data::package::import_with(paths, &src_path, &[], progress, cancel)?;
        *ex2.lock() = ex;
        Ok(installed)
    })
    .await;
    {
        let mut jobs = st.region_jobs.lock();
        for id in &ids {
            jobs.remove(id);
        }
    }
    if !ids.is_empty() {
        st.refresh_map_files();
    }
    // sharp areas: add their definitions to Settings (an area with the same id is replaced)
    let areas = std::mem::take(&mut extras.lock().areas);
    if !areas.is_empty() {
        {
            let mut s = st.settings.write();
            for a in areas.into_iter().filter(crate::data::sat_areas::valid) {
                s.satellite_areas.retain(|x| x.id != a.id);
                s.satellite_areas.push(a);
            }
        }
        if let Err(e) = st.save_settings() {
            log::warn!("saving settings after import: {e:#}");
        }
        // the frontend reloads Settings after an import
    }
    match &res {
        Ok(installed) => st.unload(installed),
        Err(_) => st.unload_all(),
    }
    let _ = app.emit("data-updated", ());
    res
}

/// The full ready-made data package of the weekly GitLab build (every component in one file).
const FULL_PACKAGE_URL: &str = "https://gitlab.com/api/v4/projects/RecoveryAdvancedFree%2FMapsOffline/packages/generic/mapsdata-all/latest/MapsOffline-data.mapsdata";

/// Progress of `data_download_package`, emitted as event `package-download`.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageDownloadProgress {
    phase: ProgressPhase,
    done: u64,
    total: u64,
    message: String,
}

/// Download the full ready-made data package to `dest` (to keep, carry to another computer or
/// import later). The file goes through the downloads folder first, so a broken or paused download
/// resumes where it stopped; cancel with `data_cancel()`. Returns the file size.
#[tauri::command]
pub async fn data_download_package(app: AppHandle, state: State<'_, Arc<AppState>>, dest: String) -> R<u64> {
    check_user_path(&state, &dest, &["mapsdata"]).map_err(err)?;
    let st = state.inner().clone();
    st.begin_exclusive_job().map_err(AppError::from)?;
    let paths = st.paths();
    let st2 = st.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<u64> {
        let emit = |phase: ProgressPhase, done: u64, total: u64, message: &str| {
            let _ = app.emit("package-download", PackageDownloadProgress { phase, done, total, message: message.to_string() });
        };
        let r = (|| -> anyhow::Result<u64> {
            let file = paths.downloads_dir().join("MapsOffline-data.mapsdata");
            let progress = |phase: ProgressPhase, d: u64, t: u64, msg: &str| -> bool {
                st2.cancel.wait_if_paused();
                emit(phase, d, t, msg);
                !st2.cancel.is_cancelled()
            };
            crate::data::download::ensure_fresh(FULL_PACKAGE_URL, &file, &progress, &st2.cancel)?;
            if st2.cancel.is_cancelled() {
                anyhow::bail!("בוטל");
            }
            emit(ProgressPhase::Finalizing, 0, 0, "שומר את הקובץ");
            let dest = std::path::PathBuf::from(&dest);
            if dest != file {
                // same drive: a rename; otherwise a copy
                if std::fs::rename(&file, &dest).is_err() {
                    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                    crate::data::disk::ensure_free(dest.parent().unwrap_or(&dest), size)?;
                    std::fs::copy(&file, &dest)?;
                    let _ = std::fs::remove_file(&file);
                }
                let _ = std::fs::remove_file(crate::data::download::meta_path(&file));
            }
            let size = std::fs::metadata(&dest)?.len();
            emit(ProgressPhase::Done, size, size, "הושלם");
            Ok(size)
        })();
        if let Err(e) = &r {
            let phase = if st2.cancel.is_cancelled() { ProgressPhase::Cancelled } else { ProgressPhase::Failed };
            emit(phase, 0, 0, &format!("{e:#}"));
        }
        st2.end_exclusive_job();
        r
    })
    .await
    .map_err(err)?;
    res.map_err(err)
}

async fn run_package_job<F>(app: AppHandle, st: Arc<AppState>, job: F) -> R<Vec<DataComponent>>
where
    F: FnOnce(&crate::paths::DataPaths, &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync), &crate::data::Cancel) -> anyhow::Result<Vec<DataComponent>>
        + Send
        + 'static,
{
    st.begin_exclusive_job().map_err(AppError::from)?;
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
        st2.end_exclusive_job();
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

// ---------------------------------------------------------------- other countries

/// Geofabrik region catalogue (continents, countries, sub-regions).
#[tauri::command]
pub async fn regions_catalog(state: State<'_, Arc<AppState>>, refresh: Option<bool>) -> R<Vec<crate::types::RegionEntry>> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || {
        crate::data::regions::catalog(&paths, refresh.unwrap_or(false)).map(|c| c.iter().map(|f| f.entry.clone()).collect()).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn regions_installed(state: State<'_, Arc<AppState>>) -> R<Vec<crate::types::InstalledRegion>> {
    // reads every country's meta and sizes: off the UI thread
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || crate::data::regions::installed(&st.paths(), &|id| st.region_busy(id))).await.map_err(err)
}

#[tauri::command]
pub async fn region_size(state: State<'_, Arc<AppState>>, id: String) -> R<crate::types::RegionSize> {
    let paths = state.paths();
    tauri::async_runtime::spawn_blocking(move || crate::data::regions::size(&paths, &id).map_err(err)).await.map_err(err)?
}

/// Install / update a country's map and / or search index; progress on `region-progress`.
#[tauri::command]
pub async fn region_install(app: AppHandle, state: State<'_, Arc<AppState>>, id: String, parts: Vec<crate::types::RegionPart>) -> R<()> {
    let st = state.inner().clone();
    let cancel = crate::data::Cancel::default();
    {
        let mut jobs = st.region_jobs.lock();
        if jobs.contains_key(&id) {
            return Err("REGION_BUSY".into());
        }
        jobs.insert(id.clone(), cancel.clone());
    }
    // hide the country from the map and the search while its files are replaced
    st.refresh_map_files();
    st.unload_regions();
    let st2 = st.clone();
    let id2 = id.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |p: crate::types::RegionProgress| {
            let _ = app.emit("region-progress", p);
        };
        let parts = if parts.is_empty() { vec![crate::types::RegionPart::Map, crate::types::RegionPart::Search] } else { parts };
        let r = crate::data::regions::install(&st2.paths(), &id2, &parts, &emit, &cancel);
        st2.region_jobs.lock().remove(&id2);
        st2.refresh_map_files();
        r
    })
    .await
    .map_err(err)?;
    res.map_err(err)
}

#[tauri::command]
pub fn region_cancel(state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    if let Some(c) = state.region_jobs.lock().get(&id) {
        c.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn region_delete(state: State<'_, Arc<AppState>>, id: String) -> R<()> {
    let st = state.inner().clone();
    if st.region_busy(&id) {
        return Err("REGION_BUSY".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        st.unload_regions();
        // leave the map source first so no request reads the file being removed
        let paths = st.paths();
        let keep: Vec<_> = crate::data::regions::ready_maps(&paths).into_iter().filter(|(i, _, _)| *i != id).collect();
        let mut files = Vec::new();
        if paths.tiles_file().exists() {
            files.push(paths.tiles_file());
        }
        files.extend(keep.into_iter().map(|(_, f, _)| f));
        crate::map_tiles::set_files(files);
        let r = crate::data::regions::delete(&paths, &id);
        st.refresh_map_files();
        r.map_err(err)
    })
    .await
    .map_err(err)?
}

/// True when the merged map source (`http://mo-map.localhost/{z}/{x}/{y}.mvt`) should be used:
/// at least one country besides Israel is installed.
#[tauri::command]
pub async fn map_tiles_available(state: State<'_, Arc<AppState>>) -> R<bool> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        st.refresh_map_files();
        !crate::data::regions::ready_maps(&st.paths()).is_empty()
    })
    .await
    .map_err(err)
}

/// Size and version of the combined ready-made data package (for the first-run choice); `null`
/// when the package host cannot be reached.
#[tauri::command]
pub async fn package_info() -> R<Option<PackageInfo>> {
    tauri::async_runtime::spawn_blocking(|| {
        let client = crate::data::download::http_client().ok()?;
        let m = crate::data::download::probe(&client, FULL_PACKAGE_URL).ok()?;
        Some(PackageInfo { size_bytes: m.content_length?, version: m.version().unwrap_or_default() })
    })
    .await
    .map_err(err)
}

/// A path the webview asks the program to write: absolute, one of `exts`, and outside the data
/// folder and the program folder (settings, manifest, indexes and the program itself must never
/// be overwritten through a "save as"). Errors carry `INVALID_PATH`.
pub(crate) fn check_user_path(state: &AppState, path: &str, exts: &[&str]) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!("INVALID_PATH: not an absolute path");
    }
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !exts.contains(&ext.as_str()) {
        anyhow::bail!("INVALID_PATH: unsupported file type .{ext}");
    }
    let mut forbidden = vec![state.paths().root, crate::paths::DataPaths::default_root()];
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)) {
        forbidden.push(dir);
    }
    if let Ok(win) = std::env::var("SystemRoot") {
        forbidden.push(win.into());
    }
    if forbidden.iter().any(|d| crate::paths::is_inside(p, d)) {
        anyhow::bail!("INVALID_PATH: the program and data folders cannot be written from here");
    }
    Ok(())
}

/// Write a text file the user picked in the save dialog (route export: GPX / KML, favorites JSON,
/// route text). The path comes from the webview: only these kinds, never into the program or
/// data folders.
#[tauri::command]
pub async fn save_text_file(state: State<'_, Arc<AppState>>, path: String, contents: String) -> R<()> {
    check_user_path(&state, &path, &["gpx", "kml", "txt", "json", "geojson", "csv"]).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&path, contents).map_err(|e| AppError::from(anyhow::Error::new(e).context("SAVE_FAILED")))).await.map_err(err)?
}

/// Read a track / drawing file the user dropped on the window or picked (GPX, KML, GeoJSON).
/// Only those extensions, absolute paths, at most 50 MB.
#[tauri::command]
pub async fn read_text_file(path: String) -> R<String> {
    let p = std::path::PathBuf::from(&path);
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !p.is_absolute() || !matches!(ext.as_deref(), Some("gpx" | "kml" | "geojson" | "json")) {
        return Err(AppError::from("INVALID_PATH: only absolute .gpx / .kml / .geojson / .json files"));
    }
    tauri::async_runtime::spawn_blocking(move || -> R<String> {
        let len = std::fs::metadata(&p).map_err(err)?.len();
        if len > 50 * 1024 * 1024 {
            return Err(AppError::from("INVALID_PATH: file larger than 50 MB"));
        }
        std::fs::read_to_string(&p).map_err(err)
    })
    .await
    .map_err(err)?
}

/// Find a city or country by name and the downloadable regions that contain it.
#[tauri::command]
pub async fn regions_locate(state: State<'_, Arc<AppState>>, query: String) -> R<Vec<crate::types::RegionLocate>> {
    let paths = state.paths();
    let lang = state.settings.read().language.clone();
    tauri::async_runtime::spawn_blocking(move || crate::data::regions::locate(&paths, &query, &lang).map_err(err)).await.map_err(err)?
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_paths_are_scoped() {
        let st = crate::state::AppState::new();
        let root = st.paths().root;
        let out = std::env::temp_dir().join("mo-export-test.gpx");
        assert!(super::check_user_path(&st, &out.display().to_string(), &["gpx"]).is_ok());
        assert!(super::check_user_path(&st, &root.join("settings.json").display().to_string(), &["json"]).is_err());
        assert!(super::check_user_path(&st, &out.with_extension("exe").display().to_string(), &["gpx"]).is_err());
        assert!(super::check_user_path(&st, "relative.gpx", &["gpx"]).is_err());
    }
}
