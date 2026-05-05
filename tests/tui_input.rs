//! Headless tests for the TUI input dispatcher driving a `TestBackend`.
//!
//! These exercise the AC2.1–AC2.5 surface end-to-end: synthetic `KeyEvent`s
//! flow through `tui::on_key` and we assert `app.selection`, `app.sort`,
//! `app.filter`, and viewport math reach the expected states. The
//! `TestBackend` ensures the renderer can also paint without panicking.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use stores::tui::app::{App, Mode, Selection, TuiOpts};
use stores::tui::data::{classify, ObsRow, Row, Section, TaskRow};
use stores::tui::filter::FilterPredicate;
use stores::tui::sort::Sort;
use stores::tui::{on_key, render, KeyOutcome};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn task(id: &str, status: &str, updated: &str) -> Row {
    Row::Task(TaskRow {
        display_id: id.to_string(),
        status: status.to_string(),
        title: format!("{id} title"),
        claimed_by: None,
        updated_at: updated.to_string(),
        ..Default::default()
    })
}

fn obs(id: &str, contract: Option<&str>) -> Row {
    Row::Obs(ObsRow {
        display_id: id.to_string(),
        status: "open".to_string(),
        priority: "high".to_string(),
        summary: format!("{id} summary"),
        updated_at: "2026-05-01".to_string(),
        contract_state: contract.map(str::to_string),
        ..Default::default()
    })
}

fn build_app(rows: Vec<Row>) -> App {
    let mut app = App::new(TuiOpts::default());
    app.rows = rows;
    app.sections = classify(&app.rows);
    app.apply_sort();
    app.viewport_height = 10;
    let flat = app.flat_rows();
    if let Some(first) = flat.first() {
        app.selection = Selection {
            section: first.section,
            row: first.row,
        };
    }
    app
}

fn fixtures() -> Vec<Row> {
    vec![
        task("T001", "plan_review", "2026-05-05"),
        task("T002", "executing", "2026-05-04"),
        task("T010", "executing", "2026-05-03"),
        task("T011", "deploy_blocked", "2026-05-02"),
        obs("L001", Some("ready")),
        obs("L002", None),
    ]
}

#[test]
fn nav_moves_across_sections() {
    let mut app = build_app(fixtures());
    let total = app.flat_rows().len();
    assert!(total >= 4);

    on_key(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.current_flat(), Some(1));

    on_key(&mut app, key(KeyCode::Char('j')));
    on_key(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.current_flat(), Some(3));

    on_key(&mut app, key(KeyCode::Char('k')));
    assert_eq!(app.current_flat(), Some(2));

    // Tab on the in-flight section collapses it.
    let before = app.flat_rows().len();
    on_key(&mut app, key(KeyCode::Tab));
    let after = app.flat_rows().len();
    assert!(after < before);
}

#[test]
fn comma_cycles_sort_through_four_positions() {
    let mut app = build_app(fixtures());
    let initial = app.sort;
    let mut seen = vec![initial];
    for _ in 0..3 {
        on_key(&mut app, key(KeyCode::Char(',')));
        seen.push(app.sort);
    }
    let mut deduped = seen.clone();
    deduped.sort_by_key(|s| *s as u8);
    deduped.dedup();
    assert_eq!(deduped.len(), 4, "comma must visit all 4 distinct keys");

    // Pressing once more wraps back to the start.
    on_key(&mut app, key(KeyCode::Char(',')));
    assert_eq!(app.sort, initial);
    assert_ne!(app.sort, Sort::Priority);
}

