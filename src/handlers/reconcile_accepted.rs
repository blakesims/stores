//! `stores tasks reconcile-accepted` — operator-grounded recovery for
//! `accepted` rows whose post-accept ceremony never fired.
//!
//! T107 / I027 precedent: a row hit `deploy_blocked` (e.g. dirty main
//! worktree blocked accept-merge), the operator cleaned up and ran
//! `tasks retry-deploy`, but the daemon's accept-merge / cargo-install
//! subscribers strictly hooked on `(in_review, accepted)` not
//! `(deploy_blocked, accepted)`, so the recovery edge silently never
//! re-fired the chain. The operator then merged the branch manually.
//! Result: row stranded at `accepted` with work shipped.
//!
//! This verb re-fires the post-accept chain by directly invoking the
//! existing builtins (`accept_merge::run` / `cargo_install::run` /
//! `schema_migrate::run`). It does NOT mark terminal states itself; the
//! builtins fire `mark_cargo_installed` / `mark_schema_migrated` through
//! the framework actor, exactly as the daemon would.
//!
//! Per Pi msg_85be1b1c (2026-05-08):
//! - Allowed only from `accepted` (or mid-chain `cargo_installed` for
//!   resume-after-partial).
//! - Branch must already be merged to main; this verb does NOT merge.
//!   Operators who haven't merged yet should use `tasks retry-deploy`
//!   from `deploy_blocked` (now correctly subscribed via I027) or merge
//!   the branch manually first.
//! - Idempotent: schema_migrated → fail-loud "already reconciled";
//!   cargo_installed → skip cargo-install step, run schema-migrate only.
//! - Operator-grounded authority: ai_with_human or human only.
//!   ai_autonomous is rejected (mirrors retry-deploy's actor gate).

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::flow::builtins::{accept_merge, cargo_install, schema_migrate, DispatchCtx};
use crate::flow::AgentsYaml;
use crate::schema::actor::{Actor, InvokerCtx};

/// Run the reconcile-accepted recovery verb. See module docs for full semantics.
pub fn run_reconcile_accepted(
    conn: &Connection,
    config_path: &Path,
    task_id: &str,
    invoker: InvokerCtx,
) -> Result<()> {
    if invoker.actor == Actor::AiAutonomous {
        bail!(
            "reconcile-accepted requires ai_with_human or human invoker; \
             ai_autonomous is not permitted. Pass --invoker ai_with_human or --invoker human."
        );
    }

    let row = read_task_row(conn, task_id)?;
    let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if status == "schema_migrated" {
        bail!(
            "reconcile-accepted: task {} is already at status='schema_migrated'; nothing to reconcile",
            task_id
        );
    }
    if status != "accepted" && status != "cargo_installed" {
        bail!(
            "reconcile-accepted: task {} has status='{}', expected 'accepted' or 'cargo_installed'",
            task_id,
            status
        );
    }

    if branch.is_empty() {
        bail!(
            "reconcile-accepted: task {} has no branch field set; nothing to reconcile",
            task_id
        );
    }
    if !branch_merged_to_main(workspace_path, branch) {
        bail!(
            "reconcile-accepted: task {}'s branch '{}' is not merged into main. \
             This verb only reconciles rows whose work is already on main. \
             Either merge the branch manually first, or use `tasks retry-deploy` \
             from a deploy_blocked state (post-I027 the daemon's subscriber chain \
             will re-fire correctly).",
            task_id,
            branch
        );
    }

    let stores_dir = crate::paths::stores_dir()?;
    let agents_path = stores_dir.join("agents.yaml");
    let agents = if agents_path.exists() {
        crate::flow::agents_yaml::load_from_path(&agents_path)
            .context("reconcile-accepted: loading .stores/agents.yaml")?
    } else {
        AgentsYaml::default_empty()
    };

    let ctx = DispatchCtx {
        conn,
        agents: &agents,
        config_path,
        policies_hash: "",
    };

    if status == "accepted" {
        accept_merge::run(&row, &ctx).with_context(|| {
            format!(
                "reconcile-accepted: accept-merge step failed for {}",
                task_id
            )
        })?;
    }

    let row_after_merge = read_task_row(conn, task_id)?;
    let status_after_merge = row_after_merge
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if status_after_merge == "accepted" {
        cargo_install::run(&row_after_merge, &ctx).with_context(|| {
            format!(
                "reconcile-accepted: cargo-install step failed for {}",
                task_id
            )
        })?;
    }

    let row_after_cargo = read_task_row(conn, task_id)?;
    let status_after_cargo = row_after_cargo
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if status_after_cargo == "cargo_installed" {
        schema_migrate::run(&row_after_cargo, &ctx).with_context(|| {
            format!(
                "reconcile-accepted: schema-migrate step failed for {}",
                task_id
            )
        })?;
    }

    let final_row = read_task_row(conn, task_id)?;
    let final_status = final_row
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("?");

    println!(
        "reconcile-accepted: task {} reconciled; final status: {}",
        task_id, final_status
    );

    Ok(())
}

fn read_task_row(conn: &Connection, task_id: &str) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT * FROM tasks WHERE display_id = ?1")?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![task_id])?;
    let row = rows
        .next()?
        .ok_or_else(|| anyhow::anyhow!("reconcile-accepted: task {} not found", task_id))?;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i)?;
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
    Ok(Value::Object(obj))
}

fn branch_merged_to_main(workspace_path: &str, branch: &str) -> bool {
    let main_repo = match resolve_main_repo(workspace_path) {
        Some(p) => p,
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(_) => return false,
        },
    };
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

fn resolve_main_repo(workspace_path: &str) -> Option<PathBuf> {
    if workspace_path.is_empty() {
        return None;
    }
    let out = Command::new("git")
        .args(["-C", workspace_path, "rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let common = if PathBuf::from(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        PathBuf::from(workspace_path).join(raw)
    };
    let canon = common.canonicalize().ok()?;
    canon.parent().map(|p| p.to_path_buf())
}
