//! `builtin:accept-merge` — fast-merge a task's branch into the project
//! main branch when the task transitions `in_review → accepted`.
//!
//! Clean merge: row stays `accepted`, daemon records success.
//! Conflict: row → `deploy_blocked` (framework-actor transition), `ntfy`
//! fires, and the row is dispatched to `agents.yaml::deployment_specialist`
//! (default: `builtin:user-escalation`).
//! Missing branch: log warning, leave row as-is.

use anyhow::Context;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::flow::builtins::{
    dispatch_to_specialist, fire_framework_transition, fire_mark_deploy_blocked,
    resolve_main_repo, BuiltinResult, DispatchCtx,
};
use crate::flow::NotifyEvent;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if branch.is_empty() {
        eprintln!(
            "[accept-merge] {}: branch field empty; leaving row accepted (already merged or work happened in-place)",
            display_id
        );
        return Ok(0);
    }
    if workspace_path.is_empty() {
        eprintln!(
            "[accept-merge] {}: workspace_path empty; cannot locate main repo",
            display_id
        );
        return Ok(1);
    }

    // Already-merged short-circuit: when the worktree at workspace_path has
    // been cleaned up post-merge, resolve_main_repo() returns None and the
    // legacy code path falls into a no-op Ok(1). Probe via a usable main-repo
    // path (workspace_path if still live, else daemon cwd) and, if the branch
    // is already merged into main, either fire mark_cargo_installed (solo path)
    // or return a no-op success (cargo-install peer path).
    //
    // When cargo-install is also subscribed on the same accepted-entry edge,
    // it is responsible for firing mark_cargo_installed after running the
    // actual install. Firing it here would race: the row would advance to
    // cargo_installed before cargo-install's subscriber runs, causing it to
    // transition from the wrong state. So we check ctx.agents: if cargo-install
    // is a peer subscriber on any accepted-entry edge, skip the fire and let
    // cargo-install own the mark_cargo_installed step (L145 / codex-revise).
    if let Some(main_repo_for_check) = resolve_main_repo_for_check(workspace_path) {
        if is_branch_merged_into_main(&main_repo_for_check, branch) {
            if cargo_install_subscribed_to_accepted(ctx) {
                eprintln!(
                    "[accept-merge] {}: branch '{}' already merged into main; cargo-install peer present — \
                     skipping mark_cargo_installed (cargo-install will fire it)",
                    display_id, branch
                );
            } else {
                eprintln!(
                    "[accept-merge] {}: branch '{}' already merged into main; firing mark_cargo_installed (noop-merge)",
                    display_id, branch
                );
                fire_framework_transition(
                    ctx.conn,
                    display_id,
                    "mark_cargo_installed",
                    BTreeMap::new(),
                    ctx.policies_hash,
                )
                .with_context(|| {
                    format!(
                        "firing mark_cargo_installed for already-merged {}",
                        display_id
                    )
                })?;
            }
            return Ok(0);
        }
    }

    let main_repo = match resolve_main_repo(workspace_path) {
        Some(p) => p,
        None => {
            eprintln!(
                "[accept-merge] {}: could not resolve main repo from workspace_path '{}'",
                display_id, workspace_path
            );
            return Ok(1);
        }
    };

    // Best-effort fetch — failures don't abort the merge attempt; offline
    // repos with the branch already locally present should still merge.
    let _ = Command::new("git")
        .args(["-C", main_repo.to_str().unwrap_or("."), "fetch"])
        .output();

    let merge = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "merge",
            "--no-ff",
            "--no-edit",
            branch,
        ])
        .output()
        .with_context(|| format!("spawning git merge for {}", display_id))?;

    if merge.status.success() {
        eprintln!(
            "[accept-merge] {}: merged branch '{}' into main",
            display_id, branch
        );
        return Ok(0);
    }

    // Conflict path: collect the conflicted file list, abort the merge,
    // flip row → deploy_blocked, ntfy, dispatch to specialist.
    let conflict_files = list_conflict_files(&main_repo);
    abort_merge(&main_repo);

    let stderr = String::from_utf8_lossy(&merge.stderr).to_string();
    let blocked_reason = format!(
        "merge conflict on branch '{}': {} (last attempt: {})",
        branch,
        if conflict_files.is_empty() {
            "<no conflict files reported>".to_string()
        } else {
            conflict_files.join(", ")
        },
        stderr.lines().next().unwrap_or("merge failed").trim()
    );

    fire_mark_deploy_blocked(ctx.conn, display_id, &blocked_reason, ctx.policies_hash)
        .with_context(|| format!("flipping {} to deploy_blocked", display_id))?;

    let event = NotifyEvent {
        row_id: display_id.to_string(),
        transition_attempted: "tasks: accepted→deploy_blocked".to_string(),
        policy_id_or_actor_halt: "accept-merge: merge conflict".to_string(),
        summary: blocked_reason.clone(),
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);

    // Dispatch to deployment specialist (default builtin:user-escalation).
    dispatch_to_specialist(row, ctx, display_id, "accept-merge");

    Ok(0)
}

/// Best-effort: returns true iff `branch` is an ancestor of `main` in
/// `main_repo`. The CLI form in scope_in (`git branch --merged main | grep
/// <branch>`) is an English description; `merge-base --is-ancestor` is the
/// robust mechanical equivalent (exit 0 = ancestor/merged, non-zero = not).
pub(crate) fn is_branch_merged_into_main(main_repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "merge-base",
            "--is-ancestor",
            branch,
            "main",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Resolve a usable main-repo path for the already-merged probe. Tries the
/// row's `workspace_path` first; if the worktree has been cleaned, falls
/// back to the daemon's cwd (which is the substrate's main repo).
fn resolve_main_repo_for_check(workspace_path: &str) -> Option<PathBuf> {
    if let Some(p) = resolve_main_repo(workspace_path) {
        return Some(p);
    }
    let cwd = std::env::current_dir().ok()?;
    let out = Command::new("git")
        .args([
            "-C",
            cwd.to_str().unwrap_or("."),
            "rev-parse",
            "--git-common-dir",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(cwd)
}

/// Returns true iff any agent in `ctx.agents` with the cargo-install command
/// subscribes to a transition whose `to` is `"accepted"`. This is used by the
/// already-merged short-circuit to decide whether to fire `mark_cargo_installed`
/// directly (solo mode — no peer) or skip it and let the cargo-install subscriber
/// run the actual install first (peer mode — retry-deploy chain, L145).
fn cargo_install_subscribed_to_accepted(ctx: &DispatchCtx) -> bool {
    ctx.agents.agents.iter().any(|a| {
        a.command.trim_start_matches("builtin:") == "cargo-install"
            && a.subscribes_to.iter().any(|s| s.transition.to == "accepted")
    })
}

pub(crate) fn list_conflict_files(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap_or("."),
            "diff",
            "--name-only",
            "--diff-filter=U",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn abort_merge(repo: &Path) {
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap_or("."), "merge", "--abort"])
        .output();
}

pub(crate) fn abort_rebase(repo: &Path) {
    let _ = Command::new("git")
        .args(["-C", repo.to_str().unwrap_or("."), "rebase", "--abort"])
        .output();
}

