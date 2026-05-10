//! Read-only drilldown renderers for task, observation, and intake rows.

use ratatui::text::Line;

use super::app::App;
use super::data::{
    ArtifactPointer, CollapsedObsRow, IntakeRow, ObsRow, RecentEvent, ReviewRow, Row, TaskRow,
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
    let mut lines = vec![
        format!("Task detail · {}", t.display_id),
        String::new(),
        "Story summary".to_string(),
        format!("  {}", present(&t.title)),
        String::new(),
        "Current state".to_string(),
        format!("  status: {}", present(&t.status)),
        format!("  priority/tier: {}", present_opt(t.tier_hint.as_deref())),
        format!(
            "  claimed: {} by {}",
            present_opt(t.claimed_at.as_deref()),
            present_opt(t.claimed_by.as_deref())
        ),
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
    lines
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
        "Status".to_string(),
        format!("  {}", present(&o.status)),
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
        String::new(),
        "Recent events".to_string(),
    ];
    append_events(&mut lines, &o.recent_events);
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
        "Status".to_string(),
        format!("  {}", present(&i.status)),
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
    vec![
        format!("External review detail · {}", r.display_id),
        String::new(),
        "Review state".to_string(),
        format!("  status: {}", present(&r.status)),
        format!("  task: {}", present(&r.task_id)),
        format!(
            "  runner: {}",
            if r.runner.trim().is_empty() {
                "unknown"
            } else {
                r.runner.as_str()
            }
        ),
        format!("  attempts: {}", r.attempts),
        format!("  verdict: {}", present_opt(r.verdict.as_deref())),
        String::new(),
        "Hold state".to_string(),
        format!(
            "  held_reason: {}",
            r.held_reason.as_deref().unwrap_or("none")
        ),
        format!(
            "  next_retry_at: {}",
            r.next_retry_at.as_deref().unwrap_or("none")
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
        format!(
            "  completed_at: {}",
            present_opt(r.completed_at.as_deref())
        ),
        format!("  duration_ms: {}", opt_i64(r.duration_ms)),
        String::new(),
        "Artifacts".to_string(),
        format!("  log_path: {}", present_opt(r.log_path.as_deref())),
        format!(
            "  transcript_path: {}",
            present_opt(r.transcript_path.as_deref())
        ),
    ]
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
            lines.push(format!(
                "  {} agent={} claimed_by={} claimed_at={} attempts={}",
                present(&lock.display_id),
                present_opt(lock.agent_name.as_deref()),
                present_opt(lock.claimed_by.as_deref()),
                present_opt(lock.claimed_at.as_deref()),
                lock.attempts,
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
    use crate::tui::data::{
        AgentRunsRoleAggregate, DaemonStartRow, DispatchLockRow, EngineDetail,
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
            "Current state",
            "Progress",
            "Blockers / held reasons",
            "Recent events",
            "Artifact pointers",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
    }

    #[test]
    fn review_detail_contains_verdict_findings_and_sha() {
        let app = App::new(TuiOpts::default());
        let row = Row::Review(ReviewRow {
            display_id: "E001".to_string(),
            task_id: "T100".to_string(),
            status: "completed".to_string(),
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
            "verdict:",
            "base_sha:",
            "findings:",
            "log_path:",
            "started_at:",
        ] {
            assert!(text.contains(needle), "missing {needle}: {text}");
        }
        // SHA truncated to 12 characters.
        assert!(text.contains("abcdef012345"), "base_sha trunc: {text}");
        assert!(!text.contains("abcdef0123456789"), "base_sha not full: {text}");
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
        app.status_bar.daemon_liveness =
            crate::tui::daemon::Liveness::Live { pid: 12345 };
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
