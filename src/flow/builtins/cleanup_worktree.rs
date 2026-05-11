//! `builtin:cleanup-worktree` — terminal task workspace artifact cleanup.
//!
//! Runs after terminal/post-land task transitions. It deletes terminal task
//! `target/` artifacts when live-safe, then attempts ordinary
//! `git worktree remove` only when the worktree is clean and merged-safe. It
//! never force-removes dirty worktrees; dirty source-only worktrees are surfaced
//! for later disposition via stderr.

use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::cleanup_worktrees::{cleanup_terminal_task, CleanupClassification};

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[cleanup-worktree] tasks row missing display_id; skipping");
        return Ok(1);
    }

    let report = cleanup_terminal_task(ctx.conn, display_id)?;

    if let Some(deleted) = &report.target_deleted {
        eprintln!(
            "[cleanup-worktree] {}: deleted target {} ({} bytes)",
            display_id,
            deleted.target_path.display(),
            deleted.target_bytes
        );
    } else if let Some(skip) = &report.target_skip {
        eprintln!(
            "[cleanup-worktree] {}: target cleanup skipped ({})",
            display_id,
            skip_label(skip)
        );
    }

    if let Some(removed) = &report.worktree_removed {
        eprintln!(
            "[cleanup-worktree] {}: removed clean worktree {}",
            display_id,
            removed.row.workspace_path.display()
        );
    } else if let Some(skip) = &report.worktree_skip {
        if *skip == CleanupClassification::DirtyWorktree {
            eprintln!(
                "[cleanup-worktree] {}: worktree is dirty; left source-only for disposition",
                display_id
            );
        } else {
            eprintln!(
                "[cleanup-worktree] {}: worktree removal skipped ({})",
                display_id,
                skip_label(skip)
            );
        }
    }

    Ok(0)
}

fn skip_label(skip: &CleanupClassification) -> String {
    format!("{skip:?}")
}

#[cfg(test)]
mod tests {
    use crate::flow::builtins::DispatchCtx;
    use crate::flow::AgentsYaml;
    use rusqlite::Connection;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn dispatch_builtin_returns_some_for_cleanup_worktree() {
        let conn = Connection::open_in_memory().unwrap();
        let agents = AgentsYaml::default_empty();
        let tmp = tempdir().unwrap();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &tmp.path().join(".stores/config.yaml"),
            policies_hash: "test",
        };
        let row = json!({});
        assert!(crate::flow::builtins::dispatch_builtin("cleanup-worktree", &row, &ctx).is_some());
    }
}
