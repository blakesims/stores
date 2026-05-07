use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

/// Default number of seconds to wait for the daemon to exit after SIGTERM.
/// Override via `STORES_AGENTS_STOP_TIMEOUT_SEC` environment variable.
const DEFAULT_STOP_TIMEOUT_SECS: u64 = 5;
const STOP_POLL: Duration = Duration::from_millis(50);

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

pub fn run_stop() -> Result<()> {
    let pidfile = crate::paths::agents_pid_path()?;
    if !pidfile.exists() {
        bail!(
            "agents daemon pid file missing: {} (is stores agents run --detach running for this project?)",
            pidfile.display()
        );
    }

    let pid = crate::handlers::agents_run::read_pidfile(&pidfile)
        .with_context(|| format!("invalid agents daemon pid file {}", pidfile.display()))?;
    if !crate::handlers::agents_run::pid_is_alive(pid) {
        bail!(
            "agents daemon pid {pid} from {} is not live; remove the stale pid file or run again",
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
            match std::fs::remove_file(&pidfile) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("removing agents daemon pid file {}", pidfile.display())
                    })
                }
            }
            println!("stopped agents daemon pid {pid}");
            return Ok(());
        }
        std::thread::sleep(STOP_POLL);
    }

    bail!(
        "timed out after {}s waiting for agents daemon pid {pid} to exit after SIGTERM",
        timeout.as_secs()
    );
}
