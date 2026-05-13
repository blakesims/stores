//! Section-grouping query layer.
//!
//! Reads tasks + observations from `.stores/db.sqlite` (read-only) and
//! classifies each row into the operator cockpit taxonomy exposed by
//! [`Section::label`]: active work, U1/U3 gates, held lanes, terminal rows,
//! priority/observation rows, intake lanes, and external-review rows.

use anyhow::Result;
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    TasksQueued,
    TasksActionableCurrentWork,
    ObsRatifiable,
    TasksAcceptU3,
    TasksIntegration,
    TasksIntegratedAwaitingPostLand,
    TasksIntegrationBlocked,
    TasksBlockedNeedsAction,
    TasksDeployRecovery,
    TasksNeedsTriage,
    IntakeHeld,
    TasksHeldAiReview,
    TasksHeldZombie,
    TasksRecentlyTerminal,
    ObsOpenNoContract,
    ObsOther,
    IntakeOpen,
    IntakeRouted,
    ExternalReviewLane,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::TasksQueued => "QUEUED",
            Section::TasksActionableCurrentWork => "ACTIVE",
            Section::ObsRatifiable => "RATIFY-U1",
            Section::TasksAcceptU3 => "AWAITING HUMAN ACCEPTANCE",
            Section::TasksIntegration => "INTEGRATION",
            Section::TasksIntegratedAwaitingPostLand => "INTEGRATED",
            Section::TasksIntegrationBlocked => "HELD-INTEGRATION",
            Section::TasksBlockedNeedsAction => "HELD-BLOCKED",
            Section::TasksDeployRecovery => "HELD-DEPLOY",
            Section::TasksNeedsTriage => "HELD-TRIAGE",
            Section::IntakeHeld => "HELD-INTAKE",
            Section::TasksHeldAiReview => "HELD-AI-REVIEW",
            Section::TasksHeldZombie => "HELD-ZOMBIE",
            Section::TasksRecentlyTerminal => "DONE",
            Section::ObsOpenNoContract => "PRIORITY",
            Section::ObsOther => "OBSERVATIONS",
            Section::IntakeOpen => "INTAKE-OPEN",
            Section::IntakeRouted => "INTAKE-ROUTED",
            Section::ExternalReviewLane => "EXTERNAL-REVIEW",
        }
    }

    pub const ALL: [Section; 19] = [
        Section::TasksQueued,
        Section::TasksActionableCurrentWork,
        Section::ObsRatifiable,
        Section::TasksAcceptU3,
        Section::TasksIntegration,
        Section::TasksIntegratedAwaitingPostLand,
        Section::TasksIntegrationBlocked,
        Section::TasksBlockedNeedsAction,
        Section::TasksDeployRecovery,
        Section::TasksNeedsTriage,
        Section::IntakeHeld,
        Section::TasksHeldAiReview,
        Section::TasksHeldZombie,
        Section::TasksRecentlyTerminal,
        Section::ObsOpenNoContract,
        Section::ObsOther,
        Section::IntakeOpen,
        Section::IntakeRouted,
        Section::ExternalReviewLane,
    ];
}

/// Top-level store cockpit lanes. Each `Row` maps to exactly one lane via
/// [`store_lane_for_row`]; engine-health is a system-state lane with no rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StoreLane {
    Intake,
    Observations,
    #[default]
    Tasks,
    ExternalReviews,
    EngineHealth,
}

impl StoreLane {
    pub const ALL: [StoreLane; 5] = [
        StoreLane::Intake,
        StoreLane::Observations,
        StoreLane::Tasks,
        StoreLane::ExternalReviews,
        StoreLane::EngineHealth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StoreLane::Intake => "INTAKE",
            StoreLane::Observations => "OBSERVATIONS",
            StoreLane::Tasks => "TASKS",
            StoreLane::ExternalReviews => "EXTERNAL REVIEWS",
            StoreLane::EngineHealth => "ENGINE",
        }
    }
}

/// Watch task classification knobs. Default hides stale terminal exhaust
/// older than 48 hours and caps recent terminal rows to the 5 newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchClassifyOptions {
    pub show_all_history: bool,
    pub recent_terminal_days: u64,
    pub recent_terminal_limit: usize,
}

