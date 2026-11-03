//! Data update pipeline: download sources, build derived files, report progress.

pub mod download;
pub mod manifest;
pub mod orthophoto;
pub mod regions;
pub mod package;
pub mod satellite;
pub mod terrain;
pub mod tiles;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use crate::paths::DataPaths;
use crate::types::{ComponentStatus, DataCheck, DataComponent, DataProgress, DataStatus, ProgressFn, ProgressPhase, UpdateCheck};

pub const PBF_URL: &str = "https://download.geofabrik.de/asia/israel-and-palestine-latest.osm.pbf";
pub const GTFS_URL: &str = "https://gtfs.mot.gov.il/gtfsfiles/israel-public-transportation.zip";

/// Cooperative control of the running update: cancel everything, cancel one component (skipped
/// if still queued, aborted at its next checkpoint if running), pause / resume.
///
/// A handle can be *bound* to the component(s) it works for (`bound_to`): `is_cancelled()` is then
/// true when all of them were cancelled, so a download shared by two components (the OSM PBF
/// feeds routing and geocoding) keeps going while one of them is still wanted. Clones keep the
/// binding, so worker threads spawned by a job see the same answer.
#[derive(Clone, Default)]
pub struct Cancel {
    inner: Arc<CancelInner>,
    bound: u32,
}

#[derive(Default)]
pub struct CancelInner {
    all: AtomicBool,
    paused: AtomicBool,
    /// bit i set = DataComponent::ALL[i] is cancelled
    skip: AtomicU32,
}

fn comp_bit(c: DataComponent) -> u32 {
    1 << DataComponent::ALL.iter().position(|&x| x == c).unwrap_or(0)
}

impl Cancel {
    /// A handle that answers for `comps` (all of them must be cancelled to count as cancelled).
    pub fn bound_to(&self, comps: &[DataComponent]) -> Cancel {
        Cancel { inner: self.inner.clone(), bound: comps.iter().map(|&c| comp_bit(c)).fold(0, |a, b| a | b) }
    }
    /// True when everything was cancelled or every component this handle is bound to was.
    pub fn is_cancelled(&self) -> bool {
        if self.inner.all.load(Ordering::Relaxed) {
            return true;
        }
        self.bound != 0 && self.inner.skip.load(Ordering::Relaxed) & self.bound == self.bound
    }
    pub fn is_cancelled_for(&self, c: DataComponent) -> bool {
        self.inner.all.load(Ordering::Relaxed) || self.inner.skip.load(Ordering::Relaxed) & comp_bit(c) != 0
    }
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Relaxed)
    }
    pub fn cancel(&self) {
        self.inner.all.store(true, Ordering::Relaxed)
    }
    pub fn cancel_component(&self, c: DataComponent) {
        self.inner.skip.fetch_or(comp_bit(c), Ordering::Relaxed);
    }
    pub fn set_paused(&self, paused: bool) {
        self.inner.paused.store(paused, Ordering::Relaxed)
    }
    /// Block while paused (cancelling also releases the wait).
    pub fn wait_if_paused(&self) {
        while self.is_paused() && !self.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    /// Pause-aware cancellation check for loops: waits while paused, errors when cancelled.
    pub fn check(&self) -> anyhow::Result<()> {
        self.wait_if_paused();
        if self.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        Ok(())
    }
    pub fn reset(&self) {
        self.inner.all.store(false, Ordering::Relaxed);
        self.inner.paused.store(false, Ordering::Relaxed);
        self.inner.skip.store(0, Ordering::Relaxed);
    }
}

pub type Emit = Arc<dyn Fn(DataProgress) + Send + Sync>;

/// Manifest version of a built component: the source's version plus the builder revision, so a
/// changed build rule (e.g. pedestrian access) shows up as an available update.
pub fn build_tag(c: DataComponent, source_version: &str) -> String {
    match c {
        DataComponent::Routing => format!("{source_version} g{}.{}", crate::routing::graph::VERSION, crate::routing::graph::BUILD_REV),
        DataComponent::Geocode => format!("{source_version} s{}.{}", crate::geocode::SCHEMA_VERSION, crate::geocode::BUILD_REV),
        DataComponent::Transit => format!("{source_version} t{}", crate::transit::model::VERSION),
        _ => source_version.to_string(),
    }
}

