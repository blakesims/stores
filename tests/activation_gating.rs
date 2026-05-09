//! T140 P2: integration-level coverage for the activation gate.
//!
//! Tests in this file exercise the gating surfaces wired in P2:
//!   (a) auto-drive subscriber predicate gates on `activation == 'active'`,
//!   (b) integrate subscriber predicate gates on the same field,
//!   (c) `start-integration` schema guard rejects inactive rows,
//!   (d) `tasks add` (no --activate) defaults to activation='inactive',
//!   (e) `tasks add --activate --invoker ai_with_human` lands 'active',
//!   (f) `tasks add --activate --invoker ai_autonomous` is fail-loud rejected,
//!   (g) safety/reconcile/observation-lifecycle subscribers remain ungated.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use rusqlite::Connection;
use serde_json::{json, Value};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::flow::agents_yaml::AgentsYaml;
use stores::flow::predicate::{eval, PredicateExpr};
use stores::schema::lifecycle::select_transition;
use stores::schema::Schema;

/// Path to the canonical agents.yaml fixture used by subscriber-edge contract
/// tests. This is the file we add the activation predicates to in this phase.
fn fixture_agents_yaml() -> AgentsYaml {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let s = std::fs::read_to_string(&path).expect("fixture agents.yaml present");
    AgentsYaml::from_yaml(&s).expect("fixture agents.yaml parses")
}

fn tasks_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "tasks")
        .map(|(_, y)| *y)
        .expect("bundled tasks schema present");
    Schema::from_yaml(yaml).expect("tasks schema parses")
}

/// Find the `predicate` of the (single) subscription on `agent_name` whose
/// transition matches `(from, to)`. Panics if not found, which keeps the
/// error message sharp when the agents.yaml drifts.
fn subscription_predicate(agent_name: &str, from: &str, to: &str) -> PredicateExpr {
    let agents = fixture_agents_yaml();
    let agent = agents
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .unwrap_or_else(|| panic!("agent '{agent_name}' missing from fixture agents.yaml"));
    let sub = agent
        .subscribes_to
        .iter()
        .find(|s| s.transition.from == from && s.transition.to == to)
        .unwrap_or_else(|| {
            panic!("agent '{agent_name}' has no subscription on ({from} -> {to})")
        });
    sub.predicate
        .clone()
        .unwrap_or_else(|| panic!("agent '{agent_name}' subscription on ({from} -> {to}) has no predicate"))
}

fn row(activation: Option<&str>, workspace_path: &str) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(v) = activation {
        obj.insert("activation".to_string(), json!(v));
    }
    obj.insert("workspace_path".to_string(), json!(workspace_path));
    obj.insert("status".to_string(), json!("planning"));
    Value::Object(obj)
}

// --------------------------------------------------------------------------
// AC2.3 — auto-drive predicate gate.
// --------------------------------------------------------------------------

/// AC2.3: the auto-drive subscriber's predicate evaluates true only when the
/// row is active and has a workspace; false on inactive; false on
/// missing/null (fail-closed).
#[test]
fn auto_drive_predicate_gate() {
    let pred = subscription_predicate("auto-drive", "", "planning");

    let active = row(Some("active"), "/tmp/wt");
    let inactive = row(Some("inactive"), "/tmp/wt");
    let missing = row(None, "/tmp/wt");

    assert!(eval(&pred, &active).unwrap(), "active row must dispatch");
    assert!(
        !eval(&pred, &inactive).unwrap(),
        "inactive row must NOT dispatch"
    );
    assert!(
        !eval(&pred, &missing).unwrap(),
        "missing activation column must NOT dispatch (fail-closed)"
    );

    // Workspace-still-empty edge: even an active row without workspace_path
    // must NOT dispatch (the auto-scaffold prerequisite is preserved).
    let active_no_workspace = row(Some("active"), "");
    assert!(
        !eval(&pred, &active_no_workspace).unwrap(),
        "active row without workspace_path must NOT dispatch"
    );
}

// --------------------------------------------------------------------------
// AC2.4 — integrate predicate gate.
// --------------------------------------------------------------------------

/// AC2.4: the integrate subscriber's predicate gates on activation for both
/// `accepted → integration_queued` and `integration_blocked → integration_queued`.
#[test]
fn integrate_predicate_gate() {
    for (from, to) in [
        ("accepted", "integration_queued"),
        ("integration_blocked", "integration_queued"),
    ] {
        let pred = subscription_predicate("integrate", from, to);

        let active = json!({"activation": "active", "status": from});
        let inactive = json!({"activation": "inactive", "status": from});
        let missing = json!({"status": from});

        assert!(
            eval(&pred, &active).unwrap(),
            "({from} -> {to}) active row must dispatch"
        );
        assert!(
            !eval(&pred, &inactive).unwrap(),
            "({from} -> {to}) inactive row must NOT dispatch"
        );
        assert!(
            !eval(&pred, &missing).unwrap(),
            "({from} -> {to}) missing column must NOT dispatch (fail-closed)"
        );
    }
}

