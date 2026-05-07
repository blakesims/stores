pub mod actor;
pub mod enum_check;
pub mod error;
pub mod expr_eval;
pub mod gatekeeper_decision;
pub mod list_enum;
pub mod regex_check;
pub mod required;

use std::collections::BTreeMap;

use crate::schema::{
    actor::{Actor, InvokerCtx},
    FieldType, Schema,
};

pub use actor::SideEffectAuthority;
pub use error::{pretty_print, ValidationError};

/// In-memory entry map: nested structure mirroring the schema.
/// Leaf values are serde_json::Value scalars; Record values are nested
/// serde_json::Value::Object.
pub type EntryMap = BTreeMap<String, serde_json::Value>;

/// Operation being validated.
#[derive(Debug, Clone)]
pub enum Op {
    Add,
    /// Update op carrying the diff (fields actually being written in this call).
    /// Actor checks scope to the diff only; required/enum/pattern use the merged entry.
    Update(EntryMap),
    /// Lifecycle transition verb (e.g. "triage", "close") carrying the diff.
    /// Actor checks scope to the diff only; required/enum/pattern use the merged entry.
    Transition(String, EntryMap),
    /// Write the plan (record) after planning — verb: submit-plan.
    /// Actor checks scope to diff only (per A15).
    SubmitPlan(EntryMap),
    /// Write a plan-review entry — verb: submit-plan-review.
    /// gate ∈ {READY, NEEDS_WORK, NOT_READY}.
    /// Actor checks scope to diff only.
    SubmitPlanReview(String, EntryMap),
    /// Write an execution entry — verb: submit-execute.
    /// Actor checks scope to diff only.
    SubmitExecute(EntryMap),
    /// Write a code-review entry — verb: submit-review.
    /// gate ∈ {PASS, REVISE, FAIL}.
    /// Actor checks scope to diff only.
    SubmitReview(String, EntryMap),
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
    invoker: InvokerCtx,
) -> Result<(), Vec<ValidationError>> {
    validate_with_authority(schema, entry, op, invoker, None)
}

