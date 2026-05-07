use serde_json::{json, Value};
use stores::schema::actor::Actor;
use stores::schema::Schema;
use stores::validate::{validate, EntryMap, Op};
use stores::validate::error::RuleKind;

fn schema() -> Schema {
    let yaml = std::fs::read_to_string("stores/observations/schema.yaml").unwrap();
    Schema::from_yaml(&yaml).unwrap()
}

fn base_entry(intent_contract: Value) -> EntryMap {
    let mut m = EntryMap::new();
    m.insert("summary".into(), json!("obs"));
    m.insert("source".into(), json!("dev"));
    m.insert("priority".into(), json!("normal"));
    m.insert("captured_at".into(), json!("2026-05-07T00:00:00Z"));
    m.insert("captured_week".into(), json!("w-test"));
    m.insert("intent_contract".into(), intent_contract);
    m
}

fn full_harden_log() -> Value {
    json!({
        "decisions": [{"id":"D1","decision":"do x","rationale":"because y","source_quote":"user said y"}],
        "scope_cuts": [{"cut":"not z","rationale":"outside objective","source_quote":"only x"}],
        "alternatives_rejected": [{"alternative":"do z","why_rejected":"scope creep"}],
        "compress_vs_surface": [{"item":"edge case","judgment":"surface","rationale":"operator must decide"}],
        "unresolved_questions": ["is y stable?"]
    })
}

#[test]
fn schema_exposes_nullable_harden_log_shape() {
    let s = schema();
    let intent = s
        .fields
        .iter()
        .find(|f| f.name == "intent_contract")
        .unwrap();
    let stores::schema::FieldType::Record(fields) = &intent.ty else {
        panic!("intent_contract record")
    };
    let harden = fields.iter().find(|f| f.name == "harden_log").unwrap();
    assert!(!harden.required);
    let stores::schema::FieldType::Record(hfields) = &harden.ty else {
        panic!("harden_log record")
    };
    for name in [
        "decisions",
        "scope_cuts",
        "alternatives_rejected",
        "compress_vs_surface",
        "unresolved_questions",
    ] {
        assert!(hfields.iter().any(|f| f.name == name), "missing {name}");
    }
}

#[test]
fn valid_harden_log_passes_validate() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": full_harden_log()}));
    validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap();
}

#[test]
fn harden_log_null_or_absent_passes_and_is_not_ready_gate() {
    let s = schema();
    validate(
        &s,
        &base_entry(json!({"harden_log": null})),
        Op::Add,
        Actor::AiWithHuman.into(),
    )
    .unwrap();
    validate(
        &s,
        &base_entry(json!({})),
        Op::Add,
        Actor::AiWithHuman.into(),
    )
    .unwrap();

    let ready = base_entry(json!({
        "contract_state":"ready",
        "drafted_by":"sidecar",
        "drafted_at":"2026-05-07T00:00:00Z",
        "objective":"do x",
        "type":"work",
        "in_scope":["x"],
        "out_of_scope":["z"],
        "acceptance":["x works"],
        "tier_hint":"T2",
        "approved_by":"blake",
        "approved_at":"2026-05-07T00:00:00Z",
        "harden_log": null
    }));
    validate(&s, &ready, Op::Add, Actor::Human.into()).unwrap();
}

#[test]
fn malformed_harden_log_reports_nested_paths() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {
        "decisions": [{"id":"D1"}],
        "scope_cuts": [{"source_quote":"q"}],
        "alternatives_rejected": "not-array",
        "compress_vs_surface": {"item":"not-array"},
        "unresolved_questions": "not-array"
    }}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    let paths: Vec<String> = errs.iter().map(|e| e.field_path.join(".")).collect();
    assert!(
        paths.contains(&"intent_contract.harden_log.decisions.decision".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.decisions.rationale".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.scope_cuts.cut".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.scope_cuts.rationale".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.alternatives_rejected".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.compress_vs_surface".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"intent_contract.harden_log.unresolved_questions".to_string()),
        "{paths:?}"
    );
}

