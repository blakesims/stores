//! Agents-daemon liveness probe + `D`-key spawn helper.
//!
//! The pidfile written by `stores agents run --detach` lives at
//! `.stores/agents.pid`. `liveness()` reads it and verifies the pid is still
//! running (`kill(pid, 0)`); `start_detached()` shells out the same verb the
//! operator would type, returning the new pid.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Liveness {
    Live {
        pid: u32,
    },
    #[default]
    Dead,
}

/// Probe a pidfile path. Missing / empty / unparseable / stale → `Dead`.
pub fn liveness(pidfile: &Path) -> Liveness {
    let text = match std::fs::read_to_string(pidfile) {
        Ok(s) => s,
        Err(_) => return Liveness::Dead,
    };
    let pid: u32 = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => return Liveness::Dead,
    };
    if pid == 0 {
        return Liveness::Dead;
    }
    pid_liveness(pid)
}

/// Probe the project daemon, accepting either the detached pidfile daemon or a
/// foreground `stores agents run` whose cwd is this project root.
pub fn project_liveness(pidfile: &Path, project_root: &Path) -> Liveness {
    match liveness(pidfile) {
        Liveness::Live { pid } => Liveness::Live { pid },
        Liveness::Dead => foreground_liveness(project_root).unwrap_or(Liveness::Dead),
    }
}

#[cfg(target_os = "linux")]
fn foreground_liveness(project_root: &Path) -> Option<Liveness> {
    let want = std::fs::canonicalize(project_root).ok()?;
    for entry in std::fs::read_dir("/proc").ok()? {
        let Ok(entry) = entry else { continue };
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let proc_dir = entry.path();
        let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) else {
            continue;
        };
        if cwd != want {
            continue;
        }
        let Ok(cmdline) = std::fs::read(proc_dir.join("cmdline")) else {
            continue;
        };
        if is_agents_run_cmdline(&cmdline) && matches!(pid_liveness(pid), Liveness::Live { .. }) {
            return Some(Liveness::Live { pid });
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn foreground_liveness(_project_root: &Path) -> Option<Liveness> {
    None
}

fn is_agents_run_cmdline(cmdline: &[u8]) -> bool {
    let args: Vec<OsString> = cmdline
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| OsString::from(String::from_utf8_lossy(s).into_owned()))
        .collect();
    args.windows(3).any(|w| w[1] == "agents" && w[2] == "run")
}

fn pid_liveness(pid: u32) -> Liveness {
    if pid == 0 {
        return Liveness::Dead;
    }
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        Liveness::Live { pid }
    } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::EPERM {
            Liveness::Live { pid }
        } else {
            Liveness::Dead
        }
    }
}

/// The canonical pidfile path under the current `.stores/`.
pub fn pidfile_path() -> Result<PathBuf> {
    crate::paths::agents_pid_path()
}

/// The canonical log path (`.stores/logs/agents-daemon.log`).
pub fn default_log_path() -> Result<PathBuf> {
    Ok(crate::paths::stores_dir()?
        .join("logs")
        .join("agents-daemon.log"))
}

/// Shell out `stores agents run --detach --log-file <log> --poll-interval 2`.
/// Returns the daemon pid printed by the parent process.
pub fn start_detached() -> Result<u32> {
    let log = default_log_path()?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let exe = std::env::current_exe().context("locating current stores binary")?;
    let out = Command::new(exe)
        .args(["agents", "run", "--detach", "--log-file"])
        .arg(&log)
        .args(["--poll-interval", "2"])
        .output()
        .context("spawning stores agents run --detach")?;
    if !out.status.success() {
        anyhow::bail!(
            "stores agents run --detach exited with status {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let pid_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
    pid_str
        .parse::<u32>()
        .with_context(|| format!("parsing daemon pid from stdout: {pid_str:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_pidfile(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn liveness_live_with_running_pid() {
        let pid = std::process::id();
        let f = tmp_pidfile(&pid.to_string());
        assert_eq!(liveness(f.path()), Liveness::Live { pid });
    }

    #[test]
    fn liveness_dead_with_stale_pid() {
        // 1 is init; we cannot signal it without root, but it exists. Use a
        // very large pid that almost certainly doesn't map to a live process.
        // /proc/sys/kernel/pid_max is typically 4_194_304 — pick something
        // beyond that to be safe across kernels.
        let stale: u32 = 0x7fff_fff0;
        let f = tmp_pidfile(&stale.to_string());
        assert_eq!(liveness(f.path()), Liveness::Dead);
    }

    #[test]
    fn liveness_dead_with_missing_file() {
        let path = std::env::temp_dir().join("stores-tui-no-such-pidfile.pid");
        let _ = std::fs::remove_file(&path);
        assert_eq!(liveness(&path), Liveness::Dead);
    }

    #[test]
    fn liveness_dead_with_unparseable() {
        let f = tmp_pidfile("not-a-pid\n");
        assert_eq!(liveness(f.path()), Liveness::Dead);
    }
}
