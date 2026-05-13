//! Read-only drilldown renderers for task, observation, and intake rows.

use ratatui::text::Line;

use super::app::App;
use super::data::{
    ArtifactPointer, CollapsedObsRow, CycleReviewGate, IntakeRow, ObsRow, PlanReviewGate,
    RecentEvent, ReviewRow, Row, TaskCycleEntry, TaskRow,
};
use super::semantics::{
    task_map_projection, MapCell, MapConfidence, MapGlyph, MapSource, TaskMapProjection,
};

fn truncate_sha(sha: Option<&str>) -> String {
    sha.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(12).collect::<String>())
        .unwrap_or_else(|| "—".to_string())
}

pub fn selected_detail_lines(app: &App) -> Vec<Line<'static>> {
    let Some(detail) = &app.detail else {
        return vec![Line::from("No detail selected")];
    };
    let Some(row) = app
        .rows
        .iter()
        .find(|r| r.display_id() == detail.display_id)
    else {
        return vec![Line::from(format!("{} not found", detail.display_id))];
    };
    lines_for_row(row, app)
        .into_iter()
        .skip(detail.scroll_offset)
        .map(Line::from)
        .collect()
}

pub fn render_text_for_row(row: &Row, app: &App) -> String {
    lines_for_row(row, app).join("\n")
}

pub(super) fn lines_for_row(row: &Row, app: &App) -> Vec<String> {
    match row {
        Row::Task(t) => task_lines(t, app),
        Row::Obs(o) => observation_lines(o, app),
        Row::CollapsedObs(c) => collapsed_observation_lines(c, app),
        Row::Review(r) => review_lines(r),
        Row::Intake(i) => intake_lines(i),
    }
}

fn task_lines(t: &TaskRow, app: &App) -> Vec<String> {
    let progress = super::progress::task_progress(t, &app.external_review).text;
    let state = super::semantics::task_presentation(t);
    let mut lines = vec![
        format!("Task detail · {}", t.display_id),
        String::new(),
        "Operator state".to_string(),
        format!(
            "  {} {}{}",
            state.glyph,
            state.label,
            signal_suffix(state.signal.as_deref())
        ),
        format!("  next valve: {}", task_next_valve(t, &state)),
        format!("  priority/tier: {}", present_opt(t.tier_hint.as_deref())),
        format!(
            "  claimed: {} by {}",
            present_opt(t.claimed_at.as_deref()),
            present_opt(t.claimed_by.as_deref())
        ),
    ];
    append_live_runner(&mut lines, t);
    lines.extend([
        String::new(),
        "Story summary".to_string(),
        format!("  {}", present(&t.title)),
        String::new(),
        "Why it matters".to_string(),
        format!(
            "  done_when: {}",
            present_opt(t.contract_done_when.as_deref())
        ),
        format!(
            "  executive_intent: {}",
            present_opt(t.contract_executive_intent.as_deref())
        ),
        String::new(),
        "Progress".to_string(),
        format!("  {progress}"),
        format!(
            "  phase: {} / {}",
            opt_i64(t.current_phase),
            opt_i64(t.total_phases)
        ),
        format!("  cycle: {}", opt_i64(t.current_cycle)),
    ]);
    append_task_map_decode(&mut lines, t);
    lines.extend([
        String::new(),
        "Blockers / held reasons".to_string(),
        format!("  {}", present_opt(t.blocked_reason.as_deref())),
        String::new(),
        "Recent events".to_string(),
    ]);
    append_events(&mut lines, &t.recent_events);
    lines.extend([
        String::new(),
        "Artifact pointers".to_string(),
        format!("  rendered task: tasks/**/{}/main.md", t.display_id),
        format!("  branch: {}", present_opt(t.branch.as_deref())),
        format!("  workspace: {}", present_opt(t.workspace_path.as_deref())),
        format!(
            "  linked observations: {}",
            list_or_dash(&t.linked_observations)
        ),
        format!(
            "  plan review log: {} item(s)",
            t.plan_review_summaries.len()
        ),
        format!("  cycles: {} item(s)", t.cycle_summaries.len()),
        format!("  wrap log: {} item(s)", t.wrap_summaries.len()),
    ]);
    append_artifacts(&mut lines, &t.artifact_pointers);
    if t.integration_attempts_count > 0 {
        lines.extend([
            String::new(),
            "Integration-attempts".to_string(),
            format!("  attempts: {}", t.integration_attempts_count),
            format!(
                "  last_outcome: {}",
                present_opt(t.last_integration_outcome.as_deref())
            ),
        ]);
    }
    append_task_debug_tuple(&mut lines, t);
    lines
}

