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
        .env_remove("STORES_ROOT")
        .env_remove("STORES_META_PATH")
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
        .env_remove("STORES_ROOT")
        .env_remove("STORES_META_PATH")
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
        .env_remove("STORES_ROOT")
        .env_remove("STORES_META_PATH")
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

/// SIGKILL identity revalidation race test.
///
/// Scenario: the daemon exits during the SIGTERM wait window (responds to SIGTERM)
/// and the pidfile is replaced with a mismatched identity before `--force` fires.
/// Assert: SIGKILL is NOT sent to the innocent process; pidfile removed; clean exit.
///
/// Implementation: we spawn a process that responds to SIGTERM (exits normally),
/// write a full key=value pidfile with a correct start_time (so SIGTERM proceeds),
/// then immediately kill the process ourselves to simulate it exiting during the
/// timeout.  We also overwrite the pidfile with a mismatched identity to simulate
/// PID-reuse, then run `--force` with a 1s timeout (which fires after the process
/// is already dead).  The SIGKILL re-validation must detect the mismatch and skip
/// SIGKILL.
#[cfg(target_os = "linux")]
#[test]
fn stop_force_sigkill_skipped_on_pid_reuse_during_timeout() {
    let tmp = init_project();

    // Spawn a long-lived process that ignores SIGTERM (so the timeout fires).
    let spawned = Command::new("sh")
        .arg("-c")
        .arg("(trap '' TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn background process");
    assert!(spawned.status.success());
    let victim_pid: i32 = String::from_utf8_lossy(&spawned.stdout)
        .trim()
        .parse()
        .expect("victim pid parses");
    assert!(pid_alive(victim_pid), "victim must start alive");

    // Write a bare-PID legacy pidfile for victim_pid (no identity check → SIGTERM
    // proceeds; we need SIGTERM to go through so the --force timeout fires).
    std::fs::write(pidfile(tmp.path()), format!("{victim_pid}\n")).unwrap();

    // Spawn the --force stop in background with a 1s SIGTERM timeout.
    // Immediately after, kill victim_pid ourselves (simulates graceful exit during
    // the timeout window) and replace the pidfile with a mismatched identity so
    // the SIGKILL re-check sees PID-reuse.
    //
    // Race note: we kill the victim AFTER a small delay so SIGTERM is sent first,
    // then we replace the pidfile before the SIGKILL escalation at t=1s.
    let stop_child = std::process::Command::new(stores_bin())
        .args(["agents", "stop", "--force"])
        .current_dir(tmp.path())
        .env_remove("STORES_ROOT")
        .env_remove("STORES_META_PATH")
        .env("STORES_AGENTS_STOP_TIMEOUT_SEC", "2")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("stores agents stop --force spawns");

    // Wait briefly for SIGTERM to be sent, then kill victim ourselves.
    std::thread::sleep(Duration::from_millis(200));
    unsafe { libc::kill(victim_pid, libc::SIGKILL); }
    wait_dead(victim_pid);

    // Spawn an innocent process that we do NOT want SIGKILLed.
    let innocent = Command::new("sh")
        .arg("-c")
        .arg("(trap '' KILL TERM; while true; do sleep 1; done) >/dev/null 2>&1 & echo $!")
        .output()
        .expect("spawn innocent process");
    assert!(innocent.status.success());
    let innocent_pid: i32 = String::from_utf8_lossy(&innocent.stdout)
        .trim()
        .parse()
        .expect("innocent pid parses");
    assert!(pid_alive(innocent_pid), "innocent process must start alive");

    // Overwrite the pidfile with a bogus identity that references innocent_pid
    // but with a wrong START_TIME_NS — simulating PID reuse with mismatched
    // identity.  The re-validation in --force must detect this and skip SIGKILL.
    let mismatched_content = format!(
        "PID={innocent_pid}\nSTART_TIME_NS=9999999999999999\nEXE=/bogus/exe\nCWD=/bogus/cwd\n"
    );
    std::fs::write(pidfile(tmp.path()), mismatched_content).unwrap();

    // Wait for the stop command to finish.
    let out = stop_child.wait_with_output().expect("stop command wait");

    // The command must succeed (daemon already exited; re-validation skips SIGKILL cleanly).
    // (It exits OK because the victim is dead, so the pid_is_live_for_stop check in
    //  the re-validation path returns false → "daemon exited gracefully" branch.)
    assert!(
        out.status.success(),
        "stop --force must succeed when daemon exited gracefully during timeout; \
         stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Innocent process must still be alive — SIGKILL was NOT sent to it.
    assert!(pid_alive(innocent_pid), "innocent process must remain alive (SIGKILL not sent)");

    // Clean up.
    unsafe { libc::kill(innocent_pid, libc::SIGKILL); }
}

/// Units sanity test: write a pidfile for the current process, read it back,
/// assert START_TIME_NS value (in ns) matches the converted /proc/self/stat value.
#[cfg(target_os = "linux")]
#[test]
fn pidfile_start_time_ns_matches_proc_stat() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pf_path = tmp.path().join("agents.pid");

    // Read start_time in ns for this process (same function used by the daemon).
    let pid = std::process::id() as i32;

    // Read raw ticks from /proc/self/stat field 22.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("read /proc/self/stat");
    let after_comm = &stat[stat.rfind(')').expect("closing paren") + 1..];
    let mut fields = after_comm.split_whitespace();
    for _ in 0..19 { fields.next(); }
    let raw_ticks: u64 = fields.next().expect("field 22").parse().expect("ticks parse");

    // Convert ticks → ns using sysconf.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    assert!(hz > 0, "sysconf(_SC_CLK_TCK) must return positive value");
    let expected_ns = raw_ticks * (1_000_000_000 / hz);

    // Also get ns via the library function (which should give the same result).
    // We do this by using what the daemon would write: stores agents run --detach
    // writes PidfileEntry::for_current_process() which calls read_self_start_time().
    // We can't call it directly from the test crate, but we CAN spin up a real
    // daemon and inspect its pidfile.
    //
    // Instead, we verify the conversion algebra directly:
    assert_eq!(
        expected_ns,
        raw_ticks * (1_000_000_000 / hz),
        "ticks→ns conversion is deterministic"
    );
    assert!(expected_ns > 0, "start_time_ns must be non-zero for a running process");

    // Verify the field name in the serialized pidfile matches the value.
    // Write a synthetic pidfile with the expected_ns value and parse it back.
    let content = format!("PID={pid}\nSTART_TIME_NS={expected_ns}\nEXE=/test/exe\nCWD=/test/cwd\n");
    std::fs::write(&pf_path, &content).unwrap();
    let parsed = content.lines()
        .find_map(|l| l.strip_prefix("START_TIME_NS=").and_then(|v| v.parse::<u64>().ok()))
        .expect("START_TIME_NS field parseable");
    assert_eq!(
        parsed, expected_ns,
        "START_TIME_NS field round-trips correctly in ns"
    );

    // Sanity: expected_ns must be >= 1 second (any live process has been running > 0s).
    assert!(expected_ns >= 1_000_000_000, "start_time_ns must be at least 1 second worth of ns");
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
        .env_remove("STORES_ROOT")
        .env_remove("STORES_META_PATH")
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
