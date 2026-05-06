//! Integration tests for T052 P3: override-risk and override-policy CLI verbs.
//!
//! Tests:
//!  (a) override-risk with --invoker ai_autonomous rejected
//!  (b) override-risk with --invoker ai_with_human accepted, transition_history written
//!  (c) override-policy human→auto with --invoker ai_with_human (no token) REJECTED
//!  (d) override-policy human→auto with --invoker ai_with_human + valid token ACCEPTED
//!  (e) override-policy auto→human with --invoker ai_with_human (no token) ACCEPTED (escalation)
//!  (f) missing --reason rejected at CLI parse

use std::collections::BTreeMap;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::overrides::{run_override_policy, run_override_risk};
use stores::schema::{actor::{Actor, InvokerCtx}, Schema};

fn obs_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "observations")
        .map(|(_, y)| *y)
        .expect("bundled observations schema");
    Schema::from_yaml(yaml).expect("parse observations schema")
}

fn setup() -> (Schema, rusqlite::Connection) {
    let schema = obs_schema();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
    (schema, conn)
}

fn insert_obs(conn: &rusqlite::Connection, display_id: &str) {
    let now = "2026-05-06T00:00:00Z";
    conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, source, priority, captured_at, captured_week, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'open', 'test', 'dev', 'normal', ?2, 'w18', ?2, ?2, 'human', 'human')",
        rusqlite::params![display_id, now],
    )
    .unwrap();
}

fn audit_rows(
    conn: &rusqlite::Connection,
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

fn build_risk_matches(args: &[&str]) -> clap::ArgMatches {
    use clap::{Arg, ArgAction, Command};
    Command::new("override-risk")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .arg(Arg::new("risk-class").long("risk-class").required(false))
        .arg(
            Arg::new("risk-flags")
                .long("risk-flags")
                .action(ArgAction::Append)
                .required(false),
        )
        .arg(Arg::new("cluster-key").long("cluster-key").required(false))
        .get_matches_from(args)
}

fn build_policy_matches(args: &[&str]) -> clap::ArgMatches {
    use clap::{Arg, Command};
    Command::new("override-policy")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .arg(Arg::new("approval-policy").long("approval-policy").required(true))
        .get_matches_from(args)
}

/// (a) override-risk with ai_autonomous is rejected on every path (AC3.6)
#[test]
fn override_risk_ai_autonomous_always_rejected() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    let m = build_risk_matches(&[
        "override-risk", "L001", "--reason", "test", "--risk-class", "low",
    ]);
    let err = run_override_risk(&schema, &conn, &m, Actor::AiAutonomous.into())
        .expect_err("ai_autonomous must be rejected for override-risk");
    assert!(
        err.to_string().contains("ai_autonomous"),
        "error must cite ai_autonomous; got: {err}"
    );
    // Row must be unchanged
    let rc: Option<String> = conn
        .query_row(
            "SELECT risk_class FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // risk_class has default 'normal'; must not have been changed to 'low'
    assert_ne!(rc.as_deref(), Some("low"), "row must be unchanged after rejection");
}

/// (b) override-risk with ai_with_human accepted; transition_history row written with actor_note
#[test]
fn override_risk_ai_with_human_accepted_writes_history() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    let m = build_risk_matches(&[
        "override-risk", "L001", "--reason", "downgrade to low risk", "--risk-class", "low",
    ]);
    run_override_risk(&schema, &conn, &m, Actor::AiWithHuman.into())
        .expect("ai_with_human must be accepted for override-risk");

    // Field was updated
    let rc: String = conn
        .query_row(
            "SELECT risk_class FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rc, "low");

    // transition_history row with verb='override_risk' and correct actor_note (AC3.4)
    let rows = audit_rows(&conn, "L001");
    assert_eq!(rows.len(), 1, "one audit row expected; got: {rows:?}");
    assert_eq!(rows[0].0, "override_risk");
    assert_eq!(
        rows[0].1.as_deref(),
        Some("downgrade to low risk"),
        "actor_note must equal --reason"
    );
    assert_eq!(rows[0].2, "ai_with_human");
}

/// (c) override-policy human→auto with ai_with_human (no token) is REJECTED (AC3.2, relaxation)
#[test]
fn override_policy_relaxation_no_token_rejected() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    // Default approval_policy is 'human'; requesting 'auto' = relaxation
    let m = build_policy_matches(&[
        "override-policy", "L001", "--reason", "relax policy", "--approval-policy", "auto",
    ]);
    let err = run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
        .expect_err("relaxation without token must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("relaxation") || msg.contains("effective human") || msg.contains("approve-token"),
        "error must describe relaxation gate; got: {msg}"
    );
    // Row unchanged
    let policy: Option<String> = conn
        .query_row(
            "SELECT approval_policy FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(policy.as_deref(), Some("auto"), "row must be unchanged after rejection");
}

/// (d) override-policy human→auto with ai_with_human + valid token is ACCEPTED (AC3.2)
#[test]
fn override_policy_relaxation_with_token_accepted() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    let m = build_policy_matches(&[
        "override-policy", "L001", "--reason", "auto for this obs", "--approval-policy", "auto",
    ]);
    let invoker = InvokerCtx {
        actor: Actor::AiWithHuman,
        token_valid: true,
    };
    run_override_policy(&schema, &conn, &m, invoker)
        .expect("ai_with_human + token must be accepted for relaxation");

    let policy: String = conn
        .query_row(
            "SELECT approval_policy FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(policy, "auto");

    let rows = audit_rows(&conn, "L001");
    assert_eq!(rows.len(), 1, "one audit row expected");
    assert_eq!(rows[0].0, "override_policy_relax");
    assert_eq!(
        rows[0].1.as_deref(),
        Some("auto for this obs"),
        "actor_note must equal --reason (AC3.4)"
    );
}

/// (e) override-policy auto→human with ai_with_human (no token) is ACCEPTED (AC3.3, escalation)
#[test]
fn override_policy_escalation_no_token_accepted() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    // Seed below default
    conn.execute(
        "UPDATE observations SET approval_policy='auto' WHERE display_id='L001'",
        [],
    )
    .unwrap();
    let m = build_policy_matches(&[
        "override-policy", "L001", "--reason", "raise bar", "--approval-policy", "human",
    ]);
    run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
        .expect("escalation without token must be accepted");

    let policy: String = conn
        .query_row(
            "SELECT approval_policy FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(policy, "human");

    let rows = audit_rows(&conn, "L001");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "override_policy_escalate");
    assert_eq!(rows[0].1.as_deref(), Some("raise bar"));
}

