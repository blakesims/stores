use stores::tui::app::{App, DetailKind, DetailSelection, Mode, TuiOpts};
use stores::tui::data::{ArtifactPointer, IntakeRow, ObsRow, RecentEvent, Row, TaskRow};
use stores::tui::detail::render_text_for_row;

fn app() -> App {
    App::new(TuiOpts::default())
}

fn event(id: &str) -> RecentEvent {
    RecentEvent {
        store: Some("tasks".to_string()),
        display_id: id.to_string(),
        from_status: Some("ready".to_string()),
        to_status: Some("executing".to_string()),
        verb: Some("start".to_string()),
        occurred_at: Some("2026-05-01".to_string()),
    }
}

fn task(id: &str, tier: &str, status: &str, phase: i64, total: i64, cycle: i64) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: format!("{tier} operator cockpit story"),
        tier_hint: Some(tier.to_string()),
        current_phase: Some(phase),
        total_phases: Some(total),
        current_cycle: Some(cycle),
        contract_done_when: Some(format!("{id} done when operators understand state")),
        contract_executive_intent: Some("reduce ambiguity".to_string()),
        blocked_reason: Some("waiting on dependency".to_string()),
        linked_observations: vec!["L700".to_string()],
        branch: Some(format!("feat/{id}")),
        workspace_path: Some(format!("/tmp/{id}")),
        artifact_pointers: vec![ArtifactPointer {
            label: "run log".to_string(),
            value: format!("runs/{id}.log"),
        }],
        recent_events: vec![event(id)],
        ..Default::default()
    })
}

fn observation() -> Row {
    Row::Obs(ObsRow {
        display_id: "L700".to_string(),
        status: "open".to_string(),
        priority: "high".to_string(),
        summary: "observation story".to_string(),
        body: Some("full observation body".to_string()),
        task_id: Some("T700".to_string()),
        contract_state: Some("ready".to_string()),
        intent_objective: Some("ratify this".to_string()),
        resolution_pointer: Some("resolution.md".to_string()),
        evidence_pointers: vec![ArtifactPointer {
            label: "log".to_string(),
            value: "runs/x.log".to_string(),
        }],
        recent_events: vec![event("L700")],
        ..Default::default()
    })
}

fn intake() -> Row {
    Row::Intake(IntakeRow {
        display_id: "I700".to_string(),
        status: "needs_info".to_string(),
        summary: "intake story".to_string(),
        body: Some("operator intake body".to_string()),
        priority: None,
        risk_flags: vec!["touches_lifecycle".to_string()],
        cluster_key: Some("watch".to_string()),
        decision: Some("needs_recon".to_string()),
        missing_info_question: Some("what evidence exists?".to_string()),
        held_reason: Some("missing evidence".to_string()),
        next_action: Some("recon needed".to_string()),
        routed_to_observation: Some("L701".to_string()),
        evidence_pointer: Some("evidence.md".to_string()),
        source_task: Some("T701".to_string()),
        recent_events: vec![event("I700")],
        ..Default::default()
    })
}

#[test]
fn tui_detail_rendering_task_observation_intake_snapshots() {
    let app = app();
    let task_text = [
        render_text_for_row(&task("T701", "T1", "executing", 1, 1, 1), &app),
        render_text_for_row(&task("T702", "T2", "executing", 1, 2, 1), &app),
        render_text_for_row(&task("T703", "T3", "code_review", 2, 3, 3), &app),
    ]
    .join("\n---\n");
    assert_eq!(
        task_text,
        include_str!("fixtures/watch/task_detail.snap").trim_end()
    );
    assert_eq!(
        render_text_for_row(&observation(), &app),
        include_str!("fixtures/watch/observation_detail.snap").trim_end()
    );
    assert_eq!(
        render_text_for_row(&intake(), &app),
        include_str!("fixtures/watch/intake_detail.snap").trim_end()
    );
}

#[test]
fn task_detail_rendering_contains_story_state_progress_blockers_events_artifacts() {
    let mut app = app();
    let row = task("T700", "T3", "executing", 2, 4, 2);
    app.rows = vec![row.clone()];
    app.mode = Mode::Detail;
    app.detail = Some(DetailSelection {
        display_id: "T700".to_string(),
        kind: DetailKind::Task,
        scroll_offset: 0,
    });
    let text = render_text_for_row(&row, &app);

    for needle in [
        "operator cockpit story",
        "Current state",
        "Progress",
        "Blockers / held reasons",
        "waiting on dependency",
        "Recent events",
        "start",
        "Artifact pointers",
        "rendered task",
        "branch",
        "workspace",
    ] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
}

#[test]
fn observation_detail_rendering_contains_summary_priority_status_contract_next_artifacts() {
    let text = render_text_for_row(&observation(), &app());
    for needle in [
        "Summary / story",
        "observation story",
        "Priority",
        "high",
        "Status",
        "open",
        "Contract state",
        "Next action / held reason",
        "ratify contract",
        "Artifact pointers",
        "linked task",
        "resolution",
    ] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
}

#[test]
fn intake_detail_rendering_contains_summary_risk_status_routing_next_artifacts() {
    let text = render_text_for_row(&intake(), &app());
    for needle in [
        "Summary / story",
        "intake story",
        "Priority / risk",
        "priority: —",
        "risk: touches_lifecycle",
        "Status",
        "needs_info",
        "Contract / routing state",
        "Next action / held reason",
        "missing evidence",
        "Artifact pointers",
    ] {
        assert!(text.contains(needle), "missing {needle}: {text}");
    }
}