fn append_task_map_decode(lines: &mut Vec<String>, t: &TaskRow) {
    let projection = task_map_projection(t);
    lines.extend([String::new(), "Task map".to_string()]);
    lines.push(format!(
        "  planning: {} attempts={} latest_gate={} source={} confidence={}{}",
        map_cell_label(&projection.planning),
        t.plan_review_entries.len(),
        latest_plan_gate(t),
        map_source_label(&projection.planning.source),
        map_confidence_label(projection.planning.confidence),
        active_marker(&projection.planning),
    ));
    lines.push(format!(
        "  plan_review: gate={} source={} confidence={}",
        latest_plan_gate(t),
        plan_review_source_label(t, &projection.planning),
        map_confidence_label(projection.planning.confidence)
    ));
    if projection.phases.is_empty() {
        lines.push("  phases: —".to_string());
    } else {
        for (idx, cell) in projection.phases.iter().enumerate() {
            let phase = (idx + 1) as i64;
            lines.push(format!(
                "  phase {phase}: {} cycle={} gate={} source={} confidence={}{}",
                map_cell_label(cell),
                opt_i64(cell.cycle),
                latest_cycle_gate_for_phase(t, phase),
                map_source_label(&cell.source),
                map_confidence_label(cell.confidence),
                active_marker(cell),
            ));
        }
    }
    if let Some(wrap) = projection.wrap.as_ref() {
        lines.push(format!(
            "  wrap: {} source={} confidence={}{}",
            map_cell_label(wrap),
            map_source_label(&wrap.source),
            map_confidence_label(wrap.confidence),
            active_marker(wrap),
        ));
    }
    append_task_map_fallback(lines, &projection);
    lines.push(format!(
        "  projection_confidence: {}",
        map_confidence_label(projection.confidence)
    ));
}

fn append_task_map_fallback(lines: &mut Vec<String>, projection: &TaskMapProjection) {
    let Some(fallback) = projection.fallback.as_ref() else {
        if projection.reason.is_some() {
            lines.push(format!(
                "  reason: {} source=unknown confidence=unknown",
                present_opt(projection.reason.as_deref())
            ));
        }
        return;
    };
    lines.push(format!(
        "  fallback: {} reason={} source={} confidence={}",
        map_cell_label(fallback),
        present_opt(projection.reason.as_deref()),
        map_source_label(&fallback.source),
        map_confidence_label(fallback.confidence)
    ));
}

fn map_cell_label(cell: &MapCell) -> String {
    let mut label = map_glyph_label(cell.glyph).to_string();
    label.push(' ');
    label.push_str(cell.glyph.symbol());
    if let Some(cycle) = cell.cycle {
        label.push_str(&format!(" cycle {cycle}"));
    }
    label
}

fn latest_plan_gate(t: &TaskRow) -> String {
    t.plan_review_entries
        .last()
        .map(|entry| plan_gate_label(&entry.gate).to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn plan_review_source_label(t: &TaskRow, planning: &MapCell) -> &'static str {
    if t.plan_review_entries.is_empty() {
        map_source_label(&planning.source)
    } else {
        "plan_review_log"
    }
}

fn latest_cycle_gate_for_phase(t: &TaskRow, phase: i64) -> String {
    latest_cycle_for_phase(t, phase)
        .and_then(|entry| entry.review_gate.as_ref())
        .map(cycle_gate_label)
        .unwrap_or("—")
        .to_string()
}

fn latest_cycle_for_phase(t: &TaskRow, phase: i64) -> Option<&TaskCycleEntry> {
    t.cycle_entries
        .iter()
        .filter(|entry| entry.phase == phase && entry.cycle > 0)
        .max_by_key(|entry| entry.cycle)
}

fn plan_gate_label(gate: &PlanReviewGate) -> &str {
    match gate {
        PlanReviewGate::Ready => "READY",
        PlanReviewGate::NeedsWork => "NEEDS_WORK",
        PlanReviewGate::NotReady => "NOT_READY",
        PlanReviewGate::Unknown(value) if value.trim().is_empty() => "UNKNOWN",
        PlanReviewGate::Unknown(value) => value.as_str(),
    }
}

fn cycle_gate_label(gate: &CycleReviewGate) -> &str {
    match gate {
        CycleReviewGate::Pass => "PASS",
        CycleReviewGate::Revise => "REVISE",
        CycleReviewGate::Fail => "FAIL",
        CycleReviewGate::Unknown(value) if value.trim().is_empty() => "UNKNOWN",
        CycleReviewGate::Unknown(value) => value.as_str(),
    }
}

fn map_glyph_label(glyph: MapGlyph) -> &'static str {
    match glyph {
        MapGlyph::Queued => "queued",
        MapGlyph::Planning => "planning",
        MapGlyph::PlanReview => "plan-review",
        MapGlyph::UnreachedPhase => "unreached",
        MapGlyph::Executing => "executing",
        MapGlyph::CodeReview => "code-review",
        MapGlyph::Wrap => "wrap",
        MapGlyph::Waiting => "waiting",
        MapGlyph::Fault => "fault",
        MapGlyph::Unknown => "unknown",
    }
}

