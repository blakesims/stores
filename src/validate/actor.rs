use crate::schema::{
    actor::{Actor, InvokerCtx},
    Field,
};
use crate::validate::error::RuleKind;
use crate::validate::required::lookup;
use crate::validate::{EntryMap, ValidationError};

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
    invoker: InvokerCtx,
    default_actor: Option<Actor>,
    errors: &mut Vec<ValidationError>,
) {
    // Only check fields that are actually being written (treat Null as absent)
    match lookup(entry, field_path) {
        None | Some(serde_json::Value::Null) => return,
        _ => {}
    }

    let required_actor = match effective_actor(field, default_actor) {
        Some(a) => a,
        None => return, // no actor constraint
    };

    if !actor_allowed(invoker.actor, required_actor, invoker.token_valid) {
        let remedy = invoker_remedy(invoker.actor, required_actor);
        errors.push(ValidationError {
            field_path: field_path.to_vec(),
            rule: RuleKind::Actor,
            message: format!(
                "field '{}' requires actor '{}'; invoker is '{}'{}",
                field_path.join("."),
                required_actor,
                invoker.actor,
                remedy,
            ),
        });
    }
}

/// Check that the invoker is permitted to invoke a transition verb.
pub fn check_transition_actor(
    verb: &str,
    transition_actor: Actor,
    invoker: InvokerCtx,
    errors: &mut Vec<ValidationError>,
) {
    if !actor_allowed(invoker.actor, transition_actor, invoker.token_valid) {
        let remedy = invoker_remedy(invoker.actor, transition_actor);
        errors.push(ValidationError {
            field_path: vec![format!("<transition:{}>", verb)],
            rule: RuleKind::Actor,
            message: format!(
                "transition '{}' requires actor '{}'; invoker is '{}'{}",
                verb, transition_actor, invoker.actor, remedy,
            ),
        });
    }
}

/// Returns true when `invoker` satisfies `required`, optionally relaxed by a
/// validated approve-token.
///
/// Rule:
/// - `human` required → `human` invoker, OR `ai_with_human` invoker WITH
///   `token_valid=true` (chat-mediated assent). Bare `ai_autonomous` never
///   satisfies, even with a valid token — the AI-only case is the one we will
///   not relax (Done When §3).
/// - `ai_autonomous` required → only `ai_autonomous` is acceptable; the token
///   does NOT unlock autonomous.
/// - `ai_with_human` required → `human` or `ai_with_human` is acceptable;
///   bare `ai_autonomous` is NOT sufficient (human oversight must be declared).
///   Token does not change this branch — `ai_with_human` already satisfies.
/// - `framework` required → only `framework` (the engine) is acceptable;
///   no human or agent invoker can satisfy this. Token irrelevant.
fn actor_allowed(invoker: Actor, required: Actor, token_valid: bool) -> bool {
    match required {
        Actor::Human => invoker == Actor::Human || (invoker == Actor::AiWithHuman && token_valid),
        Actor::AiAutonomous => invoker == Actor::AiAutonomous,
        Actor::AiWithHuman => invoker == Actor::Human || invoker == Actor::AiWithHuman,
        Actor::Framework => invoker == Actor::Framework,
    }
}

