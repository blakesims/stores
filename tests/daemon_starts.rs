use rusqlite::Connection;
use std::process::Command;

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
        "daemon_epoch",
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
fn daemon_startup_inserts_exactly_one_audit_row_with_filtered_argv() {
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
