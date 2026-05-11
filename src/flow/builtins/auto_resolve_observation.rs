//! `builtin:auto-resolve-observation` — close observations linked to a
//! successfully shipped task.
//!
//! T138 P3: subscribes to the integration-lane success edge
//! (`tasks: integrating → integrated`) — i.e. as soon as the candidate
//! lands on main — plus the legacy stores-specific terminal-success edge
//! (`tasks: cargo_installed → schema_migrated`) and every closed-out-of-band
//! edge. The post-T138 lane fires `mark_integrated` first; later
//! repo-specific subscribers (cargo-install / schema-migrate) chain off
//! the `integrated` state. Linked-observation resolution is now anchored
//! to the lane's success edge so it doesn't depend on repo-specific
//! post-`integrated` subscribers being configured.
//!
//! For every display id in `tasks.linked_observations`, marks the matching
//! observation as `status='resolved'` with `resolution=<task commit>`.
//! Already-resolved rows are skipped without mutation; missing observation
//! rows warn and ntfy but do not fail the subscriber.

use anyhow::Result;
use serde_json::Value;

use crate::flow::builtins::{
    fire_framework_transition_for, load_store_schema, BuiltinResult, DispatchCtx,
};
use crate::flow::NotifyEvent;
use crate::handlers::row::now_iso8601;
use crate::validate::EntryMap;

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
    let current: Option<(String, Option<String>, Option<String>)> = ctx
        .conn
        .query_row(
            "SELECT status, lifecycle, outcome FROM observations WHERE display_id = ?1",
            rusqlite::params![obs_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<String>>(1).ok().flatten(),
                    r.get::<_, Option<String>>(2).ok().flatten(),
                ))
            },
        )
        .optional()?;

    let Some((status, lifecycle, outcome)) = current else {
        return Ok(ResolveOutcome::Orphan);
    };
    if lifecycle.as_deref() == Some("closed") || outcome.as_deref() == Some("addressed") {
        return Ok(ResolveOutcome::AlreadyResolved);
    }

    // Route through the schema-declared `auto_resolve` transition (T037 P1
    // REVISE). The substrate gates the legal source states (open / investigating
    // / confirmed / ready / needs_info / in_progress); wont_fix and resolved
    // raise a "no transition" error which we surface as a soft skip.
    let schema = load_store_schema("observations")?;
    let mut diff: EntryMap = std::collections::BTreeMap::new();
    diff.insert(
        "resolution".to_string(),
        Value::String(resolution.to_string()),
    );
    diff.insert("resolved_at".to_string(), Value::String(now_iso8601()));

    fire_framework_transition_for(
        ctx.conn,
        &schema,
        obs_id,
        "auto_resolve",
        diff,
        ctx.policies_hash,
        None,
    )?;

    eprintln!(
        "[auto-resolve-observation] {}: {} resolved with {} (was {})",
        task_id, obs_id, resolution, status
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
        // T138 P3: the canonical success edge is now (integrating → integrated);
        // the cargo_installed→schema_migrated and accepted-entry edges remain
        // alternate triggers for legacy / repo-specific subscriber chains.
        transition_attempted: "tasks: integrating→integrated".to_string(),
        policy_id_or_actor_halt: "auto-resolve-observation: orphan linked_observations".to_string(),
        summary,
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);
}

use rusqlite::OptionalExtension;

/// Startup-sweep: replay `auto_resolve` for every terminal-success task row
/// (`status IN (schema_migrated, integrated, cargo_installed, accepted,
/// closed_out_of_band)`) that still has unresolved entries in
/// `linked_observations`. T138 P3 includes `integrated` and `cargo_installed`
/// here so the sweep catches rows that landed on main via the integration
/// lane but stranded mid-chain in the stores-specific post-`integrated`
/// subscribers. `accepted` is retained for legacy/handcrafted rows that
/// pre-date the lane. Idempotent — already-resolved obs are skipped via
/// `ResolveOutcome::AlreadyResolved`. Emits a per-row
/// `[startup-sweep] auto-resolve <task> → <obs> (was <prev>)` line for each
/// obs actually moved, plus the aggregate `[startup-sweep] resolved N linked obs`
/// summary. Errors per-task are logged and swallowed so the daemon proceeds.
pub fn startup_sweep(ctx: &DispatchCtx) -> Result<usize> {
    let mut stmt = ctx.conn.prepare(
        "SELECT t.display_id FROM tasks t \
         WHERE t.status IN ('schema_migrated','integrated','cargo_installed','accepted','closed_out_of_band') \
         AND t.linked_observations IS NOT NULL \
         AND t.linked_observations != '' \
         AND t.linked_observations != 'null' \
         AND t.linked_observations != '[]' \
         AND EXISTS ( \
            SELECT 1 FROM json_each(t.linked_observations) je \
            JOIN observations o ON o.display_id = je.value \
            WHERE COALESCE(o.lifecycle, '') != 'closed' \
              AND o.status != 'resolved' /* ADR 0002 compatibility-only T148 task 6.1 */ \
         )",
    )?;
    let task_ids: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut total_resolved = 0usize;
    for task_id in &task_ids {
        let pre = unresolved_linked_obs(ctx.conn, task_id);
        let Some(row) = crate::flow::builtins::refresh_task_row(ctx.conn, task_id) else {
            eprintln!(
                "[startup-sweep] {}: task row vanished mid-sweep; skipping",
                task_id
            );
            continue;
        };
        if let Err(e) = run(&row, ctx) {
            eprintln!("[startup-sweep] {}: subscriber errored: {:#}", task_id, e);
            continue;
        }
        for (obs_id, prev_status) in &pre {
            let now_resolved: bool = ctx
                .conn
                .query_row(
                    "SELECT status, lifecycle FROM observations WHERE display_id = ?1",
                    rusqlite::params![obs_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1).ok().flatten())),
                )
                .map(|(status, lifecycle)| lifecycle.as_deref() == Some("closed") || status == "resolved")
                .unwrap_or(false);
            if now_resolved {
                eprintln!(
                    "[startup-sweep] auto-resolve {} → {} (was {})",
                    task_id, obs_id, prev_status
                );
                total_resolved += 1;
            }
        }
    }

    eprintln!("[startup-sweep] resolved {} linked obs", total_resolved);
    Ok(total_resolved)
}

