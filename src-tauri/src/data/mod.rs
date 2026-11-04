//! Data update pipeline: download sources, build derived files, report progress.

pub mod components;
pub mod disk;
pub mod download;
pub mod manifest;
pub mod minyanim;
pub mod orthophoto;
pub mod osmdiff;
pub mod prebuilt;
pub mod regions;
pub mod package;
pub mod satellite;
pub mod terrain;
pub mod tiles;
pub mod trails;

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
    format!("{source_version}{}", components::format_suffix(c).unwrap_or_default())
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
        trails: comp(DataComponent::Trails),
        minyanim: comp(DataComponent::Minyanim),
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
        // one request for the newest Protomaps build serves both the map and the world basemap
        let t = s.spawn(|| {
            let key = download::http_client().and_then(|c| tiles::latest_build_key(&c)).map_err(|e| format!("{e:#}"));
            let check = |installed: Option<String>| match &key {
                Ok(k) => UpdateCheck { available: tiles::build_is_stale(installed.as_deref(), k), remote_version: Some(k.clone()), error: None },
                Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(e.clone()) },
            };
            (check(tiles::installed_build(paths)), check(tiles::Extract::world(paths).installed_build()))
        });
        let sat = s.spawn(|| {
            let settings = crate::state::load_settings(paths);
            // an archive full of content-filter placeholders counts as not installed
            let damaged = || paths.satellite_file().exists() && !satellite::verify_cached(&paths.satellite_file(), true);
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
                let damaged = paths.terrain_file().exists() && !satellite::verify_cached(&paths.terrain_file(), false);
                UpdateCheck { available: damaged || local(DataComponent::Terrain).as_deref() != Some(v.as_str()), remote_version: Some(v), error: None }
            }
            Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
        });
        let o = s.spawn(|| {
            // The OSM extract is versioned by Geofabrik's replication sequence (see `osmdiff`), the
            // same string the full download and the incremental update tag the PBF with.
            let remote = download::http_client().and_then(|c| osmdiff::remote_state(&c));
            [DataComponent::Routing, DataComponent::Geocode, DataComponent::Trails]
                .into_iter()
                .map(|c| match &remote {
                    Ok(st) => {
                        let expected = Some(build_tag(c, &st.version()));
                        UpdateCheck { available: local(c).is_none() || expected != local(c), remote_version: expected, error: None }
                    }
                    Err(e) => UpdateCheck { available: false, remote_version: None, error: Some(format!("{e:#}")) },
                })
                .collect::<Vec<_>>()
        });
        let g = s.spawn(|| source_check(GTFS_URL, &[DataComponent::Transit]));
        let (tiles, world) = t.join().unwrap_or_default();
        (tiles, world, sat.join().unwrap_or_default(), dem.join().unwrap_or_default(), o.join().unwrap_or_default(), g.join().unwrap_or_default())
    });
    let mut osm = osm.into_iter();
    let mut out = DataCheck {
        checked_iso: chrono::Local::now().to_rfc3339(),
        tiles,
        world,
        satellite: sat,
        terrain: dem,
        routing: osm.next().unwrap_or_default(),
        geocode: osm.next().unwrap_or_default(),
        trails: osm.next().unwrap_or_default(),
        transit: gtfs.into_iter().next().unwrap_or_default(),
        // weekly snapshot: no probe of the charity's server, only the age of what is installed
        minyanim: UpdateCheck {
            available: !paths.minyanim_file().exists() || minyanim::is_stale(local(DataComponent::Minyanim).as_deref()),
            remote_version: None,
            error: None,
        },
    };
    // components the user does not want are never offered (Settings.wantedComponents)
    if let Some(wanted) = crate::state::load_settings(paths).wanted_components {
        for (c, u) in [
            (DataComponent::Tiles, &mut out.tiles),
            (DataComponent::World, &mut out.world),
            (DataComponent::Satellite, &mut out.satellite),
            (DataComponent::Terrain, &mut out.terrain),
            (DataComponent::Routing, &mut out.routing),
            (DataComponent::Geocode, &mut out.geocode),
            (DataComponent::Transit, &mut out.transit),
            (DataComponent::Trails, &mut out.trails),
            (DataComponent::Minyanim, &mut out.minyanim),
        ] {
            if !wanted.contains(&c) {
                u.available = false;
            }
        }
    }
    out
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
                match manifest::Manifest::record(self.paths, c, version) {
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

/// Where the transit build's source comes from.
enum GtfsSource {
    /// Downloaded (or already present) zip; its version.
    Fresh(Option<String>),
    /// The zip was deleted after the last build and the server still has that same timetable
    /// (or cannot be reached): transit.bin is current.
    Unchanged(String),
}

/// Version of the timetable transit.bin was built from, when that is still the latest one and
/// the zip is gone (it is deleted after a successful build). None = download and build.
fn gtfs_unchanged(paths: &DataPaths) -> Option<String> {
    let zip = paths.gtfs_zip();
    if zip.exists() || !paths.transit_file().exists() {
        return None;
    }
    let meta = download::load_meta(&zip).filter(|m| m.completed && m.url == GTFS_URL)?;
    let version = meta.version()?;
    let built = manifest::Manifest::load(paths).entry(DataComponent::Transit).and_then(|e| e.version.clone());
    // transit.bin must come from exactly this timetable and this program's format
    if built.as_deref() != Some(build_tag(DataComponent::Transit, &version).as_str()) {
        return None;
    }
    match download::unchanged_remote(GTFS_URL, &zip) {
        Ok(Some(v)) => Some(v),
        Ok(None) => None,
        // offline: keep what is installed, like the "server unavailable, using the existing file" path
        Err(e) => {
            log::info!("GTFS server not reachable ({e:#}); transit stays as installed");
            Some(version)
        }
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
    for &c in components {
        emit(DataProgress { component: c, phase: ProgressPhase::Queued, done: 0, total: 0, message: String::new() });
    }
    let settings = crate::state::load_settings(paths);
    let fin = Finisher { paths, emit: &emit, cancel, manifest_lock: parking_lot::Mutex::new(()), errors: parking_lot::Mutex::new(Vec::new()) };
    type Job = fn(&DataPaths, ProgressFn, &Cancel) -> anyhow::Result<Option<String>>;
    let run_job = |c: DataComponent, job: Job| {
        let rep = Reporter::new(&emit, &[c], cancel);
        if rep.cancel.is_cancelled() {
            return fin.skipped(c);
        }
        let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
        fin.finish(c, job(paths, &progress, &rep.cancel));
    };

    std::thread::scope(|outer| {
        // The govmap orthophoto never comes from the package (it may not be redistributed) and its
        // server is the slowest part of an update: start it now, next to the package downloads.
        let sat_now = components.contains(&DataComponent::Satellite) && (settings.satellite_source != "sentinel" || !settings.use_prebuilt_data);
        if sat_now {
            let run_job = &run_job;
            outer.spawn(move || run_job(DataComponent::Satellite, satellite::update_satellite as Job));
        }
        // ready-made components from the weekly GitLab build first; the rest is built here
        let ready = if settings.use_prebuilt_data { prebuilt::install(paths, components, &emit, cancel) } else { Vec::new() };
        let rest: Vec<DataComponent> = components.iter().copied().filter(|c| !ready.contains(c) && !(sat_now && *c == DataComponent::Satellite)).collect();
        if rest.is_empty() {
            return;
        }
        update_rest(&rest, paths, &emit, cancel, &fin, &run_job);
    });

    let mut errors = fin.errors.into_inner();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.remove(0))
    }
}

