#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn stores_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stores")
}

fn run(project: &Path, args: &[&str]) -> Output {
    Command::new(stores_bin())
        .args(args)
        .current_dir(project)
        .output()
        .expect("stores command spawns")
}

fn init_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = run(tmp.path(), &["init"]);
    assert!(
        out.status.success(),
        "init stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    tmp
}

fn pidfile(project: &Path) -> PathBuf {
    project.join(".stores").join("agents.pid")
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn try_read_pid(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    // Support both legacy bare-PID format and new key=value format.
    for line in text.lines() {
        if let Some(val) = line.strip_prefix("PID=") {
            return val.parse().ok();
        }
    }
    // Legacy: entire trimmed content is the PID.
    text.trim().parse().ok()
}

fn wait_for_pidfile(project: &Path) -> i32 {
    let path = pidfile(project);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            if let Some(pid) = try_read_pid(&path) {
                if pid_alive(pid) {
                    return pid;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for live pidfile {}", path.display());
}

fn wait_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("pid {pid} remained live");
}

fn start_detached(project: &Path, log_name: &str) -> Output {
    let log = project.join(log_name);
    Command::new(stores_bin())
        .args(["agents", "run", "--detach", "--log-file"])
        .arg(log)
        .args(["--poll-interval", "0.1"])
        .current_dir(project)
        // Pin the daemon binary path to the test binary so the stale-binary
        // detector doesn't re-exec into an installed (different-inode) stores
        // binary. Without this, the re-exec'd binary can take >5 s to respond
        // to SIGTERM (blocked inside validate_stale_reexec_candidate), causing
        // `agents stop` to time out non-deterministically.
        .env("STORES_DAEMON_BIN_PATH", stores_bin())
        .output()
        .expect("stores agents run spawns")
}

fn stop(project: &Path) -> Output {
    run(project, &["agents", "stop"])
}

#[allow(dead_code)]
fn stop_force(project: &Path) -> Output {
    run(project, &["agents", "stop", "--force"])
}

#[test]
fn detached_start_writes_pidfile_and_stop_removes_it() {
    let tmp = init_project();
    let out = start_detached(tmp.path(), "daemon.log");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pid = wait_for_pidfile(tmp.path());
    assert!(pid > 0);

    let stopped = stop(tmp.path());
    assert!(
        stopped.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    wait_dead(pid);
    assert!(!pidfile(tmp.path()).exists(), "pidfile removed");
}

#[test]
fn double_start_rejects_live_pid_and_original_survives() {
    let tmp = init_project();
    let first = start_detached(tmp.path(), "daemon1.log");
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let pid = wait_for_pidfile(tmp.path());

    let second = start_detached(tmp.path(), "daemon2.log");
    assert!(
        !second.status.success(),
        "second start unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("already running") || stderr.contains("live pid"),
        "stderr={stderr}"
    );
    assert!(pid_alive(pid), "original daemon remains live");

    let stopped = stop(tmp.path());
    assert!(
        stopped.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    wait_dead(pid);
}

#[test]
fn stale_pidfile_warns_removes_and_proceeds() {
    let tmp = init_project();
    std::fs::write(pidfile(tmp.path()), "2147483600\n").unwrap();

    let out = start_detached(tmp.path(), "daemon.log");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stale") && stderr.contains("removing"),
        "stderr={stderr}"
    );
    let pid = wait_for_pidfile(tmp.path());
    assert_ne!(pid, 2147483600);

    let stopped = stop(tmp.path());
    assert!(
        stopped.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    wait_dead(pid);
}

#[test]
fn stop_missing_pidfile_fails_clearly() {
    let tmp = init_project();
    let out = stop(tmp.path());
    assert!(!out.status.success(), "stop unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("pid file missing") || stderr.contains("pid file"),
        "stderr={stderr}"
    );
}

#[test]
fn stop_succeeds_when_process_exits_during_wait() {
    let tmp = init_project();
    let spawned = Command::new("sh")
        .arg("-c")
        .arg("(trap 'exit 0' TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn background shell");
    assert!(spawned.status.success());
    let pid: i32 = String::from_utf8_lossy(&spawned.stdout)
        .trim()
        .parse()
        .expect("background pid parses");
    assert!(pid_alive(pid));
    std::fs::write(pidfile(tmp.path()), format!("{pid}\n")).unwrap();

    let out = stop(tmp.path());
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_dead(pid);
    assert!(!pidfile(tmp.path()).exists(), "pidfile removed");
}

/// Pidfile identity mismatch: write a pidfile with a bogus start_time; stop must
/// detect PID reuse, remove the pidfile, and exit cleanly WITHOUT signaling.
///
/// Strategy: spawn a real process, write a pidfile with its PID but a
/// deliberately wrong start_time, then run `agents stop`.  The identity check
/// should detect the mismatch and refuse to SIGTERM the process.
#[cfg(target_os = "linux")]
#[test]
fn stop_detects_pidfile_identity_mismatch_and_refuses_to_signal() {
    let tmp = init_project();

    // Spawn a background process that ignores SIGTERM so we'd notice if it were
    // killed unexpectedly.
    let spawned = Command::new("sh")
        .arg("-c")
        .arg("(trap '' TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn background process");
    assert!(spawned.status.success());
    let pid: i32 = String::from_utf8_lossy(&spawned.stdout)
        .trim()
        .parse()
        .expect("background pid parses");
    assert!(pid_alive(pid), "background process must be alive");

    // Write a pidfile with the real PID but a bogus start_time (0xdeadbeef) so
    // the identity check will fail.
    let pidfile_content = format!("PID={pid}\nSTART_TIME_NS=3735928559\nEXE=/bogus/path\nCWD=/bogus/cwd\n");
    std::fs::write(pidfile(tmp.path()), &pidfile_content).unwrap();

    let out = stop(tmp.path());

    // stop must exit non-zero (mismatch → refused to signal) and emit a clear message.
    assert!(
        !out.status.success(),
        "stop must fail on identity mismatch; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stale pidfile") || stderr.contains("PID reuse") || stderr.contains("no signal"),
        "error must describe identity mismatch / stale pidfile; stderr={stderr}"
    );

    // Pidfile must be removed.
    assert!(!pidfile(tmp.path()).exists(), "pidfile must be removed after mismatch detection");

    // The background process must still be alive (we did NOT signal it).
    assert!(pid_alive(pid), "background process must remain alive (was not signaled)");

    // Clean up.
    unsafe { libc::kill(pid, libc::SIGKILL); }
}

/// Pidfile identity match: a real daemon's pidfile has a correct start_time, so
/// `agents stop` proceeds normally with SIGTERM.
/// This exercises the identity-match path end-to-end.
#[test]
fn stop_with_identity_match_proceeds_normally() {
    let tmp = init_project();
    let out = start_detached(tmp.path(), "daemon-identity.log");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pid = wait_for_pidfile(tmp.path());
    assert!(pid > 0);

    // Verify pidfile contains key=value format with PID and START_TIME_NS.
    let content = std::fs::read_to_string(pidfile(tmp.path())).unwrap();
    assert!(content.contains("PID="), "pidfile must use key=value format; content={content:?}");
    assert!(content.contains("START_TIME_NS="), "pidfile must contain START_TIME_NS; content={content:?}");
    assert!(content.contains("EXE="), "pidfile must contain EXE; content={content:?}");
    assert!(content.contains("CWD="), "pidfile must contain CWD; content={content:?}");

    let stopped = stop(tmp.path());
    assert!(
        stopped.status.success(),
        "stop with identity match must succeed; stderr={}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    wait_dead(pid);
    assert!(!pidfile(tmp.path()).exists(), "pidfile removed");
}

/// `agents stop` (no --force) on a non-responsive daemon: must error after
/// timeout without escalating to SIGKILL (daemon still alive after stop fails).
#[test]
fn stop_without_force_times_out_daemon_stays_alive() {
    let tmp = init_project();

    // Spawn a process that ignores SIGTERM.
    let spawned = Command::new("sh")
        .arg("-c")
        .arg("(trap '' TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn SIGTERM-ignoring process");
    assert!(spawned.status.success());
    let pid: i32 = String::from_utf8_lossy(&spawned.stdout)
        .trim()
        .parse()
        .expect("background pid parses");
    assert!(pid_alive(pid));

    // Write a bare-PID legacy pidfile (start_time=0 → skip identity check, SIGTERM proceeds).
    std::fs::write(pidfile(tmp.path()), format!("{pid}\n")).unwrap();

    let out = Command::new(stores_bin())
        .args(["agents", "stop"])
        .current_dir(tmp.path())
        .env("STORES_AGENTS_STOP_TIMEOUT_SEC", "1")
        .output()
        .expect("stores agents stop");

    // Must fail (timeout, no escalation).
    assert!(!out.status.success(), "stop without --force must fail on timeout; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("timed out") || stderr.contains("timeout"),
        "stderr must mention timeout; stderr={stderr}"
    );

    // Daemon must still be alive (no SIGKILL was sent).
    assert!(pid_alive(pid), "daemon must remain alive after graceful-only stop timeout");

    // Clean up.
    unsafe { libc::kill(pid, libc::SIGKILL); }
}

/// `agents stop --force` on a non-responsive daemon: must escalate to SIGKILL
/// and daemon must be dead afterward.
#[test]
fn stop_with_force_kills_non_responsive_daemon() {
    let tmp = init_project();

    // Spawn a process that ignores SIGTERM.
    let spawned = Command::new("sh")
        .arg("-c")
        .arg("(trap '' TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn SIGTERM-ignoring process");
    assert!(spawned.status.success());
    let pid: i32 = String::from_utf8_lossy(&spawned.stdout)
        .trim()
        .parse()
        .expect("background pid parses");
    assert!(pid_alive(pid));

    // Write a bare-PID legacy pidfile (start_time=0 → skip identity check).
    std::fs::write(pidfile(tmp.path()), format!("{pid}\n")).unwrap();

    let out = Command::new(stores_bin())
        .args(["agents", "stop", "--force"])
        .current_dir(tmp.path())
        .env("STORES_AGENTS_STOP_TIMEOUT_SEC", "1")
        .output()
        .expect("stores agents stop --force");

    assert!(
        out.status.success(),
        "stop --force must succeed (SIGKILL escalation); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("force-killed") || stdout.contains("stopped"),
        "stdout must confirm kill; stdout={stdout}"
    );

    // Daemon must be dead.
    wait_dead(pid);
    assert!(!pidfile(tmp.path()).exists(), "pidfile removed after force-kill");
}
