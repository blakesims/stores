use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::schema::StoreScope;

// ---------------------------------------------------------------------------
// Process-wide stores-dir override (T023 P1)
//
// When set, all path lookups (`stores_dir`, `db_path`, `manifest_path`) point
// at the override instead of `cwd/.stores`. Used by the `--meta` flag /
// `STORES_META_PATH` env var to route a single CLI invocation at a META
// substrate without touching the caller's CWD.
// ---------------------------------------------------------------------------

static STORES_DIR_OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn stores_dir_override_slot() -> &'static Mutex<Option<PathBuf>> {
    STORES_DIR_OVERRIDE.get_or_init(|| Mutex::new(None))
}

/// Set the process-wide stores-dir override. Idempotent: a second call is a
/// no-op (the first set wins). Returns Ok(()) regardless of whether the value
/// was actually installed; callers that need to know can compare via
/// `stores_dir()` afterwards.
pub fn set_stores_dir_override(p: PathBuf) -> Result<()> {
    let mut guard = stores_dir_override_slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("stores-dir override lock poisoned"))?;
    if guard.is_none() {
        *guard = Some(p);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn clear_stores_dir_override_for_tests() {
    if let Ok(mut guard) = stores_dir_override_slot().lock() {
        *guard = None;
    }
}

pub fn stores_dir() -> Result<PathBuf> {
    if let Some(p) = stores_dir_override_slot()
        .lock()
        .map_err(|_| anyhow::anyhow!("stores-dir override lock poisoned"))?
        .clone()
    {
        return Ok(p);
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".stores"))
}

/// Resolve the META target path from the parsed --meta flag value plus the
/// `STORES_META_PATH` env var. Returns the META root (the directory
/// containing `.stores/`, NOT `.stores/` itself).
///
/// Resolution order:
/// - `flag = Some(path)` → use `path` (explicit override).
/// - `flag = Some("")` → sentinel meaning "--meta given without value"; read
///   `STORES_META_PATH`.
/// - `flag = None` → no --meta on the command line. Read `STORES_META_PATH`
///   if set; if unset, error.
///
/// Errors clean if:
/// - both flag value and env var are absent/empty
/// - the resolved path does not exist
/// - the resolved path exists but `<path>/.stores/` is missing
pub fn resolve_meta_path(flag: Option<&str>) -> Result<PathBuf> {
    let raw: String = match flag {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => match std::env::var("STORES_META_PATH") {
            Ok(v) if !v.is_empty() => v,
            _ => bail!(
                "STORES_META_PATH is not set and no --meta <PATH> provided; \
                 set the env var or pass --meta <PATH> to route to a META substrate"
            ),
        },
    };

    let path = PathBuf::from(&raw);
    if !path.exists() {
        bail!(
            "STORES_META_PATH target does not exist: {} (resolved from '{}')",
            path.display(),
            raw
        );
    }
    let stores = path.join(".stores");
    if !stores.exists() {
        bail!(
            "META path '{}' is missing a .stores/ directory; run `stores init` there first",
            path.display()
        );
    }
    Ok(path)
}

/// Resolve the substratum store root from the parsed --stores-root flag value plus
/// the `STORES_ROOT` env var. Returns the root directory containing `.stores/`.
pub fn resolve_stores_root(flag: Option<&str>) -> Result<PathBuf> {
    let raw: String = match flag {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => match std::env::var("STORES_ROOT") {
            Ok(v) if !v.is_empty() => v,
            _ => bail!(
                "STORES_ROOT is not set and no --stores-root <PATH> provided; \
                 set the env var or pass --stores-root <PATH> to route to a substratum store"
            ),
        },
    };

    let path = PathBuf::from(&raw);
    if !path.exists() {
        bail!(
            "STORES_ROOT target does not exist: {} (resolved from '{}')",
            path.display(),
            raw
        );
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("canonicalizing --stores-root target '{}'", raw))?;
    if !path.is_dir() {
        bail!(
            "--stores-root target '{}' is not a directory; pass the directory containing .stores/",
            path.display()
        );
    }
    let stores = path.join(".stores");
    if !stores.is_dir() {
        bail!(
            "--stores-root target '{}' is missing a .stores/ directory; run `stores init` there first",
            path.display()
        );
    }
    Ok(path)
}

pub fn db_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("db.sqlite"))
}

pub fn manifest_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("manifest.yaml"))
}

