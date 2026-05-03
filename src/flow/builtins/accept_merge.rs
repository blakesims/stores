//! `builtin:accept-merge` — fast-merge a task's branch into the project
//! main branch when the task transitions `in_review → accepted`.
//!
//! Clean merge: row stays `accepted`, daemon records success.
//! Conflict: row → `deploy_blocked` (framework-actor transition), `ntfy`
//! fires, and the row is dispatched to `agents.yaml::deployment_specialist`
//! (default: `builtin:user-escalation`).
//! Missing branch: log warning, leave row as-is.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use crate::flow::builtins::{resolve_main_repo, BuiltinResult, DispatchCtx};
use crate::flow::NotifyEvent;
use crate::handlers::row::read_row;
use crate::handlers::transition::execute_transition_write;
use crate::schema::actor::Actor;
use crate::schema::lifecycle::select_transition;
use crate::schema::Schema;
use crate::validate::{self, EntryMap, Op};

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
    let _ = Command::new("git")
        .args(["-C", main_repo.to_str().unwrap_or("."), "merge", "--abort"])
        .output();

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
    dispatch_to_specialist(row, ctx, display_id);

    Ok(0)
}

fn list_conflict_files(main_repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
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

fn dispatch_to_specialist(row: &Value, ctx: &DispatchCtx, display_id: &str) {
    let spec_name = ctx
        .agents
        .deployment_specialist
        .as_deref()
        .unwrap_or("builtin:user-escalation");

    // Refresh the row so user-escalation sees status=deploy_blocked + the
    // freshly-written blocked_reason.
    let refreshed = match refresh_row(ctx.conn, display_id) {
        Some(v) => v,
        None => row.clone(),
    };

    // Two shapes accepted: a direct "builtin:<kw>" sentinel OR a named agent
    // declared in agents.yaml whose `command` is a builtin.
    if let Some(kw) = spec_name.strip_prefix("builtin:") {
        if let Some(res) = crate::flow::builtins::dispatch_builtin(kw, &refreshed, ctx) {
            if let Err(e) = res {
                eprintln!("[accept-merge] specialist '{}' failed: {}", spec_name, e);
            }
        } else {
            eprintln!("[accept-merge] unknown builtin specialist: {}", spec_name);
        }
        return;
    }

    if let Some(agent) = ctx.agents.agents.iter().find(|a| a.name == spec_name) {
        if let Some(kw) = agent.command.strip_prefix("builtin:") {
            if let Some(res) = crate::flow::builtins::dispatch_builtin(kw, &refreshed, ctx) {
                if let Err(e) = res {
                    eprintln!("[accept-merge] specialist '{}' failed: {}", agent.name, e);
                }
            }
        } else {
            // Shell command specialist: best-effort spawn with row env.
            let _ = Command::new("sh")
                .arg("-c")
                .arg(&agent.command)
                .env("STORES_DISPLAY_ID", display_id)
                .env("STORES_STORE", "tasks")
                .status();
        }
    } else {
        eprintln!(
            "[accept-merge] deployment_specialist '{}' not found in agents.yaml",
            spec_name
        );
    }
}

fn refresh_row(conn: &Connection, display_id: &str) -> Option<Value> {
    let mut stmt = conn
        .prepare("SELECT * FROM tasks WHERE display_id = ?1")
        .ok()?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![display_id]).ok()?;
    let row = rows.next().ok()??;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i).ok()?;
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(i) => Value::from(i),
            rusqlite::types::Value::Real(f) => {
                Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            }
            rusqlite::types::Value::Text(s) => Value::String(s),
            rusqlite::types::Value::Blob(b) => {
                Value::String(String::from_utf8_lossy(&b).to_string())
            }
        };
        obj.insert(name.clone(), jv);
    }
    Some(Value::Object(obj))
}

/// Fire the framework-actor `mark_deploy_blocked` transition in-process.
/// Loads the bundled `tasks` schema, builds a minimal diff carrying
/// `blocked_reason`, and writes via `execute_transition_write` so the
/// transition lands on the audit trail with `policy_ref`/`policies_hash`
/// honored when the daemon set them.
fn fire_mark_deploy_blocked(
    conn: &Connection,
    display_id: &str,
    blocked_reason: &str,
    policies_hash: &str,
) -> Result<()> {
    let schema = load_tasks_schema()?;
    let tx = conn.unchecked_transaction()?;

    let (row_id, existing) = read_row(&schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut diff: EntryMap = std::collections::BTreeMap::new();
    diff.insert(
        "blocked_reason".to_string(),
        Value::String(blocked_reason.to_string()),
    );

    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    let transition = select_transition(
        &schema.lifecycle.transitions,
        &current_status,
        "mark_deploy_blocked",
        None,
        &merged,
    )?;

    validate::validate(
        &schema,
        &merged,
        Op::Transition("mark_deploy_blocked".to_string(), diff.clone()),
        Actor::Framework.into(),
    )
    .map_err(|errs| {
        anyhow!(
            "mark_deploy_blocked validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    let phash_opt = if policies_hash.is_empty() {
        None
    } else {
        Some(policies_hash)
    };
    execute_transition_write(
        &tx,
        &schema,
        row_id,
        display_id,
        &current_status,
        &transition.to,
        "mark_deploy_blocked",
        &diff,
        &merged,
        Actor::Framework,
        None,
        phash_opt,
    )?;

    tx.commit()?;
    Ok(())
}

fn load_tasks_schema() -> Result<Schema> {
    let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .map(|(_, y)| *y)
        .ok_or_else(|| anyhow!("tasks bundled schema not found"))?;
    Schema::from_yaml(yaml).context("parsing bundled tasks schema")
}
