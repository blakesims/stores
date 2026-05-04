//! `builtin:auto-scaffold` — tasks planning-arrival creates worktree + writes
//! `workspace_path`.
//!
//! Subscribes to the synthetic ''→planning edge fired by `auto-promote`.
//! Reads `.stores/config.yaml`'s `scaffold.command` template, substitutes
//! `{display_id}`, `{slug}`, `{branch}`, runs the command via `sh -c`, parses
//! the LAST non-empty stdout line as the absolute worktree path, and updates
//! `tasks.workspace_path`.
//!
//! Idempotent: if `workspace_path` is already set and the path exists as a
//! directory, the run is a no-op. If `scaffold.command` is unconfigured, the
//! run is a no-op (projects without scaffolding stay manual).
//!
//! Decision Matrix Q5: scaffold failures surface via stderr only. The row is
//! left at `planning`; recovery is out of scope per contract.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::row::now_iso8601;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[auto-scaffold] tasks row missing display_id; skipping");
        return Ok(1);
    }

    let slug = row.get("slug").and_then(|v| v.as_str()).unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !workspace_path.is_empty() && PathBuf::from(workspace_path).is_dir() {
        eprintln!(
            "[auto-scaffold] {}: workspace_path already set and exists; skipping",
            display_id
        );
        return Ok(0);
    }

    let cfg = match crate::flow::config::load(ctx.config_path) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            eprintln!(
                "[auto-scaffold] {}: no scaffold.command configured; skipping",
                display_id
            );
            return Ok(0);
        }
        Err(e) => {
            eprintln!(
                "[auto-scaffold] {}: failed to read config.yaml: {:#}; skipping",
                display_id, e
            );
            return Ok(0);
        }
    };

    let scaffold_cmd = match cfg.scaffold {
        Some(s) if !s.command.is_empty() => s.command,
        _ => {
            eprintln!(
                "[auto-scaffold] {}: no scaffold.command configured; skipping",
                display_id
            );
            return Ok(0);
        }
    };

    let substituted = scaffold_cmd
        .replace("{display_id}", display_id)
        .replace("{slug}", slug)
        .replace("{branch}", branch);

    let output = match Command::new("sh").arg("-c").arg(&substituted).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!(
                "[auto-scaffold] {}: failed to spawn scaffold command: {}",
                display_id, e
            );
            return Ok(1);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
        eprintln!(
            "[auto-scaffold] {}: scaffold command failed (status={:?}); stderr tail:\n{}",
            display_id,
            output.status.code(),
            tail
        );
        return Ok(1);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("")
        .to_string();
    if last_line.is_empty() {
        eprintln!(
            "[auto-scaffold] {}: scaffold command produced no stdout path",
            display_id
        );
        return Ok(1);
    }

    let path = PathBuf::from(&last_line);
    let canon = match path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[auto-scaffold] {}: scaffold path '{}' does not canonicalize: {}",
                display_id, last_line, e
            );
            return Ok(1);
        }
    };
    if !canon.is_dir() {
        eprintln!(
            "[auto-scaffold] {}: scaffold path '{}' is not a directory",
            display_id, last_line
        );
        return Ok(1);
    }

    let now = now_iso8601();
    let canon_str = canon.to_string_lossy().to_string();
    if let Err(e) = ctx.conn.execute(
        "UPDATE tasks SET workspace_path = ?1, updated_at = ?2 WHERE display_id = ?3",
        rusqlite::params![canon_str, now, display_id],
    ) {
        eprintln!(
            "[auto-scaffold] {}: UPDATE tasks.workspace_path failed: {}",
            display_id, e
        );
        return Ok(1);
    }

    eprintln!(
        "[auto-scaffold] {}: workspace_path = {}",
        display_id, canon_str
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::AgentsYaml;
    use crate::schema::Schema;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(tasks_yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn insert_planning_task(conn: &Connection, display_id: &str, workspace_path: Option<&str>) {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'planning', 'test', 'tslug', 'feat/tslug', ?2, ?3, ?4, ?4, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![display_id, workspace_path, contract, now],
        ).unwrap();
    }

    fn task_row_json(conn: &Connection, display_id: &str) -> Value {
        let mut stmt = conn
            .prepare("SELECT * FROM tasks WHERE display_id = ?1")
            .unwrap();
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(rusqlite::params![display_id]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i).unwrap();
            let jv = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => Value::from(n),
                rusqlite::types::Value::Real(f) => {
                    Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
                }
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(b) => {
                    Value::String(String::from_utf8_lossy(&b).to_string())
                }
            };
            obj.insert(name.clone(), jv);
        }
        Value::Object(obj)
    }

    fn ctx_for<'a>(
        conn: &'a Connection,
        agents: &'a AgentsYaml,
        cfg: &'a std::path::Path,
    ) -> DispatchCtx<'a> {
        DispatchCtx {
            conn,
            agents,
            config_path: cfg,
            policies_hash: "",
        }
    }

    fn write_scaffold_cfg(path: &std::path::Path, command: &str) {
        let yaml = format!("scaffold:\n  command: \"{}\"\n", command);
        std::fs::write(path, yaml).unwrap();
    }

    #[test]
    fn scaffold_writes_workspace_path() {
        let conn = fresh_db();
        insert_planning_task(&conn, "T100", None);

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("worktree-T100");
        let cfg_path = tmp.path().join("config.yaml");
        // Stub command: mkdir target then echo its path on stdout.
        write_scaffold_cfg(
            &cfg_path,
            &format!("mkdir -p {} && echo {}", target.display(), target.display()),
        );

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T100");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        let wp: String = conn
            .query_row(
                "SELECT workspace_path FROM tasks WHERE display_id='T100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let canon = target.canonicalize().unwrap().to_string_lossy().to_string();
        assert_eq!(wp, canon);
    }

    #[test]
    fn scaffold_is_idempotent() {
        let conn = fresh_db();
        insert_planning_task(&conn, "T101", None);

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("worktree-T101");
        let cfg_path = tmp.path().join("config.yaml");
        // Counter file: command increments each run; assert it ran exactly once.
        let counter = tmp.path().join("counter");
        let cmd = format!(
            "mkdir -p {0} && echo x >> {1} && echo {0}",
            target.display(),
            counter.display()
        );
        write_scaffold_cfg(&cfg_path, &cmd);

        let agents = AgentsYaml::default_empty();
        let row1 = task_row_json(&conn, "T101");
        run(&row1, &ctx_for(&conn, &agents, &cfg_path)).unwrap();

        // Re-fetch row (now has workspace_path set) and run again.
        let row2 = task_row_json(&conn, "T101");
        let res = run(&row2, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        // The scaffold command must have run only once.
        let runs = std::fs::read_to_string(&counter).unwrap();
        assert_eq!(
            runs.lines().count(),
            1,
            "scaffold command must run exactly once across two run() calls"
        );

        // workspace_path stays the canonicalized target.
        let wp: String = conn
            .query_row(
                "SELECT workspace_path FROM tasks WHERE display_id='T101'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let canon = target.canonicalize().unwrap().to_string_lossy().to_string();
        assert_eq!(wp, canon);
    }

    #[test]
    fn scaffold_missing_command_returns_ok_no_mutation() {
        let conn = fresh_db();
        insert_planning_task(&conn, "T102", None);

        let tmp = tempfile::tempdir().unwrap();
        // No config file at all.
        let cfg_path = tmp.path().join("config.yaml");

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T102");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        let wp: Option<String> = conn
            .query_row(
                "SELECT workspace_path FROM tasks WHERE display_id='T102'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            wp.as_deref().unwrap_or("").is_empty(),
            "workspace_path must remain unset; got: {:?}",
            wp
        );
    }

    #[test]
    fn scaffold_command_failure_returns_one_and_leaves_workspace_unset() {
        let conn = fresh_db();
        insert_planning_task(&conn, "T103", None);

        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");
        // A command that exits non-zero.
        write_scaffold_cfg(&cfg_path, "echo nope >&2 && exit 7");

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T103");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 1);

        let wp: Option<String> = conn
            .query_row(
                "SELECT workspace_path FROM tasks WHERE display_id='T103'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            wp.as_deref().unwrap_or("").is_empty(),
            "workspace_path must remain unset on failure; got: {:?}",
            wp
        );
    }

    #[test]
    fn dispatch_builtin_returns_some_for_auto_scaffold() {
        let conn = fresh_db();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        let row = serde_json::json!({"display_id": ""});
        let res = crate::flow::builtins::dispatch_builtin("auto-scaffold", &row, &ctx);
        assert!(res.is_some(), "auto-scaffold keyword must resolve");
    }
}