/// The part of [`update`] after the ready-made package: parallel downloads, sequential builds.
fn update_rest(
    components: &[DataComponent],
    paths: &DataPaths,
    emit: &Emit,
    cancel: &Cancel,
    fin: &Finisher<'_>,
    run_job: &(dyn Fn(DataComponent, fn(&DataPaths, ProgressFn, &Cancel) -> anyhow::Result<Option<String>>) + Sync),
) {
    let emit = emit.clone();
    let wants = |c: DataComponent| components.contains(&c);

    std::thread::scope(|s| {
        // --- self-contained jobs: download + finalize in one go
        type Job = fn(&DataPaths, ProgressFn, &Cancel) -> anyhow::Result<Option<String>>;
        for (c, job) in [(DataComponent::Tiles, tiles::update_tiles as Job), (DataComponent::World, tiles::update_world as Job), (DataComponent::Satellite, satellite::update_satellite as Job), (DataComponent::Terrain, terrain::update_terrain as Job), (DataComponent::Minyanim, minyanim::update_minyanim as Job)] {
            if !wants(c) {
                continue;
            }
            s.spawn(move || run_job(c, job));
        }

        // --- shared source downloads
        let osm_comps: Vec<DataComponent> = [DataComponent::Routing, DataComponent::Geocode, DataComponent::Trails].into_iter().filter(|&c| wants(c)).collect();
        let pbf = (!osm_comps.is_empty()).then(|| {
            let (comps, emit) = (osm_comps.clone(), &emit);
            s.spawn(move || {
                let rep = Reporter::new(emit, &comps, cancel);
                if rep.cancel.is_cancelled() {
                    anyhow::bail!("cancelled");
                }
                let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                osmdiff::update_pbf(&paths.pbf_file(), &progress, &rep.cancel)
            })
        });
        let gtfs = wants(DataComponent::Transit).then(|| {
            let emit = &emit;
            s.spawn(move || -> anyhow::Result<GtfsSource> {
                let rep = Reporter::new(emit, &[DataComponent::Transit], cancel);
                if rep.cancel.is_cancelled() {
                    anyhow::bail!("cancelled");
                }
                if let Some(v) = gtfs_unchanged(paths) {
                    return Ok(GtfsSource::Unchanged(v));
                }
                let progress = |p: ProgressPhase, d: u64, t: u64, m: &str| rep.report(p, d, t, m);
                download::ensure_fresh(GTFS_URL, &paths.gtfs_zip(), &progress, &rep.cancel).map(GtfsSource::Fresh)
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
            if wants(DataComponent::Trails) {
                build(DataComponent::Trails, &src, &|progress| {
                    trails::build(&paths.pbf_file(), &paths.trails_file(), progress)?;
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
            match src {
                // the timetable is the one transit.bin was built from: nothing to download or build
                Ok(GtfsSource::Unchanged(v)) if !cancel.is_cancelled_for(DataComponent::Transit) => {
                    emit(DataProgress { component: DataComponent::Transit, phase: ProgressPhase::Processing, done: 1, total: 1, message: "הקובץ כבר מעודכן".into() });
                    fin.finish(DataComponent::Transit, Ok(Some(build_tag(DataComponent::Transit, &v))));
                }
                src => {
                    let src = src.map(|g| match g {
                        GtfsSource::Fresh(v) => v,
                        GtfsSource::Unchanged(v) => Some(v),
                    });
                    build(DataComponent::Transit, &src, &|progress| {
                        let tmp = paths.transit_file().with_extension("bin.tmp");
                        // transfers are walked on the road network when the graph is installed
                        let g = crate::routing::load_graph(&paths.graph_file()).ok();
                        crate::transit::build_with_graph(&paths.gtfs_zip(), &tmp, progress, g.as_ref())?;
                        crate::paths::atomic_replace(&paths.transit_file(), &tmp)?;
                        // The 149 MB zip is not needed any more: transit.bin holds everything, and
                        // its meta (kept) is enough to tell next time whether the server has a newer
                        // timetable (`gtfs_unchanged`).
                        if let Err(e) = std::fs::remove_file(paths.gtfs_zip()) {
                            log::warn!("could not delete the used GTFS zip: {e}");
                        }
                        Ok(())
                    });
                }
            }
        }
    });
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
    fn gtfs_unchanged_needs_matching_build() {
        let dir = std::env::temp_dir().join(format!("mo-gtfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = DataPaths::new(&dir);
        paths.ensure_dirs().unwrap();
        // nothing installed: download
        assert_eq!(gtfs_unchanged(&paths), None);
        std::fs::write(paths.transit_file(), b"t").unwrap();
        let meta = download::DownloadMeta { url: GTFS_URL.into(), last_modified: Some("Mon, 03 Nov 2026 00:00:00 GMT".into()), content_length: Some(9), completed: true, ..Default::default() };
        download::save_meta(&paths.gtfs_zip(), &meta).unwrap();
        // transit.bin built from another timetable (or format): download and build again
        let mut m = manifest::Manifest::default();
        m.set(DataComponent::Transit, Some(build_tag(DataComponent::Transit, "Sun, 02 Nov 2026 00:00:00 GMT")));
        m.save(&paths).unwrap();
        assert_eq!(gtfs_unchanged(&paths), None);
        // zip still there: the normal download path decides
        m.set(DataComponent::Transit, Some(build_tag(DataComponent::Transit, "Mon, 03 Nov 2026 00:00:00 GMT")));
        m.save(&paths).unwrap();
        std::fs::write(paths.gtfs_zip(), b"z").unwrap();
        assert_eq!(gtfs_unchanged(&paths), None);
        let _ = std::fs::remove_dir_all(&dir);
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
