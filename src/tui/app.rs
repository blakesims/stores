//! TUI app state.

use anyhow::Result;
use rusqlite::Connection;

use super::data::{classify, Row, Section};

/// Options threaded from the CLI into the TUI.
#[derive(Debug, Clone, Default)]
pub struct TuiOpts {
    pub interval_ms: u64,
    pub state_filter: Option<String>,
    pub priority_filter: Option<String>,
    pub tier_filter: Option<String>,
    pub since_filter: Option<String>,
    pub legacy: bool,
}

/// Selection cursor: which (section, index-within-section) is highlighted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub section: usize,
    pub row: usize,
}

/// Sort modes for the row list (cycled with `,`). Phase 1 only stores it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Default,
    UpdatedDesc,
    PriorityDesc,
}

/// Search/filter UI state. Phase 1 only stores it.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub active: bool,
}

/// Status-bar payload (daemon liveness etc.). Phase 1 stub.
#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    pub daemon_pid: Option<u32>,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct App {
    pub opts: TuiOpts,
    pub rows: Vec<Row>,
    pub sections: Vec<(Section, Vec<usize>)>,
    pub selection: Selection,
    pub sort: Sort,
    pub filter: Option<String>,
    pub search: SearchState,
    pub expand_state: std::collections::HashSet<String>,
    pub status_bar: StatusBar,
}

impl App {
    pub fn new(opts: TuiOpts) -> Self {
        Self {
            opts,
            ..Default::default()
        }
    }

    /// Reload rows and rebuild sections.
    pub fn refresh(&mut self, conn: &Connection) -> Result<()> {
        self.rows = super::data::load_rows(conn)?;
        self.sections = classify(&self.rows);
        Ok(())
    }
}
