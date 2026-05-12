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
    #[serde(default)]
    pub review: Option<ReviewCfg>,
    #[serde(default)]
    pub codex: Option<CodexCfg>,
    #[serde(default)]
    pub cleanup: Option<CleanupCfg>,
    #[serde(default)]
    pub fake_runner: Option<FakeRunnerCfg>,
    #[serde(default)]
    pub integration_steps: BTreeMap<String, IntegrationStepCfg>,
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
pub struct CleanupCfg {
    #[serde(default)]
    pub cargo_target_dir: Option<PathBuf>,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DriveRoleCfg {
    pub runner: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
}

impl<'de> Deserialize<'de> for DriveRoleCfg {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDriveRoleCfg {
            #[serde(default)]
            runner: Option<String>,
            #[serde(default)]
            harness: Option<String>,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            thinking: Option<String>,
        }

        let raw = RawDriveRoleCfg::deserialize(deserializer)?;
        let runner = match (raw.runner, raw.harness) {
            (Some(runner), Some(harness)) if runner != harness => {
                return Err(serde::de::Error::custom(format!(
                    "drive role config has conflicting runner ('{runner}') and harness ('{harness}') values"
                )));
            }
            (Some(runner), _) => runner,
            (_, Some(harness)) => harness,
            (None, None) => {
                return Err(serde::de::Error::missing_field("runner or harness"));
            }
        };

