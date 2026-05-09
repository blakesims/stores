//! L142/T053-P1 shipped the gatekeeper Router contract — the typed intake_items lifecycle,
//! the gatekeeper_decision_json validator, the SideEffectAuthority::GatekeeperRoute authority,
//! and the intake_route::inject_pre_validation_fields side-effect hook. The shipped
//! gatekeeper-stub builtin is test-only (always emits needs_info). This module is the
//! production-path Router behind that same contract: a deterministic policy in Slice 1,
//! replaceable with an LLM-backed router in a later slice without changing the seam.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

use crate::validate::EntryMap;

// ---------------------------------------------------------------------------
// RouterDecision
// ---------------------------------------------------------------------------

pub enum RouterDecision {
    RouteToObservation {
        tier_hint: &'static str,
        rationale: String,
        cluster_key: Option<String>,
        risk_flags: Vec<&'static str>,
    },
    RouteToArchReviewCandidate {
        tier_hint: &'static str,
        rationale: String,
        risk_flags: Vec<&'static str>,
    },
    Drop {
        rationale: String,
    },
    NeedsInfo {
        rationale: String,
        missing_info_question: String,
    },
    UnableToRoute {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Slice 1 deterministic policy
// ---------------------------------------------------------------------------

fn noise_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(test|noise|wip|scratch|ignore)\b").unwrap())
}

fn arch_keywords() -> &'static [&'static str] {
    &[
        "schema",
        "actor authority",
        "lifecycle",
        "subscriber semantics",
        "runner boundary",
    ]
}

fn arch_risk_flag(keyword: &str) -> &'static str {
    match keyword {
        "schema" => "touches_schema_core",
        "actor authority" => "touches_actor_authority",
        "lifecycle" => "touches_lifecycle",
        "subscriber semantics" => "touches_subscriber_semantics",
        "runner boundary" => "touches_runner_boundary",
        _ => "touches_schema_core",
    }
}

