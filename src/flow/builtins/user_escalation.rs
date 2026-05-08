//! `builtin:user-escalation` — default deploy_blocked handler.
//!
//! Files a substrate observation pointing at the blocked task with conflict
//! context, then fires `ntfy`. Pure side-effect; the row stays `deploy_blocked`
//! awaiting human `resume`.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::flow::NotifyEvent;
use crate::handlers::row::now_iso8601;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let blocked_reason = row
        .get("blocked_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let new_obs_id = file_observation(ctx.conn, display_id, branch, blocked_reason)
        .context("filing user-escalation observation")?;

    let event = NotifyEvent {
        row_id: display_id.to_string(),
        transition_attempted: "tasks: deploy_blocked (user-escalation)".to_string(),
        policy_id_or_actor_halt: "user-escalation".to_string(),
        summary: format!(
            "filed observation {} for deploy_blocked task {} (branch '{}')",
            new_obs_id, display_id, branch
        ),
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);

    Ok(0)
}

/// Opt-in source class for auto-file dedup. Only callers using a class with
/// `dedup_summary_signature=true` fold observations; direct/human add paths do not.
struct ObservationSourceClass {
    source: &'static str,
    dedup_summary_signature: bool,
}

const DEPLOY_BLOCKED_AUTO_SOURCE: ObservationSourceClass = ObservationSourceClass {
    source: "dev",
    dedup_summary_signature: true,
};

/// Direct write into `observations` (bypasses the normal add handler so we
/// don't need a clap matches struct in-process). Returns the keeper L-id.
fn file_observation(
    conn: &Connection,
    task_display_id: &str,
    branch: &str,
    blocked_reason: &str,
) -> Result<String> {
    file_observation_for_source(
        conn,
        &DEPLOY_BLOCKED_AUTO_SOURCE,
        task_display_id,
        branch,
        blocked_reason,
    )
}

