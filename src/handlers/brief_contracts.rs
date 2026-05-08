//! Brief-context structural contracts — cargo-test-time invariants over rendered briefs.
//!
//! ## Purpose
//!
//! This registry of [`BriefContract`] implementations asserts structural invariants
//! over rendered brief text at cargo-test time. These contracts are layered on top of
//! the existing `brief.rs` template-render tests and prevent the I022/c0f45ff/5b6a41a
//! class of silent-omission regressions from shipping: the class where required context
//! (prior executor summary, external-review findings, plan-review backpressure) is
//! present in the entry data but missing from the rendered brief that agents actually
//! read.
//!
//! Contracts are evaluated by [`apply_all`] against a precomputed [`BriefContext`].
//! Each contract's [`BriefContract::applies`] predicate gates whether the contract is
//! relevant to the current role/situation; [`BriefContract::evaluate`] asserts the
//! structural invariant and returns a deterministic [`CheckResult`].
//!
//! ## I026 note
//!
//! I026 (agent literal-obedience / cognition) is out of scope for this module.
//! Brief contracts assert *structure* — whether required text is present in the
//! rendered brief — not *agent behavior* in response to that text. Closing I026
//! requires a separate, orthogonal intervention.
//!
//! ## How to add a new contract
//!
//! 1. Define an `id()` constant at the top of the module (e.g.,
//!    `pub const MY_NEW_CONTRACT: &str = "my_new_contract_id"`).
//! 2. Implement [`BriefContract`] on a unit struct, filling in `id()`, `applies()`,
//!    and `evaluate()`.
//! 3. Register the static instance in `REGISTRY` in declaration order.
//! 4. Add both a passing-fixture test (asserts `CheckResult::is_pass()`) and a
//!    failing-fixture test (asserts `outcome == CheckOutcome::Fail` with a non-null
//!    `reason` naming the missing artifact) to the `#[cfg(test)]` module below.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::flow::checks::CheckResult;
use crate::validate::EntryMap;

// ---------------------------------------------------------------------------
// Contract ID constants
// ---------------------------------------------------------------------------

pub const PLANNER_REVISION_MUST_INCLUDE_REJECTED_PLAN_AND_REVIEWS: &str =
    "planner_revision_must_include_rejected_plan_and_reviews";
pub const EXECUTOR_REVISE_MUST_INCLUDE_PRIOR_EXECUTOR_AND_CODE_REVIEW: &str =
    "executor_revise_must_include_prior_executor_and_code_review";
pub const EXECUTOR_EXTERNAL_REVISE_MUST_INCLUDE_EXTERNAL_REVIEW_BACKPRESSURE: &str =
    "executor_external_revise_must_include_external_review_backpressure";
pub const PROVENANCE_LABELS_MUST_DISTINGUISH_INTERNAL_VS_EXTERNAL: &str =
    "provenance_labels_must_distinguish_internal_vs_external";
pub const SOURCE_OBSERVATION_PROVENANCE_PRESENT_FOR_PLANNER: &str =
    "source_observation_provenance_present_for_planner";

// ---------------------------------------------------------------------------
// Input shape
// ---------------------------------------------------------------------------

/// Fully-precomputed input to a [`BriefContract`].
///
/// All fields are precomputed by the caller before contract evaluation; no
/// `Connection` is needed, and no I/O fallibility crosses the contract surface.
/// The `overlay` field is precomputed (rather than computed by the contract)
/// because `build_external_review_overlay` returns `Result`; pushing I/O
/// fallibility outside the contract surface matches what `compute()` already
/// does at `brief.rs:157-162`.
pub struct BriefContext<'a> {
    /// The fully-rendered brief text.
    pub rendered: &'a str,
    /// The raw entry map (DB row fields as JSON values).
    pub entry: &'a EntryMap,
    /// The overlay map merged into the template context (same shape as produced
    /// by `build_external_review_overlay` / `build_source_observation_overlay`).
    pub overlay: &'a HashMap<String, Value>,
    /// The agent role this brief was rendered for (e.g. `"planner"`, `"executor"`).
    pub agent_role: &'a str,
}

// ---------------------------------------------------------------------------
// BriefContract trait
// ---------------------------------------------------------------------------

