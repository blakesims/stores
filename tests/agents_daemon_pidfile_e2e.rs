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
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
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
        .output()
        .expect("stores agents run spawns")
}

fn stop(project: &Path) -> Output {
    run(project, &["agents", "stop"])
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
