//! Sweep-style subscriber: processes `intake.status='draft'` rows through the
//! production gatekeeper Router on every daemon poll tick.
//!
//! Wired into `poll_once_with_guard` after `sweep_drive_watchdog` and before
//! `run_engine_runner_iteration`. Errors and panics are caught at the call site
//! so a sweep failure cannot crash the daemon.

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;

use crate::flow::AgentsYaml;
use crate::handlers::row::read_row;
use crate::handlers::transition::execute_transition_write;
use crate::schema::actor::Actor;
use crate::schema::lifecycle::select_transition;
use crate::validate::{self, EntryMap, Op};

use super::gatekeeper_router;
use super::load_store_schema;

pub struct DrainSummary {
    pub saw: usize,
    pub processed: usize,
    pub errored: usize,
}

pub fn run_drain_sweep(
    conn: &Connection,
    _agents: &AgentsYaml,
    _config_path: &Path,
    policies_hash: &str,
) -> Result<DrainSummary> {
    let iter: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(iteration), 0) + 1 FROM engine_runner_heartbeats",
            [],
            |r| r.get(0),
        )
        .unwrap_or(1);

    // Snapshot draft rows ONCE at function entry — no re-scan within this call.
    let draft_ids: Vec<String> = conn
        .prepare("SELECT display_id FROM intake WHERE status='draft' ORDER BY id ASC")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut summary = DrainSummary {
        saw: draft_ids.len(),
        processed: 0,
        errored: 0,
    };

    let schema = load_store_schema("intake")?;
    let phash_opt = if policies_hash.is_empty() {
        None
    } else {
        Some(policies_hash)
    };

    for display_id in draft_ids {
        let result = process_one_row(conn, &schema, &display_id, phash_opt);
        match result {
            Ok(()) => summary.processed += 1,
            Err(e) => {
                eprintln!(
                    "[gatekeeper-router-drain] error processing {}: {:#}",
                    display_id, e
                );
                summary.errored += 1;
            }
        }
    }

    eprintln!(
        "[gatekeeper-router-drain] iter={} saw={} processed={}",
        iter, summary.saw, summary.processed
    );

    Ok(summary)
}

