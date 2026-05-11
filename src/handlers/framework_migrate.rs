//! Framework-DDL drift detection + auto-apply (T051).
//!
//! Older DBs predate columns added to SUBSTRATE_DDL by newer binaries. The
//! `CREATE TABLE IF NOT EXISTS` re-execution in `db::open` is a no-op on
//! existing tables, so missing columns are not applied. This module
//! introspects each framework table via `PRAGMA table_info` and ALTERs in
//! whatever columns are missing, recording each application in
//! `substrate_migrations` for audit.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::codegen::ddl::{quote_ident, FrameworkColumn, FRAMEWORK_DDL_TABLES};

/// T140 P1: in-flight task statuses. Rows whose status is in this set at
/// migration time are backfilled to `activation='active'` so currently-running
/// work continues seamlessly across a binary upgrade. Every other status
/// (planning, plan_review, ready, blocked, deploy_blocked, accepted,
/// integration_queued, integration_blocked, complete, in_review,
/// cargo_installed, schema_migrated, integrated, rejected, abandoned,
/// closed_out_of_band) backfills to `activation='inactive'` via the column's
/// DDL DEFAULT. P3 reuses this constant as the gating predicate for
/// combustion-class subscribers — single source of truth.
pub const IN_FLIGHT_STATES: &[&str] = &["executing", "code_review", "integrating"];

/// Name of the partial UNIQUE index that enforces the integration-lane merge
/// capacity invariant on the `tasks` table.
///
/// ADR0001 P3 permits multiple rows to be in `status='integrating'` while they
/// are refreshing, task_review, or testing. Only the truth-mutating merging
/// substep is singleton.
pub const INTEGRATION_SINGLETON_INDEX: &str = "idx_tasks_integration_singleton";

/// SQL for the partial UNIQUE index. Index value `1` is constant for every
/// row that matches the WHERE predicate, so the UNIQUE constraint reduces to
/// "at most one row may be merging".
pub const INTEGRATION_SINGLETON_INDEX_DDL: &str = concat!(
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_integration_singleton ",
    "ON tasks((1)) WHERE status='integrating' AND integration_step='merging'"
);

/// Create or upgrade the integration-lane merge capacity partial UNIQUE index
/// on `tasks` if the `tasks` table exists. The `tasks` store is installed
/// separately from the substrate tables, so we guard the CREATE INDEX with a
/// table-existence check (mirrors `ensure_runs_view_if_tasks_exists`).
pub fn ensure_integration_singleton_index(conn: &Connection) -> Result<()> {
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !tasks_exists {
        return Ok(());
    }
    let existing_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
            [INTEGRATION_SINGLETON_INDEX],
            |r| r.get(0),
        )
        .ok();
    if existing_sql
        .as_deref()
        .map(|sql| !sql.contains("integration_step='merging'"))
        .unwrap_or(false)
    {
        conn.execute_batch("DROP INDEX idx_tasks_integration_singleton")
            .context("drop legacy idx_tasks_integration_singleton")?;
    }
    conn.execute_batch(INTEGRATION_SINGLETON_INDEX_DDL)
        .context("create idx_tasks_integration_singleton")?;
    Ok(())
}

/// Drift between SUBSTRATE_DDL (compiled-in) and the live DB. Currently the
/// substrate only emits additive migrations; type changes / drops are out of
/// scope.
#[derive(Debug, Default)]
pub struct FrameworkDrift {
    /// (table_name, column) for every framework column missing from the DB.
    /// Tables that don't yet exist (PRAGMA empty) are skipped — `db::open`'s
    /// `execute_batch(SUBSTRATE_DDL)` creates those fresh, no ALTER needed.
    pub additive: Vec<(String, &'static FrameworkColumn)>,
}

/// One row materialised in `substrate_migrations` after a successful apply.
#[derive(Debug, Clone, Serialize)]
pub struct AppliedFrameworkMigration {
    pub table_name: String,
    pub column_name: String,
    pub ddl_applied: String,
    pub binary_version: String,
    pub applied_at: String,
}

fn diff_one_table(
    conn: &Connection,
    table: &'static crate::codegen::ddl::FrameworkTable,
) -> Result<Vec<&'static FrameworkColumn>> {
    let live = read_table_info(conn, table.name)
        .with_context(|| format!("PRAGMA table_info({})", table.name))?;
    if live.is_empty() {
        // Table doesn't yet exist — `execute_batch(SUBSTRATE_DDL)` will
        // create it whole-cloth. No ALTER drift is owed.
        return Ok(Vec::new());
    }
    let mut missing: Vec<&'static FrameworkColumn> = Vec::new();
    for col in table.columns {
        if !live.iter().any(|n| n == col.name) {
            missing.push(col);
        }
    }
    Ok(missing)
}

/// Diff every entry in `FRAMEWORK_DDL_TABLES` against the live DB.
/// Codex T051-r1 MEDIUM: only `additive` columns are candidates for ALTER
/// TABLE ADD COLUMN against an existing DB. Non-additive (baseline) columns
/// must already be present from a prior `execute_batch(SUBSTRATE_DDL)` —
/// attempting to ALTER them onto a partially-malformed existing table can
/// execute DDL the boot validator never checked.
pub fn compute_framework_drift(conn: &Connection) -> Result<FrameworkDrift> {
    let mut drift = FrameworkDrift::default();
    for t in FRAMEWORK_DDL_TABLES {
        for col in diff_one_table(conn, t)? {
            if !col.additive {
                continue;
            }
            drift.additive.push((t.name.to_string(), col));
        }
    }
    Ok(drift)
}

