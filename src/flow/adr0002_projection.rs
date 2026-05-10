//! ADR 0002 upstream read-model projection.
//!
//! Source of truth: `docs/adr/0002-inlet-triage-and-observation-routing.md`.
//! ADR 0002 §4 quotes the target inlet lifecycle as
//! `new | triaging | waiting | closed` and says routing results belong in
//! `outcome` plus typed references. §5 quotes the target observation lifecycle
//! as `candidate | ready | in_progress | closed`, contract state as
//! `none | draft | approved`, and waiting as an overlay. §6 quotes the target
//! architecture-review lifecycle as `pending | reviewing | waiting | closed`.
//! It is intentionally pure: current row fields in, owned projection out.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InletLifecycle {
    New,
    Triaging,
    Waiting,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InletWaitingKind {
    EvidenceNeeded,
    TriageCapacity,
    ExternalInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InletOutcome {
    RoutedToObservation,
    MarkedDuplicate,
    FastTracked,
    EscalatedToArchitectureReview,
    DroppedAsNoise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsLifecycle {
    Candidate,
    Ready,
    InProgress,
    Closed,
}

impl ObsLifecycle {
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsContractState {
    None,
    Draft,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsWaitingKind {
    InfoNeeded,
    ArchitectureReview,
    HumanRatification,
    LinkedTaskBlocked,
    ExternalDependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsOutcome {
    AddressedByTask,
    AddressedByCommit,
    ClosedAsDuplicate,
    ClosedWontFix,
    MergedWithCluster,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchReviewLifecycle {
    Pending,
    Reviewing,
    Waiting,
    Closed,
}

impl ArchReviewLifecycle {
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchReviewOutcome {
    LocalFixAllowed,
    ContractReframeRequired,
    MergedWithCluster,
    PrimitiveTaskCreated,
    PrimitiveTaskRequired,
    HumanDecisionRequired,
    DoctrineUpdateProposed,
    Withdrawn,
    Superseded,
}

impl InletLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Triaging => "triaging",
            Self::Waiting => "waiting",
            Self::Closed => "closed",
        }
    }
}
impl InletWaitingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EvidenceNeeded => "evidence_needed",
            Self::TriageCapacity => "triage_capacity",
            Self::ExternalInput => "external_input",
        }
    }
}
impl InletOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoutedToObservation => "routed_to_observation",
            Self::MarkedDuplicate => "marked_duplicate",
            Self::FastTracked => "fast_tracked",
            Self::EscalatedToArchitectureReview => "escalated_to_architecture_review",
            Self::DroppedAsNoise => "dropped_as_noise",
        }
    }
}
impl ObsLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Ready => "ready",
            Self::InProgress => "in_progress",
            Self::Closed => "closed",
        }
    }
}
impl ObsContractState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Draft => "draft",
            Self::Approved => "approved",
        }
    }
}
impl ObsWaitingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InfoNeeded => "info_needed",
            Self::ArchitectureReview => "architecture_review",
            Self::HumanRatification => "human_ratification",
            Self::LinkedTaskBlocked => "linked_task_blocked",
            Self::ExternalDependency => "external_dependency",
        }
    }
}
impl ObsOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AddressedByTask => "addressed_by_task",
            Self::AddressedByCommit => "addressed_by_commit",
            Self::ClosedAsDuplicate => "closed_as_duplicate",
            Self::ClosedWontFix => "closed_wont_fix",
            Self::MergedWithCluster => "merged_with_cluster",
            Self::Superseded => "superseded",
        }
    }
}
impl ArchReviewLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Reviewing => "reviewing",
            Self::Waiting => "waiting",
            Self::Closed => "closed",
        }
    }
}
impl ArchReviewOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalFixAllowed => "local_fix_allowed",
            Self::ContractReframeRequired => "contract_reframe_required",
            Self::MergedWithCluster => "merged_with_cluster",
            Self::PrimitiveTaskCreated => "primitive_task_created",
            Self::PrimitiveTaskRequired => "primitive_task_required",
            Self::HumanDecisionRequired => "human_decision_required",
            Self::DoctrineUpdateProposed => "doctrine_update_proposed",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InletReferences {
    pub produced_observation_id: Option<String>,
    pub produced_architecture_review_id: Option<String>,
    pub duplicate_of_id: Option<String>,
    /// ADR 0002 v1: no current intake schema column yet.
    pub produced_artifact_kind: Option<String>,
    /// ADR 0002 v1: no current intake schema column yet.
    pub produced_artifact_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObsReferences {
    pub linked_task_id: Option<String>,
    pub open_architecture_review_id: Option<String>,
    pub addressed_by_task_id: Option<String>,
    pub addressed_by_commit: Option<String>,
    pub duplicate_of_id: Option<String>,
    pub merged_into_id: Option<String>,
    // Intentionally absent: superseded_by_id has no typed observations column in v1.
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchReviewReferences {
    pub source_observation_id: Option<String>,
    pub source_intake_id: Option<String>,
    pub supersedes_id: Option<String>,
    pub merge_target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InletProjection {
    pub display_id: String,
    pub lifecycle: InletLifecycle,
    pub waiting: Option<InletWaitingKind>,
    pub outcome: Option<InletOutcome>,
    pub references: InletReferences,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsProjection {
    pub display_id: String,
    pub lifecycle: ObsLifecycle,
    pub contract_state: ObsContractState,
    pub waiting: Option<ObsWaitingKind>,
    pub outcome: Option<ObsOutcome>,
    pub references: ObsReferences,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchReviewProjection {
    pub display_id: String,
    pub lifecycle: ArchReviewLifecycle,
    pub outcome: Option<ArchReviewOutcome>,
    pub references: ArchReviewReferences,
}

pub struct IntakeRowInput<'a> {
    pub display_id: &'a str,
    pub status: &'a str,
    pub decision: Option<&'a str>,
    pub routed_to_observation: Option<&'a str>,
    pub routed_to_arch_review: Option<&'a str>,
    pub duplicate_of: Option<&'a str>,
}

pub struct ObsRowInput<'a> {
    pub display_id: &'a str,
    pub status: &'a str,
    pub contract_state: Option<&'a str>,
    pub pending_architecture_review: Option<bool>,
    /// Informational only; not consulted for overlay emission.
    pub clearable_by_ruling: Option<&'a str>,
    pub resolution_kind: Option<&'a str>,
    /// Raw resolution text. Parsing precedence: T### => task, L### => observation duplicate, otherwise commit sha.
    pub resolution: Option<&'a str>,
    pub merge_target_id: Option<&'a str>,
    pub resolved_by: Option<&'a str>,
    pub task_id: Option<&'a str>,
}

pub struct ArchReviewRowInput<'a> {
    pub display_id: &'a str,
    pub status: &'a str,
    pub verdict: Option<&'a str>,
    pub source_observation: Option<&'a str>,
    pub source_intake: Option<&'a str>,
    pub supersedes: Option<&'a str>,
    pub merge_target_id: Option<&'a str>,
    pub updated_at: Option<&'a str>,
}

pub fn project_intake(input: &IntakeRowInput<'_>) -> InletProjection {
    let lifecycle = match input.status {
        "draft" => InletLifecycle::New,
        "triaging" => InletLifecycle::Triaging,
        "needs_info" => InletLifecycle::Waiting,
        "routed" | "dropped" => InletLifecycle::Closed,
        other => panic!("unknown intake.status {other}"),
    };
    let waiting = (input.status == "needs_info").then_some(InletWaitingKind::EvidenceNeeded);
    let outcome = match input.status {
        "routed" => input.decision.and_then(inlet_outcome_from_decision),
        "dropped" => Some(InletOutcome::DroppedAsNoise),
        _ => None,
    };
    InletProjection {
        display_id: input.display_id.to_string(),
        lifecycle,
        waiting,
        outcome,
        references: InletReferences {
            produced_observation_id: input.routed_to_observation.map(str::to_string),
            produced_architecture_review_id: input.routed_to_arch_review.map(str::to_string),
            duplicate_of_id: input.duplicate_of.map(str::to_string),
            produced_artifact_kind: None,
            produced_artifact_id: None,
        },
    }
}

pub fn project_observation(
    obs: &ObsRowInput<'_>,
    open_arch_review: Option<&ArchReviewRowInput<'_>>,
) -> ObsProjection {
    let lifecycle = obs_lifecycle(obs.status);
    let contract_state = parse_contract_state(obs.contract_state);
    let waiting = if lifecycle.is_closed() {
        None
    } else if obs.pending_architecture_review == Some(true) && open_arch_review.is_some() {
        Some(ObsWaitingKind::ArchitectureReview)
    } else if obs.status == "needs_info" {
        Some(ObsWaitingKind::InfoNeeded)
    } else if matches!(
        contract_state,
        ObsContractState::None | ObsContractState::Draft
    ) {
        Some(ObsWaitingKind::HumanRatification)
    } else {
        None
    };
    let outcome = obs_outcome(obs.status, obs.resolution_kind);
    let mut references = ObsReferences {
        linked_task_id: obs.task_id.map(str::to_string),
        open_architecture_review_id: open_arch_review.map(|r| r.display_id.to_string()),
        ..ObsReferences::default()
    };
    populate_obs_resolution_references(&mut references, obs);
    ObsProjection {
        display_id: obs.display_id.to_string(),
        lifecycle,
        contract_state,
        waiting,
        outcome,
        references,
    }
}

pub fn project_arch_review(input: &ArchReviewRowInput<'_>) -> ArchReviewProjection {
    let lifecycle = match input.status {
        "pending" => ArchReviewLifecycle::Pending,
        "in_review" => ArchReviewLifecycle::Reviewing,
        "awaiting_human_ratification" => ArchReviewLifecycle::Waiting,
        "verdict_issued" | "withdrawn" | "superseded" => ArchReviewLifecycle::Closed,
        other => panic!("unknown architecture_reviews.status {other}"),
    };
    let outcome = match input.status {
        "verdict_issued" => input.verdict.map(arch_outcome_from_verdict),
        "withdrawn" => Some(ArchReviewOutcome::Withdrawn),
        "superseded" => Some(ArchReviewOutcome::Superseded),
        _ => None,
    };
    ArchReviewProjection {
        display_id: input.display_id.to_string(),
        lifecycle,
        outcome,
        references: ArchReviewReferences {
            source_observation_id: input.source_observation.map(str::to_string),
            source_intake_id: input.source_intake.map(str::to_string),
            supersedes_id: input.supersedes.map(str::to_string),
            merge_target_id: input.merge_target_id.map(str::to_string),
        },
    }
}

fn inlet_outcome_from_decision(decision: &str) -> Option<InletOutcome> {
    match decision {
        "duplicate" => Some(InletOutcome::MarkedDuplicate),
        "needs_info" => None,
        "fast_track" => Some(InletOutcome::FastTracked),
        "normal_observation" => Some(InletOutcome::RoutedToObservation),
        "arch_review_candidate" => Some(InletOutcome::EscalatedToArchitectureReview),
        "reject_noise" => Some(InletOutcome::DroppedAsNoise),
        other => panic!("unknown intake.decision {other}"),
    }
}

fn obs_lifecycle(status: &str) -> ObsLifecycle {
    match status {
        "open"
        | "needs_investigation"
        | "investigating"
        | "investigated"
        | "investigation_failed"
        | "confirmed"
        | "needs_info" => ObsLifecycle::Candidate,
        "ready" => ObsLifecycle::Ready,
        "in_progress" => ObsLifecycle::InProgress,
        "resolved" | "wont_fix" => ObsLifecycle::Closed,
        other => panic!("unknown observations.status {other}"),
    }
}

fn parse_contract_state(value: Option<&str>) -> ObsContractState {
    match value {
        None | Some("none") => ObsContractState::None,
        Some("draft") => ObsContractState::Draft,
        // Current observations schema uses `ready`; ADR 0002 names the same
        // ratified contract bucket `approved`.
        Some("approved" | "ready") => ObsContractState::Approved,
        Some(other) => panic!("unknown intent_contract.contract_state {other}"),
    }
}

fn obs_outcome(status: &str, resolution_kind: Option<&str>) -> Option<ObsOutcome> {
    match (status, resolution_kind) {
        ("wont_fix", _) => Some(ObsOutcome::ClosedWontFix),
        ("resolved", Some("addressed_by_task")) => Some(ObsOutcome::AddressedByTask),
        ("resolved", Some("addressed_by_commit")) => Some(ObsOutcome::AddressedByCommit),
        ("resolved", Some("addressed_by_observation")) => Some(ObsOutcome::ClosedAsDuplicate),
        ("resolved", Some("auto_resolved")) => Some(ObsOutcome::AddressedByTask),
        ("resolved", Some("merged_with_cluster")) => Some(ObsOutcome::MergedWithCluster),
        ("resolved", None) => None,
        ("resolved", Some(other)) => panic!("unknown observations.resolution_kind {other}"),
        _ => None,
    }
}

/// Parses legacy resolution references. Precedence is kind first, then text: addressed_by_task
/// accepts T###, addressed_by_observation accepts L###, addressed_by_commit treats other text as sha.
pub fn parse_resolution_reference(
    resolution_kind: Option<&str>,
    resolution: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    match (resolution_kind, resolution) {
        (Some("addressed_by_task") | Some("auto_resolved"), Some(r)) if r.starts_with('T') => {
            (Some(r.to_string()), None, None)
        }
        (Some("addressed_by_observation"), Some(r)) if r.starts_with('L') => {
            (None, None, Some(r.to_string()))
        }
        (Some("addressed_by_commit"), Some(r)) => (None, Some(r.to_string()), None),
        _ => (None, None, None),
    }
}

fn populate_obs_resolution_references(references: &mut ObsReferences, obs: &ObsRowInput<'_>) {
    let (task, commit, duplicate) = parse_resolution_reference(obs.resolution_kind, obs.resolution);
    if matches!(
        obs.resolution_kind,
        Some("addressed_by_task") | Some("auto_resolved")
    ) {
        references.addressed_by_task_id = task.or_else(|| {
            obs.resolved_by
                .filter(|v| v.starts_with('T'))
                .map(str::to_string)
        });
    }
    if obs.resolution_kind == Some("addressed_by_commit") {
        references.addressed_by_commit = commit.or_else(|| obs.resolved_by.map(str::to_string));
    }
    if obs.resolution_kind == Some("addressed_by_observation") {
        references.duplicate_of_id = duplicate.or_else(|| {
            obs.resolved_by
                .filter(|v| v.starts_with('L'))
                .map(str::to_string)
        });
    }
    if obs.resolution_kind == Some("merged_with_cluster") {
        references.merged_into_id = obs.merge_target_id.or(obs.resolved_by).map(str::to_string);
    }
}

fn arch_outcome_from_verdict(verdict: &str) -> ArchReviewOutcome {
    match verdict {
        "allow_local_fix" => ArchReviewOutcome::LocalFixAllowed,
        "reframe_contract" => ArchReviewOutcome::ContractReframeRequired,
        "merge_with_cluster" => ArchReviewOutcome::MergedWithCluster,
        "create_primitive_task" => ArchReviewOutcome::PrimitiveTaskCreated,
        "block_pending_fixes" => ArchReviewOutcome::PrimitiveTaskRequired,
        "request_human_arch_decision" => ArchReviewOutcome::HumanDecisionRequired,
        "propose_doctrine_update" => ArchReviewOutcome::DoctrineUpdateProposed,
        other => panic!("unknown architecture_reviews.verdict {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intake<'a>(status: &'a str, decision: Option<&'a str>) -> IntakeRowInput<'a> {
        IntakeRowInput {
            display_id: "I001",
            status,
            decision,
            routed_to_observation: None,
            routed_to_arch_review: None,
            duplicate_of: None,
        }
    }
    fn obs(status: &str) -> ObsRowInput<'_> {
        ObsRowInput {
            display_id: "L001",
            status,
            contract_state: Some("approved"),
            pending_architecture_review: None,
            clearable_by_ruling: None,
            resolution_kind: None,
            resolution: None,
            merge_target_id: None,
            resolved_by: None,
            task_id: None,
        }
    }
    fn arch<'a>(status: &'a str, verdict: Option<&'a str>) -> ArchReviewRowInput<'a> {
        ArchReviewRowInput {
            display_id: "A001",
            status,
            verdict,
            source_observation: None,
            source_intake: None,
            supersedes: None,
            merge_target_id: None,
            updated_at: None,
        }
    }

    macro_rules! lifecycle_test {
        ($name:ident, $status:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(project_intake(&intake($status, None)).lifecycle, $expected);
            }
        };
    }
    lifecycle_test!(inlet_draft_new, "draft", InletLifecycle::New);
    lifecycle_test!(
        inlet_triaging_triaging,
        "triaging",
        InletLifecycle::Triaging
    );
    lifecycle_test!(
        inlet_needs_info_waiting,
        "needs_info",
        InletLifecycle::Waiting
    );
    lifecycle_test!(inlet_routed_closed, "routed", InletLifecycle::Closed);
    lifecycle_test!(inlet_dropped_closed, "dropped", InletLifecycle::Closed);

    macro_rules! obs_lifecycle_test {
        ($name:ident, $status:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    project_observation(&obs($status), None).lifecycle,
                    $expected
                );
            }
        };
    }
    obs_lifecycle_test!(obs_open_candidate, "open", ObsLifecycle::Candidate);
    obs_lifecycle_test!(
        obs_needs_investigation_candidate,
        "needs_investigation",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(
        obs_investigating_candidate,
        "investigating",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(
        obs_investigated_candidate,
        "investigated",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(
        obs_investigation_failed_candidate,
        "investigation_failed",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(
        obs_confirmed_candidate,
        "confirmed",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(
        obs_needs_info_candidate,
        "needs_info",
        ObsLifecycle::Candidate
    );
    obs_lifecycle_test!(obs_ready_ready, "ready", ObsLifecycle::Ready);
    obs_lifecycle_test!(
        obs_in_progress_in_progress,
        "in_progress",
        ObsLifecycle::InProgress
    );
    obs_lifecycle_test!(obs_resolved_closed, "resolved", ObsLifecycle::Closed);
    obs_lifecycle_test!(obs_wont_fix_closed, "wont_fix", ObsLifecycle::Closed);

    macro_rules! arch_lifecycle_test {
        ($name:ident, $status:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    project_arch_review(&arch($status, None)).lifecycle,
                    $expected
                );
            }
        };
    }
    arch_lifecycle_test!(
        arch_pending_pending,
        "pending",
        ArchReviewLifecycle::Pending
    );
    arch_lifecycle_test!(
        arch_in_review_reviewing,
        "in_review",
        ArchReviewLifecycle::Reviewing
    );
    arch_lifecycle_test!(
        arch_awaiting_waiting,
        "awaiting_human_ratification",
        ArchReviewLifecycle::Waiting
    );
    arch_lifecycle_test!(
        arch_verdict_closed,
        "verdict_issued",
        ArchReviewLifecycle::Closed
    );
    arch_lifecycle_test!(
        arch_withdrawn_closed,
        "withdrawn",
        ArchReviewLifecycle::Closed
    );
    arch_lifecycle_test!(
        arch_superseded_closed,
        "superseded",
        ArchReviewLifecycle::Closed
    );

    macro_rules! inlet_outcome_test {
        ($name:ident, $decision:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    project_intake(&intake("routed", Some($decision))).outcome,
                    $expected
                );
            }
        };
    }
    inlet_outcome_test!(
        inlet_duplicate_marked_duplicate,
        "duplicate",
        Some(InletOutcome::MarkedDuplicate)
    );
    inlet_outcome_test!(
        inlet_fast_track_fast_tracked,
        "fast_track",
        Some(InletOutcome::FastTracked)
    );
    inlet_outcome_test!(
        inlet_normal_observation_routed,
        "normal_observation",
        Some(InletOutcome::RoutedToObservation)
    );
    inlet_outcome_test!(
        inlet_arch_candidate_escalated,
        "arch_review_candidate",
        Some(InletOutcome::EscalatedToArchitectureReview)
    );
    inlet_outcome_test!(
        inlet_reject_noise_dropped,
        "reject_noise",
        Some(InletOutcome::DroppedAsNoise)
    );

    macro_rules! arch_outcome_test {
        ($name:ident, $verdict:expr, $expected:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    project_arch_review(&arch("verdict_issued", Some($verdict))).outcome,
                    Some($expected)
                );
            }
        };
    }
    arch_outcome_test!(
        arch_allow_local_fix_outcome,
        "allow_local_fix",
        ArchReviewOutcome::LocalFixAllowed
    );
    arch_outcome_test!(
        arch_reframe_contract_outcome,
        "reframe_contract",
        ArchReviewOutcome::ContractReframeRequired
    );
    arch_outcome_test!(
        arch_merge_with_cluster_outcome,
        "merge_with_cluster",
        ArchReviewOutcome::MergedWithCluster
    );
    arch_outcome_test!(
        arch_create_primitive_task_outcome,
        "create_primitive_task",
        ArchReviewOutcome::PrimitiveTaskCreated
    );
    arch_outcome_test!(
        arch_block_pending_fixes_outcome,
        "block_pending_fixes",
        ArchReviewOutcome::PrimitiveTaskRequired
    );
    arch_outcome_test!(
        arch_request_human_outcome,
        "request_human_arch_decision",
        ArchReviewOutcome::HumanDecisionRequired
    );
    arch_outcome_test!(
        arch_doctrine_outcome,
        "propose_doctrine_update",
        ArchReviewOutcome::DoctrineUpdateProposed
    );
    #[test]
    fn arch_withdrawn_outcome() {
        assert_eq!(
            project_arch_review(&arch("withdrawn", None)).outcome,
            Some(ArchReviewOutcome::Withdrawn)
        );
    }
    #[test]
    fn arch_superseded_outcome() {
        assert_eq!(
            project_arch_review(&arch("superseded", None)).outcome,
            Some(ArchReviewOutcome::Superseded)
        );
    }

    #[test]
    fn waiting_d1_open_arch_review_emits_overlay_and_reference() {
        let mut o = obs("open");
        o.pending_architecture_review = Some(true);
        let a = arch("pending", None);
        let p = project_observation(&o, Some(&a));
        assert_eq!(p.waiting, Some(ObsWaitingKind::ArchitectureReview));
        assert_eq!(
            p.references.open_architecture_review_id.as_deref(),
            Some("A001")
        );
    }
    #[test]
    fn waiting_d2_missing_review_no_arch_overlay_falls_to_none() {
        let mut o = obs("open");
        o.pending_architecture_review = Some(true);
        o.contract_state = Some("approved");
        assert_eq!(project_observation(&o, None).waiting, None);
    }
    #[test]
    fn waiting_d3_missing_review_falls_to_info_needed() {
        let mut o = obs("needs_info");
        o.pending_architecture_review = Some(true);
        assert_eq!(
            project_observation(&o, None).waiting,
            Some(ObsWaitingKind::InfoNeeded)
        );
    }
    #[test]
    fn waiting_d4_missing_review_falls_to_human_ratification() {
        let mut o = obs("open");
        o.pending_architecture_review = Some(true);
        o.contract_state = Some("draft");
        assert_eq!(
            project_observation(&o, None).waiting,
            Some(ObsWaitingKind::HumanRatification)
        );
    }
    #[test]
    fn waiting_d5_arch_review_beats_needs_info() {
        let mut o = obs("needs_info");
        o.pending_architecture_review = Some(true);
        let a = arch("pending", None);
        assert_eq!(
            project_observation(&o, Some(&a)).waiting,
            Some(ObsWaitingKind::ArchitectureReview)
        );
    }
    #[test]
    fn waiting_d6_draft_candidate_human_ratification() {
        let mut o = obs("open");
        o.contract_state = Some("draft");
        assert_eq!(
            project_observation(&o, None).waiting,
            Some(ObsWaitingKind::HumanRatification)
        );
    }
    #[test]
    fn waiting_d7_none_candidate_human_ratification() {
        let mut o = obs("open");
        o.contract_state = None;
        assert_eq!(
            project_observation(&o, None).waiting,
            Some(ObsWaitingKind::HumanRatification)
        );
    }
    #[test]
    fn waiting_d8_approved_candidate_no_overlay() {
        let o = obs("open");
        assert_eq!(project_observation(&o, None).waiting, None);
    }
    #[test]
    fn waiting_ready_contract_state_maps_to_approved_no_overlay() {
        let mut o = obs("open");
        o.contract_state = Some("ready");
        let p = project_observation(&o, None);
        assert_eq!(p.contract_state, ObsContractState::Approved);
        assert_eq!(p.waiting, None);
    }
    #[test]
    fn waiting_d9_closed_resolved_suppresses_human_ratification() {
        let mut o = obs("resolved");
        o.contract_state = Some("draft");
        assert_eq!(project_observation(&o, None).waiting, None);
    }
    #[test]
    fn waiting_d10_closed_wont_fix_suppresses_open_gate() {
        let mut o = obs("wont_fix");
        o.pending_architecture_review = Some(true);
        let a = arch("pending", None);
        assert_eq!(project_observation(&o, Some(&a)).waiting, None);
    }

    #[test]
    fn refs_inlet_produced_observation() {
        let mut i = intake("routed", Some("normal_observation"));
        i.routed_to_observation = Some("L010");
        assert_eq!(
            project_intake(&i)
                .references
                .produced_observation_id
                .as_deref(),
            Some("L010")
        );
    }
    #[test]
    fn refs_inlet_duplicate() {
        let mut i = intake("routed", Some("duplicate"));
        i.duplicate_of = Some("L011");
        assert_eq!(
            project_intake(&i).references.duplicate_of_id.as_deref(),
            Some("L011")
        );
    }
    #[test]
    fn refs_inlet_escalated_arch_review() {
        let mut i = intake("routed", Some("arch_review_candidate"));
        i.routed_to_arch_review = Some("A010");
        assert_eq!(
            project_intake(&i)
                .references
                .produced_architecture_review_id
                .as_deref(),
            Some("A010")
        );
    }
    #[test]
    fn refs_obs_addressed_by_task_only() {
        let mut o = obs("resolved");
        o.resolution_kind = Some("addressed_by_task");
        o.resolution = Some("T123");
        let r = project_observation(&o, None).references;
        assert_eq!(r.addressed_by_task_id.as_deref(), Some("T123"));
        assert_eq!(r.addressed_by_commit, None);
        assert_eq!(r.duplicate_of_id, None);
    }
    #[test]
    fn refs_obs_addressed_by_commit_only() {
        let mut o = obs("resolved");
        o.resolution_kind = Some("addressed_by_commit");
        o.resolution = Some("abc1234");
        let r = project_observation(&o, None).references;
        assert_eq!(r.addressed_by_commit.as_deref(), Some("abc1234"));
        assert_eq!(r.addressed_by_task_id, None);
        assert_eq!(r.duplicate_of_id, None);
    }
    #[test]
    fn refs_obs_addressed_by_observation_only() {
        let mut o = obs("resolved");
        o.resolution_kind = Some("addressed_by_observation");
        o.resolution = Some("L045");
        let r = project_observation(&o, None).references;
        assert_eq!(r.duplicate_of_id.as_deref(), Some("L045"));
        assert_eq!(r.addressed_by_task_id, None);
        assert_eq!(r.addressed_by_commit, None);
    }
    #[test]
    fn refs_obs_merged_with_cluster_only() {
        let mut o = obs("resolved");
        o.resolution_kind = Some("merged_with_cluster");
        o.merge_target_id = Some("L010");
        let r = project_observation(&o, None).references;
        assert_eq!(r.merged_into_id.as_deref(), Some("L010"));
        assert_eq!(r.addressed_by_task_id, None);
        assert_eq!(r.addressed_by_commit, None);
        assert_eq!(r.duplicate_of_id, None);
    }
    #[test]
    fn refs_obs_linked_task() {
        let mut o = obs("in_progress");
        o.task_id = Some("T222");
        assert_eq!(
            project_observation(&o, None)
                .references
                .linked_task_id
                .as_deref(),
            Some("T222")
        );
    }
    #[test]
    fn refs_arch_sources() {
        let a = ArchReviewRowInput {
            display_id: "A002",
            status: "pending",
            verdict: None,
            source_observation: Some("L001"),
            source_intake: Some("I001"),
            supersedes: Some("A001"),
            merge_target_id: Some("L010"),
            updated_at: Some("now"),
        };
        let r = project_arch_review(&a).references;
        assert_eq!(r.source_observation_id.as_deref(), Some("L001"));
        assert_eq!(r.source_intake_id.as_deref(), Some("I001"));
        assert_eq!(r.supersedes_id.as_deref(), Some("A001"));
        assert_eq!(r.merge_target_id.as_deref(), Some("L010"));
    }

    #[test]
    fn parse_task_reference() {
        assert_eq!(
            parse_resolution_reference(Some("addressed_by_task"), Some("T123"))
                .0
                .as_deref(),
            Some("T123")
        );
    }
    #[test]
    fn parse_commit_reference() {
        assert_eq!(
            parse_resolution_reference(Some("addressed_by_commit"), Some("abc1234"))
                .1
                .as_deref(),
            Some("abc1234")
        );
    }
    #[test]
    fn parse_observation_reference() {
        assert_eq!(
            parse_resolution_reference(Some("addressed_by_observation"), Some("L045"))
                .2
                .as_deref(),
            Some("L045")
        );
    }
    #[test]
    fn label_helpers_are_stable_tokens() {
        assert_eq!(InletLifecycle::Waiting.as_str(), "waiting");
        assert_eq!(ObsOutcome::AddressedByTask.as_str(), "addressed_by_task");
        assert_eq!(
            ObsWaitingKind::ArchitectureReview.as_str(),
            "architecture_review"
        );
        assert_eq!(
            ObsWaitingKind::HumanRatification.as_str(),
            "human_ratification"
        );
    }
    #[test]
    fn is_closed_helpers() {
        assert!(ObsLifecycle::Closed.is_closed());
        assert!(!ObsLifecycle::Ready.is_closed());
        assert!(ArchReviewLifecycle::Closed.is_closed());
        assert!(!ArchReviewLifecycle::Pending.is_closed());
    }
    #[test]
    fn schema_enum_unions_do_not_panic() {
        for s in ["draft", "triaging", "needs_info", "routed", "dropped"] {
            let _ = project_intake(&intake(s, Some("needs_info")));
        }
        for d in [
            "duplicate",
            "needs_info",
            "fast_track",
            "normal_observation",
            "arch_review_candidate",
            "reject_noise",
        ] {
            let _ = project_intake(&intake("routed", Some(d)));
        }
        for s in [
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
            let _ = project_observation(&obs(s), None);
        }
        for c in ["draft", "ready"] {
            let mut o = obs("open");
            o.contract_state = Some(c);
            let _ = project_observation(&o, None);
        }
        for s in [
            "pending",
            "in_review",
            "awaiting_human_ratification",
            "verdict_issued",
            "withdrawn",
            "superseded",
        ] {
            let _ = project_arch_review(&arch(s, Some("allow_local_fix")));
        }
        for v in [
            "allow_local_fix",
            "reframe_contract",
            "merge_with_cluster",
            "create_primitive_task",
            "block_pending_fixes",
            "request_human_arch_decision",
            "propose_doctrine_update",
        ] {
            let _ = project_arch_review(&arch("verdict_issued", Some(v)));
        }
    }
}
