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
        std::fs::read(paths.manifest_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
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
    pub fn set(&mut self, c: DataComponent, version: Option<String>) {
        self.components.insert(
            key(c),
            Entry { updated_iso: Some(chrono::Local::now().to_rfc3339()), version },
        );
    }
}
