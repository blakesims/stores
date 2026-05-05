//! TUI app state.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;

use super::data::{classify, Row, Section};
use super::filter::{FilterPalette, FilterPredicate};
use super::search::SearchState;
use super::sort::{sort_indices, Sort};

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

/// Selection cursor: which (section_idx, row_idx_within_section) is highlighted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub section: usize,
    pub row: usize,
}

/// Modal state machine for the input dispatcher.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Filter,
    Search,
}

/// Status-bar payload (daemon liveness etc.). Phase 1 stub.
#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    pub daemon_pid: Option<u32>,
    pub message: String,
}

/// One selectable row in the flattened, post-filter, post-collapse list
/// (i.e. the order the user actually navigates through).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatRow {
    pub section: usize,
    pub row: usize,
    /// Index back into `App.rows`.
    pub abs: usize,
}

#[derive(Debug, Default)]
pub struct App {
    pub opts: TuiOpts,
    pub rows: Vec<Row>,
    /// Pre-classification per Section in canonical order; sort + filter are
    /// re-applied on top.
    pub sections: Vec<(Section, Vec<usize>)>,
    pub selection: Selection,
    pub mode: Mode,
    pub sort: Sort,
    pub filter: FilterPredicate,
    pub filter_palette: Option<FilterPalette>,
    pub search: SearchState,
    /// Sections whose rows are hidden (collapsed). A section not in the set
    /// is expanded (default).
    pub collapsed: HashSet<Section>,
    pub status_bar: StatusBar,
    /// Visible-rows-per-page used by PgUp/PgDn + virtualization. Updated on
    /// each render; defaults to a sentinel until the first draw.
    pub viewport_height: usize,
    /// First flat-row index currently rendered.
    pub scroll_offset: usize,
}

impl App {
    pub fn new(opts: TuiOpts) -> Self {
        Self {
            opts,
            viewport_height: 20,
            ..Default::default()
        }
    }

    /// Reload rows and rebuild sections (then re-apply current sort).
    pub fn refresh(&mut self, conn: &Connection) -> Result<()> {
        self.rows = super::data::load_rows(conn)?;
        self.sections = classify(&self.rows);
        self.apply_sort();
        Ok(())
    }

    /// Apply the active sort to every section bucket.
    pub fn apply_sort(&mut self) {
        for (_, indices) in self.sections.iter_mut() {
            sort_indices(self.sort, &self.rows, indices);
        }
    }

    /// Cycle the sort key one position and re-apply.
    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.cycle();
        self.apply_sort();
    }

    /// Toggle collapse state of the section currently under the cursor.
    pub fn toggle_collapse_current(&mut self) {
        if let Some((sec, _)) = self.sections.get(self.selection.section) {
            if self.collapsed.contains(sec) {
                self.collapsed.remove(sec);
            } else {
                self.collapsed.insert(*sec);
                self.selection.row = 0;
            }
        }
    }

    /// Compute the navigable flat-row list — rows that pass the filter and
    /// live in non-collapsed sections, walked in canonical section order.
    pub fn flat_rows(&self) -> Vec<FlatRow> {
        let mut out = Vec::new();
        for (sec_idx, (sec, indices)) in self.sections.iter().enumerate() {
            if self.collapsed.contains(sec) {
                continue;
            }
            for (within, &abs) in indices.iter().enumerate() {
                if self.filter.is_empty() || self.filter.matches(&self.rows[abs], *sec) {
                    out.push(FlatRow {
                        section: sec_idx,
                        row: within,
                        abs,
                    });
                }
            }
        }
        out
    }

    /// Find the flat-row index of the current selection (or None when the
    /// selection has fallen out of view due to filter/collapse changes).
    pub fn current_flat(&self) -> Option<usize> {
        let flat = self.flat_rows();
        flat.iter().position(|f| {
            f.section == self.selection.section && f.row == self.selection.row
        })
    }

    /// Move the selection by `delta` flat positions, clamped to bounds.
    pub fn move_selection(&mut self, delta: isize) {
        let flat = self.flat_rows();
        if flat.is_empty() {
            return;
        }
        let cur = self.current_flat().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, flat.len() as isize - 1) as usize;
        self.set_selection_to_flat(next, &flat);
    }

    /// Move by one viewport page in flat-row positions.
    pub fn move_page(&mut self, direction: isize) {
        let step = self.viewport_height.max(1) as isize;
        self.move_selection(direction * step);
    }

    /// Set the selection to a specific flat-row index.
    pub fn jump_to_flat(&mut self, idx: usize) {
        let flat = self.flat_rows();
        if flat.is_empty() {
            return;
        }
        let clamped = idx.min(flat.len() - 1);
        self.set_selection_to_flat(clamped, &flat);
    }

    fn set_selection_to_flat(&mut self, idx: usize, flat: &[FlatRow]) {
        if let Some(target) = flat.get(idx) {
            self.selection = Selection {
                section: target.section,
                row: target.row,
            };
            // Keep selection in viewport.
            if idx < self.scroll_offset {
                self.scroll_offset = idx;
            } else if idx >= self.scroll_offset + self.viewport_height.max(1) {
                self.scroll_offset = idx + 1 - self.viewport_height.max(1);
            }
        }
    }

    /// Update the predicate from the filter palette (Enter).
    pub fn commit_filter(&mut self) {
        if let Some(p) = self.filter_palette.take() {
            self.filter = p.draft;
        }
        self.mode = Mode::Normal;
        self.clamp_selection();
    }

    /// Discard the in-progress palette draft (Esc).
    pub fn cancel_filter(&mut self) {
        self.filter_palette = None;
        self.mode = Mode::Normal;
    }

    /// Clear an applied filter back to wildcard.
    pub fn clear_filter(&mut self) {
        self.filter = FilterPredicate::default();
        self.clamp_selection();
    }

    /// Apply a saved-view preset (`1`/`2`/`3`).
    pub fn apply_preset(&mut self, key: char) {
        if let Some(p) = super::filter::preset(key) {
            self.filter = p;
            self.clamp_selection();
        }
    }

    /// Recompute the search hit list against the current visible rows.
    pub fn recompute_search(&mut self) {
        let flat = self.flat_rows();
        let view: Vec<&Row> = flat.iter().map(|f| &self.rows[f.abs]).collect();
        if let Some(idx) = self.search.recompute(&view) {
            self.set_selection_to_flat(idx, &flat);
        }
    }

    pub fn search_next(&mut self) {
        let flat = self.flat_rows();
        if let Some(idx) = self.search.next() {
            self.set_selection_to_flat(idx, &flat);
        }
    }

    pub fn search_prev(&mut self) {
        let flat = self.flat_rows();
        if let Some(idx) = self.search.prev() {
            self.set_selection_to_flat(idx, &flat);
        }
    }

    /// If filter/collapse made the current selection invalid, snap back to
    /// the first visible row.
    fn clamp_selection(&mut self) {
        if self.current_flat().is_none() {
            let flat = self.flat_rows();
            if !flat.is_empty() {
                self.set_selection_to_flat(0, &flat);
            } else {
                self.selection = Selection::default();
                self.scroll_offset = 0;
            }
        }
    }
}
