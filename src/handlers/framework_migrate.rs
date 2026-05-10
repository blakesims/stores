//! Framework-DDL drift detection + auto-apply (T051).
//!
//! Older DBs predate columns added to SUBSTRATE_DDL by newer binaries. The
//! `CREATE TABLE IF NOT EXISTS` re-execution in `db::open` is a no-op on
//! existing tables, so missing columns are not applied. This module
//! introspects each framework table via `PRAGMA table_info` and ALTERs in
//! whatever columns are missing, recording each application in
//! `substrate_migrations` for audit.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

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

/// Name of the partial UNIQUE index that enforces the integration-lane
/// capacity-1 invariant on the `tasks` table. T138 P1.
///
/// At most one row substrate-wide can hold `status='integrating'`. Concurrent
/// `start-integration` attempts surface as a SQLite UNIQUE ConstraintViolation
/// at the schema layer, which the integrate builtin treats as capacity-busy
/// (returns Ok(0)) rather than as a runtime error.
pub const INTEGRATION_SINGLETON_INDEX: &str = "idx_tasks_integration_singleton";

/// SQL for the partial UNIQUE index. Index value `1` is constant for every
/// row that matches the WHERE predicate, so the UNIQUE constraint reduces to
/// "at most one row may match". Idempotent (`IF NOT EXISTS`); safe to run on
/// every boot. T138 P1.
pub const INTEGRATION_SINGLETON_INDEX_DDL: &str = concat!(
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_integration_singleton ",
    "ON tasks((1)) WHERE status='integrating'"
);

/// Create the integration-lane capacity-1 partial UNIQUE index on `tasks` if
/// the `tasks` table exists. The `tasks` store is installed separately from
/// the substrate tables, so we guard the CREATE INDEX with a table-existence
/// check (mirrors `ensure_runs_view_if_tasks_exists`). Idempotent. T138 P1.
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
    // T138 P1: ensure the integration-lane capacity-1 partial UNIQUE index on
    // `tasks` exists before the column drift loop runs. Independent of the
    // additive-column flow (this is a CREATE INDEX, not an ALTER), idempotent,
    // and a no-op when the `tasks` table is absent.
    ensure_integration_singleton_index(conn)?;

    let drift = compute_framework_drift(conn)?;
    if drift.additive.is_empty() {
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
                "lifecycle" | "active_step" | "integration_step" | "blocked" | "blocker_kind"
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
                let backfill_verb = if IN_FLIGHT_STATES.contains(&status.as_str()) {
                    "backfill"
                } else {
                    "backfill_queued"
                };
                let overlay = crate::handlers::lifecycle_overlay::derive(
                    backfill_verb,
                    "",
                    &status,
                    blocked_reason.as_deref(),
                    integration_blocked_reason.as_deref(),
                )
                .with_context(|| {
                    format!("T144 P1: derive lifecycle overlay backfill for {display_id}")
                })?;
                tx.execute(
                    "UPDATE tasks SET lifecycle=?1, active_step=?2, integration_step=?3, blocked=?4, blocker_kind=?5 WHERE id=?6",
                    rusqlite::params![
                        overlay.lifecycle,
                        overlay.active_step,
                        overlay.integration_step,
                        if overlay.blocked { 1 } else { 0 },
                        overlay.blocker_kind,
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

    Ok(applied)
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
    /// `idx_tasks_integration_singleton` on tasks(status) WHERE
    /// status='integrating'. Verified via sqlite_master introspection.
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
            upper.contains("WHERE STATUS='INTEGRATING'")
                || upper.contains("WHERE STATUS = 'INTEGRATING'"),
            "index must filter on status='integrating': {sql}"
        );
    }

    /// AC1.5: two rows attempting to UPDATE status='integrating' simultaneously
    /// result in exactly one success and one ConstraintViolation. The partial
    /// UNIQUE index makes the integrating slot a substrate-wide singleton at
    /// the schema level — concurrent ticks no longer rely on best-effort
    /// SELECT-then-INSERT.
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

        // First UPDATE to 'integrating' must succeed.
        let r1 = conn.execute(
            "UPDATE tasks SET status='integrating' WHERE display_id='T801'",
            [],
        );
        assert!(
            r1.is_ok(),
            "first UPDATE to integrating must succeed: {r1:?}"
        );

        // Second UPDATE to 'integrating' must fail with ConstraintViolation.
        let r2 = conn.execute(
            "UPDATE tasks SET status='integrating' WHERE display_id='T802'",
            [],
        );
        let err = r2.expect_err("second UPDATE to integrating must fail");
        let msg = format!("{err}");
        assert!(
            msg.to_ascii_uppercase().contains("UNIQUE")
                || msg.to_ascii_uppercase().contains("CONSTRAINT"),
            "error must surface UNIQUE/constraint violation: {msg}"
        );

        // Sanity: only T801 holds the integrating slot.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status='integrating'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "exactly one row may hold status='integrating'");
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
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES ('T901','planning','p','p','n','n','framework','framework')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES ('T902','executing','e','e','n','n','framework','framework')",
            [],
        ).unwrap();
        apply_framework_drift(&conn).unwrap();
        let queued: String = conn
            .query_row(
                "SELECT lifecycle FROM tasks WHERE display_id='T901'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let active: String = conn
            .query_row(
                "SELECT lifecycle FROM tasks WHERE display_id='T902'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, "queued");
        assert_eq!(active, "active");
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