#[test]
fn scalar_harden_log_and_non_string_unresolved_question_fail() {
    let s = schema();
    let scalar = validate(
        &s,
        &base_entry(json!({"harden_log": "not-object"})),
        Op::Add,
        Actor::AiWithHuman.into(),
    )
    .unwrap_err();
    let scalar_paths: Vec<String> = scalar.iter().map(|e| e.field_path.join(".")).collect();
    assert!(
        scalar_paths.contains(&"intent_contract.harden_log".to_string()),
        "{scalar_paths:?}"
    );

    let utf8_boundary_scalar = validate(
        &s,
        &base_entry(json!({"harden_log": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaé"})),
        Op::Add,
        Actor::AiWithHuman.into(),
    )
    .unwrap_err();
    let utf8_paths: Vec<String> = utf8_boundary_scalar
        .iter()
        .map(|e| e.field_path.join("."))
        .collect();
    assert!(
        utf8_paths.contains(&"intent_contract.harden_log".to_string()),
        "{utf8_paths:?}"
    );

    let bad_question = validate(
        &s,
        &base_entry(json!({"harden_log": {
            "unresolved_questions": ["ok", {"bad":"object"}]
        }})),
        Op::Add,
        Actor::AiWithHuman.into(),
    )
    .unwrap_err();
    let bad_paths: Vec<String> = bad_question
        .iter()
        .map(|e| e.field_path.join("."))
        .collect();
    assert!(
        bad_paths.contains(&"intent_contract.harden_log.unresolved_questions".to_string()),
        "{bad_paths:?}"
    );
}

#[test]
fn recursive_merge_preserves_harden_log_siblings() {
    let mut entry = base_entry(json!({"objective":"keep", "harden_log": {
        "decisions": [{"id":"D1","decision":"old","rationale":"old r"}],
        "scope_cuts": [{"cut":"old cut","rationale":"old r"}]
    }}));
    stores::handlers::row::deep_merge_entry_field(
        &mut entry,
        "intent_contract",
        &json!({"harden_log": {
            "alternatives_rejected": [{"alternative":"a","why_rejected":"b"}]
        }}),
    );
    let ic = &entry["intent_contract"];
    assert_eq!(ic["objective"], json!("keep"));
    assert!(ic["harden_log"]["decisions"].is_array());
    assert!(ic["harden_log"]["scope_cuts"].is_array());
    assert!(ic["harden_log"]["alternatives_rejected"].is_array());
}

#[test]
fn harden_log_markdown_includes_content_and_null_omits() {
    let rendered = stores::output::harden_log_markdown(&full_harden_log());
    assert!(rendered.contains("Decisions"));
    assert!(rendered.contains("do x"));
    assert!(rendered.contains("Unresolved questions"));
    assert!(stores::output::harden_log_markdown(&Value::Null).is_empty());
}

#[test]
fn observation_show_text_formatter_includes_and_omits_harden_log() {
    let with = base_entry(json!({"objective":"keep", "harden_log": full_harden_log()}));
    let text = stores::output::entry_text(&with);
    assert!(text.contains("intent_contract:"));
    assert!(text.contains("harden_log:"));
    assert!(text.contains("do x"));

    let null_text = stores::output::entry_text(&base_entry(json!({"harden_log": null})));
    assert!(!null_text.contains("harden_log:"), "{null_text}");

    let absent_text = stores::output::entry_text(&base_entry(json!({"objective":"keep"})));
    assert!(!absent_text.contains("harden_log:"), "{absent_text}");

    let raw_json =
        serde_json::to_value(base_entry(json!({"harden_log": full_harden_log()}))).unwrap();
    assert!(raw_json["intent_contract"]["harden_log"].is_object());
}

#[test]
fn task_template_includes_harden_logs_only_when_context_non_empty() {
    let tpl = std::fs::read_to_string("stores/tasks/templates/main.md.tpl").unwrap();
    let base = json!({
        "display_id":"T001","title":"Task","status":"planning","created_at":"c","updated_at":"u","current_phase":1,"current_cycle":1,
        "contract":{},"plan":{"phases":[]},"plan_review_log":[],"cycles":[],"cycles_have_reviews":false
    });
    let without = stores::render::render_template(&tpl, &base).unwrap();
    assert!(!without.contains("Intent Contract Harden Log"));
    let mut with = base;
    with.as_object_mut().unwrap().insert(
        "linked_observation_harden_logs".into(),
        json!([{"display_id":"L001","rendered":"### Decisions\n- **decision:** do x\n"}]),
    );
    let rendered = stores::render::render_template(&tpl, &with).unwrap();
    assert!(rendered.contains("Intent Contract Harden Log"));
    assert!(rendered.contains("Observation L001"));
    assert!(rendered.contains("do x"));
}

#[test]
fn prompt_guidance_mentions_bounded_harden_log_not_transcript() {
    let inv = std::fs::read_to_string("agents/investigator.md").unwrap();
    let side = std::fs::read_to_string("agents/sidecar/system-prompt.md").unwrap();
    assert!(inv.contains("observations.intent_contract.harden_log"));
    assert!(inv.contains("not transcript-like"));
    assert!(side.contains("intent_contract.harden_log"));
    assert!(side.contains("bounded audit artifact"));
}

// ---------------------------------------------------------------------------
// Bound-overflow rejection tests (L077 contract: max 20 items, max 500 chars)
// ---------------------------------------------------------------------------

fn make_decisions(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|i| json!({"id": format!("D{i}"), "decision": "x", "rationale": "y"}))
        .collect();
    json!(items)
}

fn make_scope_cuts(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|_| json!({"cut": "c", "rationale": "r"}))
        .collect();
    json!(items)
}

fn make_alternatives_rejected(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|_| json!({"alternative": "a", "why_rejected": "b"}))
        .collect();
    json!(items)
}

