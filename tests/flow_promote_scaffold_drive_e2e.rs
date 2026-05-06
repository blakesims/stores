//! T022 Phase 7 — E2E integration test for the full upstream-autonomy chain
//! including the auto-drive subscriber.
//!
//! Walks ratify → auto-promote → auto-scaffold → auto-drive on a tempdir
//! repo using the production agents.yaml fixture. The drive binary is stubbed
//! via the `STORES_DRIVE_CMD` env override: a tiny shell script that flips
//! `tasks.status` to `in_review` via raw SQL UPDATE, simulating the wrap
//! envelope landing without needing a real claude-code runner.
//!
//! Three scenarios:
//!
//!   * `ratify_promote_scaffold_drive_happy_path` — full chain to in_review.
//!   * `drive_failure_watchdog_flips_blocked` — sweep_drive_watchdog flips a
//!     row whose drive subprocess died without producing wrap.
//!   * `drive_idempotent_no_double_spawn_after_restart` — UNIQUE-claim on
//!     dispatch_locks blocks a second auto-drive spawn after a daemon
//!     restart simulation.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::load_from_path;
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{install_notifier, MockNotifier, NotifierBackend, NotifyEvent};
use stores::handlers::add;
use stores::handlers::agents_run::poll_once;
use stores::schema::{actor::Actor, FieldType, Schema};

/// Serialize env var manipulation across tests in this binary.
fn env_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn fresh_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "observations", "gate"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let s = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&s)).unwrap();
    }
    conn
}

fn build_obs_add_cmd(schema: &Schema) -> clap::Command {
    use clap::{Arg, ArgAction};
    let leaves = stores::schema::flatten::leaf_args(schema).unwrap();
    let mut cmd = clap::Command::new("add");
    for leaf in &leaves {
        let is_text_like = matches!(
            leaf.field.ty,
            FieldType::Text | FieldType::Timestamp | FieldType::DisplayId | FieldType::Json
        );
        let is_list = matches!(
            leaf.field.ty,
            FieldType::List(_) | FieldType::ListFk { .. } | FieldType::ListRecord(_)
        );
        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .required(false);
        if is_list {
            arg = arg.action(ArgAction::Append);
        }
        cmd = cmd.arg(arg);
        if is_text_like {
            let from_file_name = format!("{}-from-file", leaf.cli_name);
            cmd = cmd.arg(
                Arg::new(from_file_name.clone())
                    .long(from_file_name)
                    .required(false),
            );
        }
    }
    cmd = cmd.arg(Arg::new("display-id").long("display-id").required(false));
    cmd = cmd.arg(
        Arg::new("lock-contract")
            .long("lock-contract")
            .action(ArgAction::SetTrue)
            .required(false),
    );
    cmd
}

