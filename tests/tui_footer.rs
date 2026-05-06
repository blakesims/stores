//! Integration: selected-row footer renders the canonical fields.

use stores::tui::app::{App, Selection, TuiOpts};
use stores::tui::data::{classify, Row, TaskRow};
use stores::tui::footer::{iso8601_to_epoch_secs, render_text};

fn fresh_app(rows: Vec<Row>) -> App {
    let mut app = App::new(TuiOpts::default());
    app.rows = rows;
    app.sections = classify(&app.rows);
    app.apply_sort();
    let flat = app.flat_rows();
    if let Some(first) = flat.first() {
        app.selection = Selection {
            section: first.section,
            row: first.row,
        };
    }
    app
}

#[test]
fn footer_renders_canonical_task_row() {
    let app = fresh_app(vec![Row::Task(TaskRow {
        display_id: "T028".to_string(),
        status: "executing".to_string(),
        title: "watch becomes a TUI".to_string(),
        claimed_by: None,
        updated_at: "2026-05-05T11:55:00Z".to_string(),
        tier_hint: Some("T3".to_string()),
        linked_observations: vec!["L075".to_string()],
        ..Default::default()
    })]);
    let now = iso8601_to_epoch_secs("2026-05-05T12:00:00Z").unwrap();
    let text = render_text(&app, now);
    assert!(text.contains("T028"));
    assert!(text.contains("status:executing"));
    assert!(text.contains("tier:T3"));
    assert!(text.contains("linked:L075"));
    assert!(text.contains("5m ago"));
}

#[test]
fn footer_handles_empty_state() {
    let app = fresh_app(vec![]);
    let text = render_text(&app, 1_700_000_000);
    assert_eq!(text, "no rows");
}
