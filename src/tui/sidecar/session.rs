//! On-disk side-car session store.
//!
//! Layout: `.stores/runs/sidecar/{scope_key}-{session_id}.jsonl`. The TUI
//! looks for resumable sessions younger than 1h (mtime-based); fresh
//! sessions mint a new uuid v4.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{SessionId, SidecarScope};

const RESUME_TTL: Duration = Duration::from_secs(60 * 60);

/// Directory holding all side-car JSONL transcripts (production lookup).
pub fn sidecar_runs_dir() -> Result<PathBuf> {
    Ok(crate::paths::stores_dir()?.join("runs").join("sidecar"))
}

/// Build the JSONL path for a given (scope, session-id) pair under `runs_dir`.
pub fn session_path_in(runs_dir: &Path, scope: &SidecarScope, id: &SessionId) -> PathBuf {
    runs_dir.join(format!("{}-{}.jsonl", scope.scope_key(), id.as_str()))
}

/// Production helper — `session_path_in(sidecar_runs_dir()?, ...)`.
pub fn session_path(scope: &SidecarScope, id: &SessionId) -> Result<PathBuf> {
    Ok(session_path_in(&sidecar_runs_dir()?, scope, id))
}

/// Mint a fresh uuid v4 session id.
pub fn mint_session_id() -> SessionId {
    SessionId(uuid::Uuid::new_v4().to_string())
}

/// Find a resumable session for `scope` — the youngest matching JSONL
/// whose mtime is within `RESUME_TTL`. Returns `None` when the dir is
/// missing, no files match, or every match is stale.
///
/// `SidecarScope::PerRow { fresh: true, .. }` always returns `None`.
pub fn find_resumable(scope: &SidecarScope) -> Option<SessionId> {
    let dir = sidecar_runs_dir().ok()?;
    find_resumable_in(&dir, scope)
}

/// Test-friendly lookup that scans an explicit directory instead of
/// resolving via `paths::stores_dir()`.
pub fn find_resumable_in(dir: &Path, scope: &SidecarScope) -> Option<SessionId> {
    if let SidecarScope::PerRow { fresh: true, .. } = scope {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let prefix = format!("{}-", scope.scope_key());
    let now = SystemTime::now();

    let mut best: Option<(SystemTime, SessionId)> = None;
    for ent in entries.flatten() {
        let path = ent.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let stem = match name.strip_suffix(".jsonl") {
            Some(s) => s,
            None => continue,
        };
        let id_part = match stem.strip_prefix(&prefix) {
            Some(s) => s,
            None => continue,
        };
        let mtime = match ent.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Ok(age) = now.duration_since(mtime) {
            if age > RESUME_TTL {
                continue;
            }
        }
        let candidate = SessionId(id_part.to_string());
        match &best {
            Some((t, _)) if *t >= mtime => {}
            _ => best = Some((mtime, candidate)),
        }
    }
    best.map(|(_, id)| id)
}

/// Touch (create-or-update mtime) a session file. Used by tests and by
/// the spawn pipeline before invoking Claude Code.
#[allow(dead_code)]
pub fn touch(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to mkdir -p {}", parent.display())
        })?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to touch {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn resume_returns_some_for_recent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = SidecarScope::PerRow {
            display_id: "T999".to_string(),
            fresh: false,
        };
        let id = SessionId("abc-123".to_string());
        let p = session_path_in(tmp.path(), &scope, &id);
        touch(&p).unwrap();
        assert_eq!(find_resumable_in(tmp.path(), &scope), Some(id));
    }

    #[test]
    fn resume_returns_none_for_stale_file() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = SidecarScope::PerRow {
            display_id: "T999".to_string(),
            fresh: false,
        };
        let id = SessionId("old-456".to_string());
        let p = session_path_in(tmp.path(), &scope, &id);
        touch(&p).unwrap();
        let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
        let f = std::fs::File::open(&p).unwrap();
        f.set_modified(two_hours_ago).unwrap();
        assert_eq!(find_resumable_in(tmp.path(), &scope), None);
    }

    #[test]
    fn resume_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let scope = SidecarScope::PerRow {
            display_id: "T-missing".to_string(),
            fresh: false,
        };
        assert_eq!(find_resumable_in(tmp.path(), &scope), None);
    }

    #[test]
    fn fresh_flag_forces_none() {
        let tmp = tempfile::tempdir().unwrap();
        let display_id = "T999".to_string();
        let warm = SidecarScope::PerRow {
            display_id: display_id.clone(),
            fresh: false,
        };
        let id = SessionId("warm-id".to_string());
        touch(&session_path_in(tmp.path(), &warm, &id)).unwrap();
        let fresh = SidecarScope::PerRow {
            display_id,
            fresh: true,
        };
        assert_eq!(find_resumable_in(tmp.path(), &fresh), None);
    }
}
