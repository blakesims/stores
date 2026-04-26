use crate::schema::{actor::Actor, Field};
use crate::validate::{EntryMap, ValidationError};
use crate::validate::error::RuleKind;
use crate::validate::required::lookup;

/// Determine the effective actor for a field, falling back to the store default.
fn effective_actor(field: &Field, default_actor: Option<Actor>) -> Option<Actor> {
    field.actor.or(default_actor)
}

/// Check that the invoker is permitted to write a field that is PRESENT in the entry.
///
/// Only fires when the field has a value in the entry map (so absent optional
/// fields never produce an actor error).
pub fn check_actor(
    field: &Field,
    field_path: &[String],
    entry: &EntryMap,
    invoker: Actor,
    default_actor: Option<Actor>,
    errors: &mut Vec<ValidationError>,
) {
    // Only check fields that are actually being written
    if lookup(entry, field_path).is_none() {
        return;
    }

    let required_actor = match effective_actor(field, default_actor) {
        Some(a) => a,
        None => return, // no actor constraint
    };

    if !actor_allowed(invoker, required_actor) {
        let invoker_detail = invoker_detail_str(invoker);
        errors.push(ValidationError {
            field_path: field_path.to_vec(),
            rule: RuleKind::Actor,
            message: format!(
                "field '{}' requires actor '{}'; invoker is '{}'{}",
                field_path.join("."),
                required_actor,
                invoker,
                invoker_detail,
            ),
        });
    }
}

/// Check that the invoker is permitted to invoke a transition verb.
pub fn check_transition_actor(
    verb: &str,
    transition_actor: Actor,
    invoker: Actor,
    errors: &mut Vec<ValidationError>,
) {
    if !actor_allowed(invoker, transition_actor) {
        let invoker_detail = invoker_detail_str(invoker);
        errors.push(ValidationError {
            field_path: vec![format!("<transition:{}>", verb)],
            rule: RuleKind::Actor,
            message: format!(
                "transition '{}' requires actor '{}'; invoker is '{}'{}",
                verb,
                transition_actor,
                invoker,
                invoker_detail,
            ),
        });
    }
}

/// Returns true when `invoker` satisfies `required`.
///
/// Rule: `ai_with_human` means both actors are acceptable.  If the required
/// actor is `human`, only `human` is acceptable.  If required is
/// `ai_autonomous`, only `ai_autonomous` is acceptable.  `ai_with_human`
/// required means either `human` or `ai_autonomous` may write.
fn actor_allowed(invoker: Actor, required: Actor) -> bool {
    match required {
        Actor::Human => invoker == Actor::Human,
        Actor::AiAutonomous => invoker == Actor::AiAutonomous,
        Actor::AiWithHuman => {
            invoker == Actor::Human || invoker == Actor::AiAutonomous
        }
    }
}

/// Build the suffix for the error message that explains how the invoker was detected.
fn invoker_detail_str(invoker: Actor) -> String {
    match invoker {
        Actor::AiAutonomous => {
            " (auto-detected from $CLAUDECODE; pass --invoker human to override if appropriate)"
                .to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, FieldType};
    use std::collections::BTreeMap;

    fn text_field_with_actor(name: &str, actor: Option<Actor>) -> Field {
        Field {
            name: name.to_string(),
            ty: FieldType::Text,
            required: false,
            required_when: None,
            pattern: None,
            actor,
            enum_values: None,
            description: None,
        }
    }

    fn entry_with(key: &str, val: &str) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert(key.to_string(), serde_json::Value::String(val.to_string()));
        m
    }

    // ---- check_actor tests ----

    #[test]
    fn human_invoker_human_field_passes() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(&field, &["answer".to_string()], &entry, Actor::Human, None, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn ai_invoker_human_field_fails_with_claudecode_hint() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(&field, &["answer".to_string()], &entry, Actor::AiAutonomous, None, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rule, RuleKind::Actor);
        let msg = &errors[0].message;
        assert!(msg.contains("field 'answer'"), "msg: {msg}");
        assert!(msg.contains("requires actor 'human'"), "msg: {msg}");
        assert!(msg.contains("ai_autonomous"), "msg: {msg}");
        assert!(msg.contains("$CLAUDECODE"), "msg: {msg}");
        assert!(msg.contains("--invoker human"), "msg: {msg}");
    }

    #[test]
    fn absent_field_no_actor_error() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut errors = vec![];
        check_actor(&field, &["answer".to_string()], &entry, Actor::AiAutonomous, None, &mut errors);
        assert!(errors.is_empty(), "absent fields must not trigger actor errors");
    }

    #[test]
    fn field_without_actor_constraint_passes_any_invoker() {
        let field = text_field_with_actor("title", None);
        let entry = entry_with("title", "foo");
        let mut errors = vec![];
        check_actor(&field, &["title".to_string()], &entry, Actor::AiAutonomous, None, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn default_actor_applied_when_field_has_none() {
        // Field has no actor, but store default is human
        let field = text_field_with_actor("notes", None);
        let entry = entry_with("notes", "some text");
        let mut errors = vec![];
        check_actor(
            &field,
            &["notes".to_string()],
            &entry,
            Actor::AiAutonomous,
            Some(Actor::Human),
            &mut errors,
        );
        assert_eq!(errors.len(), 1, "store-default actor must apply");
        assert!(errors[0].message.contains("requires actor 'human'"));
    }

    #[test]
    fn ai_with_human_field_allows_both_actors() {
        let field = text_field_with_actor("notes", Some(Actor::AiWithHuman));
        let entry = entry_with("notes", "text");
        let mut errors = vec![];
        check_actor(&field, &["notes".to_string()], &entry, Actor::Human, None, &mut errors);
        assert!(errors.is_empty(), "human should be allowed for ai_with_human");
        let mut errors2 = vec![];
        check_actor(&field, &["notes".to_string()], &entry, Actor::AiAutonomous, None, &mut errors2);
        assert!(errors2.is_empty(), "ai_autonomous should be allowed for ai_with_human");
    }

    // ---- check_transition_actor tests ----

    #[test]
    fn transition_actor_mismatch_fires() {
        let mut errors = vec![];
        check_transition_actor("answer", Actor::Human, Actor::AiAutonomous, &mut errors);
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("transition 'answer'"), "msg: {msg}");
        assert!(msg.contains("requires actor 'human'"), "msg: {msg}");
        assert!(msg.contains("$CLAUDECODE"), "msg: {msg}");
    }

    #[test]
    fn transition_actor_match_passes() {
        let mut errors = vec![];
        check_transition_actor("close", Actor::AiAutonomous, Actor::AiAutonomous, &mut errors);
        assert!(errors.is_empty());
    }
}