fn map_source_label(source: &MapSource) -> &'static str {
    match source {
        MapSource::Lifecycle => "lifecycle",
        MapSource::ActiveStep => "active_step",
        MapSource::ActiveStepAndPlanReviewLog => "active_step+plan_review_log",
        MapSource::CurrentPhaseCycle => "current_phase/current_cycle",
        MapSource::TotalPhases => "total_phases",
        MapSource::PlanReviewLog => "plan_review_log",
        MapSource::Cycles => "cycles",
        MapSource::PlanSource => "plan",
        MapSource::Blocker => "blocker",
        MapSource::TerminalLifecycle => "terminal_lifecycle",
        MapSource::MissingEvidence => "missing_evidence",
    }
}

fn map_confidence_label(confidence: MapConfidence) -> &'static str {
    match confidence {
        MapConfidence::Exact => "exact",
        MapConfidence::Implied => "implied",
        MapConfidence::Unknown => "unknown",
    }
}

fn active_marker(cell: &MapCell) -> &'static str {
    if cell.active {
        " active"
    } else {
        ""
    }
}

fn append_live_runner(lines: &mut Vec<String>, t: &TaskRow) {
    let Some(live) = &t.live_run else {
        return;
    };
    let now = now_epoch_secs();
    let mut summary = format!(
        "  {} · {} · {}",
        present(&live.role),
        present_opt(live.runner.as_deref()),
        present_opt(live.status.as_deref())
    );
    if let Some(ts) = live.last_event_at.as_deref() {
        summary.push_str(&format!(
            " · last event {}",
            crate::tui::footer::relative_time(ts, now)
        ));
    } else if let Some(ts) = live.updated_at.as_deref() {
        summary.push_str(&format!(
            " · updated {}",
            crate::tui::footer::relative_time(ts, now)
        ));
    }
    if let Some(kind) = live.last_event_type.as_deref() {
        summary.push_str(&format!(" · {kind}"));
    }
    if let Some(activity) = live.current_activity.as_deref() {
        summary.push_str(&format!(" · {activity}"));
    }
    lines.extend([String::new(), "Live runner".to_string(), summary]);
    append_present_path(lines, "marker_path", live.marker_path.as_deref());
    append_present_path(lines, "status_path", live.status_path.as_deref());
    append_present_path(lines, "events_path", live.events_path.as_deref());
    append_present_path(lines, "transcript_path", live.transcript_path.as_deref());
    append_present_path(lines, "stderr_log_path", live.stderr_log_path.as_deref());
    lines.push(String::new());
    lines.push(format!("Live activity · last {}", live.events.len().max(1)));
    if live.events.is_empty() {
        lines.push("  runner alive; no semantic activity yet".to_string());
        return;
    }
    for event in &live.events {
        let age = event
            .ts
            .as_deref()
            .map(|ts| crate::tui::footer::relative_time(ts, now))
            .unwrap_or_else(|| "?".to_string());
        let text = if event.text.trim().is_empty() {
            event.label.clone()
        } else {
            format!("{}   {}", event.label, event.text)
        };
        lines.push(format!("  {:<7} {}", age, text));
    }
}

fn append_present_path(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) {
        lines.push(format!("  {label}: {value}"));
    }
}

fn append_nonempty_line(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none" && *s != "null")
    {
        lines.push(format!("  {label}: {value}"));
    }
}

fn append_task_debug_tuple(lines: &mut Vec<String>, t: &TaskRow) {
    lines.extend([
        String::new(),
        "Debug tuple".to_string(),
        format!("  status: {}", present(&t.status)),
        format!("  lifecycle: {}", present_opt(t.lifecycle.as_deref())),
        format!("  active_step: {}", present_opt(t.active_step.as_deref())),
        format!(
            "  integration_step: {}",
            present_opt(t.integration_step.as_deref())
        ),
        format!("  activation: {}", present_opt(t.activation.as_deref())),
        format!("  blocked: {}", opt_bool(t.blocked)),
        format!("  blocker_kind: {}", present_opt(t.blocker_kind.as_deref())),
        format!(
            "  blocked_reason: {}",
            present_opt(t.blocked_reason.as_deref())
        ),
    ]);
}

fn task_next_valve(t: &TaskRow, state: &super::semantics::Presentation) -> String {
    if let Some(kind) = t.blocker_kind.as_deref().filter(|s| !s.trim().is_empty()) {
        return format!("clear {kind}");
    }
    match state.label.as_str() {
        "queued" => "activate work".to_string(),
        "plan" => "plan review".to_string(),
        "plan-gate" => "approve plan".to_string(),
        "exec" => "code review".to_string(),
        "code-gate" => "resolve review".to_string(),
        "accept" => "human acceptance".to_string(),
        "ship" => "integration lane".to_string(),
        "done" => "terminal".to_string(),
        other => other.to_string(),
    }
}

