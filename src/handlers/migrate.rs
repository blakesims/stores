use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::codegen::ddl::{ddl_for, expected_columns, quote_ident, ExpectedColumn};
use crate::manifest::Manifest;
use crate::schema::{FieldType, Schema};

/// Outcome of an applied migration. Returned by `apply_with` / `apply_at`.
#[derive(Debug, Default, Clone)]
pub struct MigrateReport {
    /// Names of `(store, column)` pairs that were ALTER TABLE-added.
    pub applied_columns: Vec<(String, String)>,
    pub orphaned: usize,
    pub type_mismatches: usize,
    /// True when the T107 cluster_key CHECK rebuild ran (i.e. the
    /// observations table was rebuilt to add the registry CHECK constraint).
    pub cluster_key_rebuilt: bool,
}

impl MigrateReport {
    pub fn is_no_op(&self) -> bool {
        self.applied_columns.is_empty() && !self.cluster_key_rebuilt
    }
}

/// A diff between the substrate's compiled-in schema and the live DB.
#[derive(Debug, Default)]
pub struct MigrationPlan {
    /// (store, ExpectedColumn) for columns present in schema but missing in DB.
    pub additive: Vec<(String, ExpectedColumn)>,
    /// (store, column) for columns present in DB but absent from schema.
    pub orphaned: Vec<(String, String)>,
    /// (store, column, db_type, expected_type) for type-mismatched columns.
    pub type_mismatches: Vec<(String, String, String, String)>,
}

/// Diff every installed store in the manifest against its compiled-in schema.
///
/// Reserved columns (id, display_id, status, created_at, …) must be present;
/// if any are missing this returns a hard error rather than emitting an
/// additive ALTER — restoring those is outside additive-only migration.
pub fn compute_plan(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
) -> Result<MigrationPlan> {
    let mut plan = MigrationPlan::default();

    for store in &manifest.stores {
        let schema = schemas.get(&store.name).ok_or_else(|| {
            anyhow!(
                "schema for installed store '{}' not loaded; manifest/binary mismatch",
                store.name
            )
        })?;

        let table = &store.table_name;
        let live_cols = read_table_info(conn, table)
            .with_context(|| format!("failed to read PRAGMA table_info for '{table}'"))?;

        let expected = expected_columns(schema);

        // Reserved column presence check (hard error if missing).
        for col in expected.iter().filter(|c| c.is_reserved) {
            if !live_cols.contains_key(&col.name) {
                bail!(
                    "corrupt schema for store '{}': reserved column '{}' is absent; cannot auto-recover",
                    store.name,
                    col.name
                );
            }
        }

        let expected_names: HashSet<&str> = expected.iter().map(|c| c.name.as_str()).collect();

        // Additive + type mismatches: walk expected, check live.
        for col in &expected {
            if col.is_reserved {
                continue;
            }
            match live_cols.get(&col.name) {
                None => {
                    plan.additive.push((store.name.clone(), col.clone()));
                }
                Some(db_type) => {
                    if !type_eq(db_type, &col.sql_type) {
                        plan.type_mismatches.push((
                            store.name.clone(),
                            col.name.clone(),
                            db_type.clone(),
                            col.sql_type.clone(),
                        ));
                    }
                }
            }
        }

        // Orphaned: columns in DB not in schema (skip reserved — they belong).
        for (name, _) in &live_cols {
            if expected_names.contains(name.as_str()) {
                continue;
            }
            plan.orphaned.push((store.name.clone(), name.clone()));
        }
    }

    Ok(plan)
}

/// In-process apply: take a connection + the schemas/manifest already loaded
/// and apply additive migrations transactionally. Returns a `MigrateReport`
/// summarizing the outcome (or no-op if nothing was missing).
///
/// Atomicity guarantee (Pi msg_8e242c48 item 2): ONE outer `BEGIN IMMEDIATE`
/// transaction wraps the entire sequence: preflight + additive DDL + tuple
/// backfill + source_id type-rebuild + default-backfill. If any step returns
/// an error the transaction is rolled back automatically (RAII drop without
/// commit), leaving the DB in exactly the pre-migration state. Type-mismatch
/// repair for source_id also runs on every invocation (not only when additive
/// columns are pending), so a half-migrated DB (additive applied, type-rebuild
/// not yet done) is always completed.
pub fn apply_with(
    conn: &mut Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
) -> Result<MigrateReport> {
    apply_with_inner(conn, schemas, manifest, false)
}

fn apply_with_inner(
    conn: &mut Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
    inject_post_ddl_failure: bool,
) -> Result<MigrateReport> {
    let plan = compute_plan(conn, schemas, manifest)?;
    let mut report = MigrateReport {
        applied_columns: Vec::new(),
        orphaned: plan.orphaned.len(),
        type_mismatches: plan.type_mismatches.len(),
        cluster_key_rebuilt: false,
    };

    let needs_source_rebuild = observations_source_id_type_mismatch(&plan);
    let needs_cluster_rebuild = observations_cluster_key_check_missing(conn)?;

    if plan.additive.is_empty() && !needs_source_rebuild && !needs_cluster_rebuild {
        return Ok(report);
    }

    let adding_source_tuple = observations_source_tuple_added(&plan);

    // Build rebuild SQL bodies BEFORE opening the transaction so `read_table_info`
    // reads the current (pre-DDL) column list.
    let additive_col_names: Vec<&str> = plan
        .additive
        .iter()
        .filter(|(store, _)| store == "observations")
        .map(|(_, col)| col.name.as_str())
        .collect();
    let rebuild_sql_body = if needs_source_rebuild {
        Some(rebuild_observations_source_id_as_text_sql(
            conn,
            schemas,
            manifest,
            &additive_col_names,
        )?)
    } else {
        None
    };
    let cluster_rebuild_sql_body = if needs_cluster_rebuild {
        Some(rebuild_observations_with_cluster_check_sql(
            conn,
            schemas,
            manifest,
            &additive_col_names,
        )?)
    } else {
        None
    };

    // Collect additive ALTER + tuple-backfill SQL (no DB reads needed here).
    let mut sql_lines: Vec<String> = Vec::with_capacity(plan.additive.len() + 2);
    for (store, col) in &plan.additive {
        sql_lines.push(format!(
            "ALTER TABLE {} ADD COLUMN {};",
            quote_ident(store),
            col.full_def
        ));
        report
            .applied_columns
            .push((store.clone(), col.name.clone()));
    }
    if adding_source_tuple {
        sql_lines.push(observations_source_tuple_backfill_sql("observations"));
    }

    // ONE outer BEGIN IMMEDIATE transaction wrapping the full sequence:
    //   preflight → additive DDL → tuple backfill → type rebuild → cluster rebuild
    //   → cluster backfill → default backfill.
    // Any error causes RAII drop (no commit) → automatic rollback.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to open BEGIN IMMEDIATE migration transaction")?;

    // Step 1: Preflight — runs inside the TX so a failure rolls back everything.
    if adding_source_tuple {
        observations_source_preflight(&tx, "observations")?;
    }

    // Step 2: Additive DDL + tuple backfill + type rebuild + cluster rebuild as a batch.
    let mut batch_lines: Vec<String> = Vec::new();
    batch_lines.extend(sql_lines);
    if let Some(ref rebuild_body) = rebuild_sql_body {
        batch_lines.push(rebuild_body.clone());
    }
    if let Some(ref cluster_body) = cluster_rebuild_sql_body {
        batch_lines.push(cluster_body.clone());
    }
    if !batch_lines.is_empty() {
        tx.execute_batch(&batch_lines.join("\n"))
            .context("failed to apply migrations (transaction will roll back)")?;
    }

    // Step 2b: cluster_key backfill — run inside the TX for atomicity.
    if needs_cluster_rebuild {
        cluster_key_backfill(&tx)?;
        report.cluster_key_rebuilt = true;
    }

    // Step 3: Default-backfill inside the same TX (T052 P1). ALTER TABLE ADD
    // COLUMN with a DEFAULT clause normally backfills existing rows, but
    // list:text JSON cells (DEFAULT '[]') and any path where the SQLite
    // version / pragma state elides the implicit backfill must still
    // materialise as the declared default rather than SQL NULL.
    if inject_post_ddl_failure {
        bail!("injected post-DDL failure for atomicity test");
    }
    if !plan.additive.is_empty() {
        backfill_defaults(&tx, schemas, manifest, &plan)?;
    }

    // Commit — all steps succeeded.
    tx.commit()
        .context("failed to commit migration transaction")?;

    Ok(report)
}

