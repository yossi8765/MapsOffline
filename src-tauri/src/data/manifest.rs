//! manifest.json – versions & timestamps of built components.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::paths::DataPaths;
use crate::types::DataComponent;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    pub updated_iso: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub components: BTreeMap<String, Entry>,
}

fn key(c: DataComponent) -> String {
    serde_json::to_value(c).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

impl Manifest {
    pub fn load(paths: &DataPaths) -> Self {
        let file = paths.manifest_file();
        let Ok(bytes) = std::fs::read(&file) else { return Self::default() };
        match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(e) => {
                // keep the damaged file for diagnosis instead of overwriting it silently
                log::warn!("manifest.json is damaged ({e}); kept as manifest.json.bad");
                let _ = std::fs::copy(&file, file.with_extension("json.bad"));
                Self::default()
            }
        }
    }

    /// Read-modify-write one entry under a process-wide lock (jobs finish on different threads:
    /// package import, prebuilt downloads and the update pipeline all record components).
    pub fn record(paths: &DataPaths, c: DataComponent, version: Option<String>) -> anyhow::Result<()> {
        static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let _g = LOCK.lock();
        let mut m = Self::load(paths);
        m.set(c, version);
        m.save(paths)
    }
    pub fn save(&self, paths: &DataPaths) -> anyhow::Result<()> {
        std::fs::create_dir_all(&paths.root)?;
        // tmp + rename: a crash while writing must not leave a truncated manifest behind
        let dest = paths.manifest_file();
        let tmp = dest.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        crate::paths::atomic_replace(&dest, &tmp)?;
        Ok(())
    }
    pub fn entry(&self, c: DataComponent) -> Option<&Entry> {
        self.components.get(&key(c))
    }
    /// Forget a component (its files were deleted).
    pub fn remove(&mut self, c: DataComponent) {
        self.components.remove(&key(c));
    }
    pub fn set(&mut self, c: DataComponent, version: Option<String>) {
        self.components.insert(
            key(c),
            Entry { updated_iso: Some(chrono::Local::now().to_rfc3339()), version },
        );
    }
}