fn signal_suffix(signal: Option<&str>) -> String {
    signal
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(" · {s}"))
        .unwrap_or_default()
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn observation_lines(o: &ObsRow, app: &App) -> Vec<String> {
    let mut lines = vec![
        format!("Observation detail · {}", o.display_id),
        String::new(),
        "Summary / story".to_string(),
        format!("  summary: {}", present(&o.summary)),
        format!("  story: {}", present_opt(o.body.as_deref())),
        String::new(),
        "Priority".to_string(),
        format!("  {}", present(&o.priority)),
        String::new(),
        "ADR 0002 state".to_string(),
        format!("  lifecycle: {}", present_opt(o.lifecycle.as_deref())),
        format!(
            "  contract_state: {}",
            present_opt(o.contract_state.as_deref())
        ),
        format!("  waiting: {}", obs_waiting_label(o)),
        format!("  outcome: {}", present_opt(o.outcome.as_deref())),
        format!(
            "  open_architecture_review_id: {}",
            present_opt(o.open_architecture_review_id.as_deref())
        ),
        format!("  task_id: {}", present_opt(o.task_id.as_deref())),
        format!(
            "  superseded_by_id: {}",
            present_opt(o.superseded_by_id.as_deref())
        ),
        // ADR 0002 compatibility-only T148 task 6.1: display legacy observation status explicitly.
        format!("  Legacy status: {}", present(&o.status)),
        String::new(),
        "Contract state".to_string(),
        format!("  state: {}", present_opt(o.contract_state.as_deref())),
        format!(
            "  objective: {}",
            present_opt(o.intent_objective.as_deref())
        ),
        format!("  tier: {}", present_opt(o.tier_hint.as_deref())),
        format!(
            "  auto-promote: {}",
            if o.contract_state.as_deref() == Some("ready") {
                "contract ready · auto-promote eligible"
            } else {
                "contract draft · not promotable"
            }
        ),
        String::new(),
        "Next action / held reason".to_string(),
        format!("  {}", obs_next_action(o)),
    ];
    let mut linked: Vec<String> = Vec::new();
    if let Some(tid) = o.task_id.as_deref().filter(|s| !s.trim().is_empty()) {
        linked.push(tid.to_string());
    }
    for row in &app.rows {
        if let Row::Task(t) = row {
            if t.linked_observations.iter().any(|id| id == &o.display_id)
                && !linked.iter().any(|id| id == &t.display_id)
            {
                linked.push(t.display_id.clone());
            }
        }
    }
    lines.extend([
        String::new(),
        "Linked tasks".to_string(),
        format!("  {}", list_or_dash(&linked)),
        String::new(),
        "Artifact pointers".to_string(),
        format!("  linked task: {}", present_opt(o.task_id.as_deref())),
        format!(
            "  resolution: {}",
            present_opt(o.resolution_pointer.as_deref())
        ),
    ]);
    append_artifacts(&mut lines, &o.evidence_pointers);
    lines.extend([String::new(), "Recent events".to_string()]);
    append_events(&mut lines, &o.recent_events);
    lines
}

fn collapsed_observation_lines(c: &CollapsedObsRow, app: &App) -> Vec<String> {
    let mut lines = observation_lines(&c.representative, app);
    lines.push(String::new());
    lines.push("Collapsed observations".to_string());
    lines.push(format!("  summary: {}", present(&c.summary)));
    lines.push(format!("  count: {}", c.count));
    lines.push(format!("  primary: {}", c.primary_display_id));
    lines.push("  display_ids:".to_string());
    for id in &c.display_ids {
        lines.push(format!("    {id}"));
    }
    lines
}

fn intake_lines(i: &IntakeRow) -> Vec<String> {
    let mut lines = vec![
        format!("Intake detail · {}", i.display_id),
        String::new(),
        "Summary / story".to_string(),
        format!("  summary: {}", present(&i.summary)),
        format!("  story: {}", present_opt(i.body.as_deref())),
        String::new(),
        "Priority / risk".to_string(),
        format!("  priority: {}", present_opt(i.priority.as_deref())),
        format!("  risk: {}", list_or_dash(&i.risk_flags)),
        format!("  cluster: {}", present_opt(i.cluster_key.as_deref())),
        String::new(),
        "ADR 0002 state".to_string(),
        format!("  lifecycle: {}", present_opt(i.lifecycle.as_deref())),
        format!("  waiting_kind: {}", present_opt(i.waiting_kind.as_deref())),
        format!("  outcome: {}", present_opt(i.outcome.as_deref())),
        format!(
            "  produced_observation_id: {}",
            present_opt(i.produced_observation_id.as_deref())
        ),
        format!(
            "  produced_architecture_review_id: {}",
            present_opt(i.produced_architecture_review_id.as_deref())
        ),
        format!(
            "  produced_task_id: {}",
            present_opt(i.produced_task_id.as_deref())
        ),
        format!(
            "  produced_artifact: {} {}",
            present_opt(i.produced_artifact_kind.as_deref()),
            present_opt(i.produced_artifact_id.as_deref())
        ),
        format!(
            "  duplicate_of_id: {}",
            present_opt(i.duplicate_of_id.as_deref())
        ),
        format!("  Legacy status: {}", present(&i.status)),
        format!("  captured: {}", present_opt(i.captured_at.as_deref())),
        format!("  recon_round: {}", opt_i64(i.recon_round)),
        String::new(),
        "Decision metadata".to_string(),
        format!(
            "  rationale: {}",
            present_opt(i.decision_rationale.as_deref())
        ),
        format!(
            "  confidence: {}",
            present_opt(i.decision_confidence.as_deref())
        ),
        format!(
            "  tier_hint: {}",
            present_opt(i.decision_tier_hint.as_deref())
        ),
        String::new(),
        "Contract / routing state".to_string(),
        format!("  decision: {}", present_opt(i.decision.as_deref())),
        format!(
            "  routed observation: {}",
            present_opt(i.routed_to_observation.as_deref())
        ),
        format!(
            "  routed arch review: {}",
            present_opt(i.routed_to_arch_review.as_deref())
        ),
        String::new(),
        "Next action / held reason".to_string(),
        format!("  next: {}", present_opt(i.next_action.as_deref())),
        format!("  held: {}", present_opt(i.held_reason.as_deref())),
        String::new(),
        "Recon question / evidence".to_string(),
        format!(
            "  question: {}",
            present_opt(i.missing_info_question.as_deref())
        ),
        format!("  evidence: {}", present_opt(i.evidence_pointer.as_deref())),
        String::new(),
        "Recent events".to_string(),
    ];
    append_events(&mut lines, &i.recent_events);
    lines.extend([
        String::new(),
        "Artifact pointers".to_string(),
        format!("  source task: {}", present_opt(i.source_task.as_deref())),
        format!("  source agent: {}", present_opt(i.source_agent.as_deref())),
        format!("  duplicate_of: {}", present_opt(i.duplicate_of.as_deref())),
    ]);
    lines
}