#[test]
fn filter_palette_round_trip_and_esc() {
    let mut app = build_app(fixtures());
    assert!(app.filter.is_empty());

    on_key(&mut app, key(KeyCode::Char('f')));
    assert_eq!(app.mode, Mode::Filter);
    assert!(app.filter_palette.is_some());

    for c in "state=in_flight".chars() {
        on_key(&mut app, key(KeyCode::Char(c)));
    }
    on_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(
        app.filter,
        FilterPredicate {
            state: Some("in_flight".to_string()),
            ..Default::default()
        }
    );
    // Filter narrows the visible flat list to TasksInFlight rows only.
    let flat = app.flat_rows();
    assert!(!flat.is_empty());
    for fr in &flat {
        let (sec, _) = app.sections[fr.section];
        assert_eq!(sec, Section::TasksInFlight);
    }

    // Re-open and Esc-cancel: predicate is unchanged.
    let snapshot = app.filter.clone();
    on_key(&mut app, key(KeyCode::Char('f')));
    on_key(&mut app, key(KeyCode::Char('x')));
    on_key(&mut app, key(KeyCode::Esc));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.filter, snapshot);

    // Capital F clears the filter.
    on_key(&mut app, key(KeyCode::Char('F')));
    assert!(app.filter.is_empty());
}

#[test]
fn search_jumps_to_first_match_and_n_advances() {
    let mut app = build_app(fixtures());
    on_key(&mut app, key(KeyCode::Char('/')));
    assert_eq!(app.mode, Mode::Search);

    for c in "T01".chars() {
        on_key(&mut app, key(KeyCode::Char(c)));
    }
    // The first matching row has display_id containing "T01" — namely T010
    // or T011 (T001 also matches if present, but we built it to contain
    // "T01" — wait, "T001" lowercased is "t001" which does NOT contain
    // "t01"; only T010 and T011 match in our fixture).
    let cur = app.current_flat().unwrap();
    let fr = app.flat_rows()[cur];
    let id = app.rows[fr.abs].display_id().to_lowercase();
    assert!(id.contains("t01"), "first hit display_id={id}");

    on_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.mode, Mode::Normal);

    // n advances to the next hit.
    let first_hit = app.current_flat().unwrap();
    on_key(&mut app, key(KeyCode::Char('n')));
    let second_hit = app.current_flat().unwrap();
    assert_ne!(first_hit, second_hit);
}

#[test]
fn saved_view_preset_2_filters_in_flight() {
    let mut app = build_app(fixtures());
    on_key(&mut app, key(KeyCode::Char('2')));
    assert_eq!(app.filter.state.as_deref(), Some("in_flight"));
    let flat = app.flat_rows();
    for fr in &flat {
        let (sec, _) = app.sections[fr.section];
        assert_eq!(sec, Section::TasksInFlight);
    }
}

#[test]
fn quits_on_q_and_renders_via_test_backend() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut app = build_app(fixtures());

    // Render once — must not panic.
    terminal.draw(|f| render::draw(f, &mut app)).expect("draw");

    assert_eq!(on_key(&mut app, key(KeyCode::Char('q'))), KeyOutcome::Quit);
}

/// AC2.5: with 200 synthetic rows and viewport=10, only viewport-height
/// flat rows are emitted into the visible window, and PgDn keeps the
/// cursor inside that window.
#[test]
fn virtual_scroll_keeps_selection_in_viewport() {
    let rows: Vec<Row> = (0..200)
        .map(|i| {
            task(
                &format!("T{:03}", i),
                "executing",
                &format!("2026-05-{:02}", (i % 28) + 1),
            )
        })
        .collect();
    let mut app = build_app(rows);
    app.viewport_height = 10;

    let flat = app.flat_rows();
    assert_eq!(flat.len(), 200);
    assert_eq!(render::visible_window(&app, &flat).len(), 10);

    // Five page-downs.
    for _ in 0..5 {
        on_key(&mut app, key(KeyCode::PageDown));
    }
    let cursor = app.current_flat().expect("cursor");
    assert!(
        cursor >= app.scroll_offset
            && cursor < app.scroll_offset + app.viewport_height,
        "selection {} out of viewport [{}, {})",
        cursor,
        app.scroll_offset,
        app.scroll_offset + app.viewport_height
    );

    // Render through the test backend at 200 rows — must not panic.
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test backend");
    terminal.draw(|f| render::draw(f, &mut app)).expect("draw");
}