/// Like `validate`, but accepts a `SideEffectAuthority` for named internal
/// framework code-paths.
///
/// This entry point is used exclusively by `add_row_in_tx_with_authority` for
/// gatekeeper route side-effect observation inserts. Generic `observations
/// add/update` callers go through `validate` (authority=None) and are
/// unaffected by the authority allowlist.
///
/// The authority is threaded into actor checks only; all other validation rules
/// (required, enum, pattern, list_enum) are unchanged.
pub fn validate_with_authority(
    schema: &Schema,
    entry: &EntryMap,
    op: Op,
    invoker: InvokerCtx,
    authority: Option<SideEffectAuthority>,
) -> Result<(), Vec<ValidationError>> {
    let mut errors: Vec<ValidationError> = Vec::new();

    // Determine the transition verb and diff for actor scoping.
    // Actor checks use the diff (what's actually being written this call);
    // required/enum/pattern use the full merged entry.
    let (verb_opt, actor_entry) = match &op {
        Op::Transition(verb, diff) => (Some(verb.as_str()), diff),
        Op::Update(diff) => (None, diff),
        Op::Add => (None, entry),
        // Submit ops: actor checks scoped to diff only (A15), verb used for transition check.
        Op::SubmitPlan(diff) => (Some("submit-plan"), diff),
        Op::SubmitPlanReview(_, diff) => (Some("submit-plan-review"), diff),
        Op::SubmitExecute(diff) => (Some("submit-execute"), diff),
        Op::SubmitReview(_, diff) => (Some("submit-review"), diff),
    };

    // If this is a Transition or Submit op, check the transition's declared actor first.
    if let Some(verb) = verb_opt {
        if let Some(transition) = schema.lifecycle.transitions.iter().find(|t| t.verb == verb) {
            if let Some(transition_actor) = transition.actor {
                actor::check_transition_actor(verb, transition_actor, invoker, &mut errors);
            }
        }
    }

    validate_observations_source_pair(schema, entry, &op, &mut errors);

    // Walk all fields recursively. Required/enum/pattern checks run against the
    // full merged entry; actor checks run against actor_entry (diff-only for
    // Transition/Update, full entry for Add).
    for field in &schema.fields {
        validate_field_recursive(
            field,
            entry,
            actor_entry,
            &[],
            &schema.default_actor,
            invoker,
            authority,
            &mut errors,
        );
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_observations_source_pair(
    schema: &Schema,
    entry: &EntryMap,
    op: &Op,
    errors: &mut Vec<ValidationError>,
) {
    if schema.name != "observations" {
        return;
    }

    let should_check = match op {
        Op::Add => entry.contains_key("source_env") || entry.contains_key("source_id"),
        Op::Update(diff)
        | Op::Transition(_, diff)
        | Op::SubmitPlan(diff)
        | Op::SubmitExecute(diff) => diff.contains_key("source_env") || diff.contains_key("source_id"),
        Op::SubmitPlanReview(_, diff) | Op::SubmitReview(_, diff) => {
            diff.contains_key("source_env") || diff.contains_key("source_id")
        }
    };
    if !should_check {
        return;
    }

    let env_present = entry.get("source_env").is_some_and(|v| !v.is_null());
    let id_present = entry.get("source_id").is_some_and(|v| !v.is_null());
    if env_present != id_present {
        errors.push(ValidationError {
            field_path: vec!["source_env".to_string(), "source_id".to_string()],
            rule: error::RuleKind::Required,
            message: "source_env/source_id must be supplied as a required clean pair; use --source-env with --source-id".to_string(),
        });
    }
}

fn validate_field_recursive(
    field: &crate::schema::Field,
    entry: &EntryMap,
    actor_entry: &EntryMap,
    parent_path: &[String],
    default_actor: &Option<Actor>,
    invoker: InvokerCtx,
    authority: Option<SideEffectAuthority>,
    errors: &mut Vec<ValidationError>,
) {
    validate_field(
        field,
        entry,
        actor_entry,
        parent_path,
        default_actor,
        invoker,
        authority,
        errors,
    );

    let mut field_path = parent_path.to_vec();
    field_path.push(field.name.clone());

    match &field.ty {
        FieldType::Record(sub_fields) => {
            for sub in sub_fields {
                validate_field_recursive(
                    sub,
                    entry,
                    actor_entry,
                    &field_path,
                    default_actor,
                    invoker,
                    authority,
                    errors,
                );
            }
        }
        FieldType::ListRecord(sub_fields) => {
            if let Some(serde_json::Value::Array(elements)) = required::lookup(entry, &field_path) {
                let actor_list_val = required::lookup(actor_entry, &field_path);
                for (elem_idx, elem) in elements.iter().enumerate() {
                    if let serde_json::Value::Object(elem_map) = elem {
                        let elem_entry: EntryMap = elem_map
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        let actor_elem_entry: EntryMap = match actor_list_val {
                            Some(serde_json::Value::Array(actor_elems)) => {
                                if let Some(serde_json::Value::Object(ae)) =
                                    actor_elems.get(elem_idx)
                                {
                                    ae.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                } else {
                                    BTreeMap::new()
                                }
                            }
                            _ => BTreeMap::new(),
                        };
                        let before = errors.len();
                        for sub in sub_fields {
                            validate_field_recursive(
                                sub,
                                &elem_entry,
                                &actor_elem_entry,
                                &[],
                                default_actor,
                                invoker,
                                authority,
                                errors,
                            );
                        }
                        for err in errors[before..].iter_mut() {
                            let mut prefixed = field_path.clone();
                            prefixed.append(&mut err.field_path);
                            err.field_path = prefixed;
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// harden_log bounds enforcement (L077 contract: bounded structured field,
// NOT a transcript dump).
//
// Bounds (option b — narrow harden_log-specific constants):
//   - Each of the 5 list fields: max 20 items.
//   - Each string value within an item: max 500 chars.
//
// These match the mirrors in:
//   - agents/schemas/investigator.schema.json (harden_log_fragment)
//   - src/flow/builtins/investigator.rs (validate_harden_log_fragment)
// ---------------------------------------------------------------------------

const HARDEN_LOG_MAX_ITEMS: usize = 20;
const HARDEN_LOG_MAX_STR_LEN: usize = 500;

/// Called when we've confirmed the value at `field_path` is a non-null object
/// and the path matches `["intent_contract", "harden_log"]`.
fn validate_harden_log_bounds(
    value: &serde_json::Value,
    field_path: &[String],
    errors: &mut Vec<ValidationError>,
) {
    let Some(obj) = value.as_object() else {
        return;
    };

    // Check all 5 list fields for item count and per-string-value length.
    let list_fields = [
        "decisions",
        "scope_cuts",
        "alternatives_rejected",
        "compress_vs_surface",
        "unresolved_questions",
    ];

    for list_key in &list_fields {
        let Some(list_val) = obj.get(*list_key) else {
            continue;
        };
        if list_val.is_null() {
            continue;
        }
        let Some(arr) = list_val.as_array() else {
            continue; // shape error caught by type check
        };

        // max_items check
        if arr.len() > HARDEN_LOG_MAX_ITEMS {
            let mut path = field_path.to_vec();
            path.push(list_key.to_string());
            errors.push(ValidationError {
                field_path: path,
                rule: error::RuleKind::BoundsExceeded {
                    limit: HARDEN_LOG_MAX_ITEMS,
                    actual: arr.len(),
                },
                message: format!(
                    "harden_log.{} exceeds max_items={} (got {})",
                    list_key, HARDEN_LOG_MAX_ITEMS, arr.len()
                ),
            });
            continue; // no need to check strings inside an oversized list
        }

        // max_length check for each string in each item
        for (item_idx, item) in arr.iter().enumerate() {
            match item {
                serde_json::Value::String(s) => {
                    // unresolved_questions is a list of plain strings
                    let len = s.chars().count();
                    if len > HARDEN_LOG_MAX_STR_LEN {
                        let mut path = field_path.to_vec();
                        path.push(list_key.to_string());
                        errors.push(ValidationError {
                            field_path: path,
                            rule: error::RuleKind::BoundsExceeded {
                                limit: HARDEN_LOG_MAX_STR_LEN,
                                actual: len,
                            },
                            message: format!(
                                "harden_log.{}[{}] exceeds max_length={} (got {} chars)",
                                list_key, item_idx, HARDEN_LOG_MAX_STR_LEN, len
                            ),
                        });
                    }
                }
                serde_json::Value::Object(rec) => {
                    for (field_key, field_val) in rec {
                        if let serde_json::Value::String(s) = field_val {
                            let len = s.chars().count();
                            if len > HARDEN_LOG_MAX_STR_LEN {
                                let mut path = field_path.to_vec();
                                path.push(list_key.to_string());
                                errors.push(ValidationError {
                                    field_path: path,
                                    rule: error::RuleKind::BoundsExceeded {
                                        limit: HARDEN_LOG_MAX_STR_LEN,
                                        actual: len,
                                    },
                                    message: format!(
                                        "harden_log.{}[{}].{} exceeds max_length={} (got {} chars)",
                                        list_key, item_idx, field_key, HARDEN_LOG_MAX_STR_LEN, len
                                    ),
                                });
                            }
                        }
                    }
                }
                _ => {} // shape error caught elsewhere
            }
        }
    }
}

fn push_invalid_shape(
    errors: &mut Vec<ValidationError>,
    field_path: &[String],
    expected: &str,
    value: &serde_json::Value,
) {
    let got = match value {
        serde_json::Value::String(raw) => {
            let preview: String = raw.chars().take(60).collect();
            format!("string '{preview}'")
        }
        other => other.to_string(),
    };
    errors.push(ValidationError {
        field_path: field_path.to_vec(),
        rule: error::RuleKind::InvalidJson {
            expected: expected.to_string(),
        },
        message: format!("value must be a {expected}, got {got}"),
    });
}

fn validate_field(
    field: &crate::schema::Field,
    entry: &EntryMap,
    actor_entry: &EntryMap,
    parent_path: &[String],
    default_actor: &Option<Actor>,
    invoker: InvokerCtx,
    authority: Option<SideEffectAuthority>,
    errors: &mut Vec<ValidationError>,
) {
    let mut field_path = parent_path.to_vec();
    field_path.push(field.name.clone());

    // Type-shape checks for structured fields at any depth. `coerce_value` may
    // return Value::String(raw) sentinels for bad JSON / non-array input; those
    // must be rejected even when optional. Null remains valid for nullable slots.
    if let Some(value) = required::lookup(entry, &field_path) {
        match &field.ty {
            FieldType::Record(_) => {
                if !value.is_object() && !value.is_null() {
                    push_invalid_shape(errors, &field_path, "JSON object", value);
                    return;
                }
            }
            FieldType::ListRecord(_) => {
                if !value.is_array() && !value.is_null() {
                    push_invalid_shape(errors, &field_path, "JSON array", value);
                    return;
                }
                if let serde_json::Value::Array(elements) = value {
                    if let Some(bad) = elements.iter().find(|elem| !elem.is_object()) {
                        push_invalid_shape(errors, &field_path, "JSON array of objects", bad);
                        return;
                    }
                }
            }
            FieldType::List(inner) => {
                if !value.is_array() && !value.is_null() {
                    push_invalid_shape(errors, &field_path, "JSON array", value);
                    return;
                }
                if matches!(inner.as_ref(), FieldType::Text) {
                    if let serde_json::Value::Array(elements) = value {
                        if let Some(bad) = elements.iter().find(|elem| !elem.is_string()) {
                            push_invalid_shape(errors, &field_path, "JSON array of strings", bad);
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // harden_log bounds check — fires when the field path is exactly
    // ["intent_contract", "harden_log"] and the value is a non-null object.
    if field_path == ["intent_contract", "harden_log"] {
        if let Some(value) = required::lookup(entry, &field_path) {
            if !value.is_null() {
                validate_harden_log_bounds(value, &field_path, errors);
            }
        }
    }

    // T008 P3: type-shape check for Json fields.
    // coerce_value returns Value::String(raw) sentinel on parse failure (Phase 2).
    // Re-parse to distinguish (b) true sentinel (parse error) from (c) top-level JSON string
    // (valid JSON whose top-level is a string, e.g. --notes '"hello"').
    // Case (c) is treated as valid per Decision 2's documented limitation.
    if matches!(&field.ty, FieldType::Json) {
        if let Some(serde_json::Value::String(raw)) = required::lookup(entry, &field_path) {
            if serde_json::from_str::<serde_json::Value>(raw).is_err() {
                // Sentinel detected: bad JSON was written via coerce_value.
                errors.push(ValidationError {
                    field_path: field_path.clone(),
                    rule: error::RuleKind::InvalidJson {
                        expected: "valid JSON".to_string(),
                    },
                    message: format!(
                        "value must be valid JSON, got string '{}'",
                        if raw.len() > 60 {
                            &raw[..60]
                        } else {
                            raw.as_str()
                        }
                    ),
                });
                // Short-circuit: InvalidJson is the only relevant diagnostic for a sentinel.
                return;
            }
            // else: top-level JSON string (case c) — treated as valid; no error.
        }
    }

    // required / required_when
    required::check_required(field, &field_path, entry, errors);

    // enum check
    enum_check::check_enum(field, &field_path, entry, errors);

    // list_enum membership check (T052 P2)
    list_enum::check_list_enum(field, &field_path, entry, errors);

    // pattern / regex check
    regex_check::check_pattern(field, &field_path, entry, errors);

    // actor check — uses actor_entry (diff only for Transition/Update, full entry for Add)
    actor::check_actor_with_authority(
        field,
        &field_path,
        actor_entry,
        invoker,
        *default_actor,
        authority,
        errors,
    );
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
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
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
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.field_path == vec!["summary".to_string()]));
    }

    #[test]
    fn required_field_present_passes() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello"))]);
        validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap();
    }

    // ---- enum rule ----

    #[test]
    fn invalid_enum_value_rejected() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("priority", str_val("critical")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.field_path == vec!["priority".to_string()] && e.rule == error::RuleKind::Enum
        }));
    }

    #[test]
    fn valid_enum_value_passes() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello")), ("priority", str_val("high"))]);
        validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap();
    }



    #[test]
    fn t084_observations_source_env_without_source_id_rejected() {
        let schema = Schema::from_yaml(r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: source_env
    type: enum
    enum_values: [prod, sandbox]
  - name: source_id
    type: text
"#).unwrap();
        let entry = entry_from(&[("source_env", str_val("prod"))]);
        let errs = validate(&schema, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        let msg = pretty_print(&errs);
        assert!(msg.contains("source_env/source_id"), "{msg}");
        assert!(msg.contains("clean pair"), "{msg}");
    }

    #[test]
    fn t084_observations_source_id_without_source_env_rejected() {
        let schema = Schema::from_yaml(r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: source_env
    type: enum
    enum_values: [prod, sandbox]
  - name: source_id
    type: text
"#).unwrap();
        let entry = entry_from(&[("source_id", str_val("P123"))]);
        let errs = validate(&schema, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        let msg = pretty_print(&errs);
        assert!(msg.contains("source_env/source_id"), "{msg}");
        assert!(msg.contains("clean pair"), "{msg}");
    }

    // ---- pattern rule ----

    #[test]
    fn pattern_mismatch_rejected() {
        let s = schema();
        let entry = entry_from(&[
            ("summary", str_val("hello")),
            ("slug", str_val("Bad Slug!")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
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
        validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap();
    }

    // ---- actor rule ----

    #[test]
    fn ai_invoker_human_field_fails() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello")), ("answer", str_val("yes"))]);
        let errs = validate(&s, &entry, Op::Add, Actor::AiAutonomous.into()).unwrap_err();
        assert!(errs.iter().any(|e| {
            e.field_path == vec!["answer".to_string()]
                && e.rule == error::RuleKind::Actor
                && e.message.contains("$CLAUDECODE")
        }));
    }

    #[test]
    fn human_invoker_human_field_passes() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello")), ("answer", str_val("yes"))]);
        validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap();
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
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();

        let paths: Vec<String> = errs.iter().map(|e| e.field_path.join(".")).collect();
        assert!(
            paths.contains(&"contract.done_when".to_string()),
            "expected contract.done_when in errors; got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"contract.scope_in".to_string()),
            "expected contract.scope_in in errors; got: {:?}",
            paths
        );
        assert!(
            paths.contains(&"contract.scope_out".to_string()),
            "expected contract.scope_out in errors; got: {:?}",
            paths
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
        validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap();
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
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        // Must have at least 5 errors: summary required, priority enum, slug pattern,
        // done_when required_when, scope_in required_when, scope_out required_when
        assert!(
            errs.len() >= 5,
            "expected ≥5 errors but got {}: {:?}",
            errs.len(),
            errs.iter()
                .map(|e| e.field_path.join("."))
                .collect::<Vec<_>>()
        );
    }

    // ---- transition actor check ----

    #[test]
    fn transition_actor_ai_autonomous_rejected_for_ai_with_human() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello"))]);
        // "triage" transition requires ai_with_human; bare ai_autonomous must be rejected
        let diff = entry_from(&[]);
        let errs = validate(
            &s,
            &entry,
            Op::Transition("triage".to_string(), diff),
            Actor::AiAutonomous.into(),
        )
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.rule == error::RuleKind::Actor && e.message.contains("ai_with_human")),
            "expected actor error citing ai_with_human; got: {:?}",
            errs
        );
    }

    #[test]
    fn transition_actor_human_accepted_for_ai_with_human() {
        let s = schema();
        let entry = entry_from(&[("summary", str_val("hello"))]);
        let diff = entry_from(&[]);
        validate(
            &s,
            &entry,
            Op::Transition("triage".to_string(), diff),
            Actor::Human.into(),
        )
        .unwrap();
    }

    // ---- Op::Update actor scoping — regression test for carry-forward fix ----

    #[test]
    fn update_with_human_invoker_on_ai_authored_row_succeeds() {
        // Scenario: an ai_autonomous Add wrote the full row (including the human-actor `answer`
        // field as null — absent in the diff). A human then tries to update a different field
        // (`summary`) only. The validator must NOT fire the actor check on `answer` because
        // `answer` is not in the diff for this Update call.
        let s = schema();

        // Simulate the merged row as it would look after an AI-authored add:
        // summary present (required), answer absent (null-shaped, not in the human's diff).
        let merged = entry_from(&[
            ("summary", str_val("original summary")),
            ("answer", serde_json::Value::Null), // AI-authored row has answer=null
        ]);

        // The human's diff only changes summary.
        let diff = entry_from(&[("summary", str_val("updated summary"))]);

        // Should succeed: human is only mutating `summary` (no actor constraint).
        // `answer` is in the merged entry (as Null) but NOT in the diff → must not error.
        validate(&s, &merged, Op::Update(diff), Actor::Human.into())
            .expect("update scoped to summary should succeed even though merged row has answer=null written by AI");
    }

    #[test]
    fn update_with_ai_invoker_writing_human_field_fails() {
        // Sanity: AI trying to update the human-actor `answer` field directly must still fail.
        let s = schema();
        let merged = entry_from(&[("summary", str_val("hello"))]);
        // The AI's diff includes `answer` — this should be caught.
        let diff = entry_from(&[("answer", str_val("ai-wrote-this"))]);
        let errs = validate(&s, &merged, Op::Update(diff), Actor::AiAutonomous.into()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.field_path == vec!["answer".to_string()]
                    && e.rule == error::RuleKind::Actor),
            "expected actor error on answer for AI invoker; got: {:?}",
            errs
        );
    }

    // ---- pretty_print determinism ----

    #[test]
    fn pretty_print_sorted_by_field_path() {
        let s = schema();
        let entry = entry_from(&[("triage", nested_val(&[("verdict", "T3")]))]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
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

    // ---- P1-M2 closed in Phase 5: ListRecord sub-fields ARE now validated ----

    /// Phase 5 added the ListRecord walker.  A `required: true` field inside a
    /// `list_record` element now triggers a validation error when the element
    /// is present but the sub-field is missing.
    ///
    /// (This test was pinned as `list_record_required_sub_field_not_validated_phase1`
    /// with `unwrap()` expectation during Phase 1.  Phase 5 inverts it to `unwrap_err()`.)
    #[test]
    fn list_record_required_sub_field_validated_phase5() {
        const LR_SCHEMA: &str = r#"
name: items
id_format: "X{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: entries
    type: list_record
    fields:
      - name: note
        type: text
        required: true
"#;
        let s = Schema::from_yaml(LR_SCHEMA).unwrap();
        // Entry has `title` (required top-level) but `entries` list element
        // is missing its required `note` field.  In Phase 1 this PASSES because
        // the validator does not walk into ListRecord elements.
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("entries", serde_json::json!([{}])), // element missing required `note`
        ]);
        // Phase 5: error expected (ListRecord sub-fields are now walked).
        validate(&s, &entry, Op::Add, Actor::Human.into())
            .expect_err("Phase 5: ListRecord required sub-field must produce validation error");
    }

    #[test]
    fn list_record_required_sub_field_present_passes() {
        const LR_SCHEMA: &str = r#"
name: items
id_format: "X{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: entries
    type: list_record
    fields:
      - name: note
        type: text
        required: true
"#;
        let s = Schema::from_yaml(LR_SCHEMA).unwrap();
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("entries", serde_json::json!([{"note": "present"}])), // required field present
        ]);
        validate(&s, &entry, Op::Add, Actor::Human.into())
            .expect("list_record element with required sub-field present should pass");
    }

    #[test]
    fn list_record_empty_list_passes() {
        const LR_SCHEMA: &str = r#"
name: items
id_format: "X{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: entries
    type: list_record
    fields:
      - name: note
        type: text
        required: true
"#;
        let s = Schema::from_yaml(LR_SCHEMA).unwrap();
        // Empty list — no elements to validate, so no required errors
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("entries", serde_json::json!([])),
        ]);
        validate(&s, &entry, Op::Add, Actor::Human.into())
            .expect("empty list_record should pass even with required sub-fields");
    }

    // ---- Phase 3 (T008): Json sentinel detection ----

    const JSON_SCHEMA_REQUIRED: &str = r#"
name: notes_store
id_format: "N{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: notes
    type: json
    required: true
"#;

    const JSON_SCHEMA_OPTIONAL: &str = r#"
name: notes_store
id_format: "N{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: notes
    type: json
    required: false
"#;

    /// Required Json field with bad JSON (sentinel) → exactly one `InvalidJson` error on `notes`.
    /// Short-circuit: `Required` must NOT also fire (Decision 5).
    #[test]
    fn validate_json_required_field_bad_value_emits_invalid_json() {
        let s = Schema::from_yaml(JSON_SCHEMA_REQUIRED).unwrap();
        // Simulate sentinel written by coerce_value on bad JSON.
        let entry = entry_from(&[("title", str_val("hello")), ("notes", str_val("{not json"))]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();

        let notes_errs: Vec<&ValidationError> = errs
            .iter()
            .filter(|e| e.field_path == vec!["notes".to_string()])
            .collect();

        // Exactly one error for `notes` (short-circuit: no Required also firing).
        assert_eq!(
            notes_errs.len(),
            1,
            "expected exactly 1 error for notes; got: {:?}",
            notes_errs.iter().map(|e| &e.rule).collect::<Vec<_>>()
        );

        // Rule is InvalidJson { expected: "valid JSON" }.
        assert!(
            matches!(&notes_errs[0].rule, error::RuleKind::InvalidJson { expected } if expected == "valid JSON"),
            "expected InvalidJson{{valid JSON}}; got: {:?}",
            notes_errs[0].rule
        );

        // Message contains field-relevant phrase.
        assert!(
            notes_errs[0].message.contains("valid JSON"),
            "message should contain 'valid JSON'; got: {}",
            notes_errs[0].message
        );
    }

    /// Optional Json field with bad JSON (sentinel) → `InvalidJson` fires even though
    /// the field is not required.  Regression-trap: optional sentinel must NOT silently pass.
    #[test]
    fn validate_json_optional_field_bad_value_still_emits_invalid_json() {
        let s = Schema::from_yaml(JSON_SCHEMA_OPTIONAL).unwrap();
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("notes", str_val("{bad json here")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();

        let notes_errs: Vec<&ValidationError> = errs
            .iter()
            .filter(|e| e.field_path == vec!["notes".to_string()])
            .collect();

        assert_eq!(
            notes_errs.len(),
            1,
            "optional field: expected exactly 1 InvalidJson error; got: {:?}",
            notes_errs
        );
        assert!(
            matches!(&notes_errs[0].rule, error::RuleKind::InvalidJson { expected } if expected == "valid JSON"),
            "optional field: expected InvalidJson{{valid JSON}}; got: {:?}",
            notes_errs[0].rule
        );
        assert!(
            notes_errs[0].message.contains("valid JSON"),
            "message should contain 'valid JSON'; got: {}",
            notes_errs[0].message
        );
    }

    /// Top-level JSON string `'"hello"'` — coerce_value returns `Value::String("hello")`.
    /// Re-parse of `"hello"` (without quotes) fails → this IS flagged as sentinel.
    /// Decision 2 documented limitation: top-level JSON strings false-flag.
    /// This test pins the *known* behaviour (limitation is accepted).
    #[test]
    fn validate_json_top_level_string_is_treated_as_sentinel_known_limitation() {
        let s = Schema::from_yaml(JSON_SCHEMA_OPTIONAL).unwrap();
        // coerce_value parses '"hello"' → Value::String("hello").
        // The validator sees Value::String("hello"), re-parses "hello" (no quotes) → Err → sentinel.
        // Decision 2: this false-flags; documented limitation, accepted for v0.5.
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("notes", str_val("hello")), // inner content after coerce_value strips quotes
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();
        let notes_errs: Vec<&ValidationError> = errs
            .iter()
            .filter(|e| e.field_path == vec!["notes".to_string()])
            .collect();
        // Confirm the false-flag behaviour is pinned (InvalidJson fires for top-level string).
        assert_eq!(
            notes_errs.len(),
            1,
            "known limitation: top-level JSON string triggers sentinel; got: {:?}",
            notes_errs
        );
        assert!(
            matches!(&notes_errs[0].rule, error::RuleKind::InvalidJson { .. }),
            "known limitation: InvalidJson expected; got: {:?}",
            notes_errs[0].rule
        );
    }

    /// Backwards-compat: the T006 P2 list_record/list_fk InvalidJsonArray path now emits
    /// `InvalidJson { expected: "JSON array" }` and the user-facing message is unchanged.
    #[test]
    fn validate_json_existing_list_record_message_unchanged() {
        const LR_SCHEMA: &str = r#"
name: items
id_format: "X{:03d}"
default_actor: ~
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: entries
    type: list_record
    fields:
      - name: note
        type: text
        required: true
"#;
        let s = Schema::from_yaml(LR_SCHEMA).unwrap();
        // Sentinel: bad JSON string stored by coerce_value for a list_record field.
        let entry = entry_from(&[
            ("title", str_val("hello")),
            ("entries", str_val("{not an array")),
        ]);
        let errs = validate(&s, &entry, Op::Add, Actor::Human.into()).unwrap_err();

        let entries_errs: Vec<&ValidationError> = errs
            .iter()
            .filter(|e| e.field_path == vec!["entries".to_string()])
            .collect();

        assert_eq!(
            entries_errs.len(),
            1,
            "expected 1 error for entries; got: {:?}",
            entries_errs
        );

        // Rule shape: InvalidJson { expected: "JSON array" } (renamed from InvalidJsonArray).
        assert!(
            matches!(&entries_errs[0].rule, error::RuleKind::InvalidJson { expected } if expected == "JSON array"),
            "expected InvalidJson{{JSON array}}; got: {:?}",
            entries_errs[0].rule
        );

        // User-facing message must read "value must be a JSON array, got string '...'" (T006 P2 compat).
        assert!(
            entries_errs[0]
                .message
                .starts_with("value must be a JSON array, got string '"),
            "message backwards-compat check failed; got: {}",
            entries_errs[0].message
        );
    }
}

// ---- T001 P2 / AC2.4: InvokerCtx shape tests ----
//
// Top-level `validate::invoker_ctx` test module so `cargo test
// validate::invoker_ctx` matches it literally (per AC2.4). These pin the
// InvokerCtx contract that Phase 3 will consume to relax `actor: human`
// checks for `ai_with_human` writes carrying a valid approve-token.
#[cfg(test)]
mod invoker_ctx {
    use crate::schema::actor::{Actor, InvokerCtx};

    /// `InvokerCtx::bare(actor)` defaults `token_valid` to `false`.
    #[test]
    fn bare_defaults_token_valid_to_false() {
        let ctx = InvokerCtx::bare(Actor::Human);
        assert_eq!(ctx.actor, Actor::Human);
        assert!(!ctx.token_valid);
    }

    /// `From<Actor>` round-trip: the coercion preserves the actor and
    /// sets `token_valid = false`. This is the path most handler callsites
    /// use when they don't (yet) plumb the token.
    #[test]
    fn from_actor_roundtrip_preserves_actor_and_clears_token() {
        for actor in [
            Actor::Human,
            Actor::AiAutonomous,
            Actor::AiWithHuman,
            Actor::Framework,
        ] {
            let ctx: InvokerCtx = actor.into();
            assert_eq!(ctx.actor, actor);
            assert!(!ctx.token_valid, "From<Actor> must set token_valid=false");
        }
    }

    /// Explicit construction with `token_valid = true` is preserved
    /// (no implicit downgrade). Phase 3 relies on this bit being honest.
    #[test]
    fn explicit_token_valid_is_preserved() {
        let ctx = InvokerCtx {
            actor: Actor::AiWithHuman,
            token_valid: true,
        };
        assert_eq!(ctx.actor, Actor::AiWithHuman);
        assert!(ctx.token_valid);
    }

    /// Display matches the underlying actor (audit/log strings stay stable).
    #[test]
    fn display_matches_underlying_actor() {
        assert_eq!(InvokerCtx::bare(Actor::Human).to_string(), "human");
        assert_eq!(
            InvokerCtx::bare(Actor::AiAutonomous).to_string(),
            "ai_autonomous"
        );
        assert_eq!(
            InvokerCtx {
                actor: Actor::AiWithHuman,
                token_valid: true
            }
            .to_string(),
            "ai_with_human"
        );
    }
}
