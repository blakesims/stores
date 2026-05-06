/// Render target path resolution + directory-move logic for the `render` verb.
///
/// Responsibilities:
///   - `status_to_dir`: map lifecycle status → on-disk subdirectory name
///   - `resolve_render_path`: evaluate the Handlebars render_target_path template
///     from the workflow and inject `status_dir` into the context
///   - `find_existing_task_dir`: glob `tasks/*/{{display_id}}-*` to locate the
///     current on-disk directory (if any)
///   - `maybe_move_dir`: move the directory when status_dir has changed
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::render::render_template;

// ---------------------------------------------------------------------------
// status_dir mapping
// ---------------------------------------------------------------------------

/// Map a lifecycle status to the on-disk subdirectory name under `tasks/`.
///
/// Mapping:
///   planning | plan_review                         → "planning"
///   ready | executing | code_review                → "active"
///   complete | in_review                           → "active"  (transient / awaiting human)
///   blocked | rejected                             → "paused"  (human action required)
///   accepted                                       → "completed" (terminal, human signed off)
///   anything else                                  → "active" (safe fallback)
///
/// Note: `complete` is transient — a row never rests there in normal flow. If observed,
/// map to `active/` (still mid-flow). `in_review` rows are awaiting a human decision
/// but the task is not done; `active/` keeps them visible alongside executing tasks.
/// `rejected` is similar to `blocked` — awaiting human-driven `amend`; maps to `paused/`.
pub fn status_to_dir(status: &str) -> &'static str {
    match status {
        "planning" | "plan_review" => "planning",
        "ready" | "executing" | "code_review" | "complete" | "in_review" => "active",
        "blocked" | "rejected" => "paused",
        "accepted" => "completed",
        _ => "active",
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolve the render target path from the workflow's `render_target_path` template.
///
/// Injects `status_dir` into `ctx` before rendering so templates can use
/// `{{status_dir}}` to place files in the correct subdirectory.
///
/// Returns an absolute path by joining with `repo_root` when the resolved path
/// is relative.
pub fn resolve_render_path(
    render_target_path: &str,
    ctx: &Value,
    repo_root: &Path,
) -> Result<PathBuf> {
    // Inject status_dir into a mutable context copy.
    let mut ctx_with_dir = ctx.clone();
    if let Some(obj) = ctx_with_dir.as_object_mut() {
        let status = obj
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("active");
        let dir = status_to_dir(status);
        obj.insert("status_dir".to_string(), Value::String(dir.to_string()));
    }

    let rendered = render_template(render_target_path, &ctx_with_dir)
        .map_err(|e| anyhow::anyhow!("render_target_path template error: {}", e))?;

    let p = PathBuf::from(rendered.trim());
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(repo_root.join(p))
    }
}

// ---------------------------------------------------------------------------
// Directory detection + move
// ---------------------------------------------------------------------------

