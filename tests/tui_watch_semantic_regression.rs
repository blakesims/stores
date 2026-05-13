use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use rusqlite::{params, Connection};
use stores::tui::app::{App, Selection, TuiOpts};
use stores::tui::daemon::Liveness;
use stores::tui::data::{Row, StoreLane};
use stores::tui::render::{self, BOTTOM_CHROME_HEIGHT, TOP_STRIP_HEIGHT};
use stores::tui::semantics::{observation_watch_projection, WatchSlotId};

const W: u16 = 200;
const H: u16 = 42;

fn seed_semantic_watch_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE tasks (
            display_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            title TEXT,
            claimed_by TEXT,
            updated_at TEXT,
            tier_hint TEXT,
            linked_observations TEXT,
            blocked_reason TEXT,
            lifecycle TEXT,
            active_step TEXT,
            integration_step TEXT,
            blocked INTEGER,
            blocker_kind TEXT,
            current_phase INTEGER,
            current_cycle INTEGER,
            plan TEXT,
            workspace_path TEXT,
            claimed_at TEXT
        );
        CREATE TABLE observations (
            display_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            priority TEXT,
            summary TEXT,
            updated_at TEXT,
            body TEXT,
            task_id TEXT,
            priority_rank INTEGER,
            lifecycle TEXT,
            waiting INTEGER,
            waiting_kind TEXT,
            outcome TEXT,
            pending_architecture_review INTEGER,
            open_architecture_review_id TEXT,
            superseded_by_id TEXT,
            contract_state TEXT,
            intent_contract TEXT,
            investigation_failure_reason TEXT
        );
        CREATE TABLE intake (
            display_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            summary TEXT,
            body TEXT,
            updated_at TEXT,
            lifecycle TEXT,
            waiting_kind TEXT,
            outcome TEXT
        );
        CREATE TABLE external_reviews (
            display_id TEXT PRIMARY KEY,
            task_id TEXT,
            status TEXT,
            runner TEXT,
            held_reason TEXT,
            next_retry_at TEXT,
            attempts INTEGER,
            verdict TEXT,
            base_sha TEXT,
            head_sha TEXT,
            log_path TEXT,
            transcript_path TEXT,
            started_at TEXT,
            completed_at TEXT,
            duration_ms INTEGER,
            critical_count INTEGER,
            major_count INTEGER,
            minor_count INTEGER
        );
        CREATE TABLE dispatch_locks (
            display_id TEXT PRIMARY KEY,
            task_id TEXT,
            agent_name TEXT,
            claimed_by TEXT,
            claimed_at TEXT,
            heartbeat_at TEXT,
            finished_at TEXT,
            attempts INTEGER,
            last_status TEXT
        );
        "#,
    )
    .unwrap();

    let tasks = [
        (
            "T001",
            "planning",
            "queued inactive task",
            "queued",
            "none",
            "none",
            0,
            None,
            None,
        ),
        (
            "T002",
            "planning",
            "active planning task",
            "active",
            "planning",
            "none",
            0,
            None,
            None,
        ),
        (
            "T003",
            "plan_review",
            "plan review task",
            "active",
            "planning_review",
            "none",
            0,
            None,
            None,
        ),
        (
            "T004",
            "executing",
            "executing task",
            "active",
            "coding",
            "none",
            0,
            None,
            None,
        ),
        (
            "T005",
            "in_review",
            "acceptance task with pending external review",
            "active",
            "wrapping",
            "none",
            0,
            None,
            None,
        ),
        (
            "T006",
            "blocked",
            "runner blocked task",
            "active",
            "none",
            "none",
            1,
            Some("runner"),
            Some(r#"{"exit_code":42,"kind":"runner_crash"}"#),
        ),
        (
            "T007",
            "planning",
            "capacity blocked queued task",
            "queued",
            "none",
            "none",
            1,
            Some("capacity"),
            None,
        ),
    ];
    for (
        id,
        status,
        title,
        lifecycle,
        active_step,
        integration_step,
        blocked,
        blocker_kind,
        blocked_reason,
    ) in tasks
    {
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,updated_at,tier_hint,linked_observations,blocked_reason,lifecycle,active_step,integration_step,blocked,blocker_kind,current_phase,current_cycle,plan,workspace_path) \
             VALUES (?1,?2,?3,'2026-05-13T00:00:00Z','T2','[]',?4,?5,?6,?7,?8,?9,1,1,'{\"phases\":[{}]}','/tmp/semantic-watch')",
            params![id, status, title, blocked_reason, lifecycle, active_step, integration_step, blocked, blocker_kind],
        )
        .unwrap();
    }

    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,priority_rank,lifecycle) \
         VALUES ('L001','open','normal','candidate observation','2026-05-13','body',5,'candidate')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,priority_rank,lifecycle,contract_state,intent_contract) \
         VALUES ('L002','open','normal','draft contract observation','2026-05-13','body',5,'ready','draft','{\"contract_state\":\"draft\",\"tier_hint\":\"T2\",\"objective\":\"draft it\"}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,priority_rank,lifecycle,waiting,waiting_kind) \
         VALUES ('L003','needs_info','normal','needs info observation','2026-05-13','body',5,'waiting',1,'info_needed')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,priority_rank,lifecycle,contract_state,waiting,waiting_kind,investigation_failure_reason) \
         VALUES ('L004','investigation_failed','normal','schema-produced investigation failure','2026-05-13','body',5,'candidate','draft',1,'human_ratification','schema handler raised')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,priority_rank,lifecycle,outcome) \
         VALUES ('L005','resolved','normal','closed wont fix observation','2026-05-13','body',5,'closed','closed_wont_fix')",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO external_reviews (display_id,task_id,status,runner,held_reason,next_retry_at,attempts) \
         VALUES ('ER001','T005','pending','unknown','none','none',0)",
        [],
    )
    .unwrap();

    conn
}

