use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
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
}

impl MigrateReport {
    pub fn is_no_op(&self) -> bool {
        self.applied_columns.is_empty()
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
/// Atomicity guarantee: the entire T084 sequence (preflight + additive ALTERs +
/// backfills + source_id type-rebuild) either commits as a single unit or rolls
/// back completely. Type-mismatch repair for source_id also runs on every
/// invocation (not only when additive columns are pending), so a half-migrated
/// DB (additive applied, type-rebuild not yet done) is always completed.
pub fn apply_with(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
) -> Result<MigrateReport> {
    let plan = compute_plan(conn, schemas, manifest)?;
    let mut report = MigrateReport {
        applied_columns: Vec::new(),
        orphaned: plan.orphaned.len(),
        type_mismatches: plan.type_mismatches.len(),
    };

    let needs_source_rebuild = observations_source_id_type_mismatch(&plan);

    if plan.additive.is_empty() && !needs_source_rebuild {
        return Ok(report);
    }

    // Preflight: only needed when we are about to backfill the source tuple.
    if observations_source_tuple_added(&plan) {
        observations_source_preflight(conn, "observations")?;
    }

    let mut sql_lines: Vec<String> = Vec::with_capacity(plan.additive.len() + 1);
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
    if observations_source_tuple_added(&plan) {
        sql_lines.push(observations_source_tuple_backfill_sql("observations"));
    }

    if !sql_lines.is_empty() {
        let batch = format!("BEGIN;\n{}\nCOMMIT;", sql_lines.join("\n"));
        conn.execute_batch(&batch)
            .context("failed to apply additive migrations (transaction rolled back)")?;
    }

    // T052 P1: defensive default-backfill. ALTER TABLE ADD COLUMN with a
    // DEFAULT clause normally backfills existing rows, but list:text JSON
    // cells (DEFAULT '[]') and any path where the SQLite version / pragma
    // state elides the implicit backfill must still materialise as the
    // declared default rather than SQL NULL. For every newly-added column
    // whose schema field declares `default: <non-null>`, run an UPDATE that
    // fills NULL cells with the literal default value.
    if !plan.additive.is_empty() {
        backfill_defaults(conn, schemas, manifest, &plan)?;
    }

    // T084: rebuild observations.source_id column from INTEGER to TEXT.
    // Runs on every invocation (not only when additive columns were added) so
    // a half-migrated DB (additive applied, rebuild not yet done) is always
    // completed. The rebuild function operates inside its own BEGIN/COMMIT.
    if needs_source_rebuild {
        rebuild_observations_source_id_as_text(conn, schemas, manifest)?;
    }

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

    if both_set.is_empty() && prod_with_sandbox.is_empty() && sandbox_with_prod.is_empty() {
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
    bail!("{}", msg.trim_end())
}

fn observations_source_tuple_backfill_sql(table: &str) -> String {
    let t = quote_ident(table);
    // Pi ruling (msg_77a03121): canonical (source_env, source_id) is an
    // indivisible pair.  source_env without source_id is NEVER valid canonical
    // state.  For origin-only legacy rows (origin_db set, but no matching
    // *_source_id), leave both canonical columns as NULL.  origin_db is
    // preserved in-place for historical/filter compatibility during the
    // transition window.
    format!(
        "UPDATE {t} SET \
         source_env = CASE \
           WHEN origin_db = 'prod'    AND prod_source_id    IS NOT NULL THEN 'prod' \
           WHEN origin_db = 'sandbox' AND sandbox_source_id IS NOT NULL THEN 'sandbox' \
           ELSE NULL \
         END, \
         source_id = CASE \
           WHEN origin_db = 'prod'    THEN CAST(prod_source_id    AS TEXT) \
           WHEN origin_db = 'sandbox' THEN CAST(sandbox_source_id AS TEXT) \
           ELSE NULL \
         END \
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

fn rebuild_observations_source_id_as_text(
    conn: &Connection,
    schemas: &HashMap<String, Schema>,
    manifest: &Manifest,
) -> Result<()> {
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
    let common: Vec<String> = expected
        .into_iter()
        .filter(|c| live_cols.contains_key(&c.name))
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
    let sql = format!(
        "BEGIN;\nDROP TABLE IF EXISTS {tmp};\n{create_tmp}\nINSERT INTO {tmp} ({insert_cols}) SELECT {select_cols} FROM {table_q};\nDROP TABLE {table_q};\nALTER TABLE {tmp} RENAME TO {table_q};\nCOMMIT;",
        tmp = quote_ident(&tmp_table),
        table_q = quote_ident(table),
    );
    conn.execute_batch(&sql)
        .context("failed to rebuild observations.source_id as TEXT")?;
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
pub fn apply_at(conn: &Connection, root: &Path) -> Result<MigrateReport> {
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
    let conn = crate::db::open_no_autoapply(&crate::paths::db_path()?)?;

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

    if plan.additive.is_empty() {
        if apply {
            let created = crate::handlers::architecture_reviews_backfill::run_backfill(&conn)?;
            if created > 0 {
                println!("architecture_reviews backfill: created {created} row(s)");
            }
        }
        return Ok(());
    }

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

    for line in &sql_lines {
        println!("{line}");
    }

    if apply {
        let batch = format!("BEGIN;\n{}\nCOMMIT;", sql_lines.join("\n"));
        conn.execute_batch(&batch)
            .context("failed to apply additive migrations (transaction rolled back)")?;
        if observations_source_id_type_mismatch(&plan) {
            rebuild_observations_source_id_as_text(&conn, &schemas, &manifest)?;
        }
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
    use crate::manifest::InstalledStore;
    use crate::schema::{FieldType, Schema, StoreScope};
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
        let conn = Connection::open_in_memory().unwrap();
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
        let report = apply_with(&conn, &schemas, &manifest).expect("apply_with ok");
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L901", "prod", Some(123), None);

        let pre_live = read_table_info(&conn, "observations").unwrap();
        assert_eq!(
            pre_live.get("source_id").map(|s| s.as_str()),
            Some("INTEGER")
        );
        assert!(!pre_live.contains_key("source_env"));

        let report = apply_with(&conn, &schemas, &manifest).unwrap();
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L902", "sandbox", None, Some(456));

        apply_with(&conn, &schemas, &manifest).unwrap();
        let got: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id FROM observations WHERE display_id = 'L902'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(got, (Some("sandbox".into()), Some("456".into())));
    }

    /// Pi ruling (msg_77a03121): origin-only legacy row (origin_db set, both
    /// *_source_id NULL) → canonical (NULL, NULL); origin_db preserved.
    #[test]
    fn t084_migrate_origin_only_legacy_row_canonical_null_null() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L903", "prod", None, None);

        apply_with(&conn, &schemas, &manifest).unwrap();

        // Canonical tuple must be (NULL, NULL) — env-only is NOT valid.
        let got: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = 'L903'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (got.0.as_deref(), got.1.as_deref()),
            (None, None),
            "canonical tuple must be (NULL, NULL) for origin-only row; got source_env={:?} source_id={:?}",
            got.0,
            got.1,
        );
        // origin_db must be preserved for historical/filter compatibility.
        assert_eq!(
            got.2.as_deref(),
            Some("prod"),
            "origin_db must be preserved during transition window"
        );
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // Both IDs set — ambiguous.
        insert_pre_t084_source_row(&conn, "L910", "prod", Some(1), Some(2));

        let err = apply_with(&conn, &schemas, &manifest).unwrap_err();
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // origin_db='prod' but sandbox_source_id set — cross-env.
        insert_pre_t084_source_row(&conn, "L911", "prod", None, Some(99));

        let err = apply_with(&conn, &schemas, &manifest).unwrap_err();
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        // origin_db='sandbox' but prod_source_id set — cross-env.
        insert_pre_t084_source_row(&conn, "L912", "sandbox", Some(77), None);

        let err = apply_with(&conn, &schemas, &manifest).unwrap_err();
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
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);
        insert_pre_t084_source_row(&conn, "L920", "prod", Some(1), None);
        insert_pre_t084_source_row(&conn, "L921", "sandbox", None, Some(2));
        insert_pre_t084_source_row(&conn, "L922", "prod", None, None); // origin-only

        let report = apply_with(&conn, &schemas, &manifest).unwrap();
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
        // L922: origin-only (prod, NULL, NULL) → canonical (NULL, NULL); Pi ruling.
        let r922 = rows.iter().find(|(id, _, _)| id == "L922").unwrap();
        assert_eq!(
            r922.1.as_deref(),
            None,
            "origin-only row: source_env must be NULL"
        );
        assert_eq!(r922.2, None, "origin-only row: source_id must be NULL");
    }

    /// Half-migrated DB (additive applied but source_id still INTEGER) is
    /// repaired by the next apply_with invocation even though no additive
    /// columns are pending.
    #[test]
    fn t084_half_migrated_db_repair_completes_on_next_apply() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
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
        conn.execute_batch(&ddl_for(schemas.get("gate").unwrap())).unwrap();
        conn.execute_batch(&ddl_for(schemas.get("tasks").unwrap())).unwrap();

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
        let report = apply_with(&conn, &schemas, &manifest).expect("apply_with should succeed");
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

    /// Pi ruling (msg_77a03121): adversarial fixture — origin_db='prod' with BOTH
    /// *_source_id NULL → migration produces canonical (NULL, NULL), origin_db
    /// preserved.  Coherent rows (prod/42, sandbox/99) still migrate correctly.
    #[test]
    fn t084_r0_followup_origin_only_canonical_null_null_origin_db_preserved() {
        let (schemas, manifest) = load_bundled();
        let conn = Connection::open_in_memory().unwrap();
        install_pre_t084_all(&conn, &schemas);

        // Adversarial: origin_db='prod', prod_source_id=NULL, sandbox_source_id=NULL.
        insert_pre_t084_source_row(&conn, "L930", "prod", None, None);
        // Coherent prod row.
        insert_pre_t084_source_row(&conn, "L931", "prod", Some(42), None);
        // Coherent sandbox row.
        insert_pre_t084_source_row(&conn, "L932", "sandbox", None, Some(99));

        apply_with(&conn, &schemas, &manifest).expect("apply_with must succeed");

        // Origin-only: canonical (NULL, NULL); origin_db still 'prod'.
        let r930: (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_env, source_id, origin_db FROM observations WHERE display_id = 'L930'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (r930.0.as_deref(), r930.1.as_deref()),
            (None, None),
            "origin-only row L930 must have canonical (NULL, NULL); got {:?}/{:?}",
            r930.0, r930.1,
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
        assert_eq!(r932.2.as_deref(), Some("sandbox"), "L932 origin_db preserved");
    }
}
