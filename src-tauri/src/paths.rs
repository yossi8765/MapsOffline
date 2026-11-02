//! Data directory layout.
//!
//! %APPDATA%\MapsOffline\data\
//!   tiles\israel.pmtiles
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
    pub fn default_root() -> PathBuf {
        if let Ok(p) = std::env::var("MAPSOFFLINE_DATA_DIR") {
            return PathBuf::from(p);
        }
        dirs::config_dir()
            .map(|d| d.join("MapsOffline").join("data"))
            .unwrap_or_else(|| PathBuf::from("data"))
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.root.clone(),
            self.tiles_dir(),
            self.satellite_dir(),
            self.terrain_dir(),
            self.routing_dir(),
            self.geocode_dir(),
            self.transit_dir(),
            self.downloads_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    pub fn tiles_dir(&self) -> PathBuf {
        self.root.join("tiles")
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
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    pub fn tiles_file(&self) -> PathBuf {
        self.tiles_dir().join("israel.pmtiles")
    }
    pub fn satellite_file(&self) -> PathBuf {
        self.satellite_dir().join("israel-s2.pmtiles")
    }
    pub fn terrain_file(&self) -> PathBuf {
        self.terrain_dir().join("israel-dem.pmtiles")
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
        match c {
            DataComponent::Tiles => self.tiles_file(),
            DataComponent::Satellite => self.satellite_file(),
            DataComponent::Terrain => self.terrain_file(),
            DataComponent::Routing => self.graph_file(),
            DataComponent::Geocode => self.geocode_file(),
            DataComponent::Transit => self.transit_file(),
        }
    }
}

/// Write `bytes` to `tmp` next to `dest` then atomically rename over `dest`.
pub fn atomic_replace(dest: &Path, tmp: &Path) -> std::io::Result<()> {
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    std::fs::rename(tmp, dest)
}
