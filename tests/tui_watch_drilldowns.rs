//! T141 Phase 4 integration: per-store drilldown surfaces, end-to-end.
//!
//! Seeds a single in-memory DB with one rich row in every store
//! (intake / observations / tasks / external reviews) plus engine-health
//! tables (daemon_starts + dispatch_locks), then for each `StoreLane`
//! cycles focus, paints a 160x40 buffer, and asserts the per-store
//! substrings established by P2/P3 land in the side-detail / engine-panel
//! regions.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

use stores::tui::app::{App, Selection, TuiOpts};
use stores::tui::daemon::Liveness;
use stores::tui::data::StoreLane;
use stores::tui::render::{self, BOTTOM_CHROME_HEIGHT, TOP_STRIP_HEIGHT};

const W: u16 = 160;
const H: u16 = 40;

/// Build an in-memory DB with one rich row per store + engine-health rows.
fn seed_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch(
        r#"
        CREATE TABLE tasks (
            display_id TEXT, status TEXT, title TEXT, claimed_by TEXT, updated_at TEXT,
            tier_hint TEXT, linked_observations TEXT, blocked_reason TEXT,
                lifecycle TEXT, active_step TEXT, integration_step TEXT, blocked INTEGER, blocker_kind TEXT,
            current_phase INTEGER, current_cycle INTEGER, plan TEXT, plan_source TEXT,
            contract TEXT, plan_review_log TEXT, cycles TEXT, wrap_log TEXT,
            branch TEXT, workspace_path TEXT,
            claimed_at TEXT, integration_attempts TEXT
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
            held_reason TEXT, next_retry_at TEXT, attempts INTEGER,
            verdict TEXT, base_sha TEXT, head_sha TEXT,
            log_path TEXT, transcript_path TEXT,
            started_at TEXT, completed_at TEXT, duration_ms INTEGER,
            critical_count INTEGER, major_count INTEGER, minor_count INTEGER
        );
        CREATE TABLE intake (
            display_id TEXT, status TEXT, summary TEXT, body TEXT, updated_at TEXT,
            source_task TEXT, source_agent TEXT, risk_flags TEXT, cluster_key TEXT,
            decision TEXT, missing_info_question TEXT, routed_to_observation TEXT,
            routed_to_arch_review TEXT, duplicate_of TEXT, evidence TEXT,
            captured_at TEXT, recon_round INTEGER, decision_metadata TEXT
        );
        CREATE TABLE dispatch_locks (
            id INTEGER PRIMARY KEY,
            display_id TEXT, agent_name TEXT, claimed_by TEXT,
            claimed_at TEXT, heartbeat_at TEXT, finished_at TEXT, attempts INTEGER
        );
        CREATE TABLE daemon_starts (
            id INTEGER PRIMARY KEY,
            pid INTEGER NOT NULL, started_at TEXT,
            binary_version TEXT, git_sha TEXT
        );

        -- Task: executing, with linked observation L100, integration_attempts JSON.
        INSERT INTO tasks (display_id,status,title,updated_at,linked_observations,claimed_at,integration_attempts)
        VALUES ('T100','executing','active task with integration attempts','2026-05-09',
                '["L100"]','2026-05-09T10:00:00',
                '[{"attempt_no":1,"outcome":"failed"}]');

        -- Observation: tier T2, contract ready, linked task T100.
        INSERT INTO observations (display_id,status,priority,summary,updated_at,task_id,intent_contract)
        VALUES ('L100','open','normal','linked obs','2026-05-09','T100',
                '{"contract_state":"ready","tier_hint":"T2","objective":"obj"}');

        -- External review: status running so load_rows picks it up; verdict PASS,
        -- base_sha set, log_path set, finding counts populated.
        INSERT INTO external_reviews (display_id,task_id,status,runner,attempts,verdict,base_sha,head_sha,log_path,critical_count,major_count,minor_count)
        VALUES ('E100','T100','running','codex',1,'PASS','abcdef0123456789','fedcba9876543210','/tmp/E100.log',0,1,2);

        -- Intake: status draft, captured_at + recon_round + decision_metadata
        -- carrying rationale/confidence/tier_hint.
        INSERT INTO intake (display_id,status,summary,updated_at,captured_at,recon_round,decision_metadata)
        VALUES ('I100','draft','intake captured row','2026-05-09','2026-05-09T08:00:00',1,
                '{"rationale":"matches dispatch cluster","confidence":"medium","tier_hint":"T2"}');

        -- Engine: one daemon_starts row + one unfinished dispatch_lock with agent_name.
        INSERT INTO daemon_starts (pid,started_at,binary_version,git_sha)
        VALUES (4242,'2026-05-09T07:00:00','0.7.0','deadbeefcafe');
        INSERT INTO dispatch_locks (display_id,agent_name,claimed_by,claimed_at,heartbeat_at,finished_at,attempts)
        VALUES ('T100','planner','engine-1','2026-05-09T09:00:00',NULL,NULL,2);
        "#,
    )
    .expect("seed fixture");
    conn
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn paint(app: &mut App) -> Buffer {
    let backend = TestBackend::new(W, H);
    let mut terminal = Terminal::new(backend).expect("test backend");
    terminal.draw(|f| render::draw(f, app)).expect("draw");
    terminal.backend().buffer().clone()
}

