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

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::row::now_iso8601;

/// Items to symlink from main `.stores/` into the provisioned worktree's `.stores/`.
/// `logs/` stays worktree-local. `agents-daemon.pid` is omitted — only the main
/// daemon owns its pid file; symlinking would let a worktree shadow it.
const SYMLINK_ITEMS: &[&str] = &[
    "db.sqlite",
    "db.sqlite-shm",
    "db.sqlite-wal",
    "manifest.yaml",
    "agents.yaml",
    "config.yaml",
    "policies.yaml",
    "runs",
];

/// After a worktree is provisioned, link the substrate artifacts from the main
/// `.stores/` so `stores` verbs work from inside the worktree (closes L032/L067).
/// Idempotent: skips items already present in the worktree, and items missing
/// from main (e.g. WAL/SHM, optional config files). Failures are logged and
/// non-fatal — `workspace_path` is the primary deliverable.
fn symlink_substrate_into_worktree(
    worktree: &Path,
    main_stores_dir: &Path,
    display_id: &str,
) {
    let dst_dir = worktree.join(".stores");
    if let Err(e) = std::fs::create_dir_all(&dst_dir) {
        eprintln!(
            "[auto-scaffold] {}: failed to create worktree .stores/ dir: {}",
            display_id, e
        );
        return;
    }

    let main_canon = match main_stores_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[auto-scaffold] {}: failed to canonicalize main .stores/ ({}): {}",
                display_id,
                main_stores_dir.display(),
                e
            );
            return;
        }
    };

    for item in SYMLINK_ITEMS {
        let src = main_canon.join(item);
        let dst = dst_dir.join(item);
        if !src.exists() {
            continue;
        }
        if dst.exists() || dst.symlink_metadata().is_ok() {
            continue;
        }
        if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
            eprintln!(
                "[auto-scaffold] {}: symlink {} -> {} failed: {}",
                display_id,
                dst.display(),
                src.display(),
                e
            );
        }
    }
}

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

    if let Some(main_stores_dir) = ctx.config_path.parent() {
        symlink_substrate_into_worktree(&canon, main_stores_dir, display_id);
    }

    let resolved_branch = resolve_branch_from_worktree(&canon).unwrap_or_else(|e| {
        eprintln!(
            "[auto-scaffold] {}: branch resolve failed: {}; leaving tasks.branch untouched",
            display_id, e
        );
        String::new()
    });

    let now = now_iso8601();
    let canon_str = canon.to_string_lossy().to_string();
    if let Err(e) = ctx.conn.execute(
        "UPDATE tasks SET workspace_path = ?1, \
                          branch = COALESCE(NULLIF(?2, ''), branch), \
                          updated_at = ?3 \
         WHERE display_id = ?4",
        rusqlite::params![canon_str, resolved_branch, now, display_id],
    ) {
        eprintln!(
            "[auto-scaffold] {}: UPDATE tasks.workspace_path failed: {}",
            display_id, e
        );
        return Ok(1);
    }

    if resolved_branch.is_empty() {
        eprintln!(
            "[auto-scaffold] {}: workspace_path = {} (branch unchanged — could not resolve from worktree HEAD)",
            display_id, canon_str
        );
    } else {
        eprintln!(
            "[auto-scaffold] {}: workspace_path = {}, branch = {}",
            display_id, canon_str, resolved_branch
        );
    }
    Ok(0)
}

