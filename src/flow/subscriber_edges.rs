//! Subscriber-edge predicate contracts — compile-time structural coverage assertions
//! for the agent subscription registry.
//!
//! # What subscriber-edge contracts assert
//!
//! Each contract declares a MUST-fire invariant for a specific subscriber agent:
//! every valid lifecycle transition whose `to` status matches the agent's required
//! trigger status must appear in the agent's declared `subscribes_to` list in
//! `agents.yaml`, OR be explicitly listed as an opted-out edge with a documented
//! rationale. Contracts are evaluated at `cargo test` time against two committed
//! canonical surfaces and the bundled lifecycle schemas:
//!
//! - **`docs/agents-yaml-example.yaml`** (compile-time embedded, always present
//!   in the repo): the authoritative docs template for the post-T138 ceremony
//!   (generic integration lane + stores-specific post-`integrated` chain).
//!   Post-integrate contracts (`integrate`, `cargo-install`, `schema-migrate`)
//!   are evaluated against this file directly, so any drift in the docs
//!   template fails the test suite.
//! - **`tests/fixtures/agents.yaml`** (compile-time embedded, complete fixture):
//!   includes all agents — including `auto-promote` (observations store), which
//!   `docs/agents-yaml-example.yaml` does not yet carry. Auto-promote contracts
//!   are evaluated against this fixture. The fixture is kept in sync with the
//!   docs template for the agents it shares; drift is caught by the
//!   `fixture_yaml_includes_t020_builtins` test in `tests/`.
//!
//! Both surfaces are committed and `include_str!`'d at compile time so the
//! contracts run reproducibly in any checkout. The previous third surface —
//! a runtime read of `.stores/agents.yaml` — was removed because `.stores/`
//! is gitignored, the symlink was a per-worktree operator artifact, and in
//! this repo it always pointed back to one of the two committed surfaces
//! (so the smoke test added no independent signal while making `cargo test`
//! depend on uncommitted local state).
//!
//! A failing contract fails the test suite, preventing I027/I024-class
//! silent-omission regressions from shipping.
//!
//! This module tests subscriber-config × lifecycle *structure*; it does NOT
//! test the correctness of individual subscriber side-effect implementations.
//!
//! # L504-A vs L507 surface distinction
//!
//! - **L504-A** (brief-content contracts): predicates over rendered brief text —
//!   does the text a planner or executor sees contain the required sections?
//! - **L507** (this module, subscriber-edge contracts): predicates over
//!   `agents.yaml` + lifecycle schema structure — does the subscriber
//!   declaration cover every reachable transition leading to its trigger status?
//!
//! These are orthogonal surfaces. L504-A fails when rendered content drifts;
//! L507 fails when subscription wiring drifts. Both are test-time, not runtime.
//!
//! # Scope: STRUCTURAL coverage only; I026 is NOT closed
//!
//! This module asserts STRUCTURAL coverage: that the correct edges are declared
//! in the subscriber's `subscribes_to` list. It does NOT assert that the
//! subscriber's side-effect implementation is correct at runtime. I026 (cognition
//! gap — the subscriber fires but produces wrong output) is a separate problem
//! that requires integration testing of the subscriber's implementation, not
//! subscription wiring analysis. This slice does NOT close I026.
//!
//! # How to add a contract
//!
//! 1. Define a unit struct (e.g. `struct MyAgentContract;`).
//! 2. Implement `SubscriberEdgeContract` for it, overriding `name()`, `store()`,
//!    `required_to_status()`, and `agent_name()`. Override `opt_outs()` if any
//!    reachable-to_status edges should NOT trigger this subscriber.
//! 3. Add a static instance: `static MY_CONTRACT: MyAgentContract = MyAgentContract;`
//! 4. Register it in `REGISTRY` (in declaration order).
//! 5. Add two tests: `<name>_passes_against_canonical_example` and
//!    `<name>_fails_when_subscription_dropped` (using an inline YAML fixture,
//!    NOT by mutating `docs/agents-yaml-example.yaml` at test time).
//!
//! # Opt-out doctrine
//!
//! The default `evaluate` body covers EVERY transition where
//! `t.to == required_to_status()`, regardless of verb or actor. Reachable
//! to_status edges that should NOT fire the subscriber must be added to
//! `opt_outs()` as `(from, to, verb, rationale)` tuples. Today ALL contracts
//! have empty `opt_outs()` — every reachable to_status edge is covered.
//! Future framework-fired transitions reaching a required to_status MUST either
//! be subscribed OR explicitly added to `opt_outs` with a rationale string.
//!
//! # Next-slice candidates
//!
//! - `auto_resolve_must_fire_on_every_terminal_success_to_state_with_linked_obs_at_ready`
//!   (I024 cohort: `accepted/cargo_installed/closed_out_of_band/schema_migrated → ready`
//!   edges): deferred because it interacts with `linked_observations` row-state
//!   predicates and would need either new opt-out machinery or would duplicate the
//!   a9a0c79 pinning test without providing new signal. Revisit in the next slice.

