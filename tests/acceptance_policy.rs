mod acceptance_policy {
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use stores::flow::builtins::{dispatch_builtin, postcondition_for_builtin, DispatchCtx};
    use stores::flow::{AgentsYaml, PoliciesYaml};
    use stores::handlers::agents_run::poll_once;
    use stores::schema::actor::{Actor, InvokerCtx};
    use stores::schema::Schema;
    use stores::validate::{self, Op};

    fn schema(name: &str) -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        Schema::from_yaml(yaml).unwrap()
    }

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SUBSTRATE_DDL).unwrap();
        c.execute_batch(&ddl_for(&schema("tasks"))).unwrap();
        c.execute_batch(&ddl_for(&schema("external_reviews")))
            .unwrap();
        c
    }

    fn insert_task(
        c: &Connection,
        id: &str,
        policy: &str,
        review_policy: &str,
        decided: Option<&str>,
    ) -> i64 {
        let now = "2026-05-11T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        c.execute(
            "INSERT INTO tasks (display_id,status,title,slug,contract,created_at,updated_at,created_by,updated_by,lifecycle,active_step,integration_step,activation,human_acceptance_policy,task_review_policy,acceptance_decided_by) \
             VALUES (?1,'in_review','t','t',?2,?3,?3,'framework','framework','active','wrapping','none','active',?4,?5,?6)",
            params![id, contract, now, policy, review_policy, decided],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn insert_transition(c: &Connection, row_id: i64, id: &str, from: &str, to: &str) {
        c.execute(
            "INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at) \
             VALUES ('tasks',?1,?2,?3,?4,'test-seed','framework','2026-05-11T00:00:00Z')",
            params![row_id, id, from, to],
        )
        .unwrap();
    }

    fn release_agents() -> AgentsYaml {
        AgentsYaml::from_yaml(
            r#"
agents:
  - name: release-to-integration
    subscribes_to:
      - store: tasks
        transition: { from: complete, to: in_review }
    command: "builtin:release-to-integration"
    claim_window_secs: 300
    retry_policy: { max_attempts: 1, backoff: linear }
"#,
        )
        .unwrap()
    }

    fn empty_policies() -> PoliciesYaml {
        PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        }
    }

    fn insert_passed_review(c: &Connection, task_id: &str) {
        c.execute(
            "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,verdict,created_at,updated_at,created_by,updated_by) \
             VALUES ('ER001','passed',?1,1,'external_review','PASS','2026-05-11T00:00:00Z','2026-05-11T00:00:00Z','framework','framework')",
            params![task_id],
        )
        .unwrap();
    }

    fn row(c: &Connection, id: &str) -> Value {
        let mut stmt = c
            .prepare("SELECT * FROM tasks WHERE display_id=?1")
            .unwrap();
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(params![id]).unwrap();
        let r = rows.next().unwrap().unwrap();
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = r.get(i).unwrap();
            obj.insert(
                name.clone(),
                match v {
                    rusqlite::types::Value::Null => Value::Null,
                    rusqlite::types::Value::Integer(i) => json!(i),
                    rusqlite::types::Value::Real(f) => json!(f),
                    rusqlite::types::Value::Text(s) => json!(s),
                    rusqlite::types::Value::Blob(b) => {
                        json!(String::from_utf8_lossy(&b).to_string())
                    }
                },
            );
        }
        Value::Object(obj)
    }

    fn ctx(c: &Connection) -> DispatchCtx<'_> {
        let agents: &'static AgentsYaml = Box::leak(Box::new(AgentsYaml::default_empty()));
        let cfg: &'static PathBuf = Box::leak(Box::new(PathBuf::from(
            "/tmp/stores-acceptance-policy-test.yaml",
        )));
        DispatchCtx {
            conn: c,
            agents,
            config_path: cfg,
            policies_hash: "",
        }
    }

    #[test]
    fn required_acceptance_cannot_be_delegated() {
        let c = conn();
        insert_task(&c, "T501", "required", "authoritative", None);
        insert_passed_review(&c, "T501");
        let ctx = ctx(&c);
        let err = dispatch_builtin("release-to-integration", &row(&c, "T501"), &ctx)
            .unwrap()
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("human acceptance required but not recorded"));
    }

    #[test]
    fn delegated_by_policy_with_authoritative_task_review_releases() {
        let c = conn();
        let row_id = insert_task(&c, "T502", "delegated_by_policy", "authoritative", None);
        insert_passed_review(&c, "T502");
        insert_transition(&c, row_id, "T502", "complete", "in_review");
        let agents = release_agents();
        let policies = empty_policies();
        let cfg = PathBuf::from("/tmp/stores-acceptance-policy-test.yaml");
        let dispatched =
            poll_once(&c, &agents, &policies, &cfg, "test-claimer", "test-epoch").unwrap();
        assert_eq!(
            dispatched, 1,
            "release-to-integration subscriber must dispatch once"
        );
        let (lifecycle, step, by): (String, String, String) = c.query_row(
            "SELECT lifecycle,integration_step,acceptance_decided_by FROM tasks WHERE display_id='T502'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(
            (lifecycle.as_str(), step.as_str(), by.as_str()),
            ("integration", "queued", "policy_delegate")
        );
    }

    #[test]
    fn ai_autonomous_cannot_change_acceptance_policy() {
        let s = schema("tasks");
        let mut entry = BTreeMap::new();
        entry.insert("title".into(), json!("t"));
        entry.insert("slug".into(), json!("t"));
        entry.insert(
            "contract".into(),
            json!({"done_when":"x","scope_in":"y","scope_out":"z"}),
        );
        entry.insert("human_acceptance_policy".into(), json!("optional"));
        let mut diff = BTreeMap::new();
        diff.insert("human_acceptance_policy".into(), json!("optional"));
        let errs = validate::validate(
            &s,
            &entry,
            Op::Update(diff),
            InvokerCtx::bare(Actor::AiAutonomous),
        )
        .unwrap_err();
        assert!(validate::pretty_print(&errs).contains("human_acceptance_policy"));
    }

    #[test]
    fn release_subscriber_registered_and_fires() {
        let c = conn();
        insert_task(&c, "T503", "required", "none", Some("human"));
        let ctx = ctx(&c);
        assert!(postcondition_for_builtin("release-to-integration").is_some());
        assert!(dispatch_builtin("release-to-integration", &row(&c, "T503"), &ctx).is_some());
        dispatch_builtin("release-to-integration", &row(&c, "T503"), &ctx)
            .unwrap()
            .unwrap();
        let (lifecycle, step): (String, String) = c
            .query_row(
                "SELECT lifecycle,integration_step FROM tasks WHERE display_id='T503'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (lifecycle.as_str(), step.as_str()),
            ("integration", "queued")
        );
    }

    #[test]
    fn optional_policy_releases_without_acceptance() {
        let c = conn();
        insert_task(&c, "T504", "optional", "none", None);
        let ctx = ctx(&c);
        dispatch_builtin("release-to-integration", &row(&c, "T504"), &ctx)
            .unwrap()
            .unwrap();
        let lifecycle: String = c
            .query_row(
                "SELECT lifecycle FROM tasks WHERE display_id='T504'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lifecycle, "integration");
    }

    #[test]
    fn delegated_policy_without_authoritative_review_does_not_write_delegate() {
        let c = conn();
        insert_task(&c, "T505", "delegated_by_policy", "advisory", None);
        let ctx = ctx(&c);
        let err = dispatch_builtin("release-to-integration", &row(&c, "T505"), &ctx)
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().contains("acceptance_decided_by"));
    }

    #[test]
    fn ai_with_human_token_can_change_acceptance_policy() {
        let s = schema("tasks");
        let mut entry = BTreeMap::new();
        entry.insert("title".into(), json!("t"));
        entry.insert("slug".into(), json!("t"));
        entry.insert(
            "contract".into(),
            json!({"done_when":"x","scope_in":"y","scope_out":"z"}),
        );
        entry.insert("human_acceptance_policy".into(), json!("optional"));
        let mut diff = BTreeMap::new();
        diff.insert("human_acceptance_policy".into(), json!("optional"));
        validate::validate(
            &s,
            &entry,
            Op::Update(diff),
            InvokerCtx {
                actor: Actor::AiWithHuman,
                token_valid: true,
            },
        )
        .unwrap();
    }
}
