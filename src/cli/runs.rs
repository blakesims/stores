use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTranscript {
    pub display_id: String,
    pub phase: i64,
    pub cycle: i64,
    pub role: String,
    pub path: PathBuf,
}

pub enum RunsCmd {
    List { display_id: String },
    Show {
        display_id: String,
        phase: i64,
        cycle: Option<i64>,
        role: String,
    },
}

pub fn run(cmd: RunsCmd) -> Result<()> {
    match cmd {
        RunsCmd::List { display_id } => {
            let stores_dir = crate::paths::stores_dir()?;
            let rows = list_for_task(&stores_dir, &display_id)?;
            println!("phase\tcycle\trole\ttranscript_path");
            for row in rows {
                println!(
                    "{}\t{}\t{}\t{}",
                    row.phase,
                    row.cycle,
                    row.role,
                    row.path.display()
                );
            }
            Ok(())
        }
        RunsCmd::Show {
            display_id,
            phase,
            cycle,
            role,
        } => {
            let stores_dir = crate::paths::stores_dir()?;
            let row = find_transcript(&stores_dir, &display_id, phase, cycle, &role)?;
            let read_path = resolve_transcript_path(&stores_dir, &row.path);
            let body = fs::read_to_string(&read_path).with_context(|| {
                format!(
                    "failed to read transcript {} (resolved to {})",
                    row.path.display(),
                    read_path.display()
                )
            })?;
            println!("{body}");
            Ok(())
        }
    }
}

pub fn list_for_task(stores_dir: &Path, display_id: &str) -> Result<Vec<RunTranscript>> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;

    // MAJOR 2 (T072 r4): use the guarded helper — creating the VIEW directly
    // here would bypass the tasks-table existence check and install an invalid
    // VIEW when tasks is not installed.  If tasks is absent, return a clean
    // error rather than a cryptic SQL error about a missing base table.
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !tasks_exists {
        anyhow::bail!("tasks store not installed; cannot query runs");
    }
    crate::db::ensure_runs_view_if_tasks_exists(&conn)
        .context("apply runs view DDL")?;

    // Query the runs VIEW — the substrate's official query surface for
    // (display_id, phase, cycle, role, transcript_path) tuples.  JSON decoding
    // stays inside SQLite (json_each / json_extract); Rust only handles rows.
    let mut stmt = conn
        .prepare(
            "SELECT phase, cycle, role, transcript_path \
             FROM runs \
             WHERE display_id = ?1 \
             ORDER BY phase, cycle, role",
        )
        .context("prepare runs view query")?;

    let view_rows: Vec<(i64, i64, String, String)> = stmt
        .query_map([display_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .context("query runs view")?
        .collect::<rusqlite::Result<_>>()
        .context("collect runs view rows")?;

    if view_rows.is_empty() {
        bail!("no transcript backlinks found for {display_id} in tasks.cycles");
    }

    let mut rows = Vec::new();
    for (phase, cycle, role, path_str) in view_rows {
        let path = PathBuf::from(&path_str);
        let read_path = resolve_transcript_path(stores_dir, &path);
        if !read_path.exists() {
            bail!(
                "missing transcript for {display_id} phase {phase} cycle {cycle} role {role}: {} does not exist (resolved to {})",
                path.display(),
                read_path.display()
            );
        }
        rows.push(RunTranscript {
            display_id: display_id.to_string(),
            phase,
            cycle,
            role,
            path,
        });
    }

    Ok(rows)
}

pub fn find_transcript(
    stores_dir: &Path,
    display_id: &str,
    phase: i64,
    cycle: Option<i64>,
    role: &str,
) -> Result<RunTranscript> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;

    // MAJOR 2 (T072 r4): use the guarded helper — do NOT create the VIEW
    // directly here (bypasses tasks-table existence check).  Clean error when
    // tasks store is absent.
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !tasks_exists {
        anyhow::bail!("tasks store not installed; cannot query runs");
    }
    crate::db::ensure_runs_view_if_tasks_exists(&conn)
        .context("apply runs view DDL")?;

    // MAJOR 1 (T072 r4): when --cycle is absent, default to the LATEST cycle
    // for the given phase+role (highest cycle number — deterministic DESC
    // ordering).  --cycle remains an optional disambiguator for callers that
    // want a specific historical cycle.
    //
    // When --cycle is provided: exact match (existing behaviour).
    // When --cycle is absent:  ORDER BY cycle DESC LIMIT 1 → latest cycle.
    let (p, c, r, path_str): (i64, i64, String, String) = if let Some(cyc) = cycle {
        // Exact-cycle path: check existence first for a clean error.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs \
                 WHERE display_id = ?1 AND phase = ?2 AND role = ?3 AND cycle = ?4",
                params![display_id, phase, role, cyc],
                |r| r.get(0),
            )
            .context("count matching runs rows (exact cycle)")?;
        if count == 0 {
            bail!(
                "missing transcript for {display_id} phase {phase} role {role} cycle {cyc}"
            );
        }
        conn.query_row(
            "SELECT phase, cycle, role, transcript_path \
             FROM runs \
             WHERE display_id = ?1 AND phase = ?2 AND role = ?3 AND cycle = ?4",
            params![display_id, phase, role, cyc],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .context("fetch matching runs row (exact cycle)")?
    } else {
        // Latest-cycle path: ORDER BY cycle DESC LIMIT 1.
        // Returns the highest cycle number — deterministic when multiple cycles
        // exist for the same phase+role (e.g. executor retry cycles).
        let result = conn
            .query_row(
                "SELECT phase, cycle, role, transcript_path \
                 FROM runs \
                 WHERE display_id = ?1 AND phase = ?2 AND role = ?3 \
                 ORDER BY cycle DESC LIMIT 1",
                params![display_id, phase, role],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            );
        match result {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                bail!(
                    "missing transcript for {display_id} phase {phase} role {role}"
                );
            }
            Err(e) => return Err(e).context("fetch latest runs row"),
        }
    };

    let path = PathBuf::from(&path_str);
    let read_path = resolve_transcript_path(stores_dir, &path);
    if !read_path.exists() {
        bail!(
            "missing transcript for {display_id} phase {p} cycle {c} role {r}: {} does not exist (resolved to {})",
            path.display(),
            read_path.display()
        );
    }

    Ok(RunTranscript {
        display_id: display_id.to_string(),
        phase: p,
        cycle: c,
        role: r,
        path,
    })
}


