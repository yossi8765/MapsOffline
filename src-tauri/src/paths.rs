//! Data directory layout.
//!
//! %APPDATA%\MapsOffline\data\
//!   tiles\israel.pmtiles
//!   world\world.pmtiles
//!   satellite\israel-s2.pmtiles
//!   terrain\israel-dem.pmtiles
//!   routing\graph.bin
//!   geocode\index.sqlite
//!   transit\transit.bin
//!   downloads\...            (raw PBF / GTFS zip / pmtiles cli)
//!   manifest.json
//!   history.sqlite
//!   settings.json

use std::path::{Path, PathBuf};

use crate::types::DataComponent;

pub const BBOX_ISRAEL: [f64; 4] = [34.2, 29.45, 35.95, 33.35]; // min_lon, min_lat, max_lon, max_lat

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default data dir: %APPDATA%\MapsOffline\data (falls back to ./data).
    ///
    /// Portable mode: when a `portable.txt` marker or a `data` folder sits next to the executable,
    /// everything (data, settings, history) lives in that `data` folder, so the program can run
    /// from a USB stick without installation.
    pub fn default_root() -> PathBuf {
        if let Ok(p) = std::env::var("MAPSOFFLINE_DATA_DIR") {
            return PathBuf::from(p);
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            if dir.join("portable.txt").exists() || dir.join("data").is_dir() {
                return dir.join("data");
            }
        }
        dirs::config_dir()
            .map(|d| d.join("MapsOffline").join("data"))
            .unwrap_or_else(|| PathBuf::from("data"))
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.root.clone(),
            self.tiles_dir(),
            self.world_dir(),
            self.satellite_dir(),
            self.terrain_dir(),
            self.routing_dir(),
            self.geocode_dir(),
            self.transit_dir(),
            self.trails_dir(),
            self.minyanim_dir(),
            self.downloads_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    pub fn tiles_dir(&self) -> PathBuf {
        self.root.join("tiles")
    }
    pub fn world_dir(&self) -> PathBuf {
        self.root.join("world")
    }
    pub fn satellite_dir(&self) -> PathBuf {
        self.root.join("satellite")
    }
    pub fn terrain_dir(&self) -> PathBuf {
        self.root.join("terrain")
    }
    pub fn routing_dir(&self) -> PathBuf {
        self.root.join("routing")
    }
    pub fn geocode_dir(&self) -> PathBuf {
        self.root.join("geocode")
    }
    pub fn transit_dir(&self) -> PathBuf {
        self.root.join("transit")
    }
    pub fn trails_dir(&self) -> PathBuf {
        self.root.join("trails")
    }
    pub fn minyanim_dir(&self) -> PathBuf {
        self.root.join("minyanim")
    }
    /// Other countries: one folder per Geofabrik region (`regions/<id>/map.pmtiles`, `geocode.sqlite`).
    pub fn regions_dir(&self) -> PathBuf {
        self.root.join("regions")
    }
    /// Folder of a country. Every character outside `[A-Za-z0-9_-]` becomes `_`, so no id (even
    /// an invalid one that slipped past `regions::valid_region_id`) can name `..` or a drive.
    pub fn region_dir(&self, id: &str) -> PathBuf {
        let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        self.regions_dir().join(if safe.is_empty() { "_".to_string() } else { safe })
    }
    /// Imported files, drawings and recorded tracks (`<id>.geojson` + `index.json`).
    pub fn userlayers_dir(&self) -> PathBuf {
        self.root.join("userlayers")
    }
    /// Disk cache of online custom tile layers (`usercache/<layer id>/z/x/y`).
    pub fn usercache_dir(&self) -> PathBuf {
        self.root.join("usercache")
    }
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn tiles_file(&self) -> PathBuf {
        self.tiles_dir().join("israel.pmtiles")
    }
    pub fn world_file(&self) -> PathBuf {
        self.world_dir().join("world.pmtiles")
    }
    pub fn satellite_file(&self) -> PathBuf {
        self.satellite_dir().join("israel-s2.pmtiles")
    }
    /// Mapterhorn DEM (512 px WebP). A new name on purpose: older program versions only know
    /// `israel-dem.pmtiles` (256 px PNG) and would mis-scale / fail to decode the new tiles if
    /// a ready-made package put them under the old name.
    pub fn terrain_file(&self) -> PathBuf {
        self.terrain_dir().join("israel-dem-mapterhorn.pmtiles")
    }
    /// The elevation archive to read: the Mapterhorn one, else the former one until the terrain
    /// update has run (so relief and elevation profiles keep working meanwhile).
    pub fn terrain_file_in_use(&self) -> PathBuf {
        let new = self.terrain_file();
        let old = self.legacy_terrain_file();
        if !new.exists() && old.exists() {
            old
        } else {
            new
        }
    }
    /// The former AWS terrain archive, deleted once the Mapterhorn one is installed.
    pub fn legacy_terrain_file(&self) -> PathBuf {
        self.terrain_dir().join("israel-dem.pmtiles")
    }
    pub fn trails_file(&self) -> PathBuf {
        self.trails_dir().join("trails.geojson")
    }
    pub fn minyanim_file(&self) -> PathBuf {
        self.minyanim_dir().join("minyanim.json")
    }
    pub fn graph_file(&self) -> PathBuf {
        self.routing_dir().join("graph.bin")
    }
    pub fn geocode_file(&self) -> PathBuf {
        self.geocode_dir().join("index.sqlite")
    }
    pub fn transit_file(&self) -> PathBuf {
        self.transit_dir().join("transit.bin")
    }
    pub fn manifest_file(&self) -> PathBuf {
        self.root.join("manifest.json")
    }
    pub fn history_db(&self) -> PathBuf {
        self.root.join("history.sqlite")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }
    pub fn pbf_file(&self) -> PathBuf {
        self.downloads_dir().join("israel-and-palestine-latest.osm.pbf")
    }
    pub fn gtfs_zip(&self) -> PathBuf {
        self.downloads_dir().join("israel-public-transportation.zip")
    }
    pub fn pmtiles_cli(&self) -> PathBuf {
        self.downloads_dir().join("pmtiles.exe")
    }

    pub fn component_file(&self, c: DataComponent) -> PathBuf {
        let s = crate::data::components::spec(c);
        self.root.join(s.dir).join(s.file)
    }
}