/// Stores-private daemon binary path.
///
/// Default: `$HOME/.local/share/stores/bin/stores`.
/// Tests may set `STORES_DAEMON_BIN_PATH` to keep migration/promotion out of
/// the real home directory.
pub fn daemon_binary_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("STORES_DAEMON_BIN_PATH") {
        return Ok(PathBuf::from(p));
    }
    Ok(home_dir()?
        .join(".local")
        .join("share")
        .join("stores")
        .join("bin")
        .join("stores"))
}

pub fn ensure_daemon_binary_parent() -> Result<PathBuf> {
    let path = daemon_binary_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating daemon binary parent {}", parent.display()))?;
    }
    Ok(path)
}

pub fn agents_pid_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("agents.pid"))
}

/// Check that `.stores/` has been initialized (db + manifest both present).
/// Returns an error directing the user to run `stores init` if not.
pub fn ensure_initialized() -> Result<()> {
    let dir = stores_dir()?;
    if !dir.exists() || !db_path()?.exists() || !manifest_path()?.exists() {
        bail!(
            ".stores/ is not initialized in '{}'; run `stores init` first",
            std::env::current_dir()?.display()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scope-aware resolution  (Task 1.6)
// ---------------------------------------------------------------------------

/// Resolve the `.stores/` directory for the given scope.
///
/// - `Worktree` → `cwd/.stores` (same as `stores_dir()`; v0.1 default).
/// - `Repo` → parent of `git rev-parse --git-common-dir` + `.stores`.
///   Errors clearly if not inside a git repository.
/// - `User` → `$HOME/.stores`.
pub fn stores_dir_for(scope: StoreScope) -> Result<PathBuf> {
    match scope {
        StoreScope::Worktree => stores_dir(),
        StoreScope::Repo => {
            let common = git_common_dir()?;
            // git-common-dir is the `.git` directory itself (or the common `.git`
            // for worktrees).  We want the directory containing `.git`.
            let parent = common
                .parent()
                .ok_or_else(|| anyhow::anyhow!("git common dir has no parent"))?;
            Ok(parent.join(".stores"))
        }
        StoreScope::User => {
            let home = home_dir()?;
            Ok(home.join(".stores"))
        }
    }
}

/// Return the path reported by `git rev-parse --git-common-dir`.
///
/// Errors with a clear message if the current directory is not inside a git
/// repository.
pub fn git_common_dir() -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run `git rev-parse --git-common-dir`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`git rev-parse --git-common-dir` failed (not a git repository?): {}",
            stderr.trim()
        );
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("`git rev-parse --git-common-dir` returned empty output");
    }

    // The output may be a relative path; resolve it against cwd.
    let p = PathBuf::from(trimmed);
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(std::env::current_dir()?.join(p))
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| {
            // Fallback for platforms without HOME
            dirs_home()
        })
        .map_err(|_| anyhow::anyhow!("cannot determine home directory ($HOME not set)"))
}

/// Minimal home-dir fallback that does not pull in a crate dependency.
#[cfg(unix)]
fn dirs_home() -> Result<PathBuf> {
    bail!("$HOME is not set and no fallback available on this platform")
}

#[cfg(not(unix))]
fn dirs_home() -> Result<PathBuf> {
    bail!("$HOME is not set and no fallback available on this platform")
}