/// Status of files on disk (does not know about in-memory state).
pub fn status(paths: &DataPaths) -> DataStatus {
    let m = manifest::Manifest::load(paths);
    let comp = |c: DataComponent| -> ComponentStatus {
        let f = paths.component_file(c);
        match std::fs::metadata(&f) {
            Ok(md) => ComponentStatus {
                present: true,
                size_bytes: md.len(),
                updated_iso: m.entry(c).and_then(|e| e.updated_iso.clone()).or_else(|| {
                    md.modified().ok().map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339())
                }),
                version: m.entry(c).and_then(|e| e.version.clone()),
                loaded: false,
            },
            Err(_) => ComponentStatus::default(),
        }
    };
    DataStatus {
        dir: paths.root.display().to_string(),
        tiles: comp(DataComponent::Tiles),
        world: comp(DataComponent::World),
        satellite: comp(DataComponent::Satellite),
        terrain: comp(DataComponent::Terrain),
        routing: comp(DataComponent::Routing),
        geocode: comp(DataComponent::Geocode),
        transit: comp(DataComponent::Transit),
        updating: false,
        paused: false,
    }
}

/// Ask the servers whether newer data exists (three HEAD-style probes, run in parallel).
/// Blocking; call from a blocking thread. Never fails as a whole: each component carries its own
/// error so an offline machine still gets a sensible answer.
pub fn check(paths: &DataPaths) -> DataCheck {
    let m = manifest::Manifest::load(paths);
    let local = |c: DataComponent| -> Option<String> {
        if !paths.component_file(c).exists() {
            return None;
        }
        m.entry(c).and_then(|e| e.version.clone())
    };
    // A source file (PBF / GTFS) feeds the derived component; compare the server's version with
    // what the derived component was built from.
    let source_check = |url: &str, comps: &[DataComponent]| -> Vec<UpdateCheck> {
        let remote = download::http_client().and_then(|c| download::remote_version(&c, url));
        comps
            .iter()
            .map(|&c| match &remote {
                Ok(rv) => {
                    let expected = rv.as_ref().map(|r| build_tag(c, r));
                    UpdateCheck { available: local(c).is_none() || expected.is_some() && expected != local(c), remote_version: expected, error: None }
                }
                Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
            })
            .collect()
    };
    let (tiles, world, sat, dem, osm, gtfs) = std::thread::scope(|s| {
        let t = s.spawn(|| match download::http_client().and_then(|c| tiles::latest_build_key(&c)) {
            Ok(key) => UpdateCheck { available: tiles::build_is_stale(tiles::installed_build(paths).as_deref(), &key), remote_version: Some(key), error: None },
            Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
        });
        let world = s.spawn(|| match download::http_client().and_then(|c| tiles::latest_build_key(&c)) {
            Ok(key) => UpdateCheck { available: tiles::build_is_stale(tiles::Extract::world(paths).installed_build().as_deref(), &key), remote_version: Some(key), error: None },
            Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
        });
        let sat = s.spawn(|| {
            let settings = crate::state::load_settings(paths);
            // an archive full of content-filter placeholders counts as not installed
            let damaged = || paths.satellite_file().exists() && satellite::verify(&paths.satellite_file(), true).is_err();
            if settings.satellite_source != "sentinel" {
                // the orthophoto cache is fixed; only reachability is worth a probe
                let expected = orthophoto::archive_version(&settings.satellite_quality);
                return match download::http_client().and_then(|c| orthophoto::Source::new(c).probe(&Cancel::default())) {
                    Ok(()) => UpdateCheck { available: damaged() || !satellite::settings_match(local(DataComponent::Satellite).as_deref(), &settings, None), remote_version: Some(expected), error: None },
                    Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
                };
            }
            match download::http_client().and_then(|c| satellite::latest_layer(&c)) {
                Ok(layer) => UpdateCheck {
                    available: damaged() || !satellite::settings_match(local(DataComponent::Satellite).as_deref(), &settings, Some(&layer)),
                    remote_version: Some(satellite::expected_version(&settings, &layer)),
                    error: None,
                },
                Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
            }
        });
        let dem = s.spawn(|| match download::http_client().and_then(|c| terrain::remote_version(&c)) {
            Ok(v) => {
                let damaged = paths.terrain_file().exists() && satellite::verify(&paths.terrain_file(), false).is_err();
                UpdateCheck { available: damaged || local(DataComponent::Terrain).as_deref() != Some(v.as_str()), remote_version: Some(v), error: None }
            }
            Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
        });
        let o = s.spawn(|| source_check(PBF_URL, &[DataComponent::Routing, DataComponent::Geocode]));
        let g = s.spawn(|| source_check(GTFS_URL, &[DataComponent::Transit]));
        (t.join().unwrap_or_default(), world.join().unwrap_or_default(), sat.join().unwrap_or_default(), dem.join().unwrap_or_default(), o.join().unwrap_or_default(), g.join().unwrap_or_default())
    });
    let mut osm = osm.into_iter();
    DataCheck {
        checked_iso: chrono::Local::now().to_rfc3339(),
        tiles,
        world,
        satellite: sat,
        terrain: dem,
        routing: osm.next().unwrap_or_default(),
        geocode: osm.next().unwrap_or_default(),
        transit: gtfs.into_iter().next().unwrap_or_default(),
    }
}

