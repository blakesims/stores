//! Pure semantic presentation mapping for `stores watch`.
//!
//! This module translates schema/debug tuples into operator-facing labels and
//! glyphs. Rendering phases consume these structs later; rows/details keep raw
//! tuples elsewhere.

use std::collections::BTreeMap;

use serde_json::Value;

use super::daemon::Liveness;
use super::data::{
    obs_lifecycle, task_active_step, task_integration_step, task_is_blocked,
    task_is_terminal_primary, task_lifecycle, CycleReviewGate, IntakeRow, ObsRow, PlanReviewGate,
    ReviewRow, SystemHealth, TaskCycleEntry, TaskRow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationSeverity {
    Front,
    Work,
    Gate,
    Exit,
    Wait,
    Fault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    pub glyph: &'static str,
    pub label: String,
    pub severity: PresentationSeverity,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSlotPresentation {
    pub slot: PresentationSeverity,
    pub glyph: &'static str,
    pub label: &'static str,
    pub count: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSlotId {
    Front,
    Work,
    Gate,
    Exit,
    Wait,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAttention {
    Exhaust,
    Flow,
    Fault,
    Neutral,
}

/// Transitional typed projection for `stores watch` presentation.
///
/// `WatchProjection` mirrors the future declarative schema `watch:` metadata:
/// one slot vocabulary, one display-group label vocabulary, and one row-stage
/// vocabulary per row. Renderers and data aggregation should consume this
/// projection for top-card slots, focused display groups, and row labels instead
/// of re-deriving independent labels from legacy `Section` buckets or raw row
/// fields. `Section` remains an internal compatibility classifier while the
/// schema seam is still Rust-owned. Raw lifecycle/status/debug fields remain
/// available in detail panes and diagnostics; they are not the cockpit display
/// vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchProjection {
    pub slot: WatchSlotId,
    pub slot_label: &'static str,
    pub glyph: &'static str,
    pub row_stage: &'static str,
    pub row_signal: Option<String>,
    pub next_action: Option<&'static str>,
    pub attention: WatchAttention,
}

pub fn task_watch_projection(task: &TaskRow) -> WatchProjection {
    let presentation = task_presentation(task);
    let slot = watch_slot_id(presentation.severity);
    WatchProjection {
        slot,
        slot_label: task_watch_slot_label(slot),
        glyph: presentation.glyph,
        row_stage: task_watch_stage(&presentation.label),
        row_signal: presentation.signal,
        next_action: None,
        attention: watch_attention(slot),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapGlyph {
    Queued,
    Planning,
    PlanReview,
    UnreachedPhase,
    Executing,
    CodeReview,
    Wrap,
    Waiting,
    Fault,
    Unknown,
}

impl MapGlyph {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Queued => "◌",
            Self::Planning => "○",
            Self::PlanReview => "●",
            Self::UnreachedPhase => "·",
            Self::Executing => "□",
            Self::CodeReview => "▣",
            // Wrap/acceptance deliberately reuses the filled square. The task
            // map only has two execution-family squares: □ active work and
            // ▣ gate/result/acceptance; no third slanted-square variant.
            Self::Wrap => "▣",
            Self::Waiting => "△",
            Self::Fault => "▲",
            Self::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapColor {
    Inactive,
    ActiveWork,
    ActiveGate,
    Passed,
    Failed,
    Waiting,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapConfidence {
    Exact,
    Implied,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSource {
    Lifecycle,
    ActiveStep,
    ActiveStepAndPlanReviewLog,
    CurrentPhaseCycle,
    TotalPhases,
    PlanReviewLog,
    Cycles,
    PlanSource,
    Blocker,
    TerminalLifecycle,
    MissingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCell {
    pub glyph: MapGlyph,
    pub cycle: Option<i64>,
    pub color_role: MapColor,
    pub active: bool,
    pub source: MapSource,
    pub confidence: MapConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMapProjection {
    pub planning: MapCell,
    pub phases: Vec<MapCell>,
    pub wrap: Option<MapCell>,
    pub fallback: Option<MapCell>,
    pub reason: Option<String>,
    pub confidence: MapConfidence,
}

pub fn task_map_projection(task: &TaskRow) -> TaskMapProjection {
    let mut planning = planning_cell(task);
    let mut phases = phase_cells(task);
    apply_historical_cycles(task, &mut phases);
    apply_current_execution(task, &mut phases);
    infer_planning_from_progress(task, &mut planning);

    let wrap = wrap_cell(task);
    let (fallback, reason) = blocked_fallback(task);
    let confidence = projection_confidence(&planning, &phases, wrap.as_ref(), fallback.as_ref());

    TaskMapProjection {
        planning,
        phases,
        wrap,
        fallback,
        reason,
        confidence,
    }
}

fn map_cell(
    glyph: MapGlyph,
    cycle: Option<i64>,
    color_role: MapColor,
    active: bool,
    source: MapSource,
    confidence: MapConfidence,
) -> MapCell {
    MapCell {
        glyph,
        cycle: cycle.filter(|c| *c > 1),
        color_role,
        active,
        source,
        confidence,
    }
}

fn planning_cell(task: &TaskRow) -> MapCell {
    let lifecycle = task_lifecycle(task);
    let active_step = task_active_step(task);
    if lifecycle == "queued" {
        return map_cell(
            MapGlyph::Queued,
            None,
            MapColor::Inactive,
            false,
            MapSource::Lifecycle,
            MapConfidence::Exact,
        );
    }
    if lifecycle == "active" && active_step == "planning" {
        return map_cell(
            MapGlyph::Planning,
            plan_attempt_count(task),
            MapColor::ActiveWork,
            true,
            active_planning_source(task),
            MapConfidence::Exact,
        );
    }
    if lifecycle == "active" && active_step == "planning_review" {
        return map_cell(
            MapGlyph::PlanReview,
            plan_attempt_count(task),
            MapColor::ActiveGate,
            true,
            active_planning_source(task),
            MapConfidence::Exact,
        );
    }

    match task.plan_review_entries.last().map(|entry| &entry.gate) {
        Some(PlanReviewGate::Ready) => map_cell(
            MapGlyph::PlanReview,
            plan_attempt_count(task),
            MapColor::Passed,
            false,
            MapSource::PlanReviewLog,
            MapConfidence::Exact,
        ),
        Some(PlanReviewGate::NotReady) => map_cell(
            MapGlyph::PlanReview,
            plan_attempt_count(task),
            MapColor::Failed,
            false,
            MapSource::PlanReviewLog,
            MapConfidence::Exact,
        ),
        _ if task
            .plan_source
            .as_deref()
            .map(|s| s == "contract_synthesized")
            .unwrap_or(false) =>
        {
            map_cell(
                MapGlyph::PlanReview,
                None,
                MapColor::Inactive,
                false,
                MapSource::PlanSource,
                MapConfidence::Implied,
            )
        }
        _ => map_cell(
            MapGlyph::Queued,
            None,
            MapColor::Inactive,
            false,
            MapSource::MissingEvidence,
            MapConfidence::Unknown,
        ),
    }
}

fn plan_attempt_count(task: &TaskRow) -> Option<i64> {
    let attempts = task.plan_review_entries.len() as i64;
    (attempts > 1).then_some(attempts)
}

fn active_planning_source(task: &TaskRow) -> MapSource {
    if plan_attempt_count(task).is_some() {
        MapSource::ActiveStepAndPlanReviewLog
    } else {
        MapSource::ActiveStep
    }
}

fn phase_cells(task: &TaskRow) -> Vec<MapCell> {
    match task.total_phases {
        Some(total) if total > 0 => (0..total)
            .map(|_| {
                map_cell(
                    MapGlyph::UnreachedPhase,
                    None,
                    MapColor::Inactive,
                    false,
                    MapSource::TotalPhases,
                    MapConfidence::Exact,
                )
            })
            .collect(),
        _ => vec![map_cell(
            MapGlyph::Unknown,
            None,
            MapColor::Unknown,
            false,
            MapSource::MissingEvidence,
            MapConfidence::Unknown,
        )],
    }
}

fn apply_historical_cycles(task: &TaskRow, phases: &mut [MapCell]) {
    let mut latest_by_phase: BTreeMap<i64, &TaskCycleEntry> = BTreeMap::new();
    for entry in &task.cycle_entries {
        if entry.phase <= 0 || entry.cycle <= 0 {
            continue;
        }
        latest_by_phase
            .entry(entry.phase)
            .and_modify(|current| {
                if entry.cycle > current.cycle {
                    *current = entry;
                }
            })
            .or_insert(entry);
    }

    for (phase, entry) in latest_by_phase {
        let Some(cell) = phase_cell_mut(phases, phase) else {
            continue;
        };
        match entry.review_gate.as_ref() {
            Some(CycleReviewGate::Pass) => {
                *cell = map_cell(
                    MapGlyph::CodeReview,
                    Some(entry.cycle),
                    MapColor::Passed,
                    false,
                    MapSource::Cycles,
                    MapConfidence::Exact,
                );
            }
            Some(CycleReviewGate::Revise) => {
                *cell = map_cell(
                    MapGlyph::CodeReview,
                    Some(entry.cycle),
                    MapColor::ActiveGate,
                    false,
                    MapSource::Cycles,
                    MapConfidence::Exact,
                );
            }
            Some(CycleReviewGate::Fail) => {
                *cell = map_cell(
                    MapGlyph::CodeReview,
                    Some(entry.cycle),
                    MapColor::Failed,
                    false,
                    MapSource::Cycles,
                    MapConfidence::Exact,
                );
            }
            Some(CycleReviewGate::Unknown(_)) | None => {}
        }
    }
}

fn apply_current_execution(task: &TaskRow, phases: &mut [MapCell]) {
    let Some(phase) = task.current_phase.filter(|p| *p > 0) else {
        return;
    };
    let Some(cell) = phase_cell_mut(phases, phase) else {
        return;
    };
    match task_active_step(task) {
        "coding" => {
            *cell = map_cell(
                MapGlyph::Executing,
                task.current_cycle,
                MapColor::ActiveWork,
                true,
                MapSource::CurrentPhaseCycle,
                MapConfidence::Exact,
            );
        }
        "coding_review" => {
            *cell = map_cell(
                MapGlyph::CodeReview,
                task.current_cycle,
                MapColor::ActiveGate,
                true,
                MapSource::CurrentPhaseCycle,
                MapConfidence::Exact,
            );
        }
        _ => {}
    }
}

fn phase_cell_mut(phases: &mut [MapCell], phase: i64) -> Option<&mut MapCell> {
    let index = usize::try_from(phase - 1).ok()?;
    phases.get_mut(index)
}

fn infer_planning_from_progress(task: &TaskRow, planning: &mut MapCell) {
    if planning.confidence != MapConfidence::Unknown {
        return;
    }
    let has_execution_progress = task.current_phase.filter(|p| *p > 0).is_some()
        || task
            .cycle_entries
            .iter()
            .any(|entry| entry.phase > 0 && entry.cycle > 0)
        || task_active_step(task) == "wrapping"
        || task_is_terminal_primary(task);
    if has_execution_progress {
        *planning = map_cell(
            MapGlyph::PlanReview,
            None,
            MapColor::Inactive,
            false,
            MapSource::Lifecycle,
            MapConfidence::Implied,
        );
    }
}

fn wrap_cell(task: &TaskRow) -> Option<MapCell> {
    if task_lifecycle(task) == "active" && task_active_step(task) == "wrapping" {
        return Some(map_cell(
            MapGlyph::Wrap,
            None,
            MapColor::ActiveGate,
            true,
            MapSource::ActiveStep,
            MapConfidence::Exact,
        ));
    }
    if task_is_terminal_primary(task) {
        return Some(map_cell(
            MapGlyph::Wrap,
            None,
            MapColor::Passed,
            false,
            MapSource::TerminalLifecycle,
            MapConfidence::Exact,
        ));
    }
    None
}

fn blocked_fallback(task: &TaskRow) -> (Option<MapCell>, Option<String>) {
    if !task_is_blocked(task) {
        return (None, None);
    }
    let kind = task
        .blocker_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .or_else(|| {
            task.blocked_reason_class
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "none")
        })
        .unwrap_or("blocked");
    let waiting = matches!(kind, "capacity" | "dependency" | "rate_limit" | "human");
    let cell = if waiting {
        map_cell(
            MapGlyph::Waiting,
            None,
            MapColor::Waiting,
            false,
            MapSource::Blocker,
            MapConfidence::Exact,
        )
    } else {
        map_cell(
            MapGlyph::Fault,
            None,
            MapColor::Failed,
            false,
            MapSource::Blocker,
            MapConfidence::Exact,
        )
    };
    (Some(cell), Some(kind.to_string()))
}

fn projection_confidence(
    planning: &MapCell,
    phases: &[MapCell],
    wrap: Option<&MapCell>,
    fallback: Option<&MapCell>,
) -> MapConfidence {
    let cells = std::iter::once(planning)
        .chain(phases.iter())
        .chain(wrap)
        .chain(fallback);
    if cells
        .clone()
        .any(|cell| cell.confidence == MapConfidence::Unknown)
    {
        MapConfidence::Unknown
    } else if cells
        .clone()
        .any(|cell| cell.confidence == MapConfidence::Implied)
    {
        MapConfidence::Implied
    } else {
        MapConfidence::Exact
    }
}

pub fn observation_watch_projection(row: &ObsRow) -> WatchProjection {
    let presentation = observation_presentation(row);
    let slot = watch_slot_id(presentation.severity);
    WatchProjection {
        slot,
        slot_label: observation_watch_slot_label(slot),
        glyph: presentation.glyph,
        row_stage: observation_watch_stage(&presentation.label),
        row_signal: presentation.signal,
        next_action: Some(observation_watch_next_action(&presentation.label)),
        attention: watch_attention(slot),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationCheckpoint {
    SignalEvidence,
    Contract,
    Architecture,
    Resolution,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationGlyph {
    Candidate,
    Evidence,
    Contract,
    Architecture,
    Resolved,
    ClosedRejected,
    Superseded,
    Waiting,
    Fault,
    Unreached,
    Unknown,
}

impl ObservationGlyph {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Candidate => "◌",
            Self::Evidence => "●",
            Self::Contract => "▣",
            Self::Architecture => "◈",
            Self::Resolved => "✓",
            Self::ClosedRejected => "×",
            Self::Superseded => "■",
            Self::Waiting => "△",
            Self::Fault => "▲",
            Self::Unreached => "·",
            Self::Unknown => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationFlowSource {
    Lifecycle,
    Status,
    EvidencePointers,
    ContractState,
    WaitingKind,
    ArchitectureReview,
    Outcome,
    SupersededBy,
    TaskLink,
    InvestigationFailure,
    MissingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationFlowCell {
    pub checkpoint: ObservationCheckpoint,
    pub glyph: ObservationGlyph,
    pub count: Option<i64>,
    pub color_role: MapColor,
    pub active: bool,
    pub source: ObservationFlowSource,
    pub confidence: MapConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationFlowProjection {
    pub cells: Vec<ObservationFlowCell>,
    pub next: Option<String>,
    pub reason: Option<String>,
    pub link: Option<String>,
    pub confidence: MapConfidence,
}

pub fn observation_flow_projection(row: &ObsRow) -> ObservationFlowProjection {
    if row.status == "investigation_failed" || row.investigation_failure_reason.is_some() {
        let cell = observation_cell(
            ObservationCheckpoint::Fallback,
            ObservationGlyph::Fault,
            None,
            MapColor::Failed,
            false,
            ObservationFlowSource::InvestigationFailure,
            MapConfidence::Exact,
        );
        return observation_projection_from_cells(
            vec![cell],
            Some("inspect"),
            row.investigation_failure_reason
                .clone()
                .or_else(|| Some("investigation_failed".to_string())),
            observation_link(row),
        );
    }

    if let Some(kind) = generic_observation_waiting_kind(row) {
        let cell = observation_cell(
            ObservationCheckpoint::Fallback,
            ObservationGlyph::Waiting,
            None,
            MapColor::Waiting,
            false,
            ObservationFlowSource::WaitingKind,
            MapConfidence::Exact,
        );
        return observation_projection_from_cells(
            vec![cell],
            Some(observation_wait_next(kind)),
            Some(kind.to_string()),
            observation_link(row),
        );
    }

    let include_arch = observation_has_architecture_gate(row);
    let mut cells = vec![observation_signal_cell(row), observation_contract_cell(row)];
    if include_arch {
        cells.push(observation_architecture_cell(row));
    }
    cells.push(observation_resolution_cell(row));
    observation_projection_from_cells(
        cells,
        Some(observation_flow_next(row)),
        None,
        observation_link(row),
    )
}

fn observation_projection_from_cells(
    cells: Vec<ObservationFlowCell>,
    next: Option<impl Into<String>>,
    reason: Option<String>,
    link: Option<String>,
) -> ObservationFlowProjection {
    let confidence = cells_confidence(cells.iter().map(|cell| cell.confidence));
    ObservationFlowProjection {
        cells,
        next: next.map(Into::into),
        reason,
        link,
        confidence,
    }
}

fn cells_confidence(confidences: impl Iterator<Item = MapConfidence>) -> MapConfidence {
    let mut saw_implied = false;
    for confidence in confidences {
        match confidence {
            MapConfidence::Unknown => return MapConfidence::Unknown,
            MapConfidence::Implied => saw_implied = true,
            MapConfidence::Exact => {}
        }
    }
    if saw_implied {
        MapConfidence::Implied
    } else {
        MapConfidence::Exact
    }
}

fn observation_cell(
    checkpoint: ObservationCheckpoint,
    glyph: ObservationGlyph,
    count: Option<i64>,
    color_role: MapColor,
    active: bool,
    source: ObservationFlowSource,
    confidence: MapConfidence,
) -> ObservationFlowCell {
    ObservationFlowCell {
        checkpoint,
        glyph,
        count: count.filter(|c| *c > 1),
        color_role,
        active,
        source,
        confidence,
    }
}

fn observation_signal_cell(row: &ObsRow) -> ObservationFlowCell {
    let evidence_count = row.evidence_pointers.len() as i64;
    let lifecycle = obs_lifecycle(row);
    if matches!(lifecycle, "candidate")
        && !observation_has_contract(row)
        && !observation_is_terminal(row)
        && !observation_has_architecture_gate(row)
    {
        return observation_cell(
            ObservationCheckpoint::SignalEvidence,
            ObservationGlyph::Candidate,
            None,
            MapColor::Inactive,
            false,
            ObservationFlowSource::Lifecycle,
            MapConfidence::Exact,
        );
    }
    if matches!(lifecycle, "ready" | "investigating") {
        return observation_cell(
            ObservationCheckpoint::SignalEvidence,
            ObservationGlyph::Evidence,
            Some(evidence_count),
            MapColor::ActiveWork,
            true,
            if evidence_count > 0 {
                ObservationFlowSource::EvidencePointers
            } else {
                ObservationFlowSource::Lifecycle
            },
            MapConfidence::Exact,
        );
    }
    if observation_has_contract(row)
        || observation_is_terminal(row)
        || observation_has_architecture_gate(row)
    {
        return observation_cell(
            ObservationCheckpoint::SignalEvidence,
            ObservationGlyph::Evidence,
            Some(evidence_count),
            MapColor::Inactive,
            false,
            if evidence_count > 0 {
                ObservationFlowSource::EvidencePointers
            } else {
                ObservationFlowSource::Lifecycle
            },
            MapConfidence::Implied,
        );
    }
    observation_cell(
        ObservationCheckpoint::SignalEvidence,
        ObservationGlyph::Unknown,
        None,
        MapColor::Unknown,
        false,
        ObservationFlowSource::MissingEvidence,
        MapConfidence::Unknown,
    )
}

fn observation_contract_cell(row: &ObsRow) -> ObservationFlowCell {
    match row.contract_state.as_deref() {
        Some("draft") => observation_cell(
            ObservationCheckpoint::Contract,
            ObservationGlyph::Contract,
            None,
            MapColor::ActiveGate,
            true,
            ObservationFlowSource::ContractState,
            MapConfidence::Exact,
        ),
        Some("ready" | "approved") => observation_cell(
            ObservationCheckpoint::Contract,
            ObservationGlyph::Contract,
            None,
            MapColor::Passed,
            false,
            ObservationFlowSource::ContractState,
            MapConfidence::Exact,
        ),
        _ if matches!(row.waiting_kind.as_deref(), Some("human_ratification")) => observation_cell(
            ObservationCheckpoint::Contract,
            ObservationGlyph::Contract,
            None,
            MapColor::ActiveGate,
            true,
            ObservationFlowSource::WaitingKind,
            MapConfidence::Exact,
        ),
        _ if observation_is_terminal(row) || observation_has_architecture_gate(row) => {
            observation_cell(
                ObservationCheckpoint::Contract,
                ObservationGlyph::Contract,
                None,
                MapColor::Inactive,
                false,
                ObservationFlowSource::Lifecycle,
                MapConfidence::Implied,
            )
        }
        _ => observation_cell(
            ObservationCheckpoint::Contract,
            ObservationGlyph::Unreached,
            None,
            MapColor::Inactive,
            false,
            ObservationFlowSource::MissingEvidence,
            MapConfidence::Exact,
        ),
    }
}

fn observation_architecture_cell(row: &ObsRow) -> ObservationFlowCell {
    if observation_has_architecture_gate(row) {
        return observation_cell(
            ObservationCheckpoint::Architecture,
            ObservationGlyph::Architecture,
            None,
            MapColor::ActiveGate,
            true,
            ObservationFlowSource::ArchitectureReview,
            MapConfidence::Exact,
        );
    }
    observation_cell(
        ObservationCheckpoint::Architecture,
        ObservationGlyph::Unreached,
        None,
        MapColor::Inactive,
        false,
        ObservationFlowSource::MissingEvidence,
        MapConfidence::Exact,
    )
}

fn observation_resolution_cell(row: &ObsRow) -> ObservationFlowCell {
    if row
        .superseded_by_id
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        || matches!(row.outcome.as_deref(), Some("superseded"))
        || matches!(row.status.as_str(), "superseded")
    {
        return observation_cell(
            ObservationCheckpoint::Resolution,
            ObservationGlyph::Superseded,
            None,
            MapColor::Passed,
            false,
            ObservationFlowSource::SupersededBy,
            MapConfidence::Exact,
        );
    }
    if matches!(
        row.outcome.as_deref(),
        Some("closed_wont_fix" | "wont_fix" | "wont-fix")
    ) || matches!(
        row.status.as_str(),
        "closed_wont_fix" | "wont_fix" | "wont-fix" | "rejected"
    ) {
        return observation_cell(
            ObservationCheckpoint::Resolution,
            ObservationGlyph::ClosedRejected,
            None,
            MapColor::Failed,
            false,
            ObservationFlowSource::Outcome,
            MapConfidence::Exact,
        );
    }
    if obs_is_closed_exit(row) {
        return observation_cell(
            ObservationCheckpoint::Resolution,
            ObservationGlyph::Resolved,
            None,
            MapColor::Passed,
            false,
            ObservationFlowSource::Outcome,
            MapConfidence::Exact,
        );
    }
    if obs_lifecycle(row) == "in_progress" {
        return observation_cell(
            ObservationCheckpoint::Resolution,
            ObservationGlyph::Contract,
            None,
            MapColor::ActiveWork,
            true,
            ObservationFlowSource::Lifecycle,
            MapConfidence::Exact,
        );
    }
    observation_cell(
        ObservationCheckpoint::Resolution,
        ObservationGlyph::Unreached,
        None,
        MapColor::Inactive,
        false,
        ObservationFlowSource::MissingEvidence,
        MapConfidence::Exact,
    )
}

fn generic_observation_waiting_kind(row: &ObsRow) -> Option<&str> {
    let kind = row.waiting_kind.as_deref()?.trim();
    if kind.is_empty() || kind == "human_ratification" {
        None
    } else {
        Some(kind)
    }
}

fn observation_wait_next(kind: &str) -> String {
    match kind {
        "info_needed" => "answer info".to_string(),
        "external_dependency" => "dependency".to_string(),
        "triage_capacity" => "capacity".to_string(),
        "human" => "human".to_string(),
        other => other.replace('_', "-"),
    }
}

fn observation_flow_next(row: &ObsRow) -> String {
    if obs_is_closed_exit(row) {
        return "done".to_string();
    }
    if observation_has_architecture_gate(row) {
        return "architecture".to_string();
    }
    if matches!(row.contract_state.as_deref(), Some("draft"))
        || matches!(row.waiting_kind.as_deref(), Some("human_ratification"))
    {
        return "approve/revise".to_string();
    }
    if matches!(row.contract_state.as_deref(), Some("ready" | "approved")) {
        return "promote/resolve".to_string();
    }
    match obs_lifecycle(row) {
        "candidate" => "triage".to_string(),
        "ready" | "investigating" => "gather".to_string(),
        "in_progress" => "resolve".to_string(),
        _ => observation_watch_next_action(&observation_presentation(row).label).to_string(),
    }
}

fn observation_has_contract(row: &ObsRow) -> bool {
    row.contract_state
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        || matches!(row.waiting_kind.as_deref(), Some("human_ratification"))
}

fn observation_has_architecture_gate(row: &ObsRow) -> bool {
    row.pending_architecture_review.unwrap_or(false)
        || row
            .open_architecture_review_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
}

fn observation_is_terminal(row: &ObsRow) -> bool {
    obs_is_closed_exit(row)
}

fn observation_link(row: &ObsRow) -> Option<String> {
    row.task_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            row.open_architecture_review_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            row.superseded_by_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

fn task_watch_stage(label: &str) -> &'static str {
    match label {
        "done" => "done",
        "ship" => "ship",
        "queued" => "queued",
        "plan" => "plan",
        "plan-gate" => "plan-gate",
        "exec" => "exec",
        "code-gate" => "code-gate",
        "accept" => "accept",
        "work" => "work",
        "waiting-capacity" => "waiting-capacity",
        "waiting-dependency" => "waiting-dependency",
        "runner-failed" => "runner-failed",
        "rate-limited" => "rate-limited",
        "needs-human" => "needs-human",
        "review-blocked" => "review-blocked",
        "stale-base" => "stale-base",
        "config-fault" => "config-fault",
        "tests-failed" => "tests-failed",
        "main-red" => "main-red",
        "deploy-failed" => "deploy-failed",
        "migration-failed" => "migration-failed",
        _ => "blocked",
    }
}

fn watch_slot_id(severity: PresentationSeverity) -> WatchSlotId {
    match severity {
        PresentationSeverity::Front => WatchSlotId::Front,
        PresentationSeverity::Work => WatchSlotId::Work,
        PresentationSeverity::Gate => WatchSlotId::Gate,
        PresentationSeverity::Exit => WatchSlotId::Exit,
        PresentationSeverity::Wait => WatchSlotId::Wait,
        PresentationSeverity::Fault => WatchSlotId::Fault,
    }
}

pub fn task_watch_slot_label(slot: WatchSlotId) -> &'static str {
    match slot {
        WatchSlotId::Front => "queued",
        WatchSlotId::Work => "working",
        WatchSlotId::Gate => "gate",
        WatchSlotId::Exit => "done",
        WatchSlotId::Wait => "waiting",
        WatchSlotId::Fault => "failed",
    }
}

pub fn observation_watch_slot_label(slot: WatchSlotId) -> &'static str {
    match slot {
        WatchSlotId::Front => "candidates",
        WatchSlotId::Work => "investigate",
        WatchSlotId::Gate => "contract gate",
        WatchSlotId::Exit => "closed",
        WatchSlotId::Wait => "waiting",
        WatchSlotId::Fault => "errors",
    }
}

fn watch_attention(slot: WatchSlotId) -> WatchAttention {
    match slot {
        WatchSlotId::Exit => WatchAttention::Exhaust,
        WatchSlotId::Fault => WatchAttention::Fault,
        WatchSlotId::Front | WatchSlotId::Work | WatchSlotId::Gate | WatchSlotId::Wait => {
            WatchAttention::Flow
        }
    }
}

pub fn task_presentation(task: &TaskRow) -> Presentation {
    if task_is_terminal_primary(task) {
        return presentation("■", "done", PresentationSeverity::Exit, None);
    }
    if task_is_blocked(task) {
        return blocked_task_presentation(task);
    }

    match task_lifecycle(task) {
        "integration" => presentation("▱", "ship", PresentationSeverity::Gate, None),
        "queued" => presentation(
            "◌",
            "queued",
            PresentationSeverity::Front,
            task_signal(task),
        ),
        "active" => match task_active_step(task) {
            "planning" => presentation("◆", "plan", PresentationSeverity::Work, task_signal(task)),
            "planning_review" => presentation(
                "◇",
                "plan-gate",
                PresentationSeverity::Gate,
                task_signal(task),
            ),
            "coding" => presentation("▣", "exec", PresentationSeverity::Work, task_signal(task)),
            "coding_review" => presentation(
                "◈",
                "code-gate",
                PresentationSeverity::Gate,
                task_signal(task),
            ),
            "wrapping" => {
                presentation("▣", "accept", PresentationSeverity::Gate, task_signal(task))
            }
            _ => presentation("◆", "work", PresentationSeverity::Work, task_signal(task)),
        },
        _ => presentation(
            "◌",
            "queued",
            PresentationSeverity::Front,
            task_signal(task),
        ),
    }
}

fn blocked_task_presentation(task: &TaskRow) -> Presentation {
    let kind = task
        .blocker_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .or_else(|| {
            task.blocked_reason_class
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "none")
        })
        .unwrap_or("blocked");
    let (glyph, label, severity) = match kind {
        "capacity" => (
            "△",
            "waiting-capacity".to_string(),
            PresentationSeverity::Wait,
        ),
        "dependency" => (
            "△",
            "waiting-dependency".to_string(),
            PresentationSeverity::Wait,
        ),
        "runner" => (
            "▲",
            "runner-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        "rate_limit" => ("△", "rate-limited".to_string(), PresentationSeverity::Wait),
        "human_acceptance" => ("⋯", "needs-human".to_string(), PresentationSeverity::Gate),
        "task_review" => (
            "▲",
            "review-blocked".to_string(),
            PresentationSeverity::Fault,
        ),
        "stale_base" => ("△", "stale-base".to_string(), PresentationSeverity::Wait),
        "config" => ("▲", "config-fault".to_string(), PresentationSeverity::Fault),
        "test_failure" => ("▲", "tests-failed".to_string(), PresentationSeverity::Fault),
        "main_red" => ("▲", "main-red".to_string(), PresentationSeverity::Fault),
        "deploy" => (
            "▲",
            "deploy-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        "migration" => (
            "▲",
            "migration-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        other => (
            "▲",
            format!("{}-blocked", other.replace('_', "-")),
            PresentationSeverity::Fault,
        ),
    };
    Presentation {
        glyph,
        label,
        severity,
        signal: blocked_signal(task).or_else(|| task_signal(task)),
    }
}

fn task_signal(task: &TaskRow) -> Option<String> {
    if task
        .workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
        && matches!(task_lifecycle(task), "queued" | "active")
    {
        return Some("no worktree".to_string());
    }
    if task_lifecycle(task) == "integration" {
        let step = task_integration_step(task);
        if step != "none" {
            return Some(step.replace('_', "-"));
        }
    }
    None
}

fn blocked_signal(task: &TaskRow) -> Option<String> {
    let raw = task.blocked_reason.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Some(code) = value.get("exit_code").and_then(Value::as_i64) {
            return Some(format!("exit {code}"));
        }
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            return Some(kind.replace('_', "-"));
        }
    }
    Some(raw.to_string())
}

pub fn observation_presentation(row: &ObsRow) -> Presentation {
    if obs_is_closed_exit(row) {
        if row
            .superseded_by_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            return presentation("■", "superseded", PresentationSeverity::Exit, None);
        }
        return match row.outcome.as_deref().unwrap_or(row.status.as_str()) {
            "closed_wont_fix" | "wont_fix" | "wont-fix" => {
                presentation("×", "wont-fix", PresentationSeverity::Exit, None)
            }
            "superseded" => presentation("■", "superseded", PresentationSeverity::Exit, None),
            _ => presentation("✓", "addressed", PresentationSeverity::Exit, None),
        };
    }
    if row.status == "investigation_failed" || row.investigation_failure_reason.is_some() {
        return presentation(
            "▲",
            "investigation-failed",
            PresentationSeverity::Fault,
            row.investigation_failure_reason.clone(),
        );
    }
    if row.pending_architecture_review.unwrap_or(false)
        || row
            .open_architecture_review_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    {
        return presentation("◈", "arch-gate", PresentationSeverity::Gate, None);
    }
    if matches!(row.waiting_kind.as_deref(), Some("human_ratification")) {
        return presentation(
            "◈",
            "contract-draft",
            PresentationSeverity::Gate,
            Some("contract draft".to_string()),
        );
    }
    if matches!(row.contract_state.as_deref(), Some("draft")) {
        return presentation(
            "◈",
            "contract-draft",
            PresentationSeverity::Gate,
            Some("contract draft".to_string()),
        );
    }
    if matches!(row.contract_state.as_deref(), Some("approved" | "ready")) {
        return presentation("▰", "contract-approved", PresentationSeverity::Gate, None);
    }
    if let Some(kind) = row
        .waiting_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return presentation(
            "⋯",
            observation_waiting_label(kind),
            PresentationSeverity::Wait,
            Some("waiting".to_string()),
        );
    }
    match obs_lifecycle(row) {
        "candidate" => presentation(
            "◌",
            "candidate",
            PresentationSeverity::Front,
            Some("needs triage".to_string()),
        ),
        "ready" | "investigating" => presentation(
            "◆",
            "investigate",
            PresentationSeverity::Work,
            contract_signal(row),
        ),
        "in_progress" => presentation("▣", "resolving", PresentationSeverity::Work, None),
        _ => presentation(
            "◆",
            "investigate",
            PresentationSeverity::Work,
            contract_signal(row),
        ),
    }
}

fn obs_is_closed_exit(row: &ObsRow) -> bool {
    row.superseded_by_id
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        || matches!(
            row.outcome.as_deref(),
            Some("superseded" | "closed_wont_fix" | "wont_fix" | "wont-fix")
        )
        || row.lifecycle.as_deref() == Some("closed")
        || matches!(
            row.status.as_str(),
            "resolved" | "closed_wont_fix" | "wont_fix" | "wont-fix" | "rejected" | "superseded"
        )
}

fn observation_waiting_label(kind: &str) -> &'static str {
    match kind {
        "info_needed" => "needs-info",
        "external_dependency" => "external-dependency",
        "triage_capacity" => "triage-capacity",
        "human" => "needs-human",
        _ => "waiting",
    }
}

fn observation_watch_stage(label: &str) -> &'static str {
    match label {
        "candidate" => "candidate",
        "investigate" => "investigate",
        "resolving" => "resolve",
        "contract-draft" => "draft",
        "contract-approved" => "approved",
        "arch-gate" => "architecture",
        "needs-info" => "info needed",
        "external-dependency" => "external dependency",
        "triage-capacity" => "triage capacity",
        "needs-human" => "human",
        "addressed" => "addressed",
        "wont-fix" => "wont-fix",
        "superseded" => "superseded",
        "investigation-failed" => "investigation failed",
        _ => "waiting",
    }
}

fn observation_watch_next_action(label: &str) -> &'static str {
    match label {
        "candidate" => "triage",
        "investigate" => "gather evidence",
        "needs-info" => "answer info",
        "external-dependency" => "check dependency",
        "triage-capacity" => "assign triage",
        "contract-draft" => "approve/revise",
        "contract-approved" => "promote/resolve",
        "arch-gate" => "architecture review",
        "resolving" => "resolve",
        "investigation-failed" => "inspect failure",
        "addressed" | "wont-fix" | "superseded" => "done",
        _ => "triage",
    }
}

fn contract_signal(row: &ObsRow) -> Option<String> {
    match row.contract_state.as_deref() {
        Some("draft") => Some("contract draft".to_string()),
        Some("approved" | "ready") => Some("contract approved".to_string()),
        _ => None,
    }
}

pub fn intake_presentation(row: &IntakeRow) -> Presentation {
    let lifecycle = row.lifecycle.as_deref().unwrap_or(row.status.as_str());
    if lifecycle == "closed" {
        return match row
            .outcome
            .as_deref()
            .or(row.decision.as_deref())
            .unwrap_or(row.status.as_str())
        {
            "routed_to_observation" | "routed" => {
                presentation("✓", "routed", PresentationSeverity::Exit, route_signal(row))
            }
            "escalated_to_architecture_review" | "architecture_review" => presentation(
                "◈",
                "arch-review",
                PresentationSeverity::Gate,
                route_signal(row),
            ),
            "marked_duplicate" | "duplicate" => presentation(
                "≡",
                "duplicate",
                PresentationSeverity::Exit,
                route_signal(row),
            ),
            "dropped_as_noise" | "dropped" => {
                presentation("×", "dropped", PresentationSeverity::Exit, None)
            }
            _ => presentation("✓", "routed", PresentationSeverity::Exit, route_signal(row)),
        };
    }
    match lifecycle {
        "new" | "draft" => presentation("◌", "new", PresentationSeverity::Front, None),
        "triaging" => presentation("◆", "triage", PresentationSeverity::Work, None),
        "waiting" | "needs_info" => presentation(
            "?",
            "needs-info",
            PresentationSeverity::Gate,
            row.missing_info_question.clone(),
        ),
        _ => presentation("◌", "new", PresentationSeverity::Front, None),
    }
}

fn route_signal(row: &IntakeRow) -> Option<String> {
    row.routed_to_observation
        .clone()
        .or_else(|| row.produced_observation_id.clone())
        .or_else(|| row.routed_to_arch_review.clone())
        .or_else(|| row.produced_architecture_review_id.clone())
        .or_else(|| row.duplicate_of.clone())
        .or_else(|| row.duplicate_of_id.clone())
}

pub fn external_review_presentation(row: &ReviewRow) -> Presentation {
    match row.status.as_str() {
        "pending" => presentation(
            "◌",
            "pending",
            PresentationSeverity::Front,
            Some("waiting for dispatch".to_string()),
        ),
        "running" => presentation("◆", "running", PresentationSeverity::Work, None),
        "passed" => presentation(
            "✓",
            "passed",
            PresentationSeverity::Exit,
            findings_signal(row),
        ),
        "revise" => presentation(
            "↻",
            "revise",
            PresentationSeverity::Gate,
            findings_signal(row),
        ),
        "tooling_held" | "tool_fault" | "tool-fault" => presentation(
            "▲",
            "tool-fault",
            PresentationSeverity::Fault,
            row.held_reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "none")
                .map(str::to_string),
        ),
        "superseded" => presentation("■", "superseded", PresentationSeverity::Exit, None),
        other => presentation("◌", other, PresentationSeverity::Front, None),
    }
}

pub fn external_review_runner_label(row: &ReviewRow) -> &str {
    let runner = row.runner.trim();
    if runner.is_empty() || runner == "unknown" {
        "—"
    } else {
        runner
    }
}

fn findings_signal(row: &ReviewRow) -> Option<String> {
    match (row.critical_count, row.major_count, row.minor_count) {
        (Some(c), Some(mj), Some(mn)) => Some(format!("{c}/{mj}/{mn} findings")),
        _ => row.findings_count.map(|n| format!("{n} findings")),
    }
}

pub fn engine_presentation(health: &SystemHealth, daemon: &Liveness) -> Presentation {
    let daemon_live = matches!(daemon, Liveness::Live { .. });
    if daemon_live {
        return presentation("✓", "clear", PresentationSeverity::Exit, None);
    }
    if health.unfinished_dispatch_locks > 0 {
        return presentation(
            "▲",
            "daemon down",
            PresentationSeverity::Fault,
            Some(format!("{} locks", health.unfinished_dispatch_locks)),
        );
    }
    presentation("△", "manual", PresentationSeverity::Wait, None)
}

pub fn engine_flow_slots(health: &SystemHealth, daemon: &Liveness) -> Vec<FlowSlotPresentation> {
    let state = engine_presentation(health, daemon);
    vec![
        FlowSlotPresentation {
            slot: PresentationSeverity::Front,
            glyph: "◌",
            label: "dispatch",
            count: Some(0),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Work,
            glyph: "◆",
            label: "runners",
            count: Some(0),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Gate,
            glyph: "◇",
            label: "locks",
            count: Some(health.unfinished_dispatch_locks),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Exit,
            glyph: "✓",
            label: "clear",
            count: if state.label == "clear" {
                Some(1)
            } else {
                Some(0)
            },
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Wait,
            glyph: "△",
            label: if state.label == "manual" {
                "manual"
            } else {
                "wait"
            },
            count: None,
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Fault,
            glyph: "▲",
            label: if state.label == "daemon down" {
                "daemon down"
            } else {
                "fault"
            },
            count: if state.severity == PresentationSeverity::Fault {
                Some(1)
            } else {
                Some(0)
            },
        },
    ]
}

fn presentation(
    glyph: &'static str,
    label: impl Into<String>,
    severity: PresentationSeverity,
    signal: Option<String>,
) -> Presentation {
    Presentation {
        glyph,
        label: label.into(),
        severity,
        signal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(lifecycle: &str, active_step: &str) -> TaskRow {
        TaskRow {
            display_id: "T001".to_string(),
            status: "planning".to_string(),
            lifecycle: Some(lifecycle.to_string()),
            active_step: Some(active_step.to_string()),
            integration_step: Some("none".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn task_blocker_labels_runner_failed_and_waiting_capacity() {
        let runner = TaskRow {
            blocked: Some(true),
            blocker_kind: Some("runner".to_string()),
            blocked_reason: Some(r#"{"exit_code":42,"kind":"runner_crash"}"#.to_string()),
            lifecycle: Some("active".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&runner);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.severity, p.signal.as_deref()),
            (
                "▲",
                "runner-failed",
                PresentationSeverity::Fault,
                Some("exit 42")
            )
        );

        let capacity = TaskRow {
            blocked: Some(true),
            blocker_kind: Some("capacity".to_string()),
            lifecycle: Some("queued".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&capacity);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.severity),
            ("△", "waiting-capacity", PresentationSeverity::Wait)
        );
    }

    #[test]
    fn task_active_step_labels_match_watch_plan() {
        let cases = [
            ("active", "planning", "◆", "plan"),
            ("active", "planning_review", "◇", "plan-gate"),
            ("active", "coding", "▣", "exec"),
            ("active", "coding_review", "◈", "code-gate"),
            ("active", "wrapping", "▣", "accept"),
        ];
        for (lifecycle, step, glyph, label) in cases {
            let p = task_presentation(&task(lifecycle, step));
            assert_eq!((p.glyph, p.label.as_str()), (glyph, label));
        }
        let ship = TaskRow {
            lifecycle: Some("integration".to_string()),
            integration_step: Some("queued".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&ship);
        assert_eq!((p.glyph, p.label.as_str()), ("▱", "ship"));
    }

    #[test]
    fn task_watch_projection_covers_task_slots_and_stages() {
        let cases = [
            (task("queued", "none"), WatchSlotId::Front, "queued"),
            (task("active", "planning"), WatchSlotId::Work, "plan"),
            (task("active", "coding"), WatchSlotId::Work, "exec"),
            (
                task("active", "planning_review"),
                WatchSlotId::Gate,
                "plan-gate",
            ),
            (task("active", "wrapping"), WatchSlotId::Gate, "accept"),
            (
                TaskRow {
                    blocked: Some(true),
                    blocker_kind: Some("capacity".to_string()),
                    lifecycle: Some("queued".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Wait,
                "waiting-capacity",
            ),
            (
                TaskRow {
                    blocked: Some(true),
                    blocker_kind: Some("runner".to_string()),
                    blocked_reason: Some(r#"{"exit_code":42}"#.to_string()),
                    lifecycle: Some("active".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Fault,
                "runner-failed",
            ),
            (
                TaskRow {
                    lifecycle: Some("done".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Exit,
                "done",
            ),
        ];

        for (row, slot, stage) in cases {
            let projection = task_watch_projection(&row);
            assert_eq!((projection.slot, projection.row_stage), (slot, stage));
        }
    }

    fn task_with_map_fields() -> TaskRow {
        TaskRow {
            display_id: "T-map".to_string(),
            status: "planning".to_string(),
            lifecycle: Some("active".to_string()),
            active_step: Some("none".to_string()),
            integration_step: Some("none".to_string()),
            total_phases: Some(3),
            ..Default::default()
        }
    }

    fn plan_gate(gate: PlanReviewGate) -> super::super::data::TaskPlanReviewEntry {
        super::super::data::TaskPlanReviewEntry {
            gate,
            summary: None,
            at: None,
        }
    }

    fn cycle(phase: i64, cycle: i64, gate: Option<CycleReviewGate>) -> TaskCycleEntry {
        TaskCycleEntry {
            phase,
            cycle,
            review_gate: gate,
            ..Default::default()
        }
    }

    fn phase_shapes(
        projection: &TaskMapProjection,
    ) -> Vec<(MapGlyph, Option<i64>, MapColor, bool)> {
        projection
            .phases
            .iter()
            .map(|cell| (cell.glyph, cell.cycle, cell.color_role, cell.active))
            .collect()
    }

    #[test]
    fn task_map_projection_covers_planning_states() {
        let cases = [
            (
                "queued before plan",
                TaskRow {
                    lifecycle: Some("queued".to_string()),
                    active_step: Some("none".to_string()),
                    total_phases: Some(3),
                    ..Default::default()
                },
                MapGlyph::Queued,
                None,
                MapColor::Inactive,
                false,
                MapConfidence::Exact,
            ),
            (
                "planning cycle one",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("planning".to_string()),
                    total_phases: Some(3),
                    ..Default::default()
                },
                MapGlyph::Planning,
                None,
                MapColor::ActiveWork,
                true,
                MapConfidence::Exact,
            ),
            (
                "plan review current",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("planning_review".to_string()),
                    total_phases: Some(3),
                    ..Default::default()
                },
                MapGlyph::PlanReview,
                None,
                MapColor::ActiveGate,
                true,
                MapConfidence::Exact,
            ),
            (
                "plan review attempt three",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("planning_review".to_string()),
                    total_phases: Some(3),
                    plan_review_entries: vec![
                        plan_gate(PlanReviewGate::NeedsWork),
                        plan_gate(PlanReviewGate::NeedsWork),
                        plan_gate(PlanReviewGate::NotReady),
                    ],
                    ..Default::default()
                },
                MapGlyph::PlanReview,
                Some(3),
                MapColor::ActiveGate,
                true,
                MapConfidence::Exact,
            ),
            (
                "plan review passed from READY",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("none".to_string()),
                    total_phases: Some(3),
                    plan_review_entries: vec![plan_gate(PlanReviewGate::Ready)],
                    ..Default::default()
                },
                MapGlyph::PlanReview,
                None,
                MapColor::Passed,
                false,
                MapConfidence::Exact,
            ),
            (
                "plan review failed from NOT_READY",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("none".to_string()),
                    total_phases: Some(3),
                    plan_review_entries: vec![plan_gate(PlanReviewGate::NotReady)],
                    ..Default::default()
                },
                MapGlyph::PlanReview,
                None,
                MapColor::Failed,
                false,
                MapConfidence::Exact,
            ),
            (
                "T1 contract synthesized is not green plan pass",
                TaskRow {
                    lifecycle: Some("active".to_string()),
                    active_step: Some("none".to_string()),
                    total_phases: Some(1),
                    plan_source: Some("contract_synthesized".to_string()),
                    ..Default::default()
                },
                MapGlyph::PlanReview,
                None,
                MapColor::Inactive,
                false,
                MapConfidence::Implied,
            ),
        ];

        for (name, row, glyph, cycle, color, active, confidence) in cases {
            let projection = task_map_projection(&row);
            assert_eq!(projection.planning.glyph, glyph, "{name}");
            assert_eq!(projection.planning.cycle, cycle, "{name}");
            assert_eq!(projection.planning.color_role, color, "{name}");
            assert_eq!(projection.planning.active, active, "{name}");
            assert_eq!(projection.planning.confidence, confidence, "{name}");
        }

        let attempt_three = task_map_projection(&TaskRow {
            lifecycle: Some("active".to_string()),
            active_step: Some("planning_review".to_string()),
            total_phases: Some(3),
            plan_review_entries: vec![
                plan_gate(PlanReviewGate::NeedsWork),
                plan_gate(PlanReviewGate::NeedsWork),
                plan_gate(PlanReviewGate::NotReady),
            ],
            ..Default::default()
        });
        assert_eq!(
            attempt_three.planning.source,
            MapSource::ActiveStepAndPlanReviewLog
        );
    }

    #[test]
    fn task_map_projection_covers_execution_and_history() {
        let cases = [
            (
                "executing phase N cycle M",
                TaskRow {
                    active_step: Some("coding".to_string()),
                    current_phase: Some(2),
                    current_cycle: Some(3),
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::Executing, Some(3), MapColor::ActiveWork, true),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
            (
                "code review phase N cycle M",
                TaskRow {
                    active_step: Some("coding_review".to_string()),
                    current_phase: Some(2),
                    current_cycle: Some(3),
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::CodeReview, Some(3), MapColor::ActiveGate, true),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
            (
                "previous phase pass after earlier revise",
                TaskRow {
                    cycle_entries: vec![
                        cycle(1, 1, Some(CycleReviewGate::Revise)),
                        cycle(1, 2, Some(CycleReviewGate::Pass)),
                    ],
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::CodeReview, Some(2), MapColor::Passed, false),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
            (
                "current executing cycle two after REVISE",
                TaskRow {
                    active_step: Some("coding".to_string()),
                    current_phase: Some(2),
                    current_cycle: Some(2),
                    cycle_entries: vec![cycle(2, 1, Some(CycleReviewGate::Revise))],
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::Executing, Some(2), MapColor::ActiveWork, true),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
            (
                "current code review cycle two",
                TaskRow {
                    active_step: Some("coding_review".to_string()),
                    current_phase: Some(2),
                    current_cycle: Some(2),
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::CodeReview, Some(2), MapColor::ActiveGate, true),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
            (
                "FAIL is exact red structured proof",
                TaskRow {
                    cycle_entries: vec![cycle(2, 1, Some(CycleReviewGate::Fail))],
                    ..task_with_map_fields()
                },
                vec![
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                    (MapGlyph::CodeReview, None, MapColor::Failed, false),
                    (MapGlyph::UnreachedPhase, None, MapColor::Inactive, false),
                ],
            ),
        ];

        for (name, row, expected) in cases {
            let projection = task_map_projection(&row);
            assert_eq!(phase_shapes(&projection), expected, "{name}");
        }
    }

    #[test]
    fn task_map_projection_covers_unknown_blocked_and_wrap() {
        let unknown = TaskRow {
            lifecycle: Some("active".to_string()),
            active_step: Some("none".to_string()),
            total_phases: None,
            ..Default::default()
        };
        let projection = task_map_projection(&unknown);
        assert_eq!(projection.phases.len(), 1);
        assert_eq!(projection.phases[0].glyph, MapGlyph::Unknown);
        assert_eq!(projection.phases[0].confidence, MapConfidence::Unknown);

        let capacity = TaskRow {
            lifecycle: Some("queued".to_string()),
            blocked: Some(true),
            blocker_kind: Some("capacity".to_string()),
            total_phases: Some(1),
            ..Default::default()
        };
        let projection = task_map_projection(&capacity);
        assert_eq!(projection.reason.as_deref(), Some("capacity"));
        let fallback = projection.fallback.as_ref().unwrap();
        assert_eq!(fallback.glyph, MapGlyph::Waiting);
        assert_eq!(fallback.color_role, MapColor::Waiting);

        let runner = TaskRow {
            lifecycle: Some("active".to_string()),
            blocked: Some(true),
            blocker_kind: Some("runner".to_string()),
            total_phases: Some(1),
            ..Default::default()
        };
        let projection = task_map_projection(&runner);
        assert_eq!(projection.reason.as_deref(), Some("runner"));
        let fallback = projection.fallback.as_ref().unwrap();
        assert_eq!(fallback.glyph, MapGlyph::Fault);
        assert_eq!(fallback.color_role, MapColor::Failed);

        let wrapping = TaskRow {
            lifecycle: Some("active".to_string()),
            active_step: Some("wrapping".to_string()),
            total_phases: Some(1),
            ..Default::default()
        };
        let projection = task_map_projection(&wrapping);
        let wrap = projection.wrap.as_ref().unwrap();
        assert_eq!(wrap.glyph, MapGlyph::Wrap);
        assert_eq!(wrap.color_role, MapColor::ActiveGate);
        assert!(wrap.active);

        let accepted = TaskRow {
            lifecycle: Some("done".to_string()),
            total_phases: Some(1),
            ..Default::default()
        };
        let projection = task_map_projection(&accepted);
        let wrap = projection.wrap.as_ref().unwrap();
        assert_eq!(wrap.glyph, MapGlyph::Wrap);
        assert_eq!(wrap.color_role, MapColor::Passed);
        assert!(!wrap.active);
    }

    #[test]
    fn observation_contract_labels_are_semantic() {
        let draft = ObsRow {
            lifecycle: Some("candidate".to_string()),
            waiting_kind: Some("human_ratification".to_string()),
            ..Default::default()
        };
        assert_eq!(observation_presentation(&draft).label, "contract-draft");

        let approved = ObsRow {
            contract_state: Some("approved".to_string()),
            ..Default::default()
        };
        assert_eq!(
            observation_presentation(&approved).label,
            "contract-approved"
        );

        let ready = ObsRow {
            contract_state: Some("ready".to_string()),
            ..Default::default()
        };
        assert_eq!(observation_presentation(&ready).label, "contract-approved");

        let info = ObsRow {
            waiting_kind: Some("info_needed".to_string()),
            ..Default::default()
        };
        let p = observation_presentation(&info);
        assert_eq!((p.glyph, p.label.as_str()), ("⋯", "needs-info"));
    }

    #[test]
    fn observation_flow_projection_covers_dense_flow_states() {
        let cases = [
            (
                "candidate",
                ObsRow {
                    status: "open".to_string(),
                    lifecycle: Some("candidate".to_string()),
                    ..Default::default()
                },
                vec![
                    ObservationGlyph::Candidate,
                    ObservationGlyph::Unreached,
                    ObservationGlyph::Unreached,
                ],
                "triage",
            ),
            (
                "investigating",
                ObsRow {
                    status: "open".to_string(),
                    lifecycle: Some("investigating".to_string()),
                    evidence_pointers: vec![crate::tui::data::ArtifactPointer {
                        label: "log".to_string(),
                        value: "x".to_string(),
                    }],
                    ..Default::default()
                },
                vec![
                    ObservationGlyph::Evidence,
                    ObservationGlyph::Unreached,
                    ObservationGlyph::Unreached,
                ],
                "gather",
            ),
            (
                "contract draft",
                ObsRow {
                    status: "open".to_string(),
                    contract_state: Some("draft".to_string()),
                    ..Default::default()
                },
                vec![
                    ObservationGlyph::Evidence,
                    ObservationGlyph::Contract,
                    ObservationGlyph::Unreached,
                ],
                "approve/revise",
            ),
            (
                "arch gate",
                ObsRow {
                    status: "open".to_string(),
                    contract_state: Some("ready".to_string()),
                    open_architecture_review_id: Some("A003".to_string()),
                    ..Default::default()
                },
                vec![
                    ObservationGlyph::Evidence,
                    ObservationGlyph::Contract,
                    ObservationGlyph::Architecture,
                    ObservationGlyph::Unreached,
                ],
                "architecture",
            ),
            (
                "resolved",
                ObsRow {
                    status: "resolved".to_string(),
                    lifecycle: Some("closed".to_string()),
                    task_id: Some("T020".to_string()),
                    ..Default::default()
                },
                vec![
                    ObservationGlyph::Evidence,
                    ObservationGlyph::Contract,
                    ObservationGlyph::Resolved,
                ],
                "done",
            ),
        ];

        for (name, row, glyphs, next) in cases {
            let projection = observation_flow_projection(&row);
            let actual: Vec<ObservationGlyph> =
                projection.cells.iter().map(|cell| cell.glyph).collect();
            assert_eq!(actual, glyphs, "{name}");
            assert_eq!(projection.next.as_deref(), Some(next), "{name}");
        }

        let waiting = observation_flow_projection(&ObsRow {
            status: "open".to_string(),
            waiting_kind: Some("info_needed".to_string()),
            ..Default::default()
        });
        assert_eq!(waiting.cells[0].glyph, ObservationGlyph::Waiting);
        assert_eq!(waiting.reason.as_deref(), Some("info_needed"));

        let failed = observation_flow_projection(&ObsRow {
            status: "investigation_failed".to_string(),
            investigation_failure_reason: Some("runner".to_string()),
            ..Default::default()
        });
        assert_eq!(failed.cells[0].glyph, ObservationGlyph::Fault);
        assert_eq!(failed.next.as_deref(), Some("inspect"));
    }

    #[test]
    fn observation_watch_projection_covers_observation_slots_and_precedence() {
        let cases = [
            (
                ObsRow {
                    status: "open".to_string(),
                    ..Default::default()
                },
                WatchSlotId::Front,
                "candidate",
            ),
            (
                ObsRow {
                    lifecycle: Some("investigating".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Work,
                "investigate",
            ),
            (
                ObsRow {
                    contract_state: Some("draft".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Gate,
                "draft",
            ),
            (
                ObsRow {
                    contract_state: Some("approved".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Gate,
                "approved",
            ),
            (
                ObsRow {
                    contract_state: Some("ready".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Gate,
                "approved",
            ),
            (
                ObsRow {
                    waiting_kind: Some("info_needed".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Wait,
                "info needed",
            ),
            (
                ObsRow {
                    pending_architecture_review: Some(true),
                    waiting_kind: Some("info_needed".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Gate,
                "architecture",
            ),
            (
                ObsRow {
                    lifecycle: Some("closed".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Exit,
                "addressed",
            ),
            (
                ObsRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("closed_wont_fix".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Exit,
                "wont-fix",
            ),
            (
                ObsRow {
                    status: "investigation_failed".to_string(),
                    investigation_failure_reason: Some("tool fault".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Fault,
                "investigation failed",
            ),
        ];

        for (row, slot, stage) in cases {
            let projection = observation_watch_projection(&row);
            assert_eq!((projection.slot, projection.row_stage), (slot, stage));
        }
    }

    #[test]
    fn investigation_failure_beats_schema_failure_gate_shape() {
        let row = ObsRow {
            status: "investigation_failed".to_string(),
            lifecycle: Some("candidate".to_string()),
            contract_state: Some("draft".to_string()),
            waiting_kind: Some("human_ratification".to_string()),
            investigation_failure_reason: Some("schema handler raised".to_string()),
            ..Default::default()
        };

        let projection = observation_watch_projection(&row);
        assert_eq!(projection.slot, WatchSlotId::Fault);
        assert_eq!(projection.slot_label, "errors");
        assert_eq!(projection.row_stage, "investigation failed");
        assert_eq!(projection.next_action, Some("inspect failure"));
        assert_eq!(
            projection.row_signal.as_deref(),
            Some("schema handler raised")
        );
    }

    #[test]
    fn intake_labels_cover_front_work_gate_exit() {
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("new".to_string()),
                ..Default::default()
            })
            .label,
            "new"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("triaging".to_string()),
                ..Default::default()
            })
            .label,
            "triage"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("waiting".to_string()),
                ..Default::default()
            })
            .label,
            "needs-info"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("closed".to_string()),
                outcome: Some("marked_duplicate".to_string()),
                ..Default::default()
            })
            .label,
            "duplicate"
        );
    }

    #[test]
    fn external_review_labels_and_unknown_runner_are_clean() {
        let pending = ReviewRow {
            status: "pending".to_string(),
            runner: "unknown".to_string(),
            ..Default::default()
        };
        let p = external_review_presentation(&pending);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.signal.as_deref()),
            ("◌", "pending", Some("waiting for dispatch"))
        );
        assert_eq!(external_review_runner_label(&pending), "—");

        let tooling = ReviewRow {
            status: "tooling_held".to_string(),
            held_reason: Some("missing wrap brief".to_string()),
            ..Default::default()
        };
        assert_eq!(external_review_presentation(&tooling).label, "tool-fault");
    }

    #[test]
    fn engine_labels_manual_vs_fault() {
        let manual = engine_presentation(&SystemHealth::default(), &Liveness::Dead);
        assert_eq!(
            (manual.glyph, manual.label.as_str(), manual.severity),
            ("△", "manual", PresentationSeverity::Wait)
        );

        let fault = engine_presentation(
            &SystemHealth {
                unfinished_dispatch_locks: 2,
                oldest_claimed_at_epoch: None,
            },
            &Liveness::Dead,
        );
        assert_eq!(
            (
                fault.glyph,
                fault.label.as_str(),
                fault.severity,
                fault.signal.as_deref()
            ),
            (
                "▲",
                "daemon down",
                PresentationSeverity::Fault,
                Some("2 locks")
            )
        );
    }
}