fn file_observation_for_source(
    conn: &Connection,
    source_class: &ObservationSourceClass,
    task_display_id: &str,
    branch: &str,
    blocked_reason: &str,
) -> Result<String> {
    let summary = format!(
        "deploy-blocked: task {} merge conflict on branch '{}'",
        task_display_id, branch
    );
    let signature = normalize_summary_signature(&summary);
    let now = now_iso8601();
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if source_class.dedup_summary_signature {
        if let Some(keeper) = tx
            .query_row(
                "SELECT display_id FROM observations \
                 WHERE status = 'open' AND task_id = ?1 AND summary_signature = ?2 \
                   AND source = ?3 AND origin_db IS NULL \
                 ORDER BY id LIMIT 1",
                rusqlite::params![task_display_id, signature, source_class.source],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .context("SELECT observations dedup keeper")?
        {
            tx.execute(
                "UPDATE observations \
                 SET dupe_count = COALESCE(dupe_count, 1) + 1, \
                     last_seen = ?1, updated_at = ?1, updated_by = 'framework' \
                 WHERE display_id = ?2",
                rusqlite::params![now, keeper],
            )
            .context("UPDATE observations dedup keeper")?;
            tx.commit()?;
            return Ok(keeper);
        }
    }

    let max_id: i64 = tx
        .query_row("SELECT COALESCE(MAX(id), 0) FROM observations", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    let next_num = max_id + 1;
    let new_display_id = format!("L{:03}", next_num);
    let body = format!(
        "Task {task_display_id} is in deploy_blocked after accept-merge \
         hit a conflict on branch '{branch}'.\n\nDetails:\n{blocked_reason}\n\n\
         Resume after specialist intervention with: \
         stores tasks resume {task_display_id}"
    );
    let week = ops_week_label(&now);

    tx.execute(
        "INSERT INTO observations \
         (display_id, status, summary, body, source, priority, captured_at, captured_week, task_id, \
          summary_signature, dupe_count, last_seen, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'open', ?2, ?3, ?4, 'normal', ?5, ?6, ?7, ?8, 1, ?5, ?5, ?5, 'framework', 'framework')",
        rusqlite::params![
            new_display_id,
            summary,
            body,
            source_class.source,
            now,
            week,
            task_display_id,
            if source_class.dedup_summary_signature { Some(signature.as_str()) } else { None::<&str> },
        ],
    )
    .context("INSERT INTO observations")?;
    tx.commit()?;

    Ok(new_display_id)
}

fn normalize_summary_signature(summary: &str) -> String {
    let lower = summary.to_ascii_lowercase();
    if lower.starts_with("deploy-blocked:") && lower.contains("merge conflict") {
        "deploy-blocked: merge conflict".to_string()
    } else {
        lower
            .split_whitespace()
            .map(|token| {
                let trimmed = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                if trimmed.len() >= 2
                    && matches!(trimmed.as_bytes()[0], b't' | b'l')
                    && trimmed[1..].chars().all(|c| c.is_ascii_digit())
                {
                    "<id>"
                } else if looks_like_timestamp(trimmed) {
                    "<timestamp>"
                } else {
                    token
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn looks_like_timestamp(s: &str) -> bool {
    s.len() >= 10
        && s.as_bytes().get(4) == Some(&b'-')
        && s.as_bytes().get(7) == Some(&b'-')
        && s[..4].chars().all(|c| c.is_ascii_digit())
}

/// Cheap ops-week label `wWW` from an ISO-ish timestamp prefix. Best-effort —
/// observation captured_week is operator-hygiene per-schema with no pattern.
fn ops_week_label(_now_iso: &str) -> String {
    // Simple stamp; the schema doesn't enforce a pattern (per stores/observations/CLAUDE.md).
    "w-flow-engine".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::schema::Schema;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let schema =
            Schema::from_yaml(include_str!("../../../stores/observations/schema.yaml")).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn first_occurrence_inserts_keeper_with_dupe_count_one() {
        let conn = conn();
        let id = file_observation(&conn, "T123", "feat/T123-x", "merge conflict").unwrap();
        assert_eq!(id, "L001");
        let (count, dupe_count, signature): (i64, i64, String) = conn
            .query_row(
                "SELECT COUNT(*), dupe_count, summary_signature FROM observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(dupe_count, 1);
        assert_eq!(signature, "deploy-blocked: merge conflict");
    }

    #[test]
    fn second_occurrence_folds_into_keeper() {
        let conn = conn();
        let first = file_observation(&conn, "T123", "feat/T123-x", "one").unwrap();
        let second = file_observation(&conn, "T123", "feat/T999-y", "two").unwrap();
        assert_eq!(second, first);
        let dupe_count: i64 = conn
            .query_row(
                "SELECT dupe_count FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(row_count(&conn), 1);
        assert_eq!(dupe_count, 2);
    }

    #[test]
    fn cascade_of_thirty_deploy_blocked_failures_lands_one_keeper() {
        let conn = conn();
        for i in 0..30 {
            file_observation(
                &conn,
                "T999",
                &format!("feat/T999-cascade-{i}"),
                "merge conflict",
            )
            .unwrap();
        }
        let (rows, dupe_count): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(dupe_count) FROM observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(dupe_count, 30);
    }

    #[test]
    fn two_unrelated_tasks_do_not_collapse() {
        let conn = conn();
        file_observation(&conn, "T123", "feat/shared", "merge conflict").unwrap();
        file_observation(&conn, "T124", "feat/shared", "merge conflict").unwrap();
        assert_eq!(row_count(&conn), 2);
    }

    #[test]
    fn summary_with_different_task_id_in_branch_still_collapses_for_same_task() {
        let conn = conn();
        file_observation(&conn, "T123", "feat/T123-a", "merge conflict").unwrap();
        file_observation(&conn, "T123", "feat/T456-a", "merge conflict").unwrap();
        assert_eq!(row_count(&conn), 1);
        let dupe_count: i64 = conn
            .query_row("SELECT dupe_count FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dupe_count, 2);
    }

    #[test]
    fn source_class_without_opt_in_does_not_dedup() {
        let conn = conn();
        let source = ObservationSourceClass {
            source: "dev",
            dedup_summary_signature: false,
        };
        file_observation_for_source(&conn, &source, "T123", "feat/T123-a", "merge conflict")
            .unwrap();
        file_observation_for_source(&conn, &source, "T123", "feat/T123-b", "merge conflict")
            .unwrap();
        assert_eq!(row_count(&conn), 2);
    }

    #[test]
    fn different_sources_do_not_collapse() {
        let conn = conn();
        let qa_source = ObservationSourceClass {
            source: "qa",
            dedup_summary_signature: true,
        };
        file_observation(&conn, "T123", "feat/T123-a", "merge conflict").unwrap();
        file_observation_for_source(&conn, &qa_source, "T123", "feat/T123-b", "merge conflict")
            .unwrap();
        assert_eq!(row_count(&conn), 2);
    }
}