/// A deterministic structural invariant over a rendered brief.
///
/// ## Why a local trait instead of `flow::checks::Check`?
///
/// The runtime [`crate::flow::checks::Check`] trait takes
/// `CheckCtx { conn, companion }` and returns `Result<CheckResult>`. Brief
/// contracts take a fully-precomputed [`BriefContext`] (rendered text + entry +
/// overlay + agent_role) and are infallible — they never do I/O. The input-shape
/// mismatch makes reusing the runtime trait impractical.
///
/// ## Why `applies()` is infallible
///
/// `applies()` inspects only in-memory data (entry fields, overlay values, the
/// `agent_role` string); there is no I/O to fail. Making it infallible keeps the
/// registry loop in [`apply_all`] simple.
///
/// ## Why the overlay is precomputed
///
/// `build_external_review_overlay` returns `Result<HashMap>` because it queries
/// the DB. Precomputing the overlay at call time (as `compute()` already does at
/// `brief.rs:157-162`) pushes all I/O fallibility outside the contract surface.
pub trait BriefContract: Sync {
    fn id(&self) -> &'static str;
    fn applies(&self, ctx: &BriefContext<'_>) -> bool;
    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn current_cycle_val(entry: &EntryMap) -> i64 {
    entry
        .get("current_cycle")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// Returns `(executor.summary, executor.commit, review.summary)` from the prior REVISE cycle if one exists.
///
/// Looks for a `cycles[]` entry where `cycle == current_cycle - 1`, `review.gate == "REVISE"`,
/// and `executor.summary` is non-empty.
fn get_prior_revise_cycle(entry: &EntryMap) -> Option<(String, String, String)> {
    let cur = current_cycle_val(entry);
    if cur < 2 {
        return None;
    }
    let prev = cur - 1;
    let cycles = match entry.get("cycles") {
        Some(Value::Array(a)) => a,
        _ => return None,
    };
    for cycle_val in cycles {
        let cycle_num = cycle_val.get("cycle").and_then(|v| v.as_i64());
        if cycle_num != Some(prev) {
            continue;
        }
        let executor_obj = cycle_val.get("executor");
        let executor_summary = executor_obj
            .and_then(|e| e.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let executor_commit = executor_obj
            .and_then(|e| e.get("commit"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let review_gate = cycle_val
            .get("review")
            .and_then(|r| r.get("gate"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let review_summary = cycle_val
            .get("review")
            .and_then(|r| r.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if review_gate == "REVISE" && !executor_summary.is_empty() {
            return Some((
                executor_summary.to_string(),
                executor_commit.to_string(),
                review_summary.to_string(),
            ));
        }
    }
    None
}

/// Returns `true` if the entry has a prior in-cycle REVISE backpressure body:
/// `current_cycle > 1` AND a `cycles[]` entry for `cycle == current_cycle - 1`
/// with both `executor.summary` AND `review.gate == "REVISE"`.
///
/// Exposed for reuse by Contract 4.
pub(crate) fn has_prior_internal_revise(entry: &EntryMap) -> bool {
    get_prior_revise_cycle(entry).is_some()
}

/// Returns `true` if the overlay contains a non-null `external_review_backpressure` value.
///
/// Exposed for reuse by Contract 4.
pub(crate) fn has_external_revise(overlay: &HashMap<String, Value>) -> bool {
    matches!(
        overlay.get("external_review_backpressure"),
        Some(v) if !v.is_null()
    )
}

// ---------------------------------------------------------------------------
// Contract 1 — PLANNER_REVISION_MUST_INCLUDE_REJECTED_PLAN_AND_REVIEWS
// ---------------------------------------------------------------------------

struct PlannerRevisionMustIncludeRejectedPlanAndReviews;

impl BriefContract for PlannerRevisionMustIncludeRejectedPlanAndReviews {
    fn id(&self) -> &'static str {
        PLANNER_REVISION_MUST_INCLUDE_REJECTED_PLAN_AND_REVIEWS
    }

    fn applies(&self, ctx: &BriefContext<'_>) -> bool {
        if ctx.agent_role != "planner" {
            return false;
        }
        let plan_phases_non_empty = ctx
            .entry
            .get("plan")
            .and_then(|p| p.get("phases"))
            .and_then(|ph| ph.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let review_log_non_empty = ctx
            .entry
            .get("plan_review_log")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        plan_phases_non_empty && review_log_non_empty
    }

    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult {
        let id = self.id();
        let args = json!(null);
        let rendered = ctx.rendered;

        let objective = ctx
            .entry
            .get("plan")
            .and_then(|p| p.get("objective"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !objective.is_empty() && !rendered.contains(objective) {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing rejected plan objective"}),
            );
        }

        let phases = ctx
            .entry
            .get("plan")
            .and_then(|p| p.get("phases"))
            .and_then(|ph| ph.as_array())
            .cloned()
            .unwrap_or_default();
        let phase_found = phases.iter().any(|ph| {
            ph.get("name")
                .and_then(|v| v.as_str())
                .map(|name| rendered.contains(name))
                .unwrap_or(false)
        });
        if !phases.is_empty() && !phase_found {
            let first_name = phases[0]
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            return CheckResult::fail(
                id,
                &args,
                json!({"message": format!("missing phase name '{}'", first_name)}),
            );
        }

        let review_log = ctx
            .entry
            .get("plan_review_log")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let review_found = review_log.iter().any(|r| {
            let summary_match = r
                .get("summary")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| rendered.contains(s))
                .unwrap_or(false);
            let gate_match = r
                .get("gate")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| rendered.contains(s))
                .unwrap_or(false);
            summary_match || gate_match
        });
        if !review_log.is_empty() && !review_found {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing review summary substring"}),
            );
        }

        CheckResult::pass(id, &args)
    }
}

// ---------------------------------------------------------------------------
// Contract 2 — EXECUTOR_REVISE_MUST_INCLUDE_PRIOR_EXECUTOR_AND_CODE_REVIEW
// ---------------------------------------------------------------------------

struct ExecutorReviseMustIncludePriorExecutorAndCodeReview;

impl BriefContract for ExecutorReviseMustIncludePriorExecutorAndCodeReview {
    fn id(&self) -> &'static str {
        EXECUTOR_REVISE_MUST_INCLUDE_PRIOR_EXECUTOR_AND_CODE_REVIEW
    }

    fn applies(&self, ctx: &BriefContext<'_>) -> bool {
        ctx.agent_role == "executor" && has_prior_internal_revise(ctx.entry)
    }

    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult {
        let id = self.id();
        let args = json!(null);
        let rendered = ctx.rendered;

        let (executor_summary, executor_commit, review_summary) =
            match get_prior_revise_cycle(ctx.entry) {
                Some(triple) => triple,
                None => return CheckResult::pass(id, &args),
            };

        if !executor_summary.is_empty() && !rendered.contains(executor_summary.as_str()) {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing prior executor summary"}),
            );
        }

        if !executor_commit.is_empty() && !rendered.contains(executor_commit.as_str()) {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing prior executor commit"}),
            );
        }

        if !review_summary.is_empty() && !rendered.contains(review_summary.as_str()) {
            // Also accept review.details as fallback
            let cur = current_cycle_val(ctx.entry);
            let prev = cur - 1;
            let has_details = ctx
                .entry
                .get("cycles")
                .and_then(|v| v.as_array())
                .map(|cycles| {
                    cycles.iter().any(|cycle| {
                        if cycle.get("cycle").and_then(|v| v.as_i64()) != Some(prev) {
                            return false;
                        }
                        cycle
                            .get("review")
                            .and_then(|r| r.get("details"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|details| rendered.contains(details))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if !has_details {
                return CheckResult::fail(
                    id,
                    &args,
                    json!({"message": "missing prior cycle review summary"}),
                );
            }
        }

        let has_count = rendered.contains("critical")
            || rendered.contains("major")
            || rendered.contains("minor");
        if !has_count {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing review count token (critical/major/minor)"}),
            );
        }

        CheckResult::pass(id, &args)
    }
}

// ---------------------------------------------------------------------------
// Contract 3 — EXECUTOR_EXTERNAL_REVISE_MUST_INCLUDE_EXTERNAL_REVIEW_BACKPRESSURE
// ---------------------------------------------------------------------------

/// Selection-logic-reuse contract: this contract consumes the overlay shape
/// produced by `build_external_review_overlay()` at `src/handlers/brief.rs:241`.
/// The same function is the canonical overlay producer in `compute()` at
/// `brief.rs:160`. Selection-logic reuse is verified by Task 1.13's test, which
/// calls `build_external_review_overlay()` against an in-memory `external_reviews`
/// table to populate the `BriefContext` overlay and confirms `applies()` fires on
/// exactly the row the overlay builder returns.
struct ExecutorExternalReviseMustIncludeExternalReviewBackpressure;

impl BriefContract for ExecutorExternalReviseMustIncludeExternalReviewBackpressure {
    fn id(&self) -> &'static str {
        EXECUTOR_EXTERNAL_REVISE_MUST_INCLUDE_EXTERNAL_REVIEW_BACKPRESSURE
    }

    fn applies(&self, ctx: &BriefContext<'_>) -> bool {
        ctx.agent_role == "executor" && has_external_revise(ctx.overlay)
    }

    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult {
        let id = self.id();
        let args = json!(null);
        let rendered = ctx.rendered;

        let er = match ctx.overlay.get("external_review_backpressure") {
            Some(v) if !v.is_null() => v,
            _ => return CheckResult::pass(id, &args),
        };

        let mut missing: Vec<String> = Vec::new();

        let display_id = er.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
        if !display_id.is_empty() && !rendered.contains(display_id) {
            missing.push("display_id".to_string());
        }

        let runner = er.get("runner").and_then(|v| v.as_str()).unwrap_or("");
        if !runner.is_empty() && !rendered.contains(runner) {
            missing.push("runner".to_string());
        }

        if !rendered.contains("REVISE") {
            missing.push("REVISE verdict".to_string());
        }

        let head_sha = er.get("head_sha").and_then(|v| v.as_str()).unwrap_or("");
        if !head_sha.is_empty() && !rendered.contains(head_sha) {
            missing.push("head_sha".to_string());
        }

        let base_sha = er.get("base_sha").and_then(|v| v.as_str()).unwrap_or("");
        if !base_sha.is_empty() && !rendered.contains(base_sha) {
            missing.push("base_sha".to_string());
        }

        // Assert the actual numeric count values appear (e.g. "0 critical", "1 major") rather
        // than the bare words — bare-word checks would pass if findings text contains "[major]"
        // while the template's count line was dropped.
        let critical_count = er.get("critical_count").and_then(|v| v.as_i64()).unwrap_or(0);
        let major_count = er.get("major_count").and_then(|v| v.as_i64()).unwrap_or(0);
        let minor_count = er.get("minor_count").and_then(|v| v.as_i64()).unwrap_or(0);

        let critical_token = format!("{} critical", critical_count);
        let major_token = format!("{} major", major_count);
        let minor_token = format!("{} minor", minor_count);

        if !rendered.contains(critical_token.as_str()) {
            missing.push("critical_count token".to_string());
        }
        if !rendered.contains(major_token.as_str()) {
            missing.push("major_count token".to_string());
        }
        if !rendered.contains(minor_token.as_str()) {
            missing.push("minor_count token".to_string());
        }

        let findings = er.get("findings").and_then(|v| v.as_str()).unwrap_or("");
        let findings_prefix = if findings.len() >= 32 {
            &findings[..32]
        } else {
            findings
        };
        if !findings_prefix.is_empty() && !rendered.contains(findings_prefix) {
            missing.push("findings text (first 32 chars)".to_string());
        }

        if !rendered.contains("## External Review Backpressure") {
            missing.push("'## External Review Backpressure' header".to_string());
        }

        if !missing.is_empty() {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": format!("missing fields: {}", missing.join(", "))}),
            );
        }

        CheckResult::pass(id, &args)
    }
}

// ---------------------------------------------------------------------------
// Contract 4 — PROVENANCE_LABELS_MUST_DISTINGUISH_INTERNAL_VS_EXTERNAL
// ---------------------------------------------------------------------------

/// Non-conflation contract for executor briefs carrying backpressure.
///
/// Asserts that when internal and/or external backpressure bodies are present,
/// they appear under their correct section labels and do not bleed into each
/// other's sections. Does NOT require the internal header to be absent when
/// only external backpressure is present — the executor template always renders
/// `## Revision Context for This Phase` with a 'no prior backpressure' placeholder
/// (see `stores/tasks/templates/executor-brief.md.tpl:73-103`); absence-based rules
/// would falsely fail valid external-only briefs.
///
/// The `{{#if external_review_backpressure}}` branch in
/// `stores/tasks/templates/executor-brief.md.tpl:104-118` is the canonical source
/// of the external section structure (Pi msg re: external_review_backpressure
/// conditional rendering).
struct ProvenanceLabelsMustDistinguishInternalVsExternal;

impl BriefContract for ProvenanceLabelsMustDistinguishInternalVsExternal {
    fn id(&self) -> &'static str {
        PROVENANCE_LABELS_MUST_DISTINGUISH_INTERNAL_VS_EXTERNAL
    }

    fn applies(&self, ctx: &BriefContext<'_>) -> bool {
        ctx.agent_role == "executor"
            && (has_prior_internal_revise(ctx.entry) || has_external_revise(ctx.overlay))
    }

    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult {
        let id = self.id();
        let args = json!(null);
        let rendered = ctx.rendered;
        let has_internal = has_prior_internal_revise(ctx.entry);
        let has_external = has_external_revise(ctx.overlay);

        const INTERNAL_HEADER: &str = "## Revision Context for This Phase";
        const EXTERNAL_HEADER: &str = "## External Review Backpressure";

        // (a) When has_external: external header must be present; findings must appear after it.
        if has_external {
            if !rendered.contains(EXTERNAL_HEADER) {
                return CheckResult::fail(
                    id,
                    &args,
                    json!({"message": "missing external label when external body present"}),
                );
            }

            let er = match ctx.overlay.get("external_review_backpressure") {
                Some(v) if !v.is_null() => v,
                _ => return CheckResult::pass(id, &args),
            };
            let findings = er.get("findings").and_then(|v| v.as_str()).unwrap_or("");
            let findings_prefix = if findings.len() >= 32 {
                &findings[..32]
            } else {
                findings
            };

            if !findings_prefix.is_empty() {
                let ext_header_pos = rendered.find(EXTERNAL_HEADER).unwrap();
                match rendered.find(findings_prefix) {
                    None => {
                        return CheckResult::fail(
                            id,
                            &args,
                            json!({"message": "external findings not found in rendered brief"}),
                        );
                    }
                    Some(findings_pos) if findings_pos < ext_header_pos => {
                        return CheckResult::fail(
                            id,
                            &args,
                            json!({"message": "external findings appeared in internal section"}),
                        );
                    }
                    _ => {}
                }
            }
        }

        // (b) When has_internal: internal header must be present; review.summary must
        //     appear after it and before the external header (or end-of-string).
        if has_internal {
            if !rendered.contains(INTERNAL_HEADER) {
                return CheckResult::fail(
                    id,
                    &args,
                    json!({"message": "missing internal label when internal body present"}),
                );
            }

            let (_, _, review_summary) = match get_prior_revise_cycle(ctx.entry) {
                Some(triple) => triple,
                None => return CheckResult::pass(id, &args),
            };

            if !review_summary.is_empty() {
                let internal_header_pos = rendered.find(INTERNAL_HEADER).unwrap();
                match rendered.find(review_summary.as_str()) {
                    None => {
                        return CheckResult::fail(
                            id,
                            &args,
                            json!({"message": "internal review summary not found in rendered brief"}),
                        );
                    }
                    Some(summary_pos) => {
                        if summary_pos < internal_header_pos {
                            return CheckResult::fail(
                                id,
                                &args,
                                json!({"message": "internal review summary appeared before internal header"}),
                            );
                        }
                        if has_external {
                            if let Some(ext_pos) = rendered.find(EXTERNAL_HEADER) {
                                if summary_pos > ext_pos {
                                    return CheckResult::fail(
                                        id,
                                        &args,
                                        json!({"message": "internal review summary appeared in external section"}),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // (c) Cross-contamination check when both are present.
        if has_internal && has_external {
            let er = match ctx.overlay.get("external_review_backpressure") {
                Some(v) if !v.is_null() => v,
                _ => return CheckResult::pass(id, &args),
            };
            let findings = er.get("findings").and_then(|v| v.as_str()).unwrap_or("");
            let findings_prefix = if findings.len() >= 32 {
                &findings[..32]
            } else {
                findings
            };

            if !findings_prefix.is_empty() {
                if let (Some(int_pos), Some(ext_pos)) =
                    (rendered.find(INTERNAL_HEADER), rendered.find(EXTERNAL_HEADER))
                {
                    let internal_section = &rendered[int_pos..ext_pos];
                    if internal_section.contains(findings_prefix) {
                        return CheckResult::fail(
                            id,
                            &args,
                            json!({"message": "external findings appeared in internal section"}),
                        );
                    }
                }
            }

            let (_, _, review_summary) = match get_prior_revise_cycle(ctx.entry) {
                Some(triple) => triple,
                None => return CheckResult::pass(id, &args),
            };
            if !review_summary.is_empty() {
                if let Some(ext_pos) = rendered.find(EXTERNAL_HEADER) {
                    let external_section = &rendered[ext_pos..];
                    if external_section.contains(review_summary.as_str()) {
                        return CheckResult::fail(
                            id,
                            &args,
                            json!({"message": "internal review summary appeared in external section"}),
                        );
                    }
                }
            }
        }

        CheckResult::pass(id, &args)
    }
}

// ---------------------------------------------------------------------------
// Contract 5 — SOURCE_OBSERVATION_PROVENANCE_PRESENT_FOR_PLANNER
// ---------------------------------------------------------------------------

struct SourceObservationProvenancePresentForPlanner;

impl BriefContract for SourceObservationProvenancePresentForPlanner {
    fn id(&self) -> &'static str {
        SOURCE_OBSERVATION_PROVENANCE_PRESENT_FOR_PLANNER
    }

    fn applies(&self, ctx: &BriefContext<'_>) -> bool {
        if ctx.agent_role != "planner" {
            return false;
        }
        let has_linked = match ctx.entry.get("linked_observations") {
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::String(s)) => serde_json::from_str::<Value>(s)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            _ => false,
        };
        if !has_linked {
            return false;
        }
        ctx.overlay
            .get("source_observations")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    }

    fn evaluate(&self, ctx: &BriefContext<'_>) -> CheckResult {
        let id = self.id();
        let args = json!(null);
        let rendered = ctx.rendered;

        if !rendered.contains("## Source Observation Context") {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "missing '## Source Observation Context' header"}),
            );
        }

        let obs_arr = match ctx
            .overlay
            .get("source_observations")
            .and_then(|v| v.as_array())
        {
            Some(a) => a,
            None => {
                return CheckResult::fail(
                    id,
                    &args,
                    json!({"message": "no source_observations in overlay"}),
                )
            }
        };

        let found = obs_arr.iter().any(|obs| {
            obs.get("display_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|did| rendered.contains(did))
                .unwrap_or(false)
        });
        if !found {
            return CheckResult::fail(
                id,
                &args,
                json!({"message": "no observation display_id from overlay found in rendered brief"}),
            );
        }

        CheckResult::pass(id, &args)
    }
}

// ---------------------------------------------------------------------------
// Static instances & registry
// ---------------------------------------------------------------------------

static CONTRACT_1: PlannerRevisionMustIncludeRejectedPlanAndReviews =
    PlannerRevisionMustIncludeRejectedPlanAndReviews;
static CONTRACT_2: ExecutorReviseMustIncludePriorExecutorAndCodeReview =
    ExecutorReviseMustIncludePriorExecutorAndCodeReview;
static CONTRACT_3: ExecutorExternalReviseMustIncludeExternalReviewBackpressure =
    ExecutorExternalReviseMustIncludeExternalReviewBackpressure;
static CONTRACT_4: ProvenanceLabelsMustDistinguishInternalVsExternal =
    ProvenanceLabelsMustDistinguishInternalVsExternal;
static CONTRACT_5: SourceObservationProvenancePresentForPlanner =
    SourceObservationProvenancePresentForPlanner;

static REGISTRY: &[&dyn BriefContract] = &[
    &CONTRACT_1,
    &CONTRACT_2,
    &CONTRACT_3,
    &CONTRACT_4,
    &CONTRACT_5,
];

/// Returns all registered brief contracts in declaration order.
pub fn registry() -> &'static [&'static dyn BriefContract] {
    REGISTRY
}

/// Runs every contract whose [`BriefContract::applies`] returns `true` for `ctx`,
/// returning the structured results in declaration order.
pub fn apply_all(ctx: &BriefContext<'_>) -> Vec<CheckResult> {
    REGISTRY
        .iter()
        .filter(|c| c.applies(ctx))
        .map(|c| c.evaluate(ctx))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
    use crate::flow::checks::CheckOutcome;
    use crate::render::{build_context, render_template, render_template_with_overlay};
    use crate::schema::Schema;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn tasks_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        Schema::from_yaml(yaml).unwrap()
    }

    fn executor_template() -> &'static str {
        BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .and_then(|(_, ts)| {
                ts.iter()
                    .find(|(p, _)| *p == "templates/executor-brief.md.tpl")
                    .map(|(_, c)| *c)
            })
            .expect("executor-brief template")
    }

    fn planner_template() -> &'static str {
        BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .and_then(|(_, ts)| {
                ts.iter()
                    .find(|(p, _)| *p == "templates/planner-brief.md.tpl")
                    .map(|(_, c)| *c)
            })
            .expect("planner-brief template")
    }

    /// Replaces each needle with "" in `rendered`, simulating a brief missing required content.
    fn strip_substrings(rendered: &str, needles: &[&str]) -> String {
        let mut result = rendered.to_string();
        for needle in needles {
            result = result.replace(needle, "");
        }
        result
    }

    fn empty_overlay() -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "external_review_backpressure".to_string(),
            Value::Null,
        );
        m
    }

    fn planner_revision_entry() -> EntryMap {
        let mut entry = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T123"));
        entry.insert("status".to_string(), json!("planning"));
        entry.insert("title".to_string(), json!("Revise Plan"));
        entry.insert("slug".to_string(), json!("revise-plan"));
        entry.insert("tier_hint".to_string(), json!("T3"));
        entry.insert("current_phase".to_string(), json!(0));
        entry.insert("current_cycle".to_string(), json!(0));
        entry.insert(
            "contract".to_string(),
            json!({"done_when": "Done", "scope_in": "In", "scope_out": "Out"}),
        );
        entry.insert(
            "plan".to_string(),
            json!({
                "objective": "UNIQUE_REJECTED_PLAN_OBJECTIVE",
                "phases": [{
                    "name": "Rejected Phase",
                    "objective": "Rejected objective",
                    "tasks": ["Rejected task"],
                    "acceptance_criteria": ["Rejected AC"],
                    "files": ["src/rejected.rs"],
                    "dependencies": []
                }]
            }),
        );
        entry.insert(
            "plan_review_log".to_string(),
            json!([{
                "gate": "NEEDS_WORK",
                "summary": "UNIQUE_REVIEW_BACKPRESSURE",
                "open_questions": ["What about invariant X?"]
            }]),
        );
        entry.insert("cycles".to_string(), json!([]));
        entry
    }

    fn executor_revise_entry() -> EntryMap {
        let mut entry = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T124"));
        entry.insert("status".to_string(), json!("executing"));
        entry.insert("title".to_string(), json!("Revise Code"));
        entry.insert("slug".to_string(), json!("revise-code"));
        entry.insert("tier_hint".to_string(), json!("T3"));
        entry.insert("current_phase".to_string(), json!(1));
        entry.insert("current_cycle".to_string(), json!(2));
        entry.insert(
            "contract".to_string(),
            json!({"done_when": "Done", "scope_in": "In", "scope_out": "Out"}),
        );
        entry.insert(
            "plan".to_string(),
            json!({
                "objective": "Plan",
                "phases": [{
                    "name": "Phase One",
                    "objective": "Do phase",
                    "tasks": ["Task"],
                    "acceptance_criteria": ["AC"],
                    "files": [],
                    "dependencies": []
                }]
            }),
        );
        entry.insert("plan_review_log".to_string(), json!([]));
        entry.insert(
            "cycles".to_string(),
            json!([
                {
                    "phase": 1,
                    "cycle": 1,
                    "executor": {
                        "summary": "UNIQUE_PRIOR_EXECUTOR_SUMMARY",
                        "commit": "abc123",
                        "files_changed": ["src/lib.rs"]
                    },
                    "review": {
                        "gate": "REVISE",
                        "summary": "UNIQUE_REVISE_SUMMARY",
                        "details": "UNIQUE_REVISE_DETAILS",
                        "critical": 0,
                        "major": 1,
                        "minor": 0
                    }
                },
                {
                    "phase": 1,
                    "cycle": 2,
                    "executor": {
                        "summary": "UNIQUE_CURRENT_EXECUTOR_SUMMARY",
                        "files_changed": ["src/lib.rs"]
                    },
                    "review": null
                }
            ]),
        );
        entry
    }

    fn er_overlay(findings: &str) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "external_review_backpressure".to_string(),
            json!({
                "display_id": "ER340",
                "runner": "codex",
                "verdict": "REVISE",
                "attempt": 6,
                "head_sha": "aa65090",
                "base_sha": "ed33d8d",
                "critical_count": 0,
                "major_count": 1,
                "minor_count": 0,
                "findings": findings,
            }),
        );
        m
    }

    // -----------------------------------------------------------------------
    // Task 1.10 — registry shape
    // -----------------------------------------------------------------------

    #[test]
    fn registry_shape_and_apply_all_round_trip() {
        let ids: Vec<&str> = registry().iter().map(|c| c.id()).collect();
        assert_eq!(
            ids,
            vec![
                PLANNER_REVISION_MUST_INCLUDE_REJECTED_PLAN_AND_REVIEWS,
                EXECUTOR_REVISE_MUST_INCLUDE_PRIOR_EXECUTOR_AND_CODE_REVIEW,
                EXECUTOR_EXTERNAL_REVISE_MUST_INCLUDE_EXTERNAL_REVIEW_BACKPRESSURE,
                PROVENANCE_LABELS_MUST_DISTINGUISH_INTERNAL_VS_EXTERNAL,
                SOURCE_OBSERVATION_PROVENANCE_PRESENT_FOR_PLANNER,
            ]
        );

        // Verify apply_all over a synthetic always-applies planner revision fixture.
        let schema = tasks_schema();
        let entry = planner_revision_entry();
        let tpl = planner_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        let mut overlay = empty_overlay();
        overlay.insert(
            "source_observations".to_string(),
            json!([{"display_id": "L999", "summary": "obs summary", "intent_contract": {}}]),
        );
        // linked_observations for Contract 5
        let mut entry5 = entry.clone();
        entry5.insert("linked_observations".to_string(), json!(["L999"]));
        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry5,
            overlay: &overlay,
            agent_role: "planner",
        };
        let results = apply_all(&ctx);
        // Contracts 1 and 5 apply for this planner+revision+linked_obs fixture.
        assert!(!results.is_empty(), "apply_all must return results when contracts apply");
        for r in &results {
            assert!(
                r.outcome == CheckOutcome::Pass || r.outcome == CheckOutcome::Fail,
                "each result must have Pass or Fail outcome"
            );
            assert!(!r.check_id.is_empty(), "check_id must be non-empty");
        }
    }