/// Find all task directories on disk matching `<display_id>-*` (or exactly
/// `<display_id>`) under any state subdirectory of `tasks/`.
///
/// Used by render to detect stale shells across state dirs (planning/active/
/// paused/completed/etc.) so they can be canonicalized to the current state's
/// directory.
pub fn find_all_task_dirs(repo_root: &Path, display_id: &str) -> Vec<PathBuf> {
    let tasks_root = repo_root.join("tasks");
    if !tasks_root.exists() {
        return Vec::new();
    }

    let mut matches: Vec<PathBuf> = Vec::new();

    let entries = match std::fs::read_dir(&tasks_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for status_dir_entry in entries.flatten() {
        let status_dir_path = status_dir_entry.path();
        // Skip symlinked status dirs — following them could escape the repo.
        let st_meta = match std::fs::symlink_metadata(&status_dir_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if st_meta.file_type().is_symlink() || !st_meta.is_dir() {
            continue;
        }
        let inner_entries = match std::fs::read_dir(&status_dir_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for task_dir_entry in inner_entries.flatten() {
            let task_dir_path = task_dir_entry.path();
            // Skip symlinked task dirs — cleanup would otherwise migrate
            // files OUT of the symlink target (potentially outside repo_root).
            let td_meta = match std::fs::symlink_metadata(&task_dir_path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if td_meta.file_type().is_symlink() || !td_meta.is_dir() {
                continue;
            }
            if let Some(name) = task_dir_path.file_name().and_then(|n| n.to_str()) {
                let prefix = format!("{}-", display_id);
                if name.starts_with(&prefix) || name == display_id {
                    matches.push(task_dir_path);
                }
            }
        }
    }

    matches
}

/// Find an existing task directory by display_id.
///
/// Searches `tasks/*/{{display_id}}-*` relative to `repo_root`.
/// Returns `None` when no match exists; `Some(path)` for the canonical
/// existing dir when one or more matches exist. When multiple matches exist
/// (stale shells from prior states), the most-recently-modified one is
/// returned so the caller can move/canonicalize it; a follow-up cleanup pass
/// removes the other shells.
pub fn find_existing_task_dir(repo_root: &Path, display_id: &str) -> Option<PathBuf> {
    let mut matches = find_all_task_dirs(repo_root, display_id);
    match matches.len() {
        0 => None,
        1 => Some(matches.remove(0)),
        _ => {
            // Multiple stale shells exist — pick the most recently modified
            // so the caller can canonicalize it. The remaining shells are
            // cleaned up by `cleanup_stale_task_dirs` after the write.
            matches.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
            });
            matches.pop()
        }
    }
}

/// After a render write, remove any stale task directories for `display_id`
/// that are not the canonical `target_dir`.
///
/// A stale dir is removed when it is empty or contains only render artifacts
/// (`main.md`, `main.md.tmp`). When a stale dir contains other files (user
/// notes, etc.) those files are migrated into `target_dir` if no same-named
/// file already exists there; otherwise a warning is logged and the file is
/// left in place (preserving user data).
///
/// A genuine display_id collision — multiple non-target dirs that cannot be
/// safely consolidated — surfaces as a warning so the operator can resolve.
pub fn cleanup_stale_task_dirs(
    repo_root: &Path,
    display_id: &str,
    target_dir: &Path,
) -> Result<()> {
    let target_canon = target_dir.canonicalize().unwrap_or_else(|_| target_dir.to_path_buf());
    let stale_dirs: Vec<PathBuf> = find_all_task_dirs(repo_root, display_id)
        .into_iter()
        .filter(|p| {
            let pc = p.canonicalize().unwrap_or_else(|_| p.clone());
            pc != target_canon
        })
        .collect();

    for stale in stale_dirs {
        consolidate_stale_dir(&stale, target_dir)?;
    }
    Ok(())
}

/// Migrate non-render files from `stale` into `target_dir` and remove `stale`
/// if it becomes empty. Render artifacts (main.md, main.md.tmp) are dropped
/// since the canonical target_dir owns them.
fn consolidate_stale_dir(stale: &Path, target_dir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(stale) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "warning: cannot read stale task dir '{}': {}; leaving in place",
                stale.display(),
                e
            );
            return Ok(());
        }
    };

    let mut leftover = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                leftover = true;
                continue;
            }
        };

        if name == "main.md" || name == "main.md.tmp" {
            // Render artifact — the canonical target_dir owns the fresh copy.
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            continue;
        }

        // Preserve user data: migrate into target_dir if no name collision.
        //
        // NOTE: the substrate's wf_tasks.display_id UNIQUE constraint makes
        // true display_id directory collision (two distinct rows on the same
        // slug) structurally impossible. The collision shape that CAN fire
        // here is purely a per-file user-data conflict during stale-dir
        // consolidation (e.g., notes.md exists in both stale and target).
        // The warning text reflects the actual shape, not the contract's
        // "display_id collision" framing which is invariant-prevented.
        let dst = target_dir.join(&name);
        if dst.exists() {
            eprintln!(
                "warning: file-migration collision: '{}' exists in both '{}' and '{}'; \
                 leaving stale copy at '{}'",
                name,
                stale.display(),
                target_dir.display(),
                path.display()
            );
            leftover = true;
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Err(e) = std::fs::rename(&path, &dst) {
            eprintln!(
                "warning: cannot migrate '{}' → '{}': {}; leaving in place",
                path.display(),
                dst.display(),
                e
            );
            leftover = true;
        }
    }

    if !leftover {
        if let Err(e) = std::fs::remove_dir(stale) {
            eprintln!(
                "warning: cannot remove empty stale task dir '{}': {}",
                stale.display(),
                e
            );
        }
    }
    Ok(())
}

