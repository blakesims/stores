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
use rusqlite::{Connection, OpenFlags};
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

        if last_refresh.elapsed() >= refresh_interval {
            app.refresh(conn)?;
            last_refresh = Instant::now();
        }
    }
}
