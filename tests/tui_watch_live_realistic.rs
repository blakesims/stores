use ratatui::backend::TestBackend;
use ratatui::Terminal;
use rusqlite::{params, Connection};
use std::time::{SystemTime, UNIX_EPOCH};
use stores::tui::app::StatusBar;
use stores::tui::daemon::Liveness;
use stores::tui::sort::Sort;
use stores::tui::{render, App, TuiOpts};

const CLUSTER_COUNTS: [usize; 7] = [76, 47, 40, 35, 31, 28, 24];
const SNAPSHOT: &str = include_str!("fixtures/watch/live_realistic.snap");

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn seed_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE tasks (
            display_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            title TEXT,
            updated_at TEXT,
            tier_hint TEXT,
            linked_observations TEXT,
            blocked_reason TEXT,
            current_phase INTEGER,
            current_cycle INTEGER,
            plan TEXT
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
            intent_contract TEXT,
            investigation_failure_reason TEXT
        );
        CREATE TABLE intake (
            display_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            summary TEXT,
            body TEXT,
            updated_at TEXT,
            risk_flags TEXT,
            missing_info_question TEXT
        );
        CREATE TABLE external_reviews (
            display_id TEXT PRIMARY KEY,
            task_id TEXT,
            status TEXT,
            runner TEXT,
            held_reason TEXT,
            next_retry_at TEXT,
            attempts INTEGER
        );
        CREATE TABLE dispatch_locks (
            display_id TEXT PRIMARY KEY,
            task_id TEXT,
            claimed_at INTEGER,
            finished_at INTEGER,
            last_status TEXT
        );
        "#,
    )
    .unwrap();

    for n in 0..7 {
        let task_id = format!("T{:03}", 300 + n);
        insert_task(
            &conn,
            &task_id,
            "deploy_blocked",
            &format!("deploy recovery {task_id}"),
            Some("retry-deploy-recoverable"),
        );
    }
    for n in 0..4 {
        insert_task(
            &conn,
            &format!("T{:03}", 400 + n),
            "in_review",
            &format!("accept gate {}", 400 + n),
            None,
        );
    }
    insert_task(&conn, "T500", "plan_review", "planner AI review", None);
    insert_task(&conn, "T501", "code_review", "code AI review", None);
    insert_task(
        &conn,
        "T600",
        "blocked",
        "silent zombie one",
        Some("silent_zombie:pid_dead"),
    );
    insert_task(
        &conn,
        "T601",
        "blocked",
        "silent zombie two",
        Some("drive_failed:silent_zombie_pid_dead"),
    );

    let mut seq = 1usize;
    for (cluster_idx, count) in CLUSTER_COUNTS.iter().enumerate() {
        let task_id = format!("T{:03}", 300 + cluster_idx);
        let summary = format!("deploy-blocked: task {task_id} merge conflict");
        for _ in 0..*count {
            insert_obs(
                &conn,
                &format!("L{:03}", seq),
                "normal",
                &summary,
                Some(&task_id),
                None,
                None,
            );
            seq += 1;
        }
    }
    for n in 0..3 {
        insert_obs(
            &conn,
            &format!("L{:03}", 900 + n),
            "normal",
            &format!("ratify ready contract {}", n + 1),
            None,
            Some(r#"{"contract_state":"ready","acceptance":["ok"]}"#),
            None,
        );
    }

    conn.execute(
        "INSERT INTO intake (display_id,status,summary,body,updated_at,risk_flags,missing_info_question) VALUES ('I001','needs_info','intake needs routing','body','1700000000','[]','missing owner')",
        [],
    )
    .unwrap();

    let oldest = now_epoch() - (8 * 3600 + 10);
    for n in 0..8 {
        conn.execute(
            "INSERT INTO dispatch_locks (display_id,task_id,claimed_at,finished_at,last_status) VALUES (?1,?2,?3,NULL,'in_flight:pending_next')",
            params![format!("D{:03}", n + 1), format!("T{:03}", 400 + (n % 4)), oldest + n as i64],
        )
        .unwrap();
    }

    conn
}

fn insert_task(conn: &Connection, id: &str, status: &str, title: &str, reason: Option<&str>) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,updated_at,tier_hint,linked_observations,blocked_reason,current_phase,current_cycle,plan) VALUES (?1,?2,?3,'1700000000','T2','[]',?4,1,1,'{\"phases\":[{}]}')",
        params![id, status, title, reason],
    )
    .unwrap();
}

fn insert_obs(
    conn: &Connection,
    id: &str,
    priority: &str,
    summary: &str,
    task_id: Option<&str>,
    intent_contract: Option<&str>,
    investigation_failure_reason: Option<&str>,
) {
    conn.execute(
        "INSERT INTO observations (display_id,status,priority,summary,updated_at,body,task_id,priority_rank,intent_contract,investigation_failure_reason) VALUES (?1,'open',?2,?3,'1700000000','body',?4,5,?5,?6)",
        params![id, priority, summary, task_id, intent_contract, investigation_failure_reason],
    )
    .unwrap();
}

fn live_app() -> App {
    let conn = seed_db();
    let mut app = App::new(TuiOpts::default());
    app.refresh(&conn).unwrap();
    app.status_bar = StatusBar {
        daemon_liveness: Liveness::Dead,
        ..app.status_bar
    };
    app.sort = Sort::DisplayId;
    app.apply_sort();
    app
}

fn render_snapshot_text(app: &mut App) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut lines = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        let line = line.trim_end().to_string();
        if line == "stores watch · cockpit"
            || line.starts_with("j/k move")
            || line.starts_with("sort:")
        {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    format!("{}\n", lines.join("\n"))
}

#[test]
fn tui_watch_live_realistic_snapshot_and_budget() {
    let mut app = live_app();
    let got = render_snapshot_text(&mut app);
    assert_eq!(got, SNAPSHOT);

    let body_lines: Vec<&str> = got.lines().skip(4).collect();
    assert!(
        body_lines[0].starts_with("system-alert: daemon DEAD; 8 dangling locks; oldest started"),
        "{}",
        body_lines[0]
    );
    assert!(
        body_lines.len() <= 30,
        "visible body lines after header: {}\n{got}",
        body_lines.len()
    );

    let badge_lines: Vec<&str> = got.lines().filter(|line| line.contains('×')).collect();
    assert_eq!(badge_lines.len(), 7, "{badge_lines:#?}");
    for count in CLUSTER_COUNTS {
        assert_eq!(
            badge_lines
                .iter()
                .filter(|line| line.contains(&format!("×{count} ")))
                .count(),
            1,
            "missing or duplicate ×{count}: {badge_lines:#?}"
        );
    }
    assert_eq!(
        got.lines()
            .filter(|line| line.contains("deploy-blocked: task T"))
            .count(),
        7,
        "duplicate observations should collapse to seven rendered rows"
    );
    assert!(!got.contains("L281 open priority:normal deploy-blocked"));
    assert!(got.contains("HELD-AI-REVIEW"));
    assert!(got.contains("RATIFY-U1"));
    assert!(got.contains("ACCEPT-U3"));
    assert!(got.contains("HELD-INTAKE"));
}