/// Progress reporter for one job: emits to every component the job serves, blocks while paused,
/// and tells the job to stop (returns `false`) once all of them were cancelled.
struct Reporter {
    emit: Emit,
    comps: Vec<DataComponent>,
    cancel: Cancel,
}

impl Reporter {
    fn new(emit: &Emit, comps: &[DataComponent], cancel: &Cancel) -> Self {
        Self { emit: emit.clone(), comps: comps.to_vec(), cancel: cancel.bound_to(comps) }
    }
    fn report(&self, phase: ProgressPhase, done: u64, total: u64, msg: &str) -> bool {
        self.cancel.wait_if_paused();
        for &c in &self.comps {
            // a component that was cancelled on its own no longer shows the shared progress
            if !self.cancel.is_cancelled_for(c) {
                (self.emit)(DataProgress { component: c, phase, done, total, message: msg.to_string() });
            }
        }
        !self.cancel.is_cancelled()
    }
}

/// Shared end-of-job bookkeeping: manifest entry + Done / Failed / Cancelled event. The manifest
/// is read-modify-written under a lock because jobs finish on different threads.
struct Finisher<'a> {
    paths: &'a DataPaths,
    emit: &'a Emit,
    cancel: &'a Cancel,
    manifest_lock: parking_lot::Mutex<()>,
    errors: parking_lot::Mutex<Vec<anyhow::Error>>,
}

impl Finisher<'_> {
    fn finish(&self, c: DataComponent, res: anyhow::Result<Option<String>>) {
        let ev = |phase: ProgressPhase, done: u64, total: u64, message: String| {
            (self.emit)(DataProgress { component: c, phase, done, total, message });
        };
        match res {
            Ok(version) => {
                let _g = self.manifest_lock.lock();
                let mut m = manifest::Manifest::load(self.paths);
                m.set(c, version);
                match m.save(self.paths) {
                    Ok(()) => ev(ProgressPhase::Done, 1, 1, "הושלם".into()),
                    Err(e) => {
                        ev(ProgressPhase::Failed, 0, 0, format!("{e:#}"));
                        self.errors.lock().push(e);
                    }
                }
            }
            Err(err) => {
                if self.cancel.is_cancelled_for(c) {
                    ev(ProgressPhase::Cancelled, 0, 0, String::new());
                } else {
                    log::warn!("update of {c:?} failed: {err:#}");
                    ev(ProgressPhase::Failed, 0, 0, format!("{err:#}"));
                    self.errors.lock().push(err);
                }
            }
        }
    }
    fn skipped(&self, c: DataComponent) {
        (self.emit)(DataProgress { component: c, phase: ProgressPhase::Cancelled, done: 0, total: 0, message: String::new() });
    }
}

