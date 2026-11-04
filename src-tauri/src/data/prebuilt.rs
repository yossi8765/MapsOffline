//! Ready-made data from the project's GitLab package registry.
//!
//! A scheduled GitLab pipeline (`.gitlab-ci.yml`, `tools/ci-data.sh`) builds every component
//! once a week and uploads one `.mapsdata` file per component plus the data directory's
//! `manifest.json` as generic package `mapsdata/<YYYY-MM-DD>`. The data update takes a component
//! from there when the package is recent, was built for this program's data formats, and holds a
//! different version than the installed one; everything else is still built locally as before.
//! Satellite imagery comes from the package only when the user chose Sentinel-2 at the lowest
//! detail, which is what the package carries (the govmap orthophoto may not be redistributed).
use std::time::Duration;

use anyhow::{anyhow, bail, Context};

use super::download;
use super::manifest::Manifest;
use super::{Cancel, Emit};
use crate::paths::DataPaths;
use crate::types::{DataComponent, DataProgress, ProgressPhase};

const API: &str = "https://gitlab.com/api/v4/projects/RecoveryAdvancedFree%2FMapsOffline/packages";
pub const PACKAGE: &str = "mapsdata";
/// Older packages are ignored (the weekly job missed runs): build locally instead.
const MAX_AGE_DAYS: i64 = 10;

/// Components the pipeline publishes.
pub const PUBLISHED: [DataComponent; 9] = [
    DataComponent::Tiles,
    DataComponent::Satellite,
    DataComponent::World,
    DataComponent::Terrain,
    DataComponent::Routing,
    DataComponent::Geocode,
    DataComponent::Transit,
    DataComponent::Trails,
    DataComponent::Minyanim,
];

pub fn file_name(c: DataComponent) -> String {
    format!("{}.mapsdata", super::components::spec(c).key)
}

// The format tail of a component's manifest version, as this program builds it: a package
// made by another program version may hold an incompatible graph / index.
use super::components::format_suffix;

pub struct Latest {
    pub version: String,
    pub age_days: i64,
    pub manifest: Manifest,
    /// SHA-256 of each file of the package as the registry recorded it at upload (file name →
    /// hex); a downloaded file must match before it is installed.
    pub sha256: std::collections::HashMap<String, String>,
}

fn file_url(version: &str, name: &str) -> String {
    format!("{API}/generic/{PACKAGE}/{version}/{name}")
}

/// Newest published package and its manifest.
pub fn latest(client: &reqwest::blocking::Client) -> anyhow::Result<Latest> {
    let url = format!("{API}?package_name={PACKAGE}&package_type=generic&order_by=created_at&sort=desc&per_page=5");
    let r = client.get(&url).timeout(Duration::from_secs(15)).send().context("GitLab unreachable")?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(&url));
    }
    let list: Vec<serde_json::Value> = r.error_for_status()?.json()?;
    let p = list
        .iter()
        .find(|p| p["name"].as_str() == Some(PACKAGE) && p["status"].as_str().is_none_or(|s| s == "default"))
        .ok_or_else(|| anyhow!("no published data package"))?;
    let version = p["version"].as_str().ok_or_else(|| anyhow!("package without version"))?.to_string();
    let sha256 = p["id"]
        .as_i64()
        .and_then(|id| {
            let url = format!("{API}/{id}/package_files?per_page=100");
            let list: Vec<serde_json::Value> = client.get(&url).timeout(Duration::from_secs(15)).send().ok()?.error_for_status().ok()?.json().ok()?;
            Some(
                list.iter()
                    .filter_map(|f| Some((f["file_name"].as_str()?.to_string(), f["file_sha256"].as_str()?.to_string())))
                    .collect::<std::collections::HashMap<_, _>>(),
            )
        })
        .unwrap_or_default();
    let created = p["created_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let age_days = created.map(|c| (chrono::Utc::now() - c.with_timezone(&chrono::Utc)).num_days()).unwrap_or(i64::MAX);
    let m = client.get(file_url(&version, "manifest.json")).timeout(Duration::from_secs(15)).send()?.error_for_status()?;
    let manifest: Manifest = serde_json::from_slice(&m.bytes()?).context("package manifest")?;
    Ok(Latest { version, age_days, manifest, sha256 })
}

