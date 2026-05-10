mod framework_migrate {
    use rusqlite::Connection;
    use stores::codegen::ddl::ddl_for;
    use stores::handlers::framework_migrate::apply_framework_drift;
    use stores::schema::Schema;

    fn fresh_db_with_tasks() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let schema = Schema::from_yaml(include_str!("../stores/tasks/schema.yaml")).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    #[test]
    fn queued_lifecycle_backfill() {
        let conn = fresh_db_with_tasks();
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
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES ('T901','planning','p','p','n','n','framework','framework')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES ('T902','executing','e','e','n','n','framework','framework')",
            [],
        ).unwrap();
        apply_framework_drift(&conn).unwrap();
        let queued: String = conn
            .query_row(
                "SELECT lifecycle FROM tasks WHERE display_id='T901'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let active: String = conn
            .query_row(
                "SELECT lifecycle FROM tasks WHERE display_id='T902'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(queued, "queued");
        assert_eq!(active, "active");
    }
}
