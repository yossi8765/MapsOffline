//! Global application state shared by all tauri commands.
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::data::Cancel;
use crate::geocode::{Geocoder, SharedCache};
use crate::history::History;
use crate::paths::DataPaths;
use crate::routing::Graph;
use crate::transit::Transit;
use crate::types::Settings;

pub struct AppState {
    pub paths: RwLock<DataPaths>,
    pub settings: RwLock<Settings>,
    pub graph: RwLock<Option<Arc<Graph>>>,
    pub transit: RwLock<Option<Arc<Transit>>>,
    /// Pool of idle geocoder connections (SQLite connections are not `Sync`; each concurrent
    /// search checks one out instead of serialising on a single connection).
    geocoder: Mutex<GeoPool>,
    pub history: Mutex<Option<History>>,
    pub updating: std::sync::atomic::AtomicBool,
    pub cancel: Cancel,
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
}

const MAX_IDLE_GEOCODERS: usize = 4;

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
            history: Mutex::new(None),
            updating: std::sync::atomic::AtomicBool::new(false),
            cancel: Cancel::default(),
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
        *self.graph.write() = None;
        *self.transit.write() = None;
        let mut pool = self.geocoder.lock();
        pool.gen += 1;
        pool.idle.clear();
        pool.cache = None;
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
        std::thread::Builder::new()
            .name("transit-load".into())
            .spawn(move || {
                if let Err(e) = st.transit() {
                    log::debug!("background transit load: {e:#}");
                }
                st.transit_loading.store(false, Ordering::Release);
            })
            .ok();
    }

    /// Run `f` with a geocoder connection (opened lazily). No lock is held while `f` runs.
    pub fn with_geocoder<R>(&self, f: impl FnOnce(&Geocoder) -> anyhow::Result<R>) -> anyhow::Result<R> {
        let (gen, geo) = {
            let mut pool = self.geocoder.lock();
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
        if pool.gen == gen && pool.idle.len() < MAX_IDLE_GEOCODERS {
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

    pub fn save_settings(&self) -> anyhow::Result<()> {
        let paths = self.paths();
        let s = self.settings.read().clone();
        std::fs::create_dir_all(&paths.root)?;
        std::fs::write(paths.settings_file(), serde_json::to_vec_pretty(&s)?)?;
        Ok(())
    }
}

pub fn load_settings(paths: &DataPaths) -> Settings {
    std::fs::read(paths.settings_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
