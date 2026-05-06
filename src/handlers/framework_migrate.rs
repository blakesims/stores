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
        return Ok(Vec::new());
    }
    let binary_version = env!("CARGO_PKG_VERSION").to_string();
    let applied_at = crate::handlers::row::now_iso8601();

    let mut applied: Vec<AppliedFrameworkMigration> = Vec::with_capacity(drift.additive.len());
    for (table, col) in &drift.additive {
        let ddl = format!(
            "ALTER TABLE {} ADD COLUMN {};",
            quote_ident(table),
            col.full_def
        );
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
