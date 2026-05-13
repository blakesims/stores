//! AC4.4: end-to-end cockpit integration test. Builds a temp sqlite DB with
//! rows in every store, drives `App::refresh` against it, paints a 140x40
//! buffer, and asserts the contract done_when surface: lane labels with
//! counts, focus follows Right key, Up/Down navigation refreshes the side
//! detail pane, recent-exhaust strip shows terminal ids that are absent from
//! the focused-table region.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;
use rusqlite::Connection;

use stores::tui::app::{App, TuiOpts};
use stores::tui::daemon::Liveness;
use stores::tui::data::{Row, Section, StoreLane};
use stores::tui::render::{BOTTOM_CHROME_HEIGHT, TOP_STRIP_HEIGHT};
use stores::tui::{on_key, render};

const W: u16 = 140;
const H: u16 = 40;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Build a minimal in-memory DB whose schema satisfies the SQL prepared by
/// `tui::data::load_rows` / `load_external_review_state` /
/// `load_system_health`. Columns absent from production schemas are tolerated
/// via COALESCE-with-fallback in `data.rs`; we provide just enough.
fn fixture_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch(
        r#"
        CREATE TABLE tasks (
            display_id TEXT, status TEXT, title TEXT, claimed_by TEXT, updated_at TEXT,
            tier_hint TEXT, linked_observations TEXT, blocked_reason TEXT,
                lifecycle TEXT, active_step TEXT, integration_step TEXT, blocked INTEGER, blocker_kind TEXT,
            current_phase INTEGER, current_cycle INTEGER, plan TEXT, plan_source TEXT,
            contract TEXT, plan_review_log TEXT, cycles TEXT, wrap_log TEXT,
            branch TEXT, workspace_path TEXT
        );
        CREATE TABLE observations (
            display_id TEXT, status TEXT, priority TEXT, summary TEXT, updated_at TEXT,
            body TEXT, source TEXT, task_id TEXT, priority_rank INTEGER, intent_contract TEXT,
            locked_by TEXT, locked_at TEXT, lock_reason TEXT, evidence TEXT, resolution TEXT,
            investigation_failure_reason TEXT
        );
        CREATE TABLE external_reviews (
            id INTEGER PRIMARY KEY,
            display_id TEXT, task_id TEXT, status TEXT, runner TEXT,
            held_reason TEXT, next_retry_at TEXT, attempts INTEGER
        );
        CREATE TABLE intake (
            display_id TEXT, status TEXT, summary TEXT, body TEXT, updated_at TEXT,
            source_task TEXT, source_agent TEXT, risk_flags TEXT, cluster_key TEXT,
            decision TEXT, missing_info_question TEXT, routed_to_observation TEXT,
            routed_to_arch_review TEXT, duplicate_of TEXT, evidence TEXT
        );
        CREATE TABLE dispatch_locks (
            id INTEGER PRIMARY KEY,
            display_id TEXT, claimed_at TEXT, finished_at TEXT
        );

        -- Two active tasks so Up/Down navigation has an effect, plus one
        -- terminal task that must NOT appear in the focused-table region but
        -- MUST appear in the recent-exhaust strip.
        INSERT INTO tasks (display_id, status, title, updated_at, linked_observations) VALUES
            ('T100', 'executing', 'active task A', '2026-05-05', '[]'),
            ('T101', 'ready',     'active task B', '2026-05-04', '[]'),
            ('T200', 'accepted',  'done task',     '2026-05-09', '[]');

        INSERT INTO observations (display_id, status, priority, summary, updated_at) VALUES
            ('L001', 'open', 'normal', 'fixture obs', '2026-05-05');

        INSERT INTO external_reviews (display_id, task_id, status, runner, attempts) VALUES
            ('E001', 'T100', 'running', 'codex', 0);

        INSERT INTO intake (display_id, status, summary, updated_at) VALUES
            ('I001', 'draft', 'fixture intake', '2026-05-05');

        -- Stale unfinished dispatch lock — drives ENGINE card unfinished_locks=1.
        INSERT INTO dispatch_locks (display_id, claimed_at, finished_at) VALUES
            ('D001', '2026-05-01T00:00:00', NULL);
        "#,
    )
    .expect("seed fixture");
    conn
}

