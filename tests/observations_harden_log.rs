use serde_json::{json, Value};
use stores::schema::actor::Actor;
use stores::schema::Schema;
use stores::validate::{validate, EntryMap, Op};

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
