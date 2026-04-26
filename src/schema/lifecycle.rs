use crate::schema::actor::Actor;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub verb: String,
    #[serde(default)]
    pub actor: Option<Actor>,
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
}
