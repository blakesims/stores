use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::handlers::disposition::{
    operator_disposition, Disposition, GitBranchStateSource, PlanStartBucket,
};
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
        let row_status =
            row_status(conn, &raw.store, raw.row_id)?.unwrap_or_else(|| "<missing>".to_string());
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
        (
            "retry_wait",
            "terminal failure is scheduled/eligible for retry handling",
        )
    } else if matches!(
        raw.terminal_reason.as_str(),
        "exit_nonzero" | "error" | "silent_zombie" | "timeout" | "halted" | "rate_limit"
    ) {
        (
            "fresh_failure",
            "terminal failure needs operator or retry attention",
        )
    } else {
        (
            "stale_harmless",
            "terminal lock history; should not block dispatch",
        )
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
    let sql = format!(
        "SELECT status FROM \"{}\" WHERE id=?1",
        store.replace('"', "")
    );
    conn.query_row(&sql, [row_id], |r| r.get(0))
        .optional()
        .map_err(Into::into)
}

fn is_terminal_row_status(status: &str) -> bool {
    matches!(
        status,
        "abandoned"
            | "closed_out_of_band"
            | "schema_migrated"
            | "rejected"
            | "resolved"
            | "wont_fix"
            | "passed"
            | "superseded"
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
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// engine plan-start — ignition plan surface (T140 P4)
//
// Read-only end-to-end: open the substrate DB with SQLITE_OPEN_READ_ONLY,
// classify every tasks row via handlers::disposition::operator_disposition,
// group by Disposition::plan_start_bucket(), and render either a tabular
// text surface (default) or a JSON document (--json) with exactly the five
// contract-named buckets: would_run, inactive, needs_operator, blocked,
// historical. plan-start MUST NOT trigger startup sweeps, daemon ticks, or
// any side-effecting subscriber — the operator runs it before turning the
// engine on, to see exactly what would combust.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanStartEntry {
    pub display_id: String,
    pub status: String,
    pub activation: String,
    pub disposition: String,
    pub title: String,
    pub linked_observations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanStartBuckets {
    pub would_run: Vec<PlanStartEntry>,
    pub inactive: Vec<PlanStartEntry>,
    pub needs_operator: Vec<PlanStartEntry>,
    pub blocked: Vec<PlanStartEntry>,
    pub historical: Vec<PlanStartEntry>,
}

#[derive(Debug, Clone)]
struct PlanStartRow {
    entry: PlanStartEntry,
    tier_hint: String,
    label: String,
}

pub fn run_plan_start(json: bool) -> Result<()> {
    let db_path = paths::db_path()?;
    // Read-only end-to-end: opens the DB with SQLITE_OPEN_READ_ONLY so plan-start
    // cannot mutate substrate state even by accident. (T140 P4 contract; matches
    // the same pattern used by `stores watch` and the TUI.)
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening stores db {} read-only", db_path.display()))?;

    let buckets_with_meta = load_plan_start(&conn)?;

    if json {
        let buckets = strip_meta(&buckets_with_meta);
        println!("{}", serde_json::to_string_pretty(&buckets)?);
    } else {
        print_plan_start_text(&buckets_with_meta);
    }
    Ok(())
}

fn strip_meta(rows: &BucketsWithMeta) -> PlanStartBuckets {
    let extract = |bucket: &Vec<PlanStartRow>| -> Vec<PlanStartEntry> {
        bucket.iter().map(|r| r.entry.clone()).collect()
    };
    PlanStartBuckets {
        would_run: extract(&rows.would_run),
        inactive: extract(&rows.inactive),
        needs_operator: extract(&rows.needs_operator),
        blocked: extract(&rows.blocked),
        historical: extract(&rows.historical),
    }
}

#[derive(Debug, Clone, Default)]
struct BucketsWithMeta {
    would_run: Vec<PlanStartRow>,
    inactive: Vec<PlanStartRow>,
    needs_operator: Vec<PlanStartRow>,
    blocked: Vec<PlanStartRow>,
    historical: Vec<PlanStartRow>,
}

fn load_plan_start(conn: &Connection) -> Result<BucketsWithMeta> {
    if !table_exists(conn, "tasks")? {
        return Ok(BucketsWithMeta::default());
    }
    let accepted_at_map = load_accepted_at_map(conn)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let branch_state = GitBranchStateSource::new(cwd, "main");
    let today = wall_clock_today();

    let mut stmt = conn.prepare(
        "SELECT id, display_id, COALESCE(status,''), COALESCE(activation,'inactive'),
                COALESCE(branch,''), COALESCE(tier_hint,''), COALESCE(title,''),
                COALESCE(linked_observations,'[]')
         FROM tasks
         ORDER BY display_id ASC",
    )?;

    struct Raw {
        id: i64,
        display_id: String,
        status: String,
        activation: String,
        branch: String,
        tier_hint: String,
        title: String,
        linked_observations: String,
    }

    let raw_rows = stmt
        .query_map([], |r| {
            Ok(Raw {
                id: r.get(0)?,
                display_id: r.get(1)?,
                status: r.get(2)?,
                activation: r.get(3)?,
                branch: r.get(4)?,
                tier_hint: r.get(5)?,
                title: r.get(6)?,
                linked_observations: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut buckets = BucketsWithMeta::default();

    for raw in raw_rows {
        let linked_observations = parse_linked_observations(&raw.linked_observations);
        let mut row_json = json!({
            "display_id": raw.display_id,
            "status": raw.status,
            "activation": raw.activation,
            "branch": raw.branch,
            "linked_observations": linked_observations,
        });
        if let Some(at) = accepted_at_map.get(&raw.id) {
            row_json["accepted_at"] = json!(at);
        }
        let disposition = operator_disposition(&row_json, today, &branch_state);
        let bucket = disposition.plan_start_bucket();
        let label = disposition.display_label().to_string();
        let disposition_kind = disposition_kind(&disposition);

        let entry = PlanStartEntry {
            display_id: raw.display_id,
            status: raw.status,
            activation: raw.activation,
            disposition: disposition_kind,
            title: raw.title,
            linked_observations,
        };
        let row = PlanStartRow {
            entry,
            tier_hint: raw.tier_hint,
            label,
        };
        match bucket {
            PlanStartBucket::WouldRun => buckets.would_run.push(row),
            PlanStartBucket::Inactive => buckets.inactive.push(row),
            PlanStartBucket::NeedsOperator => buckets.needs_operator.push(row),
            PlanStartBucket::Blocked => buckets.blocked.push(row),
            PlanStartBucket::Historical => buckets.historical.push(row),
        }
    }

    Ok(buckets)
}

fn parse_linked_observations(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn disposition_kind(d: &Disposition) -> String {
    // Use the serde-tag form: serialize and pull `kind`. Stable wire string.
    match serde_json::to_value(d).ok().and_then(|v| {
        v.get("kind")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
    }) {
        Some(s) => s,
        None => "Unknown".to_string(),
    }
}

/// Build a map of tasks.id → accepted_at (max occurred_at across
/// transition_history rows with to_status='accepted'). Missing tasks have no
/// entry; callers leave `accepted_at` unset on the row JSON.
fn load_accepted_at_map(conn: &Connection) -> Result<BTreeMap<i64, String>> {
    let mut map = BTreeMap::new();
    if !table_exists(conn, "transition_history")? {
        return Ok(map);
    }
    let mut stmt = conn.prepare(
        "SELECT row_id, MAX(occurred_at)
         FROM transition_history
         WHERE store='tasks' AND to_status='accepted'
         GROUP BY row_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let row_id: i64 = r.get(0)?;
        let at: Option<String> = r.get(1)?;
        Ok((row_id, at))
    })?;
    for r in rows {
        let (row_id, at) = r?;
        if let Some(at) = at {
            map.insert(row_id, at);
        }
    }
    Ok(map)
}

const BUCKET_ORDER: &[(PlanStartBucket, &str, &str)] = &[
    (
        PlanStartBucket::WouldRun,
        "would_run",
        "tasks the engine will combust on activation",
    ),
    (
        PlanStartBucket::Inactive,
        "inactive",
        "rows opted out of combustion via activation",
    ),
    (
        PlanStartBucket::NeedsOperator,
        "needs_operator",
        "operator decision required before engine handles",
    ),
    (
        PlanStartBucket::Blocked,
        "blocked",
        "blocked rows awaiting human recovery",
    ),
    (
        PlanStartBucket::Historical,
        "historical",
        "terminal exhaust; not in the active lane",
    ),
];

fn print_plan_start_text(rows: &BucketsWithMeta) {
    let n_would = rows.would_run.len();
    let n_inact = rows.inactive.len();
    let n_op = rows.needs_operator.len();
    let n_block = rows.blocked.len();
    let n_hist = rows.historical.len();
    println!(
        "engine ignition plan: {n_would} would-run · {n_inact} inactive · {n_op} needs-operator · {n_block} blocked · {n_hist} historical"
    );
    println!();

    for (bucket, key, blurb) in BUCKET_ORDER {
        let bucket_rows: &Vec<PlanStartRow> = match bucket {
            PlanStartBucket::WouldRun => &rows.would_run,
            PlanStartBucket::Inactive => &rows.inactive,
            PlanStartBucket::NeedsOperator => &rows.needs_operator,
            PlanStartBucket::Blocked => &rows.blocked,
            PlanStartBucket::Historical => &rows.historical,
        };
        println!("{} ({}): {}", key, bucket_rows.len(), blurb);
        for r in bucket_rows {
            let tier = if r.tier_hint.is_empty() {
                "-".to_string()
            } else {
                r.tier_hint.clone()
            };
            println!(
                "  {:<6} [{}] {:<22} {:<8} {:<36} {}",
                r.entry.display_id,
                tier,
                truncate(&r.entry.status, 22),
                r.entry.activation,
                truncate(&r.label, 36),
                truncate(&r.entry.title, 60),
            );
        }
        println!();
    }
}

/// Wall-clock "today" anchor for [`operator_disposition`]. The disposition
/// function only consults this transitively (the cutoff comparison reads
/// `accepted_at`, not `today`), so the precise resolution doesn't matter —
/// we just need a real DateTime<Utc>. Built without chrono's `clock` feature
/// by routing through `std::time::SystemTime`.
fn wall_clock_today() -> DateTime<Utc> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_else(|| {
        DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid DateTime")
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
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
