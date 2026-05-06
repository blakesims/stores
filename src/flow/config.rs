//! `.stores/config.yaml` reader.
//!
//! Only the keys this phase needs are parsed (`ntfy.url`). Unknown keys are
//! ignored so future fields can land here without breaking older builds.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoresConfig {
    #[serde(default)]
    pub ntfy: Option<NtfyCfg>,
    #[serde(default)]
    pub scaffold: Option<ScaffoldCfg>,
    #[serde(default)]
    pub drive: Option<DriveCfg>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NtfyCfg {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScaffoldCfg {
    pub command: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DriveCfg {
    #[serde(default = "default_drive_max_parallel")]
    pub max_parallel: u32,
    #[serde(default)]
    pub default_runner: Option<String>,
    #[serde(default)]
    pub roles: BTreeMap<String, DriveRoleCfg>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DriveRoleCfg {
    pub runner: String,
    #[serde(default)]
    pub model: Option<String>,
}

fn default_drive_max_parallel() -> u32 {
    1
}

/// Default location: `<.stores>/config.yaml` resolved against the worktree's
/// `.stores/` directory.
pub fn default_config_path() -> Result<PathBuf> {
    Ok(crate::paths::stores_dir()?.join("config.yaml"))
}

/// Load `config.yaml` at `path` if it exists. Missing file → `Ok(None)` (not
/// an error). Malformed YAML → `Err`.
pub fn load(path: &Path) -> Result<Option<StoresConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: StoresConfig =
        serde_yaml::from_str(&bytes).map_err(|e| anyhow!("config.yaml parse error: {}", e))?;
    Ok(Some(cfg))
}

/// Resolution order for the ntfy URL:
///   1. `ntfy.url` from `config.yaml` at `config_path` (if present).
///   2. `STORES_NTFY_URL` env var (if non-empty).
///   3. `None` — caller falls back to stderr.
pub fn resolve_ntfy_url(config_path: &Path) -> Option<String> {
    if let Ok(Some(cfg)) = load(config_path) {
        if let Some(n) = cfg.ntfy {
            if !n.url.is_empty() {
                return Some(n.url);
            }
        }
    }
    std::env::var("STORES_NTFY_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Resolve the auto-drive `max_parallel` cap. Returns the value from
/// `drive.max_parallel` in `config.yaml` when present, otherwise 1.
pub fn resolve_drive_max_parallel(config_path: &Path) -> u32 {
    if let Ok(Some(cfg)) = load(config_path) {
        if let Some(d) = cfg.drive {
            return d.max_parallel;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that mutate process env (`STORES_NTFY_URL`).
    fn env_lock() -> &'static Mutex<()> {
        use std::sync::OnceLock;
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn missing_file_is_ok_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.yaml");
        assert!(load(&path).unwrap().is_none());
    }

    #[test]
    fn parses_ntfy_url() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://ntfy.example/topic\n").unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(cfg.ntfy.unwrap().url, "https://ntfy.example/topic");
    }

    #[test]
    fn config_yaml_wins_over_env() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://from-config\n").unwrap();
        std::env::set_var("STORES_NTFY_URL", "https://from-env");
        let url = resolve_ntfy_url(&path);
        std::env::remove_var("STORES_NTFY_URL");
        assert_eq!(url.as_deref(), Some("https://from-config"));
    }

    #[test]
    fn env_fallback_when_no_config() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml"); // does not exist
        std::env::set_var("STORES_NTFY_URL", "https://from-env");
        let url = resolve_ntfy_url(&path);
        std::env::remove_var("STORES_NTFY_URL");
        assert_eq!(url.as_deref(), Some("https://from-env"));
    }

    #[test]
    fn neither_returns_none() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::env::remove_var("STORES_NTFY_URL");
        assert!(resolve_ntfy_url(&path).is_none());
    }

    #[test]
    fn parses_scaffold_command() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "scaffold:\n  command: \"./dev scaffold {display_id}\"\n").unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(
            cfg.scaffold.unwrap().command,
            "./dev scaffold {display_id}"
        );
    }

    #[test]
    fn parses_drive_max_parallel() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "drive:\n  max_parallel: 3\n").unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(
            cfg.drive,
            Some(DriveCfg {
                max_parallel: 3,
                default_runner: None,
                roles: BTreeMap::new(),
            })
        );
        assert_eq!(resolve_drive_max_parallel(&path), 3);
    }

    #[test]
    fn drive_max_parallel_defaults_to_one_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://x\n").unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert!(cfg.drive.is_none());
        assert_eq!(resolve_drive_max_parallel(&path), 1);
    }

    #[test]
    fn parses_drive_role_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "drive:\n  default_runner: pi\n  roles:\n    planner:\n      runner: claude-code\n      model: opus\n    code_reviewer:\n      runner: pi\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        let drive = cfg.drive.unwrap();
        assert_eq!(drive.default_runner.as_deref(), Some("pi"));
        assert_eq!(drive.roles["planner"].runner, "claude-code");
        assert_eq!(drive.roles["planner"].model.as_deref(), Some("opus"));
        assert_eq!(drive.roles["code_reviewer"].runner, "pi");
    }

    #[test]
    fn drive_max_parallel_defaults_to_one_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.yaml");
        assert_eq!(resolve_drive_max_parallel(&path), 1);
    }

    #[test]
    fn malformed_yaml_errors_with_context() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy: [not-a-map\n").unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("config.yaml parse error"), "got: {err}");
    }
}
