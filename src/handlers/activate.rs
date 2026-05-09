//! T140 P1: per-row activation primitive (`tasks activate` / `tasks deactivate`).
//!
//! These verbs flip a tasks row's `activation` column without changing its
//! lifecycle `status`. They are tier-B writes (`actor: ai_with_human`) — the
//! schema's actor gate on the `activation` field rejects `ai_autonomous`
//! invokers; this handler defers the actor decision to `validate::validate`
//! (Op::Update with the diff containing only `activation`).
//!
//! Each successful flip writes a `transition_history` row with
//! `verb='activate'` or `verb='deactivate'`, `from_status` == `to_status` ==
//! current lifecycle status (no lifecycle change), and
//! `actor_note=<reason>` so the operator's grounding is durable.
//!
//! P1 ships only the column and these verbs; combustion-class subscribers do
//! NOT yet read `activation` (P2 wires that gating).

use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{actor::InvokerCtx, Schema};
use crate::validate::{self, Op};

use super::row::{now_iso8601, read_row};
use super::transition::read_policy_env;

/// Run the `tasks activate <id> --reason <text>` verb.
pub fn run_activate(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    run_set_activation(schema, conn, matches, invoker, "active", "activate")
}

/// Run the `tasks deactivate <id> --reason <text>` verb.
pub fn run_deactivate(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    run_set_activation(schema, conn, matches, invoker, "inactive", "deactivate")
}

