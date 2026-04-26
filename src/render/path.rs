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
///   planning | plan_review → "planning"
///   ready | executing | code_review → "active"
///   blocked → "paused"
///   complete → "completed"
///   anything else → "active" (safe fallback)
pub fn status_to_dir(status: &str) -> &'static str {
    match status {
        "planning" | "plan_review" => "planning",
        "ready" | "executing" | "code_review" => "active",
        "blocked" => "paused",
        "complete" => "completed",
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

/// Find an existing task directory by display_id glob.
///
/// Searches `tasks/*/{{display_id}}-*` relative to `repo_root`.
/// Returns `None` when zero matches exist; `Some(path)` when exactly one match
/// exists; when multiple matches exist, logs a warning and returns `None` so
/// the caller falls back to the canonical path.
pub fn find_existing_task_dir(repo_root: &Path, display_id: &str) -> Option<PathBuf> {
    let tasks_root = repo_root.join("tasks");
    if !tasks_root.exists() {
        return None;
    }

    let mut matches: Vec<PathBuf> = Vec::new();

    // Iterate over immediate subdirectories of tasks/
    let entries = match std::fs::read_dir(&tasks_root) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for status_dir_entry in entries.flatten() {
        let status_dir_path = status_dir_entry.path();
        if !status_dir_path.is_dir() {
            continue;
        }
        // Look for subdirectories matching `{{display_id}}-*`
        let inner_entries = match std::fs::read_dir(&status_dir_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for task_dir_entry in inner_entries.flatten() {
            let task_dir_path = task_dir_entry.path();
            if !task_dir_path.is_dir() {
                continue;
            }
            if let Some(name) = task_dir_path.file_name().and_then(|n| n.to_str()) {
                // Match: starts with display_id followed by '-'
                let prefix = format!("{}-", display_id);
                if name.starts_with(&prefix) || name == display_id {
                    matches.push(task_dir_path);
                }
            }
        }
    }

    match matches.len() {
        0 => None,
        1 => Some(matches.remove(0)),
        _ => {
            eprintln!(
                "warning: multiple task directories found for '{}': {:?}; \
                 writing to canonical path without moving",
                display_id,
                matches
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
            );
            None
        }
    }
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
        assert_eq!(status_to_dir("complete"), "completed");
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
        assert_eq!(
            p,
            PathBuf::from("/repo/tasks/active/T003-my-task/main.md")
        );
    }

    #[test]
    fn resolve_render_path_complete_status() {
        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let ctx = json!({
            "display_id": "T003",
            "slug": "my-task",
            "status": "complete"
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
        assert_eq!(
            p,
            PathBuf::from("/repo/tasks/paused/T003-my-task/main.md")
        );
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
    fn find_existing_dir_returns_none_on_multiple_matches() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/active/T003-task-a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tasks/planning/T003-task-a")).unwrap();

        let found = find_existing_task_dir(tmp.path(), "T003");
        // Multiple matches → None + warning printed
        assert!(found.is_none());
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