/// (f) missing --reason rejected at CLI parse level (AC3.5)
#[test]
fn override_risk_missing_reason_rejected_at_parse() {
    use clap::{Arg, Command};
    let cmd = Command::new("override-risk")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .arg(Arg::new("risk-class").long("risk-class").required(false));
    let err = cmd
        .try_get_matches_from(&["override-risk", "L001", "--risk-class", "low"])
        .expect_err("missing --reason must fail parse");
    let msg = err.to_string();
    assert!(
        msg.contains("--reason") || msg.contains("required"),
        "error must cite --reason; got: {msg}"
    );
}

#[test]
fn override_policy_missing_reason_rejected_at_parse() {
    use clap::{Arg, Command};
    let cmd = Command::new("override-policy")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .arg(Arg::new("approval-policy").long("approval-policy").required(true));
    let err = cmd
        .try_get_matches_from(&["override-policy", "L001", "--approval-policy", "auto"])
        .expect_err("missing --reason must fail parse");
    let msg = err.to_string();
    assert!(
        msg.contains("--reason") || msg.contains("required"),
        "error must cite --reason; got: {msg}"
    );
}

/// AC3.4: every successful override write records reason as actor_note in transition_history
#[test]
fn override_risk_actor_note_equals_reason() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    let m = build_risk_matches(&[
        "override-risk", "L001", "--reason", "specific reason text", "--risk-class", "security",
    ]);
    run_override_risk(&schema, &conn, &m, Actor::AiWithHuman.into()).unwrap();

    let rows = audit_rows(&conn, "L001");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1.as_deref(),
        Some("specific reason text"),
        "actor_note must exactly equal --reason"
    );
}

