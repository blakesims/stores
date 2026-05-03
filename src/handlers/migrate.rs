use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::codegen::ddl::{expected_columns, quote_ident, ExpectedColumn};
use crate::manifest::Manifest;
use crate::schema::Schema;

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

/// Run `stores migrate` (DRY-RUN unless `apply`).
pub fn run_migrate(apply: bool) -> Result<()> {
    crate::paths::ensure_initialized()?;
    let manifest = Manifest::load()?;
    let schemas = load_schemas(&manifest)?;
    let conn = crate::db::open(&crate::paths::db_path()?)?;

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

    if plan.additive.is_empty() {
        return Ok(());
    }

    let mut sql_lines: Vec<String> = Vec::with_capacity(plan.additive.len());
    for (store, col) in &plan.additive {
        sql_lines.push(format!(
            "ALTER TABLE {} ADD COLUMN {};",
            quote_ident(store),
            col.full_def
        ));
    }

    for line in &sql_lines {
        println!("{line}");
    }

    if apply {
        let batch = format!("BEGIN;\n{}\nCOMMIT;", sql_lines.join("\n"));
        conn.execute_batch(&batch)
            .context("failed to apply additive migrations (transaction rolled back)")?;
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
    use crate::schema::{Schema, StoreScope};
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
        assert_eq!(plan.orphaned, vec![("gate".to_string(), "extra_thing".to_string())]);
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

    /// Type comparison is case-insensitive on the bare token.
    #[test]
    fn type_eq_is_case_insensitive() {
        assert!(type_eq("text", "TEXT"));
        assert!(type_eq("INTEGER", "integer"));
        assert!(!type_eq("TEXT", "INTEGER"));
    }
}