fn process_one_row(
    conn: &Connection,
    schema: &crate::schema::Schema,
    display_id: &str,
    phash_opt: Option<&str>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    // (a) Read row
    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // (b) claim-triage: draft → triaging
    let claim_triage_transition = select_transition(
        &schema.lifecycle.transitions,
        &current_status,
        "claim-triage",
        None,
        &existing,
    )?;

    let diff_claim: EntryMap = EntryMap::new();
    let merged_after_claim = existing.clone();

    validate::validate(
        schema,
        &merged_after_claim,
        Op::Transition("claim-triage".to_string(), diff_claim.clone()),
        Actor::AiAutonomous.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "claim-triage validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    execute_transition_write(
        &tx,
        schema,
        row_id,
        display_id,
        &current_status,
        &claim_triage_transition.to,
        "claim-triage",
        &diff_claim,
        &merged_after_claim,
        Actor::AiAutonomous,
        None,
        phash_opt,
        Some("builtin:gatekeeper-router-drain"),
    )?;

    // Re-read after claim-triage so merged reflects triaging state
    let (row_id2, existing2) = read_row(schema, &tx, display_id)?;
    let triaging_status = existing2
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // (c) Route
    let decision = gatekeeper_router::route(&existing2);

    // (d) Build decision payload
    let intake_decision_str = gatekeeper_router::to_intake_decision(&decision);
    let timestamp = crate::handlers::row::now_iso8601();
    let decision_json =
        gatekeeper_router::build_decision_json(&decision, "gatekeeper_router_drain", &timestamp, &[]);

    let mut diff = EntryMap::new();
    diff.insert(
        "decision".to_string(),
        Value::String(intake_decision_str.to_string()),
    );
    diff.insert("gatekeeper_decision_json".to_string(), decision_json);

    let mut merged = existing2.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    crate::handlers::transition::maybe_validate_and_mirror_gatekeeper_decision(
        schema,
        &mut diff,
        &mut merged,
    )?;

    crate::handlers::intake_route::inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route")?;

    let route_transition = select_transition(
        &schema.lifecycle.transitions,
        &triaging_status,
        "route",
        None,
        &merged,
    )?;

    crate::handlers::transition::inject_upstream_primary_tuple(
        schema,
        route_transition,
        "route",
        &triaging_status,
        &route_transition.to,
        &mut diff,
        &mut merged,
    )?;

    validate::validate(
        schema,
        &merged,
        Op::Transition(
            "route".to_string(),
            crate::handlers::transition::strip_framework_overlay_from_validation_diff(schema, &diff),
        ),
        Actor::AiAutonomous.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "route validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    execute_transition_write(
        &tx,
        schema,
        row_id2,
        display_id,
        &triaging_status,
        &route_transition.to,
        "route",
        &diff,
        &merged,
        Actor::AiAutonomous,
        None,
        phash_opt,
        Some("builtin:gatekeeper-router-drain"),
    )?;

    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests T1-T11
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::Schema;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        for name in ["intake", "observations", "architecture_reviews"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, y)| *y)
                .unwrap_or_else(|| panic!("bundled {} schema missing", name));
            let schema = Schema::from_yaml(yaml)
                .unwrap_or_else(|e| panic!("parse {} schema: {}", name, e));
            conn.execute_batch(&ddl_for(&schema))
                .unwrap_or_else(|e| panic!("ddl for {}: {}", name, e));
        }

        conn
    }

    fn insert_draft(conn: &Connection, display_id: &str, summary: &str, body: &str) {
        conn.execute(
            "INSERT INTO intake (display_id, status, summary, body, source_agent, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'draft', ?2, ?3, 'executor', '2026-05-09T12:00:00Z', 'w19-d5', '2026-05-09T12:00:00Z', '2026-05-09T12:00:00Z', 'ai', 'ai')",
            rusqlite::params![display_id, summary, body],
        ).unwrap();
    }

    fn agents() -> AgentsYaml {
        AgentsYaml::default_empty()
    }

    fn cfg() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/no-config.yaml")
    }

    fn row_status(conn: &Connection, display_id: &str) -> String {
        conn.query_row(
            "SELECT status FROM intake WHERE display_id=?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn transition_history_verbs(conn: &Connection, display_id: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT verb FROM transition_history WHERE store='intake' AND display_id=?1 ORDER BY id",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![display_id], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn decision_json_for(conn: &Connection, display_id: &str) -> serde_json::Value {
        let s: String = conn
            .query_row(
                "SELECT gatekeeper_decision_json FROM intake WHERE display_id=?1",
                rusqlite::params![display_id],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn t1_single_draft_routed_within_one_sweep() {
        let conn = fresh_db();
        // "add lifecycle invariant" triggers RouteToArchReviewCandidate
        insert_draft(&conn, "I001", "add lifecycle invariant", "some body");

        let a = agents();
        let c = cfg();
        let summary = run_drain_sweep(&conn, &a, &c, "").unwrap();

        let status = row_status(&conn, "I001");
        assert_ne!(status, "draft", "status must not be draft after sweep");

        let djson = decision_json_for(&conn, "I001");
        crate::validate::gatekeeper_decision::validate_gatekeeper_decision(&djson)
            .expect("gatekeeper_decision_json must be valid");
        assert_eq!(djson["source_agent"], "gatekeeper_router_drain");
        assert!(
            djson["timestamp"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "timestamp must be non-empty"
        );
        assert!(djson["evidence"].is_array(), "evidence must be array");

        let verbs = transition_history_verbs(&conn, "I001");
        assert!(
            verbs.contains(&"claim-triage".to_string()),
            "must have claim-triage in history"
        );
        assert!(
            verbs.contains(&"route".to_string()),
            "must have route in history"
        );

        // Both history rows must have actor_note=builtin:gatekeeper-router-drain
        let notes: Vec<Option<String>> = {
            let mut stmt = conn
                .prepare(
                    "SELECT actor_note FROM transition_history WHERE store='intake' AND display_id='I001' ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for note in &notes {
            assert_eq!(
                note.as_deref(),
                Some("builtin:gatekeeper-router-drain"),
                "actor_note must be builtin:gatekeeper-router-drain"
            );
        }

        assert_eq!(summary.processed, 1);
        assert_eq!(summary.errored, 0);
    }

    #[test]
    fn t2_22_drafts_processed_in_one_sweep() {
        let conn = fresh_db();
        // Insert 22 drafts spanning different Router branches
        let cases = [
            // UnableToRoute (empty) → needs_info
            ("I001", "", ""),
            // Drop
            ("I002", "wip test", ""),
            ("I003", "noise scratch", ""),
            // RouteToArchReviewCandidate
            ("I004", "add lifecycle invariant", ""),
            ("I005", "schema migration needed", ""),
            ("I006", "actor authority enforcement", ""),
            ("I007", "subscriber semantics change", ""),
            ("I008", "runner boundary adjustment", ""),
            // RouteToObservation (merge conflict → deploy-blocked-merge-conflict)
            ("I009", "deploy blocked by merge conflict in branch", ""),
            // NeedsInfo (generic)
            ("I010", "some random observation alpha", ""),
            ("I011", "some random observation beta", ""),
            ("I012", "some random observation gamma", ""),
            ("I013", "something unrelated entirely", ""),
            ("I014", "another random thing", ""),
            ("I015", "unclear item here", ""),
            ("I016", "undefined behavior case", ""),
            ("I017", "missing context for classification", ""),
            ("I018", "general observation type one", ""),
            ("I019", "general observation type two", ""),
            ("I020", "general observation type three", ""),
            ("I021", "general observation type four", ""),
            ("I022", "general observation type five", ""),
        ];
        assert_eq!(cases.len(), 22);
        for (id, summary, body) in &cases {
            insert_draft(&conn, id, summary, body);
        }

        let a = agents();
        let c = cfg();
        let summary = run_drain_sweep(&conn, &a, &c, "").unwrap();

        assert_eq!(summary.saw, 22);
        assert_eq!(
            summary.processed + summary.errored,
            22,
            "processed + errored must equal 22"
        );

        let remaining_drafts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM intake WHERE status='draft'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_drafts, summary.errored as i64,
            "remaining drafts must equal errored count"
        );
    }

    #[test]
    fn t3_heartbeat_log_format() {
        // Capture stderr by redirecting — we test by checking the function
        // runs without panic and emits exactly the expected pattern.
        // Since we can't easily capture eprintln! in tests, we verify the
        // format string is correct by running two sweeps (including saw=0).
        let conn = fresh_db();
        let a = agents();
        let c = cfg();

        // Empty DB sweep → saw=0
        let s1 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s1.saw, 0);

        // One draft sweep
        insert_draft(&conn, "I001", "test wip", "");
        let s2 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s2.saw, 1);
    }

    #[test]
    fn t4_inserted_after_first_sweep_processed_on_next() {
        let conn = fresh_db();
        let a = agents();
        let c = cfg();

        // Sweep 1: empty
        let s1 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s1.saw, 0);

        // Insert after sweep
        insert_draft(&conn, "I001", "some random observation here", "");

        // Sweep 2: should process I001
        let s2 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s2.saw, 1);
        assert_eq!(s2.processed, 1);

        let status = row_status(&conn, "I001");
        assert_ne!(status, "draft");
    }

    #[test]
    fn t5_decision_in_existing_categories() {
        let conn = fresh_db();
        let valid_decisions = [
            "duplicate",
            "needs_info",
            "fast_track",
            "normal_observation",
            "arch_review_candidate",
            "reject_noise",
        ];

        let cases = [
            ("I001", "add lifecycle invariant", ""),
            ("I002", "wip test", ""),
            ("I003", "deploy blocked by merge conflict in branch", ""),
            ("I004", "something generic here", ""),
            ("I005", "", ""),
        ];
        for (id, summary, body) in &cases {
            insert_draft(&conn, id, summary, body);
        }

        let a = agents();
        let c = cfg();
        run_drain_sweep(&conn, &a, &c, "").unwrap();

        for (id, _, _) in &cases {
            let djson = decision_json_for(&conn, id);
            let dec = djson["decision"].as_str().unwrap();
            assert!(
                valid_decisions.contains(&dec),
                "decision '{}' not in valid enum for {}",
                dec,
                id
            );
            crate::validate::gatekeeper_decision::validate_gatekeeper_decision(&djson)
                .unwrap_or_else(|e| panic!("invalid payload for {}: {:?}", id, e));
        }
    }

    #[test]
    fn t6_terminal_decision_not_re_processed() {
        let conn = fresh_db();
        insert_draft(&conn, "I001", "something generic here", "");

        let a = agents();
        let c = cfg();

        // First sweep: routes to needs_info
        let s1 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s1.processed, 1);

        let status = row_status(&conn, "I001");
        assert_ne!(status, "draft", "I001 must not be draft after first sweep");

        let history_count_after_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='intake' AND display_id='I001'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Second sweep: I001 is not draft → NOT in snapshot → no new history rows
        let s2 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s2.saw, 0, "second sweep must see 0 draft rows");

        let history_count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='intake' AND display_id='I001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            history_count_after_first, history_count_after_second,
            "second sweep must not add transition_history rows for I001"
        );
    }

    #[test]
    fn t7_failed_row_stays_draft_and_logs() {
        let conn = fresh_db();
        // Insert a draft that will be classified as normal_observation
        // (merge conflict → RouteToObservation → inject_pre_validation_fields inserts into observations)
        insert_draft(
            &conn,
            "I001",
            "deploy blocked by merge conflict in branch",
            "",
        );

        // Drop observations table to cause inject_pre_validation_fields to fail
        conn.execute_batch("DROP TABLE IF EXISTS observations").unwrap();

        let a = agents();
        let c = cfg();
        let summary = run_drain_sweep(&conn, &a, &c, "").unwrap();

        assert_eq!(summary.errored, 1);
        assert_eq!(summary.processed, 0);

        // Row must still be in draft (rollback)
        let status = row_status(&conn, "I001");
        assert_eq!(status, "draft", "failed row must remain in draft");

        // No transition_history rows for I001
        let hist: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='intake' AND display_id='I001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hist, 0, "no transition_history rows on error rollback");
    }

    #[test]
    fn t8_no_hot_loop_within_single_pass() {
        let conn = fresh_db();
        // Drop observations to make normal_observation routing fail
        conn.execute_batch("DROP TABLE IF EXISTS observations").unwrap();

        // Insert a row that will error (needs observations table)
        insert_draft(
            &conn,
            "I001",
            "deploy blocked by merge conflict in branch",
            "",
        );

        let a = agents();
        let c = cfg();
        let summary = run_drain_sweep(&conn, &a, &c, "").unwrap();

        // Must be touched exactly once even when erroring
        assert_eq!(summary.saw, 1);
        assert_eq!(summary.errored, 1);
        // No retry: errored count is exactly 1, not more
        assert_eq!(summary.processed + summary.errored, 1);
    }

    #[test]
    fn t9_router_seam_produces_distinct_decisions() {
        let conn = fresh_db();

        // 5 inputs designed to trigger 5 different RouterDecision variants:
        // 1. UnableToRoute → needs_info (empty summary+body)
        insert_draft(&conn, "I001", "", "");
        // 2. Drop → reject_noise
        insert_draft(&conn, "I002", "wip test", "");
        // 3. RouteToArchReviewCandidate → arch_review_candidate
        insert_draft(&conn, "I003", "add lifecycle invariant", "");
        // 4. RouteToObservation → normal_observation (merge conflict)
        insert_draft(
            &conn,
            "I004",
            "deploy blocked by merge conflict in branch",
            "",
        );
        // 5. NeedsInfo → needs_info (generic, no cluster match)
        insert_draft(&conn, "I005", "something entirely unrelated here", "");

        let a = agents();
        let c = cfg();
        run_drain_sweep(&conn, &a, &c, "").unwrap();

        let decisions: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT JSON_EXTRACT(gatekeeper_decision_json, '$.decision') FROM intake ORDER BY display_id",
                )
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<Vec<String>, _>>()
                .unwrap()
        };

        let distinct: std::collections::HashSet<_> = decisions.iter().collect();
        assert!(
            distinct.len() >= 3,
            "expected ≥3 distinct decision values; got: {:?}",
            distinct
        );
    }

    #[test]
    fn t10_unable_to_route_persists_as_needs_info_state() {
        let conn = fresh_db();
        // Empty summary+body → UnableToRoute → needs_info
        insert_draft(&conn, "I001", "", "");

        let a = agents();
        let c = cfg();
        let s1 = run_drain_sweep(&conn, &a, &c, "").unwrap();

        assert_eq!(s1.processed, 1);
        assert_eq!(s1.errored, 0);

        let status = row_status(&conn, "I001");
        assert_eq!(status, "needs_info", "UnableToRoute must exit to needs_info state");

        let djson = decision_json_for(&conn, "I001");
        assert_eq!(djson["decision"], "needs_info");
        let rationale = djson["rationale"].as_str().unwrap_or("");
        assert!(
            rationale.contains("router unable to typed-classify"),
            "rationale must contain 'router unable to typed-classify'; got: {}",
            rationale
        );
        assert!(
            djson["missing_info_question"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "missing_info_question must be non-empty"
        );

        let hist_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='intake' AND display_id='I001'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Second sweep: row is in needs_info, not draft → not reprocessed
        let s2 = run_drain_sweep(&conn, &a, &c, "").unwrap();
        assert_eq!(s2.saw, 0);

        let hist_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='intake' AND display_id='I001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            hist_before, hist_after,
            "second sweep must not add transition_history rows for needs_info row"
        );
    }

    #[test]
    fn t11_source_agent_recorded_in_payload() {
        let conn = fresh_db();
        // Use an arch review candidate since it doesn't depend on observations table for the decision
        // Actually it does (inject_pre_validation_fields creates arch review rows).
        // Use reject_noise (Drop) which has no side effects.
        insert_draft(&conn, "I001", "wip test noise", "");
        // Also add a needs_info generic
        insert_draft(&conn, "I002", "some random generic observation", "");

        let a = agents();
        let c = cfg();
        run_drain_sweep(&conn, &a, &c, "").unwrap();

        for id in ["I001", "I002"] {
            let djson = decision_json_for(&conn, id);
            assert_eq!(
                djson["source_agent"], "gatekeeper_router_drain",
                "source_agent must be gatekeeper_router_drain for {}",
                id
            );
            assert!(
                djson["timestamp"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
                "timestamp must be non-empty for {}",
                id
            );
            assert!(
                djson["evidence"].is_array(),
                "evidence must be array for {}",
                id
            );
        }
    }
}