/// Slice [col_start, col_end) × [row_start, row_end) of `buf` to a string.
fn region_text(buf: &Buffer, col_start: u16, col_end: u16, row_start: u16, row_end: u16) -> String {
    let mut s = String::new();
    for y in row_start..row_end {
        for x in col_start..col_end {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

/// Region helpers derived from the cockpit layout. Middle band sits between
/// the top strip and bottom chrome; the horizontal split assigns 13/23 to
/// the focused-table region and 10/23 to the side-detail pane.
fn middle_rows() -> (u16, u16) {
    (TOP_STRIP_HEIGHT, H - BOTTOM_CHROME_HEIGHT)
}
fn focused_cols() -> (u16, u16) {
    let split = (W as u32 * 13 / 23) as u16;
    (0, split)
}
fn detail_cols() -> (u16, u16) {
    let split = (W as u32 * 13 / 23) as u16;
    (split, W)
}

fn side_detail_text(buf: &Buffer) -> String {
    let (r0, r1) = middle_rows();
    let (c0, c1) = detail_cols();
    region_text(buf, c0, c1, r0, r1)
}

fn focused_table_text(buf: &Buffer) -> String {
    let (r0, r1) = middle_rows();
    let (c0, c1) = focused_cols();
    region_text(buf, c0, c1, r0, r1)
}

/// Build the App, refresh against the seeded DB, and force daemon liveness
/// to a deterministic value (Dead — pidfile is unrelated to this test's cwd).
fn build_app(conn: &Connection) -> App {
    let mut app = App::new(TuiOpts::default());
    app.refresh(conn).expect("refresh");
    app.status_bar.daemon_liveness = Liveness::Dead;
    app
}

/// Cycle focus to `lane`, then snap the selection onto the first navigable
/// row in that lane (so the side-detail pane renders the seeded row).
fn focus_lane(app: &mut App, lane: StoreLane) {
    while app.focused_store != lane {
        app.cycle_focus(1);
    }
    let flat = app.flat_rows();
    if let Some(first) = flat.first() {
        app.selection = Selection {
            section: first.section,
            row: first.row,
        };
    }
}

#[test]
fn tasks_lane_side_detail_includes_integration_attempts() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::Tasks);
    let buf = paint(&mut app);
    let detail = side_detail_text(&buf);
    assert!(
        detail.to_lowercase().contains("integration"),
        "Tasks lane side-detail must surface 'integration' substring (Integration-attempts section):\n{detail}"
    );
}

#[test]
fn observations_lane_side_detail_includes_linked_task() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::Observations);
    let buf = paint(&mut app);
    let detail = side_detail_text(&buf);
    assert!(
        detail.contains("Linked tasks"),
        "Observations lane side-detail must include 'Linked tasks' header:\n{detail}"
    );
    assert!(
        detail.contains("T100"),
        "Observations lane side-detail must list linked task display_id T100:\n{detail}"
    );
}