/// Which of `comps` the package should provide: version different from the installed one,
/// compatible format, file present.
pub fn usable(paths: &DataPaths, l: &Latest, comps: &[DataComponent]) -> Vec<(DataComponent, String)> {
    if l.age_days > MAX_AGE_DAYS {
        return Vec::new();
    }
    let local = Manifest::load(paths);
    let settings = crate::state::load_settings(paths);
    // the package's imagery: Sentinel-2 at the "low" zoom; only for a user who wants exactly that
    let sat_ok = |v: &str| {
        settings.satellite_source == "sentinel"
            && v.starts_with("s2cloudless")
            && v.ends_with(&format!(" z{}", super::satellite::quality_zoom(&settings.satellite_quality)))
    };
    comps
        .iter()
        .filter(|c| PUBLISHED.contains(c))
        .filter_map(|&c| {
            let v = l.manifest.entry(c)?.version.clone()?;
            if format_suffix(c).is_some_and(|s| !v.ends_with(&s)) {
                return None;
            }
            // packages built by older versions carry the 2017 AWS terrarium DEM; this program
            // builds Mapterhorn (512 px WebP) and must not replace it with the old data
            if c == DataComponent::Terrain && !v.starts_with("mapterhorn ") {
                return None;
            }
            if c == DataComponent::Satellite && !sat_ok(&v) {
                return None;
            }
            let same = local.entry(c).and_then(|e| e.version.as_deref()) == Some(v.as_str()) && paths.component_file(c).exists();
            (!same).then_some((c, v))
        })
        .collect()
}

/// Install what the package can provide for `comps`; returns the installed components (the
/// caller builds the rest). Failures only mean "build it locally".
pub fn install(paths: &DataPaths, comps: &[DataComponent], emit: &Emit, cancel: &Cancel) -> Vec<DataComponent> {
    // components cancelled before the start are not asked for at all: the GitLab request below
    // can take up to 15 s and would delay the "cancelled" answer
    let wanted: Vec<DataComponent> = comps.iter().copied().filter(|&c| PUBLISHED.contains(&c) && !cancel.is_cancelled_for(c)).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let client = match download::http_client() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let l = match latest(&client) {
        Ok(l) => l,
        Err(e) => {
            log::info!("no ready-made data package: {e:#}");
            return Vec::new();
        }
    };
    // Downloads run three at a time (the registry serves each file at a fraction of the line);
    // imports go one after the other (each one replaces files and records its version).
    let todo = parking_lot::Mutex::new(usable(paths, &l, &wanted).into_iter());
    let done = parking_lot::Mutex::new(Vec::new());
    let import_lock = parking_lot::Mutex::new(());
    std::thread::scope(|s| {
        for _ in 0..3 {
            s.spawn(|| loop {
                let Some((c, version)) = todo.lock().next() else { break };
                if install_one(paths, &l, c, &version, emit, cancel, &import_lock) {
                    done.lock().push(c);
                }
            });
        }
    });
    done.into_inner()
}

