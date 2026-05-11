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
        let cases = [
            ("T901", "planning", "queued"),
            ("T902", "ready", "queued"),
            ("T903", "blocked", "queued"),
            ("T904", "complete", "queued"),
            ("T905", "in_review", "queued"),
            ("T906", "integrated", "queued"),
            ("T907", "executing", "active"),
            ("T908", "code_review", "active"),
            ("T909", "integrating", "integration"),
        ];
        for (display_id, status, _) in cases {
            conn.execute(
                "INSERT INTO tasks (display_id,status,title,slug,created_at,updated_at,created_by,updated_by) VALUES (?1,?2,?3,?3,'n','n','framework','framework')",
                rusqlite::params![display_id, status, status],
            ).unwrap();
        }
        apply_framework_drift(&conn).unwrap();
        for (display_id, status, expected_lifecycle) in cases {
            let got: (String, String, String) = conn
                .query_row(
                    "SELECT lifecycle, active_step, integration_step FROM tasks WHERE display_id=?1",
                    [display_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(got.0, expected_lifecycle, "{status}");
            if expected_lifecycle == "queued" {
                assert_eq!(got.1, "none", "active_step for {status}");
                assert_eq!(got.2, "none", "integration_step for {status}");
            }
            if status == "integrating" {
                assert_eq!(got.1, "none", "active_step for {status}");
                assert_eq!(got.2, "merging", "integration_step for {status}");
            }
        }
    }
}