#[test]
fn intake_lane_side_detail_includes_captured_and_rationale() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::Intake);
    let buf = paint(&mut app);
    let detail = side_detail_text(&buf);
    assert!(
        detail.contains("captured:"),
        "Intake lane side-detail must include 'captured:' label:\n{detail}"
    );
    assert!(
        detail.contains("matches dispatch cluster"),
        "Intake lane side-detail must include decision_metadata.rationale text:\n{detail}"
    );
}

#[test]
fn external_reviews_lane_side_detail_includes_verdict_and_base_sha() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::ExternalReviews);
    let buf = paint(&mut app);
    let detail = side_detail_text(&buf);
    assert!(
        detail.contains("verdict:"),
        "External reviews lane side-detail must include 'verdict:' label:\n{detail}"
    );
    assert!(
        detail.contains("base_sha:"),
        "External reviews lane side-detail must include 'base_sha:' label:\n{detail}"
    );
}

#[test]
fn engine_lane_side_detail_includes_daemon_locks_and_agent_name() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::EngineHealth);
    let buf = paint(&mut app);
    let detail = side_detail_text(&buf);
    assert!(
        detail.contains("daemon:"),
        "Engine lane side-detail must include 'daemon:' label:\n{detail}"
    );
    assert!(
        detail.contains("unfinished_locks:"),
        "Engine lane side-detail must include 'unfinished_locks:' label:\n{detail}"
    );
    assert!(
        detail.contains("planner"),
        "Engine lane side-detail must list seeded agent_name 'planner':\n{detail}"
    );
}

#[test]
fn engine_lane_renders_active_runner_state() {
    let conn = seed_conn();
    let now = now_epoch();
    conn.execute(
        "INSERT INTO dispatch_locks (display_id,agent_name,claimed_by,claimed_at,heartbeat_at,finished_at,attempts) \
         VALUES ('T101','auto-drive','engine-1',?1,?2,NULL,1)",
        rusqlite::params![(now - 10).to_string(), (now - 5).to_string()],
    )
    .unwrap();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::EngineHealth);
    let buf = paint(&mut app);
    let panel = focused_table_text(&buf);
    let detail = side_detail_text(&buf);
    let text = format!("{panel}\n{detail}");
    assert!(
        text.contains("last_progress="),
        "missing last_progress: {text}"
    );
    assert!(
        text.contains("state=active"),
        "missing active state: {text}"
    );
}

#[test]
fn engine_lane_renders_stalled_runner_state() {
    let conn = seed_conn();
    let now = now_epoch();
    conn.execute(
        "INSERT INTO dispatch_locks (display_id,agent_name,claimed_by,claimed_at,heartbeat_at,finished_at,attempts) \
         VALUES ('T102','auto-drive','engine-1',?1,?2,NULL,1)",
        rusqlite::params![(now - 599).to_string(), (now - 600).to_string()],
    )
    .unwrap();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::EngineHealth);
    let buf = paint(&mut app);
    let panel = focused_table_text(&buf);
    let detail = side_detail_text(&buf);
    let text = format!("{panel}\n{detail}");
    assert!(
        text.contains("state=stalled_no_output"),
        "missing stalled state: {text}"
    );
    let idle = text
        .split("idle=")
        .nth(1)
        .and_then(|s| s.split('s').next())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    assert!(idle >= 180, "idle must be >=180, got {idle}: {text}");
}

#[test]
fn engine_lane_focused_table_includes_recent_restart_pid_line() {
    let conn = seed_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::EngineHealth);
    let buf = paint(&mut app);
    let panel = focused_table_text(&buf);
    assert!(
        panel.contains("recent_restart:"),
        "Engine panel must include 'recent_restart:' line sourced from daemon_starts:\n{panel}"
    );
    assert!(
        panel.contains("pid="),
        "Engine panel recent_restart line must include 'pid=' substring:\n{panel}"
    );
}