fn run_set_activation(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    new_value: &str,
    verb: &str,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");
    let reason = matches
        .get_one::<String>("reason")
        .ok_or_else(|| anyhow::anyhow!("--reason is required for {verb}"))?;
    if reason.trim().is_empty() {
        anyhow::bail!("--reason must be a non-empty string");
    }

    let tx = conn
        .unchecked_transaction()
        .with_context(|| format!("{verb}: begin tx"))?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Build a diff containing only the activation field. The schema's actor
    // gate (actor: ai_with_human on `activation`) is enforced by validate
    // against this diff: ai_autonomous is rejected.
    let mut diff: crate::validate::EntryMap = std::collections::BTreeMap::new();
    diff.insert(
        "activation".to_string(),
        Value::String(new_value.to_string()),
    );

    let mut merged = existing.clone();
    merged.insert(
        "activation".to_string(),
        Value::String(new_value.to_string()),
    );

    validate::validate(schema, &merged, Op::Update(diff.clone()), invoker)
        .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // Single-column UPDATE. No lifecycle status change.
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();
    let qtable = quote_ident(&schema.name);
    tx.execute(
        &format!(
            "UPDATE {qtable} SET activation = ?1, updated_at = ?2, updated_by = ?3 \
             WHERE id = ?4"
        ),
        rusqlite::params![new_value, &now, &invoker_str, row_id],
    )
    .with_context(|| format!("{verb}: update row"))?;

    // transition_history: verb=activate|deactivate, from_status==to_status==current_status,
    // actor_note=<reason>. The audit row is the durable record of the operator's grounding.
    let (pref, phash) = read_policy_env();
    crate::db::insert_transition_history_with_note(
        &tx,
        &schema.name,
        row_id,
        display_id,
        &current_status,
        &current_status,
        verb,
        &invoker_str,
        pref.as_deref(),
        phash.as_deref(),
        Some(reason),
    )?;

    tx.commit().with_context(|| format!("{verb}: commit tx"))?;
    println!(
        "{verb} {display_id}: activation={new_value} (status={current_status} unchanged)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! T140 P1: lib-level tests for the per-row activation primitive. Covers
    //! the seven Phase-1 acceptance criteria:
    //!   AC1.3 — fresh-DB default is 'inactive'.
    //!   AC1.4 — framework-migrate parametrised backfill: IN_FLIGHT_STATES →
    //!           'active', everything else → 'inactive'.
    //!   AC1.5 — `tasks activate T### --reason r --invoker ai_with_human`
    //!           flips activation to 'active' and writes
    //!           transition_history(verb='activate', actor_note='r').
    //!   AC1.6 — ai_autonomous activate is rejected fail-loud and the row's
    //!           activation column is not mutated.
    //!   AC1.7 — `tasks activate T###` without --reason errors at clap parse.
    //!   plus a deactivate symmetry test and an idempotency test for
    //!   running framework-migrate twice.

    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::handlers::framework_migrate::{apply_framework_drift, IN_FLIGHT_STATES};
    use crate::schema::actor::Actor;
    use crate::schema::Schema;
    use clap::{Arg, ArgMatches, Command};
    use rusqlite::Connection;

    fn tasks_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, y)| *y)
            .expect("bundled tasks schema present");
        Schema::from_yaml(yaml).expect("tasks schema parses")
    }

    fn fresh_db_with_tasks() -> (Schema, Connection) {
        let schema = tasks_schema();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        (schema, conn)
    }

    /// Build a DB whose `tasks` table predates the activation column. Mirrors
    /// a pre-T140 DB: ddl_for(schema) creates the modern shape; we then DROP
    /// activation to simulate an older binary's schema.
    fn pre_t140_db() -> (Schema, Connection) {
        let (schema, conn) = fresh_db_with_tasks();
        conn.execute_batch("ALTER TABLE \"tasks\" DROP COLUMN \"activation\";")
            .expect("DROP activation simulates pre-T140 DB");
        (schema, conn)
    }

    fn insert_minimal_task(conn: &Connection, display_id: &str, status: &str) {
        let now = "2026-05-09T00:00:00Z";
        let slug = format!("task-{}", display_id.to_ascii_lowercase());
        let contract = r#"{"done_when":"d","scope_in":"i","scope_out":"o"}"#;
        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'framework', 'framework')",
            rusqlite::params![
                display_id,
                status,
                format!("task {display_id}"),
                slug,
                contract,
                now
            ],
        )
        .unwrap();
    }

    fn select_activation(conn: &Connection, display_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT activation FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap()
    }

    fn audit_rows(
        conn: &Connection,
        display_id: &str,
    ) -> Vec<(String, Option<String>, String)> {
        let mut s = conn
            .prepare(
                "SELECT verb, actor_note, invoker FROM transition_history \
                 WHERE display_id = ?1 ORDER BY id ASC",
            )
            .unwrap();
        s.query_map(rusqlite::params![display_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn build_activate_matches(args: &[&str]) -> ArgMatches {
        let cmd = Command::new("activate")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true));
        cmd.get_matches_from(args)
    }

    /// AC1.3: A fresh in-memory DB created via the schema yields tasks rows
    /// with default `activation='inactive'`.
    #[test]
    fn activation_ac1_3_fresh_db_default_is_inactive() {
        let (_schema, conn) = fresh_db_with_tasks();
        insert_minimal_task(&conn, "T100", "planning");
        let got = select_activation(&conn, "T100")
            .expect("activation column present and non-NULL");
        assert_eq!(got, "inactive", "fresh-DB row must default to inactive");
    }

    /// AC1.4: parametrised backfill — for an old DB without the activation
    /// column, after framework-migrate every IN_FLIGHT row is 'active' and
    /// every other row is 'inactive'. Seeds at least one row per status class
    /// on each side of the IN_FLIGHT_STATES boundary.
    #[test]
    fn activation_ac1_4_framework_migrate_backfills_in_flight_vs_rest() {
        let in_flight: &[&str] = IN_FLIGHT_STATES;
        let inactive_states: &[&str] = &[
            "planning",
            "plan_review",
            "ready",
            "blocked",
            "deploy_blocked",
            "accepted",
            "integration_queued",
            "integration_blocked",
            "complete",
            "in_review",
            "cargo_installed",
            "schema_migrated",
            "integrated",
            "rejected",
            "abandoned",
            "closed_out_of_band",
        ];

        let (_schema, conn) = pre_t140_db();

        let mut idx: u32 = 100;
        let mut inactive_ids: Vec<String> = Vec::new();
        for status in inactive_states {
            let display_id = format!("T{idx:03}");
            insert_minimal_task(&conn, &display_id, status);
            inactive_ids.push(display_id);
            idx += 1;
        }
        let mut in_flight_ids: Vec<String> = Vec::new();
        for status in in_flight {
            let display_id = format!("T{idx:03}");
            insert_minimal_task(&conn, &display_id, status);
            in_flight_ids.push(display_id);
            idx += 1;
        }

        // Sanity: column genuinely absent before migration.
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info('tasks')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            !cols.iter().any(|c| c == "activation"),
            "pre_t140_db must lack activation column; got: {cols:?}"
        );

        // Run framework-migrate.
        let applied = apply_framework_drift(&conn).unwrap();
        assert!(
            applied
                .iter()
                .any(|m| m.table_name == "tasks" && m.column_name == "activation"),
            "expected an applied migration for tasks.activation; got: {applied:?}"
        );

        for id in &in_flight_ids {
            let got = select_activation(&conn, id).expect("activation column populated");
            assert_eq!(
                got, "active",
                "row {id} status in IN_FLIGHT_STATES must backfill to 'active'"
            );
        }
        for id in &inactive_ids {
            let got = select_activation(&conn, id).expect("activation column populated");
            assert_eq!(
                got, "inactive",
                "row {id} not in IN_FLIGHT_STATES must backfill to 'inactive'"
            );
        }
    }

    /// AC1.5: ai_with_human activate flips activation to 'active' and writes
    /// transition_history(verb='activate', actor_note=<reason>).
    #[test]
    fn activation_ac1_5_ai_with_human_flips_and_writes_history() {
        let (schema, conn) = fresh_db_with_tasks();
        conn.execute_batch(SUBSTRATE_DDL).ok();
        insert_minimal_task(&conn, "T999", "planning");

        let m = build_activate_matches(&[
            "activate",
            "T999",
            "--reason",
            "operator armed for integration",
        ]);
        run_activate(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect("ai_with_human activate must succeed");

        assert_eq!(select_activation(&conn, "T999").unwrap(), "active");

        // Status unchanged.
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id = 'T999'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "planning", "lifecycle status must NOT change");

        let rows = audit_rows(&conn, "T999");
        assert_eq!(rows.len(), 1, "exactly one audit row; got: {rows:?}");
        assert_eq!(rows[0].0, "activate");
        assert_eq!(
            rows[0].1.as_deref(),
            Some("operator armed for integration"),
            "actor_note must record --reason verbatim"
        );
        assert_eq!(rows[0].2, "ai_with_human");
    }

    /// AC1.6: ai_autonomous invoker is rejected fail-loud and the row's
    /// activation column is NOT mutated.
    #[test]
    fn activation_ac1_6_ai_autonomous_rejected_row_unchanged() {
        let (schema, conn) = fresh_db_with_tasks();
        conn.execute_batch(SUBSTRATE_DDL).ok();
        insert_minimal_task(&conn, "T999", "planning");

        let m = build_activate_matches(&[
            "activate",
            "T999",
            "--reason",
            "should be rejected",
        ]);
        let err = run_activate(&schema, &conn, &m, Actor::AiAutonomous.into())
            .expect_err("ai_autonomous activate MUST be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("ai_with_human")
                || msg.contains("ai_autonomous")
                || msg.contains("actor"),
            "error must surface actor-rejection wording; got: {msg}"
        );

        assert_eq!(
            select_activation(&conn, "T999").unwrap(),
            "inactive",
            "rejected write must NOT mutate activation column"
        );

        let rows = audit_rows(&conn, "T999");
        assert!(
            rows.is_empty(),
            "rejected write must NOT insert a transition_history row; got: {rows:?}"
        );
    }

    /// AC1.7: missing `--reason` errors at clap parse time. The handler is
    /// never reached.
    #[test]
    fn activation_ac1_7_missing_reason_errors_at_cli_parse() {
        let cmd = Command::new("activate")
            .arg(Arg::new("display_id").required(true).index(1))
            .arg(Arg::new("reason").long("reason").required(true));
        let err = cmd
            .try_get_matches_from(["activate", "T999"])
            .expect_err("missing --reason must fail parse");
        let msg = err.to_string();
        assert!(
            msg.contains("--reason") || msg.to_lowercase().contains("required"),
            "error must cite --reason / required; got: {msg}"
        );
    }

    /// `deactivate` is the symmetric verb. ai_with_human flips active→inactive
    /// and writes verb='deactivate' / actor_note=<reason>; ai_autonomous is
    /// rejected.
    #[test]
    fn activation_deactivate_mirrors_activate() {
        let (schema, conn) = fresh_db_with_tasks();
        conn.execute_batch(SUBSTRATE_DDL).ok();
        insert_minimal_task(&conn, "T700", "planning");

        // Arm first via activate so we can observe the disarm.
        let m_arm = build_activate_matches(&["activate", "T700", "--reason", "arm"]);
        run_activate(&schema, &conn, &m_arm, Actor::AiWithHuman.into()).unwrap();
        assert_eq!(select_activation(&conn, "T700").unwrap(), "active");

        // ai_autonomous deactivate → rejected, activation stays 'active'.
        let m_bad = build_activate_matches(&[
            "deactivate",
            "T700",
            "--reason",
            "should be rejected",
        ]);
        let err = run_deactivate(&schema, &conn, &m_bad, Actor::AiAutonomous.into())
            .expect_err("ai_autonomous deactivate MUST be rejected");
        assert!(
            err.to_string().contains("ai_with_human")
                || err.to_string().contains("ai_autonomous")
                || err.to_string().contains("actor"),
            "expected actor-rejection; got: {err}"
        );
        assert_eq!(
            select_activation(&conn, "T700").unwrap(),
            "active",
            "rejected deactivate must NOT mutate row"
        );

        // ai_with_human deactivate → flips and writes audit row.
        let m_ok = build_activate_matches(&[
            "deactivate",
            "T700",
            "--reason",
            "stand down for review",
        ]);
        run_deactivate(&schema, &conn, &m_ok, Actor::AiWithHuman.into())
            .expect("ai_with_human deactivate must succeed");
        assert_eq!(select_activation(&conn, "T700").unwrap(), "inactive");

        let rows = audit_rows(&conn, "T700");
        assert_eq!(rows.len(), 2, "two audit rows expected: arm + stand-down");
        assert_eq!(rows[1].0, "deactivate");
        assert_eq!(rows[1].1.as_deref(), Some("stand down for review"));
        assert_eq!(rows[1].2, "ai_with_human");
    }

    /// Idempotency: running framework-migrate twice is a no-op the second
    /// time. The second `apply_framework_drift` returns no entry for
    /// tasks.activation and existing rows' activation values are not
    /// re-mutated (the IN_FLIGHT_STATES backfill does not re-run).
    #[test]
    fn activation_idempotent_second_framework_migrate_is_noop() {
        let (_schema, conn) = pre_t140_db();
        insert_minimal_task(&conn, "T200", "planning"); // → inactive after backfill
        insert_minimal_task(&conn, "T201", "executing"); // → active after backfill

        let applied1 = apply_framework_drift(&conn).unwrap();
        assert!(applied1
            .iter()
            .any(|m| m.table_name == "tasks" && m.column_name == "activation"));
        assert_eq!(select_activation(&conn, "T200").unwrap(), "inactive");
        assert_eq!(select_activation(&conn, "T201").unwrap(), "active");

        // Mutate T200 to 'active' to prove the second apply does NOT clobber.
        conn.execute(
            "UPDATE tasks SET activation='active' WHERE display_id='T200'",
            [],
        )
        .unwrap();

        let applied2 = apply_framework_drift(&conn).unwrap();
        assert!(
            !applied2
                .iter()
                .any(|m| m.table_name == "tasks" && m.column_name == "activation"),
            "second apply must NOT re-emit tasks.activation migration; got: {applied2:?}"
        );

        assert_eq!(
            select_activation(&conn, "T200").unwrap(),
            "active",
            "second apply must NOT re-run the IN_FLIGHT_STATES backfill"
        );
        assert_eq!(select_activation(&conn, "T201").unwrap(), "active");
    }
}