    // -----------------------------------------------------------------------
    // Task 1.11 — Contract 1 (planner revision)
    // -----------------------------------------------------------------------

    #[test]
    fn contract1_planner_revision_pass() {
        let schema = tasks_schema();
        let entry = planner_revision_entry();
        let tpl = planner_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        let overlay = empty_overlay();
        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "planner",
        };
        assert!(CONTRACT_1.applies(&ctx), "contract 1 must apply to planner+revision fixture");
        let result = CONTRACT_1.evaluate(&ctx);
        assert!(result.is_pass(), "contract 1 must pass on full planner revision brief: {:?}", result.reason);
    }

    #[test]
    fn contract1_planner_revision_fail_missing_objective_and_review() {
        let schema = tasks_schema();
        let entry = planner_revision_entry();
        let tpl = planner_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        // Strip the unique plan-objective and review-summary substrings.
        let stripped = strip_substrings(
            &rendered,
            &["UNIQUE_REJECTED_PLAN_OBJECTIVE", "UNIQUE_REVIEW_BACKPRESSURE", "NEEDS_WORK"],
        );
        let overlay = empty_overlay();
        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "planner",
        };
        let result = CONTRACT_1.evaluate(&ctx);
        assert_eq!(result.outcome, CheckOutcome::Fail, "must fail when objective+review stripped");
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("missing"),
            "reason must name missing artifact: {reason_str}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.12 — Contract 2 (executor in-cycle REVISE)
    // -----------------------------------------------------------------------

    #[test]
    fn contract2_executor_revise_pass() {
        let schema = tasks_schema();
        let entry = executor_revise_entry();
        let tpl = executor_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        let overlay = empty_overlay();
        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(CONTRACT_2.applies(&ctx), "contract 2 must apply to executor cycle-2 fixture");
        let result = CONTRACT_2.evaluate(&ctx);
        assert!(result.is_pass(), "contract 2 must pass on full executor revision brief: {:?}", result.reason);
    }

    #[test]
    fn contract2_executor_revise_fail_missing_prior_summary() {
        let schema = tasks_schema();
        let entry = executor_revise_entry();
        let tpl = executor_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        let stripped = strip_substrings(&rendered, &["UNIQUE_PRIOR_EXECUTOR_SUMMARY"]);
        let overlay = empty_overlay();
        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        let result = CONTRACT_2.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when prior executor summary stripped"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("prior executor summary"),
            "reason must name missing prior cycle artifact: {reason_str}"
        );
    }

    #[test]
    fn contract2_executor_revise_fail_missing_prior_commit() {
        let schema = tasks_schema();
        let entry = executor_revise_entry(); // cycle 1 executor has commit="abc123"
        let tpl = executor_template();
        let ctx_val = build_context(&schema, &entry);
        let rendered = render_template(tpl, &ctx_val).expect("render");
        // Verify the commit is actually rendered before stripping it.
        assert!(rendered.contains("abc123"), "fixture must render prior executor commit");
        // Strip the commit SHA; leave the summary and review text intact.
        let stripped = strip_substrings(&rendered, &["abc123"]);
        let overlay = empty_overlay();
        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        let result = CONTRACT_2.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when prior executor commit stripped"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("commit"),
            "reason must name missing prior commit: {reason_str}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.13 — Contract 3 (external review reuse + failing + null-applies)
    // -----------------------------------------------------------------------

    fn create_external_reviews_table(conn: &rusqlite::Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS external_reviews (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL,
                task_id TEXT,
                attempt INTEGER,
                runner TEXT,
                head_sha TEXT,
                base_sha TEXT,
                verdict TEXT,
                critical_count INTEGER,
                major_count INTEGER,
                minor_count INTEGER,
                findings TEXT
            );
            "#,
        )
        .unwrap();
    }

    #[test]
    fn contract3_same_function_reuse_and_pass() {
        use crate::handlers::brief::build_external_review_overlay;
        use rusqlite::Connection;

        let conn = Connection::open_in_memory().unwrap();
        create_external_reviews_table(&conn);

        // Insert PASS row (must be ignored), older REVISE, and newest REVISE.
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER001','closed','T107',1,'codex','aaa111','base000','PASS',0,0,0,'OLD_PASS')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER002','revise','T107',2,'codex','bbb222','base000','REVISE',0,1,0,'OLD_REVISE_TEXT')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER003','revise','T107',6,'codex','ccc333','base111','REVISE',0,1,0,\
             'NEWEST_REVISE_FINDINGS_KEEP_CLUSTER_KEYS_IN_ONE_REGISTRY_STRUCTURE_HERE')",
            [],
        )
        .unwrap();

        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T107"));
        entry.insert("current_cycle".to_string(), json!(1));
        entry.insert("cycles".to_string(), json!([]));

        // Call the REAL build_external_review_overlay (selection-logic reuse).
        let overlay = build_external_review_overlay(&conn, &entry).unwrap();

        // Contract 3's applies() must fire on exactly the row the overlay builder returns.
        let er_val = overlay.get("external_review_backpressure").unwrap();
        assert!(!er_val.is_null(), "overlay must have a non-null ER entry for T107");
        assert_eq!(er_val["display_id"], json!("ER003"), "must select newest REVISE row");

        // Render executor template with the real overlay.
        let schema = tasks_schema();
        let full_entry = {
            let mut m = entry.clone();
            m.insert("status".to_string(), json!("executing"));
            m.insert("title".to_string(), json!("Test Task"));
            m.insert("slug".to_string(), json!("test-task"));
            m.insert("current_phase".to_string(), json!(1));
            m.insert(
                "contract".to_string(),
                json!({"done_when": "Feature ships", "scope_in": "in", "scope_out": "out"}),
            );
            m.insert(
                "plan".to_string(),
                json!({"phases": [{"name": "P1", "objective": "do thing", "tasks": ["t1"], "acceptance_criteria": ["ac1"]}]}),
            );
            m
        };
        let ctx_val = build_context(&schema, &full_entry);
        let tpl = executor_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("executor render");

        let ctx = BriefContext {
            rendered: &rendered,
            entry: &full_entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(
            CONTRACT_3.applies(&ctx),
            "contract 3 applies() must fire when build_external_review_overlay returns non-null"
        );
        let result = CONTRACT_3.evaluate(&ctx);
        assert!(
            result.is_pass(),
            "contract 3 must pass when rendered brief contains all ER fields: {:?}",
            result.reason
        );
    }

    #[test]
    fn contract3_external_revise_fail_missing_findings() {
        let schema = tasks_schema();
        // Use findings text where the filename:line fragment IS in the first 32 chars
        // so stripping "cluster_keys.rs:27-33" removes the prefix the contract checks.
        // First 32 chars: "cluster_keys.rs:27-33 [major] du"
        let findings_text =
            "cluster_keys.rs:27-33 [major] duplicate patterns not consolidated in registry";
        let overlay = er_overlay(findings_text);

        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T107"));
        entry.insert("status".to_string(), json!("executing"));
        entry.insert("title".to_string(), json!("Test Task"));
        entry.insert("slug".to_string(), json!("test-task"));
        entry.insert("current_phase".to_string(), json!(1));
        entry.insert("current_cycle".to_string(), json!(1));
        entry.insert(
            "contract".to_string(),
            json!({"done_when": "Feature ships", "scope_in": "in", "scope_out": "out"}),
        );
        entry.insert(
            "plan".to_string(),
            json!({"phases": [{"name": "P1", "objective": "do thing", "tasks": ["t1"], "acceptance_criteria": ["ac1"]}]}),
        );
        entry.insert("cycles".to_string(), json!([]));

        let ctx_val = build_context(&schema, &entry);
        let tpl = executor_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("executor render");

        // Strip the findings filename:line fragment via the helper.
        // "cluster_keys.rs:27-33" is in the first 32 chars of findings_text, so
        // removing it from rendered causes the first-32-chars prefix check to fail.
        let stripped = strip_substrings(&rendered, &["cluster_keys.rs:27-33"]);
        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        let result = CONTRACT_3.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when findings filename:line fragment stripped"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("findings"),
            "reason must cite missing findings: {reason_str}"
        );
    }

    #[test]
    fn contract3_fail_count_value_not_just_word() {
        // Demonstrates that the contract checks actual numeric values ("1 major") rather than
        // the bare word "major" — a findings text containing "[major]" would fool a word-only
        // check but not a value-specific check.
        let schema = tasks_schema();
        // findings text deliberately contains the word "major" so bare-word check would pass.
        let findings_text = "[major] important invariant violated — contracts.rs:50";
        let overlay = er_overlay(findings_text); // critical_count=0, major_count=1, minor_count=0

        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T300"));
        entry.insert("status".to_string(), json!("executing"));
        entry.insert("title".to_string(), json!("Count Value Test"));
        entry.insert("slug".to_string(), json!("count-value-test"));
        entry.insert("current_phase".to_string(), json!(1));
        entry.insert("current_cycle".to_string(), json!(1));
        entry.insert(
            "contract".to_string(),
            json!({"done_when": "Done", "scope_in": "In", "scope_out": "Out"}),
        );
        entry.insert(
            "plan".to_string(),
            json!({"phases": [{"name": "P1", "objective": "obj", "tasks": ["t1"], "acceptance_criteria": ["ac1"]}]}),
        );
        entry.insert("cycles".to_string(), json!([]));

        let ctx_val = build_context(&schema, &entry);
        let tpl = executor_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("executor render");

        // Verify the count line is present before stripping.
        assert!(
            rendered.contains("0 critical, 1 major, 0 minor"),
            "template must render count line"
        );

        // Strip only the count line values (e.g. "0 critical, 1 major, 0 minor") — the word
        // "major" still appears in findings_text ("[major] important..."), so a bare-word
        // check would incorrectly pass while the numeric-value check correctly fails.
        let stripped = strip_substrings(&rendered, &["0 critical, 1 major, 0 minor"]);
        assert!(
            stripped.contains("[major]"),
            "stripped brief must still contain [major] from findings"
        );

        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        let result = CONTRACT_3.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when count line stripped even though word 'major' still appears in findings"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("major_count token") || reason_str.contains("critical_count token"),
            "reason must cite missing count token: {reason_str}"
        );
    }

    #[test]
    fn contract3_null_overlay_applies_false() {
        let mut overlay = HashMap::new();
        overlay.insert("external_review_backpressure".to_string(), Value::Null);
        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T999"));
        entry.insert("current_cycle".to_string(), json!(1));
        entry.insert("cycles".to_string(), json!([]));
        let ctx = BriefContext {
            rendered: "some brief text",
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(
            !CONTRACT_3.applies(&ctx),
            "contract 3 must NOT apply when external_review_backpressure is null"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.14 — Contract 4 (non-conflation, 5 tests)
    // -----------------------------------------------------------------------

    fn both_present_entry_and_overlay() -> (EntryMap, HashMap<String, Value>) {
        let entry = executor_revise_entry(); // has prior REVISE cycle
        let overlay = er_overlay(
            "[major] EXTERNAL_FINDING_X — file.rs:10\n\nMore detail about the external finding.",
        );
        (entry, overlay)
    }

    #[test]
    fn contract4_both_present_pass() {
        let schema = tasks_schema();
        let (entry, overlay) = both_present_entry_and_overlay();
        let ctx_val = build_context(&schema, &entry);
        let tpl = executor_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("executor render");

        // Verify the rendered brief has both sections in the right order.
        assert!(
            rendered.contains("## Revision Context for This Phase"),
            "must have internal header"
        );
        assert!(
            rendered.contains("## External Review Backpressure"),
            "must have external header"
        );

        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(CONTRACT_4.applies(&ctx));
        let result = CONTRACT_4.evaluate(&ctx);
        assert!(
            result.is_pass(),
            "contract 4 must pass when both bodies are in correct sections: {:?}",
            result.reason
        );
    }

    // Test B: external-only pass — the always-rendered internal header must NOT cause failure.
    #[test]
    fn contract4_external_only_pass() {
        let schema = tasks_schema();
        // cycles = [], current_cycle = 1 → no prior internal REVISE
        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), json!("T200"));
        entry.insert("status".to_string(), json!("executing"));
        entry.insert("title".to_string(), json!("External Only"));
        entry.insert("slug".to_string(), json!("external-only"));
        entry.insert("tier_hint".to_string(), json!("T3"));
        entry.insert("current_phase".to_string(), json!(1));
        entry.insert("current_cycle".to_string(), json!(1));
        entry.insert(
            "contract".to_string(),
            json!({"done_when": "Done", "scope_in": "In", "scope_out": "Out"}),
        );
        entry.insert(
            "plan".to_string(),
            json!({"objective": "Plan", "phases": [{"name": "P1", "objective": "o", "tasks": [], "acceptance_criteria": []}]}),
        );
        entry.insert("plan_review_log".to_string(), json!([]));
        entry.insert("cycles".to_string(), json!([]));

        let overlay = er_overlay("[major] EXTERNAL_FINDING_Y — other.rs:20\n\nDetails.");

        let ctx_val = build_context(&schema, &entry);
        let tpl = executor_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("executor render");

        // The always-rendered internal header IS present (but with no-prior-backpressure placeholder).
        assert!(
            rendered.contains("## Revision Context for This Phase"),
            "internal header always rendered"
        );
        assert!(
            rendered.contains("_No prior code-review backpressure for this phase._"),
            "placeholder must be present for cycle-1 executor"
        );
        assert!(
            rendered.contains("## External Review Backpressure"),
            "external header must be present"
        );

        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(CONTRACT_4.applies(&ctx), "contract 4 must apply when external is present");
        assert!(!has_prior_internal_revise(&entry), "must NOT have internal revise");
        let result = CONTRACT_4.evaluate(&ctx);
        assert!(
            result.is_pass(),
            "contract 4 must pass for external-only brief with always-rendered internal header: {:?}",
            result.reason
        );
    }

    // Test C: internal-only pass — no external header needed when overlay is null.
    #[test]
    fn contract4_internal_only_pass() {
        let schema = tasks_schema();
        let entry = executor_revise_entry();
        let overlay = empty_overlay(); // null external_review_backpressure

        let ctx_val = build_context(&schema, &entry);
        let tpl = executor_template();
        let rendered = render_template(tpl, &ctx_val).expect("executor render");

        assert!(
            !rendered.contains("## External Review Backpressure"),
            "external header must NOT be rendered when overlay null"
        );

        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(CONTRACT_4.applies(&ctx));
        let result = CONTRACT_4.evaluate(&ctx);
        assert!(
            result.is_pass(),
            "contract 4 must pass for internal-only brief: {:?}",
            result.reason
        );
    }

    // Test D: failing-fixture — external bleeds into internal section.
    #[test]
    fn contract4_fail_external_findings_in_internal_section() {
        // Hand-construct a malformed rendered string where external findings appear
        // in the internal section (before the external header).
        let external_findings = "[major] EXTERNAL_FINDING_X — file.rs:10\n\nMore detail.";
        let findings_prefix = &external_findings[..32];

        let malformed = format!(
            "# Brief\n\n\
             ## Revision Context for This Phase\n\
             Prior executor did stuff.\n\
             UNIQUE_REVISE_SUMMARY\n\
             {findings_prefix}sneaked_external_here\n\
             major 1 critical 0 minor 0\n\n\
             ## External Review Backpressure\n\
             Codex REVISE findings:\n\
             {external_findings}\n",
        );

        let entry = executor_revise_entry();
        // Make the findings prefix appear before the external header.
        let overlay = er_overlay(external_findings);

        let ctx = BriefContext {
            rendered: &malformed,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        // Verify has_prior_internal_revise so contract applies.
        assert!(has_prior_internal_revise(&entry));
        let result = CONTRACT_4.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when external findings appear in internal section"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("external findings appeared in internal section"),
            "reason must name the conflation: {reason_str}"
        );
    }

    // Test E: failing-fixture — external body present but label missing.
    #[test]
    fn contract4_fail_missing_external_label_when_body_present() {
        let external_findings = "[major] EXTERNAL_FINDING_Z — foo.rs:5\n\nSomething wrong.";
        let overlay = er_overlay(external_findings);

        // Hand-construct a rendered string that has the external findings body
        // but lacks the '## External Review Backpressure' header.
        let without_header = format!(
            "# Brief\n\n\
             ## Revision Context for This Phase\n\
             Prior stuff.\n\
             UNIQUE_REVISE_SUMMARY\n\
             major 1 critical 0 minor 0\n\n\
             You are in revision cycle 2 for this phase.\n\n\
             Codex REVISE:\n\
             {external_findings}\n",
        );

        let entry = executor_revise_entry();
        let ctx = BriefContext {
            rendered: &without_header,
            entry: &entry,
            overlay: &overlay,
            agent_role: "executor",
        };
        assert!(CONTRACT_4.applies(&ctx));
        let result = CONTRACT_4.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when external body present but label missing"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("missing external label"),
            "reason must name missing label: {reason_str}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.15 — Contract 5 (source observation provenance)
    // -----------------------------------------------------------------------

    fn planner_obs_entry() -> EntryMap {
        let mut e = planner_revision_entry();
        e.insert("linked_observations".to_string(), json!(["L504"]));
        // linked_observations is set; plan + plan_review_log already set
        e
    }

    fn source_obs_overlay(obs_id: &str) -> HashMap<String, Value> {
        let mut m = empty_overlay();
        m.insert(
            "source_observations".to_string(),
            json!([{"display_id": obs_id, "summary": "Brief summary for this obs", "intent_contract": {}}]),
        );
        m
    }

    #[test]
    fn contract5_source_observation_pass() {
        let schema = tasks_schema();
        let entry = planner_obs_entry();
        let overlay = source_obs_overlay("L504");
        let ctx_val = build_context(&schema, &entry);
        let tpl = planner_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("planner render");

        let ctx = BriefContext {
            rendered: &rendered,
            entry: &entry,
            overlay: &overlay,
            agent_role: "planner",
        };
        assert!(
            CONTRACT_5.applies(&ctx),
            "contract 5 must apply when planner has linked_observations + overlay"
        );
        let result = CONTRACT_5.evaluate(&ctx);
        assert!(
            result.is_pass(),
            "contract 5 must pass when source observation context rendered: {:?}",
            result.reason
        );
    }

    #[test]
    fn contract5_empty_source_observations_applies_false() {
        let entry = planner_obs_entry();
        let mut overlay = empty_overlay();
        overlay.insert("source_observations".to_string(), json!([]));
        let ctx = BriefContext {
            rendered: "some text",
            entry: &entry,
            overlay: &overlay,
            agent_role: "planner",
        };
        assert!(
            !CONTRACT_5.applies(&ctx),
            "contract 5 must NOT apply when source_observations overlay is empty"
        );
    }

    #[test]
    fn contract5_fail_missing_observation_display_id() {
        let schema = tasks_schema();
        let entry = planner_obs_entry();
        let overlay = source_obs_overlay("L504");
        let ctx_val = build_context(&schema, &entry);
        let tpl = planner_template();
        let rendered =
            render_template_with_overlay(tpl, &ctx_val, &overlay).expect("planner render");

        // Strip the observation display_id from the rendered brief.
        let stripped = strip_substrings(&rendered, &["L504"]);
        let ctx = BriefContext {
            rendered: &stripped,
            entry: &entry,
            overlay: &overlay,
            agent_role: "planner",
        };
        let result = CONTRACT_5.evaluate(&ctx);
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "must fail when observation display_id stripped from brief"
        );
        let reason_str = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason_str.contains("display_id"),
            "reason must cite missing display_id: {reason_str}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.16 — Immutability regression: template hashes stable
    // -----------------------------------------------------------------------

    fn hash_str(s: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    fn frozen_executor_entry() -> EntryMap {
        let mut m = std::collections::BTreeMap::new();
        m.insert("display_id".to_string(), json!("T001"));
        m.insert("status".to_string(), json!("executing"));
        m.insert("title".to_string(), json!("Frozen Executor Task"));
        m.insert("slug".to_string(), json!("frozen-executor"));
        m.insert("tier_hint".to_string(), json!("T3"));
        m.insert("current_phase".to_string(), json!(1));
        m.insert("current_cycle".to_string(), json!(1));
        m.insert(
            "contract".to_string(),
            json!({"done_when": "Done", "scope_in": "In", "scope_out": "Out"}),
        );
        m.insert(
            "plan".to_string(),
            json!({
                "objective": "Frozen Plan",
                "phases": [{
                    "name": "Frozen Phase",
                    "objective": "Frozen objective",
                    "tasks": ["Frozen task"],
                    "acceptance_criteria": ["Frozen AC"],
                    "files": ["src/frozen.rs"],
                    "dependencies": []
                }]
            }),
        );
        m.insert("plan_review_log".to_string(), json!([]));
        m.insert("cycles".to_string(), json!([]));
        m
    }

    fn frozen_planner_entry() -> EntryMap {
        // Mirror brief.rs:858-935 planner_revision_brief_includes_rejected_plan fixture.
        planner_revision_entry()
    }

    #[test]
    fn immutability_regression_brief_hashes_stable() {
        let schema = tasks_schema();

        // Executor brief hash.
        let exec_entry = frozen_executor_entry();
        let exec_ctx = build_context(&schema, &exec_entry);
        let exec_tpl = executor_template();
        let exec_rendered = render_template(exec_tpl, &exec_ctx).expect("executor render");
        let actual_exec_hash = hash_str(&exec_rendered);

        // Planner brief hash.
        let plan_entry = frozen_planner_entry();
        let plan_ctx = build_context(&schema, &plan_entry);
        let plan_tpl = planner_template();
        let plan_rendered = render_template(plan_tpl, &plan_ctx).expect("planner render");
        let actual_plan_hash = hash_str(&plan_rendered);

        // These constants were computed from the bundled templates at time of authoring (T109).
        // If this test fails, a template was changed — update these constants to the new values
        // shown in the assertion failure message.
        const EXPECTED_EXECUTOR_HASH: u64 = 0x3059c33b637615d1;
        const EXPECTED_PLANNER_HASH: u64 = 0x678a2fdf40bd8f36;

        assert_eq!(
            actual_exec_hash, EXPECTED_EXECUTOR_HASH,
            "executor brief hash changed (template modified?); actual: {:016x}",
            actual_exec_hash
        );
        assert_eq!(
            actual_plan_hash, EXPECTED_PLANNER_HASH,
            "planner brief hash changed (template modified?); actual: {:016x}",
            actual_plan_hash
        );
    }

    // -----------------------------------------------------------------------
    // Task 1.17 — Docstring doctrine
    // -----------------------------------------------------------------------

    #[test]
    fn docstring_contains_required_doctrine_phrases() {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/handlers/brief_contracts.rs"
        ));
        assert!(
            src.contains("How to add a new contract"),
            "docstring must contain 'How to add a new contract'"
        );
        assert!(src.contains("I026"), "docstring must contain 'I026'");
        assert!(
            src.contains("out of scope"),
            "docstring must contain 'out of scope'"
        );
    }
}