/// Run the update for the given components. Blocking; call from a blocking thread.
///
/// Every download runs in parallel (satellite imagery is limited by its server, not by the
/// line, so it overlaps the others; the OSM PBF is fetched once for routing + geocoding), while
/// the CPU-heavy builds run one after another because each already uses every core.
pub fn update(
    components: &[DataComponent],
    paths: &DataPaths,
    emit: Emit,
    cancel: &Cancel,
) -> anyhow::Result<()> {
    paths.ensure_dirs()?;
    let wants = |c: DataComponent| components.contains(&c);
    for &c in components {
        emit(DataProgress { component: c, phase: ProgressPhase::Queued, done: 0, total: 0, message: String::new() });
    }
    let fin = Finisher { paths, emit: &emit, cancel, manifest_lock: parking_lot::Mutex::new(()), errors: parking_lot::Mutex::new(Vec::new()) };

    std::thread::scope(|s| {
        // --- self-contained jobs: download + finalize in one go
        type Job = fn(&DataPaths, ProgressFn, &Cancel) -> anyhow::Result<Option<String>>;
        for (c, job) in [(DataComponent::Tiles, tiles::update_tiles as Job), (DataComponent::World, tiles::update_world as Job), (DataComponent::Satellite, satellite::update_satellite as Job), (DataComponent::Terrain, terrain::update_terrain as Job)] {
            if !wants(c) {
                continue;
            }
            let (fin, emit) = (&fin, &emit);
            s.spawn(move || {
                let rep = Reporter::new(emit, &[c], cancel);
                if rep.cancel.is_cancelled() {
                    return fin.skipped(c);
                }
                let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                fin.finish(c, job(paths, &progress, &rep.cancel));
            });
        }

        // --- shared source downloads
        let osm_comps: Vec<DataComponent> = [DataComponent::Routing, DataComponent::Geocode].into_iter().filter(|&c| wants(c)).collect();
        let pbf = (!osm_comps.is_empty()).then(|| {
            let (comps, emit) = (osm_comps.clone(), &emit);
            s.spawn(move || {
                let rep = Reporter::new(emit, &comps, cancel);
                let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                download::ensure_fresh(PBF_URL, &paths.pbf_file(), &progress, &rep.cancel)
            })
        });
        let gtfs = wants(DataComponent::Transit).then(|| {
            let emit = &emit;
            s.spawn(move || {
                let rep = Reporter::new(emit, &[DataComponent::Transit], cancel);
                let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                download::ensure_fresh(GTFS_URL, &paths.gtfs_zip(), &progress, &rep.cancel)
            })
        });

        // --- CPU-bound builds, one at a time, each as soon as its source is in
        let build = |c: DataComponent, source: &Result<Option<String>, String>, f: &dyn Fn(ProgressFn) -> anyhow::Result<()>| {
            if cancel.is_cancelled_for(c) {
                return fin.skipped(c);
            }
            match source {
                Err(msg) => fin.finish(c, Err(anyhow::anyhow!("{msg}"))),
                Ok(ver) => {
                    let rep = Reporter::new(&emit, &[c], cancel);
                    let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                    fin.finish(c, f(&progress).map(|()| ver.as_deref().map(|v| build_tag(c, v))));
                }
            }
        };
        if let Some(h) = pbf {
            let src = h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("download thread panicked"))).map_err(|e| format!("{e:#}"));
            for &c in &osm_comps[1..] {
                if src.is_ok() && !cancel.is_cancelled_for(c) {
                    emit(DataProgress { component: c, phase: ProgressPhase::Queued, done: 0, total: 0, message: "ממתין לעיבוד".into() });
                }
            }
            if wants(DataComponent::Routing) {
                build(DataComponent::Routing, &src, &|progress| {
                    let tmp = paths.graph_file().with_extension("bin.tmp");
                    crate::routing::build_graph(&paths.pbf_file(), &tmp, progress)?;
                    crate::paths::atomic_replace(&paths.graph_file(), &tmp)?;
                    Ok(())
                });
            }
            if wants(DataComponent::Geocode) {
                build(DataComponent::Geocode, &src, &|progress| {
                    let tmp = paths.geocode_file().with_extension("sqlite.tmp");
                    let _ = std::fs::remove_file(&tmp);
                    crate::geocode::build(&paths.pbf_file(), &tmp, progress)?;
                    crate::geocode::replace_index(&paths.geocode_file(), &tmp)?;
                    Ok(())
                });
            }
        }
        if let Some(h) = gtfs {
            let src = h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("download thread panicked"))).map_err(|e| format!("{e:#}"));
            build(DataComponent::Transit, &src, &|progress| {
                let tmp = paths.transit_file().with_extension("bin.tmp");
                crate::transit::build(&paths.gtfs_zip(), &tmp, progress)?;
                crate::paths::atomic_replace(&paths.transit_file(), &tmp)?;
                Ok(())
            });
        }
    });

    let mut errors = fin.errors.into_inner();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_per_component() {
        let c = Cancel::default();
        c.cancel_component(DataComponent::Transit);
        assert!(!c.bound_to(&[DataComponent::Routing]).is_cancelled());
        assert!(c.bound_to(&[DataComponent::Transit]).is_cancelled());
        assert!(c.is_cancelled_for(DataComponent::Transit) && !c.is_cancelled_for(DataComponent::Routing));
        // a shared download stops only when every component it serves is cancelled
        let shared = c.bound_to(&[DataComponent::Routing, DataComponent::Geocode]);
        c.cancel_component(DataComponent::Routing);
        assert!(!shared.is_cancelled());
        c.cancel_component(DataComponent::Geocode);
        assert!(shared.is_cancelled());
        assert!(!c.is_cancelled(), "unbound handle only reacts to cancel-all");
        c.cancel();
        assert!(c.is_cancelled());
        c.reset();
        assert!(!c.is_cancelled() && !c.is_paused() && !shared.is_cancelled());
    }

    #[test]
    fn cancel_releases_pause() {
        let c = Cancel::default();
        c.set_paused(true);
        let c2 = c.clone();
        let t = std::thread::spawn(move || c2.check());
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!t.is_finished(), "check() must block while paused");
        c.cancel();
        assert!(t.join().unwrap().is_err());
    }

    #[test]
    fn precancelled_component_is_skipped_without_work() {
        let dir = std::env::temp_dir().join(format!("mo-cancel-{}", std::process::id()));
        let paths = DataPaths::new(&dir);
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let emit: Emit = Arc::new(move |p: DataProgress| s2.lock().push((p.component, p.phase)));
        let c = Cancel::default();
        c.cancel_component(DataComponent::Transit);
        let t0 = std::time::Instant::now();
        update(&[DataComponent::Transit], &paths, emit, &c).unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(seen.lock().as_slice(), &[(DataComponent::Transit, ProgressPhase::Queued), (DataComponent::Transit, ProgressPhase::Cancelled)]);
        assert!(!paths.transit_file().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