// --------------------------------------------------------------------------
// AC2.5 — start-integration schema guard (defense-in-depth).
// --------------------------------------------------------------------------

/// AC2.5: the schema's `integration_queued → integrating` transition carries
/// `guard: "activation == 'active'"`. A framework caller attempting the
/// transition on an inactive row must be rejected; an active row succeeds.
#[test]
fn start_integration_schema_guard() {
    let schema = tasks_schema();

    let mut entry: BTreeMap<String, Value> = BTreeMap::new();
    entry.insert("status".to_string(), json!("integration_queued"));
    entry.insert("activation".to_string(), json!("inactive"));

    let err = select_transition(
        &schema.lifecycle.transitions,
        "integration_queued",
        "start-integration",
        None,
        &entry,
    )
    .expect_err("inactive row must be rejected by start-integration guard");
    let msg = err.to_string();
    assert!(
        msg.contains("guard") || msg.contains("no transition"),
        "expected guard-failure message; got: {msg}"
    );

    entry.insert("activation".to_string(), json!("active"));
    let t = select_transition(
        &schema.lifecycle.transitions,
        "integration_queued",
        "start-integration",
        None,
        &entry,
    )
    .expect("active row must satisfy start-integration guard");
    assert_eq!(t.from, "integration_queued");
    assert_eq!(t.to, "integrating");
    assert_eq!(t.verb, "start-integration");
}

// --------------------------------------------------------------------------
// AC2.6 / AC2.7 — `tasks add` activation default + --activate flag.
// --------------------------------------------------------------------------

fn run_stores(
    bin: &str,
    repo: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(bin);
    cmd.current_dir(repo)
        // Isolate test from any host token so --invoker ai_with_human is treated
        // as the tier-B (no-token) path; AC2.6/2.7 only care about the
        // activation field's actor gate, which fires regardless.
        .env_remove("STORES_TOKEN_DIR")
        .args(args);
    cmd.output().expect("failed to invoke stores binary")
}

fn fresh_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_stores");
    // `stores setup` is init + install bundled stores, which is what makes
    // the dynamic `tasks` subcommand visible. Plain `init` only creates the
    // sqlite file; without store install the CLI rejects `tasks add` with
    // "unrecognized subcommand 'tasks'".
    let out = Command::new(bin)
        .current_dir(tmp.path())
        .env("STORES_SUPPRESS_SETUP_OUTPUT", "1")
        .args(["setup"])
        .output()
        .expect("stores setup runs");
    assert!(
        out.status.success(),
        "stores setup failed: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    tmp
}

