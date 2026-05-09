//! Section-grouping query layer.
//!
//! Reads tasks + observations from `.stores/db.sqlite` (read-only) and
//! classifies each row into the operator cockpit taxonomy exposed by
//! [`Section::label`]: active work, U1/U3 gates, held lanes, terminal rows,
//! priority/observation rows, intake lanes, and external-review rows.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
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
            Section::TasksActionableCurrentWork => "ACTIVE WORK",
            Section::ObsRatifiable => "RATIFY-U1",
            Section::TasksAcceptU3 => "ACCEPT-U3",
            Section::TasksIntegration => "INTEGRATION",
            Section::TasksIntegratedAwaitingPostLand => "INTEGRATED",
            Section::TasksIntegrationBlocked => "HELD-INTEGRATION",
            Section::TasksBlockedNeedsAction => "HELD-BLOCKED",
            Section::TasksDeployRecovery => "HELD-DEPLOY",
            Section::TasksNeedsTriage => "HELD-TRIAGE",
            Section::IntakeHeld => "HELD-INTAKE",
            Section::TasksHeldAiReview => "HELD-AI-REVIEW",
            Section::TasksHeldZombie => "HELD-ZOMBIE",
            Section::TasksRecentlyTerminal => "TERMINAL",
            Section::ObsOpenNoContract => "PRIORITY",
            Section::ObsOther => "OBSERVATIONS",
            Section::IntakeOpen => "INTAKE-OPEN",
            Section::IntakeRouted => "INTAKE-ROUTED",
            Section::ExternalReviewLane => "EXTERNAL-REVIEW",
        }
    }

    pub const ALL: [Section; 18] = [
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreLane {
    Intake,
    Observations,
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
    /// T140 P5: latest `accepted_at` recovered from `transition_history`,
    /// used by the disposition function to distinguish historical-legacy
    /// from deploy-ceremony-pending accepted rows.
    pub accepted_at: Option<String>,
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
    pub runner: String,
    pub held_reason: Option<String>,
    pub next_retry_at: Option<String>,
    pub attempts: i64,
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
    pub evidence_pointer: Option<String>,
    pub recent_events: Vec<RecentEvent>,
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
                if matches!(t.status.as_str(), "executing") {
                    model.execution += 1;
                }
                if matches!(
                    t.status.as_str(),
                    "plan_review" | "code_review" | "in_review"
                ) {
                    model.review += 1;
                }
                if matches!(
                    t.status.as_str(),
                    "accepted" | "complete" | "cargo_installed" | "schema_migrated"
                ) {
                    model.accept += 1;
                }
                if matches!(
                    t.status.as_str(),
                    "blocked" | "deploy_blocked" | "integration_blocked"
                ) {
                    model.held += 1;
                }
                if is_in_flight_task_status(&t.status) {
                    model.active += 1;
                }
                if is_priority_task(t) {
                    model.priority += 1;
                }
            }
            Row::Obs(o) => apply_obs_to_model(o, 1, &mut model),
            Row::CollapsedObs(c) => apply_obs_to_model(&c.representative, c.count, &mut model),
            Row::Intake(i) => {
                if i.status == "needs_info" {
                    model.held += 1;
                }
                if is_priority_text(i.priority.as_deref().unwrap_or("")) || !i.risk_flags.is_empty()
                {
                    model.priority += 1;
                }
                if i.status != "routed" && i.status != "dropped" {
                    model.active += 1;
                }
            }
            Row::Review(r) => {
                if matches!(r.status.as_str(), "running" | "tooling_held" | "pending") {
                    model.active += 1;
                }
                if r.status == "tooling_held" {
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
    if o.status != "resolved" && o.status != "rejected" {
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
    pub draft: usize,
    pub triaging: usize,
    pub needs_info: usize,
    pub routed: usize,
    pub dropped: usize,
}

/// Per-status counts the cockpit renders for the observations lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObsFlow {
    pub open: usize,
    pub investigating: usize,
    pub ready: usize,
    pub resolved: usize,
    pub wont_fix: usize,
    pub investigation_failed: usize,
}

/// Per-status counts the cockpit renders for the tasks lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TasksFlow {
    pub active: usize,
    pub held: usize,
    pub plan_review: usize,
    pub code_review: usize,
    pub in_review: usize,
    pub recently_terminal: usize,
}

/// Per-status counts the cockpit renders for the external-reviews lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewsFlow {
    pub pending: usize,
    pub running: usize,
    pub passed: usize,
    pub revise: usize,
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
            Row::Intake(i) => apply_intake_to_flow(&i.status, 1, &mut model.intake),
            Row::Obs(o) => apply_obs_to_flow(&o.status, 1, &mut model.observations),
            Row::CollapsedObs(c) => {
                apply_obs_to_flow(&c.representative.status, c.count, &mut model.observations)
            }
            Row::Task(t) => apply_task_to_flow(&t.status, &mut model.tasks),
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

fn apply_intake_to_flow(status: &str, count: usize, flow: &mut IntakeFlow) {
    match status {
        "draft" => flow.draft += count,
        "triaging" => flow.triaging += count,
        "needs_info" => flow.needs_info += count,
        "routed" => flow.routed += count,
        "dropped" => flow.dropped += count,
        _ => {}
    }
}

fn apply_obs_to_flow(status: &str, count: usize, flow: &mut ObsFlow) {
    match status {
        "open" => flow.open += count,
        "investigating" => flow.investigating += count,
        "ready" => flow.ready += count,
        "resolved" => flow.resolved += count,
        "wont_fix" => flow.wont_fix += count,
        "investigation_failed" => flow.investigation_failed += count,
        _ => {}
    }
}

fn apply_task_to_flow(status: &str, flow: &mut TasksFlow) {
    match status {
        "planning" | "ready" | "executing" => flow.active += 1,
        "blocked" | "deploy_blocked" => flow.held += 1,
        "plan_review" => flow.plan_review += 1,
        "code_review" => flow.code_review += 1,
        "in_review" => flow.in_review += 1,
        s if is_terminal_task_status(s) => flow.recently_terminal += 1,
        _ => {}
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
    if is_in_flight_task_status(&t.status) {
        return VisibilityClass::ActionableRecovery;
    }
    let reason = t
        .blocked_reason
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let reason_class = blocked_reason_class(t.blocked_reason.as_deref());
    if is_silent_zombie_reason(&reason) {
        return VisibilityClass::ActionableRecovery;
    }
    if reason.contains("accept_installed_inert")
        && matches!(
            t.status.as_str(),
            "cargo_installed"
                | "schema_migrated"
                | "accepted"
                | "complete"
                | "closed_out_of_band"
                | "abandoned"
        )
    {
        return VisibilityClass::HistoricalNoise;
    }
    if t.status == "deploy_blocked" {
        if is_recoverable_deploy_reason(&reason) {
            return VisibilityClass::ActionableRecovery;
        }
        return VisibilityClass::NeedsTriage;
    }
    if t.status == "blocked" {
        return match reason_class {
            "rate_limit" | "retry" | "dependency" | "user" | "deploy" => {
                VisibilityClass::ActionableRecovery
            }
            "unknown" => VisibilityClass::NeedsTriage,
            _ => VisibilityClass::ActionableRecovery,
        };
    }
    VisibilityClass::ActionableRecovery
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

fn is_recoverable_deploy_reason(reason: &str) -> bool {
    reason.contains("retry-deploy-recoverable") || reason.contains("recoverable")
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
            Row::Task(t) => Some((t.display_id.clone(), t.status.clone())),
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

/// Load tasks + observations (+ intake when installed) from the db. Errors out only on
/// hard sqlite failures; optional tables/columns degrade to empty/default data.
pub fn load_rows(conn: &Connection) -> Result<Vec<Row>> {
    let mut rows = Vec::new();

    let task_cols = table_columns(conn, "tasks")?;
    let task_sql = format!(
        "SELECT display_id, status, {title}, {claimed_by}, {updated_at}, {tier_hint}, {linked_observations}, {blocked_reason}, \
                {current_phase}, {current_cycle}, {total_phases}, {plan_source}, \
                {contract_executive_intent}, {contract_done_when}, {contract_scope_in}, {contract_scope_out}, \
                {plan_review_log}, {cycles}, {wrap_log}, {branch}, {workspace_path}, {drive_pid}, {drive_started_at}, \
                {activation}, rowid FROM tasks",
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
        let branch: Option<String> = r.get(19).ok().flatten();
        let workspace_path: Option<String> = r.get(20).ok().flatten();
        let drive_pid: Option<i64> = r.get(21).ok().flatten();
        let drive_started_at: Option<String> = r.get(22).ok().flatten();
        let activation: Option<String> = r.get(23).ok().flatten();
        let row_id: i64 = r.get(24)?;
        let display_id: String = r.get(0)?;
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
            current_phase: r.get(8).ok().flatten(),
            current_cycle: r.get(9).ok().flatten(),
            total_phases: r.get(10).ok().flatten(),
            plan_source: r.get(11).ok().flatten(),
            contract_executive_intent: r.get(12).ok().flatten(),
            contract_done_when: r.get(13).ok().flatten(),
            contract_scope_in: r.get(14).ok().flatten(),
            contract_scope_out: r.get(15).ok().flatten(),
            plan_review_summaries: json_summary_list(
                r.get::<_, String>(16).ok().as_deref(),
                "summary",
            ),
            cycle_summaries: cycle_summary_list(r.get::<_, String>(17).ok().as_deref()),
            wrap_summaries: json_summary_list(
                r.get::<_, String>(18).ok().as_deref(),
                "executive_summary",
            ),
            branch,
            workspace_path,
            drive_pid,
            drive_started_at,
            artifact_pointers: Vec::new(),
            recent_events: Vec::new(),
            activation,
            accepted_at: accepted_at_map.get(&row_id).cloned(),
        })
    })?;
    for r in task_iter.flatten() {
        rows.push(Row::Task(r));
    }

    let obs_cols = table_columns(conn, "observations")?;
    let obs_sql = format!(
        "SELECT display_id, status, {priority}, {summary}, {updated_at}, {body}, {source}, {task_id}, {priority_rank}, \
                {contract_state}, {tier_hint}, {objective}, {itype}, {acceptance}, {in_scope}, {out_of_scope}, {known_solution}, \
                {locked_by}, {locked_at}, {lock_reason}, {evidence}, {resolution}, {investigation_failure_reason} FROM observations",
        priority = sql_col(&obs_cols, "priority", "''"), summary = sql_col(&obs_cols, "summary", "''"), updated_at = sql_col(&obs_cols, "updated_at", "''"),
        body = sql_col(&obs_cols, "body", "NULL"), source = sql_col(&obs_cols, "source", "NULL"), task_id = sql_col(&obs_cols, "task_id", "NULL"), priority_rank = sql_col(&obs_cols, "priority_rank", "NULL"),
        contract_state = json_col(&obs_cols, "intent_contract", "$.contract_state"), tier_hint = json_col(&obs_cols, "intent_contract", "$.tier_hint"),
        objective = json_col(&obs_cols, "intent_contract", "$.objective"), itype = json_col(&obs_cols, "intent_contract", "$.type"), acceptance = json_col(&obs_cols, "intent_contract", "$.acceptance"),
        in_scope = json_col(&obs_cols, "intent_contract", "$.in_scope"), out_of_scope = json_col(&obs_cols, "intent_contract", "$.out_of_scope"), known_solution = json_col(&obs_cols, "intent_contract", "$.known_solution"),
        locked_by = sql_col(&obs_cols, "locked_by", "NULL"), locked_at = sql_col(&obs_cols, "locked_at", "NULL"), lock_reason = sql_col(&obs_cols, "lock_reason", "NULL"),
        evidence = sql_col(&obs_cols, "evidence", "NULL"), resolution = sql_col(&obs_cols, "resolution", "NULL"), investigation_failure_reason = sql_col(&obs_cols, "investigation_failure_reason", "NULL"),
    );
    let mut stmt = conn.prepare(&obs_sql)?;
    let obs_iter = stmt.query_map([], |r| {
        let evidence_raw: Option<String> = r.get(20).ok().flatten();
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
            contract_state: r.get(9).ok().flatten(),
            tier_hint: r.get(10).ok().flatten(),
            intent_objective: r.get(11).ok().flatten(),
            intent_type: r.get(12).ok().flatten(),
            intent_acceptance: json_string_array(r.get::<_, String>(13).ok().as_deref()),
            intent_in_scope: json_string_array(r.get::<_, String>(14).ok().as_deref()),
            intent_out_of_scope: json_string_array(r.get::<_, String>(15).ok().as_deref()),
            intent_known_solution: r.get(16).ok().flatten(),
            locked_by: r.get(17).ok().flatten(),
            locked_at: r.get(18).ok().flatten(),
            lock_reason: r.get(19).ok().flatten(),
            evidence_pointers: evidence_pointers(evidence_raw.as_deref()),
            resolution_pointer: r.get(21).ok().flatten(),
            recent_events: Vec::new(),
            investigation_failure_reason: r.get(22).ok().flatten(),
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
        let sql = format!(
            "SELECT display_id, task_id, status, {runner_expr}, {held_expr}, {retry_expr}, {attempts_expr} FROM external_reviews WHERE status IN ('pending','running','tooling_held')"
        );
        let mut stmt = conn.prepare(&sql)?;
        let review_iter = stmt.query_map([], |r| {
            Ok(ReviewRow {
                display_id: r.get(0)?,
                task_id: r.get(1)?,
                status: r.get(2)?,
                runner: r.get(3)?,
                held_reason: r.get(4).ok(),
                next_retry_at: r.get(5).ok(),
                attempts: r.get(6)?,
            })
        })?;
        for r in review_iter.flatten() {
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
        "SELECT display_id, status, {summary}, {body}, {updated_at}, {source_task}, {source_agent}, {risk_flags}, {cluster_key}, {decision}, {missing_info_question}, {routed_to_observation}, {routed_to_arch_review}, {duplicate_of}, {evidence} FROM intake",
        summary = sql_col(&cols, "summary", "''"), body = sql_col(&cols, "body", "NULL"),
        updated_at = if cols.iter().any(|c| c == "updated_at") { quote_ident("updated_at") } else { sql_col(&cols, "captured_at", "''") },
        source_task = sql_col(&cols, "source_task", "NULL"), source_agent = sql_col(&cols, "source_agent", "NULL"), risk_flags = sql_col(&cols, "risk_flags", "NULL"), cluster_key = sql_col(&cols, "cluster_key", "NULL"),
        decision = sql_col(&cols, "decision", "NULL"), missing_info_question = sql_col(&cols, "missing_info_question", "NULL"), routed_to_observation = sql_col(&cols, "routed_to_observation", "NULL"), routed_to_arch_review = sql_col(&cols, "routed_to_arch_review", "NULL"), duplicate_of = sql_col(&cols, "duplicate_of", "NULL"), evidence = sql_col(&cols, "evidence", "NULL"),
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
        Ok(IntakeRow {
            display_id: r.get(0)?,
            status: status.clone(),
            summary: r.get(2)?,
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
            evidence_pointer: r.get(14).ok().flatten(),
            recent_events: Vec::new(),
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
        if !opts.show_all_history
            && row_visibility_class(row, &task_ctx) == VisibilityClass::HistoricalNoise
        {
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
                    .unwrap_or(false)
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

fn section_for(row: &Row) -> Option<Section> {
    match row {
        Row::Task(t) => {
            let reason = t
                .blocked_reason
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase();
            if is_terminal_task_status(&t.status) {
                return Some(Section::TasksRecentlyTerminal);
            }
            if is_silent_zombie_reason(&reason) {
                return Some(Section::TasksHeldZombie);
            }
            match t.status.as_str() {
                "blocked" => {
                    if task_visibility_class(t) == VisibilityClass::NeedsTriage {
                        Some(Section::TasksNeedsTriage)
                    } else {
                        Some(Section::TasksBlockedNeedsAction)
                    }
                }
                "deploy_blocked" => {
                    if task_visibility_class(t) == VisibilityClass::NeedsTriage {
                        Some(Section::TasksNeedsTriage)
                    } else {
                        Some(Section::TasksDeployRecovery)
                    }
                }
                "integration_queued" | "integrating" => Some(Section::TasksIntegration),
                "integrated" => Some(Section::TasksIntegratedAwaitingPostLand),
                "integration_blocked" => Some(Section::TasksIntegrationBlocked),
                "plan_review" | "code_review" => Some(Section::TasksHeldAiReview),
                "in_review" => Some(Section::TasksAcceptU3),
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
        Row::Intake(i) => match i.status.as_str() {
            "routed" | "dropped" => None,
            "needs_info" => Some(Section::IntakeHeld),
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
/// exhaust" strip — main task rows hide terminal history.
pub fn recent_exhaust(rows: &[Row], limit: usize) -> Vec<&Row> {
    if limit == 0 {
        return Vec::new();
    }
    let mut indices: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            Row::Task(t) if is_terminal_task_status(&t.status) => Some(i),
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

fn parse_epoch(s: &str) -> Option<i64> {
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
            "ACTIVE WORK",
            "RATIFY-U1",
            "ACCEPT-U3",
            "INTEGRATION",
            "INTEGRATED",
            "HELD-INTEGRATION",
            "HELD-BLOCKED",
            "HELD-DEPLOY",
            "HELD-TRIAGE",
            "HELD-INTAKE",
            "HELD-AI-REVIEW",
            "HELD-ZOMBIE",
            "TERMINAL",
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
            task("plan_review"),        // idx 0 → HELD-AI-REVIEW
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
        assert_eq!(b(Section::TasksActionableCurrentWork), Vec::<usize>::new());
        assert_eq!(b(Section::TasksBlockedNeedsAction), Vec::<usize>::new());
        assert_eq!(b(Section::TasksDeployRecovery), Vec::<usize>::new());
        assert_eq!(b(Section::TasksNeedsTriage), vec![1usize, 2]);
        assert_eq!(b(Section::TasksRecentlyTerminal), vec![3usize]);
        assert_eq!(b(Section::ObsRatifiable), vec![4usize]);
        assert_eq!(b(Section::TasksHeldAiReview), vec![0usize]);
        assert_eq!(b(Section::ObsOpenNoContract), Vec::<usize>::new());
        assert_eq!(b(Section::ObsOther), vec![5usize]);
    }

    #[test]
    fn task_status_mapping_is_exhaustive() {
        // blocked/deploy_blocked with no reason → unknown class → TasksNeedsTriage.
        // Use an explicit recoverable reason to get TasksBlockedNeedsAction / TasksDeployRecovery.
        let mappings: &[(&str, Option<&str>, Section)] = &[
            ("planning", None, Section::TasksActionableCurrentWork),
            ("plan_review", None, Section::TasksHeldAiReview),
            ("ready", None, Section::TasksActionableCurrentWork),
            ("executing", None, Section::TasksActionableCurrentWork),
            ("code_review", None, Section::TasksHeldAiReview),
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
        assert!(matches!(
            load_external_review_state(&conn).unwrap(),
            ExternalReviewState::Unavailable { .. }
        ));
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
        assert_eq!(store_lane_for_row(&obs_row("open")), StoreLane::Observations);
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

        assert_eq!(model.intake.draft, 2);
        assert_eq!(model.intake.triaging, 1);
        assert_eq!(model.intake.needs_info, 1);
        assert_eq!(model.intake.routed, 1);
        assert_eq!(model.intake.dropped, 1);

        assert_eq!(model.observations.open, 1);
        assert_eq!(model.observations.investigating, 1);
        assert_eq!(model.observations.ready, 1);
        assert_eq!(model.observations.resolved, 1);
        assert_eq!(model.observations.wont_fix, 1);
        assert_eq!(model.observations.investigation_failed, 1);

        assert_eq!(model.tasks.active, 3);
        assert_eq!(model.tasks.held, 2);
        assert_eq!(model.tasks.plan_review, 1);
        assert_eq!(model.tasks.code_review, 1);
        assert_eq!(model.tasks.in_review, 1);
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
        assert_eq!(model.observations.open, 7);
    }

    #[test]
    fn store_flow_model_empty_rows_zero_counts_but_engine_reflects_inputs() {
        let health = SystemHealth {
            unfinished_dispatch_locks: 4,
            oldest_claimed_at_epoch: Some(NOW - 90),
        };
        let daemon = super::super::daemon::Liveness::Dead;
        let model = store_flow_model_at(
            &[],
            &health,
            &daemon,
            &ExternalReviewState::default(),
            NOW,
        );
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
        let rows: Vec<Row> = vec![
            task_with_id("T1", "accepted", NOW - 300),
            task_with_id("T2", "complete", NOW - 100),
            task_with_id("T3", "rejected", NOW - 200),
            task_with_id("T4", "executing", NOW - 50),
            task_with_id("T5", "abandoned", NOW - 400),
            task_with_id("T6", "schema_migrated", NOW - 10),
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
