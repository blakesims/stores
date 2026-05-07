use stores::tui::data::{
    blocked_reason_class, classify_with_options, surface_counts, ObsRow, Row, Section, TaskRow,
    WatchClassifyOptions,
};

fn task(id: &str, status: &str, reason: Option<&str>) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: reason.unwrap_or("in-flight").to_string(),
        updated_at: "1700000000".to_string(),
        blocked_reason: reason.map(str::to_string),
        blocked_reason_class: Some(blocked_reason_class(reason).to_string()),
        ..Default::default()
    })
}

fn obs(id: &str, summary: &str) -> Row {
    Row::Obs(ObsRow {
        display_id: id.to_string(),
        status: "open".to_string(),
        priority: "normal".to_string(),
        summary: summary.to_string(),
        updated_at: "1700000000".to_string(),
        ..Default::default()
    })
}

fn fixture_rows() -> Vec<Row> {
    vec![
        task("T001", "blocked", Some("silent_zombie")),
        task("T002", "blocked", Some("drive_failed:silent_zombie_pid_dead")),
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
        obs("L001", "deploy-blocked: task T003 merge conflict"),
        obs("L002", "deploy-blocked: task T999 merge conflict"),
        obs("L003", "operator note"),
    ]
}

fn render_snapshot(show_all_history: bool) -> String {
    let rows = fixture_rows();
    let ((ta, tt), (oa, ot)) = surface_counts(&rows, show_all_history);
    let sections = classify_with_options(&rows, WatchClassifyOptions { show_all_history, ..Default::default() });
    let mut task_idxs = Vec::new();
    let mut obs_idxs = Vec::new();
    for (sec, idxs) in sections {
        match sec {
            Section::TasksActionableCurrentWork
            | Section::TasksBlockedNeedsAction
            | Section::TasksDeployRecovery
            | Section::TasksRecentlyTerminal => task_idxs.extend(idxs),
            Section::ObsRatifiable | Section::ObsOpenNoContract | Section::ObsOther => obs_idxs.extend(idxs),
        }
    }
    task_idxs.sort_by_key(|i| match &rows[*i] { Row::Task(t) => t.display_id.clone(), _ => String::new() });
    obs_idxs.sort_by_key(|i| match &rows[*i] { Row::Obs(o) => o.display_id.clone(), _ => String::new() });

    let mut out = format!("TASKS {ta} actionable / {tt} total (use --all)\n");
    for i in task_idxs {
        if let Row::Task(t) = &rows[i] {
            out.push_str(&format!("{} {} {}\n", t.display_id, t.status, t.title));
        }
    }
    out.push_str(&format!("OBSERVATIONS {oa} actionable / {ot} total (use --all)\n"));
    for i in obs_idxs {
        if let Row::Obs(o) = &rows[i] {
            out.push_str(&format!("{} {}\n", o.display_id, o.summary));
        }
    }
    out
}

#[test]
fn tui_watch_visibility_snapshots_default_actionable_view() {
    let got = render_snapshot(false);
    assert_eq!(got, include_str!("fixtures/watch/default.snap"));
    assert!(!got.contains("T001 blocked silent_zombie"));
    assert!(!got.contains("L001 deploy-blocked"));
    assert!(got.contains("TASKS 9 actionable / 13 total (use --all)"));
    assert!(got.contains("OBSERVATIONS 2 actionable / 3 total (use --all)"));
}

#[test]
fn tui_watch_visibility_snapshots_all_view_reveals_every_fixture_row() {
    let got = render_snapshot(true);
    assert_eq!(got, include_str!("fixtures/watch/all.snap"));
    assert!(got.contains("TASKS 13 actionable / 13 total (use --all)"));
    assert!(got.contains("OBSERVATIONS 3 actionable / 3 total (use --all)"));
    assert!(got.contains("T001 blocked silent_zombie"));
    assert!(got.contains("L001 deploy-blocked: task T003 merge conflict"));
}