use crate::flow::agents_yaml::AgentsYaml;
use crate::flow::checks::CheckResult;
use crate::schema::Transition;
use serde_json::json;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A subscriber-edge predicate contract.
///
/// Each implementation asserts that a named subscriber agent covers every
/// reachable lifecycle transition into its required `to` status. The default
/// `evaluate` implementation handles the predicate logic; implementors only
/// override the four metadata methods (and optionally `opt_outs`).
pub trait SubscriberEdgeContract: Sync {
    fn name(&self) -> &'static str;
    fn store(&self) -> &'static str;
    fn required_to_status(&self) -> &'static str;
    fn agent_name(&self) -> &'static str;

    /// Opt-out table: `(from, to, verb, rationale)`. Default: empty.
    ///
    /// Add entries here when a transition reaching `required_to_status`
    /// intentionally should NOT trigger this subscriber. The rationale string
    /// is documentation-only; it appears in the opt_outs slice so reviewers
    /// can see why the edge is excluded.
    fn opt_outs(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] {
        &[]
    }

    /// Evaluate the contract against the given agents config and lifecycle transitions.
    ///
    /// 1. Gather all transitions where `t.to == required_to_status()`.
    /// 2. Subtract entries listed in `opt_outs()` (matched by from + to + verb).
    /// 3. For each remaining transition, verify the named agent has a Subscription
    ///    with matching `store`, `transition.from`, and `transition.to`.
    /// 4. Pass when subscription set ⊇ filtered transition set.
    ///    Fail with `{missing_edges: [{from, to, verb}...], message}` otherwise.
    fn evaluate(&self, agents: &AgentsYaml, transitions: &[Transition]) -> CheckResult {
        let required_to = self.required_to_status();
        let agent_name = self.agent_name();
        let store = self.store();
        let opt_outs = self.opt_outs();
        let args = json!({
            "store": store,
            "agent": agent_name,
            "required_to_status": required_to
        });

        // Step 1: all transitions ending at required_to_status
        let relevant: Vec<&Transition> = transitions
            .iter()
            .filter(|t| t.to == required_to)
            .collect();

        // Step 2: subtract opt_outs (match by from + to + verb)
        let filtered: Vec<&Transition> = relevant
            .into_iter()
            .filter(|t| {
                !opt_outs
                    .iter()
                    .any(|(f, to, v, _)| t.from == *f && t.to == *to && t.verb == *v)
            })
            .collect();

        // Step 3: locate the named agent in the config
        let agent_entry = agents.agents.iter().find(|a| a.name == agent_name);

        // Step 4: check that each filtered transition is covered by a subscription
        let mut missing_edges: Vec<serde_json::Value> = Vec::new();
        for t in &filtered {
            let covered = match &agent_entry {
                Some(a) => a.subscribes_to.iter().any(|sub| {
                    sub.store == store
                        && sub.transition.from == t.from
                        && sub.transition.to == t.to
                }),
                None => false,
            };
            if !covered {
                missing_edges.push(json!({"from": t.from, "to": t.to, "verb": t.verb}));
            }
        }

        if missing_edges.is_empty() {
            CheckResult::pass(self.name(), &args)
        } else {
            let count = missing_edges.len();
            CheckResult::fail(
                self.name(),
                &args,
                json!({
                    "missing_edges": missing_edges,
                    "message": format!(
                        "agent '{}' is missing {} subscription(s) into status '{}'",
                        agent_name, count, required_to
                    )
                }),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Contract implementations
// ---------------------------------------------------------------------------

struct IntegrateContract;
struct CargoInstallContract;
struct SchemaMigrateContract;
struct AutoPromoteContract;

impl SubscriberEdgeContract for IntegrateContract {
    fn name(&self) -> &'static str {
        "integrate_must_fire_on_every_integration_queued_to_status"
    }
    fn store(&self) -> &'static str {
        "tasks"
    }
    fn required_to_status(&self) -> &'static str {
        "integration_queued"
    }
    fn agent_name(&self) -> &'static str {
        "integrate"
    }
}

