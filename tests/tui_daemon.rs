//! Integration: liveness probe reads a fixture pidfile and returns Live/Dead.

use std::io::Write;
use stores::tui::daemon::{liveness, Liveness};

fn write_pid_to_tmp(pid: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(pid.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn liveness_live_with_self_pid() {
    let pid = std::process::id();
    let f = write_pid_to_tmp(&pid.to_string());
    assert_eq!(liveness(f.path()), Liveness::Live { pid });
}

#[test]
fn liveness_dead_with_far_future_pid() {
    let f = write_pid_to_tmp("2147483600");
    assert_eq!(liveness(f.path()), Liveness::Dead);
}

#[test]
fn liveness_dead_when_pidfile_missing() {
    let path = std::env::temp_dir().join("stores-tui-daemon-no-such-file.pid");
    let _ = std::fs::remove_file(&path);
    assert_eq!(liveness(&path), Liveness::Dead);
}

/// AC3.4: e2e — pressing D starts the daemon. Guarded with #[ignore]: requires
/// the `stores` binary on PATH and writes to `.stores/`. Run manually with
/// `cargo test --test tui_daemon -- --ignored start_detached_writes_pidfile`.
#[test]
#[ignore]
fn start_detached_writes_pidfile() {
    use stores::tui::daemon::{pidfile_path, start_detached};
    let prior = pidfile_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok());
    let new_pid = start_detached().expect("start_detached succeeds");
    let written = std::fs::read_to_string(pidfile_path().unwrap())
        .expect("pidfile written");
    assert_eq!(written.trim(), new_pid.to_string());
    if let Some(prev) = prior {
        assert_ne!(prev.trim(), new_pid.to_string(), "pid is fresh");
    }
}
