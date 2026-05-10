//! AC4.3: Read-only-key audit. Drives a fixture `App` through every canonical
//! cockpit affordance key and asserts none of them queue a substrate write
//! (no `pending_spawn`, no `obs_draft_filing_request`). The DB connection is
//! opened READ_ONLY in `tui::run`, so the absence of `Connection`-level
//! writes is enforced at the app-state surface here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use stores::tui::app::{App, Selection, TuiOpts};
use stores::tui::data::{
    classify, ExternalReviewState, IntakeRow, ObsRow, ReviewRow, Row, SystemHealth, TaskRow,
};
use stores::tui::{on_key, KeyOutcome};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn fixture_app() -> App {
    let mut app = App::new(TuiOpts::default());
    app.rows = vec![
        Row::Intake(IntakeRow {
            display_id: "I001".into(),
            status: "draft".into(),
            summary: "intake row".into(),
            updated_at: "2026-05-05".into(),
            ..Default::default()
        }),
        Row::Obs(ObsRow {
            display_id: "L001".into(),
            status: "open".into(),
            priority: "normal".into(),
            summary: "obs row".into(),
            updated_at: "2026-05-05".into(),
            ..Default::default()
        }),
        Row::Task(TaskRow {
            display_id: "T100".into(),
            status: "executing".into(),
            title: "active task".into(),
            updated_at: "2026-05-05".into(),
            ..Default::default()
        }),
        Row::Task(TaskRow {
            display_id: "T101".into(),
            status: "ready".into(),
            title: "ready task".into(),
            updated_at: "2026-05-04".into(),
            ..Default::default()
        }),
        Row::Review(ReviewRow {
            display_id: "E001".into(),
            task_id: "T100".into(),
            status: "running".into(),
            runner: "codex".into(),
            ..Default::default()
        }),
    ];
    app.sections = classify(&app.rows);
    app.external_review = ExternalReviewState::default();
    app.system_health = SystemHealth::default();
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

fn assert_no_write(app: &App, label: &str) {
    assert!(
        app.pending_spawn.is_none(),
        "key {label}: pending_spawn must remain None (cockpit affordance must not enqueue side-car spawn)"
    );
    assert!(
        app.obs_draft_filing_request.is_none(),
        "key {label}: obs_draft_filing_request must remain None (cockpit affordance must not enqueue substrate write)"
    );
}

/// AC4.3: every canonical cockpit affordance key is read-only at the
/// app-state surface. Drives a fresh fixture App through each key and
/// asserts no write request was queued. DB-level read-only-ness is
/// guaranteed structurally by `tui::run` opening Connection with
/// `OpenFlags::SQLITE_OPEN_READ_ONLY`; we audit at the app surface here.
#[test]
fn cockpit_affordance_keys_are_read_only() {
    let cases: Vec<(&str, KeyCode)> = vec![
        ("Left", KeyCode::Left),
        ("Right", KeyCode::Right),
        ("h", KeyCode::Char('h')),
        ("l", KeyCode::Char('l')),
        ("Up", KeyCode::Up),
        ("Down", KeyCode::Down),
        ("j", KeyCode::Char('j')),
        ("k", KeyCode::Char('k')),
        ("PgUp", KeyCode::PageUp),
        ("PgDn", KeyCode::PageDown),
        ("Tab", KeyCode::Tab),
        ("Enter", KeyCode::Enter),
        ("Esc", KeyCode::Esc),
        ("/", KeyCode::Char('/')),
        ("f", KeyCode::Char('f')),
    ];

    for (label, code) in cases {
        let mut app = fixture_app();
        let outcome = on_key(&mut app, key(code));
        assert_eq!(
            outcome,
            KeyOutcome::Continue,
            "key {label}: cockpit affordance must not quit the loop"
        );
        assert_no_write(&app, label);
    }
}

/// AC4.3 follow-up: after Enter opens detail and Esc closes it, the read-only
/// invariant continues to hold across the modal round-trip. Detail mode is
/// itself read-only — j/k scroll the page, Esc/q return to Normal — and must
/// not enqueue any write.
#[test]
fn cockpit_enter_esc_round_trip_is_read_only() {
    let mut app = fixture_app();
    on_key(&mut app, key(KeyCode::Enter));
    assert_no_write(&app, "Enter (detail open)");
    on_key(&mut app, key(KeyCode::Char('j')));
    assert_no_write(&app, "j (detail scroll)");
    on_key(&mut app, key(KeyCode::Char('k')));
    assert_no_write(&app, "k (detail scroll)");
    on_key(&mut app, key(KeyCode::Esc));
    assert_no_write(&app, "Esc (detail close)");
}

/// AC4.3 follow-up: opening filter (`f`) and search (`/`) palettes and
/// cancelling them must not queue a write either — only Enter from those
/// modes commits the predicate, and even commit is a view filter, not a
/// substrate write.
#[test]
fn cockpit_filter_and_search_modal_open_close_are_read_only() {
    let mut app = fixture_app();
    on_key(&mut app, key(KeyCode::Char('f')));
    assert_no_write(&app, "f (filter open)");
    on_key(&mut app, key(KeyCode::Esc));
    assert_no_write(&app, "Esc (filter cancel)");

    on_key(&mut app, key(KeyCode::Char('/')));
    assert_no_write(&app, "/ (search open)");
    on_key(&mut app, key(KeyCode::Esc));
    assert_no_write(&app, "Esc (search cancel)");
}