impl SubscriberEdgeContract for CargoInstallContract {
    fn name(&self) -> &'static str {
        // T138 P3: cargo-install moved off the (accepted-entry) edges and
        // is now a stores-repo-specific subscriber on (integrating→integrated).
        "cargo_install_must_fire_on_every_integrated_to_status"
    }
    fn store(&self) -> &'static str {
        "tasks"
    }
    fn required_to_status(&self) -> &'static str {
        "integrated"
    }
    fn agent_name(&self) -> &'static str {
        "cargo-install"
    }
    fn opt_outs(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] {
        &[
            ("integrated", "integrated", "mark_cargo_installed", "stores-only post_integration_step self-transition; do not re-run cargo-install"),
            ("integrated", "integrated", "mark_schema_migrated", "stores-only post_integration_step self-transition; do not re-run cargo-install"),
            ("integrated", "integrated", "mark_deploy_blocked", "stores-only post_integration_step self-transition; do not re-run cargo-install"),
        ]
    }
}

// T146 P6: schema-migrate chains from cargo-install's `integrated → integrated`
// self-transition and the stores-only post_integration_step value, not from a
// repo-specific generic status state.
impl SubscriberEdgeContract for SchemaMigrateContract {
    fn name(&self) -> &'static str {
        "schema_migrate_must_fire_on_cargo_post_integration_step"
    }
    fn store(&self) -> &'static str {
        "tasks"
    }
    fn required_to_status(&self) -> &'static str {
        "integrated"
    }
    fn agent_name(&self) -> &'static str {
        "schema-migrate"
    }
    fn opt_outs(&self) -> &'static [(&'static str, &'static str, &'static str, &'static str)] {
        &[
            ("integrating", "integrated", "mark_verify_done", "schema-migrate waits for cargo-install post_integration_step"),
            ("integrating", "integrated", "mark_integrated", "schema-migrate waits for cargo-install post_integration_step"),
            ("integrated", "integrated", "mark_schema_migrated", "schema-migrate terminal self-transition"),
            ("integrated", "integrated", "mark_deploy_blocked", "deploy-blocked post_integration_step self-transition"),
        ]
    }
}

impl SubscriberEdgeContract for AutoPromoteContract {
    fn name(&self) -> &'static str {
        "auto_promote_must_fire_on_every_observation_ready_state"
    }
    fn store(&self) -> &'static str {
        "observations"
    }
    fn required_to_status(&self) -> &'static str {
        "ready"
    }
    fn agent_name(&self) -> &'static str {
        "auto-promote"
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

static INTEGRATE_CONTRACT: IntegrateContract = IntegrateContract;
static CARGO_INSTALL_CONTRACT: CargoInstallContract = CargoInstallContract;
static SCHEMA_MIGRATE_CONTRACT: SchemaMigrateContract = SchemaMigrateContract;
static AUTO_PROMOTE_CONTRACT: AutoPromoteContract = AutoPromoteContract;

static REGISTRY: &[&dyn SubscriberEdgeContract] = &[
    &INTEGRATE_CONTRACT,
    &CARGO_INSTALL_CONTRACT,
    &SCHEMA_MIGRATE_CONTRACT,
    &AUTO_PROMOTE_CONTRACT,
];

pub fn registry() -> &'static [&'static dyn SubscriberEdgeContract] {
    REGISTRY
}

