//! T028 P1: ratatui-based TUI for `stores watch`.
//!
//! Replaces the legacy ANSI POC behind the `--legacy` escape hatch. This
//! phase stands up the binary path: alt-screen, section-grouped row list,
//! status bar, idle 1 Hz repaint, q/Ctrl-C quit. Side-car spawn keys, sort,
//! filter, search, and daemon liveness arrive in later phases.

pub mod app;
pub mod data;
pub mod render;
pub mod term;

use anyhow::{bail, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use rusqlite::{Connection, OpenFlags};
use std::time::{Duration, Instant};

use crate::paths::db_path;

pub use app::{App, TuiOpts};

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
        terminal.draw(|f| render::draw(f, &app))?;

        if event::poll(poll_interval)? {
            if let Event::Key(KeyEvent {
                code, modifiers, ..
            }) = event::read()?
            {
                match (code, modifiers) {
                    (KeyCode::Char('q'), _) => return Ok(()),
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),
                    _ => {}
                }
            }
        }

        if last_refresh.elapsed() >= refresh_interval {
            app.refresh(conn)?;
            last_refresh = Instant::now();
        }
    }
}
