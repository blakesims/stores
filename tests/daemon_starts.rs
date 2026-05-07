use rusqlite::Connection;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn stores_bin() -> String {
    std::env::var("CARGO_BIN_EXE_stores").unwrap_or_else(|_| {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        path.push("stores");
        path.to_string_lossy().to_string()
    })
}

#[test]
fn daemon_starts_schema_parses_as_bundled_store() {
    let yaml = stores::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "daemon_starts")
        .map(|(_, yaml)| *yaml)
        .expect("daemon_starts bundled schema");
    let schema = stores::schema::Schema::from_yaml(yaml).unwrap();
    assert_eq!(schema.name, "daemon_starts");

    let fields: std::collections::BTreeMap<_, _> = schema
        .fields
        .iter()
        .map(|f| (f.name.as_str(), &f.ty))
        .collect();
    for name in [
        "pid",
        "started_at",
        "binary_path",
        "binary_version",
        "git_sha",
        "argv",
        "log_file",
        "cwd",
    ] {
        assert!(fields.contains_key(name), "missing field {name}");
    }
    assert!(
        !fields.contains_key("daemon_epoch"),
        "daemon_epoch must not be in daemon_starts schema"
    );
}

#[test]
fn db_open_and_migrate_create_daemon_starts_table() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = stores_bin();
    let init = Command::new(&bin).current_dir(tmp.path()).arg("init").output().unwrap();
    assert!(init.status.success(), "init stderr: {}", String::from_utf8_lossy(&init.stderr));
    let migrate = Command::new(&bin)
        .current_dir(tmp.path())
        .args(["migrate", "--apply"])
        .output()
        .unwrap();
    assert!(migrate.status.success(), "migrate stderr: {}", String::from_utf8_lossy(&migrate.stderr));

    let conn = Connection::open(tmp.path().join(".stores/db.sqlite")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daemon_starts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn daemon_starts_startup_inserts_exactly_one_audit_row_with_filtered_argv() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = stores_bin();
    let init = Command::new(&bin).current_dir(tmp.path()).arg("init").output().unwrap();
    assert!(init.status.success(), "init stderr: {}", String::from_utf8_lossy(&init.stderr));

    let run = Command::new(&bin)
        .current_dir(tmp.path())
        .args([
            "--approve-token",
            "super-secret-approve-value",
            "agents",
            "run",
            "--once",
            "--poll-interval",
            "0.05",
            "--log-file",
            "daemon.log",
        ])
        .output()
        .unwrap();
    assert!(run.status.success(), "daemon stderr: {}", String::from_utf8_lossy(&run.stderr));

    let conn = Connection::open(tmp.path().join(".stores/db.sqlite")).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM daemon_starts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);

    let (binary_path, binary_version, git_sha, started_at, pid, argv, log_file, cwd): (
        String,
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        String,
    ) = conn
        .query_row(
            "SELECT binary_path, binary_version, git_sha, started_at, pid, argv, log_file, cwd FROM daemon_starts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .unwrap();
    assert!(!binary_path.is_empty());
    assert_eq!(binary_version, env!("CARGO_PKG_VERSION"));
    assert!(!git_sha.is_empty());
    assert!(started_at.ends_with('Z'), "started_at={started_at}");
    assert!(pid > 0);
    assert_eq!(log_file.as_deref(), Some("daemon.log"));
    assert_eq!(cwd, tmp.path().display().to_string());
    assert!(!argv.contains("--approve-token"), "argv={argv}");
    assert!(!argv.contains("super-secret-approve-value"), "argv={argv}");
    assert!(argv.contains("agents"), "argv={argv}");
    assert!(argv.contains("run"), "argv={argv}");
    assert!(argv.contains("--poll-interval"), "argv={argv}");
}

/// Concurrency test: two threads simultaneously insert into daemon_starts using
/// the unique-pending-placeholder pattern. Both must succeed AND get distinct
/// display_ids (no UNIQUE constraint collision on the placeholder, no aliased
/// D### from a MAX(id)+1 race).
///
/// SQLite serializes writes so we use PRAGMA busy_timeout to let the second
/// writer wait rather than immediately fail with SQLITE_BUSY. The key property
/// under test is that unique-pending placeholders never collide even when two
/// threads race, and that final display_ids are always distinct D### values.
#[test]
fn daemon_starts_concurrent_inserts_get_distinct_display_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");

    // Bootstrap schema directly — no `stores init` binary needed.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(stores::codegen::ddl::SUBSTRATE_DDL).unwrap();
    }

    // Shared counter that mimics DAEMON_START_SEQ; each thread gets its own
    // monotonically increasing value so placeholders are process+seq unique.
    let counter = Arc::new(AtomicU64::new(0));

    let db1 = db_path.clone();
    let db2 = db_path.clone();
    let ctr1 = Arc::clone(&counter);
    let ctr2 = Arc::clone(&counter);

    let pid = std::process::id();

    let make_insert = |db_path: std::path::PathBuf, pid: u32, ctr: Arc<AtomicU64>| {
        move || -> String {
            let conn = Connection::open(&db_path).unwrap();
            // Give the other thread up to 2 s to release any write lock.
            conn.execute_batch("PRAGMA busy_timeout=2000; PRAGMA journal_mode=WAL;").unwrap();
            let seq = ctr.fetch_add(1, Ordering::Relaxed);
            let pending = format!("__pending_{}_{}", pid, seq);
            conn.execute(
                "INSERT INTO daemon_starts \
                 (display_id, status, created_at, updated_at, created_by, updated_by, \
                  pid, started_at, binary_path, binary_version, git_sha, argv, cwd) \
                 VALUES (?1, 'started', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                         'daemon', 'daemon', 1, '2026-01-01T00:00:00Z', \
                         '/bin/stores', '0.1.0', 'deadbeef', '[]', '/tmp')",
                rusqlite::params![pending],
            ).expect("INSERT into daemon_starts");
            let rowid = conn.last_insert_rowid();
            let display_id = format!("D{rowid:03}");
            conn.execute(
                "UPDATE daemon_starts SET display_id = ?1 WHERE id = ?2",
                rusqlite::params![display_id, rowid],
            ).expect("UPDATE daemon_starts display_id");
            display_id
        }
    };

    let h1 = std::thread::spawn(make_insert(db1, pid, ctr1));
    let h2 = std::thread::spawn(make_insert(db2, pid, ctr2));

    let d1 = h1.join().expect("thread 1 panicked");
    let d2 = h2.join().expect("thread 2 panicked");

    assert_ne!(d1, d2, "concurrent inserts must produce distinct display_ids; got {d1} and {d2}");

    // Confirm both rows are present with distinct D### IDs; no placeholder survived.
    let verify = Connection::open(&db_path).unwrap();
    let stored: Vec<String> = {
        let mut stmt = verify.prepare("SELECT display_id FROM daemon_starts ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert_eq!(stored.len(), 2, "expected 2 rows, got {stored:?}");
    for id in &stored {
        assert!(
            id.starts_with('D') && !id.starts_with("__pending_"),
            "placeholder must not survive; got {id}"
        );
    }
    assert_ne!(stored[0], stored[1], "stored display_ids must differ");
}
