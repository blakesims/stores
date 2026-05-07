use stores::tui::data::{
    blocked_reason_class, classify_with_options, cockpit_model, ExternalReviewState, IntakeRow,
    ObsRow, Row, TaskRow, WatchClassifyOptions,
};

fn task(id: &str, tier: &str, status: &str, phase: i64, cycle: i64, total: i64, reason: Option<&str>) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: reason.unwrap_or("in-flight").to_string(),
        updated_at: "1700000000".to_string(),
        tier_hint: Some(tier.to_string()),
        blocked_reason: reason.map(str::to_string),
        blocked_reason_class: Some(blocked_reason_class(reason).to_string()),
        current_phase: Some(phase),
        current_cycle: Some(cycle),
        total_phases: Some(total),
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
        task("T001", "T2", "blocked", 2, 1, 3, Some("silent_zombie")),
        task("T002", "T2", "blocked", 2, 1, 3, Some("drive_failed:silent_zombie_pid_dead")),
        task("T003", "T2", "cargo_installed", 2, 1, 3, Some("accept_installed_inert")),
        task("T004", "T2", "schema_migrated", 2, 1, 3, Some("accept_installed_inert")),
        task("T010", "T2", "executing", 1, 1, 2, None),
        task("T011", "T3", "code_review", 2, 3, 3, None),
        task("T012", "T1", "ready", 1, 1, 1, None),
        task("T020", "T2", "blocked", 2, 1, 3, Some("rate_limit")),
        task("T021", "T2", "blocked", 2, 1, 3, Some("transient_infra")),
        task("T022", "T2", "deploy_blocked", 2, 1, 3, Some("retry-deploy-recoverable")),
        task("T023", "T2", "deploy_blocked", 2, 1, 3, Some("opaque")),
        task("T024", "T2", "blocked", 2, 1, 3, Some("unknown")),
        task("T030", "T2", "accepted", 2, 1, 2, Some("terminal")),
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
        Row::Task(t) => {
            let progress = stores::tui::progress::task_progress(t, &ExternalReviewState::default());
            let progress = if progress.text == t.status { String::new() } else { format!(" {}", progress.text) };
            format!(
                "{} {}{} {}{}",
                t.display_id,
                t.status,
                progress,
                t.title,
                t.blocked_reason.as_deref().map(|r| format!(" reason:{r}")).unwrap_or_default()
            )
        },
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
    assert!(got.contains("PRIORITY (2)\nL002 open priority:high"));
    assert!(got.contains("HELD (2)\nT020 blocked"));
    assert!(got.contains("T010 executing ▮▱ ···"));
    assert!(got.contains("T011 code_review ▰◐▱ ●●·"));
    assert!(got.contains("T012 ready in-flight"));
    assert!(!got.contains("T012 ready ▮"));
    assert!(got.contains("reason:rate_limit"));
}

#[test]
fn tui_watch_visibility_snapshots_all_view_reveals_every_fixture_row() {
    let got = render_snapshot(true);
    assert_eq!(got, include_str!("fixtures/watch/all.snap"));
    assert!(got.contains("T001 blocked"));
    assert!(got.contains("L001 open priority:normal deploy-blocked: task T003 merge conflict"));
}
