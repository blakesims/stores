use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::path::PathBuf;

use crate::paths;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockStatusRow {
    pub display_id: String,
    pub store: String,
    pub agent_name: String,
    pub bucket: String,
    pub last_status: String,
    pub terminal_reason: String,
    pub attempts: i64,
    pub claim_source: String,
    pub claimed_at: String,
    pub finished_at: String,
    pub next_retry_at: String,
    pub daemon_epoch: String,
    pub postcondition_id: String,
    pub row_status: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockStatusSummary {
    pub live_claim: usize,
    pub retry_wait: usize,
    pub stale_harmless: usize,
    pub stale_blocking: usize,
    pub orphaned: usize,
    pub fresh_failure: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LockStatusReport {
    pub db_path: String,
    pub summary: LockStatusSummary,
    pub rows: Vec<LockStatusRow>,
}

pub fn run_locks(json: bool) -> Result<()> {
    let db_path = paths::db_path()?;
    let conn = Connection::open(&db_path)
        .with_context(|| format!("opening stores db {}", db_path.display()))?;
    let report = load_lock_status_report(&conn, db_path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_lock_status_text(&report);
    }
    Ok(())
}

pub fn load_lock_status_report(conn: &Connection, db_path: PathBuf) -> Result<LockStatusReport> {
    if !table_exists(conn, "dispatch_locks")? {
        return Ok(LockStatusReport {
            db_path: db_path.display().to_string(),
            summary: LockStatusSummary {
                live_claim: 0,
                retry_wait: 0,
                stale_harmless: 0,
                stale_blocking: 0,
                orphaned: 0,
                fresh_failure: 0,
            },
            rows: Vec::new(),
        });
    }

    let mut stmt = conn.prepare(
        "SELECT store, row_id, display_id, agent_name,
                COALESCE(claimed_at,''), COALESCE(claimed_by,''), COALESCE(attempts,0),
                COALESCE(last_status,''), COALESCE(finished_at,''), COALESCE(daemon_epoch,''),
                COALESCE(claim_source,''), COALESCE(attempt,0), COALESCE(pid,0),
                COALESCE(heartbeat_at,''), COALESCE(postcondition_id,''),
                COALESCE(terminal_reason,''), COALESCE(next_retry_at,'')
         FROM dispatch_locks
         ORDER BY
           CASE
             WHEN finished_at='' AND terminal_reason='' THEN 0
             WHEN terminal_reason IN ('exit_nonzero','error','silent_zombie','timeout','halted','rate_limit') THEN 1
             ELSE 2
           END,
           claimed_at DESC",
    )?;

    let raw_rows = stmt.query_map([], |r| {
        Ok(RawLockRow {
            store: r.get(0)?,
            row_id: r.get(1)?,
            display_id: r.get(2)?,
            agent_name: r.get(3)?,
            claimed_at: r.get(4)?,
            attempts: r.get(6)?,
            last_status: r.get(7)?,
            finished_at: r.get(8)?,
            daemon_epoch: r.get(9)?,
            claim_source: r.get(10)?,
            pid: r.get(12)?,
            postcondition_id: r.get(14)?,
            terminal_reason: r.get(15)?,
            next_retry_at: r.get(16)?,
        })
    })?;

    let mut rows = Vec::new();
    for raw in raw_rows {
        let raw = raw?;
        let row_status = row_status(conn, &raw.store, raw.row_id)?.unwrap_or_else(|| "<missing>".to_string());
        rows.push(classify_lock_row(raw, row_status));
    }

    let summary = LockStatusSummary {
        live_claim: rows.iter().filter(|r| r.bucket == "live_claim").count(),
        retry_wait: rows.iter().filter(|r| r.bucket == "retry_wait").count(),
        stale_harmless: rows.iter().filter(|r| r.bucket == "stale_harmless").count(),
        stale_blocking: rows.iter().filter(|r| r.bucket == "stale_blocking").count(),
        orphaned: rows.iter().filter(|r| r.bucket == "orphaned").count(),
        fresh_failure: rows.iter().filter(|r| r.bucket == "fresh_failure").count(),
    };

    Ok(LockStatusReport {
        db_path: db_path.display().to_string(),
        summary,
        rows,
    })
}

#[derive(Debug)]
struct RawLockRow {
    store: String,
    row_id: i64,
    display_id: String,
    agent_name: String,
    claimed_at: String,
    attempts: i64,
    last_status: String,
    finished_at: String,
    daemon_epoch: String,
    claim_source: String,
    pid: i64,
    postcondition_id: String,
    terminal_reason: String,
    next_retry_at: String,
}

fn classify_lock_row(raw: RawLockRow, row_status: String) -> LockStatusRow {
    let terminal = !raw.finished_at.is_empty() || !raw.terminal_reason.is_empty();
    let row_missing = row_status == "<missing>";
    let row_terminal = is_terminal_row_status(&row_status);
    let pid_live = raw.pid > 0 && process_is_live(raw.pid);
    let retry_wait = !raw.next_retry_at.is_empty()
        && matches!(
            raw.terminal_reason.as_str(),
            "exit_nonzero" | "error" | "silent_zombie" | "timeout" | "rate_limit"
        );

    let (bucket, note) = if row_missing {
        (
            "orphaned",
            "lock references a missing store row; inspect before assuming harmless",
        )
    } else if row_terminal {
        (
            "stale_harmless",
            "row is terminal; lock history should not block dispatch",
        )
    } else if !terminal {
        if pid_live {
            ("live_claim", "unfinished lock has a live pid")
        } else {
            (
                "stale_blocking",
                "unfinished lock has no live pid evidence and may block dispatch",
            )
        }
    } else if retry_wait {
        ("retry_wait", "terminal failure is scheduled/eligible for retry handling")
    } else if matches!(
        raw.terminal_reason.as_str(),
        "exit_nonzero" | "error" | "silent_zombie" | "timeout" | "halted" | "rate_limit"
    ) {
        ("fresh_failure", "terminal failure needs operator or retry attention")
    } else {
        ("stale_harmless", "terminal lock history; should not block dispatch")
    };

    LockStatusRow {
        display_id: raw.display_id,
        store: raw.store,
        agent_name: raw.agent_name,
        bucket: bucket.to_string(),
        last_status: raw.last_status,
        terminal_reason: raw.terminal_reason,
        attempts: raw.attempts,
        claim_source: raw.claim_source,
        claimed_at: raw.claimed_at,
        finished_at: raw.finished_at,
        next_retry_at: raw.next_retry_at,
        daemon_epoch: raw.daemon_epoch,
        postcondition_id: raw.postcondition_id,
        row_status,
        note: note.to_string(),
    }
}

fn row_status(conn: &Connection, store: &str, row_id: i64) -> Result<Option<String>> {
    if !table_exists(conn, store)? {
        return Ok(None);
    }
    let sql = format!("SELECT status FROM \"{}\" WHERE id=?1", store.replace('"', ""));
    conn.query_row(&sql, [row_id], |r| r.get(0))
        .optional()
        .map_err(Into::into)
}

fn is_terminal_row_status(status: &str) -> bool {
    matches!(
        status,
        "abandoned" | "closed_out_of_band" | "schema_migrated" | "rejected" | "resolved" | "wont_fix" | "passed" | "superseded"
    )
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(Into::into)
}

fn process_is_live(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn print_lock_status_text(report: &LockStatusReport) {
    println!("Engine locks ({})", report.db_path);
    println!(
        "summary: live={} retry_wait={} stale_harmless={} stale_blocking={} orphaned={} fresh_failure={}",
        report.summary.live_claim,
        report.summary.retry_wait,
        report.summary.stale_harmless,
        report.summary.stale_blocking,
        report.summary.orphaned,
        report.summary.fresh_failure
    );
    println!();

    for bucket in [
        "live_claim",
        "stale_blocking",
        "fresh_failure",
        "retry_wait",
        "orphaned",
        "stale_harmless",
    ] {
        let bucket_rows: Vec<_> = report.rows.iter().filter(|r| r.bucket == bucket).collect();
        if bucket_rows.is_empty() {
            continue;
        }
        println!("{} ({})", bucket, bucket_rows.len());
        for r in bucket_rows.iter().take(20) {
            println!(
                "  {:<6} {:<18} {:<18} row_status={:<18} attempts={} terminal={} next_retry={} note={}",
                r.display_id,
                r.store,
                r.agent_name,
                r.row_status,
                r.attempts,
                empty_dash(&r.terminal_reason),
                empty_dash(&r.next_retry_at),
                r.note
            );
        }
        if bucket_rows.len() > 20 {
            println!("  ... {} more", bucket_rows.len() - 20);
        }
        println!();
    }
}

fn empty_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_live_unfinished_lock_with_live_pid() {
        let raw = RawLockRow {
            store: "tasks".into(),
            row_id: 1,
            display_id: "T001".into(),
            agent_name: "auto-drive".into(),
            claimed_at: "now".into(),
            attempts: 1,
            last_status: "".into(),
            finished_at: "".into(),
            daemon_epoch: "e".into(),
            claim_source: "try_claim".into(),
            pid: std::process::id() as i64,
            postcondition_id: "".into(),
            terminal_reason: "".into(),
            next_retry_at: "".into(),
        };
        let row = classify_lock_row(raw, "executing".into());
        assert_eq!(row.bucket, "live_claim");
    }

    #[test]
    fn classify_unfinished_terminal_row_as_stale_harmless() {
        let raw = RawLockRow {
            store: "tasks".into(),
            row_id: 1,
            display_id: "T001".into(),
            agent_name: "auto-drive".into(),
            claimed_at: "now".into(),
            attempts: 1,
            last_status: "".into(),
            finished_at: "".into(),
            daemon_epoch: "e".into(),
            claim_source: "try_claim".into(),
            pid: 0,
            postcondition_id: "".into(),
            terminal_reason: "".into(),
            next_retry_at: "".into(),
        };
        let row = classify_lock_row(raw, "schema_migrated".into());
        assert_eq!(row.bucket, "stale_harmless");
    }

    #[test]
    fn classify_finished_ok_as_stale_harmless() {
        let raw = RawLockRow {
            store: "tasks".into(),
            row_id: 1,
            display_id: "T001".into(),
            agent_name: "auto-drive".into(),
            claimed_at: "now".into(),
            attempts: 1,
            last_status: "ok".into(),
            finished_at: "done".into(),
            daemon_epoch: "e".into(),
            claim_source: "try_claim".into(),
            pid: 0,
            postcondition_id: "".into(),
            terminal_reason: "ok".into(),
            next_retry_at: "".into(),
        };
        let row = classify_lock_row(raw, "schema_migrated".into());
        assert_eq!(row.bucket, "stale_harmless");
    }

    #[test]
    fn classify_failed_with_retry_as_retry_wait() {
        let raw = RawLockRow {
            store: "tasks".into(),
            row_id: 1,
            display_id: "T001".into(),
            agent_name: "auto-drive".into(),
            claimed_at: "now".into(),
            attempts: 2,
            last_status: "exit=1".into(),
            finished_at: "done".into(),
            daemon_epoch: "e".into(),
            claim_source: "try_claim".into(),
            pid: 0,
            postcondition_id: "".into(),
            terminal_reason: "error".into(),
            next_retry_at: "later".into(),
        };
        let row = classify_lock_row(raw, "executing".into());
        assert_eq!(row.bucket, "retry_wait");
    }

    #[test]
    fn classify_missing_row_as_orphaned() {
        let raw = RawLockRow {
            store: "tasks".into(),
            row_id: 999,
            display_id: "T999".into(),
            agent_name: "auto-drive".into(),
            claimed_at: "now".into(),
            attempts: 1,
            last_status: "ok".into(),
            finished_at: "done".into(),
            daemon_epoch: "e".into(),
            claim_source: "try_claim".into(),
            pid: 0,
            postcondition_id: "".into(),
            terminal_reason: "ok".into(),
            next_retry_at: "".into(),
        };
        let row = classify_lock_row(raw, "<missing>".into());
        assert_eq!(row.bucket, "orphaned");
    }
}
