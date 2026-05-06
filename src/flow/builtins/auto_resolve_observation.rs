//! `builtin:auto-resolve-observation` — close observations linked to a
//! successfully shipped task.
//!
//! Subscribes to the terminal-success post-deploy transition
//! (`tasks: cargo_installed → schema_migrated`). For every display id in
//! `tasks.linked_observations`, marks the matching observation as
//! `status='resolved'` with `resolution=<task commit>`. Already-resolved rows
//! are skipped without mutation; missing observation rows warn and ntfy but do
//! not fail the subscriber.

use anyhow::Result;
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::flow::NotifyEvent;
use crate::handlers::row::now_iso8601;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let task_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    if task_id.is_empty() {
        eprintln!("[auto-resolve-observation] task row missing display_id; skipping");
        return Ok(1);
    }

    let commit = task_commit_resolution(row);
    if commit.is_empty() {
        eprintln!(
            "[auto-resolve-observation] {}: no executor commit found; skipping",
            task_id
        );
        return Ok(1);
    }

    let linked = collect_linked_observations(row.get("linked_observations"));
    if linked.is_empty() {
        eprintln!(
            "[auto-resolve-observation] {}: linked_observations empty; skipping",
            task_id
        );
        return Ok(0);
    }

    let mut resolved = 0usize;
    for obs_id in linked {
        match resolve_one(ctx, task_id, &obs_id, &commit) {
            Ok(ResolveOutcome::Resolved) => resolved += 1,
            Ok(ResolveOutcome::AlreadyResolved) => {}
            Ok(ResolveOutcome::Orphan) => warn_orphan(ctx, task_id, &obs_id),
            Err(e) => {
                eprintln!(
                    "[auto-resolve-observation] {}: failed resolving {}: {:#}",
                    task_id, obs_id, e
                );
                return Ok(1);
            }
        }
    }

    eprintln!(
        "[auto-resolve-observation] {}: resolved {} linked observation(s)",
        task_id, resolved
    );
    Ok(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveOutcome {
    Resolved,
    AlreadyResolved,
    Orphan,
}

fn collect_linked_observations(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| x.as_str().map(str::trim))
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|jv| match jv {
                Value::Array(items) => Some(
                    items
                        .into_iter()
                        .filter_map(|x| x.as_str().map(str::trim).map(ToOwned::to_owned))
                        .filter(|s| !s.is_empty())
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn task_commit_resolution(row: &Value) -> String {
    if let Some(s) = row.get("commit_sha").and_then(|v| v.as_str()) {
        if !s.trim().is_empty() {
            return s.trim().to_string();
        }
    }

    let cycles = match row.get("cycles") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    cycles
        .iter()
        .rev()
        .filter_map(|cycle| cycle.get("executor"))
        .filter_map(|exec| exec.get("commit"))
        .filter_map(|commit| commit.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn resolve_one(
    ctx: &DispatchCtx,
    task_id: &str,
    obs_id: &str,
    resolution: &str,
) -> Result<ResolveOutcome> {
    let current: Option<String> = ctx
        .conn
        .query_row(
            "SELECT status FROM observations WHERE display_id = ?1",
            rusqlite::params![obs_id],
            |r| r.get(0),
        )
        .optional()?;

    let Some(status) = current else {
        return Ok(ResolveOutcome::Orphan);
    };
    if status == "resolved" {
        return Ok(ResolveOutcome::AlreadyResolved);
    }

    let now = now_iso8601();
    let tx = ctx.conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE observations \
         SET status = 'resolved', resolution = ?1, resolved_at = ?2, updated_at = ?2, updated_by = 'ai_autonomous' \
         WHERE display_id = ?3 AND status != 'resolved'",
        rusqlite::params![resolution, now, obs_id],
    )?;

    let row_id: i64 = tx.query_row(
        "SELECT id FROM observations WHERE display_id = ?1",
        rusqlite::params![obs_id],
        |r| r.get(0),
    )?;
    crate::db::insert_transition_history(
        &tx,
        "observations",
        row_id,
        obs_id,
        &status,
        "resolved",
        "auto_resolve_observation",
        "ai_autonomous",
        None,
        None,
    )?;
    tx.commit()?;

    eprintln!(
        "[auto-resolve-observation] {}: {} resolved with {}",
        task_id, obs_id, resolution
    );
    Ok(ResolveOutcome::Resolved)
}

fn warn_orphan(ctx: &DispatchCtx, task_id: &str, obs_id: &str) {
    let summary = format!(
        "linked_observations entry '{}' on task '{}' has no matching observations row",
        obs_id, task_id
    );
    eprintln!("[auto-resolve-observation] warning: {summary}");
    let event = NotifyEvent {
        row_id: task_id.to_string(),
        transition_attempted: "tasks: cargo_installed→schema_migrated".to_string(),
        policy_id_or_actor_halt: "auto-resolve-observation: orphan linked_observations".to_string(),
        summary,
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::{install_notifier, AgentsYaml, MockNotifier, NotifierBackend};
    use crate::schema::Schema;
    use rusqlite::Connection;

    struct Shim {
        inner: &'static MockNotifier,
    }
    impl NotifierBackend for Shim {
        fn send(&self, url: &str, event: &crate::flow::NotifyEvent) -> Result<()> {
            self.inner.send(url, event)
        }
    }

    fn install_mock() -> &'static MockNotifier {
        let mock: &'static MockNotifier = Box::leak(Box::new(MockNotifier::new()));
        install_notifier(Box::new(Shim { inner: mock }));
        mock
    }

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        for name in ["tasks", "observations"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, y)| *y)
                .unwrap();
            let schema = Schema::from_yaml(yaml).unwrap();
            conn.execute_batch(&ddl_for(&schema)).unwrap();
        }
        conn
    }

    fn insert_obs(conn: &Connection, id: &str, status: &str, resolution: Option<&str>) {
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, resolution, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'obs', 'dev', 'normal', '2026-05-03T00:00:00Z', ?3, '2026-05-03T00:00:00Z', '2026-05-03T00:00:00Z', 'human', 'human')",
            rusqlite::params![id, status, resolution],
        )
        .unwrap();
    }

    fn row(linked: Value, commit: &str) -> Value {
        serde_json::json!({
            "display_id": "T020",
            "status": "schema_migrated",
            "linked_observations": linked,
            "cycles": [{"executor": {"commit": commit}}]
        })
    }

    fn ctx<'a>(
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

    #[test]
    fn resolves_all_linked_observations_with_executor_commit() {
        let conn = fresh_db();
        insert_obs(&conn, "L046", "ready", None);
        insert_obs(&conn, "L047", "open", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        let code = run(
            &row(serde_json::json!(["L046", "L047"]), "abc1234"),
            &ctx(&conn, &agents, &cfg),
        )
        .unwrap();
        assert_eq!(code, 0);

        for id in ["L046", "L047"] {
            let (status, resolution): (String, String) = conn
                .query_row(
                    "SELECT status, resolution FROM observations WHERE display_id = ?1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(status, "resolved");
            assert_eq!(resolution, "abc1234");
        }
    }

    #[test]
    fn already_resolved_is_not_overwritten() {
        let conn = fresh_db();
        insert_obs(&conn, "L001", "resolved", Some("oldsha"));
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        run(
            &row(serde_json::json!(["L001"]), "newsha"),
            &ctx(&conn, &agents, &cfg),
        )
        .unwrap();

        let resolution: String = conn
            .query_row(
                "SELECT resolution FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolution, "oldsha");
    }

    #[test]
    fn orphan_warns_and_notifies_without_failing() {
        let _g = crate::paths::test_notifier_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_NTFY_URL", "https://test.local");
        let mock = install_mock();
        let conn = fresh_db();
        insert_obs(&conn, "L001", "open", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        let code = run(
            &row(serde_json::json!(["L001", "L999"]), "abc1234"),
            &ctx(&conn, &agents, &cfg),
        )
        .unwrap();
        assert_eq!(code, 0);

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "resolved");
        assert!(mock.events().iter().any(|(_, e)| {
            e.row_id == "T020" && e.summary.contains("L999") && e.summary.contains("no matching")
        }));
        std::env::remove_var("STORES_NTFY_URL");
    }

    #[test]
    fn string_encoded_linked_observations_and_commit_sha_parse() {
        let conn = fresh_db();
        insert_obs(&conn, "L002", "open", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let row = serde_json::json!({
            "display_id": "T021",
            "commit_sha": "feedface",
            "linked_observations": "[\"L002\"]"
        });

        run(&row, &ctx(&conn, &agents, &cfg)).unwrap();

        let resolution: String = conn
            .query_row(
                "SELECT resolution FROM observations WHERE display_id = 'L002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolution, "feedface");
    }

    #[test]
    fn dispatch_keyword_resolves() {
        let conn = fresh_db();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let row = row(serde_json::json!([]), "abc1234");
        let res = crate::flow::builtins::dispatch_builtin(
            "auto-resolve-observation",
            &row,
            &ctx(&conn, &agents, &cfg),
        );
        assert!(
            res.is_some(),
            "auto-resolve-observation keyword must resolve"
        );
    }
}
