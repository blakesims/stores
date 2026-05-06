//! T053 Phase 5 — fail-loud intake/gatekeeper rejection coverage.

use serde_json::json;
use std::fs;
use std::path::Path;
use stores::validate::gatekeeper_decision::validate_gatekeeper_decision;

fn errors(value: serde_json::Value) -> Vec<String> {
    validate_gatekeeper_decision(&value).expect_err("payload must be rejected")
}

#[test]
fn rejects_non_object_payload() {
    let errs = errors(json!("not-object"));
    assert!(errs.iter().any(|e| e.contains("must be a JSON object")), "{errs:?}");
}

#[test]
fn rejects_required_field_violations() {
    for (payload, needle) in [
        (json!({"confidence":"high","rationale":"x"}), "decision is required"),
        (json!({"decision":"reject_noise","rationale":"x"}), "confidence is required"),
        (json!({"decision":"reject_noise","confidence":"high"}), "rationale is required"),
    ] {
        let errs = errors(payload);
        assert!(errs.iter().any(|e| e.contains(needle)), "needle={needle}; errs={errs:?}");
    }
}

#[test]
fn rejects_enum_and_type_violations() {
    for (payload, needle) in [
        (json!({"decision":"bogus","confidence":"high","rationale":"x"}), "bogus"),
        (json!({"decision":"reject_noise","confidence":"certain","rationale":"x"}), "certain"),
        (json!({"decision":"normal_observation","confidence":"high","rationale":"x","tier_hint":"T9","cluster_key":"abc-def"}), "T9"),
        (json!({"decision":"reject_noise","confidence":"high","rationale":"x","risk_flags":"docs_only"}), "risk_flags must be an array"),
        (json!({"decision":"reject_noise","confidence":"high","rationale":"x","risk_flags":[7]}), "risk_flags[0]: must be a string"),
    ] {
        let errs = errors(payload);
        assert!(errs.iter().any(|e| e.contains(needle)), "needle={needle}; errs={errs:?}");
    }
}

#[test]
fn rejects_conditional_schema_violations() {
    for (payload, needle) in [
        (json!({"decision":"fast_track","confidence":"high","rationale":"x","risk_flags":["docs_only"]}), "tier_hint"),
        (json!({"decision":"normal_observation","confidence":"high","rationale":"x","tier_hint":"T2"}), "cluster_key"),
        (json!({"decision":"duplicate","confidence":"high","rationale":"x","cluster_key":"abc-def"}), "duplicate_candidates"),
        (json!({"decision":"duplicate","confidence":"high","rationale":"x","cluster_key":"abc-def","duplicate_candidates":[]}), "at least 1"),
        (json!({"decision":"needs_info","confidence":"low","rationale":"x"}), "missing_info_question"),
    ] {
        let errs = errors(payload);
        assert!(errs.iter().any(|e| e.contains(needle)), "needle={needle}; errs={errs:?}");
    }
}

#[test]
fn rejects_pattern_length_unique_and_additional_property_violations() {
    for (payload, needle) in [
        (json!({"decision":"normal_observation","confidence":"high","rationale":"x","tier_hint":"T2","cluster_key":"Bad Key"}), "cluster_key"),
        (json!({"decision":"duplicate","confidence":"high","rationale":"x","cluster_key":"abc-def","duplicate_candidates":["T001"]}), "duplicate_candidates[0]"),
        (json!({"decision":"reject_noise","confidence":"high","rationale":"x","risk_flags":["docs_only","docs_only"]}), "uniqueItems"),
        (json!({"decision":"reject_noise","confidence":"high","rationale":"x","unknown":"y"}), "unknown field"),
        (json!({"decision":"reject_noise","confidence":"high","rationale":"x".repeat(1201)}), "rationale exceeds"),
        (json!({"decision":"needs_info","confidence":"low","rationale":"x","missing_info_question":"x".repeat(401)}), "missing_info_question exceeds"),
    ] {
        let errs = errors(payload);
        assert!(errs.iter().any(|e| e.contains(needle)), "needle={needle}; errs={errs:?}");
    }
}

#[test]
fn rejects_fast_track_with_prohibit_surface_flag() {
    let errs = errors(json!({
        "decision": "fast_track",
        "confidence": "high",
        "rationale": "Looks small but touches lifecycle.",
        "tier_hint": "T1",
        "risk_flags": ["touches_lifecycle"]
    }));
    assert!(
        errs.iter().any(|e| e.contains("PROHIBITED") && e.contains("touches_lifecycle")),
        "{errs:?}"
    );
}

#[test]
fn implementation_files_do_not_introduce_raw_sql_writes_case_insensitive() {
    // Mechanical audit target from AC5.2: grep over src/handlers/intake*.rs and
    // src/validate/gatekeeper_decision.rs must not find raw-SQL UPDATE/INSERT/DELETE
    // writes on substrate tables, regardless of SQL casing.
    let mut files = vec!["src/validate/gatekeeper_decision.rs".to_string()];
    for entry in fs::read_dir("src/handlers").expect("src/handlers exists") {
        let entry = entry.expect("dir entry");
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("intake") && name.ends_with(".rs") {
            files.push(p.to_string_lossy().to_string());
        }
    }

    let needles = [
        "update intake",
        "insert into intake",
        "delete from intake",
        "update observations",
        "insert into observations",
        "delete from observations",
    ];
    let mut hits = Vec::new();
    for file in files {
        let body = fs::read_to_string(Path::new(&file))
            .expect("read audited file")
            .to_lowercase();
        for needle in needles {
            if body.contains(needle) {
                hits.push(format!("{file}: {needle}"));
            }
        }
    }
    assert!(hits.is_empty(), "raw SQL write hits: {hits:?}");
}