/// Set up tempdir + git + .stores/db.sqlite + scaffold config. Returns the
/// repo path, db path, config path. Mirrors the harness in
/// flow_promote_scaffold_e2e.rs (Task 7.1).
fn setup_repo_with_scaffold() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test+t022p7@example.com"]);
    git(&repo, &["config", "user.name", "T022 P7 Test"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git(&repo, &["add", "README.md"]);
    let init = git(&repo, &["commit", "-m", "init"]);
    assert!(init.status.success(), "git init commit failed: {:?}", init);

    let stores_dir = repo.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    let db_path = stores_dir.join("db.sqlite");
    let _ = fresh_db(&db_path);

    let cfg_path = stores_dir.join("config.yaml");
    let repo_canon = repo.canonicalize().unwrap();
    let repo_str = repo_canon.to_string_lossy().to_string();
    let scaffold_cmd = format!(
        "cd {repo} && git worktree add -b feat/{{display_id}}-{{slug}} \
         wt-{{display_id}} >/dev/null 2>&1 && echo {repo}/wt-{{display_id}}",
        repo = repo_str
    );
    std::fs::write(
        &cfg_path,
        format!(
            "scaffold:\n  command: \"{}\"\nntfy:\n  url: https://test.local\n",
            scaffold_cmd
        ),
    )
    .unwrap();

    (tmp, db_path, cfg_path)
}

/// File a ratified observation that auto-promote will see (Task 7.1 reuse of
/// flow_promote_scaffold_e2e harness).
fn file_ratified_observation(conn: &Connection, summary: &str) -> String {
    let obs_yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "observations")
        .map(|(_, y)| *y)
        .unwrap();
    let obs_schema = Schema::from_yaml(obs_yaml).unwrap();
    let cmd = build_obs_add_cmd(&obs_schema);
    let matches = cmd.get_matches_from([
        "add",
        "--summary",
        summary,
        "--source",
        "dev",
        "--priority",
        "normal",
        "--captured-at",
        "2026-05-04T00:00:00Z",
        "--captured-week",
        "w18-d7",
        "--objective",
        "T022 P7 drive e2e",
        "--type",
        "work",
        "--in-scope",
        "drive",
        "--out-of-scope",
        "real claude",
        "--acceptance",
        "row reaches in_review via stub",
        "--tier-hint",
        "T2",
        "--lock-contract",
    ]);
    add::run(&obs_schema, conn, &matches, Actor::Human.into())
        .expect("observations add --lock-contract --invoker human must succeed");
    conn.query_row(
        "SELECT display_id FROM observations ORDER BY id DESC LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .unwrap()
}

/// Install a fresh MockNotifier on the global backend. Returned reference is
/// leaked — fine in tests, prevents lifetime headaches.
fn install_mock_notifier() -> &'static MockNotifier {
    let mock: &'static MockNotifier = Box::leak(Box::new(MockNotifier::new()));
    struct Shim {
        inner: &'static MockNotifier,
    }
    impl NotifierBackend for Shim {
        fn send(&self, url: &str, ev: &NotifyEvent) -> anyhow::Result<()> {
            self.inner.send(url, ev)
        }
    }
    install_notifier(Box::new(Shim { inner: mock }));
    mock
}

/// Write a stub drive script to `dir` that, on invocation, flips the task's
/// `status` and `wrap_log` directly via sqlite3. Returns an absolute path.
fn write_happy_drive_stub(dir: &Path, db_path: &Path) -> PathBuf {
    let stub = dir.join("drive_stub.sh");
    let log = dir.join("drive_stub.log");
    let body = format!(
        "#!/usr/bin/env bash\n\
         exec >>{log} 2>&1\n\
         set -x\n\
         DISPLAY_ID=\"$1\"\n\
         echo \"stub invoked for $DISPLAY_ID at $(date -Iseconds)\"\n\
         sqlite3 -cmd \".timeout 10000\" {db} \"UPDATE tasks SET status='in_review', \
           wrap_log='[{{\\\"summary\\\":\\\"stub-drive ok\\\"}}]', \
           updated_at='2026-05-04T00:00:01Z' WHERE display_id='$DISPLAY_ID'\"\n\
         echo \"stub UPDATE rc=$?\"\n\
         # T049: drive_loop closes the auto-drive dispatch_lock on first\n\
         # successful submit. The stub stands in for drive_loop, so close\n\
         # the lock here to mirror that contract.\n\
         sqlite3 -cmd \".timeout 10000\" {db} \"UPDATE dispatch_locks \
           SET last_status='ok', finished_at='2026-05-04T00:00:01Z', \
               attempts=attempts+1 \
           WHERE store='tasks' AND display_id='$DISPLAY_ID' \
             AND agent_name='auto-drive' AND finished_at IS NULL\"\n\
         echo \"stub LOCK rc=$?\"\n",
        db = db_path.display(),
        log = log.display()
    );
    std::fs::write(&stub, body).unwrap();
    let mut perms = std::fs::metadata(&stub).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&stub, perms).unwrap();
    stub
}

/// Repeatedly call `poll_once` until `predicate` holds or `timeout` elapses.
fn poll_until<F: Fn(&Connection) -> bool>(
    conn: &Connection,
    agents: &stores::flow::AgentsYaml,
    policies: &PoliciesYaml,
    cfg: &Path,
    claimer: &str,
    predicate: F,
    timeout: Duration,
) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let _ = poll_once(conn, agents, policies, cfg, claimer, "").unwrap();
        if predicate(conn) {
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    panic!("poll_until: predicate not satisfied within {:?}", timeout);
}

