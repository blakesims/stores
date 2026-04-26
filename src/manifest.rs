use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths::manifest_path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledStore {
    pub name: String,
    pub schema_path: PathBuf,
    pub installed_at: String,
    pub table_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub stores: Vec<InstalledStore>,
}

impl Manifest {
    pub fn empty() -> Self {
        Manifest { stores: vec![] }
    }

    pub fn load() -> Result<Self> {
        let path = manifest_path()?;
        if !path.exists() {
            return Ok(Manifest::empty());
        }
        let content = std::fs::read_to_string(&path)?;
        let manifest: Manifest = serde_yaml::from_str(&content)?;
        Ok(manifest)
    }

    pub fn save(&self) -> Result<()> {
        let path = manifest_path()?;
        // Atomic write: write to tmp then rename
        let tmp = path.with_extension("yaml.tmp");
        let content = serde_yaml::to_string(self)?;
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
