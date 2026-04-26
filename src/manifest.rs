use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths::manifest_path;
use crate::schema::StoreScope;

/// Serialize StoreScope as a lowercase string for the manifest YAML.
fn serialize_scope<S: serde::Serializer>(scope: &StoreScope, s: S) -> Result<S::Ok, S::Error> {
    let str_val = match scope {
        StoreScope::Worktree => "worktree",
        StoreScope::Repo => "repo",
        StoreScope::User => "user",
    };
    s.serialize_str(str_val)
}

fn deserialize_scope<'de, D: serde::Deserializer<'de>>(d: D) -> Result<StoreScope, D::Error> {
    let s = String::deserialize(d)?;
    match s.as_str() {
        "worktree" | "" => Ok(StoreScope::Worktree),
        "repo" => Ok(StoreScope::Repo),
        "user" => Ok(StoreScope::User),
        other => Err(serde::de::Error::custom(format!(
            "unknown scope '{other}' in manifest"
        ))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledStore {
    pub name: String,
    pub schema_path: PathBuf,
    pub installed_at: String,
    pub table_name: String,
    /// Storage scope for this installed store.  Recorded at install time from
    /// the schema's `scope:` declaration so the runtime can resolve `.stores/`
    /// without re-reading the schema YAML.
    #[serde(
        serialize_with = "serialize_scope",
        deserialize_with = "deserialize_scope",
        default = "default_scope"
    )]
    pub scope: StoreScope,
}

fn default_scope() -> StoreScope {
    StoreScope::Worktree
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

    /// Load the manifest from an explicit root directory (instead of cwd).
    /// Useful in tests and callers that manage their own working root.
    pub fn load_from(root: &std::path::Path) -> Result<Self> {
        let path = root.join(".stores").join("manifest.yaml");
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