/// Apply every additive drift entry transactionally and record one row per
/// applied column in `substrate_migrations`. Returns the materialised audit
/// rows. No-op (returns empty Vec) when there is no drift.
pub fn apply_framework_drift(conn: &Connection) -> Result<Vec<AppliedFrameworkMigration>> {
    let drift = compute_framework_drift(conn)?;
    if drift.additive.is_empty() {
        ensure_integration_singleton_index(conn)?;
        ensure_adr0002_upstream_columns(conn)?;
        backfill_adr0002_upstream(conn)?;
        return Ok(Vec::new());
    }
    let binary_version = env!("CARGO_PKG_VERSION").to_string();
    let applied_at = crate::handlers::row::now_iso8601();

    let mut applied: Vec<AppliedFrameworkMigration> = Vec::with_capacity(drift.additive.len());
    let mut backfill_tasks_activation = false;
    let mut backfill_tasks_lifecycle_overlay = false;
    for (table, col) in &drift.additive {
        let ddl = format!(
            "ALTER TABLE {} ADD COLUMN {};",
            quote_ident(table),
            col.full_def
        );
        if table == "tasks" && col.name == "activation" {
            backfill_tasks_activation = true;
        }
        if table == "tasks"
            && matches!(
                col.name,
                "lifecycle"
                    | "active_step"
                    | "integration_step"
                    | "blocked"
                    | "blocker_kind"
                    | "post_integration_step"
            )
        {
            backfill_tasks_lifecycle_overlay = true;
        }
        applied.push(AppliedFrameworkMigration {
            table_name: table.clone(),
            column_name: col.name.to_string(),
            ddl_applied: ddl,
            binary_version: binary_version.clone(),
            applied_at: applied_at.clone(),
        });
    }

    // Codex T051-r1 HIGH: apply DDL + audit insert MUST be atomic. SQLite
    // supports ALTER TABLE inside transactions; rolling back releases the
    // schema change cleanly. If we ever split these into separate
    // transactions, a partial-failure leaves the DB migrated-but-unaudited
    // and the next boot sees no missing columns, never recording the audit.
    let tx = conn.unchecked_transaction()?;
    {
        for m in &applied {
            tx.execute_batch(&m.ddl_applied)
                .with_context(|| format!("failed ALTER for {}.{}", m.table_name, m.column_name))?;
        }
        let mut stmt = tx.prepare(
            "INSERT INTO substrate_migrations \
             (applied_at, binary_version, table_name, column_name, ddl_applied) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for m in &applied {
            stmt.execute(rusqlite::params![
                m.applied_at,
                m.binary_version,
                m.table_name,
                m.column_name,
                m.ddl_applied,
            ])?;
        }
        // T140 P1: when the activation column was just added, walk the
        // IN_FLIGHT_STATES backfill so currently-running work stays armed.
        // Every other row already received `activation='inactive'` via the
        // DDL DEFAULT applied by ALTER. Idempotent on a second boot because
        // the column will not be in `drift.additive` once present.
        if backfill_tasks_activation {
            let placeholders = std::iter::repeat_n("?", IN_FLIGHT_STATES.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("UPDATE tasks SET activation='active' WHERE status IN ({placeholders})");
            let params: Vec<&dyn rusqlite::ToSql> = IN_FLIGHT_STATES
                .iter()
                .map(|s| s as &dyn rusqlite::ToSql)
                .collect();
            tx.execute(&sql, params.as_slice())
                .context("T140 P1: backfill tasks.activation for IN_FLIGHT_STATES")?;
        }
        if backfill_tasks_lifecycle_overlay {
            let live_cols = {
                let mut pragma = tx
                    .prepare("PRAGMA table_info(tasks)")
                    .context("T144 P1: inspect tasks columns for lifecycle overlay backfill")?;
                let rows = pragma.query_map([], |row| row.get::<_, String>(1))?;
                let mut cols = Vec::new();
                for row in rows {
                    cols.push(row?);
                }
                cols
            };
            let blocked_expr = if live_cols.iter().any(|c| c == "blocked_reason") {
                "blocked_reason"
            } else {
                "NULL AS blocked_reason"
            };
            let integration_expr = if live_cols.iter().any(|c| c == "integration_blocked_reason") {
                "integration_blocked_reason"
            } else {
                "NULL AS integration_blocked_reason"
            };
            let select_sql = format!(
                "SELECT id, display_id, status, {blocked_expr}, {integration_expr} FROM tasks"
            );
            let mut stmt = tx
                .prepare(&select_sql)
                .context("T144 P1: select tasks for lifecycle overlay backfill")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .context("T144 P1: query tasks for lifecycle overlay backfill")?;
            let mut backfills = Vec::new();
            for row in rows {
                backfills.push(row?);
            }
            drop(stmt);
            for (id, display_id, status, blocked_reason, integration_blocked_reason) in backfills {
                let overlay = if IN_FLIGHT_STATES.contains(&status.as_str()) {
                    let (lifecycle, active_step, integration_step) = match status.as_str() {
                        "executing" => ("active", "coding", "none"),
                        "code_review" => ("active", "coding_review", "none"),
                        "integrating" => ("integration", "none", "merging"),
                        _ => ("active", "none", "none"),
                    };
                    crate::handlers::lifecycle_overlay::LifecycleOverlay {
                        lifecycle: lifecycle.to_string(),
                        active_step: active_step.to_string(),
                        integration_step: integration_step.to_string(),
                        blocked: false,
                        blocker_kind: None,
                        legacy_status: Some(status.clone()),
                    }
                } else {
                    crate::handlers::lifecycle_overlay::derive(
                        "backfill_queued",
                        "",
                        &status,
                        blocked_reason.as_deref(),
                        integration_blocked_reason.as_deref(),
                    )
                    .with_context(|| {
                        format!("T144 P1: derive lifecycle overlay backfill for {display_id}")
                    })?
                };
                let repo_specific_post_integration = matches!(
                    status.as_str(),
                    "cargo_installed" | "schema_migrated" | "deploy_blocked"
                );
                let post_integration_step = match status.as_str() {
                    "cargo_installed" => "cargo_installed",
                    "schema_migrated" => "schema_migrated",
                    "deploy_blocked" => "deploy_blocked",
                    _ => "none",
                };
                let migrated_status = if repo_specific_post_integration {
                    "integrated"
                } else {
                    status.as_str()
                };
                let lifecycle = if repo_specific_post_integration {
                    "done".to_string()
                } else {
                    overlay.lifecycle
                };
                let integration_step = if repo_specific_post_integration {
                    "none".to_string()
                } else {
                    overlay.integration_step
                };
                tx.execute(
                    "UPDATE tasks SET status=?1, lifecycle=?2, active_step=?3, integration_step=?4, blocked=?5, blocker_kind=?6, post_integration_step=?7 WHERE id=?8",
                    rusqlite::params![
                        migrated_status,
                        lifecycle,
                        overlay.active_step,
                        integration_step,
                        if overlay.blocked { 1 } else { 0 },
                        overlay.blocker_kind,
                        post_integration_step,
                        id
                    ],
                )
                .with_context(|| {
                    format!("T144 P1: update lifecycle overlay backfill for {display_id}")
                })?;
            }
        }
    }
    tx.commit()
        .context("failed to commit atomic framework-DDL apply + audit")?;

    ensure_integration_singleton_index(conn)?;
    ensure_adr0002_upstream_columns(conn)?;
    backfill_adr0002_upstream(conn)?;
    Ok(applied)
}

pub fn ensure_adr0002_upstream_columns(conn: &Connection) -> Result<()> {
    let specs: &[(&str, &[(&str, &str)])] = &[
        (
            "intake",
            &[
                ("lifecycle", "lifecycle TEXT DEFAULT 'new'"),
                ("waiting_kind", "waiting_kind TEXT"),
                ("outcome", "outcome TEXT"),
                ("duplicate_of_id", "duplicate_of_id TEXT"),
                ("produced_observation_id", "produced_observation_id TEXT"),
                (
                    "produced_architecture_review_id",
                    "produced_architecture_review_id TEXT",
                ),
                ("produced_task_id", "produced_task_id TEXT"),
                ("produced_artifact_kind", "produced_artifact_kind TEXT"),
                ("produced_artifact_id", "produced_artifact_id TEXT"),
            ],
        ),
        (
            "observations",
            &[
                ("lifecycle", "lifecycle TEXT DEFAULT 'candidate'"),
                ("contract_state", "contract_state TEXT"),
                ("waiting", "waiting INTEGER"),
                ("waiting_kind", "waiting_kind TEXT"),
                ("outcome", "outcome TEXT"),
                (
                    "open_architecture_review_id",
                    "open_architecture_review_id TEXT",
                ),
                ("addressed_by_task_id", "addressed_by_task_id TEXT"),
                ("addressed_by_commit_sha", "addressed_by_commit_sha TEXT"),
                ("superseded_by_id", "superseded_by_id TEXT"),
            ],
        ),
        (
            "architecture_reviews",
            &[
                ("lifecycle", "lifecycle TEXT DEFAULT 'pending'"),
                ("outcome", "outcome TEXT"),
                (
                    "linked_observation_ids",
                    "linked_observation_ids TEXT DEFAULT '[]'",
                ),
                ("produced_task_id", "produced_task_id TEXT"),
                ("superseded_by_id", "superseded_by_id TEXT"),
            ],
        ),
    ];
    for (table, cols) in specs {
        if !table_exists(conn, table)? {
            continue;
        }
        let live = read_table_info(conn, table)?;
        for (name, ddl) in *cols {
            if !live.iter().any(|c| c == name) {
                conn.execute_batch(&format!(
                    "ALTER TABLE {} ADD COLUMN {};",
                    quote_ident(table),
                    ddl
                ))
                .with_context(|| format!("T148 P3: add ADR0002 column {table}.{name}"))?;
            }
        }
    }
    Ok(())
}

pub fn backfill_adr0002_upstream(conn: &Connection) -> Result<()> {
    backfill_intake_adr0002(conn)?;
    backfill_observations_adr0002(conn)?;
    backfill_arch_reviews_adr0002(conn)?;
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0)
}