/// Download, check and install one component of the package; false = build it locally.
fn install_one(paths: &DataPaths, l: &Latest, c: DataComponent, version: &str, emit: &Emit, cancel: &Cancel, import_lock: &parking_lot::Mutex<()>) -> bool {
    {
        // a handle that answers for this component only: cancelling it stops its download
        let cancel = &cancel.bound_to(&[c]);
        if cancel.is_cancelled() {
            return false;
        }
        let file = paths.downloads_dir().join(format!("prebuilt-{}", file_name(c)));
        let progress = |phase: ProgressPhase, d: u64, t: u64, msg: &str| -> bool {
            cancel.wait_if_paused();
            let message = if phase == ProgressPhase::Downloading { format!("נתונים מוכנים מ-GitLab ({}) – {msg}", l.version) } else { msg.to_string() };
            emit(DataProgress { component: c, phase, done: d, total: t, message });
            !cancel.is_cancelled_for(c)
        };
        let r = (|| -> anyhow::Result<()> {
            download::ensure_fresh(&file_url(&l.version, &file_name(c)), &file, &progress, cancel)?;
            if let Some(want) = l.sha256.get(&file_name(c)) {
                progress(ProgressPhase::Processing, 0, 1, "מאמת את הקובץ שהורד");
                download::check_digest::<sha2::Sha256>(&file, want)?;
            }
            let _one_import = import_lock.lock();
            let info = super::package::inspect(&file)?;
            if !info.components.contains(&c) {
                bail!("the package file does not hold {c:?}");
            }
            let report = |cc: DataComponent, phase: ProgressPhase, d: u64, t: u64, msg: &str| {
                emit(DataProgress { component: cc, phase, done: d, total: t, message: msg.to_string() });
            };
            super::package::import(paths, &file, &[c], &report, cancel)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(download::meta_path(&file));
        match r {
            Ok(()) => {
                log::info!("{c:?}: installed ready-made {version} from package {}", l.version);
                true
            }
            Err(e) => {
                log::warn!("{c:?}: ready-made package not used ({e:#}); building locally");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_compatible_newer_components() {
        let dir = std::env::temp_dir().join(format!("mo-prebuilt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = DataPaths::new(&dir);
        paths.ensure_dirs().unwrap();
        // installed: routing same as remote, transit older
        std::fs::write(paths.graph_file(), b"g").unwrap();
        std::fs::write(paths.transit_file(), b"t").unwrap();
        let g = format!("pbf-1{}", format_suffix(DataComponent::Routing).unwrap());
        let mut local = Manifest::default();
        local.set(DataComponent::Routing, Some(g.clone()));
        local.set(DataComponent::Transit, Some(format!("gtfs-1{}", format_suffix(DataComponent::Transit).unwrap())));
        local.save(&paths).unwrap();
        let mut remote = Manifest::default();
        remote.set(DataComponent::Routing, Some(g));
        remote.set(DataComponent::Transit, Some(format!("gtfs-2{}", format_suffix(DataComponent::Transit).unwrap())));
        remote.set(DataComponent::Geocode, Some("pbf-2 s0.0".into())); // other format: not usable
        remote.set(DataComponent::Tiles, Some("20261101.pmtiles".into()));
        remote.set(DataComponent::Satellite, Some("s2cloudless-2025 z12".into()));
        remote.set(DataComponent::Terrain, Some("terrarium Sat, 02 Dec 2017 02:55:39 GMT".into())); // old DEM: not usable
        let l = Latest { version: "2026-11-01".into(), age_days: 2, manifest: remote, sha256: Default::default() };
        let all = [DataComponent::Routing, DataComponent::Transit, DataComponent::Geocode, DataComponent::Tiles, DataComponent::Satellite, DataComponent::Terrain];
        let got: Vec<DataComponent> = usable(&paths, &l, &all).into_iter().map(|x| x.0).collect();
        // satellite: the default settings ask for govmap, so the package's Sentinel-2 is skipped
        assert_eq!(got, vec![DataComponent::Transit, DataComponent::Tiles]);
        let old = Latest { age_days: 30, ..l };
        assert!(usable(&paths, &old, &all).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test --lib prebuilt::live -- --ignored --nocapture`: the real package vs the real data dir.
    #[test]
    #[ignore]
    fn real_package() {
        let client = crate::data::download::http_client().unwrap();
        let l = super::latest(&client).unwrap();
        eprintln!("package {} ({} days old)", l.version, l.age_days);
        for (k, e) in &l.manifest.components {
            eprintln!("  {k}: {:?}", e.version);
        }
        let paths = crate::paths::DataPaths::new(crate::paths::DataPaths::default_root());
        let all = crate::types::DataComponent::ALL;
        for (c, v) in super::usable(&paths, &l, &all) {
            eprintln!("would take {c:?} = {v}");
        }
    }
}
