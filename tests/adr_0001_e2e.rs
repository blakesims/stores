use stores::handlers::framework_migrate::apply_framework_drift;
use stores::handlers::status::{fetch_task, format_task_line};
use stores::tui::data::{classify, load_rows, Row, Section};

#[test]
fn adr_0001_e2e() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE substrate_migrations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT
        );
        CREATE TABLE observations (
            display_id TEXT, status TEXT, priority TEXT, summary TEXT, updated_at TEXT
        );
        CREATE TABLE tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            display_id TEXT NOT NULL,
            status TEXT,
            title TEXT,
            slug TEXT,
            created_at TEXT,
            updated_at TEXT,
            created_by TEXT,
            updated_by TEXT,
            blocked_reason TEXT,
            current_phase INTEGER,
            current_cycle INTEGER,
            plan TEXT
        );
        "#,
    )
    .unwrap();
    for (i, status) in [
        "planning",
        "plan_review",
        "ready",
        "executing",
        "code_review",
        "in_review",
        "accepted",
        "integration_queued",
        "integrating",
        "integration_blocked",
        "complete",
        "schema_migrated",
    ]
    .iter()
    .enumerate()
    {
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by,plan) VALUES (?1,?2,?3,?4,'2026-05-11T00:00:00Z','2026-05-11T00:00:00Z','test','test','{\"phases\":[{}]}')",
            rusqlite::params![format!("T{:03}", i + 1), status, format!("{status} task"), format!("slug-{i}")],
        )
        .unwrap();
    }

    apply_framework_drift(&conn).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE lifecycle IS NOT NULL AND active_step IS NOT NULL AND integration_step IS NOT NULL AND blocked IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(count >= 10);

    conn.execute(
        "UPDATE tasks SET status='legacy_unknown' WHERE display_id='T009'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE tasks SET lifecycle='integration', active_step='none', integration_step='testing', blocked=0 WHERE display_id='T009'",
        [],
    )
    .unwrap();

    let task = fetch_task(&conn, "T009").unwrap();
    let status_line = format_task_line(&task);
    assert!(
        status_line.contains("lifecycle=integration"),
        "{status_line}"
    );
    assert!(
        status_line.contains("integration_step=testing"),
        "{status_line}"
    );

    let rows = load_rows(&conn).unwrap();
    let sections = classify(&rows);
    let idxs = sections
        .iter()
        .find(|(s, _)| *s == Section::TasksIntegration)
        .unwrap();
    assert!(idxs
        .1
        .iter()
        .any(|&idx| matches!(&rows[idx], Row::Task(t) if t.display_id == "T009")));
}
