//! Read-only drilldown renderers for task, observation, and intake rows.

use ratatui::text::Line;

use super::app::App;
use super::data::{
    ArtifactPointer, CollapsedObsRow, IntakeRow, ObsRow, RecentEvent, ReviewRow, Row, TaskRow,
};

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
        Row::Obs(o) => observation_lines(o),
        Row::CollapsedObs(c) => collapsed_observation_lines(c),
        Row::Review(r) => review_lines(r),
        Row::Intake(i) => intake_lines(i),
    }
}

fn task_lines(t: &TaskRow, app: &App) -> Vec<String> {
    let progress = super::progress::task_progress(t, &app.external_review).text;
    let mut lines = vec![
        format!("Task detail · {}", t.display_id),
        String::new(),
        "Story summary".to_string(),
        format!("  {}", present(&t.title)),
        String::new(),
        "Current state".to_string(),
        format!("  status: {}", present(&t.status)),
        format!("  priority/tier: {}", present_opt(t.tier_hint.as_deref())),
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
        String::new(),
        "Blockers / held reasons".to_string(),
        format!("  {}", present_opt(t.blocked_reason.as_deref())),
        String::new(),
        "Recent events".to_string(),
    ];
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
    lines
}

fn observation_lines(o: &ObsRow) -> Vec<String> {
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
        "Status".to_string(),
        format!("  {}", present(&o.status)),
        String::new(),
        "Contract state".to_string(),
        format!("  state: {}", present_opt(o.contract_state.as_deref())),
        format!(
            "  objective: {}",
            present_opt(o.intent_objective.as_deref())
        ),
        String::new(),
        "Next action / held reason".to_string(),
        format!("  {}", obs_next_action(o)),
        String::new(),
        "Recent events".to_string(),
    ];
    append_events(&mut lines, &o.recent_events);
    lines.extend([
        String::new(),
        "Artifact pointers".to_string(),
        format!("  linked task: {}", present_opt(o.task_id.as_deref())),
        format!(
            "  resolution: {}",
            present_opt(o.resolution_pointer.as_deref())
        ),
    ]);
    append_artifacts(&mut lines, &o.evidence_pointers);
    lines
}

fn collapsed_observation_lines(c: &CollapsedObsRow) -> Vec<String> {
    let mut lines = observation_lines(&c.representative);
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
        "Status".to_string(),
        format!("  {}", present(&i.status)),
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
    vec![
        format!("External review detail · {}", r.display_id),
        String::new(),
        "Review state".to_string(),
        format!("  status: {}", r.status),
        format!("  task: {}", r.task_id),
        format!("  runner: {}", if r.runner.is_empty() { "unknown" } else { &r.runner }),
        format!("  attempts: {}", r.attempts),
        String::new(),
        "Hold state".to_string(),
        format!("  held_reason: {}", r.held_reason.as_deref().unwrap_or("none")),
        format!("  next_retry_at: {}", r.next_retry_at.as_deref().unwrap_or("none")),
    ]
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

fn obs_next_action(o: &ObsRow) -> String {
    if let Some(reason) = o.lock_reason.as_deref().filter(|s| !s.is_empty()) {
        format!("held: {reason}")
    } else if o.contract_state.as_deref() == Some("ready") {
        "ratify contract".to_string()
    } else if o.status == "investigation_failed" {
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
            "Current state",
            "Progress",
            "Blockers / held reasons",
            "Recent events",
            "Artifact pointers",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }
}