fn select_activation(repo: &std::path::Path, display_id: &str) -> Option<String> {
    let conn = Connection::open(repo.join(".stores/db.sqlite")).unwrap();
    conn.query_row(
        "SELECT activation FROM tasks WHERE display_id = ?1",
        rusqlite::params![display_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn count_tasks(repo: &std::path::Path) -> i64 {
    let conn = Connection::open(repo.join(".stores/db.sqlite")).unwrap();
    conn.query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap()
}

fn last_task_display_id(repo: &std::path::Path) -> String {
    let conn = Connection::open(repo.join(".stores/db.sqlite")).unwrap();
    conn.query_row(
        "SELECT display_id FROM tasks ORDER BY id DESC LIMIT 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// AC2.6: `stores tasks add --invoker ai_with_human ...` (no --activate)
/// yields a row at activation='inactive'.
#[test]
fn tasks_add_default_is_inactive() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let repo = fresh_repo();
    let out = run_stores(
        bin,
        repo.path(),
        &[
            "tasks",
            "add",
            "--invoker",
            "ai_with_human",
            "--title",
            "default-inactive",
            "--slug",
            "default-inactive",
            "--done-when",
            "doc-only test",
            "--scope-in",
            "x",
            "--scope-out",
            "y",
        ],
    );
    assert!(
        out.status.success(),
        "tasks add failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let id = last_task_display_id(repo.path());
    assert_eq!(
        select_activation(repo.path(), &id).as_deref(),
        Some("inactive"),
        "tasks add (no --activate) must yield activation='inactive'"
    );
}

/// AC2.7 (positive): `tasks add --invoker ai_with_human --activate` yields
/// activation='active'.
#[test]
fn tasks_add_activate_with_human_invoker_yields_active() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let repo = fresh_repo();
    let out = run_stores(
        bin,
        repo.path(),
        &[
            "tasks",
            "add",
            "--invoker",
            "ai_with_human",
            "--activate",
            "--title",
            "active-row",
            "--slug",
            "active-row",
            "--done-when",
            "doc-only test",
            "--scope-in",
            "x",
            "--scope-out",
            "y",
        ],
    );
    assert!(
        out.status.success(),
        "tasks add --activate failed under ai_with_human:\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = last_task_display_id(repo.path());
    assert_eq!(
        select_activation(repo.path(), &id).as_deref(),
        Some("active"),
        "tasks add --activate must yield activation='active'"
    );
}

/// AC2.7 (negative): `tasks add --invoker ai_autonomous --activate` is
/// rejected fail-loud by the schema's actor gate; no row is inserted.
#[test]
fn tasks_add_activate_with_autonomous_is_rejected() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let repo = fresh_repo();
    let before = count_tasks(repo.path());

    let out = run_stores(
        bin,
        repo.path(),
        &[
            "tasks",
            "add",
            "--invoker",
            "ai_autonomous",
            "--activate",
            "--title",
            "should-fail",
            "--slug",
            "should-fail",
            "--done-when",
            "doc-only test",
            "--scope-in",
            "x",
            "--scope-out",
            "y",
        ],
    );
    assert!(
        !out.status.success(),
        "tasks add --activate --invoker ai_autonomous MUST exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ai_with_human")
            || stderr.contains("actor")
            || stderr.contains("activation"),
        "stderr must cite the actor-gate rejection; got: {stderr}"
    );

    let after = count_tasks(repo.path());
    assert_eq!(
        after, before,
        "rejected --activate must not insert any row"
    );
}

// --------------------------------------------------------------------------
// AC2.9 — safety/reconcile + observation-lifecycle subscribers ungated.
// --------------------------------------------------------------------------

/// Recursive search for any path-lookup of `$activation` inside a predicate
/// expression (handles nested `all_of` predicates).
fn predicate_references_activation(p: &PredicateExpr) -> bool {
    fn operand_is_activation(v: &Value) -> bool {
        v.as_str() == Some("$activation")
    }
    match p {
        PredicateExpr::Eq { left, right }
        | PredicateExpr::Neq { left, right }
        | PredicateExpr::In { left, right }
        | PredicateExpr::NotIn { left, right }
        | PredicateExpr::Matches { left, right } => {
            operand_is_activation(left) || operand_is_activation(right)
        }
        PredicateExpr::AllOf { all } => all.iter().any(predicate_references_activation),
    }
}

/// AC2.9: safety/reconcile + observation-lifecycle subscribers must NOT carry
/// a `$activation` predicate. Only work_starting subscribers (auto-drive,
/// integrate) are gated.
#[test]
fn ungated_subscribers_have_no_activation_predicate() {
    let agents = fixture_agents_yaml();
    let ungated_names = [
        "user-escalation",
        "auto-resolve-observation",
        "auto-scaffold",
        "auto-promote",
        "investigator",
        "external-review",
        "gatekeeper-stub",
    ];

    let mut checked = 0usize;
    for name in &ungated_names {
        // The fixture is a representative subset; some agents (e.g.
        // external-review, gatekeeper-stub) are wired only in the production
        // .stores/agents.yaml or in dedicated fixtures. Tolerate absence —
        // the assertion is "if present, it's NOT gated on $activation". Each
        // agent encountered is counted so the test still fails if the
        // fixture goes empty.
        let Some(agent) = agents.agents.iter().find(|a| &a.name == name) else {
            continue;
        };
        checked += 1;

        for (i, sub) in agent.subscribes_to.iter().enumerate() {
            if let Some(pred) = &sub.predicate {
                assert!(
                    !predicate_references_activation(pred),
                    "agent '{name}' subscription[{i}] ({} -> {}) MUST NOT reference $activation; \
                     only work_starting subscribers are gated",
                    sub.transition.from,
                    sub.transition.to
                );
            }
        }
    }
    assert!(
        checked > 0,
        "no ungated agents found in fixture; the assertion would be vacuous"
    );
}

/// Sanity: the work_starting subscribers (auto-drive + integrate) DO carry a
/// $activation reference. Pairs with `ungated_subscribers_have_no_activation_predicate`
/// so the file fails loud if the predicate is ever silently dropped.
#[test]
fn gated_subscribers_have_activation_predicate() {
    let agents = fixture_agents_yaml();
    for name in &["auto-drive", "integrate"] {
        let agent = agents
            .agents
            .iter()
            .find(|a| &a.name == name)
            .unwrap_or_else(|| panic!("'{name}' missing from fixture agents.yaml"));
        let any_gated = agent.subscribes_to.iter().any(|s| {
            s.predicate
                .as_ref()
                .map(predicate_references_activation)
                .unwrap_or(false)
        });
        assert!(
            any_gated,
            "agent '{name}' must carry at least one subscription gated on $activation"
        );
    }
}
