/// `next-id` handler — scan canonical task directories and print the next available `T###` ID.
///
/// # Scan root
/// Resolved as `cwd/tasks/` at call time. Callers (project scripts) are expected to
/// invoke `stores` from the project root; the verb does not walk up the directory tree.
///
/// # Canonical subdirectories
/// Only the five directories listed in `tasks/CLAUDE.md` are scanned:
/// `active`, `planning`, `paused`, `completed`, `archived`.
/// Non-canonical directories (e.g. `ongoing/`) are silently ignored.
///
/// # Leniency
/// Missing directories are treated as empty — no error is returned if fewer than
/// five canonical subdirs exist.
///
/// # Output
/// Exactly `T{:03}` followed by a newline. No other output or side effects.
use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use std::path::Path;
use std::sync::OnceLock;

use crate::codegen::ddl::quote_ident;
use crate::{id_format, schema::Schema};

/// Cached regex: matches `T` followed by 3+ digits, then either `-` or end-of-string.
/// Captures the numeric portion in group 1.
fn task_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^T(\d{3,})(-|$)").expect("task_id_re is valid"))
}

/// Public entry point for `tasks next-id`: resolve cwd + `tasks/`, delegate to pure inner
/// function, print result.
pub fn run_next_id() -> Result<()> {
    let root = std::env::current_dir()?.join("tasks");
    let next = next_id_for_root(&root)?;
    println!("{}", next);
    Ok(())
}

/// Public entry point for table-backed stores: scan existing `display_id` values in the
/// store table and print max+1 according to the schema's `id_format`.
pub fn run_store_next_id(schema: &Schema, conn: &Connection) -> Result<()> {
    let next = next_id_for_store(schema, conn)?;
    println!("{}", next);
    Ok(())
}

/// Pure inner function: scan `root/{active,planning,paused,completed,archived}/` for the
/// highest `T###` directory name and return the next ID formatted as `T{:03}`.
///
/// Missing subdirectories are silently skipped. Non-canonical subdirectories are ignored.
pub(crate) fn next_id_for_root(root: &Path) -> Result<String> {
    const CANONICAL_SUBDIRS: &[&str] = &["active", "planning", "paused", "completed", "archived"];

    let re = task_id_re();
    let mut max: i64 = 0;

    for subdir_name in CANONICAL_SUBDIRS {
        let subdir = root.join(subdir_name);
        if !subdir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&subdir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(caps) = re.captures(&name_str) {
                if let Ok(n) = caps[1].parse::<i64>() {
                    if n > max {
                        max = n;
                    }
                }
            }
        }
    }

    Ok(id_format::render("T{:03d}", max + 1))
}

/// Pure inner function for DB-backed stores. Ignores malformed display IDs so a stray row
/// cannot make the read-only helper fail; valid IDs determine max+1.
pub(crate) fn next_id_for_store(schema: &Schema, conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(&format!(
        "SELECT display_id FROM {} WHERE display_id IS NOT NULL",
        quote_ident(&schema.name)
    ))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;

    let mut max = 0_i64;
    for row in rows {
        let display_id = row?;
        if let Ok(n) = id_format::parse(&schema.id_format, &display_id) {
            if n > max {
                max = n;
            }
        }
    }

    Ok(id_format::render(&schema.id_format, max + 1))
}

#[cfg(test)]
mod tests {
    use super::{next_id_for_root, next_id_for_store};
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::fs;
    use tempfile::tempdir;

    fn observations_schema() -> Schema {
        Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap()
    }

    fn observations_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE observations (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT)",
            [],
        )
        .unwrap();
        conn
    }

    fn make_dir(base: &std::path::Path, rel: &str) {
        fs::create_dir_all(base.join(rel)).expect("create_dir_all");
    }

    fn make_entry(base: &std::path::Path, rel: &str) {
        // For files, just create the file; for directories, create_dir_all.
        // Detect by whether the final component has an extension (rough heuristic for tests).
        let path = base.join(rel);
        if rel.contains('.') {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, "").expect("write file");
        } else {
            fs::create_dir_all(&path).expect("create_dir_all");
        }
    }

    #[test]
    fn empty_tree_returns_t001() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        fs::create_dir_all(&tasks).unwrap();
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T001");
    }

    #[test]
    fn single_completed_t005_returns_t006() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        make_dir(&tasks, "completed/T005-foo");
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T006");
    }

    #[test]
    fn highest_in_archived_wins() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        make_dir(&tasks, "archived/T010-x");
        make_dir(&tasks, "completed/T003-y");
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T011");
    }

    #[test]
    fn missing_directories_treated_as_empty() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        // Only `planning/` exists — the other four canonical dirs do not.
        make_dir(&tasks, "planning/T007-z");
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T008");
    }

    #[test]
    fn non_task_entries_ignored() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        // README.md — file (has extension)
        make_entry(&tasks, "completed/README.md");
        // T004-real — valid task dir
        make_dir(&tasks, "completed/T004-real");
        // notes/ — directory without T### prefix
        make_dir(&tasks, "completed/notes");
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T005");
    }

    #[test]
    fn non_canonical_directories_ignored() {
        let tmp = tempdir().unwrap();
        let tasks = tmp.path().join("tasks");
        // `ongoing/` is non-canonical per tasks/CLAUDE.md; must not be counted.
        make_dir(&tasks, "ongoing/T999-x");
        // Only active/ has a real task at T002 — that becomes the max.
        make_dir(&tasks, "active/T002-real");
        assert_eq!(next_id_for_root(&tasks).unwrap(), "T003");
    }

    #[test]
    fn observations_fresh_db_returns_l001() {
        let schema = observations_schema();
        let conn = observations_conn();
        assert_eq!(next_id_for_store(&schema, &conn).unwrap(), "L001");
    }

    #[test]
    fn observations_existing_rows_return_max_plus_one() {
        let schema = observations_schema();
        let conn = observations_conn();
        conn.execute(
            "INSERT INTO observations (display_id) VALUES ('L001'), ('L003'), ('L010')",
            [],
        )
        .unwrap();
        assert_eq!(next_id_for_store(&schema, &conn).unwrap(), "L011");
    }
}