fn review_lines(r: &ReviewRow) -> Vec<String> {
    let state = super::semantics::external_review_presentation(r);
    let mut lines = vec![
        format!("External review detail · {}", r.display_id),
        String::new(),
        "Operator state".to_string(),
        format!(
            "  {} {}{}",
            state.glyph,
            state.label,
            signal_suffix(state.signal.as_deref())
        ),
        format!("  task: {}", present(&r.task_id)),
        format!(
            "  runner: {}",
            super::semantics::external_review_runner_label(r)
        ),
        format!("  attempts: {}", r.attempts),
        format!("  verdict: {}", present_opt(r.verdict.as_deref())),
    ];
    append_nonempty_line(&mut lines, "held_reason", r.held_reason.as_deref());
    append_nonempty_line(&mut lines, "next_retry_at", r.next_retry_at.as_deref());
    lines.extend([
        String::new(),
        "Links".to_string(),
        format!(
            "  linked_observation_ids: {}",
            list_or_dash(&r.linked_observation_ids)
        ),
        format!(
            "  produced_task_id: {}",
            present_opt(r.produced_task_id.as_deref())
        ),
        String::new(),
        "SHA window".to_string(),
        format!("  base_sha: {}", truncate_sha(r.base_sha.as_deref())),
        format!("  head_sha: {}", truncate_sha(r.head_sha.as_deref())),
        String::new(),
        "Findings".to_string(),
        format!(
            "  findings: critical {} · major {} · minor {} · total {}",
            opt_i64(r.critical_count),
            opt_i64(r.major_count),
            opt_i64(r.minor_count),
            opt_i64(r.findings_count)
        ),
        String::new(),
        "Timing".to_string(),
        format!("  started_at: {}", present_opt(r.started_at.as_deref())),
        format!("  completed_at: {}", present_opt(r.completed_at.as_deref())),
        format!("  duration_ms: {}", opt_i64(r.duration_ms)),
        String::new(),
        "Artifacts".to_string(),
        format!("  log_path: {}", present_opt(r.log_path.as_deref())),
        format!(
            "  transcript_path: {}",
            present_opt(r.transcript_path.as_deref())
        ),
        String::new(),
        "Debug tuple".to_string(),
        format!("  status: {}", present(&r.status)),
        format!("  lifecycle: {}", present_opt(r.lifecycle.as_deref())),
        format!("  outcome: {}", present_opt(r.outcome.as_deref())),
        format!("  held_reason: {}", present_opt(r.held_reason.as_deref())),
        format!(
            "  next_retry_at: {}",
            present_opt(r.next_retry_at.as_deref())
        ),
    ]);
    lines
}