fn build_app(conn: &Connection) -> App {
    let mut app = App::new(TuiOpts::default());
    app.refresh(conn).unwrap();
    app.status_bar.daemon_liveness = Liveness::Dead;
    app
}

fn paint(app: &mut App) -> Buffer {
    let backend = TestBackend::new(W, H);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

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

fn focused_cols() -> (u16, u16) {
    ((W as u32 * 13 / 23) as u16, (W as u32 * 13 / 23) as u16)
}

fn focused_table_text(buf: &Buffer) -> String {
    let split = focused_cols().0;
    region_text(buf, 0, split, TOP_STRIP_HEIGHT, H - BOTTOM_CHROME_HEIGHT)
}

fn top_strip_text(buf: &Buffer) -> String {
    region_text(buf, 0, W, 0, TOP_STRIP_HEIGHT)
}

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

fn all_default_row_text(app: &mut App) -> String {
    let mut out = String::new();
    for lane in StoreLane::ALL {
        focus_lane(app, lane);
        let buf = paint(app);
        out.push_str(&focused_table_text(&buf));
        out.push('\n');
    }
    out
}

#[test]
fn seeded_clean_db_rows_render_semantic_watch_vocabulary() {
    let conn = seed_semantic_watch_conn();
    let mut app = build_app(&conn);

    let rows = all_default_row_text(&mut app);
    for expected in [
        "ID",
        "SUMMARY",
        "MAP",
        "queued",
        "○ │ ·",
        "● │ □",
        "▰",
        "runner",
        "capacity",
        "candidate",
        "CONTRACT GATE",
        "draft",
        "info needed",
        "investigation failed",
        "pending",
        "runner=—",
    ] {
        assert!(
            rows.contains(expected),
            "missing semantic token {expected:?}:\n{rows}"
        );
    }

    let failure = app
        .rows
        .iter()
        .find_map(|row| match row {
            Row::Obs(obs) if obs.display_id == "L004" => Some(observation_watch_projection(obs)),
            _ => None,
        })
        .expect("schema-produced investigation failure row");
    assert_eq!(failure.slot, WatchSlotId::Fault);
    assert_eq!(failure.slot_label, "errors");
    assert_eq!(failure.row_stage, "investigation failed");
    assert_eq!(failure.row_signal.as_deref(), Some("schema handler raised"));

    for clutter in [
        "runner:none",
        "active:none:none",
        "lifecycle=",
        "active_step=",
        "integration_step=",
        "runner=unknown",
        "held_reason=none",
        "next_retry_at=none",
    ] {
        assert!(
            !rows.contains(clutter),
            "default rows leaked raw clutter {clutter:?}:\n{rows}"
        );
    }
}

#[test]
fn seeded_clean_db_top_cards_use_shared_flow_slots() {
    let conn = seed_semantic_watch_conn();
    let mut app = build_app(&conn);
    focus_lane(&mut app, StoreLane::Tasks);
    let top = top_strip_text(&paint(&mut app));

    for expected in [
        "◌ 0", "◆ 0", "◇ 0", "✓ 0", "△ 0", "▲ 0", "◌ 1", "◇ 1", "△ 1", "◌ 1", "◆ 2", "◇ 2", "▲ 1",
        "✓ 1", "△",
    ] {
        assert!(
            top.contains(expected),
            "top card missing shared-flow token {expected:?}:\n{top}"
        );
    }
    for expected in [
        "new",
        "triage",
        "needs info",
        "routed",
        "errors",
        "candidates",
        "investigate",
        "contract",
        "closed",
        "queued",
        "working",
        "done",
        "failed",
        "pending",
        "running",
        "revise",
        "passed",
        "tool fault",
        "dispatch",
        "runners",
        "locks",
        "clear",
        "manual",
    ] {
        assert!(
            top.contains(expected),
            "top card missing shared-flow label {expected:?}:\n{top}"
        );
    }
}

#[test]
fn task_detail_retains_debug_tuple_while_default_rows_hide_it() {
    let conn = seed_semantic_watch_conn();
    let mut app = build_app(&conn);
    let runner_row = app
        .rows
        .iter()
        .position(|r| matches!(r, Row::Task(t) if t.display_id == "T006"))
        .expect("runner-blocked task row");
    focus_lane(&mut app, StoreLane::Tasks);
    let flat = app.flat_rows();
    let selected = flat
        .iter()
        .find(|fr| fr.abs == runner_row)
        .expect("runner-blocked flat row");
    app.selection = Selection {
        section: selected.section,
        row: selected.row,
    };

    let buf = paint(&mut app);
    let rows = focused_table_text(&buf);
    let detail = stores::tui::detail::render_text_for_row(app.current_row().unwrap(), &app);

    assert!(
        !rows.contains("lifecycle="),
        "row leaked debug tuple:\n{rows}"
    );
    assert!(
        !rows.contains("active_step="),
        "row leaked debug tuple:\n{rows}"
    );
    assert!(
        detail.contains("Debug tuple"),
        "detail missing debug tuple:\n{detail}"
    );
    assert!(
        detail.contains("lifecycle:"),
        "detail missing lifecycle field:\n{detail}"
    );
    assert!(
        detail.contains("active_step:"),
        "detail missing active_step field:\n{detail}"
    );
    assert!(
        detail.contains("blocked_reason:"),
        "detail missing blocker debug field:\n{detail}"
    );
}
