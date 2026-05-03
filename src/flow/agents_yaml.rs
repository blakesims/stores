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
                if sub.transition.from.is_empty() || sub.transition.to.is_empty() {
                    bail!(
                        "agents[{}].subscribes_to[{}].transition: from/to must be non-empty",
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