pub(super) fn engine_lines(app: &App) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Engine detail".to_string());
    lines.push(String::new());
    let daemon_line = match &app.status_bar.daemon_liveness {
        super::daemon::Liveness::Live { pid } => format!("daemon: LIVE pid={pid}"),
        super::daemon::Liveness::Dead => "daemon: DEAD".to_string(),
    };
    lines.push(daemon_line);
    lines.push(String::new());

    lines.push(format!(
        "unfinished_locks: {}",
        app.engine_detail.unfinished_lock_rows.len()
    ));
    if app.engine_detail.unfinished_lock_rows.is_empty() {
        lines.push("  —".to_string());
    } else {
        for lock in &app.engine_detail.unfinished_lock_rows {
            let agent = present_opt(lock.agent_name.as_deref());
            lines.push(format!(
                "  {} agent={} runner={} claimed={} last_progress={} {}",
                present(&lock.display_id),
                agent,
                agent,
                present_opt(lock.claimed_at.as_deref()),
                present_opt(lock.heartbeat_at.as_deref()),
                present(&lock.liveness_label),
            ));
        }
    }
    lines.push(String::new());

    lines.push("recent_daemon_starts:".to_string());
    if app.engine_detail.recent_daemon_starts.is_empty() {
        lines.push("  —".to_string());
    } else {
        for start in app.engine_detail.recent_daemon_starts.iter().take(5) {
            lines.push(format!(
                "  pid={} started_at={} version={} sha={}",
                start.pid,
                present_opt(start.started_at.as_deref()),
                present_opt(start.binary_version.as_deref()),
                truncate_sha(start.git_sha.as_deref()),
            ));
        }
    }
    lines.push(String::new());

    lines.push("recent_agent_runs:".to_string());
    if app.engine_detail.recent_agent_runs_by_role.is_empty() {
        lines.push("  —".to_string());
    } else {
        for run in app.engine_detail.recent_agent_runs_by_role.iter().take(5) {
            lines.push(format!(
                "  role={} count={} total_tokens={}",
                present(&run.role),
                run.count,
                run.total_tokens,
            ));
        }
    }
    lines
}

fn append_events(lines: &mut Vec<String>, events: &[RecentEvent]) {
    if events.is_empty() {
        lines.push("  —".to_string());
        return;
    }
    for e in events {
        lines.push(format!(
            "  {} {} -> {} via {} at {}",
            e.store.as_deref().unwrap_or("?"),
            e.from_status.as_deref().unwrap_or("?"),
            e.to_status.as_deref().unwrap_or("?"),
            e.verb.as_deref().unwrap_or("?"),
            e.occurred_at.as_deref().unwrap_or("?")
        ));
    }
}

fn append_artifacts(lines: &mut Vec<String>, artifacts: &[ArtifactPointer]) {
    for a in artifacts {
        lines.push(format!("  {}: {}", a.label, a.value));
    }
}

fn obs_waiting_label(o: &ObsRow) -> String {
    if let Some(kind) = o.waiting_kind.as_deref().filter(|s| !s.trim().is_empty()) {
        return kind.to_string();
    }
    if o.pending_architecture_review.unwrap_or(false) {
        return "architecture_review".to_string();
    }
    if o.waiting.unwrap_or(false) {
        return "true".to_string();
    }
    "—".to_string()
}

fn obs_next_action(o: &ObsRow) -> String {
    if let Some(reason) = o.lock_reason.as_deref().filter(|s| !s.is_empty()) {
        format!("held: {reason}")
    } else if o.contract_state.as_deref() == Some("ready") {
        "ratify contract".to_string()
    } else if o.status == "investigation_failed" { // ADR 0002 compatibility-only T148 task 6.1
        format!(
            "investigation failed: {}",
            present_opt(o.investigation_failure_reason.as_deref())
        )
    } else {
        "triage observation".to_string()
    }
}

fn present(s: &str) -> &str {
    if s.trim().is_empty() {
        "—"
    } else {
        s
    }
}

fn present_opt(s: Option<&str>) -> &str {
    s.map(str::trim).filter(|v| !v.is_empty()).unwrap_or("—")
}

fn opt_i64(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string())
}

fn opt_bool(v: Option<bool>) -> String {
    v.map(|b| b.to_string()).unwrap_or_else(|| "—".to_string())
}

