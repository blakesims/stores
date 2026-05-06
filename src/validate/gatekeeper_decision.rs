use regex::Regex;
use serde_json::Value;

const VALID_DECISIONS: &[&str] = &[
    "duplicate",
    "needs_info",
    "fast_track",
    "normal_observation",
    "arch_review_candidate",
    "reject_noise",
];

const VALID_CONFIDENCE: &[&str] = &["low", "medium", "high"];
const VALID_TIER_HINT: &[&str] = &["T0", "T1", "T2", "T3"];

pub const VALID_RISK_FLAGS: &[&str] = &[
    // Architectural-risk flags (any one triggers arch_review_candidate eligibility)
    "touches_actor_authority",
    "touches_lifecycle",
    "touches_subscriber_semantics",
    "touches_runner_boundary",
    "touches_schema_core",
    "introduces_new_primitive",
    "changes_boundary",
    "security_sensitive",
    "authority_surface_drift",
    // Low-risk / fast-track-eligible flags
    "docs_only",
    "small_local_fix",
    // Bookkeeping flags
    "duplicate_symptom",
    "contradicts_prior_decision",
];

/// Flags that are incompatible with the fast_track decision (PROHIBIT list).
/// Per docs/gatekeeper-design.md § Fast-track policy: any one of these surfaces
/// is forbidden from fast-track, regardless of how trivial the change appears.
pub const PROHIBIT_FLAGS_FOR_FAST_TRACK: &[&str] = &[
    "touches_actor_authority",
    "touches_lifecycle",
    "touches_subscriber_semantics",
    "touches_runner_boundary",
    "touches_schema_core",
    "introduces_new_primitive",
    "changes_boundary",
    "security_sensitive",
    "authority_surface_drift",
    "contradicts_prior_decision",
];

const DUPLICATE_CANDIDATE_PATTERN: &str = r"^(I|L)[0-9]{3,}$";
const CLUSTER_KEY_PATTERN: &str = r"^[a-z][a-z0-9-]{2,40}$";

const MAX_RATIONALE: usize = 1200;
const MAX_RECOMMENDED_NEXT: usize = 400;
const MAX_MISSING_INFO_QUESTION: usize = 400;

const KNOWN_KEYS: &[&str] = &[
    "decision",
    "confidence",
    "rationale",
    "tier_hint",
    "risk_flags",
    "duplicate_candidates",
    "cluster_key",
    "missing_info_question",
    "recommended_next",
];