fn make_compress_vs_surface(n: usize) -> Value {
    let items: Vec<Value> = (0..n)
        .map(|_| json!({"item": "i", "judgment": "j", "rationale": "r"}))
        .collect();
    json!(items)
}

fn make_unresolved_questions(n: usize) -> Value {
    let items: Vec<Value> = (0..n).map(|i| json!(format!("q{i}"))).collect();
    json!(items)
}

#[test]
fn harden_log_max_items_decisions_21_rejected() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {"decisions": make_decisions(21)}}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 20, actual: 21 }
        ) && e.field_path.last().map(|s| s.as_str()) == Some("decisions")),
        "expected BoundsExceeded on decisions, got: {errs:?}"
    );
}

#[test]
fn harden_log_max_items_scope_cuts_21_rejected() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {"scope_cuts": make_scope_cuts(21)}}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 20, actual: 21 }
        ) && e.field_path.last().map(|s| s.as_str()) == Some("scope_cuts")),
        "expected BoundsExceeded on scope_cuts, got: {errs:?}"
    );
}

#[test]
fn harden_log_max_items_alternatives_rejected_21_rejected() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {"alternatives_rejected": make_alternatives_rejected(21)}}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 20, actual: 21 }
        ) && e.field_path.last().map(|s| s.as_str()) == Some("alternatives_rejected")),
        "expected BoundsExceeded on alternatives_rejected, got: {errs:?}"
    );
}

#[test]
fn harden_log_max_items_compress_vs_surface_21_rejected() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {"compress_vs_surface": make_compress_vs_surface(21)}}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 20, actual: 21 }
        ) && e.field_path.last().map(|s| s.as_str()) == Some("compress_vs_surface")),
        "expected BoundsExceeded on compress_vs_surface, got: {errs:?}"
    );
}

#[test]
fn harden_log_max_items_unresolved_questions_21_rejected() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {"unresolved_questions": make_unresolved_questions(21)}}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 20, actual: 21 }
        ) && e.field_path.last().map(|s| s.as_str()) == Some("unresolved_questions")),
        "expected BoundsExceeded on unresolved_questions, got: {errs:?}"
    );
}

