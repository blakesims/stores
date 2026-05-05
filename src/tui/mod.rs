//! T028 P1: ratatui-based TUI for `stores watch`.
//!
//! Replaces the legacy ANSI POC behind the `--legacy` escape hatch. Phase 2
//! adds keyboard navigation, modal sort/filter/search, and virtualization.

pub mod app;
pub mod daemon;
pub mod data;
pub mod filter;
pub mod footer;
pub mod help;
pub mod input;
pub mod priming;
pub mod render;
pub mod search;
pub mod sidecar;
pub mod sort;
pub mod status_bar;
pub mod term;

use anyhow::{bail, Result};
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rusqlite::{Connection, OpenFlags};
use std::io::stdout;
use std::time::{Duration, Instant};

use crate::paths::db_path;

pub use app::{App, TuiOpts};
pub use input::{on_key, KeyOutcome};

/// Run the TUI event loop. Blocks until the user quits.
pub fn run(opts: TuiOpts) -> Result<()> {
    let db = db_path()?;
    if !db.exists() {
        bail!(
            ".stores/db.sqlite not found in '{}'; run `stores init` first",
            std::env::current_dir()?.display()
        );
    }

    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut terminal = term::setup()?;
    term::install_panic_hook();

    let res = event_loop(&mut terminal, &conn, &opts);

    term::teardown(&mut terminal)?;
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    conn: &Connection,
    opts: &TuiOpts,
) -> Result<()> {
    let mut app = App::new(opts.clone());
    app.refresh(conn)?;

    let refresh_interval = Duration::from_millis(opts.interval_ms);
    let poll_interval = Duration::from_millis(100);
    let mut last_refresh = Instant::now();

    loop {
        terminal.draw(|f| render::draw(f, &mut app))?;

        if event::poll(poll_interval)? {
            if let Event::Key(ev) = event::read()? {
                if matches!(on_key(&mut app, ev), KeyOutcome::Quit) {
                    return Ok(());
                }
            }
        }

        if let Some(scope) = app.pending_spawn.take() {
            handle_handoff(terminal, &mut app, conn, scope);
        }

        if let Some(draft) = app.obs_draft_filing_request.take() {
            match sidecar::obs_draft::file_observation(&draft) {
                Ok(()) => {
                    app.status_bar.message = "obs filed".to_string();
                }
                Err(e) => {
                    app.status_bar.message = format!("obs file failed: {e}");
                }
            }
            let _ = std::fs::remove_file(&draft.draft_path);
        }

        if last_refresh.elapsed() >= refresh_interval {
            app.refresh(conn)?;
            last_refresh = Instant::now();
        }
    }
}

/// Run a side-car hand-off in production: snapshot view, leave alt-screen,
/// exec claude, restore alt-screen + view, increment counter, surface
/// obs-draft confirm popup if applicable.
fn handle_handoff<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    app: &mut App,
    conn: &Connection,
    scope: sidecar::SidecarScope,
) {
    let snap = app.snapshot_view();
    let runs_dir = match sidecar::session::sidecar_runs_dir() {
        Ok(p) => p,
        Err(e) => {
            app.status_bar.message = format!("sidecar dir resolve failed: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&runs_dir) {
        app.status_bar.message = format!("mkdir runs/sidecar failed: {e}");
        return;
    }

    // Suspend ratatui.
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    let bin = app.claude_bin.clone();
    let result = sidecar::spawn::handoff_inner(scope.clone(), &runs_dir, &bin, conn);

    // Resume ratatui.
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen);
    let _ = terminal.clear();

    match result {
        Ok(outcome) if outcome.status.success() => {
            app.record_sidecar_spawn();
            if let (Some(body), Some(path)) = (outcome.obs_draft_body, outcome.obs_draft_path)
            {
                let summary = parse_draft_summary(&body)
                    .unwrap_or_else(|| "(unparseable)".to_string());
                app.obs_draft_pending = Some(app::ObsDraftConfirm {
                    draft_path: path,
                    summary,
                    body,
                });
                app.mode = app::Mode::ObsDraftConfirm;
            }
        }
        Ok(outcome) => {
            app.status_bar.message =
                format!("sidecar exited non-zero: {}", outcome.status);
        }
        Err(e) => {
            app.status_bar.message = format!("sidecar spawn failed: {e}");
        }
    }

    app.restore_view(snap);
    if let Err(e) = app.refresh(conn) {
        app.status_bar.message = format!("refresh failed: {e}");
    }
}

fn parse_draft_summary(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("summary")?.as_str().map(str::to_string))
}