fn has_cols(conn: &Connection, table: &str, cols: &[&str]) -> Result<bool> {
    if !table_exists(conn, table)? {
        return Ok(false);
    }
    let live = read_table_info(conn, table)?;
    Ok(cols.iter().all(|c| live.iter().any(|l| l == c)))
}

fn opt_col<'a>(live: &[String], name: &'a str) -> &'a str {
    if live.iter().any(|c| c == name) {
        name
    } else {
        "NULL"
    }
}

fn catch_projection<T>(
    display_id: &str,
    status: &str,
    f: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> T {
    std::panic::catch_unwind(f).unwrap_or_else(|_| {
        panic!("ADR0002 backfill failed for {display_id}: unmapped status {status}")
    })
}

fn backfill_intake_adr0002(conn: &Connection) -> Result<()> {
    if !has_cols(
        conn,
        "intake",
        &[
            "display_id",
            "status",
            "lifecycle",
            "waiting_kind",
            "outcome",
        ],
    )? {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    let live = read_table_info(&tx, "intake")?;
    let select_sql = format!(
        "SELECT display_id,status,{decision},{rto},{rta},{pt},{pak},{pai},{dup} FROM intake",
        decision = opt_col(&live, "decision"),
        rto = opt_col(&live, "routed_to_observation"),
        rta = opt_col(&live, "routed_to_arch_review"),
        pt = opt_col(&live, "produced_task_id"),
        pak = opt_col(&live, "produced_artifact_kind"),
        pai = opt_col(&live, "produced_artifact_id"),
        dup = opt_col(&live, "duplicate_of"),
    );
    let rows = {
        let mut stmt = tx.prepare(&select_sql)?;
        let out = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out
    };
    for (display_id, status, decision, rto, rta, pt, pak, pai, dup) in rows {
        let mut produced_task_id = pt;
        if produced_task_id.is_none() && decision.as_deref() == Some("fast_track") {
            produced_task_id = find_fast_track_task(&tx, &display_id)?;
        }
        let (artifact_kind, artifact_id) = if let Some(v) = rto.as_ref() {
            (Some("observation".to_string()), Some(v.clone()))
        } else if let Some(v) = rta.as_ref() {
            (Some("architecture_review".to_string()), Some(v.clone()))
        } else if let Some(v) = produced_task_id.as_ref() {
            (Some("task".to_string()), Some(v.clone()))
        } else {
            (pak, pai)
        };
        let input = crate::flow::adr0002_projection::IntakeRowInput {
            display_id: &display_id,
            status: &status,
            decision: decision.as_deref(),
            routed_to_observation: rto.as_deref(),
            routed_to_arch_review: rta.as_deref(),
            produced_task_id: produced_task_id.as_deref(),
            produced_artifact_kind: artifact_kind.as_deref(),
            produced_artifact_id: artifact_id.as_deref(),
            duplicate_of: dup.as_deref(),
        };
        let p = catch_projection(&display_id, &status, || {
            crate::flow::adr0002_projection::project_intake(&input)
        });
        tx.execute(
            "UPDATE intake SET lifecycle=?1, waiting_kind=?2, outcome=?3, produced_observation_id=?4, produced_architecture_review_id=?5, produced_task_id=?6, produced_artifact_kind=?7, produced_artifact_id=?8, duplicate_of_id=?9 WHERE display_id=?10",
            rusqlite::params![p.lifecycle.as_str(), p.waiting.map(|w| w.as_str()), p.outcome.map(|o| o.as_str()), p.references.produced_observation_id, p.references.produced_architecture_review_id, p.references.produced_task_id, p.references.produced_artifact_kind, p.references.produced_artifact_id, p.references.duplicate_of_id, display_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn find_fast_track_task(conn: &Connection, intake_id: &str) -> Result<Option<String>> {
    if !table_exists(conn, "tasks")? {
        return Ok(None);
    }
    let live = read_table_info(conn, "tasks")?;
    for col in ["source_intake", "source_intake_id", "source_item", "source"] {
        if live.iter().any(|c| c == col) {
            let sql = format!(
                "SELECT display_id FROM tasks WHERE {}=?1 ORDER BY id LIMIT 1",
                quote_ident(col)
            );
            if let Some(v) = conn.query_row(&sql, [intake_id], |r| r.get(0)).optional()? {
                return Ok(Some(v));
            }
        }
    }
    Ok(None)
}

fn backfill_observations_adr0002(conn: &Connection) -> Result<()> {
    if !has_cols(
        conn,
        "observations",
        &[
            "display_id",
            "status",
            "lifecycle",
            "contract_state",
            "waiting",
            "waiting_kind",
            "outcome",
        ],
    )? {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    let live = read_table_info(&tx, "observations")?;
    let select_sql = format!(
        "SELECT display_id,status,{ic},{pending},{clearable},{open},{rk},{res},{merge},{resolved_by},{task},{sha},{super_by} FROM observations",
        ic=opt_col(&live,"intent_contract"), pending=opt_col(&live,"pending_architecture_review"), clearable=opt_col(&live,"clearable_by_ruling"), open=opt_col(&live,"open_architecture_review_id"), rk=opt_col(&live,"resolution_kind"), res=opt_col(&live,"resolution"), merge=opt_col(&live,"merge_target_id"), resolved_by=opt_col(&live,"resolved_by"), task=opt_col(&live,"task_id"), sha=opt_col(&live,"addressed_by_commit_sha"), super_by=opt_col(&live,"superseded_by_id")
    );
    let rows = {
        let mut stmt = tx.prepare(&select_sql)?;
        let out = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out
    };
    for (
        display_id,
        status,
        ic,
        pending,
        clearable,
        open,
        rk,
        res,
        merge,
        resolved_by,
        task,
        sha,
        super_by,
    ) in rows
    {
        let contract = contract_state_from_intent(ic.as_deref());
        let open_arch = open.or_else(|| find_open_arch_review(&tx, &display_id).ok().flatten());
        let mut superseded_by = super_by;
        if superseded_by.is_none() && rk.as_deref() == Some("superseded") {
            superseded_by = resolved_by.clone().or_else(|| res.clone());
        }
        let input = crate::flow::adr0002_projection::ObsRowInput {
            display_id: &display_id,
            status: &status,
            contract_state: contract.as_deref(),
            pending_architecture_review: pending.map(|v| v != 0),
            clearable_by_ruling: clearable.as_deref(),
            open_architecture_review_id: open_arch.as_deref(),
            resolution_kind: rk.as_deref(),
            resolution: res.as_deref(),
            merge_target_id: merge.as_deref(),
            resolved_by: resolved_by.as_deref(),
            task_id: task.as_deref(),
            addressed_by_commit_sha: sha.as_deref(),
            superseded_by_id: superseded_by.as_deref(),
        };
        let p = catch_projection(&display_id, &status, || {
            crate::flow::adr0002_projection::project_observation(&input, None)
        });
        tx.execute(
            "UPDATE observations SET lifecycle=?1, contract_state=?2, waiting=?3, waiting_kind=?4, outcome=?5, open_architecture_review_id=?6, addressed_by_task_id=?7, addressed_by_commit_sha=?8, superseded_by_id=?9 WHERE display_id=?10",
            rusqlite::params![p.lifecycle.as_str(), p.contract_state.as_str(), if p.waiting.is_some(){1}else{0}, p.waiting.map(|w| w.as_str()), p.outcome.map(|o| o.as_str()), p.references.open_architecture_review_id, p.references.addressed_by_task_id, p.references.addressed_by_commit_sha, p.references.superseded_by_id, display_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn contract_state_from_intent(intent_contract: Option<&str>) -> Option<String> {
    let raw = intent_contract
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| {
            v.get("contract_state")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
    Some(
        match raw.as_deref() {
            Some("draft") => "draft",
            Some("ready") | Some("approved") => "approved",
            _ => "none",
        }
        .to_string(),
    )
}

fn find_open_arch_review(conn: &Connection, obs_id: &str) -> Result<Option<String>> {
    if !has_cols(
        conn,
        "architecture_reviews",
        &["display_id", "status", "source_observation"],
    )? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT display_id FROM architecture_reviews WHERE source_observation=?1 AND status NOT IN ('verdict_issued','withdrawn','superseded') ORDER BY id LIMIT 1",
        [obs_id], |r| r.get(0)
    ).optional().map_err(Into::into)
}

fn backfill_arch_reviews_adr0002(conn: &Connection) -> Result<()> {
    if !has_cols(
        conn,
        "architecture_reviews",
        &[
            "display_id",
            "status",
            "lifecycle",
            "outcome",
            "linked_observation_ids",
        ],
    )? {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    let live = read_table_info(&tx, "architecture_reviews")?;
    let select_sql = format!(
        "SELECT display_id,status,{verdict},{source_obs},{source_intake},{linked},{supersedes},{merge},{pt},{super_by},{updated},{cascade} FROM architecture_reviews",
        verdict=opt_col(&live,"verdict"), source_obs=opt_col(&live,"source_observation"), source_intake=opt_col(&live,"source_intake"), linked=opt_col(&live,"linked_observation_ids"), supersedes=opt_col(&live,"supersedes"), merge=opt_col(&live,"merge_target_id"), pt=opt_col(&live,"produced_task_id"), super_by=opt_col(&live,"superseded_by_id"), updated=opt_col(&live,"updated_at"), cascade=opt_col(&live,"cascade_decisions")
    );
    let rows = {
        let mut stmt = tx.prepare(&select_sql)?;
        let out = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out
    };
    for (
        display_id,
        status,
        verdict,
        source_obs,
        source_intake,
        linked_json,
        supersedes,
        merge,
        pt,
        super_by,
        updated,
        cascade,
    ) in rows
    {
        let mut linked = parse_text_list(linked_json.as_deref());
        if linked.is_empty() {
            if let Some(s) = source_obs.as_deref() {
                linked.push(s.to_string());
            }
        }
        let produced_task_id = pt.or_else(|| task_from_cascade(cascade.as_deref()));
        let linked_refs: Vec<&str> = linked.iter().map(String::as_str).collect();
        let input = crate::flow::adr0002_projection::ArchReviewRowInput {
            display_id: &display_id,
            status: &status,
            verdict: verdict.as_deref(),
            source_observation: source_obs.as_deref(),
            source_intake: source_intake.as_deref(),
            linked_observation_ids: linked_refs,
            supersedes: supersedes.as_deref(),
            merge_target_id: merge.as_deref(),
            produced_task_id: produced_task_id.as_deref(),
            superseded_by_id: super_by.as_deref(),
            updated_at: updated.as_deref(),
        };
        let p = catch_projection(&display_id, &status, || {
            crate::flow::adr0002_projection::project_arch_review(&input)
        });
        let linked_out = serde_json::to_string(&p.references.linked_observation_ids)?;
        tx.execute(
            "UPDATE architecture_reviews SET lifecycle=?1, outcome=?2, linked_observation_ids=?3, produced_task_id=?4, superseded_by_id=?5 WHERE display_id=?6",
            rusqlite::params![p.lifecycle.as_str(), p.outcome.map(|o| o.as_str()), linked_out, p.references.produced_task_id, p.references.superseded_by_id, display_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn parse_text_list(raw: Option<&str>) -> Vec<String> {
    match raw.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        Some(Value::Array(a)) => a
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s],
        _ => Vec::new(),
    }
}

fn task_from_cascade(raw: Option<&str>) -> Option<String> {
    let Value::Array(items) = serde_json::from_str::<Value>(raw?).ok()? else {
        return None;
    };
    for item in items {
        if item.get("decision").and_then(|v| v.as_str()) == Some("create_followup") {
            if let Some(t) = item
                .get("target")
                .and_then(|v| v.as_str())
                .filter(|s| s.starts_with('T'))
            {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn read_table_info(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::schema::Schema;
    use rusqlite::OptionalExtension;

    fn pre_t099_observations_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        for col in ["summary_signature", "dupe_count", "last_seen"] {
            conn.execute_batch(&format!(
                "ALTER TABLE \"observations\" DROP COLUMN \"{col}\";"
            ))
            .unwrap();
        }
        conn
    }

    #[test]
    fn t102_framework_drift_emits_observations_dedup_alters() {
        let conn = pre_t099_observations_conn();
        let drift = compute_framework_drift(&conn).unwrap();
        let got: Vec<(&str, &str, &str, bool)> = drift
            .additive
            .iter()
            .filter(|(table, _)| table == "observations")
            .map(|(_, col)| (col.name, col.sql_type, col.full_def, col.additive))
            .collect();
        assert_eq!(
            got,
            vec![
                ("summary_signature", "TEXT", "summary_signature TEXT", true),
                (
                    "dupe_count",
                    "INTEGER",
                    "dupe_count INTEGER DEFAULT 1",
                    true
                ),
                ("last_seen", "TEXT", "last_seen TEXT", true),
            ]
        );
    }

    // ---- T138 P1: integration-lane capacity-1 partial UNIQUE index --------

    /// Build a fresh in-memory DB containing the substrate tables AND the
    /// bundled tasks-store table (status column included). Used by the index
    /// tests below; mirrors `ddl_for(tasks_schema)` against
    /// `Connection::open_in_memory`.
    fn fresh_db_with_tasks() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let schema = Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    /// AC1.5: framework_migrate handler creates the partial UNIQUE index
    /// `idx_tasks_integration_singleton` on tasks(status,integration_step)
    /// WHERE status='integrating' AND integration_step='merging'. Verified
    /// via sqlite_master introspection.
    #[test]
    fn t138_p1_integration_singleton_index_created() {
        let conn = fresh_db_with_tasks();
        // Mirror the boot-time apply: drift + index ensure runs on every open.
        apply_framework_drift(&conn).unwrap();

        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type='index' AND name='idx_tasks_integration_singleton'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        let sql = sql.expect("idx_tasks_integration_singleton must exist");
        let upper = sql.to_ascii_uppercase();
        assert!(upper.contains("UNIQUE"), "index must be UNIQUE: {sql}");
        assert!(
            upper.contains("STATUS='INTEGRATING'") || upper.contains("STATUS = 'INTEGRATING'"),
            "index must filter on status='integrating': {sql}"
        );
        assert!(
            upper.contains("INTEGRATION_STEP='MERGING'")
                || upper.contains("INTEGRATION_STEP = 'MERGING'"),
            "index must filter on integration_step='merging': {sql}"
        );
    }

    /// ADR0001 P3: two rows may be `status='integrating'` in non-merging
    /// substeps, but only one row may hold `integration_step='merging'`.
    #[test]
    fn t138_p1_integration_singleton_index_enforces_capacity_one() {
        let conn = fresh_db_with_tasks();
        apply_framework_drift(&conn).unwrap();

        // Seed two rows in integration_queued.
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, created_at, updated_at, created_by, updated_by) \
             VALUES ('T801', 'integration_queued', 't1', 't1', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, created_at, updated_at, created_by, updated_by) \
             VALUES ('T802', 'integration_queued', 't2', 't2', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework')",
            [],
        ).unwrap();

        conn.execute(
            "UPDATE tasks SET status='integrating', integration_step='task_review' WHERE display_id='T801'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET status='integrating', integration_step='testing' WHERE display_id='T802'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET integration_step='merging' WHERE display_id='T801'",
            [],
        )
        .unwrap();

        // Second UPDATE to merging must fail with ConstraintViolation.
        let r2 = conn.execute(
            "UPDATE tasks SET integration_step='merging' WHERE display_id='T802'",
            [],
        );
        let err = r2.expect_err("second UPDATE to merging must fail");
        let msg = format!("{err}");
        assert!(
            msg.to_ascii_uppercase().contains("UNIQUE")
                || msg.to_ascii_uppercase().contains("CONSTRAINT"),
            "error must surface UNIQUE/constraint violation: {msg}"
        );

        // Sanity: both rows are integrating, but only T801 is merging.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status='integrating'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "non-merging substeps may run in parallel");
        let merging: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status='integrating' AND integration_step='merging'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(merging, 1, "exactly one row may be merging");
    }

    /// `ensure_integration_singleton_index` is idempotent — repeated calls do
    /// not error and do not produce duplicate index entries.
    #[test]
    fn t138_p1_ensure_integration_singleton_index_is_idempotent() {
        let conn = fresh_db_with_tasks();
        ensure_integration_singleton_index(&conn).unwrap();
        ensure_integration_singleton_index(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_tasks_integration_singleton'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 1,
            "exactly one index named idx_tasks_integration_singleton"
        );
    }

    #[test]
    fn queued_lifecycle_backfill() {
        let conn = fresh_db_with_tasks();
        for col in [
            "lifecycle",
            "active_step",
            "integration_step",
            "blocked",
            "blocker_kind",
        ] {
            conn.execute_batch(&format!("ALTER TABLE tasks DROP COLUMN {col};"))
                .unwrap();
        }
        let cases = [
            ("T901", "planning", "queued"),
            ("T902", "ready", "queued"),
            ("T903", "blocked", "queued"),
            ("T904", "complete", "queued"),
            ("T905", "in_review", "queued"),
            ("T906", "integrated", "queued"),
            ("T907", "executing", "active"),
            ("T908", "code_review", "active"),
            ("T909", "integrating", "integration"),
        ];
        for (display_id, status, _) in cases {
            conn.execute(
                "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES (?1,?2,?3,?3,'n','n','framework','framework')",
                rusqlite::params![display_id, status, status],
            ).unwrap();
        }
        apply_framework_drift(&conn).unwrap();
        for (display_id, status, expected_lifecycle) in cases {
            let got: (String, String, String) = conn
                .query_row(
                    "SELECT lifecycle, active_step, integration_step FROM tasks WHERE display_id=?1",
                    [display_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(got.0, expected_lifecycle, "{status}");
            if expected_lifecycle == "queued" {
                assert_eq!(got.1, "none", "active_step for {status}");
                assert_eq!(got.2, "none", "integration_step for {status}");
            }
            if status == "integrating" {
                assert_eq!(got.1, "none", "active_step for {status}");
                assert_eq!(got.2, "merging", "integration_step for {status}");
            }
        }
    }

    #[test]
    fn adr0002_backfill_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE substrate_migrations (applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT);\
             CREATE TABLE intake (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, decision TEXT, routed_to_observation TEXT);\
             CREATE TABLE observations (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, intent_contract TEXT);\
             CREATE TABLE architecture_reviews (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, verdict TEXT, source_observation TEXT);",
        )
        .unwrap();
        conn.execute("INSERT INTO intake (display_id,status,decision,routed_to_observation) VALUES ('I001','routed','normal_observation','L001')", []).unwrap();
        conn.execute("INSERT INTO observations (display_id,status,intent_contract) VALUES ('L001','open','{\"contract_state\":\"ready\"}')", []).unwrap();
        conn.execute("INSERT INTO architecture_reviews (display_id,status,source_observation) VALUES ('A001','pending','L001')", []).unwrap();
        apply_framework_drift(&conn).unwrap();
        let before: (String, String, String) = conn.query_row(
            "SELECT (SELECT lifecycle FROM intake WHERE display_id='I001'), (SELECT contract_state FROM observations WHERE display_id='L001'), (SELECT linked_observation_ids FROM architecture_reviews WHERE display_id='A001')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        apply_framework_drift(&conn).unwrap();
        let after: (String, String, String) = conn.query_row(
            "SELECT (SELECT lifecycle FROM intake WHERE display_id='I001'), (SELECT contract_state FROM observations WHERE display_id='L001'), (SELECT linked_observation_ids FROM architecture_reviews WHERE display_id='A001')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn adr0002_backfill_matches_projection() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE substrate_migrations (applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT); CREATE TABLE intake (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, decision TEXT, routed_to_observation TEXT);").unwrap();
        conn.execute("INSERT INTO intake (display_id,status,decision,routed_to_observation) VALUES ('I002','routed','normal_observation','L002')", []).unwrap();
        apply_framework_drift(&conn).unwrap();
        let got: (String, Option<String>) = conn
            .query_row(
                "SELECT lifecycle,produced_observation_id FROM intake WHERE display_id='I002'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(got, ("closed".into(), Some("L002".into())));
    }

    #[test]
    fn t102_framework_apply_lands_observations_dedup_columns_and_query_works() {
        let conn = pre_t099_observations_conn();
        let applied = apply_framework_drift(&conn).unwrap();
        let applied_cols: Vec<&str> = applied
            .iter()
            .filter(|m| m.table_name == "observations")
            .map(|m| m.column_name.as_str())
            .collect();
        assert_eq!(
            applied_cols,
            vec!["summary_signature", "dupe_count", "last_seen"]
        );

        conn.query_row(
            "SELECT id FROM observations WHERE summary_signature = ?1",
            rusqlite::params!["deploy-blocked: merge conflict"],
            |_| Ok(()),
        )
        .optional()
        .expect("dedup SELECT must not error after framework drift apply");
    }
}

#[cfg(test)]
mod adr0002_backfill_idempotent {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn passes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE substrate_migrations (applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT);\
             CREATE TABLE intake (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, decision TEXT, routed_to_observation TEXT);",
        ).unwrap();
        conn.execute("INSERT INTO intake (display_id,status,decision,routed_to_observation) VALUES ('I901','routed','normal_observation','L901')", []).unwrap();
        apply_framework_drift(&conn).unwrap();
        let before: (String, Option<String>) = conn
            .query_row(
                "SELECT lifecycle,produced_observation_id FROM intake WHERE display_id='I901'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        apply_framework_drift(&conn).unwrap();
        let after: (String, Option<String>) = conn
            .query_row(
                "SELECT lifecycle,produced_observation_id FROM intake WHERE display_id='I901'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(before, after);
    }
}

#[cfg(test)]
mod adr0002_backfill_matches_projection {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    #[test]
    fn passes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE substrate_migrations (applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT);\
             CREATE TABLE intake (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, decision TEXT, routed_to_observation TEXT, routed_to_arch_review TEXT, duplicate_of TEXT);\
             CREATE TABLE observations (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, intent_contract TEXT, pending_architecture_review INTEGER, resolution_kind TEXT, resolution TEXT, merge_target_id TEXT, resolved_by TEXT, task_id TEXT);\
             CREATE TABLE architecture_reviews (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, verdict TEXT, source_observation TEXT, cascade_decisions TEXT, updated_at TEXT);",
        ).unwrap();
        let intake = [
            ("I901", "draft", None, None, None, None),
            ("I902", "triaging", None, None, None, None),
            ("I903", "needs_info", Some("needs_info"), None, None, None),
            (
                "I904",
                "routed",
                Some("duplicate"),
                None,
                None,
                Some("I901"),
            ),
            ("I905", "routed", Some("fast_track"), None, None, None),
            (
                "I906",
                "routed",
                Some("normal_observation"),
                Some("L906"),
                None,
                None,
            ),
            (
                "I907",
                "routed",
                Some("arch_review_candidate"),
                Some("L907"),
                Some("A907"),
                None,
            ),
            ("I908", "dropped", Some("reject_noise"), None, None, None),
        ];
        for (id, status, decision, obs, arch, dup) in intake {
            conn.execute("INSERT INTO intake (display_id,status,decision,routed_to_observation,routed_to_arch_review,duplicate_of) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![id,status,decision,obs,arch,dup]).unwrap();
        }
        let observations = [
            (
                "L901",
                "open",
                Some("draft"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L902",
                "needs_investigation",
                Some("ready"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L903",
                "investigating",
                None,
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L904",
                "investigated",
                Some("ready"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L905",
                "investigation_failed",
                Some("draft"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L906",
                "confirmed",
                Some("ready"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L907",
                "ready",
                Some("ready"),
                0,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L908",
                "needs_info",
                Some("draft"),
                1,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "L909",
                "in_progress",
                Some("ready"),
                0,
                None,
                None,
                None,
                None,
                Some("T909"),
            ),
            (
                "L910",
                "resolved",
                Some("ready"),
                0,
                Some("addressed_by_task"),
                Some("T910"),
                None,
                None,
                None,
            ),
            (
                "L911",
                "resolved",
                Some("ready"),
                0,
                Some("addressed_by_commit"),
                Some("abc123"),
                None,
                None,
                None,
            ),
            (
                "L912",
                "resolved",
                Some("ready"),
                0,
                Some("superseded"),
                Some("L999"),
                None,
                None,
                None,
            ),
        ];
        for (id, status, contract, pending, rk, res, merge, resolved_by, task) in observations {
            let ic = contract.map(|c| json!({"contract_state": c}).to_string());
            conn.execute("INSERT INTO observations (display_id,status,intent_contract,pending_architecture_review,resolution_kind,resolution,merge_target_id,resolved_by,task_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", rusqlite::params![id,status,ic,pending,rk,res,merge,resolved_by,task]).unwrap();
        }
        let arch = [
            ("A901", "pending", None, Some("L908"), None),
            ("A902", "in_review", None, Some("L902"), None),
            (
                "A903",
                "awaiting_human_ratification",
                None,
                Some("L903"),
                None,
            ),
            (
                "A904",
                "verdict_issued",
                Some("allow_local_fix"),
                Some("L904"),
                None,
            ),
            (
                "A905",
                "verdict_issued",
                Some("create_primitive_task"),
                Some("L905"),
                Some("T905"),
            ),
            ("A906", "withdrawn", None, Some("L906"), None),
            ("A907", "superseded", None, Some("L907"), None),
        ];
        for (id, status, verdict, source, task) in arch {
            let cascade =
                task.map(|t| json!([{"decision":"create_followup","target":t}]).to_string());
            conn.execute("INSERT INTO architecture_reviews (display_id,status,verdict,source_observation,cascade_decisions,updated_at) VALUES (?1,?2,?3,?4,?5,'now')", rusqlite::params![id,status,verdict,source,cascade]).unwrap();
        }

        apply_framework_drift(&conn).unwrap();
        let mut checked = 0;
        for (id, status, decision, obs, arch, dup) in intake {
            let (kind, artifact) = if let Some(v) = obs {
                (Some("observation"), Some(v))
            } else if let Some(v) = arch {
                (Some("architecture_review"), Some(v))
            } else {
                (None, None)
            };
            let p = crate::flow::adr0002_projection::project_intake(
                &crate::flow::adr0002_projection::IntakeRowInput {
                    display_id: id,
                    status,
                    decision,
                    routed_to_observation: obs,
                    routed_to_arch_review: arch,
                    produced_task_id: None,
                    produced_artifact_kind: kind,
                    produced_artifact_id: artifact,
                    duplicate_of: dup,
                },
            );
            let got: (String, Option<String>, Option<String>) = conn.query_row("SELECT lifecycle,outcome,produced_observation_id FROM intake WHERE display_id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            assert_eq!(
                got,
                (
                    p.lifecycle.as_str().into(),
                    p.outcome.map(|o| o.as_str().into()),
                    p.references.produced_observation_id
                ),
                "{id}"
            );
            checked += 1;
        }
        for (id, status, contract, pending, rk, res, merge, resolved_by, task) in observations {
            let contract_state = match contract {
                Some("draft") => Some("draft"),
                Some("ready") => Some("approved"),
                _ => Some("none"),
            };
            let open = match id {
                "L902" => Some("A902"),
                "L903" => Some("A903"),
                "L908" => Some("A901"),
                _ => None,
            };
            let superseded = if rk == Some("superseded") { res } else { None };
            let p = crate::flow::adr0002_projection::project_observation(
                &crate::flow::adr0002_projection::ObsRowInput {
                    display_id: id,
                    status,
                    contract_state,
                    pending_architecture_review: Some(pending != 0),
                    clearable_by_ruling: None,
                    open_architecture_review_id: open,
                    resolution_kind: rk,
                    resolution: res,
                    merge_target_id: merge,
                    resolved_by,
                    task_id: task,
                    addressed_by_commit_sha: None,
                    superseded_by_id: superseded,
                },
                None,
            );
            let got: (String, String, Option<String>) = conn
                .query_row(
                    "SELECT lifecycle,contract_state,outcome FROM observations WHERE display_id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                got,
                (
                    p.lifecycle.as_str().into(),
                    p.contract_state.as_str().into(),
                    p.outcome.map(|o| o.as_str().into())
                ),
                "{id}"
            );
            checked += 1;
        }
        for (id, status, verdict, source, task) in arch {
            let linked = source.map(|s| vec![s]).unwrap_or_default();
            let p = crate::flow::adr0002_projection::project_arch_review(
                &crate::flow::adr0002_projection::ArchReviewRowInput {
                    display_id: id,
                    status,
                    verdict,
                    source_observation: source,
                    source_intake: None,
                    linked_observation_ids: linked,
                    supersedes: None,
                    merge_target_id: None,
                    produced_task_id: task,
                    superseded_by_id: None,
                    updated_at: Some("now"),
                },
            );
            let got: (String, Option<String>, Option<String>) = conn.query_row("SELECT lifecycle,outcome,produced_task_id FROM architecture_reviews WHERE display_id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            assert_eq!(
                got,
                (
                    p.lifecycle.as_str().into(),
                    p.outcome.map(|o| o.as_str().into()),
                    p.references.produced_task_id
                ),
                "{id}"
            );
            checked += 1;
        }
        assert!(
            checked >= 20,
            "checked {checked} rows against projection oracle"
        );
    }
}