#[test]
fn harden_log_max_items_exactly_20_passes() {
    let s = schema();
    let entry = base_entry(json!({"harden_log": {
        "decisions": make_decisions(20),
        "scope_cuts": make_scope_cuts(20),
        "alternatives_rejected": make_alternatives_rejected(20),
        "compress_vs_surface": make_compress_vs_surface(20),
        "unresolved_questions": make_unresolved_questions(20)
    }}));
    validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap();
}

#[test]
fn harden_log_string_501_chars_in_decision_field_rejected() {
    let s = schema();
    let long_str: String = "a".repeat(501);
    let entry = base_entry(json!({"harden_log": {
        "decisions": [{"id": "D1", "decision": long_str, "rationale": "r"}]
    }}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 500, actual: 501 }
        )),
        "expected BoundsExceeded(500, 501) for 501-char decision, got: {errs:?}"
    );
}

#[test]
fn harden_log_string_500_chars_passes() {
    let s = schema();
    let ok_str: String = "a".repeat(500);
    let entry = base_entry(json!({"harden_log": {
        "decisions": [{"id": "D1", "decision": ok_str, "rationale": "r"}]
    }}));
    validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap();
}

#[test]
fn harden_log_string_501_chars_in_unresolved_questions_rejected() {
    let s = schema();
    let long_str: String = "x".repeat(501);
    let entry = base_entry(json!({"harden_log": {
        "unresolved_questions": [long_str]
    }}));
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            &e.rule,
            RuleKind::BoundsExceeded { limit: 500, actual: 501 }
        )),
        "expected BoundsExceeded(500, 501) for 501-char unresolved_question, got: {errs:?}"
    );
}

// ---------------------------------------------------------------------------
// Malformed-byte regression test (non-UTF-8 input)
//
// The CLI write-path takes &str, so non-UTF-8 bytes cannot enter through
// coerce_value. They can arrive via the SQLite blob→String path (lossy), which
// means by the time the validator runs the bytes have already been sanitized by
// from_utf8_lossy. The remaining risk is a malformed harden_log JSON string
// injected directly. Here we test that serde_json (which requires valid UTF-8)
// produces a parse error rather than panicking, and that the resulting sentinel
// value triggers a validator error via the type-shape check path (not a panic).
//
// We feed the raw bytes as a String built from replacement chars to simulate
// what from_utf8_lossy produces, then confirm validation rejects the sentinel.
// ---------------------------------------------------------------------------
#[test]
fn harden_log_non_utf8_bytes_via_lossy_path_no_panic() {
    // Simulate what the SQLite blob->String path produces for invalid UTF-8:
    // invalid bytes are replaced with U+FFFD by from_utf8_lossy.
    let invalid_bytes: Vec<u8> = vec![0xFF, 0xFE, 0x80];
    let lossy = String::from_utf8_lossy(&invalid_bytes).into_owned();
    // The lossy string is valid UTF-8 (replacement chars), but is not valid JSON.
    // Feed it as the raw harden_log value (as if it came through the blob path).
    // serde_json::from_str should fail; the validator must not panic.
    let parse_result = serde_json::from_str::<serde_json::Value>(&lossy);
    // The lossy output (U+FFFD chars) is not valid JSON — parse must fail gracefully.
    assert!(
        parse_result.is_err(),
        "expected JSON parse failure for lossy-encoded non-UTF-8, got: {parse_result:?}"
    );
    // Verify no panic by wrapping the value as a sentinel string in the entry map
    // (mimicking coerce_value for a JSON/Record field that gets bad input).
    let s = schema();
    let entry = base_entry(json!({"harden_log": lossy}));
    // Should produce a shape error (string instead of object), not a panic.
    let errs = validate(&s, &entry, Op::Add, Actor::AiWithHuman.into()).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field_path.last().map(|s| s.as_str()) == Some("harden_log")),
        "expected shape error on harden_log for sentinel string, got: {errs:?}"
    );
}
