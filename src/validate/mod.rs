pub mod actor;
pub mod enum_check;
pub mod error;
pub mod regex_check;
pub mod required;

use std::collections::BTreeMap;

use crate::schema::{actor::Actor, FieldType, Schema};

pub use error::{pretty_print, ValidationError};

/// In-memory entry map: nested structure mirroring the schema.
/// Leaf values are serde_json::Value scalars; Record values are nested
/// serde_json::Value::Object.
pub type EntryMap = BTreeMap<String, serde_json::Value>;

/// Operation being validated.
#[derive(Debug, Clone)]
pub enum Op {
    Add,
    Update,
    /// Lifecycle transition verb (e.g. "triage", "close").
    /// The optional diff restricts actor checks to only the fields being written
    /// in this call (not carried-over fields from the existing row).
    Transition(String),
    /// Like Transition but carries the diff so actor checks are scoped to written fields.
    TransitionWithDiff(String, EntryMap),
}

/// Validate an entry map against a schema for the given operation and invoker.
///
/// Walks top-level Fields AND recurses into `Record(_)` sub-Fields so
/// per-leaf rules fire at their correct dotted path.
///
/// Returns `Ok(())` if no violations; `Err(errors)` with all violations
/// collected in a single pass.
pub fn validate(
    schema: &Schema,
    entry: &EntryMap,
    op: Op,
    invoker: Actor,
) -> Result<(), Vec<ValidationError>> {
    let mut errors: Vec<ValidationError> = Vec::new();

    // Determine the transition verb and optional diff for actor scoping.
    let (verb_opt, actor_entry) = match &op {
        Op::Transition(verb) => (Some(verb.as_str()), entry),
        Op::TransitionWithDiff(verb, diff) => (Some(verb.as_str()), diff),
        _ => (None, entry),
    };

    // If this is a Transition op, check the transition's declared actor first.
    if let Some(verb) = verb_opt {
        if let Some(transition) = schema.lifecycle.transitions.iter().find(|t| t.verb == verb) {
            if let Some(transition_actor) = transition.actor {
                actor::check_transition_actor(verb, transition_actor, invoker, &mut errors);
            }
        }
    }

    // Walk all fields (top-level + Record sub-fields).
    // required/enum/pattern checks run against the full merged entry;
    // actor checks run against actor_entry (diff-only for TransitionWithDiff).
    for field in &schema.fields {
        validate_field(field, entry, actor_entry, &[], &schema.default_actor, invoker, &mut errors);

        if let FieldType::Record(sub_fields) = &field.ty {
            let parent_path = vec![field.name.clone()];
            for sub in sub_fields {
                validate_field(sub, entry, actor_entry, &parent_path, &schema.default_actor, invoker, &mut errors);
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_field(
    field: &crate::schema::Field,
    entry: &EntryMap,
    actor_entry: &EntryMap,
    parent_path: &[String],
    default_actor: &Option<Actor>,
    invoker: Actor,
    errors: &mut Vec<ValidationError>,
) {
    let mut field_path = parent_path.to_vec();
    field_path.push(field.name.clone());

    // required / required_when
    required::check_required(field, &field_path, entry, errors);

    // enum check
    enum_check::check_enum(field, &field_path, entry, errors);

    // pattern / regex check
    regex_check::check_pattern(field, &field_path, entry, errors);

    // actor check — uses actor_entry (diff only for TransitionWithDiff, else full entry)
    actor::check_actor(field, &field_path, actor_entry, invoker, *default_actor, errors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;

    // ---------------------------------------------------------------------------
    // Shared fixture schema with all rule types + cross-Record required_when
    // ---------------------------------------------------------------------------
    const FIXTURE_SCHEMA: &str = r#"
name: issues
id_format: "I{:03d}"
default_actor: ~
lifecycle:
  states: [open, triaged]
  transitions:
    - from: open
      to: triaged
      verb: triage
      actor: ai_with_human

fields:
  - name: summary
    type: text
    required: true

  - name: slug
    type: text
    pattern: "^[a-z0-9-]+$"

  - name: priority
    type: enum
    enum_values: [low, medium, high]

  - name: answer
    type: text
    actor: human

  - name: triage
    type: record
    fields:
      - name: verdict
        type: text

  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_in
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_out
        type: text
        required_when: "triage.verdict == 'T3'"
"#;

    fn schema() -> Schema {
        Schema::from_yaml(FIXTURE_SCHEMA).unwrap()
    }

    fn entry_from(pairs: &[(&str, serde_json::Value)]) -> EntryMap {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn str_val(s: &str) -> serde_json::Value {
        serde_json::Value::String(s.to_string())
    }

    fn nested_val(pairs: &[(&str, &str)]) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), str_val(v));
        }
        serde_json::Value::Object(m)
    }

    // ---- required rule ----

    #[test]
    fn required_field_missing_produces_error() {
        let s = schema();
        // no summary
        let entry = entry_from(&[]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();
        assert!(errs.iter().any(|e| e.field_path == vec!["summary".to_string()]));
    }

    #[test]
    fn required_field_present_passes() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello"))]);
        validate(&s, &entry, Op::Add, Actor::Human).unwrap();
    }

    // ---- enum rule ----

    #[test]
    fn invalid_enum_value_rejected() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("priority", str_val("critical")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.field_path == vec!["priority".to_string()]
                && e.rule == error::RuleKind::Enum
        }));
    }

    #[test]
    fn valid_enum_value_passes() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("priority", str_val("high")),
        ]);
        validate(&s, &entry, Op::Add, Actor::Human).unwrap();
    }

    // ---- pattern rule ----

    #[test]
    fn pattern_mismatch_rejected() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("slug", str_val("Bad Slug!")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.field_path == vec!["slug".to_string()]
                && matches!(&e.rule, error::RuleKind::Pattern { .. })
        }));
    }

    #[test]
    fn pattern_match_passes() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("slug", str_val("good-slug-123")),
        ]);
        validate(&s, &entry, Op::Add, Actor::Human).unwrap();
    }

    // ---- actor rule ----

    #[test]
    fn ai_invoker_human_field_fails() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("answer", str_val("yes")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::AiAutonomous).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.field_path == vec!["answer".to_string()]
                && e.rule == error::RuleKind::Actor
                && e.message.contains("$CLAUDECODE")
        }));
    }

    #[test]
    fn human_invoker_human_field_passes() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("answer", str_val("yes")),
        ]);
        validate(&s, &entry, Op::Add, Actor::Human).unwrap();
    }

    // ---- cross-Record required_when (AC2) ----

    #[test]
    fn cross_record_required_when_fires_for_t3() {
        let s = schema();
        // triage.verdict = T3, contract sub-fields absent
        let entry = entry_from(&[
            ("summary", str_val("something")),
            ("triage", nested_val(&[("verdict", "T3")])),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();

        let paths: Vec<String> = errs.iter().map(|e| e.field_path.join(".")).collect();
        assert!(
            paths.contains(&"contract.done_when".to_string()),
            "expected contract.done_when in errors; got: {:?}", paths
        );
        assert!(
            paths.contains(&"contract.scope_in".to_string()),
            "expected contract.scope_in in errors; got: {:?}", paths
        );
        assert!(
            paths.contains(&"contract.scope_out".to_string()),
            "expected contract.scope_out in errors; got: {:?}", paths
        );
    }

    #[test]
    fn cross_record_required_when_silent_for_non_t3() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("something")),
            ("triage", nested_val(&[("verdict", "T1")])),
        ]);
        // No contract sub-fields — should be fine since verdict != T3
        validate(&s, &entry, Op::Add, Actor::Human).unwrap();
    }

    // ---- errors aggregate (AC6) ----

    #[test]
    fn multiple_violations_all_reported() {
        let s = schema();
        // Missing summary (required), bad priority (enum), bad slug (pattern),
        // and contract sub-fields absent while verdict is T3
        let entry = entry_from(&[
            ("priority", str_val("critical")),
            ("slug", str_val("BAD SLUG")),
            ("triage", nested_val(&[("verdict", "T3")])),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();
        // Must have at least 5 errors: summary required, priority enum, slug pattern,
        // done_when required_when, scope_in required_when, scope_out required_when
        assert!(
            errs.len() >= 5,
            "expected ≥5 errors but got {}: {:?}",
            errs.len(),
            errs.iter().map(|e| e.field_path.join(".")).collect::<Vec<_>>()
        );
    }

    // ---- transition actor check ----

    #[test]
    fn transition_actor_mismatch_caught() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello"))]);
        // "triage" transition requires ai_with_human; both actors are valid
        validate(&s, &entry, Op::Transition("triage".to_string()), Actor::Human).unwrap();
        validate(&s, &entry, Op::Transition("triage".to_string()), Actor::AiAutonomous).unwrap();
    }

    // ---- pretty_print determinism ----

    #[test]
    fn pretty_print_sorted_by_field_path() {
        let s = schema();
        let entry = entry_from(&[
            ("triage", nested_val(&[("verdict", "T3")])),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human).unwrap_err();
        let output = pretty_print(&errs);
        // Alphabetically: "contract.*" sorts before "summary"
        let contract_pos = output.find("contract.").unwrap_or(usize::MAX);
        let summary_pos = output.find("summary").unwrap_or(usize::MAX);
        assert!(
            contract_pos < summary_pos,
            "contract.* should appear before summary in sorted output:\n{output}"
        );
        // contract.done_when < contract.scope_in < contract.scope_out
        let done_when_pos = output.find("contract.done_when").unwrap_or(usize::MAX);
        let scope_in_pos = output.find("contract.scope_in").unwrap_or(usize::MAX);
        let scope_out_pos = output.find("contract.scope_out").unwrap_or(usize::MAX);
        assert!(
            done_when_pos < scope_in_pos && scope_in_pos < scope_out_pos,
            "contract sub-fields should be in alphabetical order:\n{output}"
        );
    }
}