fn list_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, TuiOpts};
    use crate::tui::data::{
        AgentRunsRoleAggregate, CycleReviewGate, DaemonStartRow, DispatchLockRow, EngineDetail,
        PlanReviewGate, TaskCycleEntry, TaskPlanReviewEntry,
    };

    #[test]
    fn task_detail_contains_required_headings() {
        let app = App::new(TuiOpts::default());
        let row = Row::Task(TaskRow {
            display_id: "T900".to_string(),
            status: "executing".to_string(),
            title: "story title".to_string(),
            tier_hint: Some("T3".to_string()),
            current_phase: Some(2),
            total_phases: Some(3),
            current_cycle: Some(1),
            blocked_reason: Some("waiting on dependency".to_string()),
            branch: Some("feat/T900".to_string()),
            workspace_path: Some("/tmp/T900".to_string()),
            recent_events: vec![RecentEvent {
                verb: Some("start".to_string()),
                to_status: Some("executing".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        for needle in [
            "story title",
            "Operator state",
            "▣ exec",
            "next valve: code review",
            "Progress",
            "Blockers / held reasons",
            "Recent events",
            "Artifact pointers",
            "Debug tuple",
            "status: executing",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
        let state_idx = text.find("Operator state").expect("state heading");
        let debug_idx = text.find("Debug tuple").expect("debug heading");
        assert!(
            state_idx < debug_idx,
            "semantic state must precede debug: {text}"
        );
    }

    #[test]
    fn task_detail_decodes_task_map_sources_gates_confidence_and_fallback() {
        let app = App::new(TuiOpts::default());
        let row = Row::Task(TaskRow {
            display_id: "T902".to_string(),
            status: "blocked".to_string(),
            title: "map decode".to_string(),
            lifecycle: Some("active".to_string()),
            active_step: Some("coding".to_string()),
            current_phase: Some(2),
            current_cycle: Some(2),
            total_phases: Some(3),
            blocked: Some(true),
            blocker_kind: Some("runner".to_string()),
            plan_review_entries: vec![
                TaskPlanReviewEntry {
                    gate: PlanReviewGate::NeedsWork,
                    ..Default::default()
                },
                TaskPlanReviewEntry {
                    gate: PlanReviewGate::Ready,
                    ..Default::default()
                },
            ],
            cycle_entries: vec![
                TaskCycleEntry {
                    phase: 1,
                    cycle: 1,
                    review_gate: Some(CycleReviewGate::Pass),
                    ..Default::default()
                },
                TaskCycleEntry {
                    phase: 2,
                    cycle: 1,
                    review_gate: Some(CycleReviewGate::Revise),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        for needle in [
            "Task map",
            "planning: plan-review ● cycle 2 attempts=2 latest_gate=READY source=plan_review_log confidence=exact",
            "plan_review: gate=READY source=plan_review_log confidence=exact",
            "phase 1: code-review ▣ cycle=— gate=PASS source=cycles confidence=exact",
            "phase 2: executing □ cycle 2 cycle=2 gate=REVISE source=current_phase/current_cycle confidence=exact active",
            "phase 3: unreached · cycle=— gate=— source=total_phases confidence=exact",
            "fallback: fault ▲ reason=runner source=blocker confidence=exact",
            "projection_confidence: exact",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }

    #[test]
    fn task_detail_marks_unknown_task_map_evidence() {
        let app = App::new(TuiOpts::default());
        let row = Row::Task(TaskRow {
            display_id: "T903".to_string(),
            status: "executing".to_string(),
            title: "unknown map".to_string(),
            lifecycle: Some("active".to_string()),
            active_step: Some("coding".to_string()),
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        assert!(
            text.contains(
                "phase 1: unknown ? cycle=— gate=— source=missing_evidence confidence=unknown"
            ),
            "missing unknown marker: {text}"
        );
        assert!(
            text.contains("projection_confidence: unknown"),
            "missing projection unknown: {text}"
        );
    }

    #[test]
    fn task_detail_renders_live_runner_activity_window() {
        let app = App::new(TuiOpts::default());
        let row = Row::Task(TaskRow {
            display_id: "T901".to_string(),
            status: "planning".to_string(),
            title: "live task".to_string(),
            current_cycle: Some(1),
            live_run: Some(crate::tui::data::LiveRunSummary {
                role: "planner".to_string(),
                runner: Some("claude-code:opus".to_string()),
                status: Some("running".to_string()),
                last_event_at: Some("2026-05-11T00:00:05Z".to_string()),
                last_event_type: Some("tool_start".to_string()),
                current_activity: Some("tool:bash".to_string()),
                marker_path: Some("/tmp/current-T901-planner.json".to_string()),
                status_path: Some("/tmp/live/status.json".to_string()),
                events_path: Some("/tmp/live/events.jsonl".to_string()),
                transcript_path: Some("/tmp/live/transcript.jsonl".to_string()),
                stderr_log_path: Some("/tmp/live/stderr.log".to_string()),
                events: vec![crate::tui::data::LiveRunEventSummary {
                    ts: Some("2026-05-11T00:00:04Z".to_string()),
                    event_type: "tool_start".to_string(),
                    label: "tool_start".to_string(),
                    text: "Bash cargo test live_runner_window".to_string(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        assert!(text.contains("Live runner"), "missing live section: {text}");
        assert!(
            text.contains("planner · claude-code:opus · running"),
            "missing summary: {text}"
        );
        assert!(text.contains("tool:bash"), "missing activity: {text}");
        assert!(
            text.contains("Live activity · last 1"),
            "missing activity heading: {text}"
        );
        for needle in [
            "marker_path: /tmp/current-T901-planner.json",
            "status_path: /tmp/live/status.json",
            "events_path: /tmp/live/events.jsonl",
            "transcript_path: /tmp/live/transcript.jsonl",
            "stderr_log_path: /tmp/live/stderr.log",
            "Bash cargo test live_runner_window",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }

    #[test]
    fn review_detail_contains_semantic_state_verdict_findings_and_sha() {
        let app = App::new(TuiOpts::default());
        let row = Row::Review(ReviewRow {
            display_id: "E001".to_string(),
            task_id: "T100".to_string(),
            status: "passed".to_string(),
            runner: "codex".to_string(),
            attempts: 2,
            verdict: Some("PASS".to_string()),
            base_sha: Some("abcdef0123456789feedface".to_string()),
            head_sha: Some("0123456789abcdef0badcafe".to_string()),
            log_path: Some("/tmp/E001.log".to_string()),
            transcript_path: Some("/tmp/E001.transcript".to_string()),
            started_at: Some("2026-05-10T12:00:00Z".to_string()),
            completed_at: Some("2026-05-10T12:05:00Z".to_string()),
            duration_ms: Some(300_000),
            critical_count: Some(0),
            major_count: Some(1),
            minor_count: Some(2),
            findings_count: Some(3),
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        for needle in [
            "Operator state",
            "✓ passed",
            "verdict:",
            "base_sha:",
            "findings:",
            "log_path:",
            "started_at:",
            "Debug tuple",
            "status: passed",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
        // SHA truncated to 12 characters.
        assert!(text.contains("abcdef012345"), "base_sha trunc: {text}");
        assert!(
            !text.contains("abcdef0123456789"),
            "base_sha not full: {text}"
        );
        let state_idx = text.find("Operator state").expect("state heading");
        let debug_idx = text.find("Debug tuple").expect("debug heading");
        assert!(
            state_idx < debug_idx,
            "semantic state must precede debug: {text}"
        );
    }

    #[test]
    fn review_detail_hides_none_clutter_in_primary_state() {
        let app = App::new(TuiOpts::default());
        let row = Row::Review(ReviewRow {
            display_id: "E002".to_string(),
            task_id: "T101".to_string(),
            status: "pending".to_string(),
            runner: String::new(),
            held_reason: Some("none".to_string()),
            next_retry_at: Some("none".to_string()),
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        let primary = text
            .split("\n\nLinks")
            .next()
            .expect("primary review section");
        assert!(
            primary.contains("◌ pending"),
            "missing semantic state: {text}"
        );
        assert!(
            !primary.contains("none"),
            "primary state has none clutter: {primary}"
        );
        assert!(text.contains("Debug tuple"), "missing debug tuple: {text}");
    }

    #[test]
    fn intake_detail_contains_captured_and_decision_metadata() {
        let app = App::new(TuiOpts::default());
        let row = Row::Intake(IntakeRow {
            display_id: "I042".to_string(),
            status: "needs_info".to_string(),
            summary: "intake row".to_string(),
            captured_at: Some("2026-05-10T11:00:00Z".to_string()),
            recon_round: Some(2),
            decision_rationale: Some("needs more reconnaissance evidence".to_string()),
            decision_confidence: Some("medium".to_string()),
            decision_tier_hint: Some("T2".to_string()),
            ..Default::default()
        });
        let text = render_text_for_row(&row, &app);
        assert!(text.contains("captured:"), "missing captured: {text}");
        assert!(text.contains("recon_round:"), "missing recon_round: {text}");
        assert!(
            text.contains("needs more reconnaissance evidence"),
            "missing rationale: {text}"
        );
    }

    #[test]
    fn observation_detail_contains_linked_tasks() {
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![Row::Task(TaskRow {
            display_id: "T314".to_string(),
            status: "executing".to_string(),
            title: "task linking obs".to_string(),
            linked_observations: vec!["L042".to_string()],
            ..Default::default()
        })];
        let obs = ObsRow {
            display_id: "L042".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: "linked obs".to_string(),
            ..Default::default()
        };
        let row = Row::Obs(obs);
        let text = render_text_for_row(&row, &app);
        assert!(text.contains("Linked tasks"), "missing label: {text}");
        assert!(text.contains("T314"), "missing linking task id: {text}");
    }

    #[test]
    fn engine_detail_contains_daemon_and_lock_lines() {
        let mut app = App::new(TuiOpts::default());
        app.status_bar.daemon_liveness = crate::tui::daemon::Liveness::Live { pid: 12345 };
        app.engine_detail = EngineDetail {
            recent_daemon_starts: vec![DaemonStartRow {
                pid: 9876,
                started_at: Some("2026-05-10T10:00:00Z".to_string()),
                binary_version: Some("0.7.0".to_string()),
                git_sha: Some("deadbeefcafebabe1234".to_string()),
            }],
            unfinished_lock_rows: vec![DispatchLockRow {
                display_id: "T500".to_string(),
                agent_name: Some("planner".to_string()),
                claimed_by: Some("daemon-9876".to_string()),
                claimed_at: Some("2026-05-10T11:00:00Z".to_string()),
                heartbeat_at: None,
                liveness_label: "state=unknown".to_string(),
                attempts: 2,
            }],
            recent_agent_runs_by_role: vec![AgentRunsRoleAggregate {
                role: "planner".to_string(),
                count: 4,
                total_tokens: 12_000,
            }],
        };
        let lines = engine_lines(&app);
        let text = lines.join("\n");
        assert!(text.contains("daemon:"), "missing daemon: {text}");
        assert!(text.contains("unfinished_locks:"), "missing locks: {text}");
        assert!(text.contains("planner"), "missing agent_name: {text}");
        assert!(text.contains("9876"), "missing daemon pid: {text}");
    }
}
