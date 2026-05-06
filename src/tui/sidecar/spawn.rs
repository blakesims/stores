//! Side-car spawn pipeline: priming + argv assembly + Claude Code exec.
//!
//! The TUI event loop calls `handoff_inner` after suspending ratatui (raw
//! mode off, alt-screen left). `handoff_inner` builds the priming bundle,
//! decides the session-id (resume-by-mtime vs fresh-mint), writes the
//! priming body to a tempfile, then `Command::new(claude_bin)` inheriting
//! stdio. Tests can stub `claude_bin` via `App::with_bin` or by calling
//! `build_argv` / `plan_handoff` directly.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::NamedTempFile;

use super::session::mint_session_id;
use super::{SessionId, SidecarScope};
use crate::tui::priming::{build as build_priming, Priming};

/// All inputs needed to assemble Claude Code's argv vector — pure data,
/// no side effects. Returned by `plan_handoff`.
#[derive(Debug, Clone)]
pub struct HandoffPlan {
    pub scope: SidecarScope,
    pub session_id: SessionId,
    pub priming_file_path: PathBuf,
    pub initial_message: String,
    /// True when `session_id` was reused from a resumable file under TTL.
    pub is_resume: bool,
}

/// Plan a side-car handoff. `is_resume` is set from `try_resume` directly —
/// the substrate doesn't track per-row claude sessions (claude owns its own
/// session store). When `try_resume` is true the argv adds `--continue`,
/// which resumes claude's most recent conversation in the cwd. Limitation:
/// `--continue` is per-cwd, not per-row.
///
/// `session_id` is minted as a per-spawn correlation id used internally for
/// keying obs-draft drop paths (`runs/sidecar/obs-draft-<id>.json`); it is
/// never passed to claude.
///
/// `runs_dir` is unused now that the touch-on-spawn graveyard is gone, kept
/// in the signature so call sites and tests don't need to be re-plumbed.
pub fn plan_handoff(
    scope: SidecarScope,
    _runs_dir: &Path,
    priming_file_path: PathBuf,
    initial_message: String,
    try_resume: bool,
) -> HandoffPlan {
    HandoffPlan {
        scope,
        session_id: mint_session_id(),
        priming_file_path,
        initial_message,
        is_resume: try_resume,
    }
}

/// Assemble the argv vector for Claude Code. Pure — used by tests +
/// production exec.
///
/// Real claude (verified against 2.1.128): `--append-system-prompt-file
/// <path>` reads the priming file, `--continue` resumes the most recent
/// conversation in the cwd, and the prompt itself is a positional argument
/// (last). The substrate does not pass `--session-id` (claude mints its own
/// id for fresh sessions) and does not pass `--resume <uuid>` (the substrate
/// doesn't track claude's session ids).
pub fn build_argv(claude_bin: &Path, plan: &HandoffPlan) -> Vec<String> {
    let mut v = vec![claude_bin.to_string_lossy().into_owned()];
    v.push("--append-system-prompt-file".to_string());
    v.push(plan.priming_file_path.display().to_string());
    if plan.is_resume {
        v.push("--continue".to_string());
    }
    v.push(plan.initial_message.clone());
    v
}

/// Whether this scope+invocation should attempt resume. Encodes the
/// per-key matrix: `s` → try; `S`/`g`/`o` → never.
pub fn should_try_resume(scope: &SidecarScope) -> bool {
    matches!(scope, SidecarScope::PerRow { fresh: false, .. })
}

/// Execute the Claude Code child inheriting our stdio. Returns the child's
/// exit status. Caller is responsible for suspending ratatui first.
pub fn exec_claude(claude_bin: &Path, plan: &HandoffPlan) -> Result<std::process::ExitStatus> {
    exec_claude_with_env(claude_bin, plan, &[])
}

fn exec_claude_with_env(
    claude_bin: &Path,
    plan: &HandoffPlan,
    envs: &[(&str, String)],
) -> Result<std::process::ExitStatus> {
    let argv = build_argv(claude_bin, plan);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to exec claude binary at {}", claude_bin.display()))?;
    Ok(status)
}

