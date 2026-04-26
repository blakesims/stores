use crate::schema::actor::Actor;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub verb: String,
    #[serde(default)]
    pub actor: Option<Actor>,
    /// Optional gate key that must match the submit verb's `--gate` argument for
    /// this transition to be selected.  When multiple transitions share the same
    /// `(from, verb)` pair, all but at most one must declare `requires_gate`.
    /// If two transitions share `(from, verb, requires_gate: None)` the schema
    /// fails to load with an "ambiguous transition selection" error.  (Task 1.10)
    #[serde(default)]
    pub requires_gate: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lifecycle {
    pub states: Vec<String>,
    /// Explicit initial state; if absent, defaults to `states[0]` at resolution time.
    pub initial_state: Option<String>,
    #[serde(default)]
    pub transitions: Vec<Transition>,
}

impl Lifecycle {
    /// Resolve the initial state, defaulting to `states[0]`.
    pub fn resolved_initial_state(&self) -> anyhow::Result<&str> {
        if let Some(ref s) = self.initial_state {
            return Ok(s.as_str());
        }
        self.states
            .first()
            .map(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("lifecycle.states is empty"))
    }

    /// Validate transition ambiguity: for each `(from, verb)` pair, at most one
    /// transition may have `requires_gate: None`.  Returns an error naming the
    /// ambiguous pair if two unguarded transitions share the same `(from, verb)`.
    pub fn validate_transition_ambiguity(&self) -> anyhow::Result<()> {
        // Map (from, verb) → count of transitions with requires_gate == None
        let mut unguarded_count: HashMap<(String, String), Vec<&str>> = HashMap::new();

        for t in &self.transitions {
            if t.requires_gate.is_none() {
                let key = (t.from.clone(), t.verb.clone());
                unguarded_count
                    .entry(key)
                    .or_default()
                    .push(&t.to);
            }
        }

        for ((from, verb), targets) in &unguarded_count {
            if targets.len() > 1 {
                anyhow::bail!(
                    "ambiguous transition selection: ({from}, verb={verb}) has {} transitions \
                     with requires_gate: null (targets: {:?}); all but one must declare requires_gate",
                    targets.len(),
                    targets
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_defaults_to_first() {
        let lc: Lifecycle = serde_yaml::from_str(
            "states: [triage, active, done]\ntransitions: []",
        )
        .unwrap();
        assert!(lc.initial_state.is_none());
        assert_eq!(lc.resolved_initial_state().unwrap(), "triage");
    }

    #[test]
    fn initial_state_explicit_override() {
        let lc: Lifecycle = serde_yaml::from_str(
            "states: [triage, active, done]\ninitial_state: active\ntransitions: []",
        )
        .unwrap();
        assert_eq!(lc.initial_state.as_deref(), Some("active"));
        assert_eq!(lc.resolved_initial_state().unwrap(), "active");
    }

    #[test]
    fn transitions_with_actor() {
        let lc: Lifecycle = serde_yaml::from_str(
            "states: [open, closed]\ntransitions:\n  - from: open\n    to: closed\n    verb: close\n    actor: human",
        )
        .unwrap();
        assert_eq!(lc.transitions.len(), 1);
        assert_eq!(lc.transitions[0].actor, Some(Actor::Human));
    }

    // ---- Task 1.10: requires_gate ----

    #[test]
    fn requires_gate_parses() {
        let lc: Lifecycle = serde_yaml::from_str(r#"
states: [code_review, executing, blocked, complete]
transitions:
  - from: code_review
    to: executing
    verb: submit-review
    requires_gate: REVISE
    actor: ai_autonomous
  - from: code_review
    to: complete
    verb: submit-review
    requires_gate: PASS
    actor: ai_autonomous
  - from: code_review
    to: blocked
    verb: submit-review
    requires_gate: FAIL
    actor: ai_autonomous
"#).unwrap();
        let revise = lc.transitions.iter().find(|t| t.to == "executing").unwrap();
        assert_eq!(revise.requires_gate.as_deref(), Some("REVISE"));
        let pass = lc.transitions.iter().find(|t| t.to == "complete").unwrap();
        assert_eq!(pass.requires_gate.as_deref(), Some("PASS"));
        // Ambiguity check: all three have requires_gate, no unguarded pair
        assert!(lc.validate_transition_ambiguity().is_ok());
    }

    #[test]
    fn no_requires_gate_defaults_to_none() {
        let lc: Lifecycle = serde_yaml::from_str(r#"
states: [open, done]
transitions:
  - from: open
    to: done
    verb: close
"#).unwrap();
        assert!(lc.transitions[0].requires_gate.is_none());
    }

    #[test]
    fn ambiguous_transition_selection_errors() {
        // Two transitions share (from=code_review, verb=submit-review, requires_gate=None)
        let yaml = r#"
states: [code_review, executing, blocked]
transitions:
  - from: code_review
    to: executing
    verb: submit-review
  - from: code_review
    to: blocked
    verb: submit-review
"#;
        let lc: Lifecycle = serde_yaml::from_str(yaml).unwrap();
        let err = lc.validate_transition_ambiguity().unwrap_err();
        assert!(
            err.to_string().contains("ambiguous transition selection"),
            "err: {err}"
        );
        assert!(
            err.to_string().contains("submit-review"),
            "err should name the verb: {err}"
        );
    }

    #[test]
    fn one_unguarded_is_allowed() {
        // One unguarded + one guarded with same (from, verb) is fine
        let yaml = r#"
states: [code_review, executing, blocked]
transitions:
  - from: code_review
    to: executing
    verb: submit-review
    requires_gate: REVISE
  - from: code_review
    to: blocked
    verb: submit-review
"#;
        let lc: Lifecycle = serde_yaml::from_str(yaml).unwrap();
        assert!(lc.validate_transition_ambiguity().is_ok());
    }
}
