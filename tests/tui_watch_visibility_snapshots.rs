use stores::tui::data::{
    blocked_reason_class, classify_with_options, cockpit_model, ExternalReviewState, IntakeRow,
    ObsRow, Row, TaskRow, WatchClassifyOptions,
};

fn task(id: &str, status: &str, reason: Option<&str>) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: reason.unwrap_or("in-flight").to_string(),
        updated_at: "1700000000".to_string(),
        blocked_reason: reason.map(str::to_string),
        blocked_reason_class: Some(blocked_reason_class(reason).to_string()),
        current_phase: Some(2),
        current_cycle: Some(1),
        total_phases: Some(3),
        ..Default::default()
    })
}

fn obs(id: &str, priority: &str, summary: &str) -> Row {
    Row::Obs(ObsRow {
        display_id: id.to_string(),
        status: "open".to_string(),
        priority: priority.to_string(),
        summary: summary.to_string(),
        updated_at: "1700000000".to_string(),
        ..Default::default()
    })
}

fn fixture_rows() -> Vec<Row> {
    vec![
        task("T001", "blocked", Some("silent_zombie")),
        task(
            "T002",
            "blocked",
            Some("drive_failed:silent_zombie_pid_dead"),
        ),
        task("T003", "cargo_installed", Some("accept_installed_inert")),
        task("T004", "schema_migrated", Some("accept_installed_inert")),
        task("T010", "executing", None),
        task("T011", "plan_review", None),
        task("T012", "ready", None),
        task("T020", "blocked", Some("rate_limit")),
        task("T021", "blocked", Some("transient_infra")),
        task("T022", "deploy_blocked", Some("retry-deploy-recoverable")),
        task("T023", "deploy_blocked", Some("opaque")),
        task("T024", "blocked", Some("unknown")),
        task("T030", "accepted", Some("terminal")),
        obs("L001", "normal", "deploy-blocked: task T003 merge conflict"),
        obs("L002", "high", "deploy-blocked: task T999 merge conflict"),
        obs("L003", "normal", "operator note"),
        Row::Intake(IntakeRow {
            display_id: "I001".to_string(),
            status: "needs_info".to_string(),
            summary: "intake needs answer".to_string(),
            priority: Some("high".to_string()),
            held_reason: Some("missing owner".to_string()),
            ..Default::default()
        }),
    ]
}

fn row_line(row: &Row) -> String {
    match row {
        Row::Task(t) => format!(
            "{} {} [P{}/{} C{}/3] {}{}",
            t.display_id,
            t.status,
            t.current_phase.unwrap_or_default(),
            t.total_phases.unwrap_or_default(),
            t.current_cycle.unwrap_or_default(),
            t.title,
            t.blocked_reason.as_deref().map(|r| format!(" reason:{r}")).unwrap_or_default()
        ),
        Row::Obs(o) => format!("{} {} priority:{} {}", o.display_id, o.status, o.priority, o.summary),
        Row::Intake(i) => format!(
            "{} {} priority:{} {} held:{}",
            i.display_id,
            i.status,
            i.priority.as_deref().unwrap_or("normal"),
            i.summary,
            i.held_reason.as_deref().unwrap_or("unknown")
        ),
    }
}

fn render_snapshot(show_all_history: bool) -> String {
    let rows = fixture_rows();
    let model = cockpit_model(&rows, ExternalReviewState::default());
    let sections = classify_with_options(&rows, WatchClassifyOptions { show_all_history, ..Default::default() });
    let external = match model.external_review {
        ExternalReviewState::Unavailable { reason } => reason,
        ExternalReviewState::Available { rows } => format!("external review: available rows={rows}"),
    };
    let mut out = format!(
        "daemon:DEAD\nlanes: execution={} review={} accept={} held={} active={} priority={}\n{external}\n",
        model.execution, model.review, model.accept, model.held, model.active, model.priority
    );
    for (sec, mut idxs) in sections {
        idxs.sort_by_key(|i| rows[*i].display_id().to_string());
        out.push_str(&format!("{} ({})\n", sec.label(), idxs.len()));
        for i in idxs {
            out.push_str(&row_line(&rows[i]));
            out.push('\n');
        }
    }
    out
}

#[test]
fn tui_watch_visibility_snapshots_default_actionable_view() {
    let got = render_snapshot(false);
    assert_eq!(got, include_str!("fixtures/watch/default.snap"));
    assert!(!got.contains("T001 blocked"));
    assert!(!got.contains("L001 deploy-blocked"));
    assert!(got.contains("PRIORITY (1)\nL002 open priority:high"));
    assert!(got.contains("HELD (2)\nT020 blocked"));
    assert!(got.contains("reason:rate_limit"));
}

#[test]
fn tui_watch_visibility_snapshots_all_view_reveals_every_fixture_row() {
    let got = render_snapshot(true);
    assert_eq!(got, include_str!("fixtures/watch/all.snap"));
    assert!(got.contains("T001 blocked"));
    assert!(got.contains("L001 open priority:normal deploy-blocked: task T003 merge conflict"));
}
