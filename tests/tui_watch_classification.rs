use stores::tui::data::{
    blocked_reason_class, classify_with_options, row_visibility_class, surface_counts, ObsRow, Row,
    Section, TaskRow, VisibilityClass, WatchClassifyOptions,
};
use std::collections::HashMap;

fn task(id: &str, status: &str, reason: Option<&str>) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: id.to_string(),
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

fn class(row: &Row, ctx: &[(String, String)]) -> VisibilityClass {
    row_visibility_class(row, &ctx.iter().cloned().collect::<HashMap<_, _>>())
}

fn bucket(buckets: &[(Section, Vec<usize>)], section: Section) -> Vec<usize> {
    buckets.iter().find(|(s, _)| *s == section).unwrap().1.clone()
}

#[test]
fn tui_watch_classification_classifies_required_historical_noise_patterns() {
    let cases = vec![
        task("T001", "blocked", Some("silent_zombie: pid dead")),
        task("T002", "blocked", Some("drive_failed:silent_zombie_pid_dead")),
        task("T003", "cargo_installed", Some("accept_installed_inert")),
        task("T004", "schema_migrated", Some("accept_installed_inert")),
        obs("L001", "deploy-blocked: task T005 merge conflict"),
    ];
    for row in &cases[..4] {
        assert_eq!(class(row, &[]), VisibilityClass::HistoricalNoise);
    }
    assert_eq!(
        class(&cases[4], &[("T005".into(), "closed_out_of_band".into())]),
        VisibilityClass::HistoricalNoise
    );
}

#[test]
fn tui_watch_classification_classifies_actionable_recovery_and_needs_triage_patterns() {
    for row in [
        task("T010", "deploy_blocked", Some("retry-deploy-recoverable: conflict fixed")),
        task("T011", "blocked", Some("rate_limit 429")),
        task("T012", "blocked", Some("transient_infra retry later")),
    ] {
        assert_eq!(class(&row, &[]), VisibilityClass::ActionableRecovery);
    }
    assert_eq!(class(&task("T013", "deploy_blocked", Some("opaque deploy stop")), &[]), VisibilityClass::NeedsTriage);
    assert_eq!(class(&task("T014", "blocked", Some("opaque")), &[]), VisibilityClass::NeedsTriage);
}

#[test]
fn tui_watch_classification_default_hides_historical_noise_and_all_history_shows_it() {
    let rows = vec![
        task("T001", "blocked", Some("silent_zombie")),    // HistoricalNoise → hidden in default
        task("T002", "blocked", Some("rate_limit 429")),   // ActionableRecovery → TasksBlockedNeedsAction
        task("T003", "deploy_blocked", Some("opaque")),    // NeedsTriage → TasksNeedsTriage
    ];
    let default = classify_with_options(&rows, WatchClassifyOptions::default());
    // T001 hidden; T002 in blocked-needs-action; T003 in needs-triage (not deploy-recovery)
    assert_eq!(bucket(&default, Section::TasksBlockedNeedsAction), vec![1usize]);
    assert_eq!(bucket(&default, Section::TasksDeployRecovery), Vec::<usize>::new());
    assert_eq!(bucket(&default, Section::TasksNeedsTriage), vec![2usize]);

    let all = classify_with_options(&rows, WatchClassifyOptions { show_all_history: true, ..Default::default() });
    // T001 HistoricalNoise shown in --all; section_for sees HistoricalNoise (not NeedsTriage) → TasksBlockedNeedsAction
    assert_eq!(bucket(&all, Section::TasksBlockedNeedsAction), vec![0, 1]);
    assert_eq!(bucket(&all, Section::TasksNeedsTriage), vec![2usize]);
}

#[test]
fn tui_watch_classification_in_flight_rows_are_never_hidden() {
    let rows = vec![
        task("T020", "executing", Some("silent_zombie")),
        task("T021", "plan_review", Some("silent_zombie")),
        task("T022", "code_review", Some("silent_zombie")),
        task("T023", "in_review", Some("silent_zombie")),
        task("T024", "planning", Some("silent_zombie")),
        task("T025", "ready", Some("silent_zombie")),
    ];
    let default = classify_with_options(&rows, WatchClassifyOptions::default());
    assert_eq!(bucket(&default, Section::TasksActionableCurrentWork).len(), 6);
}

#[test]
fn tui_watch_classification_surface_counts_report_actionable_and_total() {
    let rows = vec![
        task("T001", "closed_out_of_band", Some("silent_zombie")),
        task("T002", "blocked", Some("rate_limit 429")),
        obs("L001", "deploy-blocked: task T001 merge conflict"),
    ];
    // T001: HistoricalNoise (closed_out_of_band + accept_installed_inert... actually
    // closed_out_of_band status is NOT in the accept_installed_inert terminal list,
    // but silent_zombie makes it HistoricalNoise regardless.
    // T002: ActionableRecovery → counted as actionable
    // L001: task T001 is in rows but no task_status_by_id; we must pass task_status_by_id
    //   which surface_counts builds internally. T001 status is "closed_out_of_band" → terminal
    //   → L001 is HistoricalNoise.
    // actionable tasks: T002 = 1 / total = 2
    // actionable obs: 0 (L001 is HistoricalNoise) / total = 1
    assert_eq!(surface_counts(&rows, false), ((1, 2), (0, 1)));
    // --all changes visibility but NOT the actionable count: still only T002 is ActionableRecovery
    assert_eq!(surface_counts(&rows, true), ((1, 2), (0, 1)));
}

#[test]
fn tui_watch_classification_blocked_reason_classes_have_unknown_fallback() {
    assert_eq!(blocked_reason_class(Some("rate limit 429")), "rate_limit");
    assert_eq!(blocked_reason_class(Some("retry transient failure")), "retry");
    assert_eq!(blocked_reason_class(Some("waiting on dependency")), "dependency");
    assert_eq!(blocked_reason_class(Some("human approval required")), "user");
    assert_eq!(blocked_reason_class(Some("deploy window closed")), "deploy");
    assert_eq!(blocked_reason_class(Some("stale timeout")), "stale");
    assert_eq!(blocked_reason_class(Some("opaque")), "unknown");
    assert_eq!(blocked_reason_class(None), "unknown");
    assert_eq!(blocked_reason_class(Some("{\"exit_code\":1,\"kind\":\"rate_limit\"}")), "rate_limit");
}