/// Validate a `gatekeeper_decision_json` payload.
///
/// Enforces the schema in docs/gatekeeper-design.md § Gatekeeper output schema:
/// required fields, enum values, conditional requirements, max lengths,
/// uniqueItems on risk_flags, and the PROHIBIT fast-track policy.
///
/// Returns `Ok(())` on a valid payload; `Err(errors)` with all violations
/// collected in one pass.
pub fn validate_gatekeeper_decision(value: &Value) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            return Err(vec![
                "gatekeeper_decision_json must be a JSON object".to_string()
            ]);
        }
    };

    // additional_properties: false
    for key in obj.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            errors.push(format!(
                "unknown field '{}' (additional_properties: false)",
                key
            ));
        }
    }

    // decision (required, enum)
    let decision = match obj.get("decision").and_then(|v| v.as_str()) {
        Some(d) => {
            if !VALID_DECISIONS.contains(&d) {
                errors.push(format!(
                    "decision '{}' is not one of: {}",
                    d,
                    VALID_DECISIONS.join(", ")
                ));
                ""
            } else {
                d
            }
        }
        None => {
            errors.push("decision is required".to_string());
            ""
        }
    };

    // confidence (required, enum)
    match obj.get("confidence").and_then(|v| v.as_str()) {
        Some(c) => {
            if !VALID_CONFIDENCE.contains(&c) {
                errors.push(format!(
                    "confidence '{}' is not one of: {}",
                    c,
                    VALID_CONFIDENCE.join(", ")
                ));
            }
        }
        None => {
            errors.push("confidence is required".to_string());
        }
    }

    // rationale (required, max_length 1200)
    match obj.get("rationale").and_then(|v| v.as_str()) {
        Some(r) => {
            if r.len() > MAX_RATIONALE {
                errors.push(format!(
                    "rationale exceeds max length {} (got {})",
                    MAX_RATIONALE,
                    r.len()
                ));
            }
        }
        None => {
            errors.push("rationale is required".to_string());
        }
    }

    // tier_hint: required_when decision ∈ {fast_track, normal_observation, arch_review_candidate}
    let tier_hint_required = matches!(
        decision,
        "fast_track" | "normal_observation" | "arch_review_candidate"
    );
    match obj.get("tier_hint").and_then(|v| v.as_str()) {
        Some(t) => {
            if !VALID_TIER_HINT.contains(&t) {
                errors.push(format!(
                    "tier_hint '{}' is not one of: {}",
                    t,
                    VALID_TIER_HINT.join(", ")
                ));
            }
        }
        None => {
            if tier_hint_required {
                errors.push(format!(
                    "tier_hint is required when decision is '{}'",
                    decision
                ));
            }
        }
    }

    // risk_flags: optional array, items must be valid enum values, uniqueItems=true
    let risk_flags: Vec<&str> = match obj.get("risk_flags") {
        Some(Value::Array(arr)) => {
            let mut flags: Vec<&str> = Vec::new();
            for (i, item) in arr.iter().enumerate() {
                match item.as_str() {
                    Some(f) => {
                        if !VALID_RISK_FLAGS.contains(&f) {
                            errors.push(format!(
                                "risk_flags[{}]: '{}' is not a valid risk flag (valid: {})",
                                i,
                                f,
                                VALID_RISK_FLAGS.join(", ")
                            ));
                        } else if flags.contains(&f) {
                            errors.push(format!(
                                "risk_flags: '{}' appears more than once (uniqueItems violation)",
                                f
                            ));
                        } else {
                            flags.push(f);
                        }
                    }
                    None => {
                        errors.push(format!("risk_flags[{}]: must be a string", i));
                    }
                }
            }
            flags
        }
        Some(_) => {
            errors.push("risk_flags must be an array".to_string());
            vec![]
        }
        None => vec![],
    };

    // duplicate_candidates: required_when decision==duplicate, pattern, min_items=1
    let dup_re = Regex::new(DUPLICATE_CANDIDATE_PATTERN).unwrap();
    match obj.get("duplicate_candidates") {
        Some(Value::Array(arr)) => {
            if decision == "duplicate" && arr.is_empty() {
                errors.push(
                    "duplicate_candidates must have at least 1 item when decision is 'duplicate'"
                        .to_string(),
                );
            }
            for (i, item) in arr.iter().enumerate() {
                match item.as_str() {
                    Some(s) => {
                        if !dup_re.is_match(s) {
                            errors.push(format!(
                                "duplicate_candidates[{}]: '{}' does not match pattern {}",
                                i, s, DUPLICATE_CANDIDATE_PATTERN
                            ));
                        }
                    }
                    None => {
                        errors.push(format!(
                            "duplicate_candidates[{}]: must be a string",
                            i
                        ));
                    }
                }
            }
        }
        Some(_) => {
            errors.push("duplicate_candidates must be an array".to_string());
        }
        None => {
            if decision == "duplicate" {
                errors.push(
                    "duplicate_candidates is required when decision is 'duplicate'".to_string(),
                );
            }
        }
    }

    // cluster_key: required_when decision ∈ {normal_observation, arch_review_candidate, duplicate}
    let cluster_key_required = matches!(
        decision,
        "normal_observation" | "arch_review_candidate" | "duplicate"
    );
    let cluster_re = Regex::new(CLUSTER_KEY_PATTERN).unwrap();
    match obj.get("cluster_key").and_then(|v| v.as_str()) {
        Some(k) => {
            if !cluster_re.is_match(k) {
                errors.push(format!(
                    "cluster_key '{}' does not match pattern {}",
                    k, CLUSTER_KEY_PATTERN
                ));
            }
        }
        None => {
            if cluster_key_required {
                errors.push(format!(
                    "cluster_key is required when decision is '{}'",
                    decision
                ));
            }
        }
    }

    // missing_info_question: required_when decision==needs_info, max_length 400
    match obj.get("missing_info_question").and_then(|v| v.as_str()) {
        Some(q) => {
            if q.len() > MAX_MISSING_INFO_QUESTION {
                errors.push(format!(
                    "missing_info_question exceeds max length {} (got {})",
                    MAX_MISSING_INFO_QUESTION,
                    q.len()
                ));
            }
        }
        None => {
            if decision == "needs_info" {
                errors.push(
                    "missing_info_question is required when decision is 'needs_info'".to_string(),
                );
            }
        }
    }

    // recommended_next: optional, max_length 400
    if let Some(r) = obj.get("recommended_next").and_then(|v| v.as_str()) {
        if r.len() > MAX_RECOMMENDED_NEXT {
            errors.push(format!(
                "recommended_next exceeds max length {} (got {})",
                MAX_RECOMMENDED_NEXT,
                r.len()
            ));
        }
    }

    // AC2.5: fast_track PROHIBITED when risk_flags intersect any PROHIBIT-surface flag.
    // Per docs/gatekeeper-design.md § Fast-track policy.
    if decision == "fast_track" {
        let violations: Vec<&&str> = risk_flags
            .iter()
            .filter(|f| PROHIBIT_FLAGS_FOR_FAST_TRACK.contains(f))
            .collect();
        if !violations.is_empty() {
            errors.push(format!(
                "fast_track decision is PROHIBITED because risk_flags contains: {}. \
                 These PROHIBIT-surface flags must route to normal_observation or \
                 arch_review_candidate instead.",
                violations.iter().map(|f| **f).collect::<Vec<_>>().join(", ")
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_fast_track() -> Value {
        json!({
            "decision": "fast_track",
            "confidence": "high",
            "rationale": "Typo fix in docs only.",
            "tier_hint": "T1",
            "risk_flags": ["docs_only"]
        })
    }

    fn valid_normal_observation() -> Value {
        json!({
            "decision": "normal_observation",
            "confidence": "medium",
            "rationale": "Stale PID cleanup needed.",
            "tier_hint": "T2",
            "risk_flags": ["small_local_fix"],
            "cluster_key": "dispatch-lifecycle"
        })
    }

    fn valid_needs_info() -> Value {
        json!({
            "decision": "needs_info",
            "confidence": "low",
            "rationale": "Need to check if this is reproduced.",
            "missing_info_question": "Does this repro on a clean checkout?"
        })
    }

    fn valid_duplicate() -> Value {
        json!({
            "decision": "duplicate",
            "confidence": "high",
            "rationale": "Already filed as L042.",
            "cluster_key": "dispatch-lifecycle",
            "duplicate_candidates": ["L042"]
        })
    }

    fn valid_reject_noise() -> Value {
        json!({
            "decision": "reject_noise",
            "confidence": "high",
            "rationale": "This is a known local env issue, not a substrate bug."
        })
    }

    fn valid_arch_review() -> Value {
        json!({
            "decision": "arch_review_candidate",
            "confidence": "high",
            "rationale": "Touches lifecycle state machine.",
            "tier_hint": "T3",
            "risk_flags": ["touches_lifecycle"],
            "cluster_key": "dispatch-lifecycle"
        })
    }

    // ---- Happy paths ----

    #[test]
    fn valid_fast_track_passes() {
        validate_gatekeeper_decision(&valid_fast_track()).unwrap();
    }

    #[test]
    fn valid_normal_observation_passes() {
        validate_gatekeeper_decision(&valid_normal_observation()).unwrap();
    }

    #[test]
    fn valid_needs_info_passes() {
        validate_gatekeeper_decision(&valid_needs_info()).unwrap();
    }

    #[test]
    fn valid_duplicate_passes() {
        validate_gatekeeper_decision(&valid_duplicate()).unwrap();
    }

    #[test]
    fn valid_reject_noise_passes() {
        validate_gatekeeper_decision(&valid_reject_noise()).unwrap();
    }

    #[test]
    fn valid_arch_review_passes() {
        validate_gatekeeper_decision(&valid_arch_review()).unwrap();
    }

    // ---- Required field violations ----

    #[test]
    fn missing_decision_rejected() {
        let v = json!({"confidence": "high", "rationale": "ok"});
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("decision is required")), "{:?}", errs);
    }

    #[test]
    fn missing_confidence_rejected() {
        let v = json!({"decision": "reject_noise", "rationale": "ok"});
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("confidence is required")), "{:?}", errs);
    }

    #[test]
    fn missing_rationale_rejected() {
        let v = json!({"decision": "reject_noise", "confidence": "high"});
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("rationale is required")), "{:?}", errs);
    }

    // ---- Enum violations ----

    #[test]
    fn invalid_decision_rejected() {
        let v = json!({"decision": "unknown_decision", "confidence": "high", "rationale": "x"});
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("unknown_decision")), "{:?}", errs);
    }

    #[test]
    fn invalid_confidence_rejected() {
        let v = json!({"decision": "reject_noise", "confidence": "certain", "rationale": "x"});
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("certain")), "{:?}", errs);
    }

    #[test]
    fn invalid_tier_hint_rejected() {
        let v = json!({
            "decision": "normal_observation",
            "confidence": "high",
            "rationale": "x",
            "tier_hint": "T5",
            "cluster_key": "foo-bar-baz"
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("T5")), "{:?}", errs);
    }

    // ---- required_when: tier_hint ----

    #[test]
    fn tier_hint_required_for_fast_track() {
        let mut v = valid_fast_track();
        v.as_object_mut().unwrap().remove("tier_hint");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tier_hint") && e.contains("fast_track")),
            "{:?}", errs
        );
    }

    #[test]
    fn tier_hint_required_for_normal_observation() {
        let mut v = valid_normal_observation();
        v.as_object_mut().unwrap().remove("tier_hint");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tier_hint") && e.contains("normal_observation")),
            "{:?}", errs
        );
    }

    #[test]
    fn tier_hint_required_for_arch_review_candidate() {
        let mut v = valid_arch_review();
        v.as_object_mut().unwrap().remove("tier_hint");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tier_hint") && e.contains("arch_review_candidate")),
            "{:?}", errs
        );
    }

    #[test]
    fn tier_hint_not_required_for_reject_noise() {
        validate_gatekeeper_decision(&valid_reject_noise()).unwrap();
    }

    #[test]
    fn tier_hint_not_required_for_needs_info() {
        validate_gatekeeper_decision(&valid_needs_info()).unwrap();
    }

    // ---- risk_flags ----

    #[test]
    fn invalid_risk_flag_rejected() {
        let v = json!({
            "decision": "reject_noise",
            "confidence": "high",
            "rationale": "x",
            "risk_flags": ["not_a_real_flag"]
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("not_a_real_flag")), "{:?}", errs);
    }

    #[test]
    fn duplicate_risk_flag_rejected() {
        let v = json!({
            "decision": "reject_noise",
            "confidence": "high",
            "rationale": "x",
            "risk_flags": ["docs_only", "docs_only"]
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("uniqueItems")), "{:?}", errs);
    }

    // ---- required_when: duplicate_candidates ----

    #[test]
    fn duplicate_requires_duplicate_candidates() {
        let v = json!({
            "decision": "duplicate",
            "confidence": "high",
            "rationale": "already filed",
            "cluster_key": "foo-bar-baz"
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("duplicate_candidates") && e.contains("required")),
            "{:?}", errs
        );
    }

    #[test]
    fn duplicate_candidates_must_be_non_empty() {
        let v = json!({
            "decision": "duplicate",
            "confidence": "high",
            "rationale": "already filed",
            "cluster_key": "foo-bar-baz",
            "duplicate_candidates": []
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("at least 1")),
            "{:?}", errs
        );
    }

    #[test]
    fn duplicate_candidates_pattern_enforced() {
        let v = json!({
            "decision": "duplicate",
            "confidence": "high",
            "rationale": "already filed",
            "cluster_key": "foo-bar-baz",
            "duplicate_candidates": ["T001"]  // T### is tasks, not valid here
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("T001") && e.contains("pattern")),
            "{:?}", errs
        );
    }

    #[test]
    fn duplicate_candidates_valid_l_prefix_passes() {
        validate_gatekeeper_decision(&valid_duplicate()).unwrap();
    }

    // ---- required_when: cluster_key ----

    #[test]
    fn cluster_key_required_for_normal_observation() {
        let mut v = valid_normal_observation();
        v.as_object_mut().unwrap().remove("cluster_key");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("cluster_key") && e.contains("normal_observation")),
            "{:?}", errs
        );
    }

    #[test]
    fn cluster_key_required_for_arch_review_candidate() {
        let mut v = valid_arch_review();
        v.as_object_mut().unwrap().remove("cluster_key");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("cluster_key") && e.contains("arch_review_candidate")),
            "{:?}", errs
        );
    }

    #[test]
    fn cluster_key_required_for_duplicate() {
        let mut v = valid_duplicate();
        v.as_object_mut().unwrap().remove("cluster_key");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("cluster_key") && e.contains("duplicate")),
            "{:?}", errs
        );
    }

    #[test]
    fn cluster_key_pattern_enforced() {
        let v = json!({
            "decision": "normal_observation",
            "confidence": "high",
            "rationale": "x",
            "tier_hint": "T2",
            "cluster_key": "Bad Key With Spaces"
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("cluster_key") && e.contains("pattern")),
            "{:?}", errs
        );
    }

    // ---- required_when: missing_info_question ----

    #[test]
    fn needs_info_requires_missing_info_question() {
        let v = json!({
            "decision": "needs_info",
            "confidence": "low",
            "rationale": "x"
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("missing_info_question") && e.contains("required")),
            "{:?}", errs
        );
    }

    // ---- max_length ----

    #[test]
    fn rationale_too_long_rejected() {
        let long = "x".repeat(1201);
        let v = json!({
            "decision": "reject_noise",
            "confidence": "high",
            "rationale": long
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("rationale") && e.contains("max length")),
            "{:?}", errs
        );
    }

    #[test]
    fn missing_info_question_too_long_rejected() {
        let long = "x".repeat(401);
        let v = json!({
            "decision": "needs_info",
            "confidence": "low",
            "rationale": "x",
            "missing_info_question": long
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("missing_info_question") && e.contains("max length")),
            "{:?}", errs
        );
    }

    // ---- additional_properties: false ----

    #[test]
    fn unknown_field_rejected() {
        let v = json!({
            "decision": "reject_noise",
            "confidence": "high",
            "rationale": "x",
            "unknown_bonus_field": "oops"
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("unknown_bonus_field")),
            "{:?}", errs
        );
    }

    // ---- AC2.5: fast_track PROHIBIT policy ----

    #[test]
    fn fast_track_with_touches_lifecycle_rejected() {
        let v = json!({
            "decision": "fast_track",
            "confidence": "high",
            "rationale": "Looks small.",
            "tier_hint": "T1",
            "risk_flags": ["touches_lifecycle"]
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("PROHIBITED") && e.contains("touches_lifecycle")),
            "{:?}", errs
        );
    }

    #[test]
    fn fast_track_with_security_sensitive_rejected() {
        let v = json!({
            "decision": "fast_track",
            "confidence": "high",
            "rationale": "Minor token tweak.",
            "tier_hint": "T0",
            "risk_flags": ["security_sensitive"]
        });
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("PROHIBITED") && e.contains("security_sensitive")),
            "{:?}", errs
        );
    }

    #[test]
    fn fast_track_with_only_low_risk_flags_passes() {
        let v = json!({
            "decision": "fast_track",
            "confidence": "high",
            "rationale": "Typo fix only.",
            "tier_hint": "T0",
            "risk_flags": ["docs_only", "small_local_fix"]
        });
        validate_gatekeeper_decision(&v).unwrap();
    }

    #[test]
    fn non_object_rejected() {
        let v = json!("not an object");
        let errs = validate_gatekeeper_decision(&v).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("must be a JSON object")), "{:?}", errs);
    }
}