/// Move the finished `tmp` over `dest` in one step. `std::fs::rename` replaces an existing file
/// (MoveFileExW with MOVEFILE_REPLACE_EXISTING on Windows), so there is never a moment without
/// `dest`: deleting first and renaming after lost the old file when the rename then failed or
/// the process stopped in between.
/// On Windows a file that is open without delete sharing (a map tile being read, an index being
/// searched) cannot be replaced; such holds are short, so retry for a few seconds.
/// A folder chosen as the data folder: it must exist or be creatable, and be writable.
/// Errors carry the code `INVALID_DATA_DIR`.
pub fn check_data_dir(dir: &Path) -> anyhow::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_relative() {
        anyhow::bail!("INVALID_DATA_DIR: not an absolute folder: {}", dir.display());
    }
    if dir.exists() && !dir.is_dir() {
        anyhow::bail!("INVALID_DATA_DIR: {} is a file", dir.display());
    }
    std::fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("INVALID_DATA_DIR: cannot create {}: {e}", dir.display()))?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"ok").map_err(|e| anyhow::anyhow!("INVALID_DATA_DIR: cannot write in {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Is `p` inside `dir` (after resolving `..` and links where the paths exist)? Used to keep
/// webview-supplied save paths out of the data and program folders.
pub fn is_inside(p: &Path, dir: &Path) -> bool {
    fn norm(p: &Path) -> PathBuf {
        // canonicalize the longest existing ancestor, then append the rest
        let mut cur = p.to_path_buf();
        let mut tail = Vec::new();
        loop {
            if let Ok(c) = std::fs::canonicalize(&cur) {
                let mut out = c;
                for t in tail.iter().rev() {
                    out.push(t);
                }
                return out;
            }
            match (cur.file_name().map(|f| f.to_os_string()), cur.parent()) {
                (Some(f), Some(parent)) => {
                    tail.push(f);
                    cur = parent.to_path_buf();
                }
                _ => return p.to_path_buf(),
            }
        }
    }
    let lower = |p: PathBuf| PathBuf::from(p.to_string_lossy().to_lowercase());
    let (a, b) = (lower(norm(p)), lower(norm(dir)));
    a.starts_with(&b)
}

pub fn atomic_replace(dest: &Path, tmp: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        let r = std::fs::rename(tmp, dest);
        match r {
            Err(e) if attempt < 15 && e.kind() != std::io::ErrorKind::NotFound => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_dir_never_escapes() {
        let p = DataPaths::new("C:/data");
        for id in ["..", "../..", "a/../../b", "C:\\Windows", "europe/germany"] {
            let d = p.region_dir(id);
            assert_eq!(d.parent(), Some(p.regions_dir().as_path()), "{id}");
            assert!(!d.file_name().unwrap().to_string_lossy().contains(".."));
        }
        assert_eq!(p.region_dir("europe/germany"), p.regions_dir().join("europe_germany"));
    }

    #[test]
    fn inside_checks() {
        let dir = std::env::temp_dir().join(format!("mo-inside-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        assert!(is_inside(&dir.join("a").join("x.json"), &dir));
        assert!(is_inside(&dir.join("a").join("..").join("y.gpx"), &dir));
        assert!(!is_inside(&dir.join("..").join("z.gpx"), &dir));
        assert!(check_data_dir(&dir.join("new")).is_ok());
        assert!(check_data_dir(Path::new("relative/dir")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