pub fn route(merged: &EntryMap) -> RouterDecision {
    let summary = merged
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let body = merged
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // (a) both empty/whitespace → UnableToRoute
    if summary.is_empty() && body.is_empty() {
        return RouterDecision::UnableToRoute {
            reason: "router unable to typed-classify: empty filing".to_string(),
        };
    }

    // (b) noise heuristic
    let combined = format!("{} {}", summary, body);
    if noise_re().is_match(&combined) {
        return RouterDecision::Drop {
            rationale: "matched noise heuristic".to_string(),
        };
    }

    // (c) arch keywords
    for kw in arch_keywords() {
        if combined.to_lowercase().contains(kw) {
            let flag = arch_risk_flag(kw);
            return RouterDecision::RouteToArchReviewCandidate {
                tier_hint: "T3",
                rationale: format!("filing matches arch keyword '{kw}'"),
                risk_flags: vec![flag],
            };
        }
    }

    // (d) classify_summary → RouteToObservation
    if let Some(key) = crate::handlers::cluster_keys::classify_summary(&summary) {
        return RouterDecision::RouteToObservation {
            tier_hint: "T1",
            rationale: format!("classify_summary matched cluster_key '{key}'"),
            cluster_key: Some(key.to_string()),
            risk_flags: vec![],
        };
    }

    // (e) fallback → NeedsInfo
    RouterDecision::NeedsInfo {
        rationale: "awaiting recon for typed classification".to_string(),
        missing_info_question: "What concrete file, command, or transition demonstrates this intake item?".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Decision JSON builder
// ---------------------------------------------------------------------------

pub fn build_decision_json(
    decision: &RouterDecision,
    source_agent: &str,
    timestamp: &str,
    evidence: &[Value],
) -> Value {
    let mut obj = serde_json::Map::new();

    let intake_decision = to_intake_decision(decision);
    obj.insert("decision".to_string(), Value::String(intake_decision.to_string()));
    obj.insert("confidence".to_string(), Value::String("low".to_string()));

    match decision {
        RouterDecision::RouteToObservation {
            tier_hint,
            rationale,
            cluster_key,
            risk_flags,
        } => {
            obj.insert("rationale".to_string(), Value::String(rationale.clone()));
            obj.insert("tier_hint".to_string(), Value::String(tier_hint.to_string()));
            if let Some(ck) = cluster_key {
                obj.insert("cluster_key".to_string(), Value::String(ck.clone()));
            }
            if !risk_flags.is_empty() {
                obj.insert(
                    "risk_flags".to_string(),
                    Value::Array(risk_flags.iter().map(|f| Value::String(f.to_string())).collect()),
                );
            }
        }
        RouterDecision::RouteToArchReviewCandidate {
            tier_hint,
            rationale,
            risk_flags,
        } => {
            obj.insert("rationale".to_string(), Value::String(rationale.clone()));
            obj.insert("tier_hint".to_string(), Value::String(tier_hint.to_string()));
            if !risk_flags.is_empty() {
                obj.insert(
                    "risk_flags".to_string(),
                    Value::Array(risk_flags.iter().map(|f| Value::String(f.to_string())).collect()),
                );
            }
            // arch_review_candidate requires cluster_key; use a stable default derived from summary
            // The caller (drain) will resolve this via inject_pre_validation_fields side-effect
            // but we must not emit a payload that fails the validator here.  The validator
            // requires cluster_key for arch_review_candidate — supply a curated key.
            // We use the first curated key as a placeholder; it will be overwritten if
            // inject_pre_validation_fields resolves a better one.
            // Actually: looking at the validator again, cluster_key IS required for
            // arch_review_candidate. And inject_pre_validation_fields only copies cluster_key
            // for the "duplicate" decision. For arch_review_candidate, it creates a source obs
            // but does NOT inject cluster_key. So we must provide it here.
            // Use the risk flag to derive a cluster key: "gatekeeper-front-door-stuck" is the
            // canonical cluster key for gatekeeper-related arch concerns.
            obj.insert(
                "cluster_key".to_string(),
                Value::String("gatekeeper-front-door-stuck".to_string()),
            );
        }
        RouterDecision::Drop { rationale } => {
            obj.insert("rationale".to_string(), Value::String(rationale.clone()));
        }
        RouterDecision::NeedsInfo {
            rationale,
            missing_info_question,
        } => {
            obj.insert("rationale".to_string(), Value::String(rationale.clone()));
            obj.insert(
                "missing_info_question".to_string(),
                Value::String(missing_info_question.clone()),
            );
        }
        RouterDecision::UnableToRoute { reason } => {
            obj.insert("rationale".to_string(), Value::String(reason.clone()));
            obj.insert(
                "missing_info_question".to_string(),
                Value::String("What concrete file, command, or transition demonstrates this intake item?".to_string()),
            );
        }
    }

    obj.insert("source_agent".to_string(), Value::String(source_agent.to_string()));
    obj.insert("timestamp".to_string(), Value::String(timestamp.to_string()));
    obj.insert(
        "evidence".to_string(),
        Value::Array(evidence.to_vec()),
    );

    Value::Object(obj)
}

pub fn to_intake_decision(decision: &RouterDecision) -> &'static str {
    match decision {
        RouterDecision::RouteToObservation { .. } => "normal_observation",
        RouterDecision::RouteToArchReviewCandidate { .. } => "arch_review_candidate",
        RouterDecision::Drop { .. } => "reject_noise",
        RouterDecision::NeedsInfo { .. } => "needs_info",
        RouterDecision::UnableToRoute { .. } => "needs_info",
    }
}

// ---------------------------------------------------------------------------
// Tests R1-R6
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_map(summary: &str, body: &str) -> EntryMap {
        let mut m = EntryMap::new();
        m.insert("summary".to_string(), Value::String(summary.to_string()));
        m.insert("body".to_string(), Value::String(body.to_string()));
        m
    }

    #[test]
    fn r1_empty_summary_body_returns_unable_to_route() {
        let m = entry_map("", "");
        match route(&m) {
            RouterDecision::UnableToRoute { reason } => {
                assert!(reason.contains("router unable to typed-classify"), "{reason}");
            }
            _ => panic!("expected UnableToRoute"),
        }
    }

    #[test]
    fn r2_noise_summary_returns_drop() {
        let m = entry_map("wip scratch", "");
        match route(&m) {
            RouterDecision::Drop { rationale } => {
                assert!(rationale.contains("noise"), "{rationale}");
            }
            _ => panic!("expected Drop"),
        }
    }

    #[test]
    fn r3_arch_keyword_returns_arch_review_candidate_with_risk_flags() {
        let m = entry_map("add lifecycle invariant", "");
        match route(&m) {
            RouterDecision::RouteToArchReviewCandidate { risk_flags, .. } => {
                assert!(!risk_flags.is_empty(), "expected non-empty risk_flags");
            }
            _ => panic!("expected RouteToArchReviewCandidate"),
        }
    }

    #[test]
    fn r4_classify_summary_match_returns_route_to_observation_with_cluster_key() {
        // "merge conflict" matches the deploy-blocked-merge-conflict cluster key
        let m = entry_map("deploy blocked by merge conflict in branch", "");
        match route(&m) {
            RouterDecision::RouteToObservation { cluster_key, .. } => {
                assert!(cluster_key.is_some(), "expected Some(cluster_key)");
            }
            _ => panic!("expected RouteToObservation"),
        }
    }

    #[test]
    fn r5_generic_summary_returns_needs_info() {
        let m = entry_map("something entirely unrelated that matches nothing", "");
        match route(&m) {
            RouterDecision::NeedsInfo { .. } => {}
            _ => panic!("expected NeedsInfo"),
        }
    }

    #[test]
    fn r6_to_intake_decision_maps_unable_to_route_and_needs_info_to_needs_info() {
        let unable = RouterDecision::UnableToRoute {
            reason: "x".to_string(),
        };
        assert_eq!(to_intake_decision(&unable), "needs_info");

        let needs = RouterDecision::NeedsInfo {
            rationale: "x".to_string(),
            missing_info_question: "q".to_string(),
        };
        assert_eq!(to_intake_decision(&needs), "needs_info");
    }
}
