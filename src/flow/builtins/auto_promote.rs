//! `builtin:auto-promote` — observations confirmed→ready creates tasks row.
//!
//! Subscribes to the post-ratify transition (observations: confirmed→ready
//! fired by `maybe_auto_ratify_observation`). Reads the observation's
//! `intent_contract`, mints a tasks row at `planning` with
//! `linked_observations` populated and `contract.{done_when,scope_in,scope_out}`
//! mapped from the observation's contract, then back-links
//! `observation.task_id` to the new task.
//!
//! Idempotent: if any tasks row already lists this obs in
//! `linked_observations`, the run is a no-op.
//!
//! Invoker for the tasks insert is `ai_autonomous` — the U-moment that
//! grounds the row was the human's ratify (Phase 1); auto-promote is
//! engine activity, not a fresh authority moment.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::row::now_iso8601;
use crate::id_format;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let obs_display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if obs_display_id.is_empty() {
        eprintln!("[auto-promote] observation row missing display_id; skipping");
        return Ok(1);
    }

    // Idempotency guard: if any tasks row already lists this obs in
    // linked_observations, treat as already promoted.
    let already_promoted: bool = ctx
        .conn
        .query_row(
            "SELECT 1 FROM tasks, json_each(tasks.linked_observations) je \
             WHERE je.value = ?1 LIMIT 1",
            rusqlite::params![obs_display_id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if already_promoted {
        eprintln!(
            "[auto-promote] {}: already promoted (linked_observations match); skipping",
            obs_display_id
        );
        return Ok(0);
    }

    // Parse intent_contract (stored as JSON text in the observations row).
    let ic_value: Value = match row.get("intent_contract") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };

    let state = ic_value
        .get("contract_state")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if state != "ready" {
        eprintln!(
            "[auto-promote] {}: intent_contract.contract_state != 'ready' (got '{}'); skipping",
            obs_display_id, state
        );
        return Ok(1);
    }

    let objective = ic_value
        .get("objective")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let in_scope = collect_text_list(ic_value.get("in_scope"));
    let out_of_scope = collect_text_list(ic_value.get("out_of_scope"));
    let acceptance = collect_text_list(ic_value.get("acceptance"));
    let tier_hint = ic_value
        .get("tier_hint")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let summary = row
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or(obs_display_id)
        .to_string();

    // Mapping (Decision Matrix Q3).
    let done_when = if acceptance.is_empty() {
        objective.clone()
    } else {
        format!(
            "{}\n\nAcceptance:\n- {}",
            objective,
            acceptance.join("\n- ")
        )
    };
    let scope_in = format_bullets(&in_scope);
    let scope_out = format_bullets(&out_of_scope);

    let title = summary.clone();
    let slug = format!("auto-promoted-{}", obs_display_id.to_lowercase());

    let contract = serde_json::json!({
        "done_when": done_when,
        "scope_in": scope_in,
        "scope_out": scope_out,
    });
    let linked = serde_json::json!([obs_display_id]);

    match promote(
        ctx.conn,
        obs_display_id,
        &title,
        &slug,
        &tier_hint,
        &contract,
        &linked,
    ) {
        Ok(new_task_id) => {
            eprintln!(
                "[auto-promote] {}: promoted to {} (planning)",
                obs_display_id, new_task_id
            );
            Ok(0)
        }
        Err(e) => {
            eprintln!(
                "[auto-promote] {}: failed to promote: {:#}",
                obs_display_id, e
            );
            Ok(1)
        }
    }
}

