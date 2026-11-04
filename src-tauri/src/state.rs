//! Global application state shared by all tauri commands.
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::data::Cancel;
use crate::geocode::{Geocoder, SharedCache};
use crate::history::History;
use crate::paths::DataPaths;
use crate::routing::Graph;
use crate::transit::Transit;
use crate::types::{DataComponent, Settings};

pub struct AppState {
    pub paths: RwLock<DataPaths>,
    pub settings: RwLock<Settings>,
    pub graph: RwLock<Option<Arc<Graph>>>,
    pub transit: RwLock<Option<Arc<Transit>>>,
    /// Pool of idle geocoder connections (SQLite connections are not `Sync`; each concurrent
    /// search checks one out instead of serialising on a single connection).
    geocoder: Mutex<GeoPool>,
    /// Geocoder pools of the downloaded countries, by index file.
    region_geo: Mutex<std::collections::HashMap<std::path::PathBuf, GeoPool>>,
    /// Running country installs (id → cancel handle); a busy country is hidden from the
    /// map and the search so its files can be replaced.
    pub region_jobs: Mutex<std::collections::HashMap<String, Cancel>>,
    /// The country graph used last (only one stays in memory: big countries are big).
    region_graph: Mutex<Option<(std::path::PathBuf, Arc<Graph>)>>,
    /// Serialises country graph loads (held only while reading a file, never while routing).
    region_graph_load: Mutex<()>,
    pub history: Mutex<Option<History>>,
    pub updating: std::sync::atomic::AtomicBool,
    pub cancel: Cancel,
    /// A program (installer) download is running – separate from data updates.
    pub app_updating: std::sync::atomic::AtomicBool,
    pub app_cancel: Cancel,
    /// Held while the graph / transit model is being loaded from disk (one lock per data set,
    /// so a slow transit load never delays the first route).
    graph_lock: Mutex<()>,
    transit_lock: Mutex<()>,
    transit_loading: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct GeoPool {
    /// bumped by `unload_all`; connections from an older generation are dropped on return
    gen: u64,
    idle: Vec<Geocoder>,
    cache: Option<SharedCache>,
    /// `geocode::EPOCH` the pooled connections belong to
    epoch: u64,
}

const MAX_IDLE_GEOCODERS: usize = 4;

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let root = DataPaths::default_root();
        let paths = DataPaths::new(root);
        let _ = paths.ensure_dirs();
        let settings = load_settings(&paths);
        let paths = match &settings.data_dir {
            Some(d) if !d.is_empty() => {
                let p = DataPaths::new(d);
                let _ = p.ensure_dirs();
                p
            }
            _ => paths,
        };
        Self {
            paths: RwLock::new(paths),
            settings: RwLock::new(settings),
            graph: RwLock::new(None),
            transit: RwLock::new(None),
            geocoder: Mutex::new(GeoPool::default()),
            region_geo: Mutex::new(Default::default()),
            region_jobs: Mutex::new(Default::default()),
            region_graph: Mutex::new(None),
            region_graph_load: Mutex::new(()),
            history: Mutex::new(None),
            updating: std::sync::atomic::AtomicBool::new(false),
            cancel: Cancel::default(),
            app_updating: std::sync::atomic::AtomicBool::new(false),
            app_cancel: Cancel::default(),
            graph_lock: Mutex::new(()),
            transit_lock: Mutex::new(()),
            transit_loading: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn paths(&self) -> DataPaths {
        self.paths.read().clone()
    }

    /// Drop all loaded data (after an update) so it is reloaded lazily.
    pub fn unload_all(&self) {
        self.unload(&DataComponent::ALL);
        self.unload_regions();
    }

    /// Drop only what `changed` components replaced: an update of the prayer times no longer
    /// throws away the routing graph and the transit model (which take seconds to load again).
    pub fn unload(&self, changed: &[DataComponent]) {
        if changed.contains(&DataComponent::Routing) {
            *self.graph.write() = None;
        }
        if changed.contains(&DataComponent::Transit) {
            *self.transit.write() = None;
        }
        if changed.contains(&DataComponent::Geocode) {
            self.release_geocoder();
        }
        if changed.iter().any(|c| matches!(c, DataComponent::Tiles | DataComponent::World)) {
            self.refresh_map_files();
        }
    }

    /// Flush what must not be lost when the program exits (history WAL).
    pub fn on_exit(&self) {
        if let Some(h) = self.history.lock().as_ref() {
            h.checkpoint();
        }
    }

    /// Close every pooled connection to the Israel index now (before its file is replaced).
    pub fn release_geocoder(&self) {
        let mut pool = self.geocoder.lock();
        pool.gen += 1;
        pool.idle.clear();
        pool.cache = None;
    }

    /// Drop the pooled connections of every country index (before files are replaced / removed).
    pub fn unload_regions(&self) {
        self.region_geo.lock().clear();
        *self.region_graph.lock() = None;
    }

    /// The routing graph that covers every point: Israel's when they are all inside it, else the
    /// smallest downloaded country whose bbox holds them all (a sub-region before its country).
    pub fn graph_for(&self, pts: &[[f64; 2]]) -> anyhow::Result<Arc<Graph>> {
        let inside = |b: &[f64; 4]| pts.iter().all(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]);
        if inside(&crate::paths::BBOX_ISRAEL) {
            return self.graph();
        }
        let mut cands: Vec<_> = crate::data::regions::ready_graphs(&self.paths())
            .into_iter()
            .filter(|(id, _, b)| !self.region_busy(id) && inside(b))
            .collect();
        cands.sort_by(|a, b| {
            let area = |b: &[f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
            area(&a.2).total_cmp(&area(&b.2))
        });
        let Some((_, path, _)) = cands.into_iter().next() else { anyhow::bail!("NO_ROUTING_DATA_REGION") };
        let cached = |g: &Option<(std::path::PathBuf, Arc<Graph>)>| g.as_ref().filter(|(p, _)| *p == path).map(|(_, g)| g.clone());
        if let Some(g) = cached(&self.region_graph.lock()) {
            return Ok(g);
        }
        // one load at a time; a second request for the same country waits for it, while routes
        // in the already loaded country (or Israel) never wait behind a load
        let _load = self.region_graph_load.lock();
        if let Some(g) = cached(&self.region_graph.lock()) {
            return Ok(g);
        }
        *self.region_graph.lock() = None; // free the previous country first
        let graph = Arc::new(crate::routing::load_graph(&path).map_err(|e| anyhow::anyhow!("NO_ROUTING_DATA_REGION: {e:#}"))?);
        *self.region_graph.lock() = Some((path, graph.clone()));
        Ok(graph)
    }

    pub fn region_busy(&self, id: &str) -> bool {
        self.region_jobs.lock().contains_key(id)
    }

    /// Tell the merged tile source which map archives exist: Israel first, then the countries
    /// that are not being installed right now.
    pub fn refresh_map_files(&self) {
        let p = self.paths();
        let mut v = Vec::new();
        if p.tiles_file().exists() {
            v.push(p.tiles_file());
        }
        for (id, f, _) in crate::data::regions::ready_maps(&p) {
            if !self.region_busy(&id) {
                v.push(f);
            }
        }
        crate::map_tiles::set_files(v);
    }

    /// Search indexes of the countries that are ready: (id, file, bbox).
    pub fn region_geocoders(&self) -> Vec<(String, std::path::PathBuf, [f64; 4])> {
        crate::data::regions::ready_geocoders(&self.paths()).into_iter().filter(|(id, _, _)| !self.region_busy(id)).collect()
    }

    /// Like [`with_geocoder`] for a country's index file.
    pub fn with_region_geocoder<R>(&self, path: &std::path::Path, f: impl FnOnce(&Geocoder) -> anyhow::Result<R>) -> anyhow::Result<R> {
        let (gen, geo) = {
            let mut pools = self.region_geo.lock();
            let pool = pools.entry(path.to_path_buf()).or_default();
            let gen = pool.gen;
            match pool.idle.pop() {
                Some(g) => (gen, g),
                None => {
                    let cache = pool.cache.clone().unwrap_or_default();
                    let g = crate::geocode::open_with_cache(path, cache)?;
                    pool.cache = Some(g.shared_cache());
                    (gen, g)
                }
            }
        };
        let r = f(&geo);
        let mut pools = self.region_geo.lock();
        if let Some(pool) = pools.get_mut(path) {
            if pool.gen == gen && pool.idle.len() < MAX_IDLE_GEOCODERS {
                pool.idle.push(geo);
            }
        }
        r
    }

    /// True once the geocoder index has been opened (never blocks on a running search).
    pub fn geocoder_loaded(&self) -> bool {
        self.geocoder.lock().cache.is_some()
    }

    pub fn graph(&self) -> anyhow::Result<Arc<Graph>> {
        if let Some(g) = self.graph.read().clone() {
            return Ok(g);
        }
        // Serialize concurrent loaders so the file is parsed only once.
        let _guard = self.graph_lock.lock();
        if let Some(g) = self.graph.read().clone() {
            return Ok(g);
        }
        let path = self.paths().graph_file();
        if !path.exists() {
            anyhow::bail!("NO_ROUTING_DATA");
        }
        let g = match crate::routing::load_graph(&path) {
            Ok(g) => Arc::new(g),
            // an outdated / damaged file behaves like missing data so the UI offers the download
            Err(e) => anyhow::bail!("NO_ROUTING_DATA: {e:#}"),
        };
        *self.graph.write() = Some(g.clone());
        Ok(g)
    }

    pub fn transit(&self) -> anyhow::Result<Arc<Transit>> {
        if let Some(t) = self.transit.read().clone() {
            return Ok(t);
        }
        let _guard = self.transit_lock.lock();
        if let Some(t) = self.transit.read().clone() {
            return Ok(t);
        }
        let path = self.paths().transit_file();
        if !path.exists() {
            anyhow::bail!("NO_TRANSIT_DATA");
        }
        let t = Arc::new(crate::transit::load(&path)?);
        *self.transit.write() = Some(t.clone());
        Ok(t)
    }

    /// Start loading the transit model on a background thread if it is not loaded yet and no
    /// load is in flight (search results include stops once it is there).
    pub fn transit_load_background(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        if self.transit.read().is_some() || self.transit_lock.is_locked() {
            return;
        }
        if self.transit_loading.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let st = self.clone();
        let spawned = std::thread::Builder::new().name("transit-load".into()).spawn(move || {
            if let Err(e) = st.transit() {
                log::debug!("background transit load: {e:#}");
            }
            st.transit_loading.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            // no thread, no load: let the next search try again
            self.transit_loading.store(false, Ordering::Release);
        }
    }

    /// Run `f` with a geocoder connection (opened lazily). No lock is held while `f` runs.
    pub fn with_geocoder<R>(&self, f: impl FnOnce(&Geocoder) -> anyhow::Result<R>) -> anyhow::Result<R> {
        use std::sync::atomic::Ordering as O;
        if crate::geocode::REPLACING.load(O::SeqCst) {
            anyhow::bail!("NO_GEOCODE_DATA");
        }
        let (gen, geo) = {
            let mut pool = self.geocoder.lock();
            let epoch = crate::geocode::EPOCH.load(O::SeqCst);
            if pool.epoch != epoch {
                // the index file was replaced: forget every connection to the old one
                pool.idle.clear();
                pool.cache = None;
                pool.gen += 1;
                pool.epoch = epoch;
            }
            let gen = pool.gen;
            match pool.idle.pop() {
                Some(g) => (gen, g),
                None => {
                    let path = self.paths().geocode_file();
                    if !path.exists() {
                        anyhow::bail!("NO_GEOCODE_DATA");
                    }
                    let cache = pool.cache.clone().unwrap_or_default();
                    let g = crate::geocode::open_with_cache(&path, cache)?;
                    pool.cache = Some(g.shared_cache());
                    (gen, g)
                }
            }
        };
        let r = f(&geo);
        let mut pool = self.geocoder.lock();
        let replacing = crate::geocode::REPLACING.load(O::SeqCst) || pool.epoch != crate::geocode::EPOCH.load(O::SeqCst);
        if !replacing && pool.gen == gen && pool.idle.len() < MAX_IDLE_GEOCODERS {
            pool.idle.push(geo);
        }
        r
    }

    pub fn with_history<R>(&self, f: impl FnOnce(&History) -> anyhow::Result<R>) -> anyhow::Result<R> {
        let mut guard = self.history.lock();
        if guard.is_none() {
            let paths = self.paths();
            let _ = paths.ensure_dirs();
            *guard = Some(crate::history::open(&paths.history_db())?);
        }
        f(guard.as_ref().unwrap())
    }

    /// settings.json always lives in the default root: that is where `new()` reads it, and only
    /// there does `data_dir` point onward to a custom folder. (Writing it into the custom folder
    /// lost every change made after switching folders.) Written to a temp file and renamed, so a
    /// crash mid-write cannot leave a truncated file.
    pub fn save_settings(&self) -> anyhow::Result<()> {
        let paths = DataPaths::new(DataPaths::default_root());
        let s = self.settings.read().clone();
        std::fs::create_dir_all(&paths.root)?;
        let file = paths.settings_file();
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&s)?)?;
        std::fs::rename(&tmp, &file)?;
        Ok(())
    }
}

/// settings.json, or the defaults. A file that cannot be read as settings is kept as
/// `settings.json.bad` (instead of being overwritten by the defaults at the next save) and logged.
pub fn load_settings(paths: &DataPaths) -> Settings {
    let file = paths.settings_file();
    let Ok(bytes) = std::fs::read(&file) else { return Settings::default() };
    match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("settings.json is damaged ({e}); kept as settings.json.bad, using defaults");
            let _ = std::fs::copy(&file, file.with_extension("json.bad"));
            Settings::default()
        }
    }
}