        Ok(Self {
            runner,
            model: raw.model,
            thinking: raw.thinking,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReviewCfg {
    #[serde(default = "default_review_runner")]
    pub runner: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_review_max_parallel")]
    pub max_parallel: u32,
    #[serde(default = "default_review_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for ReviewCfg {
    fn default() -> Self {
        Self {
            runner: default_review_runner(),
            model: None,
            max_parallel: default_review_max_parallel(),
            timeout_secs: default_review_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CodexCfg {
    #[serde(default = "default_codex_command")]
    pub command: String,
    #[serde(default = "default_codex_args")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct FakeRunnerCfg {
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default)]
    pub seed: Option<String>,
    #[serde(default)]
    pub scenario: Option<String>,
    #[serde(default)]
    pub fake_external_review: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IntegrationStepCfg {
    pub agent: String,
}

impl Default for CodexCfg {
    fn default() -> Self {
        Self {
            command: default_codex_command(),
            args: default_codex_args(),
        }
    }
}

fn default_drive_max_parallel() -> u32 {
    1
}

fn default_review_runner() -> String {
    "codex".to_string()
}

fn default_review_max_parallel() -> u32 {
    1
}

fn default_review_timeout_secs() -> u64 {
    1800
}

fn default_codex_command() -> String {
    "codex".to_string()
}

fn default_codex_args() -> Vec<String> {
    crate::runner::codex::default_codex_args()
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

pub fn resolve_review_config(config_path: &Path) -> ReviewCfg {
    if let Ok(Some(cfg)) = load(config_path) {
        return cfg.review.unwrap_or_default();
    }
    ReviewCfg::default()
}

pub fn resolve_codex_config(config_path: &Path) -> CodexCfg {
    if let Ok(Some(cfg)) = load(config_path) {
        return cfg.codex.unwrap_or_default();
    }
    CodexCfg::default()
}

pub fn resolve_fake_runner_config(config_path: &Path) -> FakeRunnerCfg {
    if let Ok(Some(cfg)) = load(config_path) {
        return cfg.fake_runner.unwrap_or_default();
    }
    FakeRunnerCfg::default()
}

pub fn llm_off_enabled() -> bool {
    match std::env::var("STORES_LLM_OFF") {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !normalized.is_empty() && !matches!(normalized.as_str(), "0" | "false" | "no" | "off")
        }
        Err(_) => false,
    }
}

pub fn fake_external_review_enabled(config_path: &Path) -> bool {
    if !llm_off_enabled() {
        return false;
    }
    resolve_fake_runner_config(config_path)
        .fake_external_review
        .unwrap_or(true)
}

pub fn fake_runner_env(config_path: &Path) -> Vec<(String, String)> {
    let cfg = resolve_fake_runner_config(config_path);
    let mut env = Vec::new();
    if let Some(delay_ms) = cfg.delay_ms {
        env.push(("STORES_FAKE_DELAY_MS".to_string(), delay_ms.to_string()));
    }
    if let Some(seed) = cfg.seed.filter(|s| !s.is_empty()) {
        env.push(("STORES_FAKE_SEED".to_string(), seed));
    }
    if let Some(scenario) = cfg.scenario.filter(|s| !s.is_empty()) {
        env.push(("STORES_FAKE_SCENARIO".to_string(), scenario));
    }
    env
}

/// Resolve the shared Cargo target directory for child commands.
///
/// Resolution order:
///   1. Existing `CARGO_TARGET_DIR` environment variable, so tests/operators can
///      force isolation without editing repo config.
///   2. `cleanup.cargo_target_dir` from `.stores/config.yaml`, resolved relative
///      to the real/canonical config file's parent (`.stores/`) when not absolute,
///      so symlinked worktree configs still point at one shared target dir.
///   3. `None` — callers leave Cargo's default per-workspace target behavior.
pub fn resolve_cargo_target_dir(config_path: &Path) -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("CARGO_TARGET_DIR") {
        if !env.is_empty() {
            return Some(PathBuf::from(env));
        }
    }

    let cfg = load(config_path).ok().flatten()?;
    let configured = cfg.cleanup?.cargo_target_dir?;
    if configured.as_os_str().is_empty() {
        return None;
    }
    if configured.is_absolute() {
        Some(configured)
    } else {
        let canonical_config_path = config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.to_path_buf());
        let base = canonical_config_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        Some(base.join(configured))
    }
}

pub fn cargo_target_env(config_path: &Path) -> Vec<(String, String)> {
    resolve_cargo_target_dir(config_path)
        .map(|p| {
            vec![(
                "CARGO_TARGET_DIR".to_string(),
                p.to_string_lossy().to_string(),
            )]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Serialize tests that mutate process env (`STORES_NTFY_URL`) against
    /// notifier tests that also rely on that process-global env var.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        &crate::runner::test_support::ENV_LOCK
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
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
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
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml"); // does not exist
        std::env::set_var("STORES_NTFY_URL", "https://from-env");
        let url = resolve_ntfy_url(&path);
        std::env::remove_var("STORES_NTFY_URL");
        assert_eq!(url.as_deref(), Some("https://from-env"));
    }

    #[test]
    fn neither_returns_none() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::env::remove_var("STORES_NTFY_URL");
        assert!(resolve_ntfy_url(&path).is_none());
    }

    #[test]
    fn parses_scaffold_command() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "scaffold:\n  command: \"./dev scaffold {display_id}\"\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(cfg.scaffold.unwrap().command, "./dev scaffold {display_id}");
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
    fn parses_fake_runner_config_and_boolean_env() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "fake_runner:\n  delay_ms: 0\n  seed: fixed\n  scenario: all-pass\n  fake_external_review: false\n",
        )
        .unwrap();
        let cfg = resolve_fake_runner_config(&path);
        assert_eq!(cfg.delay_ms, Some(0));
        assert_eq!(cfg.seed.as_deref(), Some("fixed"));
        assert_eq!(cfg.scenario.as_deref(), Some("all-pass"));
        assert_eq!(cfg.fake_external_review, Some(false));
        let old = std::env::var_os("STORES_LLM_OFF");
        for enabled in ["1", "true", "yes", "anything"] {
            std::env::set_var("STORES_LLM_OFF", enabled);
            assert!(llm_off_enabled(), "{enabled} should enable LLM_OFF");
        }
        for disabled in ["", "0", "false", "no", "off"] {
            std::env::set_var("STORES_LLM_OFF", disabled);
            assert!(!llm_off_enabled(), "{disabled:?} should disable LLM_OFF");
        }
        match old {
            Some(v) => std::env::set_var("STORES_LLM_OFF", v),
            None => std::env::remove_var("STORES_LLM_OFF"),
        }
    }

    #[test]
    fn parses_drive_role_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "drive:\n  default_runner: pi\n  roles:\n    planner:\n      runner: claude-code\n      model: opus\n      thinking: high\n    code_reviewer:\n      runner: pi\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        let drive = cfg.drive.unwrap();
        assert_eq!(drive.default_runner.as_deref(), Some("pi"));
        assert_eq!(drive.roles["planner"].runner, "claude-code");
        assert_eq!(drive.roles["planner"].model.as_deref(), Some("opus"));
        assert_eq!(drive.roles["planner"].thinking.as_deref(), Some("high"));
        assert_eq!(drive.roles["code_reviewer"].runner, "pi");
    }

    #[test]
    fn parses_drive_role_harness_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "drive:\n  roles:\n    plan_reviewer:\n      harness: pi\n      model: gpt-5.5\n      thinking: medium\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        let role = &cfg.drive.unwrap().roles["plan_reviewer"];
        assert_eq!(role.runner, "pi");
        assert_eq!(role.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(role.thinking.as_deref(), Some("medium"));
    }

