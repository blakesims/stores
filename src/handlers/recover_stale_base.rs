use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

use crate::handlers::{
    external_reviews::git_output, next_id::next_id_for_store, row::now_iso8601,
};
use crate::schema::{
    actor::{Actor, InvokerCtx},
    Schema,
};

/// Lazy ALTER: add superseded_by column to external_reviews if absent.
/// Mirrors the ensure_dispatch_locks_typed pattern in agents_run.rs.
fn ensure_external_reviews_superseded_by_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info('external_reviews')")?;
    let existing: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !existing.iter().any(|col| col == "superseded_by") {
        conn.execute_batch("ALTER TABLE external_reviews ADD COLUMN superseded_by TEXT")?;
    }
    Ok(())
}

struct HeldRow {
    row_id: i64,
    display_id: String,
    head_sha: String,
}

pub fn run_recover_stale_base(
    conn: &Connection,
    external_reviews_schema: &Schema,
    task_id: &str,
    invoker: InvokerCtx,
) -> Result<()> {
    // Actor gate first — no DB access needed for this check (Task 1.5).
    if invoker.actor == Actor::AiAutonomous {
        bail!(
            "recover-stale-base requires ai_with_human or human invoker; \
             ai_autonomous is not permitted. Pass --invoker ai_with_human or --invoker human."
        );
    }

    // Lazy ALTER: ensure superseded_by column exists before any other DB read (Task 1.4).
    ensure_external_reviews_superseded_by_column(conn)?;

    // Load task row: status, workspace_path, branch (Task 1.6).
    let task_row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT status, workspace_path, branch FROM tasks WHERE display_id=?1",
            params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    let (status, workspace_path_opt, branch_opt) = match task_row {
        Some(row) => row,
        None => bail!("recover-stale-base: task {} not found", task_id),
    };

    if status != "blocked" && status != "in_review" {
        bail!(
            "recover-stale-base: task {} has status '{}'; expected 'blocked' or 'in_review'",
            task_id,
            status
        );
    }

    let workspace_path = PathBuf::from(
        workspace_path_opt
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("task {} has no workspace_path", task_id))?,
    );

    // Checkout task branch if set, mirroring prepare_external_review_git lines 151-157 (Task 1.7).
    if let Some(branch) = branch_opt.as_deref().filter(|s| !s.is_empty()) {
        git_output(&workspace_path, &["checkout", branch]).map_err(|e| {
            anyhow::anyhow!(
                "recover-stale-base: cannot checkout task branch '{}': {}",
                branch,
                e
            )
        })?;
    }

    // Resolve current main SHA and branch HEAD SHA (Task 1.7).
    let current_main = git_output(&workspace_path, &["rev-parse", "--verify", "main"])
        .map(|s| s.trim().to_string())
        .map_err(|e| anyhow::anyhow!("recover-stale-base: cannot resolve main SHA: {}", e))?;

    let current_head = git_output(&workspace_path, &["rev-parse", "--verify", "HEAD"])
        .map(|s| s.trim().to_string())
        .map_err(|e| anyhow::anyhow!("recover-stale-base: cannot resolve HEAD SHA: {}", e))?;

    // Query all tooling_held/stale_base_requires_rebase ER rows for this task (Task 1.8).
    let held: Vec<HeldRow> = {
        let mut stmt = conn.prepare(
            "SELECT id, display_id, COALESCE(head_sha, '') \
             FROM external_reviews \
             WHERE task_id=?1 AND status='tooling_held' \
               AND held_reason='stale_base_requires_rebase' \
             ORDER BY attempt DESC, id DESC",
        )?;
        let rows = stmt
            .query_map(params![task_id], |r| {
                Ok(HeldRow {
                    row_id: r.get(0)?,
                    display_id: r.get(1)?,
                    head_sha: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    // Test 3: no held rows (Task 1.8 bail).
    if held.is_empty() {
        bail!(
            "recover-stale-base: no stale_base_requires_rebase tooling_held ER rows found for task {}",
            task_id
        );
    }

    // Test 4: branch not rebased — current head matches latest held head_sha (Task 1.9).
    let latest_held_head = &held[0].head_sha;
    if &current_head == latest_held_head {
        bail!(
            "recover-stale-base: the task branch has not been rebased. \
             Current branch HEAD {} equals the held row's head_sha {}. \
             Please rebase the task branch onto main before running this verb. \
             Current main SHA: {}",
            current_head,
            latest_held_head,
            current_main
        );
    }

    // Idempotency guard: fresh pending/running ER with current head_sha (Task 1.10).
    let existing_fresh: Option<String> = conn
        .query_row(
            "SELECT display_id FROM external_reviews \
             WHERE task_id=?1 AND status IN ('pending','running') AND head_sha=?2 \
             LIMIT 1",
            params![task_id, &current_head],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(existing_er_id) = existing_fresh {
        bail!(
            "recover-stale-base: a fresh external_review {} with the current head_sha already \
             exists in pending/running state for task {}; no action needed",
            existing_er_id,
            task_id
        );
    }

    // Allocate new ER display_id via next_id_for_store (Task 1.11).
    let new_er_id = next_id_for_store(external_reviews_schema, conn)?;

    // Compute new attempt number (Task 1.11).
    let new_attempt: i64 = conn.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM external_reviews WHERE task_id=?1",
        params![task_id],
        |r| r.get(0),
    )?;

    let now = now_iso8601();
    let actor_str = invoker.actor.to_string();

    // Open deferred transaction (Task 1.12, unchecked_transaction matches add.rs/transition.rs).
    let tx = conn.unchecked_transaction()?;

    // INSERT new pending external_review row (Task 1.12).
    tx.execute(
        "INSERT INTO external_reviews \
         (display_id, status, task_id, attempt, adapter, base_sha, head_sha, \
          verdict, held_reason, next_retry_at, superseded_by, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'pending', ?2, ?3, 'external_review', ?4, ?5, \
                 NULL, NULL, NULL, NULL, ?6, ?6, ?7, ?7)",
        params![
            &new_er_id,
            task_id,
            new_attempt,
            &current_main,
            &current_head,
            &now,
            &actor_str
        ],
    )?;
    let new_er_rowid = tx.last_insert_rowid();

    // INSERT creation transition_history for new ER (Task 1.15).
    // from_status="" is the empty-string convention from add.rs:386 for newly-created rows.
    crate::db::insert_transition_history(
        &tx,
        "external_reviews",
        new_er_rowid,
        &new_er_id,
        "",
        "pending",
        "recover-stale-base",
        &actor_str,
        None,
        None,
        Some("operator-recovery"),
    )?;

    // For each held row: UPDATE to superseded + INSERT transition_history (Tasks 1.13-1.14).
    for held_row in &held {
        tx.execute(
            "UPDATE external_reviews \
             SET status='superseded', superseded_by=?2, updated_at=?3 \
             WHERE display_id=?1",
            params![&held_row.display_id, &new_er_id, &now],
        )?;
        crate::db::insert_transition_history(
            &tx,
            "external_reviews",
            held_row.row_id,
            &held_row.display_id,
            "tooling_held",
            "superseded",
            "supersede",
            "framework",
            None,
            None,
            Some("external-review"),
        )?;
    }

    tx.commit()?;

    // Print operator-readable success line (Task 1.16).
    let held_csv: Vec<&str> = held.iter().map(|r| r.display_id.as_str()).collect();
    let base_short = &current_main[..7.min(current_main.len())];
    let head_short = &current_head[..7.min(current_head.len())];
    println!(
        "recover-stale-base: spawned {} pending against base={} head={}; \
         superseded {} older held ER row(s): {}",
        new_er_id,
        base_short,
        head_short,
        held.len(),
        held_csv.join(", ")
    );

    Ok(())
}
