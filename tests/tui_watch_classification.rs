use stores::tui::data::{
    blocked_reason_class, classify_with_options, Row, Section, TaskRow, WatchClassifyOptions,
};

const NOW: i64 = 1_700_000_000;

fn task(id: &str, status: &str, updated_at: i64) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: id.to_string(),
        updated_at: updated_at.to_string(),
        blocked_reason_class: Some("unknown".to_string()),
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
fn default_hides_old_terminal_exhaust_and_all_history_shows_it() {
    let old = NOW - 49 * 3600;
    let rows = vec![
        task("schema", "schema_migrated", old),
        task("closed", "closed_out_of_band", old),
        task("rejected", "rejected", old),
        task("accepted", "accepted", old),
        task("complete", "complete", old),
        task("cargo", "cargo_installed", old),
    ];
    let default = classify_with_options(&rows, WatchClassifyOptions::default());
    assert!(bucket(&default, Section::TasksActionableCurrentWork).is_empty());
    assert!(bucket(&default, Section::TasksRecentlyTerminal).is_empty());

    let all = classify_with_options(
        &rows,
        WatchClassifyOptions {
            show_all_history: true,
            ..Default::default()
        },
    );
    assert_eq!(bucket(&all, Section::TasksRecentlyTerminal).len(), 6);
}

#[test]
fn recent_terminal_rows_are_capped_to_five_by_default() {
    let rows: Vec<Row> = (0..7)
        .map(|i| task(&format!("T{i}"), "accepted", i64::MAX - i))
        .collect();
    let default = classify_with_options(&rows, WatchClassifyOptions::default());
    assert_eq!(
        bucket(&default, Section::TasksRecentlyTerminal),
        vec![0, 1, 2, 3, 4]
    );

    let all = classify_with_options(
        &rows,
        WatchClassifyOptions {
            show_all_history: true,
            ..Default::default()
        },
    );
    assert_eq!(
        bucket(&all, Section::TasksRecentlyTerminal),
        vec![0, 1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn blocked_reason_classes_have_unknown_fallback() {
    assert_eq!(blocked_reason_class(Some("rate limit 429")), "rate_limit");
    assert_eq!(
        blocked_reason_class(Some("retry transient failure")),
        "retry"
    );
    assert_eq!(
        blocked_reason_class(Some("waiting on dependency")),
        "dependency"
    );
    assert_eq!(
        blocked_reason_class(Some("human approval required")),
        "user"
    );
    assert_eq!(blocked_reason_class(Some("deploy window closed")), "deploy");
    assert_eq!(blocked_reason_class(Some("stale timeout")), "stale");
    assert_eq!(blocked_reason_class(Some("opaque")), "unknown");
    assert_eq!(blocked_reason_class(None), "unknown");
}