impl Default for WatchClassifyOptions {
    fn default() -> Self {
        Self {
            show_all_history: false,
            recent_terminal_days: 2,
            recent_terminal_limit: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Row {
    Task(TaskRow),
    Obs(ObsRow),
    CollapsedObs(CollapsedObsRow),
    Review(ReviewRow),
    Intake(IntakeRow),
}

#[derive(Debug, Clone, Default)]
pub struct LiveRunEventSummary {
    pub ts: Option<String>,
    pub event_type: String,
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct LiveRunSummary {
    pub role: String,
    pub runner: Option<String>,
    pub status: Option<String>,
    pub updated_at: Option<String>,
    pub last_event_at: Option<String>,
    pub last_event_type: Option<String>,
    pub current_activity: Option<String>,
    pub marker_path: Option<String>,
    pub status_path: Option<String>,
    pub events_path: Option<String>,
    pub transcript_path: Option<String>,
    pub stderr_log_path: Option<String>,
    pub events: Vec<LiveRunEventSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanReviewGate {
    Ready,
    NeedsWork,
    NotReady,
    Unknown(String),
}

impl Default for PlanReviewGate {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl<'de> serde::Deserialize<'de> for PlanReviewGate {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
        let gate = match value {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        Ok(match gate.as_str() {
            "READY" => Self::Ready,
            "NEEDS_WORK" => Self::NeedsWork,
            "NOT_READY" => Self::NotReady,
            other => Self::Unknown(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleReviewGate {
    Pass,
    Revise,
    Fail,
    Unknown(String),
}

impl<'de> serde::Deserialize<'de> for CycleReviewGate {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <Value as serde::Deserialize>::deserialize(deserializer)?;
        let gate = match value {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        Ok(match gate.as_str() {
            "PASS" => Self::Pass,
            "REVISE" => Self::Revise,
            "FAIL" => Self::Fail,
            other => Self::Unknown(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct TaskPlanReviewEntry {
    #[serde(default)]
    pub gate: PlanReviewGate,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default, alias = "timestamp")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct TaskCycleActorRaw {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default, alias = "timestamp")]
    at: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct TaskCycleReviewRaw {
    #[serde(default)]
    gate: Option<CycleReviewGate>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default, alias = "timestamp")]
    at: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct TaskCycleEntryRaw {
    #[serde(default)]
    phase: i64,
    #[serde(default)]
    cycle: i64,
    #[serde(default)]
    executor: Option<TaskCycleActorRaw>,
    #[serde(default)]
    review: Option<TaskCycleReviewRaw>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskCycleEntry {
    pub phase: i64,
    pub cycle: i64,
    pub executor_summary: Option<String>,
    pub executor_at: Option<String>,
    pub review_gate: Option<CycleReviewGate>,
    pub review_summary: Option<String>,
    pub review_at: Option<String>,
}

impl From<TaskCycleEntryRaw> for TaskCycleEntry {
    fn from(raw: TaskCycleEntryRaw) -> Self {
        Self {
            phase: raw.phase,
            cycle: raw.cycle,
            executor_summary: raw
                .executor
                .as_ref()
                .and_then(|executor| executor.summary.clone()),
            executor_at: raw.executor.and_then(|executor| executor.at),
            review_gate: raw.review.as_ref().and_then(|review| review.gate.clone()),
            review_summary: raw
                .review
                .as_ref()
                .and_then(|review| review.summary.clone()),
            review_at: raw.review.and_then(|review| review.at),
        }
    }
}

impl<'de> serde::Deserialize<'de> for TaskCycleEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        TaskCycleEntryRaw::deserialize(deserializer).map(Self::from)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskRow {
    pub display_id: String,
    pub status: String,
    pub title: String,
    pub claimed_by: Option<String>,
    pub updated_at: String,
    pub tier_hint: Option<String>,
    pub linked_observations: Vec<String>,
    pub blocked_reason: Option<String>,
    pub blocked_reason_class: Option<String>,
    pub lifecycle: Option<String>,
    pub active_step: Option<String>,
    pub integration_step: Option<String>,
    pub blocked: Option<bool>,
    pub blocker_kind: Option<String>,
    pub current_phase: Option<i64>,
    pub current_cycle: Option<i64>,
    pub total_phases: Option<i64>,
    pub plan_source: Option<String>,
    pub contract_executive_intent: Option<String>,
    pub contract_done_when: Option<String>,
    pub contract_scope_in: Option<String>,
    pub contract_scope_out: Option<String>,
    pub plan_review_summaries: Vec<String>,
    pub cycle_summaries: Vec<String>,
    pub plan_review_entries: Vec<TaskPlanReviewEntry>,
    pub cycle_entries: Vec<TaskCycleEntry>,
    pub wrap_summaries: Vec<String>,
    pub branch: Option<String>,
    pub workspace_path: Option<String>,
    pub drive_pid: Option<i64>,
    pub drive_started_at: Option<String>,
    pub artifact_pointers: Vec<ArtifactPointer>,
    pub recent_events: Vec<RecentEvent>,
    /// T140 P5: per-row activation flag, used by `stores watch` to compute
    /// the operator-disposition glyph. `None` means the substrate did not
    /// expose the column (legacy schema); render falls back to "inactive".
    pub activation: Option<String>,
    pub human_acceptance_policy: Option<String>,
    pub task_review_policy: Option<String>,
    pub acceptance_decided_by: Option<String>,
    /// T140 P5: latest `accepted_at` recovered from `transition_history`,
    /// used by the disposition function to distinguish historical-legacy
    /// from deploy-ceremony-pending accepted rows.
    pub accepted_at: Option<String>,
    pub claimed_at: Option<String>,
    pub integration_attempts_count: usize,
    pub last_integration_outcome: Option<String>,
    pub live_run: Option<LiveRunSummary>,
}

#[derive(Debug, Clone, Default)]
pub struct ObsRow {
    pub display_id: String,
    pub status: String,
    pub priority: String,
    pub summary: String,
    pub updated_at: String,
    pub body: Option<String>,
    pub source: Option<String>,
    pub task_id: Option<String>,
    pub priority_rank: Option<i64>,
    pub lifecycle: Option<String>,
    pub waiting: Option<bool>,
    pub waiting_kind: Option<String>,
    pub outcome: Option<String>,
    pub pending_architecture_review: Option<bool>,
    pub open_architecture_review_id: Option<String>,
    pub superseded_by_id: Option<String>,
    /// `intent_contract.contract_state`, when present.
    pub contract_state: Option<String>,
    pub tier_hint: Option<String>,
    pub intent_objective: Option<String>,
    pub intent_type: Option<String>,
    pub intent_acceptance: Vec<String>,
    pub intent_in_scope: Vec<String>,
    pub intent_out_of_scope: Vec<String>,
    pub intent_known_solution: Option<String>,
    pub locked_by: Option<String>,
    pub locked_at: Option<String>,
    pub lock_reason: Option<String>,
    pub evidence_pointers: Vec<ArtifactPointer>,
    pub resolution_pointer: Option<String>,
    pub recent_events: Vec<RecentEvent>,
    pub investigation_failure_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CollapsedObsRow {
    pub section: Section,
    pub summary: String,
    pub count: usize,
    pub primary_display_id: String,
    pub display_ids: Vec<String>,
    pub representative: ObsRow,
}

#[derive(Debug, Clone, Default)]
pub struct ReviewRow {
    pub display_id: String,
    pub task_id: String,
    pub status: String,
    pub lifecycle: Option<String>,
    pub outcome: Option<String>,
    pub linked_observation_ids: Vec<String>,
    pub produced_task_id: Option<String>,
    pub runner: String,
    pub held_reason: Option<String>,
    pub next_retry_at: Option<String>,
    pub attempts: i64,
    pub verdict: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub log_path: Option<String>,
    pub transcript_path: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub critical_count: Option<i64>,
    pub major_count: Option<i64>,
    pub minor_count: Option<i64>,
    pub findings_count: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactPointer {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentEvent {
    pub store: Option<String>,
    pub display_id: String,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub verb: Option<String>,
    pub occurred_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct IntakeRow {
    pub display_id: String,
    pub status: String,
    pub summary: String,
    pub lifecycle: Option<String>,
    pub waiting_kind: Option<String>,
    pub outcome: Option<String>,
    pub body: Option<String>,
    pub updated_at: String,
    pub source_task: Option<String>,
    pub source_agent: Option<String>,
    pub priority: Option<String>,
    pub risk_flags: Vec<String>,
    pub cluster_key: Option<String>,
    pub decision: Option<String>,
    pub missing_info_question: Option<String>,
    pub held_reason: Option<String>,
    pub next_action: Option<String>,
    pub routed_to_observation: Option<String>,
    pub routed_to_arch_review: Option<String>,
    pub duplicate_of: Option<String>,
    pub duplicate_of_id: Option<String>,
    pub produced_observation_id: Option<String>,
    pub produced_architecture_review_id: Option<String>,
    pub produced_task_id: Option<String>,
    pub produced_artifact_kind: Option<String>,
    pub produced_artifact_id: Option<String>,
    pub evidence_pointer: Option<String>,
    pub recent_events: Vec<RecentEvent>,
    pub captured_at: Option<String>,
    pub recon_round: Option<i64>,
    pub decision_rationale: Option<String>,
    pub decision_confidence: Option<String>,
    pub decision_tier_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalReviewState {
    Unavailable {
        reason: String,
    },
    Available {
        rows: usize,
        lane: Option<String>,
        status: Option<String>,
    },
}

impl Default for ExternalReviewState {
    fn default() -> Self {
        Self::Unavailable {
            reason: "external review: unavailable / not installed".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemHealth {
    pub unfinished_dispatch_locks: usize,
    pub oldest_claimed_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonStartRow {
    pub pid: i64,
    pub started_at: Option<String>,
    pub binary_version: Option<String>,
    pub git_sha: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchLockRow {
    pub display_id: String,
    pub agent_name: Option<String>,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub liveness_label: String,
    pub attempts: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRunsRoleAggregate {
    pub role: String,
    pub count: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineDetail {
    pub recent_daemon_starts: Vec<DaemonStartRow>,
    pub unfinished_lock_rows: Vec<DispatchLockRow>,
    pub recent_agent_runs_by_role: Vec<AgentRunsRoleAggregate>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CockpitModel {
    pub execution: usize,
    pub review: usize,
    pub accept: usize,
    pub held: usize,
    pub active: usize,
    pub priority: usize,
    pub external_review: ExternalReviewState,
}

pub fn cockpit_model(rows: &[Row], external_review: ExternalReviewState) -> CockpitModel {
    let mut model = CockpitModel {
        external_review,
        ..Default::default()
    };
    let visibility_ctx = task_status_by_id(rows);
    for row in rows {
        if row_visibility_class(row, &visibility_ctx) == VisibilityClass::HistoricalNoise {
            continue;
        }
        match row {
            Row::Task(t) => {
                if task_lifecycle(t) == "active" && task_active_step(t) == "coding" {
                    model.execution += 1;
                }
                if task_lifecycle(t) == "active"
                    && matches!(
                        task_active_step(t),
                        "planning_review" | "coding_review" | "wrapping"
                    )
                {
                    model.review += 1;
                }
                if task_is_terminal_with_compat(t) {
                    model.accept += 1;
                }
                if task_is_blocked(t) {
                    model.held += 1;
                }
                if !task_is_terminal_with_compat(t)
                    && !task_is_blocked(t)
                    && task_is_in_flight_primary(t)
                {
                    model.active += 1;
                }
                if is_priority_task(t) {
                    model.priority += 1;
                }
            }
            Row::Obs(o) => apply_obs_to_model(o, 1, &mut model),
            Row::CollapsedObs(c) => apply_obs_to_model(&c.representative, c.count, &mut model),
            Row::Intake(i) => {
                if i.lifecycle.as_deref() == Some("waiting")
                    || matches!(
                        i.waiting_kind.as_deref(),
                        Some("evidence_needed" | "external_input" | "triage_capacity")
                    )
                {
                    model.held += 1;
                }
                if is_priority_text(i.priority.as_deref().unwrap_or("")) || !i.risk_flags.is_empty()
                {
                    model.priority += 1;
                }
                if i.lifecycle.as_deref() != Some("closed") {
                    model.active += 1;
                }
            }
            Row::Review(r) => {
                if r.lifecycle.as_deref() != Some("closed") {
                    model.active += 1;
                }
                if r.lifecycle.as_deref() == Some("waiting")
                    || r.held_reason.as_deref().is_some_and(|s| !s.is_empty())
                {
                    model.held += 1;
                }
            }
        }
    }
    model
}

fn apply_obs_to_model(o: &ObsRow, count: usize, model: &mut CockpitModel) {
    if is_priority_text(&o.priority) || o.priority_rank.map(|r| r <= 1).unwrap_or(false) {
        model.priority += count;
    }
    if o.lock_reason
        .as_deref()
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        model.held += count;
    }
    if o.lifecycle.as_deref() != Some("closed") {
        model.active += count;
    }
}

fn is_priority_task(t: &TaskRow) -> bool {
    t.tier_hint
        .as_deref()
        .map(is_priority_text)
        .unwrap_or(false)
}

/// Per-status counts the cockpit renders for the intake lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntakeFlow {
    pub new: usize,
    pub triaging: usize,
    pub waiting: usize,
    pub closed: usize,
    pub waiting_kinds: BTreeMap<String, usize>,
    pub outcomes: BTreeMap<String, usize>,
}

/// Per-status counts the cockpit renders for the observations lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObsFlow {
    pub candidate: usize,
    pub ready: usize,
    pub in_progress: usize,
    pub closed: usize,
    pub errors: usize,
    pub waiting_kinds: BTreeMap<String, usize>,
    pub outcomes: BTreeMap<String, usize>,
}

/// Per-status counts the cockpit renders for the tasks lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TasksFlow {
    pub queued: usize,
    pub work: usize,
    pub gate: usize,
    pub wait: usize,
    pub fail: usize,
    pub recently_terminal: usize,
}

/// Per-status counts the cockpit renders for the external-reviews lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewsFlow {
    pub pending: usize,
    pub running: usize,
    pub passed: usize,
    pub revise: usize,
    pub wait: usize,
    pub tooling_held: usize,
}

/// System-state counts the cockpit renders for the engine-health lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineFlow {
    pub daemon_live: bool,
    pub unfinished_locks: usize,
    pub oldest_lock_age_secs: Option<i64>,
    pub agent_runs_recent: usize,
}

/// Per-lane store-flow model rendered at the top of the cockpit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreFlowModel {
    pub intake: IntakeFlow,
    pub observations: ObsFlow,
    pub tasks: TasksFlow,
    pub external_reviews: ReviewsFlow,
    pub engine: EngineFlow,
}

/// Build a [`StoreFlowModel`] by walking `rows` once and folding per-lane
/// status counts; engine flow is filled from `system_health` + `daemon`.
/// `external_review` is accepted to keep the call-site shape aligned with
/// the cockpit data path even though external-review counts come from `rows`.
pub fn store_flow_model(
    rows: &[Row],
    system_health: &SystemHealth,
    daemon: &super::daemon::Liveness,
    external_review: &ExternalReviewState,
) -> StoreFlowModel {
    store_flow_model_at(rows, system_health, daemon, external_review, now_epoch())
}

fn store_flow_model_at(
    rows: &[Row],
    system_health: &SystemHealth,
    daemon: &super::daemon::Liveness,
    _external_review: &ExternalReviewState,
    now: i64,
) -> StoreFlowModel {
    let mut model = StoreFlowModel::default();
    for row in rows {
        match row {
            Row::Intake(i) => apply_intake_to_flow(i, 1, &mut model.intake),
            Row::Obs(o) => apply_obs_to_flow(o, 1, &mut model.observations),
            Row::CollapsedObs(c) => {
                apply_obs_to_flow(&c.representative, c.count, &mut model.observations)
            }
            Row::Task(t) => apply_task_to_flow(t, &mut model.tasks),
            Row::Review(r) => apply_review_to_flow(&r.status, &mut model.external_reviews),
        }
    }
    model.engine = EngineFlow {
        daemon_live: matches!(daemon, super::daemon::Liveness::Live { .. }),
        unfinished_locks: system_health.unfinished_dispatch_locks,
        oldest_lock_age_secs: system_health
            .oldest_claimed_at_epoch
            .map(|epoch| (now - epoch).max(0)),
        agent_runs_recent: 0,
    };
    model
}

fn apply_intake_to_flow(row: &IntakeRow, count: usize, flow: &mut IntakeFlow) {
    match intake_lifecycle(row) {
        "new" => flow.new += count,
        "triaging" => flow.triaging += count,
        "waiting" => flow.waiting += count,
        "closed" => flow.closed += count,
        _ => {}
    }
    if let Some(kind) = row.waiting_kind.as_deref().filter(|s| !s.is_empty()) {
        *flow.waiting_kinds.entry(kind.to_string()).or_default() += count;
    }
    if let Some(outcome) = row.outcome.as_deref().filter(|s| !s.is_empty()) {
        *flow.outcomes.entry(outcome.to_string()).or_default() += count;
    }
}

fn apply_obs_to_flow(row: &ObsRow, count: usize, flow: &mut ObsFlow) {
    // Top-card observation slots are mutually exclusive and route through the
    // same projection buckets the focused observation list renders.
    match super::semantics::observation_watch_projection(row).slot {
        super::semantics::WatchSlotId::Front => flow.candidate += count,
        super::semantics::WatchSlotId::Work => flow.in_progress += count,
        super::semantics::WatchSlotId::Gate => flow.ready += count,
        super::semantics::WatchSlotId::Exit => flow.closed += count,
        super::semantics::WatchSlotId::Wait => {
            *flow
                .waiting_kinds
                .entry(obs_waiting_kind(row).unwrap_or("waiting").to_string())
                .or_default() += count;
        }
        super::semantics::WatchSlotId::Fault => flow.errors += count,
    }
    if let Some(outcome) = row.outcome.as_deref().filter(|s| !s.is_empty()) {
        *flow.outcomes.entry(outcome.to_string()).or_default() += count;
    }
}

fn obs_waiting_kind(row: &ObsRow) -> Option<&str> {
    row.waiting_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "human_ratification")
}

pub fn intake_lifecycle(row: &IntakeRow) -> &str {
    if let Some(v) = row
        .lifecycle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    // ADR 0002 compatibility-only (T148): legacy intake.status fallback for
    // old test schemas and rows not yet carrying primary lifecycle columns.
    match row.status.as_str() {
        "draft" => "new",
        "triaging" => "triaging",
        "needs_info" => "waiting",
        "routed" | "dropped" => "closed",
        _ => "new",
    }
}

pub fn obs_lifecycle(row: &ObsRow) -> &str {
    if let Some(v) = row
        .lifecycle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    // ADR 0002 compatibility-only (T148): legacy observations.status fallback
    // for old test schemas and migrated rows absent primary lifecycle fields.
    match row.status.as_str() {
        "open" | "needs_info" => "candidate",
        "confirmed" | "ready" => "ready",
        "investigating" | "confirming" | "claiming" | "in_progress" => "in_progress",
        "resolved" | "wont_fix" | "rejected" | "investigation_failed" => "closed",
        _ => "candidate",
    }
}

fn apply_task_to_flow(t: &TaskRow, flow: &mut TasksFlow) {
    match super::semantics::task_watch_projection(t).slot {
        super::semantics::WatchSlotId::Front => flow.queued += 1,
        super::semantics::WatchSlotId::Work => flow.work += 1,
        super::semantics::WatchSlotId::Gate => flow.gate += 1,
        super::semantics::WatchSlotId::Exit => flow.recently_terminal += 1,
        super::semantics::WatchSlotId::Wait => flow.wait += 1,
        super::semantics::WatchSlotId::Fault => flow.fail += 1,
    }
}

fn apply_review_to_flow(status: &str, flow: &mut ReviewsFlow) {
    match status {
        "pending" => flow.pending += 1,
        "running" => flow.running += 1,
        "passed" => flow.passed += 1,
        "revise" => flow.revise += 1,
        "tooling_held" => flow.tooling_held += 1,
        _ => {}
    }
}

fn is_priority_text(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "high" | "urgent" | "p0" | "p1" | "t1"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VisibilityClass {
    ActionableRecovery,
    HistoricalNoise,
    NeedsTriage,
}

impl VisibilityClass {
    pub fn as_str(self) -> &'static str {
        match self {
            VisibilityClass::ActionableRecovery => "actionable_recovery",
            VisibilityClass::HistoricalNoise => "historical_noise",
            VisibilityClass::NeedsTriage => "needs_triage",
        }
    }
}

pub fn row_visibility_class(
    row: &Row,
    task_status_by_id: &HashMap<String, String>,
) -> VisibilityClass {
    match row {
        Row::Task(t) => task_visibility_class(t),
        Row::Obs(o) => obs_visibility_class(o, task_status_by_id),
        Row::CollapsedObs(c) => obs_visibility_class(&c.representative, task_status_by_id),
        Row::Review(_) => VisibilityClass::ActionableRecovery,
        Row::Intake(_) => VisibilityClass::ActionableRecovery,
    }
}

pub fn task_visibility_class(t: &TaskRow) -> VisibilityClass {
    let reason = t
        .blocked_reason
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let reason_class = blocked_reason_class(t.blocked_reason.as_deref());
    if is_silent_zombie_reason(&reason) {
        return VisibilityClass::ActionableRecovery;
    }
    if reason.contains("accept_installed_inert") {
        return VisibilityClass::HistoricalNoise;
    }
    if task_is_blocked(t) {
        if t.status == "deploy_blocked" {
            return if is_recoverable_deploy_reason(&reason) {
                VisibilityClass::ActionableRecovery
            } else {
                VisibilityClass::NeedsTriage
            };
        }
        return match reason_class {
            "rate_limit" | "retry" | "dependency" | "user" | "deploy" => {
                VisibilityClass::ActionableRecovery
            }
            "unknown" => VisibilityClass::NeedsTriage,
            _ => VisibilityClass::ActionableRecovery,
        };
    }
    if task_is_in_flight_primary(t) {
        return VisibilityClass::ActionableRecovery;
    }
    VisibilityClass::ActionableRecovery
}

fn is_recoverable_deploy_reason(reason: &str) -> bool {
    reason.contains("retry-deploy-recoverable") || reason.contains("recoverable")
}

pub fn obs_visibility_class(
    o: &ObsRow,
    task_status_by_id: &HashMap<String, String>,
) -> VisibilityClass {
    if matches!(o.status.as_str(), "resolved" | "wont_fix") {
        return VisibilityClass::HistoricalNoise;
    }
    if is_silent_zombie_reason(o.investigation_failure_reason.as_deref().unwrap_or("")) {
        return VisibilityClass::ActionableRecovery;
    }
    let lower = o.summary.to_ascii_lowercase();
    if lower.starts_with("deploy-blocked:") {
        if let Some(task_id) = extract_task_id(&o.summary) {
            if task_status_by_id
                .get(&task_id)
                .map(|s| is_terminal_task_status(s))
                .unwrap_or(false)
            {
                return VisibilityClass::HistoricalNoise;
            }
        }
        if lower.contains("retry-deploy-recoverable") || lower.contains("recoverable") {
            return VisibilityClass::ActionableRecovery;
        }
        return VisibilityClass::NeedsTriage;
    }
    VisibilityClass::ActionableRecovery
}

fn is_silent_zombie_reason(reason: &str) -> bool {
    let reason = reason.trim().to_ascii_lowercase();
    matches!(
        reason.as_str(),
        "silent_zombie"
            | "drive_failed:silent_zombie"
            | "drive_failed:silent_zombie_pid_dead"
            | "drive_failed:pid_never_recorded"
    )
}

fn extract_task_id(s: &str) -> Option<String> {
    for word in s.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.len() >= 2 && word.starts_with('T') && word[1..].chars().all(|c| c.is_ascii_digit())
        {
            return Some(word.to_string());
        }
    }
    None
}

pub fn is_in_flight_task_status(s: &str) -> bool {
    matches!(
        s,
        "executing"
            | "plan_review"
            | "code_review"
            | "in_review"
            | "planning"
            | "ready"
            | "integration_queued"
            | "integrating"
            | "integrated"
    )
}

pub fn surface_counts(rows: &[Row], _show_all_history: bool) -> ((usize, usize), (usize, usize)) {
    let ctx = task_status_by_id(rows);
    let mut task = (0, 0);
    let mut obs = (0, 0);
    for row in rows {
        let class = row_visibility_class(row, &ctx);
        match row {
            Row::Task(_) => {
                task.1 += 1;
                if class == VisibilityClass::ActionableRecovery {
                    task.0 += 1;
                }
            }
            Row::Obs(_) => {
                obs.1 += 1;
                if class == VisibilityClass::ActionableRecovery {
                    obs.0 += 1;
                }
            }
            Row::CollapsedObs(c) => {
                obs.1 += c.count;
                if class == VisibilityClass::ActionableRecovery {
                    obs.0 += c.count;
                }
            }
            Row::Review(_) => {}
            Row::Intake(_) => {}
        }
    }
    (task, obs)
}

fn task_status_by_id(rows: &[Row]) -> HashMap<String, String> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Task(t) => Some((
                t.display_id.clone(),
                if task_is_terminal_with_compat(t) {
                    "done".to_string()
                } else {
                    task_lifecycle(t).to_string()
                },
            )),
            Row::Obs(_) | Row::CollapsedObs(_) | Row::Review(_) | Row::Intake(_) => None,
        })
        .collect()
}

impl Row {
    pub fn display_id(&self) -> &str {
        match self {
            Row::Task(t) => &t.display_id,
            Row::Obs(o) => &o.display_id,
            Row::CollapsedObs(c) => &c.primary_display_id,
            Row::Review(r) => &r.display_id,
            Row::Intake(i) => &i.display_id,
        }
    }

    pub fn title_or_summary(&self) -> &str {
        match self {
            Row::Task(t) => &t.title,
            Row::Obs(o) => &o.summary,
            Row::CollapsedObs(c) => &c.summary,
            Row::Review(r) => &r.task_id,
            Row::Intake(i) => &i.summary,
        }
    }
}

const LIVE_ACTIVITY_LIMIT: usize = 5;
const LIVE_EVENTS_READ_BYTES: u64 = 32 * 1024;
const LIVE_EVENTS_SCAN_BYTES: u64 = 8 * 1024 * 1024;
const LIVE_MARKER_STATUS_READ_BYTES: u64 = 8 * 1024;
const LIVE_TEXT_LIMIT: usize = 100;

fn load_live_run_summary(display_id: &str, workspace_path: Option<&str>) -> Option<LiveRunSummary> {
    let workspace = workspace_path?.trim();
    if workspace.is_empty() {
        return None;
    }
    let stores_dir = Path::new(workspace).join(".stores");
    let runs_dir = stores_dir.join("runs");
    let prefix = format!("current-{display_id}-");
    let entries = std::fs::read_dir(&runs_dir).ok()?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        let Some(marker) = read_small_json::<crate::cli::runs::CurrentRunMarker>(
            &path,
            LIVE_MARKER_STATUS_READ_BYTES,
        ) else {
            continue;
        };
        if marker.status.as_deref() != Some("running") {
            continue;
        }
        candidates.push(crate::cli::runs::CurrentRun {
            marker_path: path,
            marker,
        });
    }
    candidates.sort_by(|a, b| {
        let au = a.marker.updated_at.as_deref().unwrap_or("");
        let bu = b.marker.updated_at.as_deref().unwrap_or("");
        au.cmp(bu).then_with(|| a.marker.role.cmp(&b.marker.role))
    });
    let current = candidates.pop()?;
    let status = crate::cli::runs::current_status_path(&stores_dir, &current)
        .as_deref()
        .and_then(|path| {
            read_small_json::<crate::cli::runs::CurrentRunStatus>(
                path,
                LIVE_MARKER_STATUS_READ_BYTES,
            )
        });
    let events_path = current
        .marker
        .events_path
        .as_ref()
        .map(|p| crate::cli::runs::resolve_marker_path(&stores_dir, &current.marker_path, p))
        .or_else(|| {
            current
                .marker
                .status_path
                .as_ref()
                .map(|p| {
                    crate::cli::runs::resolve_marker_path(&stores_dir, &current.marker_path, p)
                })
                .and_then(|p| p.parent().map(|parent| parent.join("events.jsonl")))
        });
    let events = events_path
        .as_deref()
        .map(read_live_event_summaries)
        .unwrap_or_default();
    let status_path = crate::cli::runs::current_status_path(&stores_dir, &current)
        .map(|p| p.display().to_string());
    let marker_path = Some(current.marker_path.display().to_string());
    let transcript_path = current
        .marker
        .transcript_path
        .as_ref()
        .map(|p| crate::cli::runs::resolve_marker_path(&stores_dir, &current.marker_path, p))
        .map(|p| p.display().to_string());
    let stderr_log_path = current
        .marker
        .stderr_log_path
        .as_ref()
        .map(|p| crate::cli::runs::resolve_marker_path(&stores_dir, &current.marker_path, p))
        .map(|p| p.display().to_string());
    let events_path = events_path.map(|p| p.display().to_string());
    Some(LiveRunSummary {
        role: current.marker.role,
        runner: current.marker.runner,
        status: current.marker.status,
        updated_at: current.marker.updated_at,
        last_event_at: status.as_ref().and_then(|s| s.last_event_at.clone()),
        last_event_type: status.as_ref().and_then(|s| s.last_event_type.clone()),
        current_activity: status.and_then(|s| s.current_activity),
        marker_path,
        status_path,
        events_path,
        transcript_path,
        stderr_log_path,
        events,
    })
}

fn read_live_event_summaries(path: &Path) -> Vec<LiveRunEventSummary> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };

    let mut read_bytes = LIVE_EVENTS_READ_BYTES.min(len.max(1));
    let mut last_fallback = None;
    loop {
        let start = len.saturating_sub(read_bytes);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_err() {
            return Vec::new();
        }
        if start > 0 {
            if let Some(pos) = buf.find('\n') {
                buf = buf[pos + 1..].to_string();
            }
        }

        let mut meaningful = Vec::new();
        let mut fallback = None;
        for line in buf.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let Some(summary) = summarize_live_event(&v) else {
                continue;
            };
            if summary.event_type == "heartbeat" {
                fallback = Some(summary);
            } else {
                meaningful.push(summary);
            }
        }
        if fallback.is_some() {
            last_fallback = fallback;
        }
        if !meaningful.is_empty() || start == 0 || read_bytes >= LIVE_EVENTS_SCAN_BYTES {
            if meaningful.is_empty() {
                return last_fallback.into_iter().collect();
            }
            let keep_from = meaningful.len().saturating_sub(LIVE_ACTIVITY_LIMIT);
            return meaningful.split_off(keep_from);
        }
        read_bytes = (read_bytes * 2).min(len).min(LIVE_EVENTS_SCAN_BYTES);
    }
}

fn summarize_live_event(v: &Value) -> Option<LiveRunEventSummary> {
    let event_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("event");
    let ts = v.get("ts").and_then(|x| x.as_str()).map(str::to_string);
    let (label, text) = match event_type {
        "heartbeat" => ("heartbeat".to_string(), String::new()),
        "assistant_text" => (
            if v.get("partial").and_then(|x| x.as_bool()).unwrap_or(false) {
                "assistant*".to_string()
            } else {
                "assistant".to_string()
            },
            string_field(v, "text").unwrap_or_default(),
        ),
        "tool_start" => {
            let name = string_field(v, "name").unwrap_or_else(|| "tool".to_string());
            let path = string_field(v, "path").unwrap_or_default();
            let args = string_field(v, "args_preview").unwrap_or_default();
            ("tool_start".to_string(), join_nonempty(&[name, path, args]))
        }
        "tool_end" => {
            let ok = v
                .get("ok")
                .and_then(|x| x.as_bool())
                .map(|ok| if ok { "ok" } else { "error" })
                .unwrap_or("done");
            let summary = string_field(v, "summary").unwrap_or_default();
            (
                "tool_end".to_string(),
                join_nonempty(&[ok.to_string(), summary]),
            )
        }
        "retry" => {
            let reason = string_field(v, "reason").unwrap_or_else(|| "api retry".to_string());
            ("retry".to_string(), reason)
        }
        "usage" => {
            let input = v.get("input_tokens").and_then(|x| x.as_i64());
            let output = v.get("output_tokens").and_then(|x| x.as_i64());
            let cache = v.get("cache_read_tokens").and_then(|x| x.as_i64());
            (
                "usage".to_string(),
                format!(
                    "in={} out={} cache={}",
                    input
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    output
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    cache
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".to_string())
                ),
            )
        }
        "final_output" => ("final".to_string(), "final output received".to_string()),
        "error" => (
            "error".to_string(),
            string_field(v, "message")
                .or_else(|| string_field(v, "subtype"))
                .unwrap_or_default(),
        ),
        other => ("event".to_string(), other.to_string()),
    };
    Some(LiveRunEventSummary {
        ts,
        event_type: event_type.to_string(),
        label,
        text: truncate_chars(&one_line(&text), LIVE_TEXT_LIMIT),
    })
}

fn read_small_json<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Option<T> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > max_bytes {
        return None;
    }
    let mut body = String::new();
    file.read_to_string(&mut body).ok()?;
    serde_json::from_str(&body).ok()
}

fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).map(|x| match x {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn join_nonempty(parts: &[String]) -> String {
    parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = s.chars().take(max.saturating_sub(1)).collect::<String>();
    out.push('…');
    out
}

/// Load tasks + observations (+ intake when installed) from the db. Errors out only on
/// hard sqlite failures; optional tables/columns degrade to empty/default data.
pub fn load_rows(conn: &Connection) -> Result<Vec<Row>> {
    let mut rows = Vec::new();

    let task_cols = table_columns(conn, "tasks")?;
    let integration_attempts_expr = sql_col(&task_cols, "integration_attempts", "NULL");
    // T144 P2: lifecycle overlay columns are migration-guaranteed by
    // db::open/apply_framework_drift before watch/TUI handlers run. Read them
    // directly (no COALESCE/fallback) so operator opt-outs fail loudly when the
    // framework migration is disabled.
    let task_sql = format!(
        "SELECT display_id, status, {title}, {claimed_by}, {updated_at}, {tier_hint}, {linked_observations}, {blocked_reason}, \
                lifecycle, active_step, integration_step, blocked, blocker_kind, \
                {current_phase}, {current_cycle}, {total_phases}, {plan_source}, \
                {contract_executive_intent}, {contract_done_when}, {contract_scope_in}, {contract_scope_out}, \
                {plan_review_log}, {cycles}, {wrap_log}, {branch}, {workspace_path}, {drive_pid}, {drive_started_at}, \
                {activation}, {human_acceptance_policy}, {task_review_policy}, {acceptance_decided_by}, {claimed_at}, {integration_attempts_expr}, rowid FROM tasks",
        title = sql_col(&task_cols, "title", "''"),
        claimed_by = sql_col(&task_cols, "claimed_by", "NULL"),
        updated_at = sql_col(&task_cols, "updated_at", "''"),
        tier_hint = sql_col(&task_cols, "tier_hint", "NULL"),
        linked_observations = sql_col(&task_cols, "linked_observations", "'[]'"),
        blocked_reason = sql_col(&task_cols, "blocked_reason", "NULL"),
        current_phase = sql_col(&task_cols, "current_phase", "NULL"),
        current_cycle = sql_col(&task_cols, "current_cycle", "NULL"),
        total_phases = if task_cols.iter().any(|c| c == "plan") { "json_array_length(json_extract(plan, '$.phases'))".to_string() } else { "NULL".to_string() },
        plan_source = sql_col(&task_cols, "plan_source", "NULL"),
        contract_executive_intent = json_col(&task_cols, "contract", "$.executive_intent"),
        contract_done_when = json_col(&task_cols, "contract", "$.done_when"),
        contract_scope_in = json_col(&task_cols, "contract", "$.scope_in"),
        contract_scope_out = json_col(&task_cols, "contract", "$.scope_out"),
        plan_review_log = sql_col(&task_cols, "plan_review_log", "'[]'"),
        cycles = sql_col(&task_cols, "cycles", "'[]'"),
        wrap_log = sql_col(&task_cols, "wrap_log", "'[]'"),
        branch = sql_col(&task_cols, "branch", "NULL"),
        workspace_path = sql_col(&task_cols, "workspace_path", "NULL"),
        drive_pid = sql_col(&task_cols, "drive_pid", "NULL"),
        drive_started_at = sql_col(&task_cols, "drive_started_at", "NULL"),
        activation = sql_col(&task_cols, "activation", "NULL"),
        human_acceptance_policy = sql_col(&task_cols, "human_acceptance_policy", "NULL"),
        task_review_policy = sql_col(&task_cols, "task_review_policy", "NULL"),
        acceptance_decided_by = sql_col(&task_cols, "acceptance_decided_by", "NULL"),
        claimed_at = sql_col(&task_cols, "claimed_at", "NULL"),
    );

    // T140 P5: pre-load accepted_at for every task in one shot from
    // transition_history, so the watch render can populate TaskRow.accepted_at
    // without N+1 queries. Map row_id → max(occurred_at) where to_status='accepted'.
    // Skip silently when transition_history is absent or lacks the row_id column
    // (legacy / cockpit-test schemas).
    let accepted_at_map: std::collections::HashMap<i64, String> =
        if table_exists(conn, "transition_history")?
            && column_exists(conn, "transition_history", "row_id")?
        {
            let mut stmt = conn.prepare(
                "SELECT row_id, MAX(occurred_at) FROM transition_history \
             WHERE store='tasks' AND to_status='accepted' GROUP BY row_id",
            )?;
            let rows = stmt.query_map([], |r| {
                let row_id: i64 = r.get(0)?;
                let at: Option<String> = r.get(1)?;
                Ok((row_id, at))
            })?;
            rows.flatten()
                .filter_map(|(rid, at)| at.map(|s| (rid, s)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
    let mut stmt = conn.prepare(&task_sql)?;
    let task_iter = stmt.query_map([], |r| {
        let linked_raw: String = r.get(6)?;
        let blocked_reason: Option<String> = r.get(7).ok().flatten();
        let lifecycle: Option<String> = r.get(8).ok().flatten();
        let active_step: Option<String> = r.get(9).ok().flatten();
        let integration_step: Option<String> = r.get(10).ok().flatten();
        let blocked: Option<bool> = r.get(11).ok().flatten();
        let blocker_kind: Option<String> = r.get(12).ok().flatten();
        let branch: Option<String> = r.get(24).ok().flatten();
        let workspace_path: Option<String> = r.get(25).ok().flatten();
        let drive_pid: Option<i64> = r.get(26).ok().flatten();
        let drive_started_at: Option<String> = r.get(27).ok().flatten();
        let activation: Option<String> = r.get(28).ok().flatten();
        let human_acceptance_policy: Option<String> = r.get(29).ok().flatten();
        let task_review_policy: Option<String> = r.get(30).ok().flatten();
        let acceptance_decided_by: Option<String> = r.get(31).ok().flatten();
        let claimed_at: Option<String> = r.get(32).ok().flatten();
        let integration_attempts_raw: Option<String> = r.get(33).ok().flatten();
        let row_id: i64 = r.get(34)?;
        let display_id: String = r.get(0)?;
        let plan_review_log_raw: Option<String> = r.get(21).ok();
        let cycles_raw: Option<String> = r.get(22).ok();
        let (integration_attempts_count, last_integration_outcome) =
            integration_attempts_summary(integration_attempts_raw.as_deref());
        Ok(TaskRow {
            display_id: display_id.clone(),
            status: r.get(1)?,
            title: r.get(2)?,
            claimed_by: r.get(3).ok().flatten(),
            updated_at: r.get(4)?,
            tier_hint: r.get(5).ok().flatten(),
            linked_observations: serde_json::from_str(&linked_raw).unwrap_or_default(),
            blocked_reason: blocked_reason.clone(),
            blocked_reason_class: Some(blocked_reason_class(blocked_reason.as_deref()).to_string()),
            lifecycle,
            active_step,
            integration_step,
            blocked,
            blocker_kind,
            current_phase: r.get(13).ok().flatten(),
            current_cycle: r.get(14).ok().flatten(),
            total_phases: r.get(15).ok().flatten(),
            plan_source: r.get(16).ok().flatten(),
            contract_executive_intent: r.get(17).ok().flatten(),
            contract_done_when: r.get(18).ok().flatten(),
            contract_scope_in: r.get(19).ok().flatten(),
            contract_scope_out: r.get(20).ok().flatten(),
            plan_review_summaries: json_summary_list(plan_review_log_raw.as_deref(), "summary"),
            cycle_summaries: cycle_summary_list(cycles_raw.as_deref()),
            plan_review_entries: plan_review_entries(plan_review_log_raw.as_deref()),
            cycle_entries: cycle_entries(cycles_raw.as_deref()),
            wrap_summaries: json_summary_list(
                r.get::<_, String>(23).ok().as_deref(),
                "executive_summary",
            ),
            branch,
            workspace_path: workspace_path.clone(),
            drive_pid,
            drive_started_at,
            artifact_pointers: Vec::new(),
            recent_events: Vec::new(),
            activation,
            human_acceptance_policy,
            task_review_policy,
            acceptance_decided_by,
            accepted_at: accepted_at_map.get(&row_id).cloned(),
            claimed_at,
            integration_attempts_count,
            last_integration_outcome,
            live_run: load_live_run_summary(&display_id, workspace_path.as_deref()),
        })
    })?;
    for r in task_iter.flatten() {
        rows.push(Row::Task(r));
    }

    let obs_cols = table_columns(conn, "observations")?;
    let obs_sql = format!(
        "SELECT display_id, status, {priority}, {summary}, {updated_at}, {body}, {source}, {task_id}, {priority_rank}, \
                {lifecycle}, {waiting}, {waiting_kind}, {outcome}, {pending_architecture_review}, {open_architecture_review_id}, {superseded_by_id}, \
                {contract_state}, {tier_hint}, {objective}, {itype}, {acceptance}, {in_scope}, {out_of_scope}, {known_solution}, \
                {locked_by}, {locked_at}, {lock_reason}, {evidence}, {resolution}, {investigation_failure_reason} FROM observations",
        priority = sql_col(&obs_cols, "priority", "''"), summary = sql_col(&obs_cols, "summary", "''"), updated_at = sql_col(&obs_cols, "updated_at", "''"),
        body = sql_col(&obs_cols, "body", "NULL"), source = sql_col(&obs_cols, "source", "NULL"), task_id = sql_col(&obs_cols, "task_id", "NULL"), priority_rank = sql_col(&obs_cols, "priority_rank", "NULL"),
        lifecycle = sql_col(&obs_cols, "lifecycle", "NULL"), waiting = sql_col(&obs_cols, "waiting", "NULL"), waiting_kind = sql_col(&obs_cols, "waiting_kind", "NULL"), outcome = sql_col(&obs_cols, "outcome", "NULL"), pending_architecture_review = sql_col(&obs_cols, "pending_architecture_review", "NULL"), open_architecture_review_id = sql_col(&obs_cols, "open_architecture_review_id", "NULL"), superseded_by_id = sql_col(&obs_cols, "superseded_by_id", "NULL"),
        contract_state = if obs_cols.iter().any(|c| c == "contract_state") { format!("COALESCE(contract_state, {})", json_col(&obs_cols, "intent_contract", "$.contract_state")) } else { json_col(&obs_cols, "intent_contract", "$.contract_state") }, tier_hint = json_col(&obs_cols, "intent_contract", "$.tier_hint"),
        objective = json_col(&obs_cols, "intent_contract", "$.objective"), itype = json_col(&obs_cols, "intent_contract", "$.type"), acceptance = json_col(&obs_cols, "intent_contract", "$.acceptance"),
        in_scope = json_col(&obs_cols, "intent_contract", "$.in_scope"), out_of_scope = json_col(&obs_cols, "intent_contract", "$.out_of_scope"), known_solution = json_col(&obs_cols, "intent_contract", "$.known_solution"),
        locked_by = sql_col(&obs_cols, "locked_by", "NULL"), locked_at = sql_col(&obs_cols, "locked_at", "NULL"), lock_reason = sql_col(&obs_cols, "lock_reason", "NULL"),
        evidence = sql_col(&obs_cols, "evidence", "NULL"), resolution = sql_col(&obs_cols, "resolution", "NULL"), investigation_failure_reason = sql_col(&obs_cols, "investigation_failure_reason", "NULL"),
    );
    let mut stmt = conn.prepare(&obs_sql)?;
    let obs_iter = stmt.query_map([], |r| {
        let evidence_raw: Option<String> = r.get(27).ok().flatten();
        Ok(ObsRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            priority: r.get(2)?,
            summary: r.get(3)?,
            updated_at: r.get(4)?,
            body: r.get(5).ok().flatten(),
            source: r.get(6).ok().flatten(),
            task_id: r.get(7).ok().flatten(),
            priority_rank: r.get(8).ok().flatten(),
            lifecycle: r.get(9).ok().flatten(),
            waiting: r.get(10).ok().flatten(),
            waiting_kind: r.get(11).ok().flatten(),
            outcome: r.get(12).ok().flatten(),
            pending_architecture_review: r.get(13).ok().flatten(),
            open_architecture_review_id: r.get(14).ok().flatten(),
            superseded_by_id: r.get(15).ok().flatten(),
            contract_state: r.get(16).ok().flatten(),
            tier_hint: r.get(17).ok().flatten(),
            intent_objective: r.get(18).ok().flatten(),
            intent_type: r.get(19).ok().flatten(),
            intent_acceptance: json_string_array(r.get::<_, String>(20).ok().as_deref()),
            intent_in_scope: json_string_array(r.get::<_, String>(21).ok().as_deref()),
            intent_out_of_scope: json_string_array(r.get::<_, String>(22).ok().as_deref()),
            intent_known_solution: r.get(23).ok().flatten(),
            locked_by: r.get(24).ok().flatten(),
            locked_at: r.get(25).ok().flatten(),
            lock_reason: r.get(26).ok().flatten(),
            evidence_pointers: evidence_pointers(evidence_raw.as_deref()),
            resolution_pointer: r.get(28).ok().flatten(),
            recent_events: Vec::new(),
            investigation_failure_reason: r.get(29).ok().flatten(),
        })
    })?;
    for r in obs_iter.flatten() {
        rows.push(Row::Obs(r));
    }

    if table_exists(conn, "external_reviews")? {
        let cols = table_columns(conn, "external_reviews")?;
        let runner_expr = if cols.iter().any(|c| c == "runner") {
            "COALESCE(runner,'')"
        } else {
            "''"
        };
        let held_expr = if cols.iter().any(|c| c == "held_reason") {
            "held_reason"
        } else {
            "NULL"
        };
        let retry_expr = if cols.iter().any(|c| c == "next_retry_at") {
            "next_retry_at"
        } else {
            "NULL"
        };
        let attempts_expr = if cols.iter().any(|c| c == "attempts") {
            "COALESCE(attempts,0)"
        } else {
            "0"
        };
        let findings_count_expr = if cols.iter().any(|c| c == "findings") {
            "json_array_length(findings)".to_string()
        } else {
            "NULL".to_string()
        };
        let sql = format!(
            "SELECT display_id, task_id, status, {lifecycle}, {outcome}, {linked_observation_ids}, {produced_task_id}, {runner_expr}, {held_expr}, {retry_expr}, {attempts_expr}, \
             {verdict}, {base_sha}, {head_sha}, {log_path}, {transcript_path}, {started_at}, {completed_at}, {duration_ms}, \
             {critical_count}, {major_count}, {minor_count}, {findings_count_expr} \
             FROM external_reviews WHERE status IN ('pending','running','tooling_held')",
            lifecycle = sql_col(&cols, "lifecycle", "NULL"),
            outcome = sql_col(&cols, "outcome", "NULL"),
            linked_observation_ids = sql_col(&cols, "linked_observation_ids", "NULL"),
            produced_task_id = sql_col(&cols, "produced_task_id", "NULL"),
            verdict = sql_col(&cols, "verdict", "NULL"),
            base_sha = sql_col(&cols, "base_sha", "NULL"),
            head_sha = sql_col(&cols, "head_sha", "NULL"),
            log_path = sql_col(&cols, "log_path", "NULL"),
            transcript_path = sql_col(&cols, "transcript_path", "NULL"),
            started_at = sql_col(&cols, "started_at", "NULL"),
            completed_at = sql_col(&cols, "completed_at", "NULL"),
            duration_ms = sql_col(&cols, "duration_ms", "NULL"),
            critical_count = sql_col(&cols, "critical_count", "NULL"),
            major_count = sql_col(&cols, "major_count", "NULL"),
            minor_count = sql_col(&cols, "minor_count", "NULL"),
        );
        let mut stmt = conn.prepare(&sql)?;
        let review_iter = stmt.query_map([], |r| {
            Ok(ReviewRow {
                display_id: r.get(0)?,
                task_id: r.get(1)?,
                status: r.get(2)?,
                lifecycle: r.get(3).ok().flatten(),
                outcome: r.get(4).ok().flatten(),
                linked_observation_ids: json_string_array(r.get::<_, String>(5).ok().as_deref()),
                produced_task_id: r.get(6).ok().flatten(),
                runner: r.get(7)?,
                held_reason: r.get(8).ok().flatten(),
                next_retry_at: r.get(9).ok().flatten(),
                attempts: r.get(10)?,
                verdict: r.get(11).ok().flatten(),
                base_sha: r.get(12).ok().flatten(),
                head_sha: r.get(13).ok().flatten(),
                log_path: r.get(14).ok().flatten(),
                transcript_path: r.get(15).ok().flatten(),
                started_at: r.get(16).ok().flatten(),
                completed_at: r.get(17).ok().flatten(),
                duration_ms: r.get(18).ok().flatten(),
                critical_count: r.get(19).ok().flatten(),
                major_count: r.get(20).ok().flatten(),
                minor_count: r.get(21).ok().flatten(),
                findings_count: r.get(22).ok().flatten(),
            })
        })?;
        for r in review_iter.flatten() {
            rows.push(Row::Review(r));
        }
    }

    if table_exists(conn, "architecture_reviews")? {
        let cols = table_columns(conn, "architecture_reviews")?;
        let sql = format!(
            "SELECT display_id, status, {lifecycle}, {outcome}, {linked_observation_ids}, {produced_task_id}, {source_observation}, {verdict} \
             FROM architecture_reviews \
             WHERE status IN ('pending','in_review','awaiting_human_ratification','verdict_issued')",
            lifecycle = sql_col(&cols, "lifecycle", "NULL"),
            outcome = sql_col(&cols, "outcome", "NULL"),
            linked_observation_ids = sql_col(&cols, "linked_observation_ids", "NULL"),
            produced_task_id = sql_col(&cols, "produced_task_id", "NULL"),
            source_observation = sql_col(&cols, "source_observation", "NULL"),
            verdict = sql_col(&cols, "verdict", "NULL"),
        );
        let mut stmt = conn.prepare(&sql)?;
        let arch_iter = stmt.query_map([], |r| {
            Ok(ReviewRow {
                display_id: r.get(0)?,
                task_id: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                status: r.get(1)?,
                lifecycle: r.get(2).ok().flatten(),
                outcome: r.get(3).ok().flatten(),
                linked_observation_ids: json_string_array(r.get::<_, String>(4).ok().as_deref()),
                produced_task_id: r.get(5).ok().flatten(),
                verdict: r.get(7).ok().flatten(),
                ..Default::default()
            })
        })?;
        for r in arch_iter.flatten() {
            rows.push(Row::Review(r));
        }
    }

    if table_exists(conn, "intake")? {
        load_intake_rows(conn, &mut rows)?;
    }
    attach_recent_events(conn, &mut rows)?;
    Ok(rows)
}

pub fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

pub fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    Ok(table_columns(conn, table)?.iter().any(|c| c == col))
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    if !table_exists(conn, table)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote_ident(table)))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn sql_col(cols: &[String], col: &str, fallback: &str) -> String {
    if cols.iter().any(|c| c == col) {
        quote_ident(col)
    } else {
        fallback.to_string()
    }
}

fn json_col(cols: &[String], col: &str, path: &str) -> String {
    if cols.iter().any(|c| c == col) {
        format!(
            "json_extract({}, '{}')",
            quote_ident(col),
            path.replace('\'', "''")
        )
    } else {
        "NULL".to_string()
    }
}

fn json_string_array(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn json_summary_list(raw: Option<&str>, key: &str) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.get(key).and_then(|s| s.as_str()).map(str::to_string))
        .collect()
}

fn cycle_summary_list(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|v| {
            let mut out = Vec::new();
            if let Some(s) = v.pointer("/executor/summary").and_then(|s| s.as_str()) {
                out.push(format!("executor: {s}"));
            }
            if let Some(s) = v.pointer("/review/summary").and_then(|s| s.as_str()) {
                out.push(format!("review: {s}"));
            }
            out
        })
        .collect()
}

fn plan_review_entries(raw: Option<&str>) -> Vec<TaskPlanReviewEntry> {
    raw.and_then(|s| serde_json::from_str::<Vec<TaskPlanReviewEntry>>(s).ok())
        .unwrap_or_default()
}

fn cycle_entries(raw: Option<&str>) -> Vec<TaskCycleEntry> {
    raw.and_then(|s| serde_json::from_str::<Vec<TaskCycleEntry>>(s).ok())
        .unwrap_or_default()
}

fn decision_metadata_summary(
    raw: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (None, None, None);
    };
    let pluck =
        |key: &str| -> Option<String> { v.get(key).and_then(|x| x.as_str()).map(str::to_string) };
    (pluck("rationale"), pluck("confidence"), pluck("tier_hint"))
}

fn integration_attempts_summary(raw: Option<&str>) -> (usize, Option<String>) {
    let Some(raw) = raw else {
        return (0, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (0, None);
    };
    let Some(arr) = v.as_array() else {
        return (0, None);
    };
    let count = arr.len();
    let last_outcome = arr
        .last()
        .and_then(|item| item.get("outcome"))
        .and_then(|s| s.as_str())
        .map(str::to_string);
    (count, last_outcome)
}

fn evidence_pointers(raw: Option<&str>) -> Vec<ArtifactPointer> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(arr) = v.get("external_refs").and_then(|x| x.as_array()) {
        for item in arr {
            let system = item
                .get("system")
                .and_then(|x| x.as_str())
                .unwrap_or("external");
            let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if !id.is_empty() {
                out.push(ArtifactPointer {
                    label: system.to_string(),
                    value: id.to_string(),
                });
            }
        }
    }
    out
}

fn load_intake_rows(conn: &Connection, rows: &mut Vec<Row>) -> Result<()> {
    let cols = table_columns(conn, "intake")?;
    let sql = format!(
        "SELECT display_id, status, {summary}, {body}, {updated_at}, {source_task}, {source_agent}, {risk_flags}, {cluster_key}, {decision}, {missing_info_question}, {routed_to_observation}, {routed_to_arch_review}, {duplicate_of}, {evidence}, {captured_at}, {recon_round}, {decision_metadata}, {lifecycle}, {waiting_kind}, {outcome}, {duplicate_of_id}, {produced_observation_id}, {produced_architecture_review_id}, {produced_task_id}, {produced_artifact_kind}, {produced_artifact_id} FROM intake",
        summary = sql_col(&cols, "summary", "''"), body = sql_col(&cols, "body", "NULL"),
        updated_at = if cols.iter().any(|c| c == "updated_at") { quote_ident("updated_at") } else { sql_col(&cols, "captured_at", "''") },
        source_task = sql_col(&cols, "source_task", "NULL"), source_agent = sql_col(&cols, "source_agent", "NULL"), risk_flags = sql_col(&cols, "risk_flags", "NULL"), cluster_key = sql_col(&cols, "cluster_key", "NULL"),
        decision = sql_col(&cols, "decision", "NULL"), missing_info_question = sql_col(&cols, "missing_info_question", "NULL"), routed_to_observation = sql_col(&cols, "routed_to_observation", "NULL"), routed_to_arch_review = sql_col(&cols, "routed_to_arch_review", "NULL"), duplicate_of = sql_col(&cols, "duplicate_of", "NULL"), evidence = sql_col(&cols, "evidence", "NULL"),
        captured_at = sql_col(&cols, "captured_at", "NULL"),
        recon_round = sql_col(&cols, "recon_round", "NULL"),
        decision_metadata = sql_col(&cols, "decision_metadata", "NULL"),
        lifecycle = sql_col(&cols, "lifecycle", "NULL"),
        waiting_kind = sql_col(&cols, "waiting_kind", "NULL"),
        outcome = sql_col(&cols, "outcome", "NULL"),
        duplicate_of_id = sql_col(&cols, "duplicate_of_id", "NULL"),
        produced_observation_id = sql_col(&cols, "produced_observation_id", "NULL"),
        produced_architecture_review_id = sql_col(&cols, "produced_architecture_review_id", "NULL"),
        produced_task_id = sql_col(&cols, "produced_task_id", "NULL"),
        produced_artifact_kind = sql_col(&cols, "produced_artifact_kind", "NULL"),
        produced_artifact_id = sql_col(&cols, "produced_artifact_id", "NULL"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let iter = stmt.query_map([], |r| {
        let status: String = r.get(1)?;
        let decision: Option<String> = r.get(9).ok().flatten();
        let missing: Option<String> = r.get(10).ok().flatten();
        let held_reason = if status == "needs_info" {
            missing.clone()
        } else {
            None
        };
        let decision_metadata_raw: Option<String> = r.get(17).ok().flatten();
        let (decision_rationale, decision_confidence, decision_tier_hint) =
            decision_metadata_summary(decision_metadata_raw.as_deref());
        Ok(IntakeRow {
            display_id: r.get(0)?,
            status: status.clone(),
            summary: r.get(2)?,
            lifecycle: r.get(18).ok().flatten(),
            waiting_kind: r.get(19).ok().flatten(),
            outcome: r.get(20).ok().flatten(),
            body: r.get(3).ok().flatten(),
            updated_at: r.get(4)?,
            source_task: r.get(5).ok().flatten(),
            source_agent: r.get(6).ok().flatten(),
            priority: None,
            risk_flags: json_string_array(r.get::<_, String>(7).ok().as_deref()),
            cluster_key: r.get(8).ok().flatten(),
            decision: decision.clone(),
            missing_info_question: missing,
            held_reason,
            next_action: Some(intake_next_action(&status, decision.as_deref()).to_string()),
            routed_to_observation: r.get(11).ok().flatten(),
            routed_to_arch_review: r.get(12).ok().flatten(),
            duplicate_of: r.get(13).ok().flatten(),
            duplicate_of_id: r.get(21).ok().flatten(),
            produced_observation_id: r.get(22).ok().flatten(),
            produced_architecture_review_id: r.get(23).ok().flatten(),
            produced_task_id: r.get(24).ok().flatten(),
            produced_artifact_kind: r.get(25).ok().flatten(),
            produced_artifact_id: r.get(26).ok().flatten(),
            evidence_pointer: r.get(14).ok().flatten(),
            recent_events: Vec::new(),
            captured_at: r.get(15).ok().flatten(),
            recon_round: r.get(16).ok().flatten(),
            decision_rationale,
            decision_confidence,
            decision_tier_hint,
        })
    })?;
    for r in iter.flatten() {
        rows.push(Row::Intake(r));
    }
    Ok(())
}

fn intake_next_action(status: &str, decision: Option<&str>) -> &'static str {
    match status {
        "draft" => "claim triage",
        "triaging" => "gatekeeper route",
        "needs_info" => "recon needed",
        "routed" => match decision {
            Some("duplicate") => "duplicate linked",
            Some("arch_review_candidate") => "architecture review routed",
            _ => "routed",
        },
        "dropped" => "dropped unless amended",
        _ => "inspect",
    }
}

fn attach_recent_events(conn: &Connection, rows: &mut [Row]) -> Result<()> {
    if !table_exists(conn, "transition_history")? {
        return Ok(());
    }
    let cols = table_columns(conn, "transition_history")?;
    if !cols.iter().any(|c| c == "display_id") {
        return Ok(());
    }
    let has = |c: &str| cols.iter().any(|x| x == c);
    let sql = format!(
        "SELECT {store}, display_id, {from_status}, {to_status}, {verb}, {occurred_at} FROM transition_history WHERE display_id=?1 ORDER BY {order_col} DESC LIMIT 5",
        store = sql_col(&cols, "store", "NULL"), from_status = sql_col(&cols, "from_status", "NULL"), to_status = sql_col(&cols, "to_status", "NULL"), verb = sql_col(&cols, "verb", "NULL"), occurred_at = sql_col(&cols, "occurred_at", "NULL"), order_col = if has("id") { quote_ident("id") } else if has("occurred_at") { quote_ident("occurred_at") } else { quote_ident("display_id") },
    );
    for row in rows.iter_mut() {
        let id = row.display_id().to_string();
        let events: Vec<RecentEvent> = conn
            .prepare(&sql)?
            .query_map([id], |r| {
                Ok(RecentEvent {
                    store: r.get(0).ok().flatten(),
                    display_id: r.get(1)?,
                    from_status: r.get(2).ok().flatten(),
                    to_status: r.get(3).ok().flatten(),
                    verb: r.get(4).ok().flatten(),
                    occurred_at: r.get(5).ok().flatten(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        match row {
            Row::Task(t) => t.recent_events = events,
            Row::Obs(o) => o.recent_events = events,
            Row::CollapsedObs(c) => c.representative.recent_events = events,
            Row::Review(_) => {} // ReviewRow has no recent_events field
            Row::Intake(i) => i.recent_events = events,
        }
    }
    Ok(())
}

pub fn load_system_health(conn: &Connection) -> Result<SystemHealth> {
    if !table_exists(conn, "dispatch_locks")?
        || !column_exists(conn, "dispatch_locks", "claimed_at")?
        || !column_exists(conn, "dispatch_locks", "finished_at")?
    {
        return Ok(SystemHealth::default());
    }
    let (count, oldest): (usize, Option<rusqlite::types::Value>) = conn.query_row(
        "SELECT COUNT(*), MIN(claimed_at) FROM dispatch_locks WHERE finished_at IS NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1).ok())),
    )?;
    Ok(SystemHealth {
        unfinished_dispatch_locks: count,
        oldest_claimed_at_epoch: oldest.and_then(value_to_epoch),
    })
}

fn value_to_epoch(value: rusqlite::types::Value) -> Option<i64> {
    match value {
        rusqlite::types::Value::Integer(v) => Some(v),
        rusqlite::types::Value::Real(v) => Some(v.floor() as i64),
        rusqlite::types::Value::Text(s) => parse_epoch(&s),
        rusqlite::types::Value::Null | rusqlite::types::Value::Blob(_) => None,
    }
}

/// Aggregate richer engine-health detail (recent daemon starts, unfinished
/// dispatch_locks with agent_name/claimed_by/attempts, recent agent_runs by
/// role). Each sub-load is column-presence-guarded; absent tables/columns
/// return an empty vec rather than erroring.
pub fn load_engine_detail(conn: &Connection) -> Result<EngineDetail> {
    let recent_daemon_starts = load_daemon_starts(conn).unwrap_or_default();
    let unfinished_lock_rows = load_unfinished_dispatch_locks(conn).unwrap_or_default();
    let recent_agent_runs_by_role = load_agent_runs_by_role(conn).unwrap_or_default();
    Ok(EngineDetail {
        recent_daemon_starts,
        unfinished_lock_rows,
        recent_agent_runs_by_role,
    })
}

fn load_daemon_starts(conn: &Connection) -> Result<Vec<DaemonStartRow>> {
    if !table_exists(conn, "daemon_starts")? {
        return Ok(Vec::new());
    }
    let cols = table_columns(conn, "daemon_starts")?;
    if !cols.iter().any(|c| c == "pid") {
        return Ok(Vec::new());
    }
    let order_col = if cols.iter().any(|c| c == "started_at") {
        "started_at"
    } else if cols.iter().any(|c| c == "id") {
        "id"
    } else {
        "rowid"
    };
    let sql = format!(
        "SELECT pid, {started_at}, {binary_version}, {git_sha} FROM daemon_starts \
         ORDER BY {order_col} DESC LIMIT 5",
        started_at = sql_col(&cols, "started_at", "NULL"),
        binary_version = sql_col(&cols, "binary_version", "NULL"),
        git_sha = sql_col(&cols, "git_sha", "NULL"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let iter = stmt.query_map([], |r| {
        Ok(DaemonStartRow {
            pid: r.get(0)?,
            started_at: r.get(1).ok().flatten(),
            binary_version: r.get(2).ok().flatten(),
            git_sha: r.get(3).ok().flatten(),
        })
    })?;
    Ok(iter.flatten().collect())
}

fn load_unfinished_dispatch_locks(conn: &Connection) -> Result<Vec<DispatchLockRow>> {
    if !table_exists(conn, "dispatch_locks")? {
        return Ok(Vec::new());
    }
    let cols = table_columns(conn, "dispatch_locks")?;
    if !cols.iter().any(|c| c == "display_id") || !cols.iter().any(|c| c == "finished_at") {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT display_id, {agent_name}, {claimed_by}, {claimed_at}, {heartbeat_at}, {attempts} \
         FROM dispatch_locks WHERE finished_at IS NULL",
        agent_name = sql_col(&cols, "agent_name", "NULL"),
        claimed_by = sql_col(&cols, "claimed_by", "NULL"),
        claimed_at = sql_col(&cols, "claimed_at", "NULL"),
        heartbeat_at = sql_col(&cols, "heartbeat_at", "NULL"),
        attempts = if cols.iter().any(|c| c == "attempts") {
            "COALESCE(attempts,0)"
        } else {
            "0"
        },
    );
    let mut stmt = conn.prepare(&sql)?;
    let iter = stmt.query_map([], |r| {
        Ok(DispatchLockRow {
            display_id: r.get(0)?,
            agent_name: r.get(1).ok().flatten(),
            claimed_by: r.get(2).ok().flatten(),
            claimed_at: r.get(3).ok().flatten(),
            heartbeat_at: r.get(4).ok().flatten(),
            liveness_label: crate::runner::liveness::classify(
                r.get::<_, Option<String>>(3)
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(parse_epoch),
                r.get::<_, Option<String>>(4)
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(parse_epoch),
                now_epoch(),
                &crate::runner::liveness::LivenessThresholds::from_env(),
            )
            .label(),
            attempts: r.get(5)?,
        })
    })?;
    Ok(iter.flatten().collect())
}

fn load_agent_runs_by_role(conn: &Connection) -> Result<Vec<AgentRunsRoleAggregate>> {
    if !table_exists(conn, "agent_runs")? {
        return Ok(Vec::new());
    }
    let cols = table_columns(conn, "agent_runs")?;
    if !cols.iter().any(|c| c == "role") {
        return Ok(Vec::new());
    }
    let total_tokens_expr = if cols.iter().any(|c| c == "total_tokens") {
        "COALESCE(SUM(total_tokens),0)".to_string()
    } else if cols.iter().any(|c| c == "tokens_in") || cols.iter().any(|c| c == "tokens_out") {
        let ti = if cols.iter().any(|c| c == "tokens_in") {
            "COALESCE(tokens_in,0)"
        } else {
            "0"
        };
        let to = if cols.iter().any(|c| c == "tokens_out") {
            "COALESCE(tokens_out,0)"
        } else {
            "0"
        };
        format!("COALESCE(SUM({ti} + {to}),0)")
    } else {
        "0".to_string()
    };
    let sql = format!(
        "SELECT role, COUNT(*) AS cnt, {total_tokens_expr} AS tokens \
         FROM agent_runs GROUP BY role ORDER BY cnt DESC, role ASC LIMIT 20"
    );
    let mut stmt = conn.prepare(&sql)?;
    let iter = stmt.query_map([], |r| {
        Ok(AgentRunsRoleAggregate {
            role: r.get(0)?,
            count: r.get(1)?,
            total_tokens: r.get(2)?,
        })
    })?;
    Ok(iter.flatten().collect())
}

pub fn load_external_review_state(conn: &Connection) -> Result<ExternalReviewState> {
    let table = if table_exists(conn, "external_reviews")? {
        Some("external_reviews")
    } else if table_exists(conn, "external_review")? {
        Some("external_review")
    } else {
        None
    };
    let Some(table) = table else {
        return Ok(ExternalReviewState::default());
    };
    let rows = conn.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote_ident(table)),
        [],
        |r| r.get::<_, usize>(0),
    )?;
    if rows == 0 {
        return Ok(ExternalReviewState::Unavailable {
            reason: "external review: unavailable / not installed".to_string(),
        });
    }
    let cols = table_columns(conn, table)?;
    let sql = format!(
        "SELECT {lane}, {status} FROM {} LIMIT 1",
        quote_ident(table),
        lane = sql_col(&cols, "lane", "NULL"),
        status = sql_col(&cols, "status", "NULL"),
    );
    let (lane, status) = conn.query_row(&sql, [], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
        ))
    })?;
    Ok(ExternalReviewState::Available { rows, lane, status })
}

/// Classify each row into a section using default watch options.
pub fn classify(rows: &[Row]) -> Vec<(Section, Vec<usize>)> {
    classify_with_options(rows, WatchClassifyOptions::default())
}

/// Classify each row into a section. Returns `[(Section, indices)]` in the
/// canonical section order; sections with no rows are still present.
pub fn classify_with_options(
    rows: &[Row],
    opts: WatchClassifyOptions,
) -> Vec<(Section, Vec<usize>)> {
    classify_with_options_at(rows, opts, now_epoch())
}

/// Collapse duplicate observation summaries independently within each section.
/// Returns a new row store plus section indices over that store; no Section
/// variant is introduced for collapsed observations.
pub fn dedup_observation_summaries_by_section(
    rows: &[Row],
    sections: &[(Section, Vec<usize>)],
) -> (Vec<Row>, Vec<(Section, Vec<usize>)>) {
    let mut out_rows = Vec::new();
    let mut out_sections = Vec::with_capacity(sections.len());

    for (section, indices) in sections {
        let mut by_summary: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &idx in indices {
            if let Some(Row::Obs(o)) = rows.get(idx) {
                by_summary.entry(o.summary.clone()).or_default().push(idx);
            }
        }

        let mut new_indices = Vec::new();
        let mut consumed = std::collections::HashSet::new();
        for &idx in indices {
            if consumed.contains(&idx) {
                continue;
            }
            match rows.get(idx) {
                Some(Row::Obs(o)) => {
                    let group = by_summary
                        .get(&o.summary)
                        .cloned()
                        .unwrap_or_else(|| vec![idx]);
                    if group.len() >= 2 {
                        for member in &group {
                            consumed.insert(*member);
                        }
                        let mut display_ids: Vec<String> = group
                            .iter()
                            .filter_map(|&i| match rows.get(i) {
                                Some(Row::Obs(obs)) => Some(obs.display_id.clone()),
                                _ => None,
                            })
                            .collect();
                        display_ids.sort();
                        let primary_display_id = display_ids.first().cloned().unwrap_or_default();
                        let representative = group
                            .iter()
                            .filter_map(|&i| match rows.get(i) {
                                Some(Row::Obs(obs)) => Some(obs.clone()),
                                _ => None,
                            })
                            .min_by(|a, b| a.display_id.cmp(&b.display_id))
                            .unwrap_or_else(|| o.clone());
                        let abs = out_rows.len();
                        out_rows.push(Row::CollapsedObs(CollapsedObsRow {
                            section: *section,
                            summary: o.summary.clone(),
                            count: display_ids.len(),
                            primary_display_id,
                            display_ids,
                            representative,
                        }));
                        new_indices.push(abs);
                    } else {
                        consumed.insert(idx);
                        let abs = out_rows.len();
                        out_rows.push(rows[idx].clone());
                        new_indices.push(abs);
                    }
                }
                Some(row) => {
                    let abs = out_rows.len();
                    out_rows.push(row.clone());
                    new_indices.push(abs);
                }
                None => {}
            }
        }
        out_sections.push((*section, new_indices));
    }

    (out_rows, out_sections)
}

fn classify_with_options_at(
    rows: &[Row],
    opts: WatchClassifyOptions,
    now: i64,
) -> Vec<(Section, Vec<usize>)> {
    let mut buckets: Vec<(Section, Vec<usize>)> =
        Section::ALL.iter().map(|s| (*s, Vec::new())).collect();
    let mut terminal = Vec::new();
    let task_ctx = task_status_by_id(rows);

    for (i, row) in rows.iter().enumerate() {
        let visibility = row_visibility_class(row, &task_ctx);
        if visibility == VisibilityClass::HistoricalNoise {
            if opts.show_all_history {
                match row {
                    Row::Task(_) => terminal.push(i),
                    Row::Obs(_) | Row::CollapsedObs(_) | Row::Review(_) | Row::Intake(_) => {
                        if let Some(sec) = section_for(row) {
                            push_bucket(&mut buckets, sec, i);
                        }
                    }
                }
            }
            continue;
        }
        match section_for(row) {
            Some(Section::TasksRecentlyTerminal) => terminal.push(i),
            Some(sec) => push_bucket(&mut buckets, sec, i),
            None => {}
        }
    }

    terminal.sort_by(|a, b| {
        let ta = task_updated_epoch(&rows[*a]).unwrap_or(i64::MIN);
        let tb = task_updated_epoch(&rows[*b]).unwrap_or(i64::MIN);
        tb.cmp(&ta)
            .then_with(|| rows[*a].display_id().cmp(rows[*b].display_id()))
    });

    let cutoff =
        now.saturating_sub((opts.recent_terminal_days as i64).saturating_mul(SECS_PER_DAY));
    for idx in terminal
        .into_iter()
        .filter(|idx| {
            opts.show_all_history
                || task_updated_epoch(&rows[*idx])
                    .map(|updated| updated >= cutoff)
                    .unwrap_or(true)
        })
        .take(if opts.show_all_history {
            usize::MAX
        } else {
            opts.recent_terminal_limit
        })
    {
        push_bucket(&mut buckets, Section::TasksRecentlyTerminal, idx);
    }

    buckets
}

fn push_bucket(buckets: &mut [(Section, Vec<usize>)], sec: Section, idx: usize) {
    let bucket = buckets
        .iter_mut()
        .find(|(s, _)| *s == sec)
        .expect("section_for returns a member of Section::ALL");
    bucket.1.push(idx);
}

fn active_overlay_step(t: &TaskRow) -> Option<&str> {
    t.active_step
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
}

pub(crate) fn visible_step(t: &TaskRow) -> Option<&str> {
    active_overlay_step(t).or_else(|| {
        t.integration_step
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "none")
    })
}

pub fn task_lifecycle(t: &TaskRow) -> &str {
    if let Some(v) = t
        .lifecycle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    legacy_lifecycle(&t.status)
}

pub fn task_active_step(t: &TaskRow) -> &str {
    if let Some(v) = t
        .active_step
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    legacy_active_step(&t.status)
}

pub fn task_integration_step(t: &TaskRow) -> &str {
    if let Some(v) = t
        .integration_step
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return v;
    }
    legacy_integration_step(&t.status)
}

pub fn task_has_live_running_agent(t: &TaskRow) -> bool {
    t.live_run
        .as_ref()
        .and_then(|live| live.status.as_deref())
        .map(|status| status == "running")
        .unwrap_or(false)
}

pub fn task_is_blocked(t: &TaskRow) -> bool {
    if task_is_terminal_with_compat(t) {
        return false;
    }
    // `blocked`/`blocker_kind=capacity` can be a scheduler-side overlay while a
    // planner/executor child is already live. The watch cockpit is an operator
    // surface: a task with a running agent is active work, not HELD-TRIAGE.
    if task_has_live_running_agent(t) {
        return false;
    }
    t.blocked.unwrap_or_else(|| {
        matches!(
            t.status.as_str(),
            "blocked" | "deploy_blocked" | "integration_blocked"
        )
    }) || t
        .blocker_kind
        .as_deref()
        .map(|s| !s.is_empty() && s != "none")
        .unwrap_or(false)
}

fn legacy_lifecycle(status: &str) -> &str {
    match status {
        "integration_queued" | "integrating" | "integrated" | "integration_blocked" => {
            "integration"
        }
        "accepted" | "complete" | "cargo_installed" | "schema_migrated" | "rejected"
        | "abandoned" | "closed_out_of_band" => "done",
        _ => "active",
    }
}

fn legacy_active_step(status: &str) -> &str {
    match status {
        "planning" => "planning",
        "plan_review" => "planning_review",
        "executing" => "coding",
        "code_review" => "coding_review",
        "in_review" => "wrapping",
        _ => "none",
    }
}

fn legacy_integration_step(status: &str) -> &str {
    match status {
        "integration_queued" => "queued",
        "integrating" => "merging",
        "integrated" => "deploying",
        _ => "none",
    }
}

pub fn task_is_terminal_primary(t: &TaskRow) -> bool {
    task_lifecycle(t) == "done"
}

pub fn task_is_terminal_with_compat(t: &TaskRow) -> bool {
    if t.lifecycle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some()
    {
        return task_is_terminal_primary(t);
    }
    is_terminal_task_status(&t.status)
}

pub fn task_is_in_flight_primary(t: &TaskRow) -> bool {
    matches!(task_lifecycle(t), "queued" | "active" | "integration")
}

fn section_for(row: &Row) -> Option<Section> {
    match row {
        Row::Task(t) => {
            let reason = t
                .blocked_reason
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase();
            if task_is_terminal_with_compat(t) {
                return Some(Section::TasksRecentlyTerminal);
            }
            if is_silent_zombie_reason(&reason) {
                return Some(Section::TasksHeldZombie);
            }
            if task_active_step(t) == "wrapping" {
                return Some(Section::TasksAcceptU3);
            }
            if task_is_blocked(t) {
                if task_lifecycle(t) == "integration" {
                    return Some(Section::TasksIntegrationBlocked);
                }
                if task_visibility_class(t) == VisibilityClass::NeedsTriage {
                    return Some(Section::TasksNeedsTriage);
                }
                if t.status == "deploy_blocked" || t.blocker_kind.as_deref() == Some("deploy") {
                    return Some(Section::TasksDeployRecovery);
                }
                return Some(Section::TasksBlockedNeedsAction);
            }
            match task_lifecycle(t) {
                "active" => match task_active_step(t) {
                    "wrapping" => Some(Section::TasksAcceptU3),
                    _ if is_priority_task(t) => Some(Section::ObsOpenNoContract),
                    _ => Some(Section::TasksActionableCurrentWork),
                },
                "integration" => match task_integration_step(t) {
                    "deploying" | "verifying" => Some(Section::TasksIntegratedAwaitingPostLand),
                    _ => Some(Section::TasksIntegration),
                },
                "done" => Some(Section::TasksRecentlyTerminal),
                "queued" => Some(Section::TasksQueued),
                _ if is_priority_task(t) => Some(Section::ObsOpenNoContract),
                _ => Some(Section::TasksActionableCurrentWork),
            }
        }
        Row::Obs(o) => {
            if is_silent_zombie_reason(o.investigation_failure_reason.as_deref().unwrap_or("")) {
                Some(Section::TasksHeldZombie)
            } else if o.contract_state.as_deref() == Some("ready") {
                Some(Section::ObsRatifiable)
            } else if is_priority_text(&o.priority)
                || o.priority_rank.map(|r| r <= 1).unwrap_or(false)
            {
                Some(Section::ObsOpenNoContract)
            } else {
                Some(Section::ObsOther)
            }
        }
        Row::CollapsedObs(c) => Some(c.section),
        Row::Review(_) => Some(Section::ExternalReviewLane),
        Row::Intake(i) => match intake_lifecycle(i) {
            "closed" => None,
            "waiting" => Some(Section::IntakeHeld),
            _ if is_priority_text(i.priority.as_deref().unwrap_or(""))
                || !i.risk_flags.is_empty() =>
            {
                Some(Section::ObsOpenNoContract)
            }
            _ => Some(Section::IntakeOpen),
        },
    }
}

/// Map a `Row` to the cockpit's top-level [`StoreLane`].
pub fn store_lane_for_row(row: &Row) -> StoreLane {
    match row {
        Row::Intake(_) => StoreLane::Intake,
        Row::Obs(_) | Row::CollapsedObs(_) => StoreLane::Observations,
        Row::Task(_) => StoreLane::Tasks,
        Row::Review(_) => StoreLane::ExternalReviews,
    }
}

/// Return up to `limit` newest terminal task rows, sorted by `updated_at`
/// descending (display_id as tiebreaker). Used to render the "recent
/// exhaust" strip — main task rows hide terminal history. This mirrors the
/// default watch classification policy: terminal rows older than the recent
/// terminal window stay hidden unless the operator explicitly asks for history.
pub fn recent_exhaust(rows: &[Row], limit: usize) -> Vec<&Row> {
    if limit == 0 {
        return Vec::new();
    }
    let mut indices: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            Row::Task(t) if task_is_terminal_with_compat(t) => Some(i),
            _ => None,
        })
        .collect();
    indices.sort_by(|a, b| {
        let ea = task_updated_epoch(&rows[*a]).unwrap_or(i64::MIN);
        let eb = task_updated_epoch(&rows[*b]).unwrap_or(i64::MIN);
        eb.cmp(&ea)
            .then_with(|| rows[*a].display_id().cmp(rows[*b].display_id()))
    });
    indices.into_iter().take(limit).map(|i| &rows[i]).collect()
}

pub fn is_terminal_task_status(s: &str) -> bool {
    matches!(
        s,
        "closed_out_of_band"
            | "accepted"
            | "complete"
            | "cargo_installed"
            | "schema_migrated"
            | "rejected"
            | "abandoned"
            | "done"
    )
}

pub fn blocked_reason_class(reason: Option<&str>) -> &'static str {
    let raw = reason.unwrap_or("").trim();
    if raw.is_empty() {
        return "unknown";
    }
    // Codex T059-r1 MEDIUM: real blocked_reason values are often structured
    // JSON like `{"exit_code":1,"kind":"rate_limit","reset_at":...}` written
    // by the drive runner. Parse the `kind` first; fall back to substring
    // heuristics on the raw text only if the JSON path doesn't yield one of
    // the known classes.
    if raw.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(kind) = v.get("kind").and_then(|k| k.as_str()) {
                match kind {
                    "rate_limit" => return "rate_limit",
                    "retry" => return "retry",
                    "dependency" => return "dependency",
                    "user" => return "user",
                    "deploy" => return "deploy",
                    "stale" => return "stale",
                    _ => {} // unknown kind → fall through to heuristics below
                }
            }
        }
    }
    let r = raw.to_lowercase();
    if r.contains("rate limit")
        || r.contains("ratelimit")
        || r.contains("429")
        || r.contains("rate_limit")
    {
        "rate_limit"
    } else if r.contains("retry") || r.contains("again") || r.contains("transient") {
        "retry"
    } else if r.contains("depend") || r.contains("blocked by") || r.contains("waiting on") {
        "dependency"
    } else if r.contains("user")
        || r.contains("human")
        || r.contains("approval")
        || r.contains("approve")
    {
        "user"
    } else if r.contains("deploy") || r.contains("release") || r.contains("production") {
        "deploy"
    } else if r.contains("stale")
        || r.contains("old")
        || r.contains("timeout")
        || r.contains("timed out")
    {
        "stale"
    } else {
        "unknown"
    }
}

fn task_updated_epoch(row: &Row) -> Option<i64> {
    match row {
        Row::Task(t) => parse_epoch(&t.updated_at),
        Row::Obs(_) | Row::CollapsedObs(_) | Row::Review(_) | Row::Intake(_) => None,
    }
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn parse_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(v);
    }
    let date = s.get(0..10)?;
    let mut parts = date.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    let (hh, mm, ss) = if let Some(time) = s.get(11..19) {
        let mut t = time.split(':');
        (
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };
    let days = days_from_civil(y, m, d)?;
    Some(days * SECS_PER_DAY + hh * 3600 + mm * 60 + ss)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = y - (m <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m as i32 + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    fn task(status: &str) -> Row {
        task_at(status, NOW.to_string(), None)
    }

    fn task_at(status: &str, updated_at: impl Into<String>, blocked_reason: Option<&str>) -> Row {
        let updated_at = updated_at.into();
        Row::Task(TaskRow {
            display_id: format!("T-{status}-{updated_at}"),
            status: status.to_string(),
            title: "t".to_string(),
            claimed_by: None,
            updated_at,
            tier_hint: None,
            linked_observations: Vec::new(),
            blocked_reason: blocked_reason.map(str::to_string),
            blocked_reason_class: Some(blocked_reason_class(blocked_reason).to_string()),
            ..Default::default()
        })
    }

    #[test]
    fn primary_task_steps_keep_explicit_none() {
        let row = TaskRow {
            status: "planning".to_string(),
            active_step: Some("none".to_string()),
            integration_step: Some("none".to_string()),
            ..Default::default()
        };
        assert_eq!(task_active_step(&row), "none");
        assert_eq!(task_integration_step(&row), "none");
    }

    fn task_with_id(id: &str, status: &str, updated_at: i64) -> Row {
        Row::Task(TaskRow {
            display_id: id.to_string(),
            status: status.to_string(),
            title: "t".to_string(),
            updated_at: updated_at.to_string(),
            blocked_reason_class: Some("unknown".to_string()),
            ..Default::default()
        })
    }

    fn obs(status: &str, contract: Option<&str>) -> Row {
        Row::Obs(ObsRow {
            display_id: format!("L-{status}"),
            status: status.to_string(),
            priority: "normal".to_string(),
            summary: "s".to_string(),
            updated_at: String::new(),
            contract_state: contract.map(str::to_string),
            tier_hint: None,
            investigation_failure_reason: None,
            ..Default::default()
        })
    }

    fn obs_with_id(id: &str, summary: &str, priority: &str, contract: Option<&str>) -> Row {
        Row::Obs(ObsRow {
            display_id: id.to_string(),
            status: "open".to_string(),
            priority: priority.to_string(),
            summary: summary.to_string(),
            updated_at: id.to_string(),
            contract_state: contract.map(str::to_string),
            ..Default::default()
        })
    }

    fn bucket(buckets: &[(Section, Vec<usize>)], section: Section) -> Vec<usize> {
        buckets
            .iter()
            .find(|(s, _)| *s == section)
            .unwrap()
            .1
            .clone()
    }

    #[test]
    fn dedup_collapses_same_summary_per_section_only() {
        let rows = vec![
            obs_with_id("L010", "same summary", "normal", None),
            obs_with_id("L009", "same summary", "normal", None),
            obs_with_id("L001", "same summary", "high", None),
            obs_with_id("L002", "same summary", "high", None),
        ];
        let sections = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        let (deduped, buckets) = dedup_observation_summaries_by_section(&rows, &sections);
        let other = bucket(&buckets, Section::ObsOther);
        let priority = bucket(&buckets, Section::ObsOpenNoContract);
        assert_eq!(other.len(), 1);
        assert_eq!(priority.len(), 1);
        assert_eq!(deduped.len(), 2);
        match &deduped[other[0]] {
            Row::CollapsedObs(c) => {
                assert_eq!(c.section, Section::ObsOther);
                assert_eq!(c.summary, "same summary");
                assert_eq!(c.count, 2);
                assert_eq!(c.primary_display_id, "L009");
            }
            got => panic!("expected collapsed obs, got {got:?}"),
        }
        match &deduped[priority[0]] {
            Row::CollapsedObs(c) => {
                assert_eq!(c.section, Section::ObsOpenNoContract);
                assert_eq!(c.count, 2);
                assert_eq!(c.primary_display_id, "L001");
            }
            got => panic!("expected collapsed obs, got {got:?}"),
        }
    }

    #[test]
    fn dedup_collapses_76_row_cluster_to_primary_lexicographic_id() {
        let mut rows = Vec::new();
        for i in (0..76).rev() {
            rows.push(obs_with_id(
                &format!("L{:03}", i),
                "dupe cluster",
                "normal",
                None,
            ));
        }
        let sections = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        let (deduped, buckets) = dedup_observation_summaries_by_section(&rows, &sections);
        let other = bucket(&buckets, Section::ObsOther);
        assert_eq!(other.len(), 1);
        match &deduped[other[0]] {
            Row::CollapsedObs(c) => {
                assert_eq!(c.count, 76);
                assert_eq!(c.primary_display_id, "L000");
                assert_eq!(c.display_ids.len(), 76);
            }
            got => panic!("expected collapsed obs, got {got:?}"),
        }
    }

    #[test]
    fn section_all_labels_match_contract_order_and_are_unique() {
        let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
        let expected = vec![
            "QUEUED",
            "ACTIVE",
            "RATIFY-U1",
            "AWAITING HUMAN ACCEPTANCE",
            "INTEGRATION",
            "INTEGRATED",
            "HELD-INTEGRATION",
            "HELD-BLOCKED",
            "HELD-DEPLOY",
            "HELD-TRIAGE",
            "HELD-INTAKE",
            "HELD-AI-REVIEW",
            "HELD-ZOMBIE",
            "DONE",
            "PRIORITY",
            "OBSERVATIONS",
            "INTAKE-OPEN",
            "INTAKE-ROUTED",
            "EXTERNAL-REVIEW",
        ];
        assert_eq!(labels, expected);
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn section_classification() {
        // blocked/deploy_blocked with no blocked_reason → unknown class → NeedsTriage section.
        let rows = vec![
            task("plan_review"),        // idx 0 → ACTIVE WORK (ADR 0001 §3)
            task("blocked"),            // idx 1 → HELD-TRIAGE (no reason → unknown)
            task("deploy_blocked"),     // idx 2 → HELD-TRIAGE (no reason → unknown)
            task("accepted"),           // idx 3 → TERMINAL
            obs("open", Some("ready")), // idx 4 → RATIFY-U1
            obs("open", None),          // idx 5 → OBSERVATIONS
            obs("resolved", None),      // idx 6 → hidden historical noise
        ];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(buckets.len(), Section::ALL.len());
        assert_eq!(
            buckets.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            Section::ALL.to_vec()
        );
        let b = |sec: Section| -> Vec<usize> {
            buckets.iter().find(|(s, _)| *s == sec).unwrap().1.clone()
        };
        assert_eq!(b(Section::TasksQueued), Vec::<usize>::new());
        assert_eq!(b(Section::TasksActionableCurrentWork), vec![0usize]);
        assert_eq!(b(Section::TasksBlockedNeedsAction), Vec::<usize>::new());
        assert_eq!(b(Section::TasksDeployRecovery), Vec::<usize>::new());
        assert_eq!(b(Section::TasksNeedsTriage), vec![1usize, 2]);
        assert_eq!(b(Section::TasksRecentlyTerminal), vec![3usize]);
        assert_eq!(b(Section::ObsRatifiable), vec![4usize]);
        assert_eq!(b(Section::TasksHeldAiReview), Vec::<usize>::new());
        assert_eq!(b(Section::ObsOpenNoContract), Vec::<usize>::new());
        assert_eq!(b(Section::ObsOther), vec![5usize]);
    }

    #[test]
    fn task_status_mapping_is_exhaustive() {
        // blocked/deploy_blocked with no reason → unknown class → TasksNeedsTriage.
        // Use an explicit recoverable reason to get TasksBlockedNeedsAction / TasksDeployRecovery.
        let mappings: &[(&str, Option<&str>, Section)] = &[
            ("planning", None, Section::TasksActionableCurrentWork),
            ("plan_review", None, Section::TasksActionableCurrentWork),
            ("ready", None, Section::TasksActionableCurrentWork),
            ("executing", None, Section::TasksActionableCurrentWork),
            ("code_review", None, Section::TasksActionableCurrentWork),
            (
                "blocked",
                Some("rate_limit 429"),
                Section::TasksBlockedNeedsAction,
            ),
            ("blocked", None, Section::TasksNeedsTriage),
            ("complete", None, Section::TasksRecentlyTerminal),
            ("in_review", None, Section::TasksAcceptU3),
            ("accepted", None, Section::TasksRecentlyTerminal),
            ("rejected", None, Section::TasksRecentlyTerminal),
            (
                "deploy_blocked",
                Some("retry-deploy-recoverable"),
                Section::TasksDeployRecovery,
            ),
            ("deploy_blocked", None, Section::TasksNeedsTriage),
            ("closed_out_of_band", None, Section::TasksRecentlyTerminal),
            ("cargo_installed", None, Section::TasksRecentlyTerminal),
            ("schema_migrated", None, Section::TasksRecentlyTerminal),
            ("abandoned", None, Section::TasksRecentlyTerminal),
            ("integration_queued", None, Section::TasksIntegration),
            ("integrating", None, Section::TasksIntegration),
            ("integrated", None, Section::TasksIntegratedAwaitingPostLand),
            (
                "integration_blocked",
                None,
                Section::TasksIntegrationBlocked,
            ),
        ];
        for (status, reason, expected) in mappings {
            let r = task_at(status, NOW.to_string(), *reason);
            assert_eq!(
                section_for(&r),
                Some(*expected),
                "task status {status} reason {reason:?}"
            );
        }
    }

    #[test]
    fn primary_queued_lifecycle_classifies_to_queued_section() {
        let row = Row::Task(TaskRow {
            display_id: "T-queued-primary".to_string(),
            status: "executing".to_string(),
            title: "queued by primary lifecycle".to_string(),
            lifecycle: Some("queued".to_string()),
            active_step: Some("none".to_string()),
            integration_step: Some("none".to_string()),
            blocked: Some(false),
            blocker_kind: Some("none".to_string()),
            updated_at: NOW.to_string(),
            ..Default::default()
        });
        let rows = vec![row];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(bucket(&buckets, Section::TasksQueued), vec![0usize]);
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
        assert_eq!(section_for(&rows[0]), Some(Section::TasksQueued));
    }

    #[test]
    fn live_running_agent_overrides_capacity_blocker_in_watch_classification() {
        let row = Row::Task(TaskRow {
            display_id: "T-live-capacity".to_string(),
            status: "planning".to_string(),
            title: "planner is already running".to_string(),
            lifecycle: Some("active".to_string()),
            active_step: Some("planning".to_string()),
            integration_step: Some("none".to_string()),
            blocked: Some(true),
            blocker_kind: Some("capacity".to_string()),
            updated_at: NOW.to_string(),
            live_run: Some(LiveRunSummary {
                role: "planner".to_string(),
                runner: Some("claude-code:opus".to_string()),
                status: Some("running".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let rows = vec![row];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(
            bucket(&buckets, Section::TasksActionableCurrentWork),
            vec![0usize]
        );
        assert!(bucket(&buckets, Section::TasksNeedsTriage).is_empty());
        match &rows[0] {
            Row::Task(t) => assert!(!task_is_blocked(t)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn primary_lifecycle_overrides_terminal_compat_status() {
        let row = Row::Task(TaskRow {
            display_id: "T-queued-compat".to_string(),
            status: "accepted".to_string(),
            title: "queued primary beats legacy terminal status".to_string(),
            lifecycle: Some("queued".to_string()),
            active_step: Some("none".to_string()),
            integration_step: Some("none".to_string()),
            blocked: Some(false),
            blocker_kind: Some("none".to_string()),
            updated_at: NOW.to_string(),
            ..Default::default()
        });
        assert_eq!(section_for(&row), Some(Section::TasksQueued));
    }

    #[test]
    fn integration_queued_and_integrating_classify_to_tasks_integration() {
        // AC4.4: integration_queued / integrating live in TasksIntegration —
        // they MUST NOT appear in the ordinary ACTIVE WORK bucket.
        let rows = vec![task("integration_queued"), task("integrating")];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(bucket(&buckets, Section::TasksIntegration), vec![0usize, 1]);
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
        for (status, idx) in [("integration_queued", 0usize), ("integrating", 1)] {
            let r = task(status);
            assert_eq!(
                section_for(&r),
                Some(Section::TasksIntegration),
                "{status} (idx {idx}) must classify to TasksIntegration"
            );
        }
    }

    #[test]
    fn integrated_classifies_to_tasks_integrated_awaiting_post_land() {
        let rows = vec![task("integrated")];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(
            bucket(&buckets, Section::TasksIntegratedAwaitingPostLand),
            vec![0usize]
        );
        assert!(bucket(&buckets, Section::TasksRecentlyTerminal).is_empty());
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
    }

    #[test]
    fn integration_blocked_classifies_to_tasks_integration_blocked() {
        // Mirrors HELD-DEPLOY / HELD-BLOCKED: the row is awaiting a
        // human-authorized retry-integration. Must NOT collapse into the
        // generic ACTIVE WORK or HELD-BLOCKED buckets.
        let rows = vec![task("integration_blocked")];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(
            bucket(&buckets, Section::TasksIntegrationBlocked),
            vec![0usize]
        );
        assert!(bucket(&buckets, Section::TasksBlockedNeedsAction).is_empty());
        assert!(bucket(&buckets, Section::TasksDeployRecovery).is_empty());
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
    }

    #[test]
    fn rejected_not_actionable_and_closed_out_of_band_terminal() {
        let rows = vec![task("rejected"), task("closed_out_of_band")];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
        assert_eq!(bucket(&buckets, Section::TasksRecentlyTerminal).len(), 2);
        assert!(bucket(&buckets, Section::TasksRecentlyTerminal).contains(&0));
        assert!(bucket(&buckets, Section::TasksRecentlyTerminal).contains(&1));
    }

    #[test]
    fn stale_terminal_rows_are_hidden_by_default() {
        let old = NOW - 49 * 3600;
        let rows = vec![
            task_with_id("T-schema", "schema_migrated", old),
            task_with_id("T-closed", "closed_out_of_band", old),
            task_with_id("T-rejected", "rejected", old),
            task_with_id("T-accepted", "accepted", old),
            task_with_id("T-complete", "complete", old),
            task_with_id("T-cargo", "cargo_installed", old),
        ];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        for sec in [
            Section::TasksActionableCurrentWork,
            Section::TasksBlockedNeedsAction,
            Section::TasksDeployRecovery,
            Section::TasksNeedsTriage,
            Section::TasksRecentlyTerminal,
        ] {
            assert_eq!(bucket(&buckets, sec).len(), 0, "{sec:?} visibility");
        }
    }

    #[test]
    fn terminal_rows_are_capped_by_default_and_all_history_uncaps() {
        let rows: Vec<Row> = (0..7)
            .map(|i| task_with_id(&format!("T{i}"), "accepted", NOW - i * 60))
            .collect();
        let default_buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(
            bucket(&default_buckets, Section::TasksRecentlyTerminal),
            vec![0, 1, 2, 3, 4]
        );

        let all_buckets = classify_with_options_at(
            &rows,
            WatchClassifyOptions {
                show_all_history: true,
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(
            bucket(&all_buckets, Section::TasksRecentlyTerminal),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn blocked_reason_classes_cover_first_pass_examples() {
        let cases = [
            (Some("hit rate limit 429"), "rate_limit"),
            (Some("retry after transient failure"), "retry"),
            (Some("waiting on dependency T123"), "dependency"),
            (Some("needs human approval"), "user"),
            (Some("deploy window closed"), "deploy"),
            (Some("stale lock timed out"), "stale"),
            (Some(""), "unknown"),
            (Some("opaque"), "unknown"),
            (None, "unknown"),
        ];
        for (reason, expected) in cases {
            assert_eq!(blocked_reason_class(reason), expected, "{reason:?}");
        }
    }

    fn cockpit_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                display_id TEXT, status TEXT, title TEXT, claimed_by TEXT, updated_at TEXT,
                tier_hint TEXT, linked_observations TEXT, blocked_reason TEXT,
                lifecycle TEXT, active_step TEXT, integration_step TEXT, blocked INTEGER, blocker_kind TEXT,
                current_phase INTEGER, current_cycle INTEGER, plan_source TEXT, contract TEXT,
                plan TEXT, plan_review_log TEXT, cycles TEXT, wrap_log TEXT, branch TEXT, workspace_path TEXT
            );
            CREATE TABLE observations (
                display_id TEXT, status TEXT, priority TEXT, summary TEXT, updated_at TEXT,
                body TEXT, source TEXT, task_id TEXT, priority_rank INTEGER, intent_contract TEXT,
                locked_by TEXT, locked_at TEXT, lock_reason TEXT, evidence TEXT, resolution TEXT, investigation_failure_reason TEXT
            );
            "#,
        ).unwrap();
        conn
    }

    #[test]
    fn parse_epoch_accepts_rfc3339_prefix() {
        assert_eq!(parse_epoch("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn load_system_health_counts_unfinished_dispatch_locks_and_oldest_claimed_at() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE dispatch_locks (display_id TEXT, claimed_at INTEGER, finished_at INTEGER);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D999', 1700009999, 1700019999);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D001', 1700000001, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D002', 1700000002, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D003', 1700000003, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D004', 1700000004, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D005', 1700000005, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D006', 1700000006, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D007', 1700000007, NULL);
            INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES ('D008', 1700000008, NULL);
            "#,
        )
        .unwrap();

        let health = load_system_health(&conn).unwrap();
        assert_eq!(health.unfinished_dispatch_locks, 8);
        assert_eq!(health.oldest_claimed_at_epoch, Some(1_700_000_001));
    }

    #[test]
    fn load_system_health_absent_table_or_claimed_at_returns_zero() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(load_system_health(&conn).unwrap(), SystemHealth::default());
        conn.execute_batch("CREATE TABLE dispatch_locks (finished_at INTEGER);")
            .unwrap();
        assert_eq!(load_system_health(&conn).unwrap(), SystemHealth::default());
    }

    #[test]
    fn load_rows_gracefully_absent_intake_and_external_review_tables() {
        let conn = cockpit_conn();
        conn.execute("INSERT INTO tasks (display_id,status,title,updated_at,linked_observations) VALUES ('T001','executing','task','2026-05-01','[]')", []).unwrap();
        conn.execute("INSERT INTO observations (display_id,status,priority,summary,updated_at) VALUES ('L001','open','high','obs','2026-05-01')", []).unwrap();

        let rows = load_rows(&conn).unwrap();
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Task(_))).count(), 1);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Obs(_))).count(), 1);
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(
            task.live_run.is_none(),
            "missing workspace/marker should not create live section"
        );
        assert!(matches!(
            load_external_review_state(&conn).unwrap(),
            ExternalReviewState::Unavailable { .. }
        ));
    }

    #[test]
    fn load_rows_adds_live_run_summary_and_suppresses_heartbeat_noise() {
        let conn = cockpit_conn();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("wt");
        let runs = workspace.join(".stores/runs");
        std::fs::create_dir_all(runs.join("sess")).unwrap();
        std::fs::write(
            runs.join("current-T777-planner.json"),
            serde_json::json!({
                "display_id":"T777",
                "role":"planner",
                "runner":"claude-code:opus",
                "status":"running",
                "session_id":"sess",
                "updated_at":"2026-05-11T00:00:00Z",
                "events_path": runs.join("sess/events.jsonl"),
                "status_path": runs.join("sess/status.json")
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            runs.join("sess/status.json"),
            r#"{"last_event_at":"2026-05-11T00:00:07Z","last_event_type":"tool_start","current_activity":"tool:bash"}"#,
        ).unwrap();
        let mut event_lines = Vec::new();
        event_lines.push(r#"{"type":"heartbeat","ts":"2026-05-11T00:00:01Z"}"#.to_string());
        for i in 0..6 {
            event_lines.push(
                serde_json::json!({
                    "type":"assistant_text",
                    "ts": format!("2026-05-11T00:00:0{}Z", i + 2),
                    "text": format!("message {i}")
                })
                .to_string(),
            );
        }
        event_lines.push("{not json".to_string());
        std::fs::write(runs.join("sess/events.jsonl"), event_lines.join("\n")).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,workspace_path) VALUES ('T777','planning','task','2026-05-01','[]',?1)",
            [workspace.to_string_lossy().to_string()],
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        let live = task.live_run.as_ref().expect("live summary");
        assert_eq!(live.role, "planner");
        assert_eq!(live.runner.as_deref(), Some("claude-code:opus"));
        assert_eq!(live.current_activity.as_deref(), Some("tool:bash"));
        assert_eq!(
            live.events.len(),
            5,
            "sliding window keeps last five meaningful events"
        );
        assert!(
            live.events.iter().all(|e| e.event_type != "heartbeat"),
            "heartbeat noise suppressed: {:?}",
            live.events
        );
        assert_eq!(live.events[0].text, "message 1");
        assert_eq!(live.events[4].text, "message 5");
    }

    #[test]
    fn live_run_summary_scans_past_heartbeat_only_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let events = tmp.path().join("events.jsonl");
        let mut event_lines = Vec::new();
        for i in 0..5 {
            event_lines.push(
                serde_json::json!({
                    "type":"assistant_text",
                    "ts": format!("2026-05-11T00:00:0{i}Z"),
                    "text": format!("message {i}")
                })
                .to_string(),
            );
        }
        let heartbeat = r#"{"type":"heartbeat","ts":"2026-05-11T00:00:09Z"}"#;
        while event_lines.join("\n").len() < (LIVE_EVENTS_READ_BYTES as usize + 1024) {
            event_lines.push(heartbeat.to_string());
        }
        std::fs::write(&events, event_lines.join("\n")).unwrap();

        let summaries = read_live_event_summaries(&events);
        assert_eq!(summaries.len(), 5);
        assert!(summaries.iter().all(|e| e.event_type == "assistant_text"));
        assert_eq!(summaries[0].text, "message 0");
        assert_eq!(summaries[4].text, "message 4");
    }

    #[test]
    fn completed_live_marker_is_not_rendered_as_active_runner() {
        let conn = cockpit_conn();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("wt");
        let runs = workspace.join(".stores/runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("current-T778-planner.json"),
            serde_json::json!({
                "display_id":"T778",
                "role":"planner",
                "runner":"claude-code:opus",
                "status":"completed",
                "session_id":"sess",
                "updated_at":"2026-05-11T00:00:00Z",
                "transcript_path": runs.join("sess.jsonl")
            })
            .to_string(),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,workspace_path) VALUES ('T778','planning','task','2026-05-01','[]',?1)",
            [workspace.to_string_lossy().to_string()],
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(
            task.live_run.is_none(),
            "completed marker must not render as active Live runner"
        );
    }

    #[test]
    fn oversized_live_marker_is_ignored_without_error() {
        let conn = cockpit_conn();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("wt");
        let runs = workspace.join(".stores/runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("current-T779-planner.json"),
            "{".to_string() + &"x".repeat((LIVE_MARKER_STATUS_READ_BYTES as usize) + 1),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,workspace_path) VALUES ('T779','planning','task','2026-05-01','[]',?1)",
            [workspace.to_string_lossy().to_string()],
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(task.live_run.is_none());
    }

    #[test]
    fn live_tool_start_summary_includes_path_and_args() {
        let summary = summarize_live_event(&serde_json::json!({
            "type": "tool_start",
            "ts": "2026-05-11T00:00:00Z",
            "name": "Read",
            "path": "docs/adr/0002.md",
            "args_preview": "limit=120"
        }))
        .unwrap();
        assert_eq!(summary.label, "tool_start");
        assert!(summary.text.contains("Read"), "{}", summary.text);
        assert!(
            summary.text.contains("docs/adr/0002.md"),
            "{}",
            summary.text
        );
        assert!(summary.text.contains("limit=120"), "{}", summary.text);
    }

    #[test]
    fn live_event_summary_truncates_long_text() {
        let long = "x".repeat(160);
        let summary = summarize_live_event(&serde_json::json!({
            "type": "assistant_text",
            "ts": "2026-05-11T00:00:00Z",
            "text": long
        }))
        .unwrap();
        assert_eq!(summary.label, "assistant");
        assert!(summary.text.chars().count() <= LIVE_TEXT_LIMIT);
        assert!(
            summary.text.ends_with('…'),
            "expected ellipsis: {}",
            summary.text
        );
    }

    #[test]
    fn fixture_t3_task_loads_phase_counts_and_first_class_locations() {
        let conn = cockpit_conn();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,tier_hint,linked_observations,current_phase,current_cycle,plan_source,contract,plan,branch,workspace_path) VALUES (?1,?2,?3,?4,?5,'[]',?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params!["T003", "executing", "three phase", "2026-05-01", "T3", 2i64, 3i64, "planner_authored", r#"{"done_when":"done"}"#, r#"{"phases":[{"name":"a"},{"name":"b"},{"name":"c"}]}"#, "feat/T003", "/tmp/T003"],
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(task.total_phases, Some(3));
        assert_eq!(task.current_phase, Some(2));
        assert_eq!(task.current_cycle, Some(3));
        assert_eq!(task.tier_hint.as_deref(), Some("T3"));
        assert_eq!(task.branch.as_deref(), Some("feat/T003"));
        assert_eq!(task.workspace_path.as_deref(), Some("/tmp/T003"));
        assert!(task.artifact_pointers.is_empty());
    }

    #[test]
    fn task_structured_plan_reviews_and_cycles_round_trip() {
        let conn = cockpit_conn();
        let plan_review_log = r#"[
            {"gate":"NEEDS_WORK","summary":"tighten scope","at":"2026-05-01T00:00:00Z"},
            {"gate":"READY","summary":"approved","at":"2026-05-01T01:00:00Z"},
            {"gate":"SURPRISE","summary":"future gate","timestamp":"2026-05-01T02:00:00Z"}
        ]"#;
        let cycles = r#"[
            {"phase":1,"cycle":1,"executor":{"summary":"built phase one","at":"2026-05-01T03:00:00Z"},"review":{"gate":"REVISE","summary":"needs fix","at":"2026-05-01T04:00:00Z"}},
            {"phase":1,"cycle":2,"executor":{"summary":"fixed phase one"},"review":{"gate":"PASS","summary":"passed"}},
            {"phase":2,"cycle":1,"executor":{"summary":"built phase two"},"review":null}
        ]"#;
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,plan_review_log,cycles) VALUES ('T-map','executing','task','2026-05-01','[]',?1,?2)",
            (plan_review_log, cycles),
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();

        assert_eq!(task.plan_review_entries.len(), 3);
        assert_eq!(task.plan_review_entries[0].gate, PlanReviewGate::NeedsWork);
        assert_eq!(task.plan_review_entries[1].gate, PlanReviewGate::Ready);
        assert_eq!(
            task.plan_review_entries[2].gate,
            PlanReviewGate::Unknown("SURPRISE".to_string())
        );
        assert_eq!(
            task.plan_review_entries[2].at.as_deref(),
            Some("2026-05-01T02:00:00Z")
        );
        assert_eq!(
            task.plan_review_summaries,
            vec!["tighten scope", "approved", "future gate"]
        );

        assert_eq!(task.cycle_entries.len(), 3);
        assert_eq!(task.cycle_entries[0].phase, 1);
        assert_eq!(task.cycle_entries[0].cycle, 1);
        assert_eq!(
            task.cycle_entries[0].review_gate,
            Some(CycleReviewGate::Revise)
        );
        assert_eq!(
            task.cycle_entries[1].review_gate,
            Some(CycleReviewGate::Pass)
        );
        assert_eq!(task.cycle_entries[2].review_gate, None);
        assert_eq!(
            task.cycle_entries[2].executor_summary.as_deref(),
            Some("built phase two")
        );
        assert!(task
            .cycle_summaries
            .contains(&"executor: built phase one".to_string()));
        assert!(task.cycle_summaries.contains(&"review: passed".to_string()));
    }

    #[test]
    fn task_structured_evidence_degrades_on_missing_or_malformed_json() {
        let conn = cockpit_conn();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,plan_review_log,cycles) VALUES ('T-bad','executing','bad','2026-05-01','[]','not-json','{')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,plan_review_log,cycles) VALUES ('T-missing','executing','missing','2026-05-01','[]',?1,?2)",
            (r#"[{"summary":"no gate"}]"#, r#"[{"executor":{"summary":"no phase or cycle"},"review":{"gate":"ODD"}}]"#),
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let tasks: Vec<&TaskRow> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .collect();

        let bad = tasks.iter().find(|t| t.display_id == "T-bad").unwrap();
        assert!(bad.plan_review_entries.is_empty());
        assert!(bad.cycle_entries.is_empty());

        let missing = tasks.iter().find(|t| t.display_id == "T-missing").unwrap();
        assert_eq!(missing.plan_review_entries.len(), 1);
        assert_eq!(
            missing.plan_review_entries[0].gate,
            PlanReviewGate::Unknown(String::new())
        );
        assert_eq!(missing.cycle_entries.len(), 1);
        assert_eq!(missing.cycle_entries[0].phase, 0);
        assert_eq!(missing.cycle_entries[0].cycle, 0);
        assert_eq!(
            missing.cycle_entries[0].review_gate,
            Some(CycleReviewGate::Unknown("ODD".to_string()))
        );
    }

    #[test]
    fn intake_row_loads_when_table_exists_and_classifies_held() {
        let conn = cockpit_conn();
        conn.execute_batch(
            r#"
            CREATE TABLE intake (
                display_id TEXT, status TEXT, summary TEXT, body TEXT, captured_at TEXT,
                source_task TEXT, source_agent TEXT, risk_flags TEXT, cluster_key TEXT, decision TEXT,
                missing_info_question TEXT, routed_to_observation TEXT, routed_to_arch_review TEXT, duplicate_of TEXT, evidence TEXT
            );
            INSERT INTO intake (display_id,status,summary,body,captured_at,source_task,source_agent,risk_flags,cluster_key,missing_info_question,evidence)
            VALUES ('I001','needs_info','needs more','story','2026-05-01','T001','executor','["touches_lifecycle"]','watch','what is missing?','path:line');
            "#,
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let intake = rows
            .iter()
            .find_map(|r| match r {
                Row::Intake(i) => Some(i),
                _ => None,
            })
            .unwrap();
        assert_eq!(intake.display_id, "I001");
        assert_eq!(intake.summary, "needs more");
        assert_eq!(intake.status, "needs_info");
        assert_eq!(intake.source_task.as_deref(), Some("T001"));
        assert_eq!(intake.risk_flags, vec!["touches_lifecycle"]);
        assert_eq!(intake.held_reason.as_deref(), Some("what is missing?"));
        assert_eq!(
            section_for(&Row::Intake(intake.clone())),
            Some(Section::IntakeHeld)
        );
    }

    #[test]
    fn intake_waiting_kind_evidence_needed_counts_as_held() {
        let rows = vec![Row::Intake(IntakeRow {
            display_id: "I002".into(),
            status: "needs_info".into(),
            summary: "needs evidence".into(),
            lifecycle: Some("waiting".into()),
            waiting_kind: Some("evidence_needed".into()),
            ..Default::default()
        })];
        let model = cockpit_model(&rows, ExternalReviewState::default());
        assert_eq!(model.held, 1);
    }

    #[test]
    fn observation_load_prefers_primary_contract_state_column() {
        let conn = cockpit_conn();
        conn.execute_batch(
            "ALTER TABLE observations ADD COLUMN contract_state TEXT;
             INSERT INTO observations (display_id,status,priority,summary,updated_at,intent_contract,contract_state) \
             VALUES ('L777','confirmed','normal','obs','2026-05-01', '{\"contract_state\":\"ready\"}', 'approved');"
        ).unwrap();
        let rows = load_rows(&conn).unwrap();
        let obs = rows
            .iter()
            .find_map(|r| match r {
                Row::Obs(o) if o.display_id == "L777" => Some(o),
                _ => None,
            })
            .expect("observation row loaded");
        assert_eq!(obs.contract_state.as_deref(), Some("approved"));
    }

    #[test]
    fn transition_history_recent_events_are_attached_newest_first() {
        let conn = cockpit_conn();
        conn.execute("INSERT INTO tasks (display_id,status,title,updated_at,linked_observations) VALUES ('T009','executing','task','2026-05-01','[]')", []).unwrap();
        conn.execute_batch(
            "CREATE TABLE transition_history (id INTEGER PRIMARY KEY, store TEXT, display_id TEXT, from_status TEXT, to_status TEXT, verb TEXT, occurred_at TEXT);
             INSERT INTO transition_history (store,display_id,from_status,to_status,verb,occurred_at) VALUES ('tasks','T009','ready','executing','start','2026-05-01');
             INSERT INTO transition_history (store,display_id,from_status,to_status,verb,occurred_at) VALUES ('tasks','T009','executing','code_review','submit-execute','2026-05-02');"
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(task.recent_events.len(), 2);
        assert_eq!(
            task.recent_events[0].verb.as_deref(),
            Some("submit-execute")
        );
    }

    fn intake_row(status: &str) -> Row {
        Row::Intake(IntakeRow {
            display_id: format!("I-{status}"),
            status: status.to_string(),
            summary: "s".to_string(),
            updated_at: NOW.to_string(),
            ..Default::default()
        })
    }

    fn obs_row(status: &str) -> Row {
        Row::Obs(ObsRow {
            display_id: format!("L-{status}"),
            status: status.to_string(),
            priority: "normal".to_string(),
            summary: "s".to_string(),
            updated_at: NOW.to_string(),
            ..Default::default()
        })
    }

    fn review_row(status: &str) -> Row {
        Row::Review(ReviewRow {
            display_id: format!("E-{status}"),
            task_id: "T001".to_string(),
            status: status.to_string(),
            runner: "codex".to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn store_lane_all_has_five_lanes_in_canonical_order() {
        assert_eq!(StoreLane::ALL.len(), 5);
        assert_eq!(
            StoreLane::ALL,
            [
                StoreLane::Intake,
                StoreLane::Observations,
                StoreLane::Tasks,
                StoreLane::ExternalReviews,
                StoreLane::EngineHealth,
            ]
        );
    }

    #[test]
    fn store_lane_for_row_covers_each_row_variant() {
        assert_eq!(store_lane_for_row(&intake_row("draft")), StoreLane::Intake);
        assert_eq!(
            store_lane_for_row(&obs_row("open")),
            StoreLane::Observations
        );
        let collapsed = Row::CollapsedObs(CollapsedObsRow {
            section: Section::ObsOther,
            summary: "dupe".to_string(),
            count: 3,
            primary_display_id: "L001".to_string(),
            display_ids: vec!["L001".to_string(), "L002".to_string(), "L003".to_string()],
            representative: ObsRow {
                display_id: "L001".to_string(),
                status: "open".to_string(),
                ..Default::default()
            },
        });
        assert_eq!(store_lane_for_row(&collapsed), StoreLane::Observations);
        assert_eq!(store_lane_for_row(&task("executing")), StoreLane::Tasks);
        assert_eq!(
            store_lane_for_row(&review_row("running")),
            StoreLane::ExternalReviews,
        );
    }

    #[test]
    fn store_flow_model_counts_mixed_rows_per_lane() {
        let rows = vec![
            intake_row("draft"),
            intake_row("draft"),
            intake_row("triaging"),
            intake_row("needs_info"),
            intake_row("routed"),
            intake_row("dropped"),
            obs_row("open"),
            obs_row("investigating"),
            obs_row("ready"),
            obs_row("resolved"),
            obs_row("wont_fix"),
            obs_row("investigation_failed"),
            task("planning"),
            task("ready"),
            task("executing"),
            task("blocked"),
            task("deploy_blocked"),
            task("plan_review"),
            task("code_review"),
            task("in_review"),
            task("accepted"),
            task("complete"),
            task("rejected"),
            review_row("pending"),
            review_row("running"),
            review_row("passed"),
            review_row("revise"),
            review_row("tooling_held"),
        ];
        let health = SystemHealth::default();
        let daemon = super::super::daemon::Liveness::Live { pid: 4242 };
        let model = store_flow_model(&rows, &health, &daemon, &ExternalReviewState::default());

        assert_eq!(model.intake.new, 2);
        assert_eq!(model.intake.triaging, 1);
        assert_eq!(model.intake.waiting, 1);
        assert_eq!(model.intake.closed, 2);

        assert_eq!(model.observations.candidate, 1);
        assert_eq!(model.observations.in_progress, 2);
        assert_eq!(model.observations.ready, 0);
        assert_eq!(model.observations.closed, 2);
        assert_eq!(model.observations.errors, 1);

        assert_eq!(model.tasks.queued, 0);
        assert_eq!(model.tasks.work, 3);
        assert_eq!(model.tasks.gate, 3);
        assert_eq!(model.tasks.wait, 0);
        assert_eq!(model.tasks.fail, 2);
        assert_eq!(model.tasks.recently_terminal, 3);

        assert_eq!(model.external_reviews.pending, 1);
        assert_eq!(model.external_reviews.running, 1);
        assert_eq!(model.external_reviews.passed, 1);
        assert_eq!(model.external_reviews.revise, 1);
        assert_eq!(model.external_reviews.tooling_held, 1);

        assert!(model.engine.daemon_live);
        assert_eq!(model.engine.unfinished_locks, 0);
        assert_eq!(model.engine.oldest_lock_age_secs, None);
        assert_eq!(model.engine.agent_runs_recent, 0);
    }

    #[test]
    fn task_flow_model_counts_projection_slots_for_representative_rows() {
        let rows = vec![
            Row::Task(TaskRow {
                lifecycle: Some("queued".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                lifecycle: Some("active".to_string()),
                active_step: Some("coding".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                lifecycle: Some("active".to_string()),
                active_step: Some("planning_review".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                blocked: Some(true),
                blocker_kind: Some("capacity".to_string()),
                lifecycle: Some("queued".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                blocked: Some(true),
                blocker_kind: Some("runner".to_string()),
                lifecycle: Some("active".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                lifecycle: Some("done".to_string()),
                ..Default::default()
            }),
        ];
        let model = store_flow_model(
            &rows,
            &SystemHealth::default(),
            &super::super::daemon::Liveness::Dead,
            &ExternalReviewState::default(),
        );

        assert_eq!(model.tasks.queued, 1);
        assert_eq!(model.tasks.work, 1);
        assert_eq!(model.tasks.gate, 1);
        assert_eq!(model.tasks.wait, 1);
        assert_eq!(model.tasks.fail, 1);
        assert_eq!(model.tasks.recently_terminal, 1);
    }

    #[test]
    fn observation_flow_slots_are_mutually_exclusive_for_waiting_overlay() {
        let rows = vec![Row::Obs(ObsRow {
            display_id: "L-wait".to_string(),
            status: "open".to_string(),
            priority: "high".to_string(),
            summary: "waiting candidate".to_string(),
            waiting_kind: Some("info_needed".to_string()),
            ..Default::default()
        })];
        let model = store_flow_model(
            &rows,
            &SystemHealth::default(),
            &super::super::daemon::Liveness::Dead,
            &ExternalReviewState::default(),
        );
        assert_eq!(model.observations.candidate, 0);
        assert_eq!(model.observations.ready, 0);
        assert_eq!(model.observations.waiting_kinds.values().sum::<usize>(), 1);
    }

    #[test]
    fn observation_flow_slots_are_mutually_exclusive_for_contract_gate() {
        let rows = vec![Row::Obs(ObsRow {
            display_id: "L-contract".to_string(),
            status: "open".to_string(),
            priority: "high".to_string(),
            summary: "contract candidate".to_string(),
            waiting_kind: Some("human_ratification".to_string()),
            contract_state: Some("draft".to_string()),
            ..Default::default()
        })];
        let model = store_flow_model(
            &rows,
            &SystemHealth::default(),
            &super::super::daemon::Liveness::Dead,
            &ExternalReviewState::default(),
        );
        assert_eq!(model.observations.candidate, 0);
        assert_eq!(model.observations.ready, 1);
        assert_eq!(model.observations.waiting_kinds.values().sum::<usize>(), 0);
    }

    #[test]
    fn store_flow_model_collapsed_obs_uses_cluster_count() {
        let rep = ObsRow {
            display_id: "L001".to_string(),
            status: "open".to_string(),
            ..Default::default()
        };
        let rows = vec![Row::CollapsedObs(CollapsedObsRow {
            section: Section::ObsOther,
            summary: "dupe".to_string(),
            count: 7,
            primary_display_id: "L001".to_string(),
            display_ids: (0..7).map(|i| format!("L00{i}")).collect(),
            representative: rep,
        })];
        let model = store_flow_model(
            &rows,
            &SystemHealth::default(),
            &super::super::daemon::Liveness::Dead,
            &ExternalReviewState::default(),
        );
        assert_eq!(model.observations.candidate, 7);
        assert_eq!(model.observations.in_progress, 0);
        assert_eq!(model.observations.ready, 0);
        assert_eq!(model.observations.closed, 0);
        assert_eq!(model.observations.errors, 0);
    }

    #[test]
    fn store_flow_model_empty_rows_zero_counts_but_engine_reflects_inputs() {
        let health = SystemHealth {
            unfinished_dispatch_locks: 4,
            oldest_claimed_at_epoch: Some(NOW - 90),
        };
        let daemon = super::super::daemon::Liveness::Dead;
        let model =
            store_flow_model_at(&[], &health, &daemon, &ExternalReviewState::default(), NOW);
        assert_eq!(model.intake, IntakeFlow::default());
        assert_eq!(model.observations, ObsFlow::default());
        assert_eq!(model.tasks, TasksFlow::default());
        assert_eq!(model.external_reviews, ReviewsFlow::default());
        assert!(!model.engine.daemon_live);
        assert_eq!(model.engine.unfinished_locks, 4);
        assert_eq!(model.engine.oldest_lock_age_secs, Some(90));
        assert_eq!(model.engine.agent_runs_recent, 0);
    }

    #[test]
    fn engine_flow_dead_daemon_with_unfinished_locks_reflected() {
        let health = SystemHealth {
            unfinished_dispatch_locks: 3,
            oldest_claimed_at_epoch: Some(NOW - 600),
        };
        let model = store_flow_model_at(
            &[],
            &health,
            &super::super::daemon::Liveness::Dead,
            &ExternalReviewState::default(),
            NOW,
        );
        assert!(!model.engine.daemon_live);
        assert_eq!(model.engine.unfinished_locks, 3);
        assert_eq!(model.engine.oldest_lock_age_secs, Some(600));
    }

    #[test]
    fn recent_exhaust_caps_at_limit_and_orders_newest_first() {
        let now = now_epoch();
        let rows: Vec<Row> = vec![
            task_with_id("T1", "accepted", now - 300),
            task_with_id("T2", "complete", now - 100),
            task_with_id("T3", "rejected", now - 200),
            task_with_id("T4", "executing", now - 50),
            task_with_id("T5", "abandoned", now - 400),
            task_with_id("T6", "schema_migrated", now - 10),
        ];
        let exhaust = recent_exhaust(&rows, 3);
        assert_eq!(exhaust.len(), 3);
        let ids: Vec<&str> = exhaust.iter().map(|r| r.display_id()).collect();
        assert_eq!(ids, vec!["T6", "T2", "T3"]);

        // Non-task rows are excluded; in-flight tasks are excluded.
        let mut mixed = rows.clone();
        mixed.push(obs_row("open"));
        mixed.push(intake_row("draft"));
        mixed.push(review_row("pending"));
        let only_terminal = recent_exhaust(&mixed, 10);
        assert_eq!(only_terminal.len(), 5);
        for r in &only_terminal {
            match r {
                Row::Task(t) => assert!(is_terminal_task_status(&t.status)),
                other => panic!("non-task in recent_exhaust: {other:?}"),
            }
        }
    }

    #[test]
    fn recent_exhaust_zero_limit_returns_empty() {
        let rows = vec![task_with_id("T1", "accepted", NOW)];
        assert!(recent_exhaust(&rows, 0).is_empty());
    }

    #[test]
    fn parse_epoch_is_pub_crate_visible_from_super() {
        // Lock pub(crate) visibility so render.rs (sibling module) can call it.
        use super::parse_epoch;
        assert_eq!(parse_epoch("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn seeded_external_review_round_trips_extended_fields() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                display_id TEXT, status TEXT, title TEXT, claimed_by TEXT, updated_at TEXT,
                tier_hint TEXT, linked_observations TEXT, blocked_reason TEXT,
                lifecycle TEXT, active_step TEXT, integration_step TEXT, blocked INTEGER, blocker_kind TEXT,
                current_phase INTEGER, current_cycle INTEGER, plan TEXT, plan_source TEXT,
                contract TEXT, plan_review_log TEXT, cycles TEXT, wrap_log TEXT,
                branch TEXT, workspace_path TEXT
            );
            CREATE TABLE observations (
                display_id TEXT, status TEXT, priority TEXT, summary TEXT, updated_at TEXT,
                body TEXT, source TEXT, task_id TEXT, priority_rank INTEGER, intent_contract TEXT,
                locked_by TEXT, locked_at TEXT, lock_reason TEXT, evidence TEXT, resolution TEXT,
                investigation_failure_reason TEXT
            );
            CREATE TABLE external_reviews (
                id INTEGER PRIMARY KEY,
                display_id TEXT, task_id TEXT, status TEXT, runner TEXT,
                held_reason TEXT, next_retry_at TEXT, attempts INTEGER,
                verdict TEXT, base_sha TEXT, head_sha TEXT, log_path TEXT,
                transcript_path TEXT, started_at TEXT, completed_at TEXT, duration_ms INTEGER,
                critical_count INTEGER, major_count INTEGER, minor_count INTEGER, findings TEXT
            );
            INSERT INTO external_reviews (
                display_id, task_id, status, runner, attempts,
                verdict, base_sha, head_sha, log_path, transcript_path,
                started_at, completed_at, duration_ms,
                critical_count, major_count, minor_count, findings
            ) VALUES (
                'ER001', 'T100', 'running', 'codex', 2,
                'REVISE', 'abc123', 'def456', '/tmp/log', '/tmp/transcript',
                '2026-05-09T01:00:00', '2026-05-09T01:05:00', 300000,
                1, 2, 3, '[{"severity":"critical"},{"severity":"major"},{"severity":"major"},{"severity":"minor"}]'
            );
            "#,
        )
        .unwrap();
        let rows = load_rows(&conn).unwrap();
        let review = rows
            .iter()
            .find_map(|r| match r {
                Row::Review(r) => Some(r),
                _ => None,
            })
            .expect("review row loaded");
        assert_eq!(review.verdict.as_deref(), Some("REVISE"));
        assert_eq!(review.base_sha.as_deref(), Some("abc123"));
        assert_eq!(review.head_sha.as_deref(), Some("def456"));
        assert_eq!(review.log_path.as_deref(), Some("/tmp/log"));
        assert_eq!(review.transcript_path.as_deref(), Some("/tmp/transcript"));
        assert_eq!(review.started_at.as_deref(), Some("2026-05-09T01:00:00"));
        assert_eq!(review.completed_at.as_deref(), Some("2026-05-09T01:05:00"));
        assert_eq!(review.duration_ms, Some(300000));
        assert_eq!(review.critical_count, Some(1));
        assert_eq!(review.major_count, Some(2));
        assert_eq!(review.minor_count, Some(3));
        assert_eq!(review.findings_count, Some(4));
    }

    #[test]
    fn architecture_review_rows_round_trip_adr0002_fields() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE tasks (
                display_id TEXT, status TEXT, title TEXT, claimed_by TEXT, updated_at TEXT,
                tier_hint TEXT, linked_observations TEXT, blocked_reason TEXT,
                lifecycle TEXT, active_step TEXT, integration_step TEXT, blocked INTEGER, blocker_kind TEXT,
                current_phase INTEGER, current_cycle INTEGER, plan TEXT, plan_source TEXT,
                contract TEXT, plan_review_log TEXT, cycles TEXT, wrap_log TEXT,
                branch TEXT, workspace_path TEXT
            );
            CREATE TABLE observations (
                display_id TEXT, status TEXT, priority TEXT, summary TEXT, updated_at TEXT,
                body TEXT, source TEXT, task_id TEXT, priority_rank INTEGER, intent_contract TEXT,
                locked_by TEXT, locked_at TEXT, lock_reason TEXT, evidence TEXT, resolution TEXT,
                investigation_failure_reason TEXT
            );
            CREATE TABLE architecture_reviews (
                display_id TEXT, status TEXT, lifecycle TEXT, outcome TEXT,
                source_observation TEXT, linked_observation_ids TEXT, produced_task_id TEXT, verdict TEXT
            );
            INSERT INTO architecture_reviews (
                display_id, status, lifecycle, outcome, source_observation,
                linked_observation_ids, produced_task_id, verdict
            ) VALUES (
                'A001', 'verdict_issued', 'closed', 'primitive_task_created', 'L010',
                '["L010","L011","L012"]', 'T777', 'create_primitive_task'
            );
            "#,
        )
        .unwrap();
        let rows = load_rows(&conn).unwrap();
        let review = rows
            .iter()
            .find_map(|r| match r {
                Row::Review(r) if r.display_id == "A001" => Some(r),
                _ => None,
            })
            .expect("architecture review row loaded");
        assert_eq!(review.task_id, "L010");
        assert_eq!(review.lifecycle.as_deref(), Some("closed"));
        assert_eq!(review.outcome.as_deref(), Some("primitive_task_created"));
        assert_eq!(review.linked_observation_ids, vec!["L010", "L011", "L012"]);
        assert_eq!(review.produced_task_id.as_deref(), Some("T777"));
        assert_eq!(review.verdict.as_deref(), Some("create_primitive_task"));
    }

    #[test]
    fn task_row_extended_fields_round_trip() {
        let conn = cockpit_conn();
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN claimed_at TEXT;
             ALTER TABLE tasks ADD COLUMN integration_attempts TEXT;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,claimed_at,integration_attempts) VALUES ('T200','executing','t','2026-05-01','[]','2026-05-01T12:00:00',?1)",
            rusqlite::params![
                r#"[{"attempt_no":1,"outcome":"rebase_conflict"},{"attempt_no":2,"outcome":"integrated"}]"#
            ],
        ).unwrap();

        let rows = load_rows(&conn).unwrap();
        let task = rows
            .iter()
            .find_map(|r| match r {
                Row::Task(t) => Some(t),
                _ => None,
            })
            .expect("task row loaded");
        assert_eq!(task.claimed_at.as_deref(), Some("2026-05-01T12:00:00"));
        assert_eq!(task.integration_attempts_count, 2);
        assert_eq!(task.last_integration_outcome.as_deref(), Some("integrated"));
    }

    #[test]
    fn intake_row_extended_fields_round_trip() {
        let conn = cockpit_conn();
        conn.execute_batch(
            r#"
            CREATE TABLE intake (
                display_id TEXT, status TEXT, summary TEXT, body TEXT, captured_at TEXT,
                source_task TEXT, source_agent TEXT, risk_flags TEXT, cluster_key TEXT, decision TEXT,
                missing_info_question TEXT, routed_to_observation TEXT, routed_to_arch_review TEXT, duplicate_of TEXT, evidence TEXT,
                recon_round INTEGER, decision_metadata TEXT
            );
            INSERT INTO intake (display_id,status,summary,captured_at,recon_round,decision_metadata)
            VALUES ('I042','triaging','captured','2026-05-08T09:00:00',2,'{"rationale":"matches dispatch cluster","confidence":"high","tier_hint":"T2"}');
            "#,
        )
        .unwrap();
        let rows = load_rows(&conn).unwrap();
        let intake = rows
            .iter()
            .find_map(|r| match r {
                Row::Intake(i) => Some(i),
                _ => None,
            })
            .expect("intake row loaded");
        assert_eq!(intake.captured_at.as_deref(), Some("2026-05-08T09:00:00"));
        assert_eq!(intake.recon_round, Some(2));
        assert_eq!(
            intake.decision_rationale.as_deref(),
            Some("matches dispatch cluster")
        );
        assert_eq!(intake.decision_confidence.as_deref(), Some("high"));
        assert_eq!(intake.decision_tier_hint.as_deref(), Some("T2"));
    }

    #[test]
    fn silent_zombie_reason_exact_token_matching() {
        // canonical tokens must match
        assert!(is_silent_zombie_reason("silent_zombie"));
        assert!(is_silent_zombie_reason("drive_failed:silent_zombie"));
        // known drive_failed variants (real substrate data)
        assert!(is_silent_zombie_reason(
            "drive_failed:silent_zombie_pid_dead"
        ));
        assert!(is_silent_zombie_reason("drive_failed:pid_never_recorded"));
        // colon-namespace form must NOT match (not in canonical set)
        assert!(!is_silent_zombie_reason("silent_zombie: pid dead"));
        // case and whitespace tolerance (trimmed + lowercased)
        assert!(is_silent_zombie_reason("  Silent_Zombie  "));
        // plain suffix attachment must NOT match (the regression this test locks)
        assert!(!is_silent_zombie_reason("silent_zombieish"));
        // unrecognised drive_failed:silent_zombie variant must NOT match
        assert!(!is_silent_zombie_reason(
            "drive_failed:silent_zombie_unrecognized"
        ));
        // empty and unrelated reasons must not match
        assert!(!is_silent_zombie_reason(""));
        assert!(!is_silent_zombie_reason("drive_failed"));
    }
}
