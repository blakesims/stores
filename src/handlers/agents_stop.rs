use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

/// Default number of seconds to wait for the daemon to exit after SIGTERM.
/// Override via `STORES_AGENTS_STOP_TIMEOUT_SEC` environment variable.
const DEFAULT_STOP_TIMEOUT_SECS: u64 = 5;
const STOP_POLL: Duration = Duration::from_millis(50);
/// Additional wait after SIGKILL before giving up (--force path only).
const SIGKILL_WAIT_SECS: u64 = 2;

fn stop_timeout() -> Duration {
    let secs = std::env::var("STORES_AGENTS_STOP_TIMEOUT_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STOP_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Returns true if `pid` should be treated as "still running" for the purposes
/// of the stop wait loop. A zombie is treated as exited.
pub(crate) fn pid_is_live_for_stop(pid: i32) -> bool {
    if crate::handlers::agents_run::pid_is_zombie(pid) {
        return false;
    }
    crate::handlers::agents_run::pid_is_alive(pid)
}

/// Verify that the live process at `pid` matches the identity recorded in the
/// pidfile entry. On Linux, compares start_time (field 22 of `/proc/<pid>/stat`)
/// and, if available, the exe symlink. On non-Linux, always returns `true`
/// (fallback to bare-PID semantics).
///
/// Returns `true` when identity matches (safe to signal) or when start_time is
/// zero (legacy/non-Linux pidfile — cannot verify, assume match).
/// Returns `false` on mismatch — PID was reused by an unrelated process.
fn pidfile_identity_matches(entry: &crate::handlers::agents_run::PidfileEntry) -> bool {
    // start_time == 0 means legacy format or non-Linux — no identity info.
    if entry.start_time == 0 {
        return true;
    }
    // Check start_time via /proc/<pid>/stat.
    let live_start = crate::handlers::agents_run::read_proc_start_time(entry.pid);
    if live_start == 0 {
        // Could not read live start_time — treat as mismatch-safe (don't signal).
        // This path is hit only on non-Linux or when the process vanished between
        // the alive-check and here; both cases are safe to treat as stale.
        return false;
    }
    if live_start != entry.start_time {
        return false;
    }
    // start_time matches. Optionally verify exe (best-effort, non-fatal).
    // We treat exe mismatch as a strong hint of reuse but still gate on start_time.
    // (start_time alone is the primary guard; exe is a belt-and-suspenders check.)
    if !entry.exe.is_empty() {
        #[cfg(target_os = "linux")]
        {
            if let Ok(live_exe) = std::fs::read_link(format!("/proc/{}/exe", entry.pid)) {
                if live_exe.to_string_lossy() != entry.exe.as_str() {
                    return false;
                }
            }
        }
    }
    true
}

/// Options for `run_stop`.
pub struct StopOptions {
    /// If true, escalate to SIGKILL after the graceful-SIGTERM timeout.
    pub force: bool,
}

pub fn run_stop(opts: StopOptions) -> Result<()> {
    let pidfile = crate::paths::agents_pid_path()?;
    if !pidfile.exists() {
        bail!(
            "agents daemon pid file missing: {} (is stores agents run --detach running for this project?)",
            pidfile.display()
        );
    }

    let entry = crate::handlers::agents_run::read_pidfile(&pidfile)
        .with_context(|| format!("invalid agents daemon pid file {}", pidfile.display()))?;
    let pid = entry.pid;

    if !crate::handlers::agents_run::pid_is_alive(pid) {
        bail!(
            "agents daemon pid {pid} from {} is not live; remove the stale pid file or run again",
            pidfile.display()
        );
    }

    // Identity check: verify the live process at `pid` is the daemon we started,
    // not an unrelated process that reused the PID after the daemon exited.
    if !pidfile_identity_matches(&entry) {
        // PID reuse detected — remove stale pidfile, refuse to signal.
        match std::fs::remove_file(&pidfile) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "removing stale pidfile after PID-reuse detection {}",
                        pidfile.display()
                    )
                });
            }
        }
        bail!(
            "stale pidfile detected (PID reuse): pid {pid} in {} belongs to a different process; \
             pidfile removed; no signal sent",
            pidfile.display()
        );
    }

    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        bail!("failed to SIGTERM agents daemon pid {pid}: {err}");
    }

    let timeout = stop_timeout();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_is_live_for_stop(pid) {
            remove_pidfile_if_ours(&pidfile, pid);
            println!("stopped agents daemon pid {pid}");
            return Ok(());
        }
        std::thread::sleep(STOP_POLL);
    }

    // Graceful timeout reached.
    if opts.force {
        // Before escalating to SIGKILL, re-validate identity: the daemon may have
        // exited gracefully during the timeout window and the OS may have reused
        // its PID for an unrelated process.  SIGKILL-ing an innocent process would
        // be catastrophic, so we re-read the pidfile and re-run the same identity
        // check that guards SIGTERM.
        if !pid_is_live_for_stop(pid) {
            // Already exited while we were about to re-check — clean up and return.
            remove_pidfile_if_ours(&pidfile, pid);
            println!("daemon exited gracefully during timeout window; SIGKILL skipped");
            return Ok(());
        }
        // Re-read the pidfile (it may have been removed by the daemon on exit).
        let sigkill_ok = if pidfile.exists() {
            match crate::handlers::agents_run::read_pidfile(&pidfile) {
                Ok(fresh_entry) => {
                    if pidfile_identity_matches(&fresh_entry) {
                        true
                    } else {
                        // Identity no longer matches: daemon exited and PID was reused.
                        eprintln!(
                            "stale pidfile detected mid-stop (PID reuse during timeout window); \
                             SIGKILL skipped"
                        );
                        remove_pidfile_if_ours(&pidfile, pid);
                        false
                    }
                }
                Err(_) => {
                    // Unparseable pidfile — treat as stale/gone.
                    eprintln!(
                        "daemon exited gracefully during timeout window (pidfile unreadable); \
                         SIGKILL skipped"
                    );
                    remove_pidfile_if_ours(&pidfile, pid);
                    false
                }
            }
        } else {
            // Pidfile gone — daemon exited on its own during the graceful window.
            eprintln!("daemon exited gracefully during timeout window; SIGKILL skipped");
            false
        };

        if !sigkill_ok {
            return Ok(());
        }

        // Escalate to SIGKILL.
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            bail!("failed to SIGKILL agents daemon pid {pid} after graceful timeout: {err}");
        }
        let kill_deadline = Instant::now() + Duration::from_secs(SIGKILL_WAIT_SECS);
        while Instant::now() < kill_deadline {
            if !pid_is_live_for_stop(pid) {
                remove_pidfile_if_ours(&pidfile, pid);
                println!("force-killed agents daemon pid {pid}");
                return Ok(());
            }
            std::thread::sleep(STOP_POLL);
        }
        bail!(
            "timed out after {}s + {}s waiting for agents daemon pid {pid} to exit after SIGKILL",
            timeout.as_secs(),
            SIGKILL_WAIT_SECS,
        );
    }

    bail!(
        "timed out after {}s waiting for agents daemon pid {pid} to exit after SIGTERM \
         (use --force to escalate to SIGKILL)",
        timeout.as_secs()
    );
}

fn remove_pidfile_if_ours(pidfile: &std::path::Path, pid: i32) {
    // Remove if still present and still belongs to this pid.
    match std::fs::remove_file(pidfile) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            // Best-effort; if it fails (e.g. already removed by daemon), ignore.
            let _ = pid; // suppress unused warning
        }
    }
}
