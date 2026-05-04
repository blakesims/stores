//! .stores/agents.yaml — declarative agent registry.
//!
//! Each agent subscribes to a list of {store, transition} triples and runs a
//! command (shell or `builtin:*` keyword) when a matching transition fires.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentsYaml {
    #[serde(default)]
    pub agents: Vec<AgentEntry>,
    /// Name of the agent (must appear in `agents`) that handles
    /// `tasks: accepted -> deploy_blocked` escalations. Defaults to
    /// `builtin:user-escalation` when absent.
    #[serde(default)]
    pub deployment_specialist: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    pub subscribes_to: Vec<Subscription>,
    pub command: String,
    #[serde(default = "default_claim_window_secs", rename = "claim_window_secs")]
    pub claim_window_secs: u64,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// Free-form arguments consumed by the builtin (e.g. cargo-install reads
    /// `command_args.features`). Untyped on purpose so each builtin defines
    /// its own contract; absence means "use defaults".
    #[serde(default)]
    pub command_args: Option<serde_yaml::Mapping>,
}

fn default_claim_window_secs() -> u64 {
    300
}

impl AgentEntry {
    pub fn claim_window(&self) -> Duration {
        Duration::from_secs(self.claim_window_secs)
    }

    /// True if `command` is a `builtin:*` directive rather than a shell command.
    pub fn is_builtin(&self) -> bool {
        self.command.starts_with("builtin:")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Subscription {
    pub store: String,
    pub transition: TransitionEdge,
    /// Optional row-state predicate. When present, the daemon evaluates it
    /// against the row JSON after the policy gate; a false result skips the
    /// claim+dispatch (no ntfy). Reuses `flow::predicate::PredicateExpr` so
    /// the syntax matches policies.yaml.
    #[serde(default)]
    pub predicate: Option<crate::flow::predicate::PredicateExpr>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TransitionEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetryPolicy {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default)]
    pub backoff: BackoffKind,
}

fn default_max_attempts() -> u32 {
    3
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            backoff: BackoffKind::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackoffKind {
    Linear,
    Exponential,
}

impl Default for BackoffKind {
    fn default() -> Self {
        BackoffKind::Linear
    }
}

impl AgentsYaml {
    /// Parse + structurally validate.
    pub fn from_yaml(s: &str) -> Result<Self> {
        let parsed: Self = serde_yaml::from_str(s)
            .map_err(|e| anyhow!("agents.yaml parse error: {}", format_yaml_error(&e)))?;
        parsed.validate()?;
        Ok(parsed)
    }

    fn validate(&self) -> Result<()> {
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, a) in self.agents.iter().enumerate() {
            if a.name.is_empty() {
                bail!("agents[{}].name: empty string not permitted", i);
            }
            if !seen.insert(a.name.as_str()) {
                bail!("agents[{}].name: duplicate agent name '{}'", i, a.name);
            }
            if a.command.trim().is_empty() {
                bail!("agents[{}].command: empty command not permitted", i);
            }
            if a.is_builtin() {
                let kw = a.command.trim_start_matches("builtin:");
                if kw.is_empty() {
                    bail!(
                        "agents[{}].command: 'builtin:' prefix requires a keyword",
                        i
                    );
                }
            }
            for (j, sub) in a.subscribes_to.iter().enumerate() {
                if sub.store.is_empty() {
                    bail!("agents[{}].subscribes_to[{}].store: empty", i, j);
                }
                // `from` may be empty-string: that's the row-creation
                // arrival convention (T020 Phase 2 writes a synthetic
                // create-event with from_status=''). `to` must be non-empty
                // — a subscription with no destination state is meaningless.
                if sub.transition.to.is_empty() {
                    bail!(
                        "agents[{}].subscribes_to[{}].transition.to: must be non-empty",
                        i,
                        j
                    );
                }
            }
        }
        if let Some(spec) = &self.deployment_specialist {
            if !self.agents.iter().any(|a| &a.name == spec) {
                bail!(
                    "deployment_specialist: '{}' is not declared in agents[]",
                    spec
                );
            }
        }
        Ok(())
    }
}

fn format_yaml_error(e: &serde_yaml::Error) -> String {
    // serde_yaml errors include location + field path when present.
    e.to_string()
}

pub fn load_from_path(path: &std::path::Path) -> Result<AgentsYaml> {
    let bytes =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    AgentsYaml::from_yaml(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WELL_FORMED: &str = r#"
agents:
  - name: accept-merge
    subscribes_to:
      - store: tasks
        transition:
          from: in_review
          to: accepted
    command: "builtin:accept-merge"
    claim_window_secs: 300
    retry_policy:
      max_attempts: 3
      backoff: linear
  - name: user-escalation
    subscribes_to:
      - store: tasks
        transition: { from: accepted, to: deploy_blocked }
    command: "builtin:user-escalation"
deployment_specialist: user-escalation
"#;

    #[test]
    fn parses_well_formed_fixture() {
        let p = AgentsYaml::from_yaml(WELL_FORMED).unwrap();
        assert_eq!(p.agents.len(), 2);
        assert_eq!(p.agents[0].name, "accept-merge");
        assert!(p.agents[0].is_builtin());
        assert_eq!(p.agents[0].claim_window(), Duration::from_secs(300));
        assert_eq!(p.agents[0].retry_policy.max_attempts, 3);
        assert_eq!(p.agents[0].retry_policy.backoff, BackoffKind::Linear);
        assert_eq!(p.deployment_specialist.as_deref(), Some("user-escalation"));
    }

    #[test]
    fn defaults_applied() {
        let yaml = r#"
agents:
  - name: a
    subscribes_to:
      - store: tasks
        transition: { from: x, to: y }
    command: "/bin/true"
"#;
        let p = AgentsYaml::from_yaml(yaml).unwrap();
        assert_eq!(p.agents[0].claim_window_secs, 300);
        assert_eq!(p.agents[0].retry_policy.max_attempts, 3);
    }

    #[test]
    fn duplicate_name_rejected() {
        let yaml = r#"
agents:
  - name: dup
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
    command: "/bin/true"
  - name: dup
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
    command: "/bin/true"
"#;
        let err = AgentsYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("duplicate agent name"), "got: {err}");
        assert!(err.contains("agents[1]"), "got: {err}");
    }

    #[test]
    fn missing_required_field_reports_path() {
        // Missing `command`.
        let yaml = r#"
agents:
  - name: a
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
"#;
        let err = AgentsYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("command"), "expected field path; got: {err}");
    }

