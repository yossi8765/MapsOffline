//! Deleting installed data: chosen components, the other countries, the sharp-imagery areas,
//! downloads and caches, personal data – or all of it. A file that is still open (a map archive
//! the tile server has mapped, an index in use) cannot be removed on Windows: it is written to
//! `pending-delete.json` and removed at the next start, before anything opens it.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::Manifest;
use crate::paths::DataPaths;
use crate::types::DataComponent;

/// What can be deleted. The component keys ("tiles", "routing", …) name one component each.
pub const EXTRA_TARGETS: [&str; 5] = ["regions", "areas", "downloads", "cache", "personal"];

/// Bytes on disk per target (for the Data panel).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetUsage {
    pub target: String,
    pub bytes: u64,
    /// one country ("region:<id>"): its name and ISO code, for the label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    pub freed_bytes: u64,
    /// files still in use: removed at the next start
    pub pending: Vec<String>,
}

fn component_of(target: &str) -> Option<DataComponent> {
    serde_json::from_value(serde_json::Value::String(target.to_string())).ok()
}

/// Every regular file directly inside `dir` (not its sub-folders).
fn files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default()
}

/// Files and folders a target consists of.
pub fn target_paths(paths: &DataPaths, target: &str) -> Vec<PathBuf> {
    // one country: "region:<id>"
    if let Some(id) = target.strip_prefix("region:") {
        return if super::regions::valid_region_id(id) { vec![paths.region_dir(id)] } else { Vec::new() };
    }
    if let Some(c) = component_of(target) {
        // the component's folder holds its file, sidecars and leftovers of interrupted updates;
        // sub-folders (satellite/areas, satellite/live) belong to other targets
        let dir = paths.component_file(c).parent().map(Path::to_path_buf).unwrap_or_else(|| paths.root.clone());
        return files_in(&dir);
    }
    match target {
        // country folders (the two list caches stay: they only spare a download)
        "regions" => std::fs::read_dir(paths.regions_dir()).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default(),
        "areas" => vec![super::sat_areas::dir(paths)],
        "downloads" => vec![paths.downloads_dir()],
        "cache" => vec![paths.usercache_dir(), paths.satellite_dir().join("live")],
        "personal" => {
            let h = paths.history_db();
            let mut v = vec![paths.userlayers_dir()];
            for ext in ["sqlite", "sqlite-wal", "sqlite-shm"] {
                v.push(h.with_extension(ext));
            }
            v
        }
        _ => Vec::new(),
    }
}