fn observations_source_tuple_added(plan: &MigrationPlan) -> bool {
    plan.additive.iter().any(|(store, col)| {
        store == "observations" && matches!(col.name.as_str(), "source_env" | "source_id")
    })
}

/// Preflight scan for cross-env ID coherence.
///
/// Fails loud if any row has:
///   (a) both prod_source_id AND sandbox_source_id set (ambiguous COALESCE pick)
///   (b) origin_db='prod' but sandbox_source_id set (wrong-env ID)
///   (c) origin_db='sandbox' but prod_source_id set (wrong-env ID)
///
/// Returns `Err` with a descriptive message listing the offending display_ids.
fn observations_source_preflight(conn: &Connection, table: &str) -> Result<()> {
    let t = quote_ident(table);

    // (a) Both IDs set.
    let both_set: Vec<String> = {
        let sql = format!(
            "SELECT display_id FROM {t} WHERE prod_source_id IS NOT NULL AND sandbox_source_id IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // (b) origin_db='prod' but sandbox_source_id set.
    let prod_with_sandbox: Vec<String> = {
        let sql = format!(
            "SELECT display_id FROM {t} WHERE origin_db = 'prod' AND sandbox_source_id IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // (c) origin_db='sandbox' but prod_source_id set.
    let sandbox_with_prod: Vec<String> = {
        let sql = format!(
            "SELECT display_id FROM {t} WHERE origin_db = 'sandbox' AND prod_source_id IS NOT NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // (d) origin_db IS NULL but legacy source IDs are populated — the backfill
    // query is `WHERE origin_db IS NOT NULL`, so these rows would be silently
    // skipped, producing canonical (NULL, NULL) while legacy IDs still exist.
    let null_origin_with_ids: Vec<String> = {
        let sql = format!(
            "SELECT display_id FROM {t} WHERE origin_db IS NULL AND (prod_source_id IS NOT NULL OR sandbox_source_id IS NOT NULL)"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    if both_set.is_empty()
        && prod_with_sandbox.is_empty()
        && sandbox_with_prod.is_empty()
        && null_origin_with_ids.is_empty()
    {
        return Ok(());
    }

    let mut msg = String::from(
        "T084 migration preflight FAILED — cross-env ID incoherence detected. \
         Manual repair required before migration can proceed.\n",
    );
    if !both_set.is_empty() {
        msg.push_str(&format!(
            "  Rows with BOTH prod_source_id AND sandbox_source_id set (ambiguous): {}\n",
            both_set.join(", ")
        ));
    }
    if !prod_with_sandbox.is_empty() {
        msg.push_str(&format!(
            "  Rows with origin_db='prod' but sandbox_source_id set (cross-env ID): {}\n",
            prod_with_sandbox.join(", ")
        ));
    }
    if !sandbox_with_prod.is_empty() {
        msg.push_str(&format!(
            "  Rows with origin_db='sandbox' but prod_source_id set (cross-env ID): {}\n",
            sandbox_with_prod.join(", ")
        ));
    }
    if !null_origin_with_ids.is_empty() {
        msg.push_str(&format!(
            "  Rows with origin_db=NULL but legacy source IDs populated (would be silently skipped by backfill): {}\n",
            null_origin_with_ids.join(", ")
        ));
    }
    bail!("{}", msg.trim_end())
}

fn observations_source_tuple_backfill_sql(table: &str) -> String {
    let t = quote_ident(table);
    // T084 contract: backfill canonical source_env from legacy origin_db for
    // prod/sandbox rows, even when the matching legacy ID is NULL. source_id is
    // the text COALESCE(prod_source_id, sandbox_source_id). origin_db is
    // preserved in-place for historical/filter compatibility during the
    // transition window.
    format!(
        "UPDATE {t} SET \
         source_env = CASE \
           WHEN origin_db IN ('prod', 'sandbox') THEN origin_db \
           ELSE NULL \
         END, \
         source_id = CAST(COALESCE(prod_source_id, sandbox_source_id) AS TEXT) \
         WHERE source_env IS NULL AND origin_db IS NOT NULL;"
    )
}

fn observations_source_id_type_mismatch(plan: &MigrationPlan) -> bool {
    plan.type_mismatches
        .iter()
        .any(|(store, col, db_type, expected_type)| {
            store == "observations"
                && col == "source_id"
                && db_type.eq_ignore_ascii_case("INTEGER")
                && expected_type.eq_ignore_ascii_case("TEXT")
        })
}

/// Returns the SQL body (without BEGIN/COMMIT) for rebuilding
/// `observations.source_id` from INTEGER to TEXT. Callers are responsible
/// for wrapping this in a transaction.
///
/// `extra_cols` lists column names that will be present in the table by the
/// time this SQL executes (e.g. columns added via ALTER in the same outer
/// transaction earlier in the batch) but are not yet visible to
/// `read_table_info` at call time. These are merged into the INSERT SELECT.
fn rebuild_observations_source_id_as_text_sql(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
    extra_cols: &[&str],
) -> Result<String> {
    let schema = schemas
        .get("observations")
        .ok_or_else(|| anyhow!("observations schema not loaded for T084 source_id rebuild"))?;
    let table = manifest
        .stores
        .iter()
        .find(|s| s.name == "observations")
        .map(|s| s.table_name.as_str())
        .unwrap_or("observations");
    let tmp_table = format!("{table}__t084_source_id_text");
    let create_tmp = ddl_for(schema).replace(
        &format!("CREATE TABLE IF NOT EXISTS {}", quote_ident(table)),
        &format!("CREATE TABLE {}", quote_ident(&tmp_table)),
    );
    let live_cols = read_table_info(conn, table)?;
    let expected = expected_columns(schema);
    // Include: columns present in live DB now, PLUS any columns that will be
    // present by the time the rebuild SQL executes (added in same transaction).
    let common: Vec<String> = expected
        .into_iter()
        .filter(|c| live_cols.contains_key(&c.name) || extra_cols.contains(&c.name.as_str()))
        .map(|c| c.name)
        .collect();
    let insert_cols = common
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let select_cols = common
        .iter()
        .map(|c| {
            if c == "source_id" {
                format!("CAST({} AS TEXT)", quote_ident(c))
            } else {
                quote_ident(c)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "DROP TABLE IF EXISTS {tmp};\n{create_tmp}\nINSERT INTO {tmp} ({insert_cols}) SELECT {select_cols} FROM {table_q};\nDROP TABLE {table_q};\nALTER TABLE {tmp} RENAME TO {table_q};",
        tmp = quote_ident(&tmp_table),
        table_q = quote_ident(table),
    ))
}

// ---------------------------------------------------------------------------
// T107: cluster_key CHECK rebuild helpers
// ---------------------------------------------------------------------------

/// Returns true iff the observations table exists but its CREATE TABLE SQL
/// does NOT contain the first curated registry key inside a CHECK clause for
/// cluster_key. This is the signal that the table was created before T107 and
/// must be rebuilt with the registry CHECK.
///
/// Returns false when the table does not exist (it will be created fresh with
/// the correct DDL) or when the CHECK is already present.
fn observations_cluster_key_check_missing(conn: &Connection) -> Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'observations' AND type = 'table'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .context("read sqlite_master for observations")?;
    let Some(create_sql) = sql else {
        return Ok(false);
    };
    let first_key = crate::handlers::cluster_keys::CURATED_CLUSTER_KEYS[0];
    Ok(!create_sql.contains(first_key))
}

/// Returns the SQL body (without BEGIN/COMMIT) for rebuilding the observations
/// table to add a registry-derived CHECK constraint on cluster_key. Rows whose
/// existing cluster_key is not in the registry have cluster_key reset to NULL
/// (defensive: there should be none on a well-managed DB, but the rebuild must
/// be safe on arbitrary pre-T107 data).
///
/// `extra_cols` lists column names that will be present in the table by the
/// time this SQL executes but are not yet visible to `read_table_info` at call
/// time (added in the same outer transaction earlier in the batch).
fn rebuild_observations_with_cluster_check_sql(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
    extra_cols: &[&str],
) -> Result<String> {
    let schema = schemas
        .get("observations")
        .ok_or_else(|| anyhow!("observations schema not loaded for T107 cluster_key rebuild"))?;
    let table = manifest
        .stores
        .iter()
        .find(|s| s.name == "observations")
        .map(|s| s.table_name.as_str())
        .unwrap_or("observations");
    let tmp_table = format!("{table}__t107_cluster_key_check");
    let create_tmp = ddl_for(schema).replace(
        &format!("CREATE TABLE IF NOT EXISTS {}", quote_ident(table)),
        &format!("CREATE TABLE {}", quote_ident(&tmp_table)),
    );
    let live_cols = read_table_info(conn, table)?;
    let expected = expected_columns(schema);
    let common: Vec<String> = expected
        .into_iter()
        .filter(|c| live_cols.contains_key(&c.name) || extra_cols.contains(&c.name.as_str()))
        .map(|c| c.name)
        .collect();
    let insert_cols = common
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    // Build IN list for out-of-registry check
    let in_list = crate::handlers::cluster_keys::CURATED_CLUSTER_KEYS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let select_cols = common
        .iter()
        .map(|c| {
            if c == "cluster_key" {
                // Reset any value that's not in the registry to NULL
                format!(
                    "CASE WHEN {ck} IS NOT NULL AND {ck} NOT IN ({in_list}) THEN NULL ELSE {ck} END",
                    ck = quote_ident(c),
                )
            } else {
                quote_ident(c)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "DROP TABLE IF EXISTS {tmp};\n{create_tmp}\nINSERT INTO {tmp} ({insert_cols}) SELECT {select_cols} FROM {table_q};\nDROP TABLE {table_q};\nALTER TABLE {tmp} RENAME TO {table_q};",
        tmp = quote_ident(&tmp_table),
        table_q = quote_ident(table),
    ))
}

/// Conservative regex backfill: for each observations row where cluster_key IS
/// NULL, call `classify_summary` and set cluster_key to the single matching
/// registry key. Ambiguous/unrelated rows remain NULL. Runs inside the outer
/// migration transaction so it's atomic with the rebuild.
fn cluster_key_backfill(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT display_id, summary FROM observations WHERE cluster_key IS NULL",
    )?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("read observations for cluster_key backfill")?;
    drop(stmt);
    for (display_id, summary) in rows {
        if let Some(key) = crate::handlers::cluster_keys::classify_summary(&summary) {
            conn.execute(
                "UPDATE observations SET cluster_key = ?1 WHERE display_id = ?2 AND cluster_key IS NULL",
                rusqlite::params![key, display_id],
            )
            .with_context(|| format!("backfill cluster_key on {display_id}"))?;
        }
    }
    Ok(())
}

/// Defensive UPDATE pass after ALTER TABLE ADD COLUMN (T052 P1).
/// For every additive column whose schema field declares a non-null default,
/// run `UPDATE <store> SET <col> = ? WHERE <col> IS NULL`.
fn backfill_defaults(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
    plan: &MigrationPlan,
) -> Result<()> {
    for (store_name, col) in &plan.additive {
        let schema = match schemas.get(store_name) {
            Some(s) => s,
            None => continue,
        };
        let field = match schema.fields.iter().find(|f| f.name == col.name) {
            Some(f) => f,
            None => continue,
        };
        let default_value = match &field.default {
            Some(v) if !v.is_null() => v,
            _ => continue,
        };
        let table = manifest
            .stores
            .iter()
            .find(|s| &s.name == store_name)
            .map(|s| s.table_name.as_str())
            .unwrap_or(store_name.as_str());
        let sql = format!(
            "UPDATE {} SET {} = ?1 WHERE {} IS NULL",
            quote_ident(table),
            quote_ident(&col.name),
            quote_ident(&col.name),
        );
        let sql_value = default_to_sql_value(&field.ty, default_value);
        conn.execute(&sql, rusqlite::params![sql_value])
            .with_context(|| {
                format!("failed to backfill default for '{store_name}.{}'", col.name)
            })?;
    }
    Ok(())
}

/// Convert a JSON default value into the SQLite literal representation that
/// matches how the add handler / DDL would store it.
fn default_to_sql_value(ty: &FieldType, v: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as SqlValue;
    match ty {
        FieldType::List(_)
        | FieldType::Record(_)
        | FieldType::ListRecord(_)
        | FieldType::ListFk { .. }
        | FieldType::Json => {
            let json = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
            SqlValue::Text(json)
        }
        FieldType::Bool => match v {
            serde_json::Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
            _ => SqlValue::Null,
        },
        FieldType::Integer => match v {
            serde_json::Value::Number(n) => SqlValue::Integer(n.as_i64().unwrap_or(0)),
            _ => SqlValue::Null,
        },
        _ => match v {
            serde_json::Value::String(s) => SqlValue::Text(s.clone()),
            serde_json::Value::Null => SqlValue::Null,
            other => SqlValue::Text(other.to_string()),
        },
    }
}

/// Convenience: load manifest + schemas from `root/.stores/manifest.yaml`
/// and apply against `conn`. Used by the `builtin:schema-migrate` subscriber.
pub fn apply_at(conn: &mut Connection, root: &Path) -> Result<MigrateReport> {
    let manifest = Manifest::load_from(root)?;
    let schemas = load_schemas(&manifest)?;
    apply_with(conn, &schemas, &manifest)
}

/// Run `stores migrate` (DRY-RUN unless `apply`).
pub fn run_migrate(apply: bool) -> Result<()> {
    crate::paths::ensure_initialized()?;
    let manifest = Manifest::load()?;
    let schemas = load_schemas(&manifest)?;
    // Open WITHOUT auto-applying framework drift so we can show the diff
    // before mutating the DB. (db::open would have already applied it.)
    let mut conn = crate::db::open_no_autoapply(&crate::paths::db_path()?)?;

    // --- Framework-DDL drift (T051) ----------------------------------------
    let framework_drift = crate::handlers::framework_migrate::compute_framework_drift(&conn)?;
    for (table, col) in &framework_drift.additive {
        println!(
            "framework: ALTER TABLE {} ADD COLUMN {};",
            quote_ident(table),
            col.full_def
        );
    }

    let plan = compute_plan(&conn, &schemas, &manifest)?;

    // Stderr warnings (orphaned + type mismatches).
    for (store, col) in &plan.orphaned {
        eprintln!(
            "warning: store '{store}': orphaned column '{col}' present in DB but not in schema; not auto-dropped (additive-only)"
        );
    }
    for (store, col, db_type, expected_type) in &plan.type_mismatches {
        eprintln!(
            "warning: store '{store}': column '{col}' type mismatch — DB has '{db_type}', schema expects '{expected_type}'; not auto-coerced (additive-only)"
        );
    }

    if apply && !framework_drift.additive.is_empty() {
        let applied = crate::handlers::framework_migrate::apply_framework_drift(&conn)?;
        for m in &applied {
            let line = serde_json::to_string(m).context("serialize AppliedFrameworkMigration")?;
            println!("applied: {line}");
        }
    }

    let needs_source_rebuild = observations_source_id_type_mismatch(&plan);

    if plan.additive.is_empty() && !needs_source_rebuild {
        if apply {
            let created = crate::handlers::architecture_reviews_backfill::run_backfill(&conn)?;
            if created > 0 {
                println!("architecture_reviews backfill: created {created} row(s)");
            }
        }
        return Ok(());
    }

    // Print the dry-run view of pending additive DDL.
    let mut sql_lines: Vec<String> = Vec::with_capacity(plan.additive.len() + 1);
    for (store, col) in &plan.additive {
        sql_lines.push(format!(
            "ALTER TABLE {} ADD COLUMN {};",
            quote_ident(store),
            col.full_def
        ));
    }
    if observations_source_tuple_added(&plan) {
        sql_lines.push(observations_source_tuple_backfill_sql("observations"));
    }
    if needs_source_rebuild {
        sql_lines.push("-- rebuild observations.source_id INTEGER → TEXT".to_string());
    }

    for line in &sql_lines {
        println!("{line}");
    }

    if apply {
        // Route through apply_with() — the single canonical apply path that
        // includes preflight, atomic transaction, default-backfill, and
        // source_id type-rebuild. This prevents CLI/lib code-path divergence.
        apply_with(&mut conn, &schemas, &manifest)?;
        let created = crate::handlers::architecture_reviews_backfill::run_backfill(&conn)?;
        if created > 0 {
            println!("architecture_reviews backfill: created {created} row(s)");
        }
    }

    Ok(())
}

/// Mirror of main.rs's manifest→schema loading logic.
fn load_schemas(manifest: &Manifest) -> Result<HashMap<String, Schema>> {
    let mut out: HashMap<String, Schema> = HashMap::new();
    for store in &manifest.stores {
        let path_str = store.schema_path.to_string_lossy();
        let yaml = if let Some(bundled_name) = path_str.strip_prefix("bundled:") {
            crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == bundled_name)
                .map(|(_, y)| y.to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "bundled store '{}' not found in binary; was the binary rebuilt?",
                        bundled_name
                    )
                })?
        } else {
            let schema_file = store.schema_path.join("schema.yaml");
            std::fs::read_to_string(&schema_file)?
        };
        let schema = Schema::from_yaml(&yaml)?;
        out.insert(store.name.clone(), schema);
    }
    Ok(out)
}

/// `PRAGMA table_info(<table>)` → map of column name → sql_type token.
fn read_table_info(conn: &Connection, table: &str) -> Result<HashMap<String, String>> {
    let sql = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(1)?;
        let ty: String = row.get(2)?;
        Ok((name, ty))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (name, ty) = r?;
        map.insert(name, ty);
    }
    Ok(map)
}

/// Case-insensitive equality on bare sql_type tokens (e.g. "TEXT" == "text").
fn type_eq(db_type: &str, expected_type: &str) -> bool {
    db_type.eq_ignore_ascii_case(expected_type)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::handlers::row::read_row;
    use crate::manifest::InstalledStore;
    use crate::schema::{actor::Actor, FieldType, Schema, StoreScope};
    use crate::validate::{self, Op};
    use std::path::PathBuf;

    const OBSERVATIONS_YAML: &str = include_str!("../../stores/observations/schema.yaml");
    const GATE_YAML: &str = include_str!("../../stores/gate/schema.yaml");
    const TASKS_YAML: &str = include_str!("../../stores/tasks/schema.yaml");

    fn load_bundled() -> (HashMap<String, Schema>, Manifest) {
        let mut schemas = HashMap::new();
        let mut stores = Vec::new();
        for (name, yaml) in [
            ("observations", OBSERVATIONS_YAML),
            ("gate", GATE_YAML),
            ("tasks", TASKS_YAML),
        ] {
            let s = Schema::from_yaml(yaml).expect("parse bundled");
            schemas.insert(name.to_string(), s.clone());
            stores.push(InstalledStore {
                name: name.to_string(),
                schema_path: PathBuf::from(format!("bundled:{name}")),
                installed_at: "2026-05-03T00:00:00Z".into(),
                table_name: name.to_string(),
                scope: StoreScope::Worktree,
            });
        }
        (schemas, Manifest { stores })
    }

    fn install_all(conn: &Connection, schemas: &HashMap<String, Schema>) {
        for s in schemas.values() {
            conn.execute_batch(&ddl_for(s)).unwrap();
        }
    }

    #[test]
    fn in_sync_db_yields_empty_plan() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);

        let plan = compute_plan(&conn, &schemas, &manifest).unwrap();
        assert!(plan.additive.is_empty(), "additive: {:?}", plan.additive);
        assert!(plan.orphaned.is_empty(), "orphaned: {:?}", plan.orphaned);
        assert!(
            plan.type_mismatches.is_empty(),
            "mismatches: {:?}",
            plan.type_mismatches
        );
    }

    /// (b) DB is missing a non-reserved scalar column → reported as additive.
    #[test]
    fn missing_column_reported_as_additive() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);

        // Pick a non-reserved scalar column from the observations schema and drop it.
        let obs = schemas.get("observations").unwrap();
        let target = expected_columns(obs)
            .into_iter()
            .find(|c| !c.is_reserved && c.sql_type == "TEXT")
            .expect("observations must have a TEXT field");

        // SQLite supports DROP COLUMN since 3.35.
        conn.execute_batch(&format!(
            "ALTER TABLE \"observations\" DROP COLUMN \"{}\";",
            target.name
        ))
        .unwrap();

        let plan = compute_plan(&conn, &schemas, &manifest).unwrap();
        assert_eq!(plan.additive.len(), 1, "additive: {:?}", plan.additive);
        assert_eq!(plan.additive[0].0, "observations");
        assert_eq!(plan.additive[0].1.name, target.name);
        assert!(plan.orphaned.is_empty());
        assert!(plan.type_mismatches.is_empty());
    }

    /// (c) Orphaned column reported when DB has an extra column.
    #[test]
    fn orphaned_column_reported() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);

        conn.execute_batch("ALTER TABLE \"gate\" ADD COLUMN extra_thing TEXT;")
            .unwrap();

        let plan = compute_plan(&conn, &schemas, &manifest).unwrap();
        assert!(plan.additive.is_empty());
        assert_eq!(
            plan.orphaned,
            vec![("gate".to_string(), "extra_thing".to_string())]
        );
        assert!(plan.type_mismatches.is_empty());
    }

    /// (d) Type mismatch reported when DB column type differs from schema.
    #[test]
    fn type_mismatch_reported() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();

        // Find a TEXT scalar field in tasks, then install with that column
        // mutated to INTEGER on the DB side.
        let tasks = schemas.get("tasks").unwrap();
        let text_field = expected_columns(tasks)
            .into_iter()
            .find(|c| !c.is_reserved && c.sql_type == "TEXT" && !c.full_def.contains("CHECK"))
            .expect("tasks must have a non-CHECK TEXT field");

        // Install observations + gate normally.
        conn.execute_batch(&ddl_for(schemas.get("observations").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();

        // Install tasks with the chosen field swapped to INTEGER.
        let mut ddl = ddl_for(tasks);
        let from = format!("    {} TEXT", text_field.name);
        let to = format!("    {} INTEGER", text_field.name);
        assert!(
            ddl.contains(&from),
            "expected DDL fragment {from:?} in:\n{ddl}"
        );
        ddl = ddl.replacen(&from, &to, 1);
        conn.execute_batch(&ddl).unwrap();

        let plan = compute_plan(&conn, &schemas, &manifest).unwrap();
        assert!(plan.additive.is_empty());
        assert!(plan.orphaned.is_empty());
        assert_eq!(plan.type_mismatches.len(), 1);
        let (store, col, db_type, expected_type) = &plan.type_mismatches[0];
        assert_eq!(store, "tasks");
        assert_eq!(col, &text_field.name);
        assert_eq!(db_type, "INTEGER");
        assert_eq!(expected_type, "TEXT");
    }

    /// Reserved column missing → hard error (not auto-recovered).
    #[test]
    fn missing_reserved_column_is_hard_error() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);

        conn.execute_batch("ALTER TABLE \"gate\" DROP COLUMN created_by;")
            .unwrap();

        let err = compute_plan(&conn, &schemas, &manifest).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("reserved column 'created_by' is absent"),
            "unexpected error: {msg}"
        );
    }

    // ---- T052 P1: defaults backfill on migration ----

    /// AC1.3 / Task 1.6 (b): apply_with backfills risk_class='normal',
    /// approval_policy='human', risk_flags='[]', cluster_key IS NULL on
    /// existing rows when the four columns were absent before migration.
    #[test]
    fn t052_p1_migrate_backfills_risk_taxonomy_defaults_on_existing_rows() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);

        // Drop the four T052 columns so the live DB is shaped like a pre-T052
        // observations table.
        for col in ["risk_class", "approval_policy", "risk_flags", "cluster_key"] {
            conn.execute_batch(&format!(
                "ALTER TABLE \"observations\" DROP COLUMN \"{col}\";"
            ))
            .unwrap();
        }

        // Insert an existing row that pre-dates the migration.
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                "L001", "open", "2026-05-01T00:00:00Z", "2026-05-01T00:00:00Z",
                "ai_autonomous", "ai_autonomous",
                "pre-existing observation", "dev", "normal",
                "2026-05-01T00:00:00Z", "w18-d1"
            ],
        ).unwrap();

        // Run additive migration: should ALTER ADD COLUMN the four columns
        // and backfill defaults on the existing row.
        let report = apply_with(&mut conn, &schemas, &manifest).expect("apply_with ok");
        let added: Vec<&str> = report
            .applied_columns
            .iter()
            .filter(|(s, _)| s == "observations")
            .map(|(_, c)| c.as_str())
            .collect();
        for expected in ["risk_class", "approval_policy", "risk_flags", "cluster_key"] {
            assert!(
                added.contains(&expected),
                "expected '{expected}' in applied_columns: {added:?}"
            );
        }

        // Read the pre-existing row and assert backfilled defaults.
        let (risk_class, approval_policy, risk_flags, cluster_key): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT risk_class, approval_policy, risk_flags, cluster_key FROM observations WHERE display_id = ?1",
                rusqlite::params!["L001"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(risk_class.as_deref(), Some("normal"));
        assert_eq!(approval_policy.as_deref(), Some("human"));
        assert_eq!(risk_flags.as_deref(), Some("[]"));
        assert_eq!(cluster_key, None, "cluster_key must remain NULL");
    }

    #[test]
    fn observations_temporal_read_regression_preserves_stored_and_derives_null_week() {
        let (schemas, _manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);
        let obs = schemas.get("observations").unwrap();

        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week) \
             VALUES (?1, 'open', ?2, ?2, 'human', 'human', ?3, 'dev', 'normal', ?4, ?5)",
            rusqlite::params!["L901", "2026-03-12T00:00:00Z", "stored week", "2026-03-13T08:00:00Z", "w11-d4"],
        ).unwrap();
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week) \
             VALUES (?1, 'open', ?2, ?2, 'human', 'human', ?3, 'dev', 'normal', ?4, NULL)",
            rusqlite::params!["L902", "2026-03-12T00:00:00Z", "null week", "2026-03-12T08:00:00Z"],
        ).unwrap();

        let (_, stored) = crate::handlers::row::read_row(obs, &conn, "L901").unwrap();
        let (_, derived) = crate::handlers::row::read_row(obs, &conn, "L902").unwrap();
        assert_eq!(
            stored.get("captured_week").and_then(|v| v.as_str()),
            Some("w11-d4")
        );
        assert_eq!(
            derived.get("captured_week").and_then(|v| v.as_str()),
            Some("w11-d4")
        );
    }

    /// AC1.5: PRAGMA table_info reports the four columns with the expected
    /// SQL types after install. CHECK constraints are exercised by the
    /// integration test below; PRAGMA only reports the bare type.
    #[test]
    fn t052_p1_observations_pragma_table_info_reports_taxonomy_columns() {
        let (schemas, _manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_all(&conn, &schemas);
        let live = read_table_info(&conn, "observations").unwrap();
        for col in ["risk_class", "approval_policy", "risk_flags", "cluster_key"] {
            assert!(
                live.contains_key(col),
                "column '{col}' missing from observations PRAGMA table_info: {live:?}"
            );
        }
        assert_eq!(live.get("risk_class").map(|s| s.as_str()), Some("TEXT"));
        assert_eq!(
            live.get("approval_policy").map(|s| s.as_str()),
            Some("TEXT")
        );
        assert_eq!(live.get("risk_flags").map(|s| s.as_str()), Some("TEXT"));
        assert_eq!(live.get("cluster_key").map(|s| s.as_str()), Some("TEXT"));
    }

    fn install_pre_t084_all(conn: &Connection, schemas: &HashMap<String, Schema>) {
        let mut pre_t084_observations = schemas.get("observations").unwrap().clone();
        pre_t084_observations
            .fields
            .retain(|f| f.name != "source_env");
        pre_t084_observations
            .fields
            .iter_mut()
            .find(|f| f.name == "source_id")
            .expect("observations.source_id exists")
            .ty = FieldType::Integer;
        conn.execute_batch(&ddl_for(&pre_t084_observations))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();
    }

    fn insert_pre_t084_source_row(
        conn: &Connection,
        display_id: &str,
        origin_db: &str,
        prod_source_id: Option<i64>,
        sandbox_source_id: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week, origin_db, prod_source_id, sandbox_source_id) \
             VALUES (?1, 'open', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', 'human', 'human', ?2, 'dashboard', 'normal', '2026-05-01T00:00:00Z', 'w18-d1', ?3, ?4, ?5)",
            rusqlite::params![display_id, display_id, origin_db, prod_source_id, sandbox_source_id],
        ).unwrap();
    }

    #[test]
    fn t084_migrate_backfills_prod_source_tuple() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L901", "prod", Some(123), None);

        let pre_live = read_table_info(&conn, "observations").unwrap();
        assert_eq!(
            pre_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER")
        );
        assert!(!pre_live.contains_key("source_env"));

        let report = apply_with(&mut conn, &schemas, &manifest).unwrap();
        assert!(report
            .applied_columns
            .iter()
            .any(|(s, c)| s == "observations" && c == "source_env"));
        let post_live = read_table_info(&conn, "observations").unwrap();
        assert_eq!(post_live.get("source_id").map(|s| s.as_str()), Some("TEXT"));
        let got: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id FROM observations WHERE display_id = 'L901'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(got, (Some("prod".into()), Some("123".into())));
    }

    #[test]
    fn t084_migrate_backfills_sandbox_source_tuple() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L902", "sandbox", None, Some(456));

        apply_with(&mut conn, &schemas, &manifest).unwrap();
        let got: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id FROM observations WHERE display_id = 'L902'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(got, (Some("sandbox".into()), Some("456".into())));
    }

    /// AC1.5: origin-only legacy rows (origin_db set, both *_source_id NULL)
    /// backfill source_env from origin_db, source_id NULL, and remain readable.
    #[test]
    fn t084_migrate_origin_only_legacy_row_backfills_env_null_id() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L903", "prod", None, None);
        insert_pre_t084_source_row(&conn, "L904", "sandbox", None, None);

        apply_with(&mut conn, &schemas, &manifest).unwrap();

        for (display_id, expected_env) in [("L903", "prod"), ("L904", "sandbox")] {
            let got: (Option<String>, Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = ?1",
                    [display_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(got.0.as_deref(), Some(expected_env), "{display_id} source_env");
            assert_eq!(got.1.as_deref(), None, "{display_id} source_id");
            assert_eq!(got.2.as_deref(), Some(expected_env), "{display_id} origin_db preserved");
        }

        // Readability plus unrelated update validation: no source tuple flags/diff
        // means the historical env-without-id tuple is tolerated for transition rows.
        let observations = schemas.get("observations").unwrap();
        let (_, mut merged) = read_row(observations, &conn, "L903").unwrap();
        let mut diff = validate::EntryMap::new();
        diff.insert("status".to_string(), serde_json::json!("triaged"));
        merged.insert("status".to_string(), serde_json::json!("triaged"));
        validate::validate(
            observations,
            &merged,
            Op::Update(diff),
            Actor::Human.into(),
        )
        .expect("unrelated update should not revalidate historical source_env without source_id");
    }

    /// Type comparison is case-insensitive on the bare token.
    #[test]
    fn type_eq_is_case_insensitive() {
        assert!(type_eq("text", "TEXT"));
        assert!(type_eq("INTEGER", "integer"));
        assert!(!type_eq("TEXT", "INTEGER"));
    }

    // ---- T084 codex-revise r0: preflight, CASE backfill, atomicity, repair-skip ----

    /// Preflight rejects a row with BOTH prod_source_id AND sandbox_source_id set.
    #[test]
    fn t084_preflight_fails_loud_both_ids_set() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // Both IDs set — ambiguous.
        insert_pre_t084_source_row(&conn, "L910", "prod", Some(1), Some(2));

        let err = apply_with(&mut conn, &schemas, &manifest).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("L910"),
            "expected offending display_id 'L910' in error: {msg}"
        );
        assert!(
            msg.contains("BOTH") || msg.contains("both"),
            "expected 'both' in error message: {msg}"
        );
    }

    /// Preflight rejects a row with origin_db='prod' but sandbox_source_id set.
    #[test]
    fn t084_preflight_fails_loud_prod_origin_with_sandbox_id() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // origin_db='prod' but sandbox_source_id set — cross-env.
        insert_pre_t084_source_row(&conn, "L911", "prod", None, Some(99));

        let err = apply_with(&mut conn, &schemas, &manifest).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("L911"),
            "expected offending display_id 'L911' in error: {msg}"
        );
    }

    /// Preflight rejects a row with origin_db='sandbox' but prod_source_id set.
    #[test]
    fn t084_preflight_fails_loud_sandbox_origin_with_prod_id() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // origin_db='sandbox' but prod_source_id set — cross-env.
        insert_pre_t084_source_row(&conn, "L912", "sandbox", Some(77), None);

        let err = apply_with(&mut conn, &schemas, &manifest).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("L912"),
            "expected offending display_id 'L912' in error: {msg}"
        );
    }

    /// Clean coherent rows (prod→prod_id, sandbox→sandbox_id, origin-only) succeed.
    #[test]
    fn t084_preflight_passes_clean_coherent_rows() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L920", "prod", Some(1), None);
        insert_pre_t084_source_row(&conn, "L921", "sandbox", None, Some(2));
        insert_pre_t084_source_row(&conn, "L922", "prod", None, None); // origin-only

        let report = apply_with(&mut conn, &schemas, &manifest).unwrap();
        // All three env-additive columns applied.
        assert!(report
            .applied_columns
            .iter()
            .any(|(s, c)| s == "observations" && c == "source_env"));

        // Check CASE backfill correctness.
        let rows: Vec<(String, Option<String>, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT display_id, source_env, source_id FROM observations ORDER BY display_id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        // L920: prod → source_env=prod, source_id=1
        let r920 = rows.iter().find(|(id, _, _)| id == "L920").unwrap();
        assert_eq!(r920.1.as_deref(), Some("prod"));
        assert_eq!(r920.2.as_deref(), Some("1"));
        // L921: sandbox → source_env=sandbox, source_id=2
        let r921 = rows.iter().find(|(id, _, _)| id == "L921").unwrap();
        assert_eq!(r921.1.as_deref(), Some("sandbox"));
        assert_eq!(r921.2.as_deref(), Some("2"));
        // L922: origin-only (prod, NULL, NULL) → source_env=prod, source_id=NULL.
        let r922 = rows.iter().find(|(id, _, _)| id == "L922").unwrap();
        assert_eq!(
            r922.1.as_deref(),
            Some("prod"),
            "origin-only row: source_env must backfill from origin_db"
        );
        assert_eq!(r922.2, None, "origin-only row: source_id must be NULL");
    }

    /// Half-migrated DB (additive applied but source_id still INTEGER) is
    /// repaired by the next apply_with invocation even though no additive
    /// columns are pending.
    #[test]
    fn t084_half_migrated_db_repair_completes_on_next_apply() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        // Simulate the half-migrated state: source_env column is already present
        // (via ALTER ADD COLUMN) but source_id is still INTEGER.
        let mut pre_half = schemas.get("observations").unwrap().clone();
        pre_half
            .fields
            .iter_mut()
            .find(|f| f.name == "source_id")
            .expect("source_id exists")
            .ty = FieldType::Integer;
        // source_env is present (unlike install_pre_t084_all), so no additive
        // columns are pending — only the type mismatch remains.
        conn.execute_batch(&ddl_for(&pre_half)).unwrap();
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();

        let pre_live = read_table_info(&conn, "observations").unwrap();
        assert_eq!(
            pre_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER"),
            "fixture must have INTEGER source_id before migration"
        );
        assert!(
            pre_live.contains_key("source_env"),
            "fixture must already have source_env (half-migrated)"
        );

        // apply_with with no additive columns pending — but type mismatch present.
        let report = apply_with(&mut conn, &schemas, &manifest).expect("apply_with should succeed");
        assert!(
            report.applied_columns.is_empty(),
            "no additive columns expected: {:?}",
            report.applied_columns
        );

        // source_id must now be TEXT.
        let post_live = read_table_info(&conn, "observations").unwrap();
        assert_eq!(
            post_live.get("source_id").map(|s| s.as_str()),
            Some("TEXT"),
            "source_id must be TEXT after repair"
        );
    }

    /// Preflight rejects a row with origin_db=NULL but a legacy source ID populated.
    /// These rows would be silently skipped by the backfill WHERE clause
    /// (`WHERE origin_db IS NOT NULL`), leaving canonical (NULL, NULL) while
    /// legacy IDs still exist.
    #[test]
    fn t084_preflight_fails_loud_null_origin_db_with_prod_source_id() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // origin_db IS NULL but prod_source_id is set — dirty shape.
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, created_by, updated_by, summary, source, priority, captured_at, captured_week, origin_db, prod_source_id, sandbox_source_id) \
             VALUES (?1, 'open', '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', 'human', 'human', ?1, 'dashboard', 'normal', '2026-05-01T00:00:00Z', 'w18-d1', NULL, 42, NULL)",
            rusqlite::params!["L940"],
        ).unwrap();

        let err = apply_with(&mut conn, &schemas, &manifest).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("L940"),
            "expected offending display_id 'L940' in error: {msg}"
        );
        assert!(
            msg.contains("origin_db") || msg.contains("NULL"),
            "error should mention origin_db=NULL gap: {msg}"
        );
    }

    /// AC1.5: adversarial fixture — origin_db='prod' with BOTH *_source_id NULL
    /// → migration produces source_env='prod', source_id=NULL, origin_db
    /// preserved. Coherent rows (prod/42, sandbox/99) still migrate correctly.
    #[test]
    fn t084_r0_followup_origin_only_backfills_env_null_id_origin_db_preserved() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);

        // Adversarial: origin_db='prod', prod_source_id=NULL, sandbox_source_id=NULL.
        insert_pre_t084_source_row(&conn, "L930", "prod", None, None);
        // Coherent prod row.
        insert_pre_t084_source_row(&conn, "L931", "prod", Some(42), None);
        // Coherent sandbox row.
        insert_pre_t084_source_row(&conn, "L932", "sandbox", None, Some(99));

        apply_with(&mut conn, &schemas, &manifest).expect("apply_with must succeed");

        // Origin-only: source_env='prod', source_id=NULL; origin_db still 'prod'.
        let r930: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = 'L930'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (r930.0.as_deref(), r930.1.as_deref()),
            (Some("prod"), None),
            "origin-only row L930 must backfill env with NULL id; got {:?}/{:?}",
            r930.0,
            r930.1,
        );
        assert_eq!(
            r930.2.as_deref(),
            Some("prod"),
            "origin_db must be preserved for L930 during transition window"
        );

        // Coherent prod row: canonical (prod, 42).
        let r931: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = 'L931'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(r931.0.as_deref(), Some("prod"), "L931 source_env");
        assert_eq!(r931.1.as_deref(), Some("42"), "L931 source_id");
        assert_eq!(r931.2.as_deref(), Some("prod"), "L931 origin_db preserved");

        // Coherent sandbox row: canonical (sandbox, 99).
        let r932: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = 'L932'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(r932.0.as_deref(), Some("sandbox"), "L932 source_env");
        assert_eq!(r932.1.as_deref(), Some("99"), "L932 source_id");
        assert_eq!(
            r932.2.as_deref(),
            Some("sandbox"),
            "L932 origin_db preserved"
        );
    }

    /// Atomicity: when preflight fails (cross-env row present), the DB must
    /// be left in exactly the pre-migration state — no additive columns, no
    /// partial backfill.
    #[test]
    fn t084_preflight_failure_leaves_db_in_pre_migration_state() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);

        // Coherent row (will be fine if migration ran).
        insert_pre_t084_source_row(&conn, "L950", "prod", Some(1), None);
        // Poisoned row: both IDs set — will trigger preflight failure.
        insert_pre_t084_source_row(&conn, "L951", "prod", Some(2), Some(3));

        let pre_live = read_table_info(&conn, "observations").unwrap();
        assert!(
            !pre_live.contains_key("source_env"),
            "pre-condition: no source_env column"
        );
        assert_eq!(
            pre_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER"),
            "pre-condition: INTEGER source_id"
        );

        // Migration must fail because of L951.
        let err = apply_with(&mut conn, &schemas, &manifest).unwrap_err();
        assert!(
            format!("{err:#}").contains("L951"),
            "error must mention L951"
        );

        // Post-failure: DB must be unchanged — no source_env column added, source_id still INTEGER.
        let post_live = read_table_info(&conn, "observations").unwrap();
        assert!(
            !post_live.contains_key("source_env"),
            "source_env column must NOT be present after failed migration"
        );
        assert_eq!(
            post_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER"),
            "source_id must still be INTEGER after failed migration"
        );
    }

    /// Atomicity (post-DDL failure path, Finding 2): when a failure occurs AFTER
    /// additive DDL is applied but before the transaction commits (injected via
    /// the test-only apply_with_inner flag), the entire TX must roll back — no additive
    /// columns must remain in the schema.
    ///
    /// This covers the case preflight-only atomicity cannot: a partial migration
    /// that succeeded DDL but failed before commit.
    #[test]
    fn t084_post_ddl_failure_rolls_back_entire_transaction() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);

        // Insert a clean coherent row so preflight passes and DDL runs.
        insert_pre_t084_source_row(&conn, "L960", "prod", Some(10), None);

        let pre_live = read_table_info(&conn, "observations").unwrap();
        assert!(
            !pre_live.contains_key("source_env"),
            "pre-condition: source_env must not exist before migration"
        );
        assert_eq!(
            pre_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER"),
            "pre-condition: source_id must be INTEGER"
        );

        // Inject failure AFTER DDL executes but before commit.
        let result = apply_with_inner(&mut conn, &schemas, &manifest, true);

        assert!(
            result.is_err(),
            "apply_with must fail with injected post-DDL error"
        );
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("injected post-DDL failure"),
            "error must mention injected failure: {msg}"
        );

        // Post-failure: the TX must have rolled back — schema unchanged.
        let post_live = read_table_info(&conn, "observations").unwrap();
        assert!(
            !post_live.contains_key("source_env"),
            "source_env column must NOT be present after post-DDL failure rollback"
        );
        assert_eq!(
            post_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER"),
            "source_id must still be INTEGER after post-DDL failure rollback"
        );

        // Data must also be intact — prod_source_id is still INTEGER in pre-T084 schema.
        let prod_source_id: Option<i64> = conn
            .query_row(
                "SELECT prod_source_id FROM observations WHERE display_id = 'L960'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            prod_source_id,
            Some(10),
            "prod_source_id must be intact after post-DDL failure rollback"
        );
    }

    // ---- T107: cluster_key CHECK rebuild tests ----

    /// Install observations WITHOUT the cluster_key CHECK (pre-T107 shape).
    fn install_pre_t107_observations(conn: &Connection, schema: &Schema) {
        // Create the table with cluster_key as plain TEXT (no CHECK)
        let ddl = ddl_for(schema);
        let check = crate::handlers::cluster_keys::check_clause_sql();
        let pre_t107_ddl = ddl.replace(&format!(" {check}"), "");
        conn.execute_batch(&pre_t107_ddl).unwrap();
    }

    /// Check cluster_key_check_missing detection via sqlite_master.sql
    #[test]
    fn t107_check_missing_detection() {
        let (schemas, _manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        let obs_schema = schemas.get("observations").unwrap();

        // Before creating the table: not missing (table doesn't exist)
        assert!(!observations_cluster_key_check_missing(&conn).unwrap());

        // Install without CHECK
        install_pre_t107_observations(&conn, obs_schema);
        assert!(
            observations_cluster_key_check_missing(&conn).unwrap(),
            "pre-T107 table without CHECK must be detected as missing"
        );

        // Install WITH CHECK (fresh install)
        conn.execute_batch("DROP TABLE IF EXISTS observations;")
            .unwrap();
        conn.execute_batch(&ddl_for(obs_schema)).unwrap();
        assert!(
            !observations_cluster_key_check_missing(&conn).unwrap(),
            "post-T107 table with CHECK must not be missing"
        );
    }

    /// apply_with on a pre-T107 DB runs the rebuild and is idempotent on the second call.
    #[test]
    fn t107_apply_with_rebuilds_and_is_idempotent() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        let obs_schema = schemas.get("observations").unwrap();

        // Install observations without CHECK + other stores normally
        install_pre_t107_observations(&conn, obs_schema);
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();

        // Insert a pre-existing row
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week) \
             VALUES ('L001','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                     'human','human','pre-existing obs','dev','normal',\
                     '2026-01-01T00:00:00Z','w01-d1')",
            [],
        )
        .unwrap();

        // First apply_with: should rebuild
        let report = apply_with(&mut conn, &schemas, &manifest).expect("first apply_with ok");
        assert!(
            report.cluster_key_rebuilt,
            "first apply_with must set cluster_key_rebuilt=true"
        );

        // Verify the CHECK is now present
        assert!(
            !observations_cluster_key_check_missing(&conn).unwrap(),
            "after rebuild, CHECK must be present"
        );

        // Verify pre-existing row survived with cluster_key=NULL
        let ck: Option<String> = conn
            .query_row(
                "SELECT cluster_key FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ck, None, "pre-existing row must have cluster_key=NULL");

        // Second apply_with: idempotent
        let report2 = apply_with(&mut conn, &schemas, &manifest).expect("second apply_with ok");
        assert!(
            report2.is_no_op(),
            "second apply_with must be a no-op: {report2:?}"
        );
        assert!(!report2.cluster_key_rebuilt, "second apply_with must not rebuild");
    }

    /// Backfill: 3 unambiguous + 1 ambiguous + 1 unrelated → only 3 get cluster_key set.
    #[test]
    fn t107_backfill_sets_exactly_3_unambiguous_rows() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        let obs_schema = schemas.get("observations").unwrap();

        install_pre_t107_observations(&conn, obs_schema);
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();

        // Insert 5 rows:
        // 3 unambiguous deploy-blocked matches
        for i in 0..3u32 {
            conn.execute(
                "INSERT INTO observations \
                 (display_id, status, created_at, updated_at, created_by, updated_by, \
                  summary, source, priority, captured_at, captured_week) \
                 VALUES (?1,'open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                         'human','human',?2,'dev','normal','2026-01-01T00:00:00Z','w01-d1')",
                rusqlite::params![
                    format!("L{:03}", i + 1),
                    "deploy blocked by merge conflict"
                ],
            )
            .unwrap();
        }
        // 1 ambiguous: matches both deploy-blocked and stale-base
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week) \
             VALUES ('L004','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                     'human','human','stale-base merge conflict','dev','normal',\
                     '2026-01-01T00:00:00Z','w01-d1')",
            [],
        )
        .unwrap();
        // 1 unrelated
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week) \
             VALUES ('L005','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                     'human','human','completely unrelated observation','dev','normal',\
                     '2026-01-01T00:00:00Z','w01-d1')",
            [],
        )
        .unwrap();

        apply_with(&mut conn, &schemas, &manifest).expect("apply_with ok");

        // Verify backfill results
        let rows: Vec<(String, Option<String>)> = {
            let mut stmt = conn
                .prepare("SELECT display_id, cluster_key FROM observations ORDER BY display_id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        let deploy_blocked: Vec<&(String, Option<String>)> = rows
            .iter()
            .filter(|(id, _)| matches!(id.as_str(), "L001" | "L002" | "L003"))
            .collect();
        for (id, ck) in &deploy_blocked {
            assert_eq!(
                ck.as_deref(),
                Some("deploy-blocked-merge-conflict"),
                "row {id} must have deploy-blocked-merge-conflict"
            );
        }
        // Ambiguous and unrelated must remain NULL
        let (_, l004_ck) = rows.iter().find(|(id, _)| id == "L004").unwrap();
        assert_eq!(l004_ck, &None, "L004 (ambiguous) must remain NULL");
        let (_, l005_ck) = rows.iter().find(|(id, _)| id == "L005").unwrap();
        assert_eq!(l005_ck, &None, "L005 (unrelated) must remain NULL");
    }

    /// Rows with a valid registry value before the rebuild retain their value.
    #[test]
    fn t107_rebuild_preserves_existing_valid_cluster_key() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        let obs_schema = schemas.get("observations").unwrap();

        install_pre_t107_observations(&conn, obs_schema);
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();

        // Row with a valid registry value set before migration
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week, cluster_key) \
             VALUES ('L001','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                     'human','human','test obs','dev','normal','2026-01-01T00:00:00Z',\
                     'w01-d1','silent-zombie-watchdog')",
            [],
        )
        .unwrap();

        apply_with(&mut conn, &schemas, &manifest).expect("apply_with ok");

        let ck: Option<String> = conn
            .query_row(
                "SELECT cluster_key FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ck.as_deref(),
            Some("silent-zombie-watchdog"),
            "valid pre-existing cluster_key must be preserved"
        );
    }

    /// Rows with an invalid (out-of-registry) cluster_key before the rebuild
    /// have cluster_key reset to NULL after the rebuild.
    #[test]
    fn t107_rebuild_resets_invalid_cluster_key_to_null() {
        let (schemas, manifest) = load_bundled();
        let mut conn = Connection::open_in_memory().unwrap();
        let obs_schema = schemas.get("observations").unwrap();

        install_pre_t107_observations(&conn, obs_schema);
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap()))
            .unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap()))
            .unwrap();

        // Row with an invalid cluster_key (old format not in registry)
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week, cluster_key) \
             VALUES ('L001','open','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',\
                     'human','human','test obs','dev','normal','2026-01-01T00:00:00Z',\
                     'w01-d1','old-invalid-key')",
            [],
        )
        .unwrap();

        apply_with(&mut conn, &schemas, &manifest).expect("apply_with ok");

        let ck: Option<String> = conn
            .query_row(
                "SELECT cluster_key FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ck, None, "invalid pre-existing cluster_key must be reset to NULL");
    }
}