fn paint(app: &mut App) -> Buffer {
    let backend = TestBackend::new(W, H);
    let mut terminal = Terminal::new(backend).expect("test backend");
    terminal.draw(|f| render::draw(f, app)).expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer_to_string(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn line_at(buf: &Buffer, y: u16) -> String {
    let mut s = String::new();
    for x in 0..buf.area.width {
        s.push_str(buf[(x, y)].symbol());
    }
    s
}

/// Locate the column where each top-strip card's left border sits by scanning
/// row 0 for `┌`. Returns one column per card found.
fn card_left_borders(buf: &Buffer) -> Vec<u16> {
    let mut cols = Vec::new();
    for x in 0..buf.area.width {
        if buf[(x, 0)].symbol() == "┌" {
            cols.push(x);
        }
    }
    cols
}

/// Construct an App from the fixture conn, refresh, and pin daemon liveness
/// to a deterministic value (Dead — pidfile is unrelated to this test's cwd).
fn build_cockpit_app(conn: &Connection) -> App {
    let mut app = App::new(TuiOpts::default());
    app.refresh(conn).expect("refresh");
    // Make daemon liveness deterministic regardless of the test runner's cwd
    // / pidfile state. Engine card depends on this.
    app.status_bar.daemon_liveness = Liveness::Dead;
    app
}

/// AC4.5: `TuiOpts::default()` represents a flagless `stores watch` invocation;
/// `legacy` must be false so `main.rs` routes through `tui::run` (the cockpit)
/// rather than `cli::watch::run` (the legacy ANSI POC). The same `TuiOpts`
/// painted once must produce the cockpit render-path output (5-card top strip),
/// not the legacy text dump — verified via run-once snapshot here.
#[test]
fn default_tui_opts_routes_through_cockpit_render_path() {
    let opts = TuiOpts::default();
    assert!(
        !opts.legacy,
        "TuiOpts::default().legacy must be false so flagless `stores watch` lands on the cockpit"
    );

    let conn = fixture_conn();
    let mut app = App::new(opts);
    app.refresh(&conn).expect("refresh");
    app.status_bar.daemon_liveness = Liveness::Dead;

    let buf = paint(&mut app);
    let painted = buffer_to_string(&buf);

    // The cockpit render path paints a 5-card top strip with `┌` border
    // glyphs; the legacy ANSI POC has no such structural surface.
    let cards = card_left_borders(&buf);
    assert_eq!(
        cards.len(),
        5,
        "default TuiOpts must produce the 5-card cockpit top strip; got {} cards",
        cards.len()
    );
    for label in [
        "INTAKE",
        "OBSERVATIONS",
        "TASKS",
        "EXTERNAL REVIEWS",
        "ENGINE",
    ] {
        assert!(
            painted.contains(label),
            "default TuiOpts run-once paint must include cockpit lane label {label:?}"
        );
    }
}

#[test]
fn cockpit_adr_0001_review_steps_render_under_active_work() {
    let conn = fixture_conn();
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, updated_at, linked_observations, lifecycle, active_step, integration_step, blocked, blocker_kind) \
         VALUES ('T102', 'code_review', 'code review active', '2026-05-06', '[]', 'active', 'coding_review', 'none', 0, NULL)",
        [],
    )
    .expect("seed code_review overlay");
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, updated_at, linked_observations, lifecycle, active_step, integration_step, blocked, blocker_kind) \
         VALUES ('T103', 'plan_review', 'plan review active', '2026-05-06', '[]', 'active', 'planning_review', 'none', 0, NULL)",
        [],
    )
    .expect("seed plan_review overlay");

    let mut app = build_cockpit_app(&conn);
    let active_idxs = app
        .sections
        .iter()
        .find(|(s, _)| *s == Section::TasksActionableCurrentWork)
        .map(|(_, idxs)| idxs.clone())
        .unwrap_or_default();
    let active_ids: Vec<&str> = active_idxs
        .iter()
        .filter_map(|i| match &app.rows[*i] {
            Row::Task(t) => Some(t.display_id.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        active_ids.contains(&"T102"),
        "code_review/coding_review must classify ACTIVE WORK: {active_ids:?}"
    );
    assert!(
        active_ids.contains(&"T103"),
        "plan_review/planning_review must classify ACTIVE WORK: {active_ids:?}"
    );
    let held_ids: Vec<&str> = app
        .sections
        .iter()
        .find(|(s, _)| *s == Section::TasksHeldAiReview)
        .map(|(_, idxs)| {
            idxs.iter()
                .filter_map(|i| match &app.rows[*i] {
                    Row::Task(t) => Some(t.display_id.as_str()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !held_ids.contains(&"T102"),
        "code_review/coding_review must not classify HELD-AI-REVIEW: {held_ids:?}"
    );
    assert!(
        !held_ids.contains(&"T103"),
        "plan_review/planning_review must not classify HELD-AI-REVIEW: {held_ids:?}"
    );

    let buf = paint(&mut app);
    let painted = buffer_to_string(&buf);
    let lines: Vec<&str> = painted.lines().collect();
    let active_line = lines
        .iter()
        .position(|line| line.contains("ACTIVE"))
        .expect("render_frame must show ACTIVE header");
    let next_section_line = lines
        .iter()
        .enumerate()
        .skip(active_line + 1)
        .find_map(|(idx, line)| {
            Section::ALL
                .iter()
                .filter(|s| **s != Section::TasksActionableCurrentWork)
                .any(|s| line.contains(s.label()))
                .then_some(idx)
        })
        .unwrap_or(lines.len());
    assert!(
        lines[active_line + 1..next_section_line]
            .iter()
            .any(|line| line.contains("T102") && line.contains("code-gate")),
        "render_frame must place T102 under ACTIVE with semantic code-gate label before next section:\n{painted}"
    );
    if let Some(held_line) = lines
        .iter()
        .position(|line| line.contains("HELD-AI-REVIEW"))
    {
        let next_after_held = lines
            .iter()
            .enumerate()
            .skip(held_line + 1)
            .find_map(|(idx, line)| {
                Section::ALL
                    .iter()
                    .filter(|s| **s != Section::TasksHeldAiReview)
                    .any(|s| line.contains(s.label()))
                    .then_some(idx)
            })
            .unwrap_or(lines.len());
        assert!(
            !lines[held_line + 1..next_after_held]
                .iter()
                .any(|line| line.contains("T102") && line.contains("code-gate")),
            "render_frame must not place T102 under HELD-AI-REVIEW:\n{painted}"
        );
    }
}

#[test]
fn cockpit_top_strip_paints_all_five_lane_labels_with_counts() {
    let conn = fixture_conn();
    let mut app = build_cockpit_app(&conn);

    let buf = paint(&mut app);
    let painted = buffer_to_string(&buf);
    let top: String = painted
        .lines()
        .take(TOP_STRIP_HEIGHT as usize)
        .collect::<Vec<_>>()
        .join("\n");

    // (i) All five lane labels present.
    for label in [
        "INTAKE",
        "OBSERVATIONS",
        "TASKS",
        "EXTERNAL REVIEWS",
        "ENGINE",
    ] {
        assert!(
            top.contains(label),
            "top strip missing lane label {label:?}; got top region:\n{top}\n\nfull paint:\n{painted}"
        );
    }

    // (i) Shared-flow top cards expose canonical glyph slots with fixture counts.
    for expected in [
        "◌new1",   // intake draft
        "◆tri0",   // intake triage
        "◌cand1",  // observation open
        "◆inv0",   // observation investigation
        "◌q0",     // no primary-lifecycle queued task
        "◆wrk2",   // ready + executing legacy active work
        "◇g0",     // no task review/integration gate
        "✓dn1",    // one terminal task
        "△w0",     // no task wait
        "▲f0",     // no task failure
        "◆run1",   // external review running
        "▲tool0",  // no external review tool fault
        "◇lock1",  // one dangling dispatch lock
        "▲daemon", // deterministic dead daemon
    ] {
        assert!(
            top.contains(expected),
            "top region missing shared-flow slot {expected:?}:\n{top}\n\nfull paint:\n{painted}"
        );
    }
}

#[test]
fn cockpit_right_key_moves_focused_card_highlight_visibly() {
    let conn = fixture_conn();
    let mut app = build_cockpit_app(&conn);

    // Default focus is Tasks (column index 2 in StoreLane::ALL).
    let buf = paint(&mut app);
    let cols = card_left_borders(&buf);
    assert_eq!(cols.len(), 5, "expected 5 cards, got cols {cols:?}");
    assert_eq!(
        buf[(cols[2], 0)].fg,
        Color::Cyan,
        "default focus must paint cyan border at Tasks column (index 2)"
    );
    assert_ne!(
        buf[(cols[3], 0)].fg,
        Color::Cyan,
        "non-focused column 3 (ExternalReviews) must NOT be cyan before Right key"
    );

    // Press Right → focus advances to ExternalReviews (column index 3).
    on_key(&mut app, key(KeyCode::Right));
    assert_eq!(app.focused_store, StoreLane::ExternalReviews);

    let buf2 = paint(&mut app);
    let cols2 = card_left_borders(&buf2);
    assert_eq!(
        buf2[(cols2[3], 0)].fg,
        Color::Cyan,
        "after Right key the cyan focus border must follow to ExternalReviews (col index 3)"
    );
    assert_ne!(
        buf2[(cols2[2], 0)].fg,
        Color::Cyan,
        "after Right key the previous Tasks column must lose cyan border"
    );
}

#[test]
fn cockpit_up_down_changes_selection_and_side_detail_pane() {
    let conn = fixture_conn();
    let mut app = build_cockpit_app(&conn);

    // Tasks lane has T100 (executing) + T101 (ready). T200 (accepted, terminal)
    // is hidden from the main rows but lives in the recent-exhaust strip.
    // Anchor selection on the first navigable row in the focused lane.
    let flat = app.flat_rows();
    assert!(
        flat.len() >= 2,
        "fixture must expose ≥2 navigable Tasks-lane rows (T100, T101); got {}",
        flat.len()
    );
    app.selection = stores::tui::app::Selection {
        section: flat[0].section,
        row: flat[0].row,
    };

    let initial_id = app
        .current_row()
        .map(|r| r.display_id().to_string())
        .expect("initial row");
    let buf_before = paint(&mut app);
    let painted_before = buffer_to_string(&buf_before);
    assert!(
        painted_before.contains(&format!("Task detail · {initial_id}")),
        "side detail pane must show initial row's detail header (Task detail · {initial_id}) before Down key:\n{painted_before}"
    );

    on_key(&mut app, key(KeyCode::Down));
    let after_id = app
        .current_row()
        .map(|r| r.display_id().to_string())
        .expect("after Down row");
    assert_ne!(
        initial_id, after_id,
        "Down key must change current_row's display_id ({initial_id} → {after_id})"
    );

    let buf_after = paint(&mut app);
    let painted_after = buffer_to_string(&buf_after);
    assert!(
        painted_after.contains(&format!("Task detail · {after_id}")),
        "side detail pane must update to new row's detail header (Task detail · {after_id}) after Down key:\n{painted_after}"
    );

    // Up key restores the original selection — symmetry sanity.
    on_key(&mut app, key(KeyCode::Up));
    assert_eq!(
        app.current_row().map(|r| r.display_id().to_string()),
        Some(initial_id),
        "Up after Down must restore the initial selection"
    );
}

#[test]
fn cockpit_recent_exhaust_shows_terminal_id_absent_from_main_rows() {
    let conn = fixture_conn();
    let mut app = build_cockpit_app(&conn);

    let buf = paint(&mut app);
    let painted = buffer_to_string(&buf);
    let lines: Vec<&str> = painted.lines().collect();

    // Layout (top → bottom): top-strip (TOP_STRIP_HEIGHT) · middle ·
    // exhaust (1) · hint (1) · status (1). With no Search bar the exhaust
    // strip sits at the top of the bottom-chrome band, i.e. row
    // H - BOTTOM_CHROME_HEIGHT. Deriving from named constants makes the
    // test fail loudly if either height shifts in render.rs instead of
    // silently re-slicing the wrong region.
    let exhaust_y = H - BOTTOM_CHROME_HEIGHT;
    let exhaust_line = line_at(&buf, exhaust_y);
    assert!(
        exhaust_line.contains("recent exhaust"),
        "expected 'recent exhaust' label on exhaust strip line {exhaust_y}: {exhaust_line}"
    );
    assert!(
        exhaust_line.contains("T200"),
        "exhaust strip must include terminal task id T200; got: {exhaust_line}"
    );

    // (v) The middle region (focused-table + side detail) must NOT contain
    // the terminal task id T200 — terminal tasks are filtered out of the
    // focused Tasks-lane table by classify().
    let middle_start = TOP_STRIP_HEIGHT as usize;
    let middle_end = (H - BOTTOM_CHROME_HEIGHT) as usize;
    let middle: String = lines[middle_start..middle_end].join("\n");
    assert!(
        !middle.contains("T200"),
        "terminal task id T200 must NOT appear in the focused-table region:\n{middle}"
    );
}