fn resolve_transcript_path(stores_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(stripped) = path.strip_prefix(".stores") {
        if let Some(root) = stores_dir.parent() {
            return root.join(".stores").join(stripped);
        }
    }
    stores_dir.join(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs")).unwrap();
        fs::write(
            stores.join("runs/executor-session.jsonl"),
            r#"{"role":"executor","summary":"fixture executor"}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/review-session.jsonl"),
            r#"{"role":"code-reviewer","gate":"PASS"}"#,
        )
        .unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        let cycles = serde_json::json!([
            {
                "phase": 2,
                "cycle": 2,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"}
            },
            {
                "phase": 2,
                "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"},
                "review": {"transcript_path": ".stores/runs/review-session.jsonl"}
            }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T999", serde_json::to_string(&cycles).unwrap()],
        )
        .unwrap();
        tmp
    }

    #[test]
    fn list_outputs_deterministic_order_from_cycle_backlinks() {
        let tmp = fixture();
        let rows = list_for_task(&tmp.path().join(".stores"), "T999").unwrap();
        let keys: Vec<_> = rows
            .iter()
            .map(|r| (r.phase, r.cycle, r.role.as_str(), r.path.to_string_lossy().to_string()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (
                    2,
                    1,
                    "code-reviewer",
                    ".stores/runs/review-session.jsonl".to_string()
                ),
                (
                    2,
                    1,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
                (
                    2,
                    2,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
            ]
        );
    }

    #[test]
    fn show_finds_existing_transcript_backlink() {
        let tmp = fixture();
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            Some(1),
            "executor",
        )
        .unwrap();
        assert_eq!(row.path, PathBuf::from(".stores/runs/executor-session.jsonl"));
    }

    #[test]
    fn missing_transcript_errors_cleanly() {
        let tmp = fixture();
        let err =
            find_transcript(&tmp.path().join(".stores"), "T999", 3, None, "executor").unwrap_err();
        assert!(err
            .to_string()
            .contains("missing transcript for T999 phase 3 role executor"));
    }

    #[test]
    fn missing_linked_file_errors_cleanly() {
        let tmp = fixture();
        fs::remove_file(tmp.path().join(".stores/runs/review-session.jsonl")).unwrap();
        let err = list_for_task(&tmp.path().join(".stores"), "T999").unwrap_err();
        assert!(err.to_string().contains(
            "missing transcript for T999 phase 2 cycle 1 role code-reviewer"
        ));
    }

    // ---- T072 r2: runs VIEW tests ----

    /// Build a minimal in-memory DB (tasks table + VIEW) and verify the VIEW
    /// exists and returns expected columns.  Uses rusqlite::Connection directly
    /// to isolate the DDL from the filesystem fixture.
    #[test]
    fn runs_view_exists_after_ddl_applied() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL)
            .expect("RUNS_VIEW_DDL must apply cleanly");
        // Verify the view exists via sqlite_master.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name='runs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "runs VIEW must exist after DDL is applied");
    }

    /// VIEW returns expected rows from a fixture cycles JSON blob.
    #[test]
    fn runs_view_returns_expected_rows() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        let cycles = serde_json::json!([
            {
                "phase": 1, "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/ex1.jsonl"},
                "review":   {"transcript_path": ".stores/runs/rv1.jsonl"}
            },
            {
                "phase": 2, "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/ex2.jsonl"}
            }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T001", serde_json::to_string(&cycles).unwrap()],
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT display_id, phase, cycle, role, transcript_path \
                 FROM runs WHERE display_id='T001' ORDER BY phase, cycle, role",
            )
            .unwrap();
        let rows: Vec<(String, i64, i64, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(rows.len(), 3, "expected 3 rows (2 executor + 1 reviewer)");
        assert_eq!(rows[0], ("T001".into(), 1, 1, "code-reviewer".into(), ".stores/runs/rv1.jsonl".into()));
        assert_eq!(rows[1], ("T001".into(), 1, 1, "executor".into(),      ".stores/runs/ex1.jsonl".into()));
        assert_eq!(rows[2], ("T001".into(), 2, 1, "executor".into(),      ".stores/runs/ex2.jsonl".into()));
    }

    /// Empty cycles array produces zero rows without error.
    #[test]
    fn runs_view_empty_cycles_no_error() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T002", "[]"],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE display_id='T002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "empty cycles must yield zero runs rows");
    }

    // ---- T072 r4: MAJOR 1 — latest-cycle default ----

    /// When --cycle is absent, `find_transcript` returns the row with the
    /// highest cycle number (cycle DESC ordering — deterministic).
    #[test]
    fn show_without_cycle_returns_latest_cycle() {
        let tmp = fixture();
        // fixture has phase=2 executor at cycle=1 AND cycle=2; cycle=2 is latest.
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            None,   // no --cycle: should default to latest (cycle=2)
            "executor",
        )
        .unwrap();
        assert_eq!(
            row.cycle, 2,
            "show without --cycle must return the highest cycle (latest)"
        );
        assert_eq!(row.path, PathBuf::from(".stores/runs/executor-session.jsonl"));
    }

    /// When --cycle is provided, `find_transcript` returns that exact cycle.
    #[test]
    fn show_with_explicit_cycle_returns_that_cycle() {
        let tmp = fixture();
        // fixture has executor at cycle=1 and cycle=2; request cycle=1 explicitly.
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            Some(1),    // explicit --cycle=1
            "executor",
        )
        .unwrap();
        assert_eq!(row.cycle, 1, "explicit --cycle must return that exact cycle");
        assert_eq!(row.path, PathBuf::from(".stores/runs/executor-session.jsonl"));
    }

    // ---- T072 r4: MAJOR 2 — tasks-absent clean error ----

    /// When the tasks table is absent, `list_for_task` returns a clean error
    /// rather than a cryptic SQL error about a missing base table.
    #[test]
    fn list_errors_cleanly_when_tasks_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(&stores).unwrap();
        // Open a DB with NO tasks table (substrate-only, or just empty).
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        drop(conn); // close; list_for_task opens its own connection

        let err = list_for_task(&stores, "T999").unwrap_err();
        assert!(
            err.to_string().contains("tasks store not installed"),
            "expected clean tasks-absent error, got: {err}"
        );
    }

    /// When the tasks table is absent, `find_transcript` returns a clean error.
    #[test]
    fn show_errors_cleanly_when_tasks_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(&stores).unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        drop(conn);

        let err = find_transcript(&stores, "T999", 1, None, "executor").unwrap_err();
        assert!(
            err.to_string().contains("tasks store not installed"),
            "expected clean tasks-absent error, got: {err}"
        );
    }

    /// VIEW is idempotent: applying RUNS_VIEW_DDL twice must not error.
    #[test]
    fn runs_view_ddl_is_idempotent() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        conn.execute_batch(RUNS_VIEW_DDL)
            .expect("applying RUNS_VIEW_DDL a second time must be a no-op (CREATE VIEW IF NOT EXISTS)");
    }
}
