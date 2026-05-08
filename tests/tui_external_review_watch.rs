use rusqlite::Connection;
use stores::tui::data::{cockpit_model, load_external_review_state, ExternalReviewState};

fn external_review_line(state: ExternalReviewState) -> String {
    match cockpit_model(&[], state).external_review {
        ExternalReviewState::Unavailable { reason } => reason,
        ExternalReviewState::Available { rows, lane, status } => format!(
            "external review: lane={} status={} rows={rows}",
            lane.as_deref().unwrap_or("unknown"),
            status.as_deref().unwrap_or("unknown")
        ),
    }
}

#[test]
fn tui_external_review_watch_absent_table_renders_unavailable_placeholder() {
    let conn = Connection::open_in_memory().unwrap();
    let state = load_external_review_state(&conn).unwrap();
    let line = external_review_line(state);
    assert_eq!(line, "external review: unavailable / not installed");
}

#[test]
fn tui_external_review_watch_present_table_renders_lane_status_not_placeholder() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE external_reviews (
            id INTEGER PRIMARY KEY,
            display_id TEXT,
            lane TEXT,
            status TEXT,
            reviewer TEXT,
            created_at TEXT
        );
        INSERT INTO external_reviews (display_id,lane,status,reviewer,created_at)
        VALUES ('T700','external-review','pending','codex','2026-05-01');
        "#,
    )
    .unwrap();

    let state = load_external_review_state(&conn).unwrap();
    let line = external_review_line(state);
    assert!(
        line.contains("external review: lane=external-review status=pending rows=1"),
        "{line}"
    );
    assert!(!line.contains("unavailable / not installed"), "{line}");
}

#[test]
fn tui_external_review_watch_present_table_tolerates_missing_lane_status_columns() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE external_review (id INTEGER PRIMARY KEY, display_id TEXT);
        INSERT INTO external_review (display_id) VALUES ('T701');
        "#,
    )
    .unwrap();

    let state = load_external_review_state(&conn).unwrap();
    let line = external_review_line(state);
    assert_eq!(line, "external review: lane=unknown status=unknown rows=1");
}