fn unresolved_linked_obs(
    conn: &rusqlite::Connection,
    task_display_id: &str,
) -> Vec<(String, String)> {
    let mut stmt = match conn.prepare(
        "SELECT o.display_id, o.status \
         FROM tasks t, json_each(t.linked_observations) je \
         JOIN observations o ON o.display_id = je.value \
         WHERE t.display_id = ?1 AND COALESCE(o.lifecycle, '') != 'closed' AND o.status != 'resolved'\n         /* ADR 0002 compatibility-only T148 task 6.1: select legacy status for log text only. */",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(rusqlite::params![task_display_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

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
             (display_id, status, summary, source, priority, captured_at, captured_week, resolution, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'obs', 'dev', 'normal', '2026-05-03T00:00:00Z', 'w-test', ?3, '2026-05-03T00:00:00Z', '2026-05-03T00:00:00Z', 'human', 'human')",
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
        let cfg_dir = tempfile::tempdir().unwrap();
        let cfg = cfg_dir.path().join("config.yaml");
        std::fs::write(&cfg, "ntfy:\n  url: https://test.local\n").unwrap();
        let mock = install_mock();
        let conn = fresh_db();
        insert_obs(&conn, "L001", "open", None);
        let agents = AgentsYaml::default_empty();

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
        insert_obs(&conn, "L555", "ready", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let row = row(serde_json::json!(["L555"]), "deadbeef");
        let res = crate::flow::builtins::dispatch_builtin(
            "auto-resolve-observation",
            &row,
            &ctx(&conn, &agents, &cfg),
        )
        .expect("auto-resolve-observation keyword must resolve");
        let code = res.expect("dispatch must not error");
        assert_eq!(code, 0);

        let (status, resolution): (String, String) = conn
            .query_row(
                "SELECT status, resolution FROM observations WHERE display_id = 'L555'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(resolution, "deadbeef");
    }

    #[test]
    fn auto_resolve_writes_declared_transition_history_row() {
        let conn = fresh_db();
        insert_obs(&conn, "L046", "ready", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        run(
            &row(serde_json::json!(["L046"]), "abc1234"),
            &ctx(&conn, &agents, &cfg),
        )
        .unwrap();

        let (verb, from_status, to_status, invoker): (String, String, String, String) = conn
            .query_row(
                "SELECT verb, from_status, to_status, invoker \
                 FROM transition_history \
                 WHERE store = 'observations' AND display_id = 'L046' \
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(verb, "auto_resolve");
        assert_eq!(from_status, "ready");
        assert_eq!(to_status, "resolved");
        assert_eq!(invoker, "framework");
    }

    fn insert_shipped_task(conn: &Connection, display_id: &str, commit: &str, linked: &str) {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        let cycles = format!(r#"[{{"executor":{{"commit":"{}"}}}}]"#, commit);
        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, branch, workspace_path, contract, \
              linked_observations, cycles, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'schema_migrated', 't', 'ts', 'feat/ts', '/tmp/ws', ?2, ?3, ?4, ?5, ?5, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![display_id, contract, linked, cycles, now],
        )
        .unwrap();
    }

    #[test]
    fn startup_sweep_resolves_three_historically_shipped_tasks() {
        let conn = fresh_db();
        // Three schema_migrated tasks each linking one stale-ready observation.
        insert_obs(&conn, "L101", "ready", None);
        insert_obs(&conn, "L102", "ready", None);
        insert_obs(&conn, "L103", "ready", None);
        insert_shipped_task(&conn, "T101", "sha101", r#"["L101"]"#);
        insert_shipped_task(&conn, "T102", "sha102", r#"["L102"]"#);
        insert_shipped_task(&conn, "T103", "sha103", r#"["L103"]"#);

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let dctx = ctx(&conn, &agents, &cfg);

        let n = startup_sweep(&dctx).unwrap();
        assert_eq!(n, 3, "sweep must resolve all three stale linked obs");

        for (obs, expected_sha) in [("L101", "sha101"), ("L102", "sha102"), ("L103", "sha103")] {
            let (status, resolution): (String, String) = conn
                .query_row(
                    "SELECT status, resolution FROM observations WHERE display_id = ?1",
                    rusqlite::params![obs],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(status, "resolved", "{} must be resolved", obs);
            assert_eq!(resolution, expected_sha);
        }

        // Idempotent: re-running on already-resolved obs is a no-op.
        let n2 = startup_sweep(&dctx).unwrap();
        assert_eq!(n2, 0, "second sweep finds no work");
    }

    #[test]
    fn startup_sweep_skips_tasks_with_already_resolved_linked_obs() {
        let conn = fresh_db();
        insert_obs(&conn, "L200", "resolved", Some("oldsha"));
        insert_shipped_task(&conn, "T200", "newsha", r#"["L200"]"#);

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let n = startup_sweep(&ctx(&conn, &agents, &cfg)).unwrap();
        assert_eq!(n, 0);

        let resolution: String = conn
            .query_row(
                "SELECT resolution FROM observations WHERE display_id = 'L200'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolution, "oldsha", "already-resolved obs not overwritten");
    }

    #[test]
    fn auto_resolve_rejects_terminal_source_state() {
        // wont_fix is terminal; the schema declares no auto_resolve from there.
        // The subscriber must surface the rejection (soft failure code 1)
        // rather than silently force-flipping the row.
        let conn = fresh_db();
        insert_obs(&conn, "L800", "wont_fix", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        let code = run(
            &row(serde_json::json!(["L800"]), "abc1234"),
            &ctx(&conn, &agents, &cfg),
        )
        .unwrap();
        assert_eq!(code, 1, "wont_fix is terminal; auto_resolve must fail");

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L800'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "wont_fix", "row must not have been force-flipped");
    }

    fn insert_task_at_status(
        conn: &Connection,
        display_id: &str,
        status: &str,
        commit: &str,
        linked: &str,
    ) {
        let now = "2026-05-09T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        let cycles = format!(r#"[{{"executor":{{"commit":"{}"}}}}]"#, commit);
        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, branch, workspace_path, contract, \
              linked_observations, cycles, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 't', 'ts', 'feat/ts', '/tmp/ws', ?3, ?4, ?5, ?6, ?6, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![display_id, status, contract, linked, cycles, now],
        )
        .unwrap();
    }

    #[test]
    fn auto_resolve_fires_on_accepted_task() {
        let conn = fresh_db();
        insert_obs(&conn, "L210", "ready", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        let row = serde_json::json!({
            "display_id": "T210",
            "status": "accepted",
            "linked_observations": ["L210"],
            "cycles": [{"executor": {"commit": "sha-accepted"}}],
        });
        let code = run(&row, &ctx(&conn, &agents, &cfg)).unwrap();
        assert_eq!(code, 0);

        let (status, resolution): (String, String) = conn
            .query_row(
                "SELECT status, resolution FROM observations WHERE display_id = 'L210'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(resolution, "sha-accepted");
    }

    #[test]
    fn auto_resolve_fires_on_closed_out_of_band_task() {
        let conn = fresh_db();
        insert_obs(&conn, "L211", "ready", None);
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");

        let row = serde_json::json!({
            "display_id": "T211",
            "status": "closed_out_of_band",
            "linked_observations": ["L211"],
            "cycles": [{"executor": {"commit": "sha-oob"}}],
        });
        let code = run(&row, &ctx(&conn, &agents, &cfg)).unwrap();
        assert_eq!(code, 0);

        let (status, resolution): (String, String) = conn
            .query_row(
                "SELECT status, resolution FROM observations WHERE display_id = 'L211'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(resolution, "sha-oob");
    }

    #[test]
    fn startup_sweep_resolves_accepted_and_closed_out_of_band_cohorts() {
        let conn = fresh_db();
        // I024-class: one accepted task, one closed_out_of_band task, each with one unresolved obs.
        insert_obs(&conn, "L301", "ready", None);
        insert_obs(&conn, "L302", "ready", None);
        insert_task_at_status(&conn, "T301", "accepted", "sha-acc-301", r#"["L301"]"#);
        insert_task_at_status(
            &conn,
            "T302",
            "closed_out_of_band",
            "sha-oob-302",
            r#"["L302"]"#,
        );

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/stores-test-config.yaml");
        let dctx = ctx(&conn, &agents, &cfg);

        let n = startup_sweep(&dctx).unwrap();
        assert_eq!(n, 2, "sweep must resolve both I024-class obs");

        for (obs, sha) in [("L301", "sha-acc-301"), ("L302", "sha-oob-302")] {
            let (status, resolution): (String, String) = conn
                .query_row(
                    "SELECT status, resolution FROM observations WHERE display_id = ?1",
                    rusqlite::params![obs],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(status, "resolved", "{} must be resolved", obs);
            assert_eq!(resolution, sha, "{} resolution must match task commit", obs);
        }

        // Idempotent: second sweep returns 0.
        let n2 = startup_sweep(&dctx).unwrap();
        assert_eq!(n2, 0, "second sweep must be a no-op");
    }
}