pub fn lookup(name: &str) -> Option<&'static dyn SubscriberEdgeContract> {
    REGISTRY.iter().copied().find(|c| c.name() == name)
}

// ---------------------------------------------------------------------------
// Test helpers (compiled only in test builds; private to this module)
// ---------------------------------------------------------------------------

/// Complete fixture including all agents (integrate, cargo-install,
/// schema-migrate, auto-promote, …). Used for the auto-promote contract and
/// the runtime non-regression test, because `docs/agents-yaml-example.yaml`
/// does not yet carry observations-store agents.
fn load_canonical_agents() -> AgentsYaml {
    const YAML: &str = include_str!("../../tests/fixtures/agents.yaml");
    AgentsYaml::from_yaml(YAML).expect("tests/fixtures/agents.yaml must parse cleanly")
}

/// Embeds the canonical docs template (`docs/agents-yaml-example.yaml`) at
/// compile time. Post-integrate contracts evaluate against this surface
/// directly so that any omission in the docs template fails the test suite
/// immediately. Auto-promote is NOT in the docs template yet; see
/// `load_canonical_agents`.
fn load_docs_example_agents() -> AgentsYaml {
    const YAML: &str = include_str!("../../docs/agents-yaml-example.yaml");
    AgentsYaml::from_yaml(YAML).expect("docs/agents-yaml-example.yaml must parse cleanly")
}

fn load_transitions(store: &str) -> Vec<Transition> {
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::schema::Schema;
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == store)
        .map(|(_, y)| *y)
        .unwrap_or_else(|| panic!("bundled schema not found for store '{store}'"));
    Schema::from_yaml(yaml)
        .unwrap_or_else(|e| panic!("failed to parse {store} schema: {e}"))
        .lifecycle
        .transitions
}

fn parse_inline_agents(yaml: &str) -> AgentsYaml {
    AgentsYaml::from_yaml(yaml).expect("inline agents yaml must parse")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::checks::CheckOutcome;

    // -----------------------------------------------------------------------
    // (a) Registry shape
    // -----------------------------------------------------------------------

    #[test]
    fn registry_lists_expected_named_contracts() {
        let names: Vec<&str> = registry().iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            vec![
                "integrate_must_fire_on_every_integration_queued_to_status",
                "cargo_install_must_fire_on_every_integrated_to_status",
                "schema_migrate_must_fire_on_cargo_post_integration_step",
                "auto_promote_must_fire_on_every_observation_ready_state",
            ],
            "registry order must match declaration order"
        );
        assert!(
            lookup("integrate_must_fire_on_every_integration_queued_to_status").is_some(),
            "lookup must find integrate contract"
        );
        assert!(
            lookup("cargo_install_must_fire_on_every_integrated_to_status").is_some(),
            "lookup must find cargo_install contract"
        );
        assert!(
            lookup("schema_migrate_must_fire_on_cargo_post_integration_step").is_some(),
            "lookup must find schema_migrate contract"
        );
        assert!(
            lookup("auto_promote_must_fire_on_every_observation_ready_state").is_some(),
            "lookup must find auto_promote contract"
        );
        assert!(
            lookup("nonexistent_contract").is_none(),
            "lookup of unknown name must return None"
        );
    }

    // -----------------------------------------------------------------------
    // (b–e) Passing-fixture tests: each contract passes against canonical example
    // -----------------------------------------------------------------------

    #[test]
    fn integrate_passes_against_canonical_example() {
        let agents = load_canonical_agents();
        let transitions = load_transitions("tasks");
        let result = INTEGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "integrate contract should pass; reason: {:?}",
            result.reason
        );
    }

    #[test]
    fn cargo_install_passes_against_canonical_example() {
        let agents = load_canonical_agents();
        let transitions = load_transitions("tasks");
        let result = CARGO_INSTALL_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "cargo-install contract should pass; reason: {:?}",
            result.reason
        );
    }

    #[test]
    fn schema_migrate_passes_against_canonical_example() {
        let agents = load_canonical_agents();
        let transitions = load_transitions("tasks");
        let result = SCHEMA_MIGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "schema-migrate contract should pass; reason: {:?}",
            result.reason
        );
    }

    #[test]
    fn auto_promote_passes_against_canonical_example() {
        let agents = load_canonical_agents();
        let transitions = load_transitions("observations");
        let result = AUTO_PROMOTE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "auto-promote contract should pass; reason: {:?}",
            result.reason
        );
    }

    // -----------------------------------------------------------------------
    // (f–i) Failing-fixture tests: each contract fails when a subscription is
    //       dropped from an inline YAML fixture (NOT from the canonical file)
    // -----------------------------------------------------------------------

    #[test]
    fn integrate_fails_when_either_entry_subscription_dropped() {
        // T138 P3 AC3.6: the integrate contract requires BOTH entry edges into
        // integration_queued — (accepted, integration_queued) and
        // (integration_blocked, integration_queued). Dropping either one fails.

        // (1) Both subscriptions present → contract passes.
        let yaml_both = r#"
agents:
  - name: integrate
    subscribes_to:
      - store: tasks
        transition: { from: accepted, to: integration_queued }
      - store: tasks
        transition: { from: complete, to: integration_queued }
      - store: tasks
        transition: { from: in_review, to: integration_queued }
      - store: tasks
        transition: { from: integration_blocked, to: integration_queued }
    command: "builtin:integrate"
"#;
        let agents = parse_inline_agents(yaml_both);
        let transitions = load_transitions("tasks");
        let result = INTEGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "integrate contract must pass when both entry edges are subscribed; reason: {:?}",
            result.reason
        );

        // (2) Drop (integration_blocked → integration_queued).
        let yaml_drop_retry = r#"