/// Move `src_dir` to `dst_dir` (parent of dst is created if needed).
///
/// This is a best-effort move: on cross-device renames (unlikely for a local
/// tasks/ tree but possible) we fall back to a copy+delete.  On any error,
/// we log a warning and return so render can still write to the canonical path.
pub fn maybe_move_dir(src_dir: &Path, dst_dir: &Path) -> Result<()> {
    if src_dir == dst_dir {
        return Ok(());
    }

    // Ensure the parent of dst_dir exists.
    if let Some(parent) = dst_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Attempt atomic rename first.
    if let Err(e) = std::fs::rename(src_dir, dst_dir) {
        // EXDEV: cross-device move — fall back to copy+delete.
        // For simplicity (task dirs are small), just propagate the error and
        // let the caller warn + fall back to canonical path.
        return Err(anyhow::anyhow!(
            "directory move failed: {} → {}: {}",
            src_dir.display(),
            dst_dir.display(),
            e
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    // status_to_dir mapping
    #[test]
    fn status_dir_planning_states() {
        assert_eq!(status_to_dir("planning"), "planning");
        assert_eq!(status_to_dir("plan_review"), "planning");
    }

    #[test]
    fn status_dir_active_states() {
        assert_eq!(status_to_dir("ready"), "active");
        assert_eq!(status_to_dir("executing"), "active");
        assert_eq!(status_to_dir("code_review"), "active");
    }

    #[test]
    fn status_dir_paused() {
        assert_eq!(status_to_dir("blocked"), "paused");
    }

    #[test]
    fn status_dir_complete() {
        // `complete` is transient (on_state follow-on fires immediately); maps to active/.
        assert_eq!(status_to_dir("complete"), "active");
    }

    #[test]
    fn status_dir_in_review() {
        // in_review: awaiting human GO/NO_GO; still in active/ (task not yet done).
        assert_eq!(status_to_dir("in_review"), "active");
    }

    #[test]
    fn status_dir_accepted() {
        // accepted: human signed off; task done → completed/.
        assert_eq!(status_to_dir("accepted"), "completed");
    }

    #[test]
    fn status_dir_rejected() {
        // rejected: human said no; awaiting amend → paused/.
        assert_eq!(status_to_dir("rejected"), "paused");
    }

    #[test]
    fn status_dir_unknown_falls_back_to_active() {
        assert_eq!(status_to_dir("something_else"), "active");
    }

    // resolve_render_path
    #[test]
    fn resolve_render_path_injects_status_dir() {
        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let ctx = json!({
            "display_id": "T003",
            "slug": "my-task",
            "status": "executing"
        });
        let root = Path::new("/repo");
        let p = resolve_render_path(tpl, &ctx, root).unwrap();
        assert_eq!(p, PathBuf::from("/repo/tasks/active/T003-my-task/main.md"));
    }

    #[test]
    fn resolve_render_path_complete_status() {
        // complete is now transient (maps to active/).
        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let ctx = json!({
            "display_id": "T003",
            "slug": "my-task",
            "status": "complete"
        });
        let root = Path::new("/repo");
        let p = resolve_render_path(tpl, &ctx, root).unwrap();
        assert_eq!(p, PathBuf::from("/repo/tasks/active/T003-my-task/main.md"));
    }

    #[test]
    fn resolve_render_path_accepted_status() {
        // accepted: terminal, human signed off → completed/.
        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let ctx = json!({
            "display_id": "T003",
            "slug": "my-task",
            "status": "accepted"
        });
        let root = Path::new("/repo");
        let p = resolve_render_path(tpl, &ctx, root).unwrap();
        assert_eq!(
            p,
            PathBuf::from("/repo/tasks/completed/T003-my-task/main.md")
        );
    }

    #[test]
    fn resolve_render_path_blocked_status() {
        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let ctx = json!({
            "display_id": "T003",
            "slug": "my-task",
            "status": "blocked"
        });
        let root = Path::new("/repo");
        let p = resolve_render_path(tpl, &ctx, root).unwrap();
        assert_eq!(p, PathBuf::from("/repo/tasks/paused/T003-my-task/main.md"));
    }

    // find_existing_task_dir
    #[test]
    fn find_existing_dir_returns_match() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("tasks/active/T003-my-task");
        std::fs::create_dir_all(&dir).unwrap();

        let found = find_existing_task_dir(tmp.path(), "T003");
        assert_eq!(found, Some(dir));
    }

    #[test]
    fn find_existing_dir_returns_none_when_absent() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/active")).unwrap();

        let found = find_existing_task_dir(tmp.path(), "T003");
        assert!(found.is_none());
    }

    #[test]
    fn find_existing_dir_returns_some_on_multiple_matches() {
        // T036: multi-match used to return None (causing accumulation).
        // It now returns Some(most_recent) so the caller can canonicalize.
        let tmp = tempdir().unwrap();
        let older = tmp.path().join("tasks/planning/T003-task-a");
        let newer = tmp.path().join("tasks/active/T003-task-a");
        std::fs::create_dir_all(&older).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(&newer).unwrap();

        let found = find_existing_task_dir(tmp.path(), "T003");
        assert!(found.is_some(), "should pick a canonical match, not None");
    }

    #[test]
    fn find_all_task_dirs_returns_every_match() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/active/T036-slug")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/planning/T036-slug")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/completed/T036-slug")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/active/T999-other")).unwrap();

        let all = find_all_task_dirs(tmp.path(), "T036");
        assert_eq!(all.len(), 3, "should find all 3 stale shells");
    }

    #[test]
    fn cleanup_stale_task_dirs_removes_empty_shells() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("tasks/completed/T036-slug");
        let stale_a = tmp.path().join("tasks/active/T036-slug");
        let stale_b = tmp.path().join("tasks/planning/T036-slug");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&stale_a).unwrap();
        std::fs::create_dir_all(&stale_b).unwrap();
        std::fs::write(target.join("main.md"), "current").unwrap();
        std::fs::write(stale_a.join("main.md"), "old").unwrap();

        cleanup_stale_task_dirs(tmp.path(), "T036", &target).unwrap();

        assert!(target.exists(), "target dir preserved");
        assert!(!stale_a.exists(), "stale active/ shell removed");
        assert!(!stale_b.exists(), "stale planning/ shell removed");
    }

    #[test]
    fn cleanup_stale_task_dirs_migrates_user_files() {
        // notes.md (non-render artifact) should migrate into target_dir.
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("tasks/completed/T036-slug");
        let stale = tmp.path().join("tasks/active/T036-slug");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(target.join("main.md"), "current").unwrap();
        std::fs::write(stale.join("main.md"), "old").unwrap();
        std::fs::write(stale.join("notes.md"), "user notes").unwrap();

        cleanup_stale_task_dirs(tmp.path(), "T036", &target).unwrap();

        assert!(!stale.exists(), "stale dir removed after migration");
        assert_eq!(
            std::fs::read_to_string(target.join("notes.md")).unwrap(),
            "user notes",
            "user notes migrated to target"
        );
    }

    #[test]
    fn cleanup_stale_task_dirs_warns_on_collision() {
        // notes.md exists in both stale and target — leave stale's copy.
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("tasks/completed/T036-slug");
        let stale = tmp.path().join("tasks/active/T036-slug");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(target.join("notes.md"), "kept").unwrap();
        std::fs::write(stale.join("notes.md"), "stale dup").unwrap();

        cleanup_stale_task_dirs(tmp.path(), "T036", &target).unwrap();

        assert!(stale.exists(), "stale dir preserved when collision present");
        assert_eq!(
            std::fs::read_to_string(target.join("notes.md")).unwrap(),
            "kept",
            "target's notes.md untouched"
        );
    }

    #[test]
    fn cleanup_stale_task_dirs_noop_when_no_stale() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("tasks/active/T036-slug");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("main.md"), "x").unwrap();

        cleanup_stale_task_dirs(tmp.path(), "T036", &target).unwrap();
        assert!(target.exists());
    }

    /// T036 codex-revise: symlinked task dirs MUST NOT be followed during
    /// stale-dir discovery. A malicious or accidental symlink in
    /// `tasks/<status>/` pointing outside the repo would otherwise let
    /// `cleanup_stale_task_dirs` migrate files OUT of that target into
    /// `target_dir` (or, on render-artifact deletion, unlink the symlink
    /// target's contents). Both `find_all_task_dirs` and the cleanup pass
    /// must skip symlinks at the status-dir AND task-dir level.
    #[cfg(unix)]
    #[test]
    fn find_all_task_dirs_skips_symlinks() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // Outside tempdir contains a "victim" file we must NOT touch.
        std::fs::write(outside.path().join("notes.md"), "user secret").unwrap();

        std::fs::create_dir_all(tmp.path().join("tasks/active")).unwrap();
        // Plant a symlink shaped like a task dir, pointing outside the repo.
        symlink(
            outside.path(),
            tmp.path().join("tasks/active/T036-slug"),
        )
        .unwrap();
        // Plant a real (legitimate) match elsewhere as a control.
        std::fs::create_dir_all(tmp.path().join("tasks/completed/T036-slug")).unwrap();

        let matches = find_all_task_dirs(tmp.path(), "T036");
        assert_eq!(
            matches.len(),
            1,
            "symlinked task dir must be skipped; got {matches:?}"
        );
        assert!(
            matches[0].ends_with("tasks/completed/T036-slug"),
            "only the real (non-symlinked) match should be returned"
        );

        // The outside target's victim file MUST remain untouched after
        // a cleanup pass against the legitimate target.
        let target = tmp.path().join("tasks/completed/T036-slug");
        cleanup_stale_task_dirs(tmp.path(), "T036", &target).unwrap();
        assert!(
            outside.path().join("notes.md").exists(),
            "cleanup must not have followed the symlink to migrate outside files"
        );
    }

    /// T036 codex-revise: a symlinked status dir (e.g., tasks/active → /etc)
    /// MUST also be skipped — otherwise its inner entries would be enumerated
    /// as if they were legitimate task dirs.
    #[cfg(unix)]
    #[test]
    fn find_all_task_dirs_skips_symlinked_status_dir() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("T036-evil")).unwrap();
        std::fs::write(
            outside.path().join("T036-evil/notes.md"),
            "would be migrated out",
        )
        .unwrap();

        std::fs::create_dir_all(tmp.path().join("tasks")).unwrap();
        symlink(outside.path(), tmp.path().join("tasks/active")).unwrap();

        let matches = find_all_task_dirs(tmp.path(), "T036");
        assert!(
            matches.is_empty(),
            "symlinked status dir must be skipped; got {matches:?}"
        );
    }

    // maybe_move_dir
    #[test]
    fn maybe_move_dir_renames_directory() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("tasks/active/T003-task");
        let dst = tmp.path().join("tasks/completed/T003-task");
        std::fs::create_dir_all(&src).unwrap();
        // Create a file inside to verify content moves.
        std::fs::write(src.join("main.md"), "content").unwrap();

        maybe_move_dir(&src, &dst).unwrap();

        assert!(!src.exists(), "src should no longer exist");
        assert!(dst.exists(), "dst should exist");
        assert_eq!(
            std::fs::read_to_string(dst.join("main.md")).unwrap(),
            "content"
        );
    }

    #[test]
    fn maybe_move_dir_noop_when_same() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("tasks/active/T003-task");
        std::fs::create_dir_all(&dir).unwrap();

        // Same src and dst — should be a no-op, no error.
        maybe_move_dir(&dir, &dir).unwrap();
        assert!(dir.exists());
    }
}
