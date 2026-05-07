use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_POLL: Duration = Duration::from_millis(50);

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

    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !crate::handlers::agents_run::pid_is_alive(pid) {
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
        STOP_TIMEOUT.as_secs()
    );
}