fn size_of(p: &Path) -> u64 {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

/// Size of every target that has something on disk; the other countries one by one
/// ("region:<id>") instead of all together.
pub fn usage(paths: &DataPaths) -> Vec<TargetUsage> {
    let mut targets: Vec<String> = DataComponent::ALL.iter().map(|c| serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()).collect();
    targets.extend(EXTRA_TARGETS.iter().filter(|t| **t != "regions").map(|s| s.to_string()));
    let mut out: Vec<TargetUsage> = targets
        .into_iter()
        .map(|t| {
            let bytes = target_paths(paths, &t).iter().map(|p| size_of(p)).sum();
            TargetUsage { target: t, bytes, name: None, iso: None }
        })
        .filter(|u| u.bytes > 0)
        .collect();
    for r in super::regions::installed(paths, &|_| false) {
        let bytes = size_of(&paths.region_dir(&r.id));
        if bytes > 0 {
            out.push(TargetUsage { target: format!("region:{}", r.id), bytes, name: Some(r.name), iso: r.iso });
        }
    }
    out
}

fn pending_file(paths: &DataPaths) -> PathBuf {
    paths.root.join("pending-delete.json")
}

/// A path is deleted only when it lies inside the data folder (never the folder itself).
fn inside(paths: &DataPaths, p: &Path) -> bool {
    p != paths.root && p.starts_with(&paths.root) && !p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

fn remove(p: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Delete the targets. The caller unloads whatever has them open first; files that still cannot
/// be removed are left for the next start.
pub fn delete(paths: &DataPaths, targets: &[String]) -> DeleteReport {
    let mut report = DeleteReport::default();
    let mut pending: Vec<String> = Vec::new();
    for t in targets {
        for p in target_paths(paths, t) {
            if !inside(paths, &p) {
                continue;
            }
            let bytes = size_of(&p);
            match remove(&p) {
                Ok(()) => report.freed_bytes += bytes,
                Err(e) => {
                    log::info!("delete {}: {e}; again at the next start", p.display());
                    // a folder may have lost part of its files already
                    report.freed_bytes += bytes.saturating_sub(size_of(&p));
                    pending.push(p.display().to_string());
                }
            }
        }
        if let Some(c) = component_of(t) {
            let mut m = Manifest::load(paths);
            m.remove(c);
            if let Err(e) = m.save(paths) {
                log::warn!("manifest after delete: {e:#}");
            }
        }
    }
    if !pending.is_empty() {
        let mut all = read_pending(paths);
        all.extend(pending.iter().cloned());
        all.sort();
        all.dedup();
        let _ = std::fs::write(pending_file(paths), serde_json::to_vec_pretty(&all).unwrap_or_default());
    }
    report.pending = pending;
    let _ = paths.ensure_dirs();
    report
}

fn read_pending(paths: &DataPaths) -> Vec<String> {
    std::fs::read(pending_file(paths)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// At start, before anything opens a data file: remove what an earlier delete could not.
pub fn run_pending(paths: &DataPaths) {
    let list = read_pending(paths);
    if list.is_empty() {
        return;
    }
    let left: Vec<String> = list.into_iter().filter(|s| inside(paths, Path::new(s)) && remove(Path::new(s)).is_err()).collect();
    if left.is_empty() {
        let _ = std::fs::remove_file(pending_file(paths));
    } else {
        let _ = std::fs::write(pending_file(paths), serde_json::to_vec_pretty(&left).unwrap_or_default());
    }
    let _ = paths.ensure_dirs();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_targets_and_keeps_the_rest() {
        let root = std::env::temp_dir().join(format!("mo-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = DataPaths::new(root.clone());
        paths.ensure_dirs().unwrap();
        std::fs::write(paths.graph_file(), vec![1u8; 1000]).unwrap();
        std::fs::write(paths.graph_file().with_extension("bin.tmp"), b"x").unwrap();
        std::fs::write(paths.satellite_file(), b"sat").unwrap();
        std::fs::create_dir_all(super::super::sat_areas::dir(&paths)).unwrap();
        std::fs::write(super::super::sat_areas::file(&paths, "a"), b"area").unwrap();
        std::fs::write(paths.history_db(), b"favorites").unwrap();
        let mut m = Manifest::load(&paths);
        m.set(DataComponent::Routing, Some("v".into()));
        m.save(&paths).unwrap();
        let u = usage(&paths);
        assert!(u.iter().any(|t| t.target == "routing" && t.bytes == 1001));
        let r = delete(&paths, &["routing".into(), "satellite".into()]);
        assert_eq!(r.freed_bytes, 1004);
        assert!(r.pending.is_empty());
        assert!(!paths.graph_file().exists() && !paths.satellite_file().exists());
        // the satellite target is the country archive only: sharp areas and personal data stay
        assert!(super::super::sat_areas::file(&paths, "a").exists());
        assert!(paths.history_db().exists());
        assert!(Manifest::load(&paths).entry(DataComponent::Routing).is_none());
        // the folders are back for the next update
        assert!(paths.routing_dir().is_dir());
        // unknown targets and paths outside the data folder are ignored
        assert_eq!(delete(&paths, &["../../x".into(), "nothing".into(), "region:..".into(), "region:../x".into()]).freed_bytes, 0);
        // one country of several
        for id in ["europe/malta", "africa/burundi"] {
            let d = paths.region_dir(id);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("region.json"), format!(r#"{{"id":"{id}","name":"{id}","iso":null,"bbox":[0,0,1,1],"searchVersion":null,"updatedIso":null}}"#)).unwrap();
            std::fs::write(d.join("map.pmtiles"), b"0123456789").unwrap();
        }
        let u = usage(&paths);
        assert!(u.iter().any(|t| t.target == "region:europe/malta" && t.name.as_deref() == Some("europe/malta")), "{u:?}");
        delete(&paths, &["region:europe/malta".into()]);
        assert!(!paths.region_dir("europe/malta").exists());
        assert!(paths.region_dir("africa/burundi").join("map.pmtiles").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