// ---------------------------------------------------------------------------
// Process-wide CWD lock for tests
//
// Any test module that calls `std::env::set_current_dir` must hold this lock
// for the duration of the CWD-sensitive code to prevent races with other
// parallel tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn test_cwd_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Process-wide notifier lock for tests
//
// The global ntfy backend (installed via `flow::install_notifier`) is shared
// across the whole process. Any test that installs a mock notifier and
// asserts on captured events must hold this lock for the duration of the
// install→exercise→assert sequence so concurrent tests don't clobber the
// global with a fresh mock between install and assertion.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn test_notifier_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Tests  (Task 1.6 — AC1.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use std::path::Path;

    /// Serialize tests that mutate `current_dir` so they don't race.
    fn cwd_lock() -> &'static std::sync::Mutex<()> {
        test_cwd_lock()
    }

    /// Create a temporary directory containing a bare git repo and test worktree.
    /// Returns (tmp_dir, worktree_path, expected_stores_dir).
    fn make_tmp_git_repo() -> (tempfile::TempDir, PathBuf) {
        // We use the system `git` binary; this test is skipped if git is unavailable.
        let tmp = tempfile::tempdir().expect("tempdir failed");
        let repo_path = tmp.path().to_path_buf();

        // git init
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo_path)
            .status()
            .expect("git init failed");
        assert!(status.success(), "git init must succeed");

        (tmp, repo_path)
    }

    #[test]
    fn stores_dir_for_worktree_equals_cwd_stores() {
        // Worktree scope should return cwd/.stores regardless of git status.
        let result = stores_dir_for(StoreScope::Worktree).unwrap();
        let expected = stores_dir().unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn stores_dir_for_user_uses_home() {
        let home = std::env::var("HOME").expect("HOME must be set for this test");
        let result = stores_dir_for(StoreScope::User).unwrap();
        let expected = PathBuf::from(home).join(".stores");
        assert_eq!(result, expected);
    }

    #[test]
    fn git_common_dir_errors_outside_git() {
        let _guard = cwd_lock().lock().unwrap();
        // Run in a temp dir that is not a git repo.
        let tmp = tempfile::tempdir().expect("tempdir failed");
        let old_cwd = std::env::current_dir().unwrap();

        // Change cwd to the non-git tmp dir.  We must restore even on panic.
        std::env::set_current_dir(&tmp).expect("set_current_dir failed");
        let result = git_common_dir();
        std::env::set_current_dir(&old_cwd).expect("restore cwd failed");
        drop(tmp);

        assert!(
            result.is_err(),
            "git_common_dir should error outside a git repo"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not a git repository") || msg.contains("failed"),
            "error should mention git failure: {msg}"
        );
    }

    #[test]
    fn stores_dir_for_repo_errors_outside_git() {
        let _guard = cwd_lock().lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir failed");
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&tmp).expect("set_current_dir failed");
        let result = stores_dir_for(StoreScope::Repo);
        std::env::set_current_dir(&old_cwd).expect("restore cwd failed");
        drop(tmp);

        assert!(
            result.is_err(),
            "stores_dir_for(Repo) should error outside a git repo"
        );
    }

    // -----------------------------------------------------------------
    // T023 P1 — override + resolve_meta_path
    //
    // Tests for the override slot CANNOT mutate the global OnceLock from
    // multiple #[test]s safely (OnceLock is set-once, process-wide). To
    // exercise both the "override unset" and "override set" code paths
    // without cross-test interference, we assert the two behaviours
    // sequentially in a single test, guarded by `cwd_lock` so we are the
    // sole CWD-toucher.
    // -----------------------------------------------------------------
    #[test]
    fn override_changes_stores_db_and_manifest_paths() {
        let _guard = cwd_lock().lock().unwrap();
        clear_stores_dir_override_for_tests();
        // Before override (first observation in the process): cwd/.stores
        let cwd = std::env::current_dir().unwrap();
        let before = stores_dir().unwrap();
        assert_eq!(before, cwd.join(".stores"));

        // Install override
        let tmp = tempfile::tempdir().expect("tempdir failed");
        let override_dir = tmp.path().join(".stores");
        let _ = set_stores_dir_override(override_dir.clone());

        // After override
        assert_eq!(stores_dir().unwrap(), override_dir);
        assert_eq!(db_path().unwrap(), override_dir.join("db.sqlite"));
        assert_eq!(manifest_path().unwrap(), override_dir.join("manifest.yaml"));
        clear_stores_dir_override_for_tests();
    }

    #[test]
    fn resolve_meta_path_errors_when_env_unset_and_flag_none() {
        // Save and clear env so we know it's unset.
        let _guard = cwd_lock().lock().unwrap();
        let saved = std::env::var("STORES_META_PATH").ok();
        std::env::remove_var("STORES_META_PATH");

        let result = resolve_meta_path(None);

        // Restore env before asserting so failure does not pollute neighbours.
        if let Some(v) = saved {
            std::env::set_var("STORES_META_PATH", v);
        }

        let err = result.expect_err("must error when env unset and flag None");
        let msg = err.to_string();
        assert!(
            msg.contains("STORES_META_PATH"),
            "error must mention env var name: {msg}"
        );
    }

    #[test]
    fn resolve_meta_path_errors_when_env_points_to_nonexistent_dir() {
        let _guard = cwd_lock().lock().unwrap();
        let saved = std::env::var("STORES_META_PATH").ok();
        let bogus = "/tmp/__stores_meta_does_not_exist_T023__";
        std::env::set_var("STORES_META_PATH", bogus);

        let result = resolve_meta_path(None);

        match saved {
            Some(v) => std::env::set_var("STORES_META_PATH", v),
            None => std::env::remove_var("STORES_META_PATH"),
        }

        let err = result.expect_err("must error on non-existent path");
        let msg = err.to_string();
        assert!(
            msg.contains("STORES_META_PATH") || msg.contains("does not exist"),
            "error must mention env var or non-existence: {msg}"
        );
    }

    #[test]
    fn resolve_meta_path_errors_when_target_missing_dot_stores() {
        let _guard = cwd_lock().lock().unwrap();
        let saved = std::env::var("STORES_META_PATH").ok();

        // tmp dir exists but has no `.stores/` child.
        let tmp = tempfile::tempdir().expect("tempdir failed");
        std::env::set_var("STORES_META_PATH", tmp.path());

        let result = resolve_meta_path(None);

        match saved {
            Some(v) => std::env::set_var("STORES_META_PATH", v),
            None => std::env::remove_var("STORES_META_PATH"),
        }

        let err = result.expect_err("must error when .stores/ missing");
        let msg = err.to_string();
        assert!(msg.contains(".stores"), "error must mention .stores: {msg}");
    }

    #[test]
    fn resolve_stores_root_errors_when_env_unset_and_flag_none() {
        let _guard = cwd_lock().lock().unwrap();
        let saved = std::env::var("STORES_ROOT").ok();
        std::env::remove_var("STORES_ROOT");

        let result = resolve_stores_root(None);

        if let Some(v) = saved {
            std::env::set_var("STORES_ROOT", v);
        }

        let err = result.expect_err("must error when env unset and flag None");
        let msg = err.to_string();
        assert!(
            msg.contains("STORES_ROOT"),
            "error must mention env var name: {msg}"
        );
        assert!(
            msg.contains("--stores-root"),
            "error must mention flag name: {msg}"
        );
    }

    #[test]
    fn resolve_stores_root_errors_when_target_missing_dot_stores() {
        let _guard = cwd_lock().lock().unwrap();
        let saved = std::env::var("STORES_ROOT").ok();
        let tmp = tempfile::tempdir().expect("tempdir failed");
        std::env::set_var("STORES_ROOT", tmp.path());

        let result = resolve_stores_root(None);

        match saved {
            Some(v) => std::env::set_var("STORES_ROOT", v),
            None => std::env::remove_var("STORES_ROOT"),
        }

        let err = result.expect_err("must error when .stores/ missing");
        let msg = err.to_string();
        assert!(msg.contains(".stores"), "error must mention .stores: {msg}");
        assert!(
            msg.contains("--stores-root"),
            "error must mention flag: {msg}"
        );
    }

    #[test]
    fn resolve_stores_root_errors_when_dot_stores_is_file() {
        let _guard = cwd_lock().lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir failed");
        std::fs::write(tmp.path().join(".stores"), "not a dir").unwrap();

        let err = resolve_stores_root(Some(tmp.path().to_str().unwrap()))
            .expect_err("must error when .stores is not a directory");
        let msg = err.to_string();
        assert!(msg.contains(".stores/"), "error must mention .stores/: {msg}");
        assert!(msg.contains("--stores-root"), "error must mention flag: {msg}");
    }

    #[test]
    fn resolve_stores_root_happy_path_via_flag() {
        let _guard = cwd_lock().lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir failed");
        std::fs::create_dir(tmp.path().join(".stores")).unwrap();

        let result = resolve_stores_root(Some(tmp.path().to_str().unwrap())).unwrap();

        assert_eq!(result, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn resolve_stores_root_returns_canonical_path_for_relative_flag() {
        let _guard = cwd_lock().lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir failed");
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join(".stores")).unwrap();
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(tmp.path()).expect("set_current_dir failed");
        let result = resolve_stores_root(Some("root"));
        std::env::set_current_dir(&old_cwd).expect("restore cwd failed");

        assert_eq!(result.unwrap(), root.canonicalize().unwrap());
    }

    #[test]
    fn stores_dir_for_repo_in_git_repo() {
        let _guard = cwd_lock().lock().unwrap();
        // Only run if git is available.
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let (tmp, repo_path) = make_tmp_git_repo();
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&repo_path).expect("set_current_dir failed");
        let result = stores_dir_for(StoreScope::Repo);
        std::env::set_current_dir(&old_cwd).expect("restore cwd failed");

        // tmp must stay alive until after cwd is restored
        let _ = &tmp;

        let dir = result.expect("stores_dir_for(Repo) should succeed inside a git repo");
        // The result should be <repo_root>/.stores
        assert!(
            dir.ends_with(".stores"),
            "result should end with .stores: {:?}",
            dir
        );
        // The parent of <repo>/.git is <repo>; our result should be sibling of .git
        let git_dir = repo_path.join(".git");
        let expected = git_dir.parent().unwrap().join(".stores");
        assert_eq!(
            dir, expected,
            "Repo scope should resolve to <git-root>/.stores"
        );
    }
}
