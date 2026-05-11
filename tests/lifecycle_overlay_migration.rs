use rusqlite::Connection;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::db;
use stores::schema::Schema;

fn tasks_schema() -> Schema {
    Schema::from_yaml(include_str!("../stores/tasks/schema.yaml")).unwrap()
}

#[test]
fn pre_t144_tasks_rows_backfill_lifecycle_overlay_on_open() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    {
        let conn = Connection::open(tmp.path()).unwrap();
        let schema = tasks_schema();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        for col in [
            "lifecycle",
            "active_step",
            "integration_step",
            "blocked",
            "blocker_kind",
        ] {
            conn.execute_batch(&format!("ALTER TABLE tasks DROP COLUMN {col};"))
                .unwrap();
        }
        let rows = [
            ("T001", "executing", None, None),
            ("T002", "code_review", None, None),
            ("T003", "in_review", None, None),
            ("T004", "accepted", None, None),
            ("T005", "integration_queued", None, None),
            ("T006", "integrating", None, None),
            (
                "T007",
                "integration_blocked",
                None,
                Some("merge_failure: rejected"),
            ),
            ("T008", "schema_migrated", None, None),
            ("T009", "blocked", Some("terminal_reason=rate_limit"), None),
        ];
        for (display_id, status, blocked_reason, integration_blocked_reason) in rows {
            conn.execute(
                "INSERT INTO tasks (display_id, status, title, slug, activation, blocked_reason, integration_blocked_reason) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)",
                rusqlite::params![display_id, status, display_id, display_id.to_lowercase(), blocked_reason, integration_blocked_reason],
            )
            .unwrap();
        }
    }

    let conn = db::open(tmp.path()).unwrap();
    let got: Vec<(String, String, String, String, i64, Option<String>)> = conn
        .prepare(
            "SELECT display_id, lifecycle, active_step, integration_step, blocked, blocker_kind FROM tasks ORDER BY display_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(
        got,
        vec![
            (
                "T001".into(),
                "active".into(),
                "coding".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T002".into(),
                "active".into(),
                "coding_review".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T003".into(),
                "queued".into(),
                "none".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T004".into(),
                "queued".into(),
                "none".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T005".into(),
                "queued".into(),
                "none".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T006".into(),
                "active".into(),
                "none".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T007".into(),
                "queued".into(),
                "none".into(),
                "none".into(),
                1,
                Some("main_red".into())
            ),
            (
                "T008".into(),
                "done".into(),
                "none".into(),
                "none".into(),
                0,
                None
            ),
            (
                "T009".into(),
                "queued".into(),
                "none".into(),
                "none".into(),
                1,
                Some("rate_limit".into())
            ),
        ]
    );
}
