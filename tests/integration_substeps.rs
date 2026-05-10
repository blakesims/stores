use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::framework_migrate::ensure_integration_singleton_index;
use stores::handlers::resource_locks::{self, AcquireParams};
use stores::schema::actor::Actor;
use stores::schema::Schema;

mod integration_substeps {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        ensure_integration_singleton_index(&conn).unwrap();
        conn
    }

    fn insert_task_result(conn: &Connection, id: &str, step: &str) -> rusqlite::Result<usize> {
        conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, contract, activation, lifecycle, active_step, integration_step, blocked, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integrating', 'x', ?2, '{\"done_when\":\"x\",\"scope_in\":\"x\",\"scope_out\":\"x\"}', \
                 'active', 'integration', 'none', ?3, 0, '2026-05-11T00:00:00Z', '2026-05-11T00:00:00Z', 'framework', 'framework')",
        rusqlite::params![id, id.to_ascii_lowercase(), step],
    )
    }

    fn insert_task(conn: &Connection, id: &str, step: &str) {
        insert_task_result(conn, id, step).unwrap();
    }

    fn main_branch_locked(conn: &Connection) -> bool {
        resource_locks::list(conn)
            .unwrap()
            .iter()
            .any(|(resource_id, _, _, _)| resource_id == "main_branch")
    }

    #[test]
    fn refreshing_does_not_acquire_main_branch_lock() {
        let conn = conn();
        insert_task(&conn, "T301", "refreshing");
        assert!(!main_branch_locked(&conn));
    }

    #[test]
    fn task_review_does_not_acquire_main_branch_lock() {
        let conn = conn();
        insert_task(&conn, "T302", "task_review");
        assert!(!main_branch_locked(&conn));
    }

    #[test]
    fn testing_does_not_acquire_main_branch_lock() {
        let conn = conn();
        insert_task(&conn, "T303", "testing");
        assert!(!main_branch_locked(&conn));
    }

    #[test]
    fn main_branch_lock_only_during_merging() {
        let conn = conn();
        for (id, step) in [
            ("T304", "refreshing"),
            ("T305", "task_review"),
            ("T306", "testing"),
        ] {
            insert_task(&conn, id, step);
            assert!(
                !main_branch_locked(&conn),
                "{step} must not hold main_branch"
            );
        }
        insert_task(&conn, "T307", "merging");
        resource_locks::acquire(
            &conn,
            &AcquireParams {
                resource_id: "main_branch",
                owner_display_id: "T307",
                owner_kind: "task",
                ttl_secs: Some(600),
                claim_source: Some("integrate"),
                invoker: Actor::Framework,
            },
        )
        .unwrap();
        assert!(main_branch_locked(&conn));
    }

    #[test]
    fn only_one_merging_row_allowed_by_partial_unique_index() {
        let conn = conn();
        insert_task(&conn, "T308", "merging");
        let err = insert_task_result(&conn, "T309", "merging").unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::SqliteFailure(_, _)),
            "second merging row must violate partial unique index: {err:?}"
        );
    }

    #[test]
    fn three_rows_parallel_before_merge_one_row_merging() {
        let conn = conn();
        insert_task(&conn, "T310", "task_review");
        insert_task(&conn, "T311", "testing");
        insert_task(&conn, "T312", "merging");
        let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status='integrating' AND integration_step IN ('task_review','testing','merging')",
            [],
            |r| r.get(0),
        )
        .unwrap();
        assert_eq!(n, 3);
        let merging: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status='integrating' AND integration_step='merging'",
            [],
            |r| r.get(0),
        )
        .unwrap();
        assert_eq!(merging, 1);
    }
}