/// Build the suffix for an actor-mismatch error: detection note + concrete remedy.
///
/// The remedy is keyed off `required` (not the invoker) so the operator is told
/// the actor that will actually satisfy the constraint — closes L267-walk feedback
/// item 2, where the previous always-suggest-`--invoker human` wording was wrong
/// for fields/transitions that require `ai_autonomous` or `ai_with_human`.
fn invoker_remedy(invoker: Actor, required: Actor) -> String {
    if required == Actor::Framework {
        return " (this field is reserved for the framework engine and not writable from the CLI)"
            .to_string();
    }

    let auto_note = match invoker {
        Actor::AiAutonomous => " (auto-detected from $CLAUDECODE)",
        _ => "",
    };

    // The actor flag the operator must declare to satisfy `required`.
    // For `human`, the token-mediated path is the alternative for AI sessions —
    // name it explicitly so the operator sees both routes.
    // For `ai_with_human`, a `human` invoker is also accepted — name both.
    let flag_clause = match required {
        Actor::Human => {
            "pass --invoker human, or --invoker ai_with_human --approve-token <T>".to_string()
        }
        Actor::AiAutonomous => "pass --invoker ai_autonomous".to_string(),
        Actor::AiWithHuman => "pass --invoker ai_with_human (or --invoker human)".to_string(),
        Actor::Framework => unreachable!(), // handled above
    };

    format!("{}; to proceed: {}", auto_note, flag_clause)
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
            auto_increment: false,
            auto_increment_within: None,
            default: None,
        }
    }

    fn entry_with(key: &str, val: &str) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert(key.to_string(), serde_json::Value::String(val.to_string()));
        m
    }

    fn ctx(actor: Actor) -> InvokerCtx {
        InvokerCtx::bare(actor)
    }

    fn ctx_token(actor: Actor) -> InvokerCtx {
        InvokerCtx {
            actor,
            token_valid: true,
        }
    }

    // ---- check_actor tests ----

    #[test]
    fn human_invoker_human_field_passes() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::Human),
            None,
            &mut errors,
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn ai_invoker_human_field_fails_with_claudecode_hint() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
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
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
        assert!(
            errors.is_empty(),
            "absent fields must not trigger actor errors"
        );
    }

    #[test]
    fn field_without_actor_constraint_passes_any_invoker() {
        let field = text_field_with_actor("title", None);
        let entry = entry_with("title", "foo");
        let mut errors = vec![];
        check_actor(
            &field,
            &["title".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
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
            ctx(Actor::AiAutonomous),
            Some(Actor::Human),
            &mut errors,
        );
        assert_eq!(errors.len(), 1, "store-default actor must apply");
        assert!(errors[0].message.contains("requires actor 'human'"));
    }

    #[test]
    fn ai_with_human_field_allows_human_and_ai_with_human() {
        let field = text_field_with_actor("notes", Some(Actor::AiWithHuman));
        let entry = entry_with("notes", "text");
        let mut errors = vec![];
        check_actor(
            &field,
            &["notes".to_string()],
            &entry,
            ctx(Actor::Human),
            None,
            &mut errors,
        );
        assert!(
            errors.is_empty(),
            "human should be allowed for ai_with_human"
        );
        let mut errors2 = vec![];
        check_actor(
            &field,
            &["notes".to_string()],
            &entry,
            ctx(Actor::AiWithHuman),
            None,
            &mut errors2,
        );
        assert!(
            errors2.is_empty(),
            "ai_with_human invoker should be allowed for ai_with_human"
        );
    }

    #[test]
    fn ai_with_human_field_rejects_ai_autonomous() {
        let field = text_field_with_actor("notes", Some(Actor::AiWithHuman));
        let entry = entry_with("notes", "text");
        let mut errors = vec![];
        check_actor(
            &field,
            &["notes".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
        assert_eq!(
            errors.len(),
            1,
            "bare ai_autonomous must be rejected for ai_with_human field"
        );
        assert!(
            errors[0].message.contains("requires actor 'ai_with_human'"),
            "msg: {}",
            errors[0].message
        );
    }

    #[test]
    fn framework_field_only_satisfied_by_framework_invoker() {
        let field = text_field_with_actor("current_phase", Some(Actor::Framework));
        let entry = entry_with("current_phase", "1");
        // Human invoker must be rejected
        let mut errors = vec![];
        check_actor(
            &field,
            &["current_phase".to_string()],
            &entry,
            ctx(Actor::Human),
            None,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("framework"));
        // AiAutonomous invoker must be rejected
        let mut errors2 = vec![];
        check_actor(
            &field,
            &["current_phase".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors2,
        );
        assert_eq!(errors2.len(), 1);
        // Framework invoker must pass
        let mut errors3 = vec![];
        check_actor(
            &field,
            &["current_phase".to_string()],
            &entry,
            ctx(Actor::Framework),
            None,
            &mut errors3,
        );
        assert!(
            errors3.is_empty(),
            "Framework invoker must satisfy framework field"
        );
    }

    // ---- check_transition_actor tests ----

    #[test]
    fn transition_actor_mismatch_fires() {
        let mut errors = vec![];
        check_transition_actor(
            "answer",
            Actor::Human,
            ctx(Actor::AiAutonomous),
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("transition 'answer'"), "msg: {msg}");
        assert!(msg.contains("requires actor 'human'"), "msg: {msg}");
        assert!(msg.contains("$CLAUDECODE"), "msg: {msg}");
    }

    // ---- L267-walk feedback item 2: remedy names the required actor, not always 'human' ----

    #[test]
    fn transition_remedy_for_ai_autonomous_required_does_not_suggest_human() {
        let mut errors = vec![];
        check_transition_actor(
            "claim",
            Actor::AiAutonomous,
            ctx(Actor::AiWithHuman),
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("requires actor 'ai_autonomous'"), "msg: {msg}");
        assert!(msg.contains("pass --invoker ai_autonomous"), "msg: {msg}");
        assert!(
            !msg.contains("pass --invoker human"),
            "msg must NOT suggest human: {msg}"
        );
    }

    #[test]
    fn field_remedy_for_ai_with_human_required_offers_both_invokers() {
        let field = text_field_with_actor("notes", Some(Actor::AiWithHuman));
        let entry = entry_with("notes", "text");
        let mut errors = vec![];
        check_actor(
            &field,
            &["notes".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("--invoker ai_with_human"), "msg: {msg}");
        assert!(
            msg.contains("--invoker human"),
            "msg should also mention human as alternative: {msg}"
        );
    }

    #[test]
    fn transition_remedy_for_framework_required_calls_out_engine_only() {
        let mut errors = vec![];
        check_transition_actor("internal", Actor::Framework, ctx(Actor::Human), &mut errors);
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(msg.contains("framework engine"), "msg: {msg}");
        assert!(
            !msg.contains("--invoker"),
            "framework remedy must not suggest --invoker: {msg}"
        );
    }

    #[test]
    fn transition_actor_match_passes() {
        let mut errors = vec![];
        check_transition_actor(
            "close",
            Actor::AiAutonomous,
            ctx(Actor::AiAutonomous),
            &mut errors,
        );
        assert!(errors.is_empty());
    }

    // ---- T001 P3: token-mediated actor relaxation tests (AC3.1) ----

    /// AC3.1 (a): actor:human + invoker=ai_with_human + token_valid=true → allowed.
    /// The chat-mediated assent path: human typed yes, AI holds a valid token.
    #[test]
    fn human_field_allows_ai_with_human_when_token_valid() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx_token(Actor::AiWithHuman),
            None,
            &mut errors,
        );
        assert!(
            errors.is_empty(),
            "ai_with_human + valid token must satisfy actor:human"
        );
    }

    /// AC3.1 (b): actor:human + invoker=ai_with_human + token_valid=false → rejected.
    /// Bare ai_with_human (no token) is NOT enough for actor:human.
    #[test]
    fn human_field_rejects_ai_with_human_when_token_invalid() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::AiWithHuman),
            None,
            &mut errors,
        );
        assert_eq!(
            errors.len(),
            1,
            "ai_with_human without token must NOT satisfy actor:human"
        );
        assert!(errors[0].message.contains("requires actor 'human'"));
    }

    /// AC3.1 (c): actor:human + invoker=ai_autonomous + token_valid=true → STILL rejected.
    /// The token does NOT relax the AI-only case (Done When §3).
    #[test]
    fn human_field_rejects_ai_autonomous_even_with_valid_token() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx_token(Actor::AiAutonomous),
            None,
            &mut errors,
        );
        assert_eq!(
            errors.len(),
            1,
            "ai_autonomous + token must STILL be rejected for actor:human"
        );
        assert!(errors[0].message.contains("requires actor 'human'"));
    }

    /// AC3.1 (d): actor:human + invoker=human + token_valid=false → allowed (preserved).
    #[test]
    fn human_field_allows_human_invoker_without_token() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::Human),
            None,
            &mut errors,
        );
        assert!(
            errors.is_empty(),
            "human invoker preserves prior behaviour without token"
        );
    }

    /// AC3.1 (e): actor:ai_autonomous + token_valid=true → still rejected for non-autonomous.
    /// Token does not unlock autonomous-required transitions.
    #[test]
    fn ai_autonomous_field_rejects_ai_with_human_even_with_token() {
        let field = text_field_with_actor("internal", Some(Actor::AiAutonomous));
        let entry = entry_with("internal", "x");
        let mut errors = vec![];
        check_actor(
            &field,
            &["internal".to_string()],
            &entry,
            ctx_token(Actor::AiWithHuman),
            None,
            &mut errors,
        );
        assert_eq!(
            errors.len(),
            1,
            "token does NOT unlock actor:ai_autonomous from ai_with_human"
        );
    }

    /// AC3.1 (f): remedy for actor:human mentions both `--invoker human` AND `--approve-token`.
    #[test]
    fn human_field_remedy_mentions_approve_token() {
        let field = text_field_with_actor("answer", Some(Actor::Human));
        let entry = entry_with("answer", "yes");
        let mut errors = vec![];
        check_actor(
            &field,
            &["answer".to_string()],
            &entry,
            ctx(Actor::AiAutonomous),
            None,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        let msg = &errors[0].message;
        assert!(
            msg.contains("--invoker human"),
            "remedy must name --invoker human; got: {msg}"
        );
        assert!(
            msg.contains("--approve-token"),
            "remedy must name --approve-token; got: {msg}"
        );
    }

    /// Mirror of (a) for transitions: actor:human transition + ai_with_human + token → allowed.
    #[test]
    fn human_transition_allows_ai_with_human_when_token_valid() {
        let mut errors = vec![];
        check_transition_actor(
            "confirm",
            Actor::Human,
            ctx_token(Actor::AiWithHuman),
            &mut errors,
        );
        assert!(
            errors.is_empty(),
            "ai_with_human + valid token must satisfy actor:human transition"
        );
    }

    /// Mirror of (c) for transitions: actor:human transition + ai_autonomous + token → still rejected.
    #[test]
    fn human_transition_rejects_ai_autonomous_even_with_valid_token() {
        let mut errors = vec![];
        check_transition_actor(
            "confirm",
            Actor::Human,
            ctx_token(Actor::AiAutonomous),
            &mut errors,
        );
        assert_eq!(
            errors.len(),
            1,
            "token must NOT unlock ai_autonomous on actor:human transition"
        );
    }
}
