use crate::schema::actor::Actor;
use crate::schema::expr::{parse_guard, Expr};
use crate::validate::expr_eval::eval;
use crate::validate::EntryMap;
use anyhow::bail;
use serde::Deserialize;
use std::collections::HashMap;

/// Helper for deserializing the `guard:` field as a string, then parsing it.
fn deserialize_guard<'de, D>(d: D) -> Result<Option<Expr>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => parse_guard(&s)
            .map(Some)
            .map_err(|e| D::Error::custom(e.to_string())),
    }
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub verb: String,
    pub actor: Option<Actor>,
    /// Optional gate key that must match the submit verb's `--gate` argument for
    /// this transition to be selected.  When multiple transitions share the same
    /// `(from, verb)` pair, all but at most one must declare `requires_gate`.
    /// If two transitions share `(from, verb, requires_gate: None)` the schema
    /// fails to load with an "ambiguous transition selection" error.  (Task 1.10)
    pub requires_gate: Option<String>,
    /// Optional guard expression evaluated against the (engine-bumped) merged entry
    /// BEFORE the transition is applied.  If the guard is present and evaluates
    /// false, the engine looks for a fallback transition.  (Task 5.2)
    pub guard: Option<Expr>,
}

impl<'de> Deserialize<'de> for Transition {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            from: String,
            to: String,
            verb: String,
            #[serde(default)]
            actor: Option<Actor>,
            #[serde(default)]
            requires_gate: Option<String>,
            #[serde(default, deserialize_with = "deserialize_guard")]
            guard: Option<Expr>,
        }
        let r = Raw::deserialize(d)?;
        Ok(Transition {
            from: r.from,
            to: r.to,
            verb: r.verb,
            actor: r.actor,
            requires_gate: r.requires_gate,
            guard: r.guard,
        })
    }
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
    /// transition may be fully unambiguous (no `requires_gate` AND no `guard`).
    /// Guard-partitioned pairs (all transitions in the pair carry a `guard:`) are
    /// allowed — `select_transition` resolves them at runtime by evaluating guards.
    /// Returns an error naming the ambiguous pair if two fully-unguarded transitions
    /// share the same `(from, verb)`.
    pub fn validate_transition_ambiguity(&self) -> anyhow::Result<()> {
        // Map (from, verb) → count of transitions with requires_gate == None AND guard == None
        let mut fully_unguarded: HashMap<(String, String), Vec<&str>> = HashMap::new();

        for t in &self.transitions {
            if t.requires_gate.is_none() && t.guard.is_none() {
                let key = (t.from.clone(), t.verb.clone());
                fully_unguarded.entry(key).or_default().push(&t.to);
            }
        }

        for ((from, verb), targets) in &fully_unguarded {
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

// ---------------------------------------------------------------------------
// Runtime transition selection (shared between submit.rs and transition.rs)
// ---------------------------------------------------------------------------

/// Select the best matching transition from `transitions` for a given
/// (`from_state`, `verb`, `gate`) triple, evaluated against `entry`.
///
/// Algorithm (mirrors `submit::find_transition` / task 5.5 / 5.5b):
/// 1. Filter candidates by `(from, verb, requires_gate)`.
/// 2. Among candidates, prefer the one whose `guard` evaluates true.
///    If two guards both evaluate true, that is a schema-design error → bail.
/// 3. If no guarded-true candidate exists, fall back to the unguarded candidate
///    (the catch-all for guard-fail cases, e.g. REVISE → blocked).
/// 4. If neither is found, bail with a "guard not satisfied" message.
pub fn select_transition<'a>(
    transitions: &'a [Transition],
    from_state: &str,
    verb: &str,
    gate: Option<&str>,
    entry: &EntryMap,
) -> anyhow::Result<&'a Transition> {
    let candidates: Vec<_> = transitions
        .iter()
        .filter(|t| t.from == from_state && t.verb == verb && t.requires_gate.as_deref() == gate)
        .collect();

    if candidates.is_empty() {
        // The verb may be valid but unreachable from the current state.  Scan
        // the full transition table for the verb (ignoring `from`/`gate`) and
        // surface the reachable from-states so the operator sees the next legal
        // hop rather than a bare "not found in schema".
        let reachable_froms: Vec<&str> = transitions
            .iter()
            .filter(|t| t.verb == verb)
            .map(|t| t.from.as_str())
            .collect();
        let mut dedup: Vec<&str> = Vec::new();
        for f in &reachable_froms {
            if !dedup.contains(f) {
                dedup.push(*f);
            }
        }

        let prefix = match gate {
            Some(g) => format!(
                "no transition from '{}' via verb '{}' with gate '{}' found in schema",
                from_state, verb, g
            ),
            None => format!(
                "no transition from '{}' via verb '{}' found in schema",
                from_state, verb
            ),
        };

        if dedup.is_empty() {
            bail!(
                "{}; verb '{}' is not declared anywhere in this schema",
                prefix,
                verb
            );
        }

        // Find a hop FROM current state TO one of the reachable_froms (one-step lookahead).
        // If exactly one, name it as the suggested next step.
        let next_hops: Vec<&Transition> = transitions
            .iter()
            .filter(|t| t.from == from_state && dedup.contains(&t.to.as_str()))
            .collect();
        let hop_hint = match next_hops.as_slice() {
            [] => String::new(),
            [t] => format!(
                "; current state '{}' transitions there via verb '{}'",
                from_state, t.verb
            ),
            many => {
                let verbs: Vec<&str> = many.iter().map(|t| t.verb.as_str()).collect();
                format!(
                    "; current state '{}' can reach those states via {{{}}}",
                    from_state,
                    verbs.join(", ")
                )
            }
        };

        bail!(
            "{}; verb '{}' is reachable from {{{}}}{}",
            prefix,
            verb,
            dedup.join(", "),
            hop_hint
        );
    }

    // Prefer guarded transitions whose guard evaluates true.
    let mut guarded_true: Vec<_> = candidates
        .iter()
        .filter(|t| t.guard.is_some() && eval(t.guard.as_ref().unwrap(), entry))
        .copied()
        .collect();

    if guarded_true.len() > 1 {
        bail!(
            "ambiguous: multiple transitions from '{}' via '{}' (gate {:?}) have guards that evaluate true — schema guards must partition entry space",
            from_state, verb, gate
        );
    }

    if let Some(t) = guarded_true.pop() {
        return Ok(t);
    }

    // No guarded-true match: use the unguarded fallback (catch-all).
    let unguarded: Vec<_> = candidates
        .iter()
        .filter(|t| t.guard.is_none())
        .copied()
        .collect();
    if let Some(t) = unguarded.first() {
        return Ok(t);
    }

    bail!(
        "no transition from '{}' via '{}' (gate {:?}) had its guard satisfied and no unguarded fallback exists",
        from_state, verb, gate
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_defaults_to_first() {
        let lc: Lifecycle =
            serde_yaml::from_str("states: [triage, active, done]\ntransitions: []").unwrap();
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
        let lc: Lifecycle = serde_yaml::from_str(
            r#"
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
"#,
        )
        .unwrap();
        let revise = lc.transitions.iter().find(|t| t.to == "executing").unwrap();
        assert_eq!(revise.requires_gate.as_deref(), Some("REVISE"));
        let pass = lc.transitions.iter().find(|t| t.to == "complete").unwrap();
        assert_eq!(pass.requires_gate.as_deref(), Some("PASS"));
        // Ambiguity check: all three have requires_gate, no unguarded pair
        assert!(lc.validate_transition_ambiguity().is_ok());
    }

    #[test]
    fn no_requires_gate_defaults_to_none() {
        let lc: Lifecycle = serde_yaml::from_str(
            r#"
states: [open, done]
transitions:
  - from: open
    to: done
    verb: close
"#,
        )
        .unwrap();
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

    // ---- Task 5.2: guard field ----

    #[test]
    fn transition_guard_parses() {
        let lc: Lifecycle = serde_yaml::from_str(
            r#"
states: [code_review, executing, blocked]
transitions:
  - from: code_review
    to: executing
    verb: submit-review
    requires_gate: REVISE
    guard: "current_cycle <= 4"
    actor: ai_autonomous
  - from: code_review
    to: blocked
    verb: submit-review
    requires_gate: REVISE
    actor: ai_autonomous
"#,
        )
        .unwrap();
        let revise_guarded = lc.transitions.iter().find(|t| t.guard.is_some()).unwrap();
        assert!(revise_guarded.guard.is_some(), "guard should parse");
        let unguarded = lc
            .transitions
            .iter()
            .find(|t| t.guard.is_none() && t.to == "blocked")
            .unwrap();
        assert!(unguarded.guard.is_none());
    }

    #[test]
    fn transition_guard_invalid_expression_errors() {
        let result: Result<Lifecycle, _> = serde_yaml::from_str(
            r#"
states: [open, done]
transitions:
  - from: open
    to: done
    verb: close
    guard: "a && b"
"#,
        );
        assert!(
            result.is_err(),
            "invalid guard expression should fail deserialization"
        );
    }

    // ---- "no transition found" wording: surface next-hop hint ----
    // Closes L267-walk feedback item 3: instead of "no transition from 'confirmed'
    // via verb 'resolve' found in schema", the message names where 'resolve' IS
    // reachable and the verb that gets you there from the current state.

    fn obs_lifecycle_with_claim_resolve() -> Lifecycle {
        serde_yaml::from_str(
            r#"
states: [open, confirmed, in_progress, resolved]
transitions:
  - from: open
    to: confirmed
    verb: confirm
  - from: confirmed
    to: in_progress
    verb: claim
    actor: ai_autonomous
  - from: in_progress
    to: resolved
    verb: resolve
    actor: ai_autonomous
"#,
        )
        .unwrap()
    }

    #[test]
    fn select_transition_unreachable_verb_names_reachable_froms() {
        let lc = obs_lifecycle_with_claim_resolve();
        let entry = std::collections::BTreeMap::new();
        let err =
            select_transition(&lc.transitions, "confirmed", "resolve", None, &entry).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("verb 'resolve' is reachable from {in_progress}"),
            "msg: {msg}"
        );
    }

    #[test]
    fn select_transition_unreachable_verb_names_next_hop_when_unique() {
        let lc = obs_lifecycle_with_claim_resolve();
        let entry = std::collections::BTreeMap::new();
        let err =
            select_transition(&lc.transitions, "confirmed", "resolve", None, &entry).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transitions there via verb 'claim'"),
            "msg: {msg}"
        );
    }

    #[test]
    fn select_transition_undeclared_verb_says_so() {
        let lc = obs_lifecycle_with_claim_resolve();
        let entry = std::collections::BTreeMap::new();
        let err = select_transition(&lc.transitions, "open", "teleport", None, &entry).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("verb 'teleport' is not declared anywhere in this schema"),
            "msg: {msg}"
        );
    }

    #[test]
    fn transition_guard_path_length_parses() {
        let lc: Lifecycle = serde_yaml::from_str(
            r#"
states: [code_review, complete, executing]
transitions:
  - from: code_review
    to: complete
    verb: submit-review
    requires_gate: PASS
    guard: "current_phase >= plan.phases.length"
    actor: ai_autonomous
  - from: code_review
    to: executing
    verb: submit-review
    requires_gate: PASS
    guard: "current_phase < plan.phases.length"
    actor: ai_autonomous
"#,
        )
        .unwrap();
        assert_eq!(lc.transitions.len(), 2);
        assert!(lc.transitions.iter().all(|t| t.guard.is_some()));
    }
}