fn collect_text_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Value::String(s)) => {
            // Substrate stores JSON-typed list fields as TEXT (serialized JSON).
            serde_json::from_str::<Value>(s)
                .ok()
                .and_then(|jv| match jv {
                    Value::Array(items) => Some(
                        items
                            .into_iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn format_bullets(items: &[String]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!("- {}", items.join("\n- "))
    }
}

/// Insert the tasks row + back-link the observation in a single transaction.
/// Returns the minted T-id.
fn promote(
    conn: &Connection,
    obs_display_id: &str,
    title: &str,
    slug: &str,
    tier_hint: &str,
    contract: &Value,
    linked_observations: &Value,
) -> Result<String> {
    let now = now_iso8601();
    let tx = conn.unchecked_transaction()?;

    // Direct INSERT into tasks bypassing the validator: this is engine
    // activity (auto-promote), grounded by the prior human ratify; the
    // ai_with_human-actor fields (title, slug) are written under the
    // 'ai_autonomous' invoker label because the U-moment was upstream.
    let tier_val: rusqlite::types::Value = if tier_hint.is_empty() {
        rusqlite::types::Value::Null
    } else {
        rusqlite::types::Value::Text(tier_hint.to_string())
    };

    tx.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, tier_hint, contract, linked_observations, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'planning', ?2, ?3, ?4, ?5, ?6, ?7, ?7, 'ai_autonomous', 'ai_autonomous')",
        rusqlite::params![
            "__PLACEHOLDER__",
            title,
            slug,
            tier_val,
            serde_json::to_string(contract).unwrap_or_else(|_| "null".to_string()),
            serde_json::to_string(linked_observations).unwrap_or_else(|_| "null".to_string()),
            now,
        ],
    )
    .context("INSERT INTO tasks (auto-promote)")?;

    let rowid = tx.last_insert_rowid();
    let new_display_id = id_format::render("T{:03d}", rowid);

    tx.execute(
        "UPDATE tasks SET display_id = ?1 WHERE id = ?2",
        rusqlite::params![new_display_id, rowid],
    )
    .context("UPDATE tasks display_id (auto-promote)")?;

    // Synthetic 'create' transition row for tasks (mirrors handlers/add.rs).
    crate::db::insert_transition_history(
        &tx,
        "tasks",
        rowid,
        &new_display_id,
        "",
        "planning",
        "create",
        "ai_autonomous",
        None,
        None,
    )?;

    // Back-link the observation. task_id is a non-status text field with no
    // actor restriction, so a direct UPDATE is correct (no transition needed).
    tx.execute(
        "UPDATE observations SET task_id = ?1, updated_at = ?2 WHERE display_id = ?3",
        rusqlite::params![new_display_id, now, obs_display_id],
    )
    .context("UPDATE observations.task_id (auto-promote back-link)")?;

    // Fire on-entry follow-ons for the planning state (L117). Without this,
    // tier-conditional actions declared on tasks.workflow.on_state.planning
    // never evaluate — most visibly, T1 rows stay at planning and `tasks
    // drive` errors with `next-action returned no agent for status 'planning'`
    // because the schema's `{transition_to: ready, when: tier_hint == 'T1'}`
    // never fires. submit handlers all call this; auto-promote was the gap.
    let tasks_schema = crate::flow::builtins::load_tasks_schema()
        .context("auto-promote: load tasks schema for on-entry hooks")?;
    crate::handlers::submit::fire_on_entry_follow_ons(
        &tx,
        &tasks_schema,
        &new_display_id,
        rowid,
        "planning",
    )
    .context("auto-promote: fire on-entry hooks for planning state")?;

    tx.commit().context("commit auto-promote")?;
    Ok(new_display_id)
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

    fn insert_ready_obs(conn: &Connection, display_id: &str) {
        let ic = serde_json::json!({
            "contract_state": "ready",
            "objective": "fix the X bug",
            "type": "work",
            "in_scope": ["fix module A", "add test"],
            "out_of_scope": ["refactor B"],
            "acceptance": ["test_x passes", "no regressions"],
            "tier_hint": "T3",
            "approved_by": "blake",
            "approved_at": "2026-05-03T00:00:00Z",
        });
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              intent_contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'ready', 'auto-promote candidate', 'dev', 'normal', ?2, 'w-test', ?3, ?2, ?2, 'human', 'human')",
            rusqlite::params![
                display_id,
                "2026-05-03T00:00:00Z",
                serde_json::to_string(&ic).unwrap(),
            ],
        )
        .unwrap();
    }

    fn obs_row_json(conn: &Connection, display_id: &str) -> Value {
        let mut stmt = conn
            .prepare("SELECT * FROM observations WHERE display_id = ?1")
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

    fn ctx_for<'a>(conn: &'a Connection, agents: &'a AgentsYaml, cfg: &'a std::path::Path) -> DispatchCtx<'a> {
        DispatchCtx {
            conn,
            agents,
            config_path: cfg,
            policies_hash: "",
        }
    }

    #[test]
    fn promote_creates_task() {
        let conn = fresh_db();
        insert_ready_obs(&conn, "L001");
        let row = obs_row_json(&conn, "L001");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);

        let res = run(&row, &ctx).unwrap();
        assert_eq!(res, 0);

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "exactly one tasks row created");

        let (task_id, status, contract_str, linked_str, tier): (String, String, String, String, Option<String>) = conn
            .query_row(
                "SELECT display_id, status, contract, linked_observations, tier_hint FROM tasks LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(status, "planning");
        assert_eq!(tier.as_deref(), Some("T3"));

        let contract: Value = serde_json::from_str(&contract_str).unwrap();
        let done_when = contract.get("done_when").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            done_when.contains("fix the X bug"),
            "done_when must carry objective; got: {done_when}"
        );
        assert!(
            done_when.contains("test_x passes"),
            "done_when must include acceptance; got: {done_when}"
        );

        let linked: Value = serde_json::from_str(&linked_str).unwrap();
        assert_eq!(linked, serde_json::json!(["L001"]));

        let obs_task_id: String = conn
            .query_row(
                "SELECT task_id FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(obs_task_id, task_id, "obs.task_id back-linked to new task");
    }

    #[test]
    fn promote_is_idempotent() {
        let conn = fresh_db();
        insert_ready_obs(&conn, "L001");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        // First run.
        let row1 = obs_row_json(&conn, "L001");
        run(&row1, &ctx_for(&conn, &agents, &cfg)).unwrap();
        let n1: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n1, 1);

        // NULL out observations.task_id to prove the second-run skip is
        // driven by linked_observations match, not by the back-link.
        conn.execute(
            "UPDATE observations SET task_id = NULL WHERE display_id = 'L001'",
            [],
        )
        .unwrap();

        // Second run — observation no longer has task_id, but the tasks row's
        // linked_observations still contains 'L001'.
        let row2 = obs_row_json(&conn, "L001");
        let res = run(&row2, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 1, "second run must not create a duplicate tasks row");
    }

    #[test]
    fn promote_skips_surfacing_task_id_pattern_is_actually_promoted() {
        // Regression for L063: an obs filed with --task-id <surfacing-task>
        // (a pre-existing task that does NOT list the obs in its
        // linked_observations) must still be promoted, not skipped.
        let conn = fresh_db();

        // Pre-existing surfacing task T-foo, with EMPTY linked_observations.
        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, contract, linked_observations, \
              created_at, updated_at, created_by, updated_by) \
             VALUES ('T-foo', 'planning', 'pre-existing', 'pre-existing', '{}', '[]', \
                     ?1, ?1, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params!["2026-05-03T00:00:00Z"],
        )
        .unwrap();

        // Obs L010 filed with task_id pointing at T-foo (the surfacing task).
        insert_ready_obs(&conn, "L010");
        conn.execute(
            "UPDATE observations SET task_id = 'T-foo' WHERE display_id = 'L010'",
            [],
        )
        .unwrap();

        let row = obs_row_json(&conn, "L010");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0, "auto-promote must succeed");

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "a NEW tasks row must be created (T-foo + the new one)");

        let (new_task_id, linked_str): (String, String) = conn
            .query_row(
                "SELECT display_id, linked_observations FROM tasks \
                 WHERE display_id != 'T-foo' LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let linked: Value = serde_json::from_str(&linked_str).unwrap();
        assert_eq!(linked, serde_json::json!(["L010"]));

        let obs_task_id: String = conn
            .query_row(
                "SELECT task_id FROM observations WHERE display_id='L010'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            obs_task_id, new_task_id,
            "obs.task_id rewritten from T-foo to the new T-id"
        );
    }

    #[test]
    fn promote_is_idempotent_via_linked_observations_only() {
        // Proves the idempotency check no longer depends on
        // observations.task_id being present.
        let conn = fresh_db();
        insert_ready_obs(&conn, "L020");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        let row1 = obs_row_json(&conn, "L020");
        run(&row1, &ctx_for(&conn, &agents, &cfg)).unwrap();

        // NULL out observations.task_id between runs.
        conn.execute(
            "UPDATE observations SET task_id = NULL WHERE display_id = 'L020'",
            [],
        )
        .unwrap();

        let row2 = obs_row_json(&conn, "L020");
        let res = run(&row2, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "exactly one tasks row even without task_id back-link");
    }

    #[test]
    fn promote_rejects_non_ready_contract() {
        let conn = fresh_db();
        // Insert an obs with a draft contract (not ready).
        let ic = serde_json::json!({"contract_state": "draft"});
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              intent_contract, created_at, updated_at, created_by, updated_by) \
             VALUES ('L002', 'open', 'draft only', 'dev', 'normal', ?1, 'w-test', ?2, ?1, ?1, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![
                "2026-05-03T00:00:00Z",
                serde_json::to_string(&ic).unwrap(),
            ],
        )
        .unwrap();

        let row = obs_row_json(&conn, "L002");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 1);

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "no tasks row when contract is not ready");
    }

    /// Regression for L117: a T1 observation must land its promoted task at
    /// `executing`, not `planning`, because the planning state's on-entry
    /// action `{transition_to: ready, when: tier_hint == 'T1'}` cascades
    /// through `ready → executing` (ready's on-entry is unconditional
    /// `transition_to: executing`). Without firing on-entry, drive errors
    /// with `next-action returned no agent for status 'planning'`.
    #[test]
    fn t1_promotion_cascades_planning_to_executing_via_skip_plan() {
        let conn = fresh_db();
        let ic = serde_json::json!({
            "contract_state": "ready",
            "objective": "fix T1 thing",
            "type": "work",
            "in_scope": ["edit module A"],
            "out_of_scope": [],
            "acceptance": ["test passes"],
            "tier_hint": "T1",
            "approved_by": "blake",
            "approved_at": "2026-05-06T00:00:00Z",
        });
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              intent_contract, created_at, updated_at, created_by, updated_by) \
             VALUES ('L117', 'ready', 't1 candidate', 'dev', 'normal', ?1, 'w-test', ?2, ?1, ?1, 'human', 'human')",
            rusqlite::params!["2026-05-06T00:00:00Z", serde_json::to_string(&ic).unwrap()],
        )
        .unwrap();

        let row = obs_row_json(&conn, "L117");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let (status, tier): (String, Option<String>) = conn
            .query_row(
                "SELECT status, tier_hint FROM tasks LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(tier.as_deref(), Some("T1"));
        assert_eq!(
            status, "executing",
            "T1 task must cascade planning → ready → executing via on-entry follow-ons (L117)"
        );
    }

    /// Companion to the L117 regression: a T2 promotion MUST stay at planning
    /// (the on-entry action for planning is `dispatch_agent: planner` when
    /// tier_hint != T1; that action does not change status).
    #[test]
    fn t2_promotion_stays_at_planning_for_planner_dispatch() {
        let conn = fresh_db();
        let ic = serde_json::json!({
            "contract_state": "ready",
            "objective": "fix T2 thing",
            "type": "work",
            "in_scope": ["edit module A"],
            "out_of_scope": [],
            "acceptance": ["test passes"],
            "tier_hint": "T2",
            "approved_by": "blake",
            "approved_at": "2026-05-06T00:00:00Z",
        });
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, \
              intent_contract, created_at, updated_at, created_by, updated_by) \
             VALUES ('L118', 'ready', 't2 candidate', 'dev', 'normal', ?1, 'w-test', ?2, ?1, ?1, 'human', 'human')",
            rusqlite::params!["2026-05-06T00:00:00Z", serde_json::to_string(&ic).unwrap()],
        )
        .unwrap();

        let row = obs_row_json(&conn, "L118");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let (status, tier): (String, Option<String>) = conn
            .query_row(
                "SELECT status, tier_hint FROM tasks LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(tier.as_deref(), Some("T2"));
        assert_eq!(status, "planning", "T2 task must stay at planning (planner dispatches there)");
    }

    #[test]
    fn dispatch_builtin_returns_some_for_auto_promote() {
        let conn = fresh_db();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        // Pass a benign empty row — we only assert dispatch returns Some.
        let row = serde_json::json!({"display_id": ""});
        let res = crate::flow::builtins::dispatch_builtin("auto-promote", &row, &ctx);
        assert!(res.is_some(), "auto-promote keyword must resolve");
    }

}