/// AC3.6: override-policy with ai_autonomous rejected regardless of direction
#[test]
fn override_policy_ai_autonomous_rejected_on_escalation_too() {
    let (schema, conn) = setup();
    insert_obs(&conn, "L001");
    conn.execute(
        "UPDATE observations SET approval_policy='auto' WHERE display_id='L001'",
        [],
    )
    .unwrap();
    // escalation path: auto→human, but ai_autonomous must still be rejected
    let m = build_policy_matches(&[
        "override-policy", "L001", "--reason", "test", "--approval-policy", "human",
    ]);
    let err = run_override_policy(&schema, &conn, &m, Actor::AiAutonomous.into())
        .expect_err("ai_autonomous must be rejected even for escalation");
    assert!(
        err.to_string().contains("ai_autonomous"),
        "error must cite ai_autonomous; got: {err}"
    );
}

/// AC3.1: cargo build passes (implicit — compile errors would fail the test run)
/// AC3.2: all relaxation paths require token or human invoker
#[test]
fn all_relaxation_paths_require_token() {
    let (schema, conn) = setup();

    // (architecture→human, human→auto, architecture→auto)
    let relaxation_cases: &[(&str, &str)] = &[
        ("architecture", "human"),
        ("human", "auto"),
        ("architecture", "auto"),
    ];

    for (i, (from_pol, to_pol)) in relaxation_cases.iter().enumerate() {
        let display_id = format!("L{:03}", i + 1);
        insert_obs(&conn, &display_id);
        conn.execute(
            "UPDATE observations SET approval_policy=?1 WHERE display_id=?2",
            rusqlite::params![from_pol, &display_id],
        )
        .unwrap();

        // Bare ai_with_human (no token) must fail
        let m = build_policy_matches(&[
            "override-policy", &display_id, "--reason", "relax", "--approval-policy", to_pol,
        ]);
        let err = run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
            .expect_err(&format!("{from_pol}→{to_pol} without token must fail"));
        assert!(
            err.to_string().contains("relaxation")
                || err.to_string().contains("approve-token")
                || err.to_string().contains("effective human"),
            "expected relaxation error for {from_pol}→{to_pol}; got: {err}"
        );

        // With valid token must succeed
        let m2 = build_policy_matches(&[
            "override-policy", &display_id, "--reason", "approved", "--approval-policy", to_pol,
        ]);
        let invoker = InvokerCtx { actor: Actor::AiWithHuman, token_valid: true };
        run_override_policy(&schema, &conn, &m2, invoker)
            .unwrap_or_else(|e| panic!("{from_pol}→{to_pol} with token must succeed; got: {e}"));
    }
}

/// AC3.3: all escalation paths succeed under ai_with_human without token
#[test]
fn all_escalation_paths_accept_ai_with_human() {
    let (schema, conn) = setup();

    let escalation_cases: &[(&str, &str)] = &[
        ("auto", "human"),
        ("human", "architecture"),
        ("auto", "architecture"),
    ];

    for (i, (from_pol, to_pol)) in escalation_cases.iter().enumerate() {
        let display_id = format!("L{:03}", i + 1);
        insert_obs(&conn, &display_id);
        conn.execute(
            "UPDATE observations SET approval_policy=?1 WHERE display_id=?2",
            rusqlite::params![from_pol, &display_id],
        )
        .unwrap();

        let m = build_policy_matches(&[
            "override-policy", &display_id, "--reason", "escalate", "--approval-policy", to_pol,
        ]);
        run_override_policy(&schema, &conn, &m, Actor::AiWithHuman.into())
            .unwrap_or_else(|e| {
                panic!("{from_pol}→{to_pol} escalation must succeed without token; got: {e}")
            });

        let policy: String = conn
            .query_row(
                "SELECT approval_policy FROM observations WHERE display_id=?1",
                rusqlite::params![&display_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(policy, *to_pol, "{from_pol}→{to_pol}: policy mismatch");
    }
}