    #[test]
    fn bad_builtin_rejected() {
        let yaml = r#"
agents:
  - name: a
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
    command: "builtin:"
"#;
        let err = AgentsYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("builtin"), "got: {err}");
    }

    /// AC2.4: agents.yaml fixture with `command: "builtin:cargo-install"`
    /// (and an optional command_args.features list) parses cleanly.
    #[test]
    fn cargo_install_entry_parses() {
        let yaml = r#"
agents:
  - name: cargo-install
    subscribes_to:
      - store: tasks
        transition: { from: accepted, to: cargo_installed }
    command: "builtin:cargo-install"
    command_args:
      features:
        - runner-claude-code
        - daemon
"#;
        let p = AgentsYaml::from_yaml(yaml).unwrap();
        assert_eq!(p.agents.len(), 1);
        assert_eq!(p.agents[0].command, "builtin:cargo-install");
        assert!(p.agents[0].is_builtin());
        let args = p.agents[0].command_args.as_ref().unwrap();
        let feats = args
            .get(serde_yaml::Value::String("features".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(feats.len(), 2);
    }

    /// AC5.2: row-creation arrival convention — empty-string `from` is now
    /// accepted by the validator (was previously rejected).
    #[test]
    fn empty_from_status_is_allowed() {
        let yaml = r#"
agents:
  - name: auto-scaffold
    subscribes_to:
      - store: tasks
        transition: { from: "", to: planning }
    command: "builtin:auto-scaffold"
"#;
        let p = AgentsYaml::from_yaml(yaml).expect("empty-string from must parse");
        assert_eq!(p.agents[0].subscribes_to[0].transition.from, "");
        assert_eq!(p.agents[0].subscribes_to[0].transition.to, "planning");
    }

    /// AC5.3: empty `to` is still rejected — a subscription with no
    /// destination state is meaningless.
    #[test]
    fn empty_to_status_still_rejected() {
        let yaml = r#"
agents:
  - name: bad
    subscribes_to:
      - store: tasks
        transition: { from: planning, to: "" }
    command: "/bin/true"
"#;
        let err = AgentsYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("transition.to"), "got: {err}");
    }

    /// AC5.1: the bundled tests/fixtures/agents.yaml parses cleanly and
    /// contains the two new T020 builtins (`auto-promote`, `auto-scaffold`)
    /// with the expected subscription edges.
    #[test]
    fn fixture_yaml_includes_t020_builtins() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents.yaml");
        let p = load_from_path(&path).expect("fixture must parse");
        let names: Vec<&str> = p.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(
            names.contains(&"auto-promote"),
            "fixture missing auto-promote; got: {names:?}"
        );
        assert!(
            names.contains(&"auto-scaffold"),
            "fixture missing auto-scaffold; got: {names:?}"
        );

        let promote = p.agents.iter().find(|a| a.name == "auto-promote").unwrap();
        assert_eq!(promote.command, "builtin:auto-promote");
        assert_eq!(promote.subscribes_to[0].store, "observations");
        assert_eq!(promote.subscribes_to[0].transition.from, "confirmed");
        assert_eq!(promote.subscribes_to[0].transition.to, "ready");
        assert_eq!(promote.retry_policy.max_attempts, 1);

        let scaffold = p.agents.iter().find(|a| a.name == "auto-scaffold").unwrap();
        assert_eq!(scaffold.command, "builtin:auto-scaffold");
        assert_eq!(scaffold.subscribes_to[0].store, "tasks");
        assert_eq!(scaffold.subscribes_to[0].transition.from, "");
        assert_eq!(scaffold.subscribes_to[0].transition.to, "planning");
        assert_eq!(scaffold.retry_policy.max_attempts, 1);
    }

    /// T022 P6 / AC6.1: tests/fixtures/agents.yaml carries the auto-drive entry
    /// with the `workspace_path != ""` predicate gate.
    #[test]
    fn fixture_yaml_includes_auto_drive() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents.yaml");
        let p = load_from_path(&path).expect("fixture must parse");
        let drive = p
            .agents
            .iter()
            .find(|a| a.name == "auto-drive")
            .expect("fixture missing auto-drive entry");
        assert_eq!(drive.command, "builtin:auto-drive");
        assert_eq!(drive.retry_policy.max_attempts, 1);
        let sub = &drive.subscribes_to[0];
        assert_eq!(sub.store, "tasks");
        assert_eq!(sub.transition.from, "");
        assert_eq!(sub.transition.to, "planning");
        let pred = sub
            .predicate
            .as_ref()
            .expect("auto-drive subscription must carry a predicate");
        match pred {
            crate::flow::predicate::PredicateExpr::Neq { left, right } => {
                assert_eq!(left.as_str(), Some("$workspace_path"));
                assert_eq!(right.as_str(), Some(""));
            }
            other => panic!("expected Neq predicate, got {:?}", other),
        }
    }

    /// T022 P2 / AC2.1: a subscription bearing a `predicate` block round-trips
    /// through the parser and lands as a `PredicateExpr::Neq` (i.e. the
    /// declarative `workspace_path != ""` gate is preserved).
    #[test]
    fn subscription_with_predicate_parses() {
        let yaml = r#"
agents:
  - name: auto-drive
    subscribes_to:
      - store: tasks
        transition: { from: "", to: planning }
        predicate:
          op: "!="
          left: "$workspace_path"
          right: ""
    command: "builtin:auto-drive"
"#;
        let p = AgentsYaml::from_yaml(yaml).expect("predicate-bearing entry must parse");
        let sub = &p.agents[0].subscribes_to[0];
        let pred = sub.predicate.as_ref().expect("predicate must be present");
        match pred {
            crate::flow::predicate::PredicateExpr::Neq { left, right } => {
                assert_eq!(left.as_str(), Some("$workspace_path"));
                assert_eq!(right.as_str(), Some(""));
            }
            other => panic!("expected Neq predicate, got {:?}", other),
        }
    }

    /// T022 P2: omitting `predicate` leaves the field as `None` (existing
    /// fixtures must keep parsing untouched).
    #[test]
    fn subscription_without_predicate_defaults_to_none() {
        let yaml = r#"
agents:
  - name: a
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
    command: "/bin/true"
"#;
        let p = AgentsYaml::from_yaml(yaml).unwrap();
        assert!(p.agents[0].subscribes_to[0].predicate.is_none());
    }

    #[test]
    fn deployment_specialist_must_exist() {
        let yaml = r#"
agents:
  - name: a
    subscribes_to:
      - store: tasks
        transition: { from: a, to: b }
    command: "/bin/true"
deployment_specialist: ghost
"#;
        let err = AgentsYaml::from_yaml(yaml).unwrap_err().to_string();
        assert!(err.contains("deployment_specialist"), "got: {err}");
    }
}