/// Resolve the worktree's current branch via `git -C <wt> rev-parse --abbrev-ref HEAD`.
/// Returns empty string on detached HEAD or git failures (caller decides whether
/// to UPDATE; the COALESCE in the UPDATE means an empty string leaves the column
/// untouched). Closes L080.
fn resolve_branch_from_worktree(worktree: &Path) -> std::io::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Ok(String::new());
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name == "HEAD" || name.is_empty() {
        // Detached HEAD or empty — no branch to record.
        return Ok(String::new());
    }
    Ok(name)
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
    fn scaffold_symlinks_substrate_into_worktree() {
        let conn = fresh_db();
        insert_planning_task(&conn, "T104", None);

        let tmp = tempfile::tempdir().unwrap();
        // Treat tmp.path() as the "main .stores/" dir — that's the parent of cfg_path.
        let main_stores = tmp.path();
        std::fs::write(main_stores.join("db.sqlite"), b"main-db").unwrap();
        std::fs::write(main_stores.join("manifest.yaml"), b"main-manifest").unwrap();
        std::fs::write(main_stores.join("agents.yaml"), b"main-agents").unwrap();
        // Intentionally omit policies.yaml + WAL/SHM to confirm missing items are skipped.

        let target = main_stores.join("worktree-T104");
        let cfg_path = main_stores.join("config.yaml");
        write_scaffold_cfg(
            &cfg_path,
            &format!("mkdir -p {} && echo {}", target.display(), target.display()),
        );

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T104");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        let worktree_stores = target.join(".stores");
        assert!(
            worktree_stores.is_dir(),
            "worktree .stores/ dir must exist after scaffold"
        );

        for must_exist in ["db.sqlite", "manifest.yaml", "agents.yaml"] {
            let p = worktree_stores.join(must_exist);
            let meta = p.symlink_metadata().unwrap_or_else(|_| {
                panic!("expected symlink at {}", p.display())
            });
            assert!(
                meta.file_type().is_symlink(),
                "{} must be a symlink, got {:?}",
                p.display(),
                meta.file_type()
            );
            // Reading through the symlink yields the main file's content.
            assert!(
                std::fs::read(&p).unwrap().starts_with(b"main-"),
                "symlink {} did not resolve to main-side content",
                p.display()
            );
        }

        // policies.yaml was missing in main → must NOT be created in worktree.
        assert!(
            !worktree_stores.join("policies.yaml").exists(),
            "policies.yaml should have been skipped (missing in main)"
        );

        // logs/ is intentionally NOT in the symlink list (worktree-local).
        // It also wasn't created in main, so it should be absent here too.
        assert!(
            !worktree_stores.join("logs").exists(),
            "logs/ must not be symlinked"
        );
    }

    #[test]
    fn scaffold_symlink_is_idempotent_when_already_present() {
        // If the worktree's .stores/ already has an entry (e.g. logs/ created
        // by the scaffold command, or a prior auto-scaffold run), don't overwrite.
        let conn = fresh_db();
        insert_planning_task(&conn, "T105", None);

        let tmp = tempfile::tempdir().unwrap();
        let main_stores = tmp.path();
        std::fs::write(main_stores.join("db.sqlite"), b"main-db-v1").unwrap();

        let target = main_stores.join("worktree-T105");
        // Pre-create the worktree's .stores/db.sqlite as a regular file
        // (simulating something the scaffold command put there).
        std::fs::create_dir_all(target.join(".stores")).unwrap();
        std::fs::write(target.join(".stores/db.sqlite"), b"pre-existing").unwrap();

        let cfg_path = main_stores.join("config.yaml");
        write_scaffold_cfg(
            &cfg_path,
            &format!("mkdir -p {} && echo {}", target.display(), target.display()),
        );

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T105");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        // Pre-existing file must be preserved, not replaced by symlink.
        let p = target.join(".stores/db.sqlite");
        let meta = p.symlink_metadata().unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "pre-existing regular file must not be replaced by a symlink"
        );
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"pre-existing",
            "pre-existing content must be preserved"
        );
    }

    fn init_git_repo_on_branch(repo: &std::path::Path, branch: &str) {
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("git invocation");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        std::fs::create_dir_all(repo).unwrap();
        // Some CI envs have init.defaultBranch = main; force the branch we want.
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "-q", "-b", branch])
            .output()
            .expect("git init");
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    #[test]
    fn scaffold_resolves_and_writes_branch_from_worktree_head() {
        let conn = fresh_db();
        // Insert with empty branch — auto-scaffold should resolve it from the worktree.
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES ('T106', 'planning', 'test', 'tslug', '', NULL, ?1, ?2, ?2, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![contract, now],
        ).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("worktree-T106");
        init_git_repo_on_branch(&target, "feat/T106-resolved");

        let cfg_path = tmp.path().join("config.yaml");
        write_scaffold_cfg(&cfg_path, &format!("echo {}", target.display()));

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T106");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        let branch: String = conn
            .query_row(
                "SELECT branch FROM tasks WHERE display_id='T106'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(branch, "feat/T106-resolved");
    }

    #[test]
    fn scaffold_branch_resolve_does_not_clobber_preset_value() {
        // If branch was already set on the row (e.g. by tasks add), and the worktree
        // happens not to be a git repo (or rev-parse fails), the existing value
        // must be preserved by the COALESCE/NULLIF guard.
        let conn = fresh_db();
        // Existing branch value 'feat/tslug' from insert_planning_task helper.
        insert_planning_task(&conn, "T107", None);

        let tmp = tempfile::tempdir().unwrap();
        // Plain mkdir: NOT a git repo → resolve_branch returns empty.
        let target = tmp.path().join("worktree-T107");
        let cfg_path = tmp.path().join("config.yaml");
        write_scaffold_cfg(
            &cfg_path,
            &format!("mkdir -p {} && echo {}", target.display(), target.display()),
        );

        let agents = AgentsYaml::default_empty();
        let row = task_row_json(&conn, "T107");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg_path)).unwrap();
        assert_eq!(res, 0);

        let branch: String = conn
            .query_row(
                "SELECT branch FROM tasks WHERE display_id='T107'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            branch, "feat/tslug",
            "preset branch value must be preserved when resolve fails"
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
