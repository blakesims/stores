use std::path::PathBuf;
use anyhow::{bail, Result};

use crate::schema::StoreScope;

pub fn stores_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".stores"))
}

pub fn db_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("db.sqlite"))
}

pub fn manifest_path() -> Result<PathBuf> {
    Ok(stores_dir()?.join("manifest.yaml"))
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
// Tests  (Task 1.6 — AC1.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[allow(unused_imports)]
    use std::path::Path;

    /// Serialize tests that mutate `current_dir` so they don't race.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = CWD_LOCK.lock().unwrap();
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
        let _guard = CWD_LOCK.lock().unwrap();
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

    #[test]
    fn stores_dir_for_repo_in_git_repo() {
        let _guard = CWD_LOCK.lock().unwrap();
        // Only run if git is available.
        if std::process::Command::new("git").arg("--version").output().is_err() {
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
        let expected = git_dir
            .parent()
            .unwrap()
            .join(".stores");
        assert_eq!(dir, expected, "Repo scope should resolve to <git-root>/.stores");
    }
}