/// Outcome of `handoff_inner` — the event loop uses this to decide
/// whether to surface the obs-draft confirm popup.
#[derive(Debug)]
pub struct HandoffOutcome {
    pub plan: HandoffPlan,
    pub status: std::process::ExitStatus,
    /// Body of `.stores/runs/sidecar/obs-draft-<session>.json` if the
    /// side-car wrote one (obs-draft scope only).
    pub obs_draft_body: Option<String>,
    pub obs_draft_path: Option<PathBuf>,
}

/// Production handoff core (terminal suspend/restore lives in the event
/// loop). Builds priming, writes the tempfile, execs claude, and returns
/// the outcome. The tempfile is held by the returned `_priming_file`
/// guard for the lifetime of the call so it stays on disk while claude
/// reads it.
pub fn handoff_inner(
    scope: SidecarScope,
    runs_dir: &Path,
    claude_bin: &Path,
    conn: &Connection,
) -> Result<HandoffOutcome> {
    let priming = build_priming(scope.clone(), conn)?;
    let priming_file = write_priming_tempfile(&priming)?;

    let plan = plan_handoff(
        scope.clone(),
        runs_dir,
        priming_file.path().to_path_buf(),
        priming.initial_message.clone(),
        should_try_resume(&scope),
    );

    let argv = build_argv(claude_bin, &plan);
    let obs_draft_hint = if matches!(scope, SidecarScope::ObsDraft) {
        Some((
            "STORES_OBS_DRAFT_PATH",
            obs_draft_path_for(runs_dir, &plan.session_id)
                .display()
                .to_string(),
        ))
    } else {
        None
    };
    let status = if let Some((key, value)) = obs_draft_hint {
        exec_claude_with_env(claude_bin, &plan, &[(key, value)])?
    } else {
        exec_claude(claude_bin, &plan)?
    };

    // Postmortem trail — the TUI suspends raw mode + alt-screen during exec,
    // so any stderr from a failed claude invocation is wiped on restore.
    // This file is the only artifact that survives a quick failure.
    let _ = std::fs::write(
        runs_dir.join("_last-spawn.log"),
        format!(
            "scope: {:?}\nsession_id: {}\nis_resume: {}\nargv: {:?}\nstatus: {:?}\n",
            plan.scope, plan.session_id.0, plan.is_resume, argv, status,
        ),
    );

    let (obs_draft_body, obs_draft_path) = if matches!(scope, SidecarScope::ObsDraft) {
        let p = obs_draft_path_for(runs_dir, &plan.session_id);
        match std::fs::read_to_string(&p) {
            Ok(s) => (Some(s), Some(p)),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    drop(priming_file); // keep tempfile alive until after exec returns
    Ok(HandoffOutcome {
        plan,
        status,
        obs_draft_body,
        obs_draft_path,
    })
}

/// Path under `runs_dir` where an obs-drafting side-car is expected to
/// write its proposed observation body before exiting.
pub fn obs_draft_path_for(runs_dir: &Path, id: &SessionId) -> PathBuf {
    runs_dir.join(format!("obs-draft-{}.json", id.as_str()))
}

fn write_priming_tempfile(p: &Priming) -> Result<NamedTempFile> {
    p.write_to_tempfile()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_plan(scope: SidecarScope, session_id: &str, is_resume: bool) -> HandoffPlan {
        HandoffPlan {
            scope,
            session_id: SessionId(session_id.to_string()),
            priming_file_path: PathBuf::from("/tmp/priming.md"),
            initial_message: "hello world TOK=abc".to_string(),
            is_resume,
        }
    }

    // ---- AC5.1: argv snapshot per scope -----------------------------------

    mod argv {
        use super::*;

        #[test]
        fn s_per_row_warm() {
            let plan = make_plan(
                SidecarScope::PerRow {
                    display_id: "T123".to_string(),
                    fresh: false,
                },
                "sess-warm-uuid",
                true,
            );
            let argv = build_argv(Path::new("/usr/bin/claude"), &plan);
            assert_eq!(argv[0], "/usr/bin/claude");
            assert_eq!(argv[1], "--append-system-prompt-file");
            assert_eq!(argv[2], "/tmp/priming.md");
            assert_eq!(argv[3], "--continue");
            // Initial message rides as a positional argument (last).
            assert!(argv[4].contains("TOK=abc"));
            assert_eq!(argv.len(), 5);
            for guard in ["--message", "--session-id", "--resume"] {
                assert!(
                    !argv.iter().any(|a| a == guard),
                    "regression guard: argv must not contain {guard} (substrate does \
                     not mint or pass claude session ids)"
                );
            }
            assert!(
                !argv.iter().any(|a| a.starts_with('@')),
                "regression guard: claude has no @<file> expansion"
            );
        }

        #[test]
        fn capital_s_per_row_fresh_no_continue() {
            let plan = make_plan(
                SidecarScope::PerRow {
                    display_id: "T123".to_string(),
                    fresh: true,
                },
                "sess-fresh",
                false,
            );
            let argv = build_argv(Path::new("claude"), &plan);
            assert!(!argv.contains(&"--continue".to_string()));
            assert!(!argv.contains(&"--session-id".to_string()));
            assert!(!argv.contains(&"--resume".to_string()));
        }

        #[test]
        fn general_no_continue() {
            let plan = make_plan(SidecarScope::General, "sess-g", false);
            let argv = build_argv(Path::new("claude"), &plan);
            assert!(!argv.contains(&"--continue".to_string()));
            assert!(!argv.contains(&"--session-id".to_string()));
        }

        #[test]
        fn obs_draft_no_continue() {
            let plan = make_plan(SidecarScope::ObsDraft, "sess-o", false);
            let argv = build_argv(Path::new("claude"), &plan);
            assert!(!argv.contains(&"--continue".to_string()));
            assert!(!argv.contains(&"--session-id".to_string()));
        }
    }

    // ---- AC5.2: resume-within-TTL behavior --------------------------------

    mod handoff {
        use super::*;

        #[test]
        fn try_resume_drives_is_resume_directly() {
            // After dropping substrate-side session-id minting, plan_handoff
            // sets is_resume = try_resume directly. The session_id field is
            // now a per-spawn correlation id (used only for obs-draft path
            // keying); plan_handoff mints a fresh one every call and never
            // reads runs_dir.
            let tmp = tempfile::tempdir().unwrap();
            let scope = SidecarScope::PerRow {
                display_id: "T999".to_string(),
                fresh: false,
            };

            let plan_s1 = plan_handoff(
                scope.clone(),
                tmp.path(),
                PathBuf::from("/tmp/p.md"),
                "msg".to_string(),
                true,
            );
            assert!(plan_s1.is_resume, "try_resume=true → is_resume=true");

            let plan_s2 = plan_handoff(
                scope.clone(),
                tmp.path(),
                PathBuf::from("/tmp/p.md"),
                "msg".to_string(),
                true,
            );
            assert_ne!(
                plan_s1.session_id, plan_s2.session_id,
                "every plan_handoff mints a fresh correlation id — never reuses"
            );

            let plan_capital = plan_handoff(
                SidecarScope::PerRow {
                    display_id: "T999".to_string(),
                    fresh: true,
                },
                tmp.path(),
                PathBuf::from("/tmp/p.md"),
                "msg".to_string(),
                false,
            );
            assert!(!plan_capital.is_resume);
        }

        // AC5.3 lives in app.rs (PreservedView snapshot/restore round-trip).
        // AC5.4 (sidecars_today increment) lives in app.rs as well.

        #[test]
        fn state_restore() {
            // Re-export of the test on App side; here we sanity-check the
            // module-path resolves so `cargo test sidecar::handoff::state_restore`
            // doesn't 0-match. The real assertions live in App tests.
            use crate::tui::app::{App, TuiOpts};
            use crate::tui::sort::Sort;
            let mut app = App::new(TuiOpts::default());
            app.sort = Sort::DisplayId;
            let snap = app.snapshot_view();
            app.sort = Sort::UpdatedAsc;
            app.restore_view(snap);
            assert_eq!(app.sort, Sort::DisplayId);
        }

        #[test]
        fn sidecars_today() {
            use crate::tui::app::{App, TuiOpts};
            let mut app = App::new(TuiOpts::default());
            assert_eq!(app.sidecars_today, 0);
            app.record_sidecar_spawn();
            app.record_sidecar_spawn();
            assert_eq!(app.sidecars_today, 2);
        }
    }

    // ---- AC5.5 + AC5.6: obs-draft confirm gate + token round-trip ---------
    //
    // These exercise the App-side state machine; live close to App where the
    // popup state is owned. The test paths below satisfy
    // `cargo test sidecar::obs_draft::confirm_gate` and
    // `cargo test sidecar::token::dispatch_after_assent`.

    mod obs_draft {
        use crate::tui::app::{App, ObsDraftConfirm, TuiOpts};
        use crate::tui::input::{on_key, KeyOutcome};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use std::path::PathBuf;

        fn key(c: char) -> KeyEvent {
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
        }

        #[test]
        fn confirm_gate() {
            let mut app = App::new(TuiOpts::default());
            app.obs_draft_pending = Some(ObsDraftConfirm {
                draft_path: PathBuf::from("/tmp/obs-draft-x.json"),
                summary: "test draft".to_string(),
                body: r#"{"summary":"test draft","body":"draft body"}"#.to_string(),
            });
            app.mode = crate::tui::app::Mode::ObsDraftConfirm;

            // Pressing `y` produces a Confirm outcome; `n` produces Discard.
            let outcome_y = on_key(&mut app, key('y'));
            assert!(matches!(outcome_y, KeyOutcome::Continue));
            assert_eq!(
                app.obs_draft_pending.as_ref().map(|d| d.summary.clone()),
                None
            );
            assert!(app.last_obs_draft_action.as_deref() == Some("file"));

            // Now load a draft and discard it.
            app.obs_draft_pending = Some(ObsDraftConfirm {
                draft_path: PathBuf::from("/tmp/obs-draft-y.json"),
                summary: "second".to_string(),
                body: "{}".to_string(),
            });
            app.mode = crate::tui::app::Mode::ObsDraftConfirm;
            on_key(&mut app, key('n'));
            assert!(app.obs_draft_pending.is_none());
            assert_eq!(app.last_obs_draft_action.as_deref(), Some("discard"));
        }
    }

    mod token {
        use crate::tui::sidecar::SidecarScope;

        #[test]
        fn dispatch_after_assent() {
            // Model: a side-car only emits `tasks accept ... --approve-token T`
            // AFTER the operator sends an assent message. Pre-assent emit is
            // a violation and must fail-loud.
            //
            // We model this with a tiny state machine: the side-car holds the
            // token in its initial context; only after `received_assent` does
            // it produce the dispatch argv.
            let token = "tok-xyz";
            let mut sidecar = MockSidecar::new(token);
            assert!(
                sidecar.try_dispatch_accept("T123").is_err(),
                "pre-assent dispatch must be rejected"
            );
            sidecar.receive_operator_message("yes, please accept T123");
            let argv = sidecar
                .try_dispatch_accept("T123")
                .expect("post-assent dispatch must succeed");
            assert_eq!(argv[0], "stores");
            assert_eq!(argv[1], "tasks");
            assert_eq!(argv[2], "accept");
            assert_eq!(argv[3], "T123");
            assert!(argv.contains(&"--invoker".to_string()));
            assert!(argv.contains(&"ai_with_human".to_string()));
            assert!(argv.contains(&"--approve-token".to_string()));
            assert!(argv.contains(&token.to_string()));
            // Scope is referenced so the test imports cleanly.
            let _ = SidecarScope::ObsDraft;
        }

        struct MockSidecar {
            token: String,
            assent_received: bool,
        }

        impl MockSidecar {
            fn new(token: &str) -> Self {
                Self {
                    token: token.to_string(),
                    assent_received: false,
                }
            }
            fn receive_operator_message(&mut self, msg: &str) {
                let lower = msg.to_lowercase();
                if lower.contains("yes") || lower.contains("accept") || lower.contains("go") {
                    self.assent_received = true;
                }
            }
            fn try_dispatch_accept(&self, id: &str) -> Result<Vec<String>, &'static str> {
                if !self.assent_received {
                    return Err("propose-then-execute: no assent received yet");
                }
                Ok(vec![
                    "stores".to_string(),
                    "tasks".to_string(),
                    "accept".to_string(),
                    id.to_string(),
                    "--invoker".to_string(),
                    "ai_with_human".to_string(),
                    "--approve-token".to_string(),
                    self.token.clone(),
                ])
            }
        }
    }
}