    #[test]
    fn drive_role_runner_and_harness_may_match() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "drive:\n  roles:\n    executor:\n      runner: pi\n      harness: pi\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(cfg.drive.unwrap().roles["executor"].runner, "pi");
    }

    #[test]
    fn drive_role_runner_harness_conflict_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "drive:\n  roles:\n    executor:\n      runner: pi\n      harness: claude-code\n",
        )
        .unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(
            err.contains("conflicting runner ('pi') and harness ('claude-code')"),
            "got: {err}"
        );
    }

    #[test]
    fn drive_max_parallel_defaults_to_one_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.yaml");
        assert_eq!(resolve_drive_max_parallel(&path), 1);
    }

    #[test]
    fn parses_cleanup_cargo_target_dir_and_resolves_relative_to_config_parent() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CARGO_TARGET_DIR");
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        std::fs::create_dir_all(&stores).unwrap();
        let path = stores.join("config.yaml");
        std::fs::write(
            &path,
            "cleanup:\n  cargo_target_dir: ../.cargo-target/stores\n",
        )
        .unwrap();

        let cfg = load(&path).unwrap().unwrap();
        assert_eq!(
            cfg.cleanup.unwrap().cargo_target_dir.unwrap(),
            PathBuf::from("../.cargo-target/stores")
        );
        let resolved = stores.join("../.cargo-target/stores");
        assert_eq!(resolve_cargo_target_dir(&path).unwrap(), resolved);
        assert_eq!(
            cargo_target_env(&path),
            vec![(
                "CARGO_TARGET_DIR".to_string(),
                stores
                    .join("../.cargo-target/stores")
                    .to_string_lossy()
                    .to_string()
            )]
        );
    }

    #[test]
    fn cargo_target_dir_env_overrides_config_for_test_isolation() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "cleanup:\n  cargo_target_dir: configured\n").unwrap();
        let override_dir = tmp.path().join("override-target");
        std::env::set_var("CARGO_TARGET_DIR", &override_dir);
        let resolved = resolve_cargo_target_dir(&path);
        std::env::remove_var("CARGO_TARGET_DIR");
        assert_eq!(resolved.as_deref(), Some(override_dir.as_path()));
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_config_resolves_cargo_target_relative_to_real_config_parent() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CARGO_TARGET_DIR");
        let tmp = tempfile::tempdir().unwrap();
        let main_stores = tmp.path().join("main/.stores");
        let worktree_stores = tmp.path().join("worktree/.stores");
        std::fs::create_dir_all(&main_stores).unwrap();
        std::fs::create_dir_all(&worktree_stores).unwrap();
        let real_config = main_stores.join("config.yaml");
        std::fs::write(
            &real_config,
            "cleanup:\n  cargo_target_dir: ../.cargo-target/stores\n",
        )
        .unwrap();
        let linked_config = worktree_stores.join("config.yaml");
        std::os::unix::fs::symlink(&real_config, &linked_config).unwrap();

        assert_eq!(
            resolve_cargo_target_dir(&linked_config).unwrap(),
            main_stores.join("../.cargo-target/stores"),
            "relative cleanup.cargo_target_dir must be anchored at the real shared config, not at each worktree's symlinked .stores/"
        );
    }

    #[test]
    fn review_config_parses_runners_and_defaults_to_codex() {
        for runner in ["codex", "pi", "claude-code"] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("config.yaml");
            std::fs::write(
                &path,
                format!("review:\n  runner: {runner}\n  model: test-model\n  max_parallel: 2\n  timeout_secs: 77\n"),
            )
            .unwrap();
            let cfg = load(&path).unwrap().unwrap();
            let review = cfg.review.unwrap();
            assert_eq!(review.runner, runner);
            assert_eq!(review.model.as_deref(), Some("test-model"));
            assert_eq!(review.max_parallel, 2);
            assert_eq!(review.timeout_secs, 77);
            assert_eq!(resolve_review_config(&path).runner, runner);
        }

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(&path, "ntfy:\n  url: https://x\n").unwrap();
        assert_eq!(resolve_review_config(&path).runner, "codex");
    }

    #[test]
    fn parses_codex_command_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        std::fs::write(
            &path,
            "codex:\n  command: /bin/codex-shim\n  args: [exec, --color, never]\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap().unwrap();
        let codex = cfg.codex.unwrap();
        assert_eq!(codex.command, "/bin/codex-shim");
        assert_eq!(codex.args, vec!["exec", "--color", "never"]);

        let missing = tmp.path().join("missing.yaml");
        let defaulted = resolve_codex_config(&missing);
        assert_eq!(defaulted.command, "codex");
        assert!(defaulted
            .args
            .contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
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