agents:
  - name: integrate
    subscribes_to:
      - store: tasks
        transition: { from: accepted, to: integration_queued }
      - store: tasks
        transition: { from: complete, to: integration_queued }
      - store: tasks
        transition: { from: in_review, to: integration_queued }
    command: "builtin:integrate"
"#;
        let agents = parse_inline_agents(yaml_drop_retry);
        let result = INTEGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "contract must fail when (integration_blocked, integration_queued) subscription is absent"
        );
        let reason = result.reason.unwrap();
        let missing = reason["missing_edges"].as_array().expect("missing_edges must be an array");
        assert!(
            missing
                .iter()
                .any(|e| e["from"] == "integration_blocked" && e["to"] == "integration_queued"),
            "expected (integration_blocked, integration_queued) in missing_edges; got: {:?}",
            missing
        );

        // (3) Drop (accepted → integration_queued).
        let yaml_drop_accept = r#"
agents:
  - name: integrate
    subscribes_to:
      - store: tasks
        transition: { from: complete, to: integration_queued }
      - store: tasks
        transition: { from: in_review, to: integration_queued }
      - store: tasks
        transition: { from: integration_blocked, to: integration_queued }
    command: "builtin:integrate"
"#;
        let agents = parse_inline_agents(yaml_drop_accept);
        let result = INTEGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "contract must fail when (accepted, integration_queued) subscription is absent"
        );
        let reason = result.reason.unwrap();
        let missing = reason["missing_edges"].as_array().expect("missing_edges must be an array");
        assert!(
            missing
                .iter()
                .any(|e| e["from"] == "accepted" && e["to"] == "integration_queued"),
            "expected (accepted, integration_queued) in missing_edges; got: {:?}",
            missing
        );
    }

    #[test]
    fn cargo_install_fails_when_subscription_dropped() {
        // T138 P3: the only edge into `integrated` is (integrating, integrated).
        // Drop it; agent has no subscriptions.
        let yaml = r#"
agents:
  - name: cargo-install
    subscribes_to: []
    command: "builtin:cargo-install"
"#;
        let agents = parse_inline_agents(yaml);
        let transitions = load_transitions("tasks");
        let result = CARGO_INSTALL_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "contract must fail when (integrating, integrated) subscription is absent"
        );
        let reason = result.reason.unwrap();
        let missing = reason["missing_edges"].as_array().expect("missing_edges must be an array");
        assert!(
            missing
                .iter()
                .any(|e| e["from"] == "integrating" && e["to"] == "integrated"),
            "expected (integrating, integrated) in missing_edges; got: {:?}",
            missing
        );
    }

    #[test]
    fn schema_migrate_fails_when_subscription_dropped() {
        // Drop (integrated → integrated mark_cargo_installed); agent has no subscriptions.
        // T146 P6: schema-migrate observes cargo-install through post_integration_step,
        // not through a repo-specific generic status state.
        let yaml = r#"
agents:
  - name: schema-migrate
    subscribes_to: []
    command: "builtin:schema-migrate"
"#;
        let agents = parse_inline_agents(yaml);
        let transitions = load_transitions("tasks");
        let result = SCHEMA_MIGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "contract must fail when (integrated, integrated) cargo post-step subscription is absent"
        );
        let reason = result.reason.unwrap();
        let missing = reason["missing_edges"].as_array().expect("missing_edges must be an array");
        assert!(
            missing
                .iter()
                .any(|e| e["from"] == "integrated" && e["to"] == "integrated"),
            "expected (integrated, integrated) in missing_edges; got: {:?}",
            missing
        );
    }

    #[test]
    fn auto_promote_fails_when_subscription_dropped() {
        // Drop (confirmed → ready); agent has no subscriptions.
        let yaml = r#"
agents:
  - name: auto-promote
    subscribes_to: []
    command: "builtin:auto-promote"
"#;
        let agents = parse_inline_agents(yaml);
        let transitions = load_transitions("observations");
        let result = AUTO_PROMOTE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "contract must fail when (confirmed, ready) subscription is absent"
        );
        let reason = result.reason.unwrap();
        let missing = reason["missing_edges"].as_array().expect("missing_edges must be an array");
        assert!(
            missing
                .iter()
                .any(|e| e["from"] == "confirmed" && e["to"] == "ready"),
            "expected (confirmed, ready) in missing_edges; got: {:?}",
            missing
        );
    }

    // -----------------------------------------------------------------------
    // Passing-fixture tests against the canonical docs template
    // (docs/agents-yaml-example.yaml) — guards the authoritative surface
    // -----------------------------------------------------------------------

    #[test]
    fn integrate_passes_against_docs_agents_yaml_example() {
        let agents = load_docs_example_agents();
        let transitions = load_transitions("tasks");
        let result = INTEGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "integrate contract must pass against docs/agents-yaml-example.yaml; \
             reason: {:?}",
            result.reason
        );
    }

    #[test]
    fn cargo_install_passes_against_docs_agents_yaml_example() {
        let agents = load_docs_example_agents();
        let transitions = load_transitions("tasks");
        let result = CARGO_INSTALL_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "cargo-install contract must pass against docs/agents-yaml-example.yaml; \
             reason: {:?}",
            result.reason
        );
    }

    #[test]
    fn schema_migrate_passes_against_docs_agents_yaml_example() {
        let agents = load_docs_example_agents();
        let transitions = load_transitions("tasks");
        let result = SCHEMA_MIGRATE_CONTRACT.evaluate(&agents, &transitions);
        assert_eq!(
            result.outcome,
            CheckOutcome::Pass,
            "schema-migrate contract must pass against docs/agents-yaml-example.yaml; \
             reason: {:?}",
            result.reason
        );
    }

    // -----------------------------------------------------------------------
    // (j) Smoke runs: all contracts pass against both canonical surfaces
    // -----------------------------------------------------------------------

    /// Smoke run against tests/fixtures/agents.yaml (complete fixture including
    /// auto-promote). Layered drift-guard alongside a9a0c79.
    #[test]
    fn all_contracts_pass_against_canonical_example() {
        let agents = load_canonical_agents();
        for contract in registry() {
            let transitions = load_transitions(contract.store());
            let result = contract.evaluate(&agents, &transitions);
            assert_eq!(
                result.outcome,
                CheckOutcome::Pass,
                "contract '{}' failed against tests/fixtures/agents.yaml: {:?}",
                contract.name(),
                result.reason
            );
        }
    }

    /// Smoke run against docs/agents-yaml-example.yaml for the three
    /// post-integrate contracts. Guards the canonical docs template directly so
    /// drift in the docs file fails the test suite, not just the fixture.
    /// Auto-promote is excluded here because docs/agents-yaml-example.yaml
    /// does not yet carry observations-store agents.
    #[test]
    fn post_integrate_contracts_pass_against_docs_agents_yaml_example() {
        let agents = load_docs_example_agents();
        let post_integrate_names = [
            "integrate_must_fire_on_every_integration_queued_to_status",
            "cargo_install_must_fire_on_every_integrated_to_status",
            "schema_migrate_must_fire_on_cargo_post_integration_step",
        ];
        for name in &post_integrate_names {
            let contract = lookup(name).unwrap_or_else(|| {
                panic!("contract '{}' missing from registry", name)
            });
            let transitions = load_transitions(contract.store());
            let result = contract.evaluate(&agents, &transitions);
            assert_eq!(
                result.outcome,
                CheckOutcome::Pass,
                "contract '{}' failed against docs/agents-yaml-example.yaml: {:?}",
                contract.name(),
                result.reason
            );
        }
    }

    // -----------------------------------------------------------------------
    // (k) Opt-outs are documented
    // -----------------------------------------------------------------------

    #[test]
    fn opt_outs_have_rationales_for_all_contracts() {
        for contract in registry() {
            for (_, _, _, rationale) in contract.opt_outs() {
                assert!(
                    !rationale.trim().is_empty(),
                    "contract '{}' has an opt_out without rationale",
                    contract.name()
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // (l) Runtime non-regression: subscription matching mirrors agents_run.rs
    // -----------------------------------------------------------------------

    fn agents_matching_edge<'a>(
        agents: &'a AgentsYaml,
        store: &str,
        from: &str,
        to: &str,
    ) -> Vec<&'a str> {
        agents
            .agents
            .iter()
            .filter(|a| {
                a.subscribes_to.iter().any(|sub| {
                    sub.store == store
                        && sub.transition.from == from
                        && sub.transition.to == to
                })
            })
            .map(|a| a.name.as_str())
            .collect()
    }

    /// Pins runtime dispatch semantics using the same subscription matching shape
    /// as `src/handlers/agents_run.rs` (which queries transition_history by
    /// store + from_status + to_status and iterates agent.subscribes_to).
    ///
    /// Post-T138: the integration-lane entry edges dispatch the `integrate`
    /// builtin (and only that builtin); the post-integrated edge dispatches
    /// `cargo-install` and `auto-resolve-observation`; cargo_installed →
    /// schema_migrated dispatches `schema-migrate` (and auto-resolve).
    #[test]
    fn subscribers_fire_unchanged_for_integration_lane_edges() {
        let agents = load_canonical_agents();

        let mut accepted_to_queued =
            agents_matching_edge(&agents, "tasks", "accepted", "integration_queued");
        accepted_to_queued.sort_unstable();
        assert_eq!(
            accepted_to_queued,
            vec!["integrate"],
            "(accepted → integration_queued) must dispatch only the integrate builtin"
        );

        let mut blocked_to_queued =
            agents_matching_edge(&agents, "tasks", "integration_blocked", "integration_queued");
        blocked_to_queued.sort_unstable();
        assert_eq!(
            blocked_to_queued,
            vec!["integrate"],
            "(integration_blocked → integration_queued) must dispatch only the integrate builtin"
        );

        let mut integrated_subscribers =
            agents_matching_edge(&agents, "tasks", "integrating", "integrated");
        integrated_subscribers.sort_unstable();
        assert_eq!(
            integrated_subscribers,
            vec!["auto-resolve-observation", "cargo-install"],
            "(integrating → integrated) must dispatch cargo-install and auto-resolve-observation"
        );

        let mut cargo_post_step_subscribers =
            agents_matching_edge(&agents, "tasks", "integrated", "integrated");
        cargo_post_step_subscribers.sort_unstable();
        assert_eq!(
            cargo_post_step_subscribers,
            vec!["schema-migrate"],
            "(integrated → integrated with post_integration_step=cargo_installed) must dispatch exactly schema-migrate"
        );
    }
}
