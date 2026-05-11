//! ADR 0002 upstream primary-tuple derivation.
//!
//! Thin handler-facing wrapper over `flow::adr0002_projection`: callers pass
//! store/transition context plus typed references and receive the same primary
//! tuple emitted by the pure projection functions.

pub use crate::flow::adr0002_projection::{
    ArchReviewLifecycle, ArchReviewOutcome, ArchReviewProjection, ArchReviewReferences,
    ArchReviewRowInput, InletLifecycle, InletOutcome, InletProjection, InletReferences,
    InletWaitingKind, IntakeRowInput, ObsContractState, ObsLifecycle, ObsOutcome, ObsProjection,
    ObsReferences, ObsRowInput, ObsWaitingKind,
};

use crate::flow::adr0002_projection::{project_arch_review, project_intake, project_observation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamStore {
    Intake,
    Observations,
    ArchitectureReviews,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReferencesIn {
    pub routed_to_observation: Option<String>,
    pub routed_to_arch_review: Option<String>,
    pub produced_task_id: Option<String>,
    pub produced_artifact_kind: Option<String>,
    pub produced_artifact_id: Option<String>,
    pub duplicate_of: Option<String>,
    pub contract_state: Option<String>,
    pub pending_architecture_review: Option<bool>,
    pub clearable_by_ruling: Option<String>,
    pub open_architecture_review_id: Option<String>,
    pub resolution_kind: Option<String>,
    pub resolution: Option<String>,
    pub merge_target_id: Option<String>,
    pub resolved_by: Option<String>,
    pub task_id: Option<String>,
    pub addressed_by_commit_sha: Option<String>,
    pub superseded_by_id: Option<String>,
    pub source_observation: Option<String>,
    pub source_intake: Option<String>,
    pub linked_observation_ids: Vec<String>,
    pub supersedes: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimaryTuple {
    Intake(InletProjection),
    Observation(ObsProjection),
    ArchitectureReview(ArchReviewProjection),
}

pub fn derive(
    store: UpstreamStore,
    _verb: &str,
    _from_status: &str,
    to_status: &str,
    decision_or_verdict: Option<&str>,
    references_in: ReferencesIn,
) -> PrimaryTuple {
    match store {
        UpstreamStore::Intake => {
            let row = IntakeRowInput {
                display_id: "I000",
                status: to_status,
                decision: decision_or_verdict,
                routed_to_observation: references_in.routed_to_observation.as_deref(),
                routed_to_arch_review: references_in.routed_to_arch_review.as_deref(),
                produced_task_id: references_in.produced_task_id.as_deref(),
                produced_artifact_kind: references_in.produced_artifact_kind.as_deref(),
                produced_artifact_id: references_in.produced_artifact_id.as_deref(),
                duplicate_of: references_in.duplicate_of.as_deref(),
            };
            PrimaryTuple::Intake(project_intake(&row))
        }
        UpstreamStore::Observations => {
            let row = ObsRowInput {
                display_id: "L000",
                status: to_status,
                contract_state: references_in.contract_state.as_deref(),
                pending_architecture_review: references_in.pending_architecture_review,
                clearable_by_ruling: references_in.clearable_by_ruling.as_deref(),
                open_architecture_review_id: references_in.open_architecture_review_id.as_deref(),
                resolution_kind: references_in.resolution_kind.as_deref(),
                resolution: references_in.resolution.as_deref(),
                merge_target_id: references_in.merge_target_id.as_deref(),
                resolved_by: references_in.resolved_by.as_deref(),
                task_id: references_in.task_id.as_deref(),
                addressed_by_commit_sha: references_in.addressed_by_commit_sha.as_deref(),
                superseded_by_id: references_in.superseded_by_id.as_deref(),
            };
            PrimaryTuple::Observation(project_observation(&row, None))
        }
        UpstreamStore::ArchitectureReviews => {
            let linked: Vec<&str> = references_in
                .linked_observation_ids
                .iter()
                .map(String::as_str)
                .collect();
            let row = ArchReviewRowInput {
                display_id: "A000",
                status: to_status,
                verdict: decision_or_verdict,
                source_observation: references_in.source_observation.as_deref(),
                source_intake: references_in.source_intake.as_deref(),
                linked_observation_ids: linked,
                supersedes: references_in.supersedes.as_deref(),
                merge_target_id: references_in.merge_target_id.as_deref(),
                produced_task_id: references_in.produced_task_id.as_deref(),
                superseded_by_id: references_in.superseded_by_id.as_deref(),
                updated_at: references_in.updated_at.as_deref(),
            };
            PrimaryTuple::ArchitectureReview(project_arch_review(&row))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::adr0002_projection::{
        project_arch_review, project_intake, project_observation,
    };

    #[test]
    fn intake_schema_enum_union_matches_projection() {
        for status in ["draft", "triaging", "needs_info", "routed", "dropped"] {
            let decision = if status == "routed" {
                Some("normal_observation")
            } else {
                None
            };
            let refs = ReferencesIn {
                routed_to_observation: Some("L001".into()),
                ..ReferencesIn::default()
            };
            let got = derive(
                UpstreamStore::Intake,
                "route",
                "",
                status,
                decision,
                refs.clone(),
            );
            let row = IntakeRowInput {
                display_id: "I000",
                status,
                decision,
                routed_to_observation: refs.routed_to_observation.as_deref(),
                routed_to_arch_review: None,
                produced_task_id: None,
                produced_artifact_kind: None,
                produced_artifact_id: None,
                duplicate_of: None,
            };
            assert_eq!(got, PrimaryTuple::Intake(project_intake(&row)), "{status}");
        }
    }

    #[test]
    fn observations_schema_enum_union_matches_projection() {
        for status in [
            "open",
            "needs_investigation",
            "investigating",
            "investigated",
            "investigation_failed",
            "confirmed",
            "ready",
            "needs_info",
            "in_progress",
            "resolved",
            "wont_fix",
        ] {
            let refs = ReferencesIn {
                contract_state: Some("approved".into()),
                resolution_kind: (status == "resolved").then(|| "addressed_by_task".into()),
                resolution: (status == "resolved").then(|| "T001".into()),
                task_id: Some("T001".into()),
                ..ReferencesIn::default()
            };
            let got = derive(
                UpstreamStore::Observations,
                "x",
                "",
                status,
                None,
                refs.clone(),
            );
            let row = ObsRowInput {
                display_id: "L000",
                status,
                contract_state: refs.contract_state.as_deref(),
                pending_architecture_review: None,
                clearable_by_ruling: None,
                open_architecture_review_id: None,
                resolution_kind: refs.resolution_kind.as_deref(),
                resolution: refs.resolution.as_deref(),
                merge_target_id: None,
                resolved_by: None,
                task_id: refs.task_id.as_deref(),
                addressed_by_commit_sha: None,
                superseded_by_id: None,
            };
            assert_eq!(
                got,
                PrimaryTuple::Observation(project_observation(&row, None)),
                "{status}"
            );
        }
    }

    #[test]
    fn architecture_review_schema_enum_union_matches_projection() {
        for status in [
            "pending",
            "in_review",
            "awaiting_human_ratification",
            "verdict_issued",
            "withdrawn",
            "superseded",
        ] {
            let verdict = if status == "verdict_issued" {
                Some("allow_local_fix")
            } else {
                None
            };
            let refs = ReferencesIn {
                source_observation: Some("L001".into()),
                source_intake: Some("I001".into()),
                linked_observation_ids: vec!["L001".into(), "L002".into()],
                supersedes: Some("A001".into()),
                ..ReferencesIn::default()
            };
            let got = derive(
                UpstreamStore::ArchitectureReviews,
                "x",
                "",
                status,
                verdict,
                refs.clone(),
            );
            let linked: Vec<&str> = refs
                .linked_observation_ids
                .iter()
                .map(String::as_str)
                .collect();
            let row = ArchReviewRowInput {
                display_id: "A000",
                status,
                verdict,
                source_observation: refs.source_observation.as_deref(),
                source_intake: refs.source_intake.as_deref(),
                linked_observation_ids: linked,
                supersedes: refs.supersedes.as_deref(),
                merge_target_id: None,
                produced_task_id: None,
                superseded_by_id: None,
                updated_at: None,
            };
            assert_eq!(
                got,
                PrimaryTuple::ArchitectureReview(project_arch_review(&row)),
                "{status}"
            );
        }
    }
}
