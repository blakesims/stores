use anyhow::Context;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessThresholds {
    pub no_output_secs: i64,
    pub wall_clock_max_secs: i64,
}

impl Default for LivenessThresholds {
    fn default() -> Self {
        Self {
            no_output_secs: 600,
            wall_clock_max_secs: 1800,
        }
    }
}

impl LivenessThresholds {
    pub fn from_env() -> Self {
        let mut t = Self::default();
        if let Ok(v) = std::env::var("STORES_RUNNER_NO_OUTPUT_SECS") {
            if let Ok(n) = v.parse::<i64>() {
                t.no_output_secs = n;
            }
        }
        if let Ok(v) = std::env::var("STORES_RUNNER_WALL_CLOCK_MAX_SECS") {
            if let Ok(n) = v.parse::<i64>() {
                t.wall_clock_max_secs = n;
            }
        }
        t
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivenessClass {
    Active { idle_secs: i64 },
    StalledNoOutput { idle_secs: i64, threshold_secs: i64 },
    WallClockElapsed { runtime_secs: i64, warn_secs: i64 },
    Unknown,
}

impl LivenessClass {
    pub fn label(&self) -> String {
        match self {
            Self::Active { idle_secs } => format!("state=active idle={idle_secs}s"),
            Self::StalledNoOutput {
                idle_secs,
                threshold_secs,
            } => {
                format!("state=stalled_no_output idle={idle_secs}s threshold={threshold_secs}s")
            }
            Self::WallClockElapsed {
                runtime_secs,
                warn_secs,
            } => {
                format!("state=active warning=wall_clock_elapsed runtime={runtime_secs}s warn={warn_secs}s")
            }
            Self::Unknown => "state=unknown".to_string(),
        }
    }
}

pub fn classify(
    claimed_at_epoch: Option<i64>,
    heartbeat_at_epoch: Option<i64>,
    now_epoch: i64,
    t: &LivenessThresholds,
) -> LivenessClass {
    let Some(claimed_at) = claimed_at_epoch else {
        return LivenessClass::Unknown;
    };
    let runtime = now_epoch.saturating_sub(claimed_at);
    let progress_at = heartbeat_at_epoch.unwrap_or(claimed_at);
    let idle = now_epoch.saturating_sub(progress_at);
    if idle >= t.no_output_secs {
        LivenessClass::StalledNoOutput {
            idle_secs: idle,
            threshold_secs: t.no_output_secs,
        }
    } else if runtime >= t.wall_clock_max_secs {
        LivenessClass::WallClockElapsed {
            runtime_secs: runtime,
            warn_secs: t.wall_clock_max_secs,
        }
    } else {
        LivenessClass::Active { idle_secs: idle }
    }
}

#[derive(Debug, Clone)]
pub struct StreamingOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub payload_error: Option<String>,
    pub killed_for: Option<LivenessClass>,
}

enum Stream {
    Stdout(String),
    Stderr(String),
    Eof,
}

thread_local! {
    static HEARTBEAT_FILE_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(crate) struct HeartbeatFileOverride {
    prior: Option<PathBuf>,
}

impl HeartbeatFileOverride {
    pub(crate) fn install(path: PathBuf) -> Self {
        let prior = HEARTBEAT_FILE_OVERRIDE.with(|slot| slot.replace(Some(path)));
        Self { prior }
    }
}

impl Drop for HeartbeatFileOverride {
    fn drop(&mut self) {
        let prior = self.prior.take();
        HEARTBEAT_FILE_OVERRIDE.with(|slot| {
            slot.replace(prior);
        });
    }
}

pub fn touch_heartbeat_file_from_env() {
    let override_path = HEARTBEAT_FILE_OVERRIDE.with(|slot| slot.borrow().clone());
    let path =
        override_path.or_else(|| std::env::var_os("STORES_HEARTBEAT_FILE").map(PathBuf::from));
    if let Some(path) = path {
        if let Ok(mut f) = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(path)
        {
            let _ = writeln!(f, "{}", crate::handlers::row::now_iso8601());
        }
    }
}

fn killed_payload_error(c: &LivenessClass) -> Option<String> {
    match c {
        LivenessClass::StalledNoOutput {
            idle_secs,
            threshold_secs,
        } => Some(format!(
            "runner timed out: no output for {idle_secs}s (threshold {threshold_secs}s)"
        )),
        LivenessClass::WallClockElapsed { .. } => None,
        _ => None,
    }
}

pub fn run_streaming_with_liveness(
    cmd: &mut Command,
    t: &LivenessThresholds,
    mut on_stdout_line: impl FnMut(&str) -> anyhow::Result<()>,
    mut on_stderr_line: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<StreamingOutput> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn streaming child")?;
    let stdout_pipe = child.stdout.take().context("child stdout was not piped")?;
    let stderr_pipe = child.stderr.take().context("child stderr was not piped")?;
    let (tx, rx) = mpsc::sync_channel::<Stream>(64);

    let tx_out = tx.clone();
    let out_handle = std::thread::Builder::new()
        .name("stores-runner-stdout".to_string())
        .spawn(move || {
            for line in BufReader::new(stdout_pipe).lines().map_while(Result::ok) {
                if tx_out.send(Stream::Stdout(line)).is_err() {
                    return;
                }
            }
            let _ = tx_out.send(Stream::Eof);
        })?;

    let tx_err = tx.clone();
    let err_handle = std::thread::Builder::new()
        .name("stores-runner-stderr".to_string())
        .spawn(move || {
            for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                if tx_err.send(Stream::Stderr(line)).is_err() {
                    return;
                }
            }
            let _ = tx_err.send(Stream::Eof);
        })?;
    drop(tx);

    let started = Instant::now();
    let mut last_output_at = started;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut eof_count = 0usize;
    let mut killed_for = None;
    let mut sink_error: Option<anyhow::Error> = None;

    loop {
        let idle = last_output_at.elapsed().as_secs() as i64;
        if idle > t.no_output_secs {
            let c = LivenessClass::StalledNoOutput {
                idle_secs: idle,
                threshold_secs: t.no_output_secs,
            };
            let _ = child.kill();
            killed_for = Some(c);
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Stream::Stdout(line)) => {
                stdout.push_str(&line);
                stdout.push('\n');
                if let Err(e) = on_stdout_line(&line) {
                    let _ = child.kill();
                    sink_error = Some(e.context("runner stdout live sink failed"));
                    break;
                }
                touch_heartbeat_file_from_env();
                last_output_at = Instant::now();
            }
            Ok(Stream::Stderr(line)) => {
                stderr.push_str(&line);
                stderr.push('\n');
                if let Err(e) = on_stderr_line(&line) {
                    let _ = child.kill();
                    sink_error = Some(e.context("runner stderr live sink failed"));
                    break;
                }
                touch_heartbeat_file_from_env();
                last_output_at = Instant::now();
            }
            Ok(Stream::Eof) => {
                eof_count += 1;
                if eof_count >= 2 {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait().context("failed to wait on streaming child")?;
    while sink_error.is_none() && eof_count < 2 {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Stream::Stdout(line)) => {
                stdout.push_str(&line);
                stdout.push('\n');
                if let Err(e) = on_stdout_line(&line) {
                    sink_error = Some(e.context("runner stdout live sink failed"));
                    break;
                }
                touch_heartbeat_file_from_env();
            }
            Ok(Stream::Stderr(line)) => {
                stderr.push_str(&line);
                stderr.push('\n');
                if let Err(e) = on_stderr_line(&line) {
                    sink_error = Some(e.context("runner stderr live sink failed"));
                    break;
                }
                touch_heartbeat_file_from_env();
            }
            Ok(Stream::Eof) => {
                eof_count += 1;
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = out_handle.join();
    let _ = err_handle.join();
    if let Some(e) = sink_error {
        return Err(e);
    }
    let exit_code = if killed_for.is_some() {
        -1
    } else {
        status.code().unwrap_or(-1)
    };
    let payload_error = killed_for.as_ref().and_then(killed_payload_error);
    Ok(StreamingOutput {
        stdout,
        stderr,
        exit_code,
        payload_error,
        killed_for,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    fn thresholds() -> LivenessThresholds {
        LivenessThresholds {
            no_output_secs: 10,
            wall_clock_max_secs: 100,
        }
    }

    #[test]
    fn active_boundary_below_threshold() {
        assert_eq!(
            classify(Some(50), Some(91), 100, &thresholds()),
            LivenessClass::Active { idle_secs: 9 }
        );
    }

    #[test]
    fn stalled_no_output_boundary_at_threshold() {
        assert_eq!(
            classify(Some(50), Some(90), 100, &thresholds()),
            LivenessClass::StalledNoOutput {
                idle_secs: 10,
                threshold_secs: 10
            }
        );
    }

    #[test]
    fn stalled_no_output_precedes_wall_clock_elapsed() {
        assert_eq!(
            classify(Some(0), Some(0), 100, &thresholds()),
            LivenessClass::StalledNoOutput {
                idle_secs: 100,
                threshold_secs: 10
            }
        );
    }

    #[test]
    fn wall_clock_elapsed_is_advisory_when_active() {
        assert_eq!(
            classify(Some(0), Some(99), 100, &thresholds()),
            LivenessClass::WallClockElapsed {
                runtime_secs: 100,
                warn_secs: 100
            }
        );
    }

    #[test]
    fn missing_heartbeat_recent_claim_is_active() {
        assert_eq!(
            classify(Some(95), None, 100, &thresholds()),
            LivenessClass::Active { idle_secs: 5 }
        );
    }

    #[test]
    fn missing_heartbeat_old_claim_is_stalled() {
        assert_eq!(
            classify(Some(90), None, 100, &thresholds()),
            LivenessClass::StalledNoOutput {
                idle_secs: 10,
                threshold_secs: 10
            }
        );
    }

    #[test]
    fn unknown_when_claimed_at_missing() {
        assert_eq!(
            classify(None, Some(99), 100, &thresholds()),
            LivenessClass::Unknown
        );
    }

    #[test]
    fn label_formats_exactly() {
        assert_eq!(
            LivenessClass::Active { idle_secs: 3 }.label(),
            "state=active idle=3s"
        );
        assert_eq!(
            LivenessClass::StalledNoOutput {
                idle_secs: 4,
                threshold_secs: 2
            }
            .label(),
            "state=stalled_no_output idle=4s threshold=2s"
        );
        assert_eq!(
            LivenessClass::WallClockElapsed {
                runtime_secs: 5,
                warn_secs: 4
            }
            .label(),
            "state=active warning=wall_clock_elapsed runtime=5s warn=4s"
        );
        assert_eq!(LivenessClass::Unknown.label(), "state=unknown");
    }

    #[test]
    fn streaming_helper_touches_heartbeat_for_stdout_and_stderr() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let heartbeat = tempfile::NamedTempFile::new().unwrap();
        std::env::set_var("STORES_HEARTBEAT_FILE", heartbeat.path());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo stdout-progress; echo stderr-progress >&2");
        let out = run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds {
                no_output_secs: 5,
                wall_clock_max_secs: 30,
            },
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("stdout-progress"));
        assert!(out.stderr.contains("stderr-progress"));
        let mtime = std::fs::metadata(heartbeat.path())
            .unwrap()
            .modified()
            .unwrap();
        assert!(mtime.elapsed().unwrap() <= Duration::from_secs(2));
        std::env::remove_var("STORES_HEARTBEAT_FILE");
    }

    #[test]
    fn streaming_callbacks_fire_before_child_exit() {
        let started = Instant::now();
        let mut first_stdout_at = None;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("echo first; sleep 1; echo second");
        let out = run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds {
                no_output_secs: 5,
                wall_clock_max_secs: 30,
            },
            |line| {
                if line == "first" {
                    first_stdout_at = Some(started.elapsed());
                }
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("first"));
        assert!(out.stdout.contains("second"));
        let first = first_stdout_at.expect("first line callback must fire");
        assert!(
            first < Duration::from_millis(800),
            "first callback should fire before child exit; got {first:?}"
        );
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[test]
    fn streaming_callback_error_surfaces() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("echo first; sleep 5");
        let err = run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds {
                no_output_secs: 10,
                wall_clock_max_secs: 30,
            },
            |_| anyhow::bail!("sink exploded"),
            |_| Ok(()),
        )
        .expect_err("sink failure must fail runner infrastructure");
        assert!(
            err.to_string().contains("runner stdout live sink failed"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn cargo_no_output_pattern_killed_within_threshold() {
        let started = Instant::now();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("exec sleep 5");
        let out = run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds {
                no_output_secs: 1,
                wall_clock_max_secs: 30,
            },
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert!(
            started.elapsed() <= Duration::from_secs(3),
            "elapsed={:?}",
            started.elapsed()
        );
        assert_eq!(out.exit_code, -1);
        assert!(matches!(
            out.killed_for,
            Some(LivenessClass::StalledNoOutput { .. })
        ));
    }

    #[test]
    fn wall_clock_elapsed_does_not_kill_noisy_child() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("echo start; sleep 2; echo still-active; sleep 1; echo done");
        let out = run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds {
                no_output_secs: 30,
                wall_clock_max_secs: 1,
            },
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("still-active"));
        assert!(out.killed_for.is_none());
    }
}
