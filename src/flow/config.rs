//! `.stores/config.yaml` reader.
//!
//! Only the keys this phase needs are parsed (`ntfy.url`). Unknown keys are
//! ignored so future fields can land here without breaking older builds.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct StoresConfig {
    #[serde(default)]
    pub ntfy: Option<NtfyCfg>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NtfyCfg {
    pub url: String,
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
    fn malformed_yaml_errors_with_context() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy: [not-a-map\n").unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("config.yaml parse error"), "got: {err}");
    }
}