fn insert_task_history(
    conn: &Connection,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
    verb: &str,
) {
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, ?2, ?3, ?4, ?5, 'framework', '2026-05-04T00:00:02Z')",
        rusqlite::params![row_id, display_id, from, to, verb],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// AC7.1 / AC7.2: happy-path full chain (ratify → promote → scaffold → drive
// → in_review) within ~5s of scaffold completion.
// ---------------------------------------------------------------------------

#[test]
fn ratify_promote_scaffold_drive_happy_path() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (tmp, db_path, cfg_path) = setup_repo_with_scaffold();
    let conn = Connection::open(&db_path).unwrap();

    let stub = write_happy_drive_stub(tmp.path(), &db_path);
    // The framework invokes the stub via `sh -c "<cmd> \"$@\"" auto-drive-stub <id>`.
    // Setting STORES_DRIVE_CMD to the script's path means $1 is the display_id.
    std::env::set_var("STORES_DRIVE_CMD", stub.to_string_lossy().to_string());

    let obs_id = file_ratified_observation(&conn, "T022 P7 happy path candidate");
    let obs_status: String = conn
        .query_row(
            "SELECT status FROM observations WHERE display_id=?1",
            rusqlite::params![obs_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(obs_status, "ready", "obs must auto-ratify to 'ready'");

    let agents_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("tests/fixtures/agents.yaml must parse");
    let policies = PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    };

    let started = Instant::now();
    let stub_log = tmp.path().join("drive_stub.log");
    let predicate = |c: &Connection| {
        let s: Option<String> = c
            .query_row("SELECT status FROM tasks LIMIT 1", [], |r| r.get(0))
            .ok();
        s.as_deref() == Some("in_review")
    };
    let mut elapsed = Duration::ZERO;
    let mut hit = false;
    let pl_start = Instant::now();
    while pl_start.elapsed() < Duration::from_secs(30) {
        let _ = poll_once(&conn, &agents, &policies, &cfg_path, "t022p7-claimer", "").unwrap();
        if predicate(&conn) {
            hit = true;
            elapsed = pl_start.elapsed();
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    if !hit {
        let log = std::fs::read_to_string(&stub_log).unwrap_or_else(|_| "<no stub log>".into());
        let (status, drive_pid): (Option<String>, Option<i64>) = conn
            .query_row("SELECT status, drive_pid FROM tasks LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap_or((None, None));
        panic!(
            "AC7.1: status never reached in_review (status={:?}, drive_pid={:?}); stub log:\n{}",
            status, drive_pid, log
        );
    }

    // AC7.2: total wall-clock from observation ratify to task in_review < 30s.
    let total = started.elapsed();
    assert!(
        total < Duration::from_secs(30),
        "AC7.2: ratify→in_review must be <30s; took {:?} (poll loop took {:?})",
        total,
        elapsed
    );

    let (task_display, status, drive_pid): (String, String, Option<i64>) = conn
        .query_row(
            "SELECT display_id, status, drive_pid FROM tasks LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "in_review");
    assert!(task_display.starts_with('T'));
    assert!(
        drive_pid.unwrap_or(0) > 0,
        "auto-drive must have recorded a drive_pid"
    );

    // AC7.1: dispatch_locks for auto-drive shows finished_at non-null with
    // last_status='ok' (the spawn returned 0).
    let (finished_at, last_status): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT finished_at, last_status FROM dispatch_locks \
             WHERE agent_name='auto-drive' AND display_id=?1",
            rusqlite::params![task_display],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        finished_at.is_some(),
        "AC7.1: auto-drive lock must have finished_at non-null"
    );
    assert_eq!(
        last_status.as_deref(),
        Some("ok"),
        "AC7.1: auto-drive lock last_status must be 'ok' (spawn returned 0)"
    );

    std::env::remove_var("STORES_DRIVE_CMD");
}

// ---------------------------------------------------------------------------
// T037: ratify → promote → scaffold → drive → accept → deploy-success → resolve.
// The drive is stubbed as above; the human accept + deploy success transitions
// are represented by transition_history rows so the daemon dispatches the new
// auto-resolve subscriber on cargo_installed→schema_migrated.
// ---------------------------------------------------------------------------

#[test]
fn ratify_promote_scaffold_drive_accept_deploy_resolves_observation() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (tmp, db_path, cfg_path) = setup_repo_with_scaffold();
    let conn = Connection::open(&db_path).unwrap();

    let stub = write_happy_drive_stub(tmp.path(), &db_path);
    std::env::set_var("STORES_DRIVE_CMD", stub.to_string_lossy().to_string());

    let obs_id = file_ratified_observation(&conn, "T037 resolve chain candidate");
    let agents_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("tests/fixtures/agents.yaml must parse");
    let policies = PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    };

    poll_until(
        &conn,
        &agents,
        &policies,
        &cfg_path,
        "t037-chain",
        |c| {
            c.query_row("SELECT status FROM tasks LIMIT 1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .as_deref()
                == Some("in_review")
        },
        Duration::from_secs(30),
    );

    let (task_row_id, task_display): (i64, String) = conn
        .query_row("SELECT id, display_id FROM tasks LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let commit = "abc1234def5678";
    let cycles = serde_json::json!([{ "executor": { "commit": commit } }]).to_string();

    conn.execute(
        "UPDATE tasks SET status='schema_migrated', cycles=?1, updated_at='2026-05-04T00:00:02Z' WHERE id=?2",
        rusqlite::params![cycles, task_row_id],
    )
    .unwrap();
    insert_task_history(
        &conn,
        task_row_id,
        &task_display,
        "in_review",
        "accepted",
        "accept",
    );
    insert_task_history(
        &conn,
        task_row_id,
        &task_display,
        "accepted",
        "cargo_installed",
        "mark_cargo_installed",
    );
    insert_task_history(
        &conn,
        task_row_id,
        &task_display,
        "cargo_installed",
        "schema_migrated",
        "mark_schema_migrated",
    );

    let resolver_only = stores::flow::AgentsYaml {
        agents: agents
            .agents
            .iter()
            .filter(|a| a.name == "auto-resolve-observation")
            .cloned()
            .collect(),
        deployment_specialist: None,
    };
    poll_until(
        &conn,
        &resolver_only,
        &policies,
        &cfg_path,
        "t037-resolve",
        |c| {
            c.query_row(
                "SELECT status FROM observations WHERE display_id=?1",
                rusqlite::params![obs_id.clone()],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .as_deref()
                == Some("resolved")
        },
        Duration::from_secs(5),
    );

    let (obs_status, resolution): (String, String) = conn
        .query_row(
            "SELECT status, resolution FROM observations WHERE display_id=?1",
            rusqlite::params![obs_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(obs_status, "resolved");
    assert_eq!(resolution, commit);

    std::env::remove_var("STORES_DRIVE_CMD");
}

// ---------------------------------------------------------------------------
// AC7.3: failure path — drive subprocess dies without producing wrap.
// `sweep_drive_watchdog` flips the row to blocked / drive_failed, files an
// observation, and ntfy fires.
//
// We exercise the watchdog directly with a pre-built "drive in flight" state
// (open lock + dead drive_pid) — the same input shape sweep_drive_watchdog
// sees in production after a drive subprocess crashes mid-cycle.
// ---------------------------------------------------------------------------

#[test]
fn drive_failure_watchdog_flips_blocked() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (_tmp, db_path, cfg_path) = setup_repo_with_scaffold();
    let conn = Connection::open(&db_path).unwrap();
    let mock = install_mock_notifier();

    // Insert a tasks row mid-drive: status='executing', drive_pid pointing at
    // a high unallocated PID, and an OPEN auto-drive lock. This is the state
    // the watchdog reconciles after a real drive subprocess dies before
    // submit-wrap.
    let now = "2026-05-04T00:00:00Z";
    let dead_pid: i64 = 0x7fff_fffe;
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, drive_pid, created_at, updated_at, created_by, updated_by) \
         VALUES ('T900', 'executing', 't', 't', 'feat/t', '/tmp/none', ?1, ?2, ?3, ?3, 'framework', 'framework')",
        rusqlite::params![contract, dead_pid, now],
    ).unwrap();
    let row_id: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T900'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by) \
         VALUES ('tasks', ?1, 'T900', 'auto-drive', 1, ?2, 't022p7')",
        rusqlite::params![row_id, now],
    )
    .unwrap();

    // Load the production fixture so user-escalation routes via deployment_specialist.
    let agents_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("agents.yaml must parse");

    let acted =
        stores::flow::builtins::auto_drive::sweep_drive_watchdog(&conn, &agents, &cfg_path, "", "")
            .unwrap();
    assert!(acted >= 1, "watchdog must have acted on the dead drive");

    let (status, reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE display_id='T900'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "blocked", "AC7.3: row must flip to blocked");
    assert_eq!(
        reason.as_deref(),
        Some("drive_failed:silent_zombie_pid_dead"),
        "AC7.3 (T049): blocked_reason carries silent-zombie suffix"
    );

    // Observation filed with task_id back-link to T900.
    let (obs_count, obs_task_id): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(task_id), '') FROM observations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(obs_count, 1, "AC7.3: exactly one observation must be filed");
    assert_eq!(obs_task_id, "T900");

    // Lock closed.
    let finished: Option<String> = conn
        .query_row(
            "SELECT finished_at FROM dispatch_locks WHERE row_id=?1",
            rusqlite::params![row_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(finished.is_some(), "AC7.3: lock must be closed");

    // ntfy MockNotifier captured an event mentioning blocked / T900.
    let evs = mock.events();
    let blocked_evs: Vec<_> = evs
        .iter()
        .filter(|(_, e)| e.row_id == "T900" && e.transition_attempted.contains("blocked"))
        .collect();
    assert_eq!(
        blocked_evs.len(),
        1,
        "AC7.3: exactly one ntfy event with 'blocked' for T900; got: {:?}",
        evs
    );
}

// ---------------------------------------------------------------------------
// AC7.4: idempotency — daemon restart mid-drive does NOT re-spawn auto-drive.
// The dispatch_locks UNIQUE(store,row_id,agent_name) guards against double
// claim, even after the connection is dropped and rebuilt.
// ---------------------------------------------------------------------------

#[test]
fn drive_idempotent_no_double_spawn_after_restart() {
    let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let (tmp, db_path, cfg_path) = setup_repo_with_scaffold();
    let stub = write_happy_drive_stub(tmp.path(), &db_path);
    std::env::set_var("STORES_DRIVE_CMD", stub.to_string_lossy().to_string());

    let conn = Connection::open(&db_path).unwrap();
    let _obs = file_ratified_observation(&conn, "T022 P7 idempotency candidate");

    let agents_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("agents.yaml must parse");
    let policies = PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    };

    // Drive the chain to the point where auto-drive has spawned (and the stub
    // already flipped the row to in_review — happy path).
    poll_until(
        &conn,
        &agents,
        &policies,
        &cfg_path,
        "t022p7-claimer-1",
        |c| {
            c.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
                >= 1
        },
        Duration::from_secs(30),
    );

    let lock_count_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lock_count_before, 1, "expected exactly one auto-drive lock");

    // Restart simulation: drop connection, rebuild from disk, run poll_once.
    drop(conn);
    let conn2 = Connection::open(&db_path).unwrap();
    let _ = poll_once(&conn2, &agents, &policies, &cfg_path, "t022p7-claimer-2", "").unwrap();

    let lock_count_after: i64 = conn2
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        lock_count_after, 1,
        "AC7.4: dispatch_locks UNIQUE(store,row_id,agent_name) must block a \
         second auto-drive claim across restart; got {} locks",
        lock_count_after
    );

    std::env::remove_var("STORES_DRIVE_CMD");
}
