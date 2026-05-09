//! TUI app state.

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::PathBuf;

use super::daemon::Liveness;
use super::data::{
    classify_with_options, is_terminal_task_status, store_lane_for_row, ExternalReviewState, Row,
    Section, StoreLane, SystemHealth, WatchClassifyOptions,
};
use super::filter::{FilterPalette, FilterPredicate};
use super::search::SearchState;
use super::sidecar::SidecarScope;
use super::sort::{sort_indices, Sort};
use std::time::{SystemTime, UNIX_EPOCH};

/// Options threaded from the CLI into the TUI.
#[derive(Debug, Clone, Default)]
pub struct TuiOpts {
    pub interval_ms: u64,
    pub state_filter: Option<String>,
    pub priority_filter: Option<String>,
    pub tier_filter: Option<String>,
    pub since_filter: Option<String>,
    pub legacy: bool,
    pub all_history: bool,
    /// Override for the `claude` executable used by side-car hand-off.
    /// `None` → resolves "claude" from `$PATH` at spawn time.
    pub claude_bin: Option<PathBuf>,
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
    Detail,
    /// Confirm popup after an obs-drafting side-car returned with a draft.
    ObsDraftConfirm,
}

/// Row type captured when opening a read-only drilldown page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Task,
    Observation,
    Review,
    Intake,
}

/// Selected read-only drilldown target and page scroll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailSelection {
    pub display_id: String,
    pub kind: DetailKind,
    pub scroll_offset: usize,
}

/// Pending obs-drafting decision: the side-car wrote a draft to disk and
/// the operator must press y (file) or n (discard) before normal mode
/// resumes.
#[derive(Debug, Clone)]
pub struct ObsDraftConfirm {
    pub draft_path: std::path::PathBuf,
    pub summary: String,
    pub body: String,
}

/// Snapshot of the user-visible view state, captured before a side-car
/// hand-off and restored on return.
#[derive(Debug, Clone, Default)]
pub struct PreservedView {
    pub selection: Selection,
    pub sort: Sort,
    pub filter: FilterPredicate,
    pub collapsed: HashSet<Section>,
    pub scroll_offset: usize,
    pub focused_store: StoreLane,
}

/// Status-bar payload (daemon liveness, db path, clock, cap, message).
#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    pub daemon_pid: Option<u32>,
    pub daemon_liveness: Liveness,
    pub db_path: Option<String>,
    pub clock: String,
    /// `(active, total)` task counts driving the cap free/total readout.
    pub cap: (usize, usize),
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
    pub detail: Option<DetailSelection>,
    pub sort: Sort,
    pub filter: FilterPredicate,
    pub filter_palette: Option<FilterPalette>,
    pub search: SearchState,
    /// Sections whose rows are hidden (collapsed). A section not in the set
    /// is expanded (default).
    pub collapsed: HashSet<Section>,
    pub status_bar: StatusBar,
    /// Optional T083 external-review lane state; absent tables degrade to unavailable.
    pub external_review: ExternalReviewState,
    /// Dispatch-lock health read during refresh; draw never opens SQLite.
    pub system_health: SystemHealth,
    /// Visible-rows-per-page used by PgUp/PgDn + virtualization. Updated on
    /// each render; defaults to a sentinel until the first draw.
    pub viewport_height: usize,
    /// First flat-row index currently rendered.
    pub scroll_offset: usize,
    /// Number of side-car spawns this session (status-bar counter).
    pub sidecars_today: u32,
    /// Whether the `?` cheat-sheet popup is currently visible.
    pub show_help: bool,
    /// Side-car spawn requested by the input dispatcher; the event loop
    /// drains this each tick.
    pub pending_spawn: Option<SidecarScope>,
    /// Side-car path to the `claude` binary. Defaults to `claude`
    /// (resolved via `$PATH`) — tests override via `with_bin`.
    pub claude_bin: PathBuf,
    /// Pending obs-draft confirm popup (after `o` returned with a draft).
    pub obs_draft_pending: Option<ObsDraftConfirm>,
    /// When the operator pressed `y` on the popup, the draft moves here for
    /// the event loop to pick up and shell out to `stores observations add`.
    pub obs_draft_filing_request: Option<ObsDraftConfirm>,
    /// Trace of the last obs-draft confirm action ("file" or "discard").
    /// Tests assert against it; production ignores.
    pub last_obs_draft_action: Option<String>,
    /// Cockpit lane currently focused for navigation. Defaults to
    /// `StoreLane::Tasks`; left/right (h/l) cycle through `StoreLane::ALL`.
    pub focused_store: StoreLane,
}

impl App {
    pub fn new(opts: TuiOpts) -> Self {
        let claude_bin = opts
            .claude_bin
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude"));
        Self {
            opts,
            viewport_height: 20,
            claude_bin,
            ..Default::default()
        }
    }

    /// Test-only override of the claude binary path. Mirrors
    /// `ClaudeCodeRunner::with_bin`.
    pub fn with_bin(mut self, p: PathBuf) -> Self {
        self.claude_bin = p;
        self
    }

    /// Capture the user-visible view state for restore after a side-car.
    pub fn snapshot_view(&self) -> PreservedView {
        PreservedView {
            selection: self.selection,
            sort: self.sort,
            filter: self.filter.clone(),
            collapsed: self.collapsed.clone(),
            scroll_offset: self.scroll_offset,
            focused_store: self.focused_store,
        }
    }

    /// Restore a view snapshot post-hand-off. Re-applies the sort against
    /// possibly-refreshed `rows`.
    pub fn restore_view(&mut self, snap: PreservedView) {
        self.selection = snap.selection;
        self.sort = snap.sort;
        self.filter = snap.filter;
        self.collapsed = snap.collapsed;
        self.scroll_offset = snap.scroll_offset;
        self.focused_store = snap.focused_store;
        self.apply_sort();
        self.clamp_selection();
    }

    /// Reload rows and rebuild sections (then re-apply current sort).
    pub fn refresh(&mut self, conn: &Connection) -> Result<()> {
        self.rows = super::data::load_rows(conn)?;
        self.external_review = super::data::load_external_review_state(conn)?;
        self.system_health = super::data::load_system_health(conn)?;
        self.sections = classify_with_options(&self.rows, self.watch_classify_options());
        let (rows, sections) =
            super::data::dedup_observation_summaries_by_section(&self.rows, &self.sections);
        self.rows = rows;
        self.sections = sections;
        self.apply_sort();
        self.recompute_status_bar();
        Ok(())
    }

    /// Refresh the status-bar derived fields (cap counts, daemon liveness,
    /// clock). Caller-provided `db_path` and pidfile path stay sticky between
    /// refreshes.
    pub fn recompute_status_bar(&mut self) {
        let total = self
            .rows
            .iter()
            .filter(|r| matches!(r, Row::Task(_)))
            .count();
        let active = self
            .rows
            .iter()
            .filter(|r| match r {
                Row::Task(t) => !is_terminal_task_status(&t.status),
                _ => false,
            })
            .count();
        self.status_bar.cap = (active, total);
        self.status_bar.clock = local_clock_string();
        if let Ok(p) = super::daemon::pidfile_path() {
            let live = super::daemon::liveness(&p);
            self.status_bar.daemon_liveness = live.clone();
            self.status_bar.daemon_pid = match live {
                Liveness::Live { pid } => Some(pid),
                Liveness::Dead => None,
            };
        }
    }

    fn watch_classify_options(&self) -> WatchClassifyOptions {
        WatchClassifyOptions {
            show_all_history: self.opts.all_history,
            ..Default::default()
        }
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

    /// Cycle the focused store lane by `direction` (+1 right, -1 left) with
    /// wrap-around across `StoreLane::ALL`. After moving, snap the selection
    /// to the first navigable row in the new lane (or `Selection::default()`
    /// if the lane is empty).
    pub fn cycle_focus(&mut self, direction: isize) {
        let n = StoreLane::ALL.len() as isize;
        let cur_idx = StoreLane::ALL
            .iter()
            .position(|&l| l == self.focused_store)
            .map(|p| p as isize)
            .unwrap_or(0);
        let new_idx = (cur_idx + direction).rem_euclid(n) as usize;
        self.focused_store = StoreLane::ALL[new_idx];
        self.scroll_offset = 0;
        let flat = self.flat_rows();
        if let Some(first) = flat.first() {
            self.selection = Selection {
                section: first.section,
                row: first.row,
            };
        } else {
            self.selection = Selection::default();
        }
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

    /// Compute the navigable flat-row list — rows that pass the filter, live
    /// in non-collapsed sections, and belong to the currently focused store
    /// lane. Walked in canonical section order so per-lane sub-taxonomy is
    /// preserved.
    pub fn flat_rows(&self) -> Vec<FlatRow> {
        let mut out = Vec::new();
        for (sec_idx, (sec, indices)) in self.sections.iter().enumerate() {
            if self.collapsed.contains(sec) {
                continue;
            }
            for (within, &abs) in indices.iter().enumerate() {
                let row = &self.rows[abs];
                if store_lane_for_row(row) != self.focused_store {
                    continue;
                }
                if self.filter.is_empty() || self.filter.matches(row, *sec) {
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

    /// Return the row under the current cursor.
    pub fn current_row(&self) -> Option<&Row> {
        let flat = self.flat_rows();
        let cursor = self.current_flat()?;
        let fr = flat.get(cursor)?;
        self.rows.get(fr.abs)
    }

    /// Find the flat-row index of the current selection (or None when the
    /// selection has fallen out of view due to filter/collapse changes).
    pub fn current_flat(&self) -> Option<usize> {
        let flat = self.flat_rows();
        flat.iter()
            .position(|f| f.section == self.selection.section && f.row == self.selection.row)
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

    /// Increment the per-session sidecar counter (status-bar readout).
    pub fn record_sidecar_spawn(&mut self) {
        self.sidecars_today = self.sidecars_today.saturating_add(1);
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

    /// Open a read-only detail page for the selected task/observation/intake row.
    pub fn open_detail_for_current(&mut self) {
        let Some(row) = self.current_row() else {
            return;
        };
        let kind = match row {
            Row::Task(_) => DetailKind::Task,
            Row::Obs(_) | Row::CollapsedObs(_) => DetailKind::Observation,
            Row::Review(_) => DetailKind::Review,
            Row::Intake(_) => DetailKind::Intake,
        };
        self.detail = Some(DetailSelection {
            display_id: row.display_id().to_string(),
            kind,
            scroll_offset: 0,
        });
        self.mode = Mode::Detail;
    }

    /// Leave the read-only detail page without changing substrate state.
    pub fn close_detail(&mut self) {
        self.detail = None;
        self.mode = Mode::Normal;
    }

    // ----------------------------------------------------------------------

    /// If filter/collapse made the current selection invalid, snap back to
    /// the first visible row.
    /// Toggle the `?` cheat-sheet popup.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

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

fn local_clock_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::super::data::{
        classify, store_lane_for_row, IntakeRow, ObsRow, ReviewRow, TaskRow,
    };
    use super::*;

    fn task(status: &str) -> Row {
        Row::Task(TaskRow {
            display_id: format!("T-{status}"),
            status: status.to_string(),
            title: "task".to_string(),
            claimed_by: None,
            updated_at: String::new(),
            tier_hint: None,
            linked_observations: Vec::new(),
            blocked_reason: None,
            blocked_reason_class: None,
            ..Default::default()
        })
    }

    fn obs(id: &str, summary: &str) -> Row {
        Row::Obs(ObsRow {
            display_id: id.to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: summary.to_string(),
            updated_at: "2026-05-05".to_string(),
            ..Default::default()
        })
    }

    fn intake(id: &str) -> Row {
        Row::Intake(IntakeRow {
            display_id: id.to_string(),
            status: "draft".to_string(),
            summary: id.to_string(),
            updated_at: "2026-05-05".to_string(),
            ..Default::default()
        })
    }

    fn review(id: &str) -> Row {
        Row::Review(ReviewRow {
            display_id: id.to_string(),
            task_id: "T001".to_string(),
            status: "running".to_string(),
            runner: "codex".to_string(),
            ..Default::default()
        })
    }

    fn five_lane_app() -> App {
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![
            intake("I001"),
            obs("L001", "obs-a"),
            obs("L002", "obs-b"),
            task("executing"),
            task("ready"),
            task("planning"),
            review("E001"),
        ];
        app.sections = classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 10;
        app
    }

    #[test]
    fn status_bar_counts_abandoned_as_terminal_history() {
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![task("executing"), task("abandoned"), task("rejected")];

        app.recompute_status_bar();

        assert_eq!(app.status_bar.cap, (1, 3));
    }

    #[test]
    fn abandoned_is_terminal_task_status() {
        assert!(is_terminal_task_status("abandoned"));
        assert!(is_terminal_task_status("closed_out_of_band"));
        assert!(is_terminal_task_status("rejected"));
        assert!(!is_terminal_task_status("executing"));
    }

    #[test]
    fn focused_store_defaults_to_tasks() {
        let app = App::new(TuiOpts::default());
        assert_eq!(app.focused_store, StoreLane::Tasks);
    }

    #[test]
    fn cycle_focus_wraps_in_both_directions() {
        let mut app = App::new(TuiOpts::default());
        // Default lane is Tasks (idx 2). Walk +1 across all five and wrap.
        assert_eq!(app.focused_store, StoreLane::Tasks);
        app.cycle_focus(1);
        assert_eq!(app.focused_store, StoreLane::ExternalReviews);
        app.cycle_focus(1);
        assert_eq!(app.focused_store, StoreLane::EngineHealth);
        app.cycle_focus(1);
        assert_eq!(app.focused_store, StoreLane::Intake);
        app.cycle_focus(1);
        assert_eq!(app.focused_store, StoreLane::Observations);
        app.cycle_focus(1);
        assert_eq!(app.focused_store, StoreLane::Tasks);

        // Reverse: -1 from Tasks wraps to EngineHealth via the front edge,
        // and from Intake wraps back to EngineHealth.
        app.cycle_focus(-1);
        assert_eq!(app.focused_store, StoreLane::Observations);
        app.cycle_focus(-1);
        assert_eq!(app.focused_store, StoreLane::Intake);
        app.cycle_focus(-1);
        assert_eq!(app.focused_store, StoreLane::EngineHealth);
        app.cycle_focus(-1);
        assert_eq!(app.focused_store, StoreLane::ExternalReviews);
        app.cycle_focus(-1);
        assert_eq!(app.focused_store, StoreLane::Tasks);
    }

    #[test]
    fn flat_rows_length_matches_focused_lane_for_each_lane() {
        let mut app = five_lane_app();

        let expected: Vec<(StoreLane, usize)> = vec![
            (StoreLane::Intake, 1),
            (StoreLane::Observations, 2),
            (StoreLane::Tasks, 3),
            (StoreLane::ExternalReviews, 1),
            (StoreLane::EngineHealth, 0),
        ];

        for (lane, expected_count) in expected {
            app.focused_store = lane;
            let flat = app.flat_rows();
            assert_eq!(
                flat.len(),
                expected_count,
                "lane {:?} should expose {} rows",
                lane,
                expected_count
            );
            for fr in &flat {
                let row = &app.rows[fr.abs];
                assert_eq!(
                    store_lane_for_row(row),
                    lane,
                    "flat_rows must only emit rows whose lane matches focused_store"
                );
            }
        }
    }

    #[test]
    fn cycle_focus_snaps_selection_to_first_row_in_new_lane() {
        let mut app = five_lane_app();
        // Land focus on a lane with rows; selection should land on the first
        // navigable row in that lane.
        app.focused_store = StoreLane::Intake;
        app.cycle_focus(1); // → Observations
        assert_eq!(app.focused_store, StoreLane::Observations);
        let flat = app.flat_rows();
        assert!(!flat.is_empty());
        let first = flat[0];
        assert_eq!(app.selection.section, first.section);
        assert_eq!(app.selection.row, first.row);
    }

    #[test]
    fn cycle_focus_to_empty_lane_resets_selection() {
        let mut app = five_lane_app();
        app.cycle_focus(1); // Tasks → ExternalReviews
        app.cycle_focus(1); // → EngineHealth (empty in flat_rows)
        assert_eq!(app.focused_store, StoreLane::EngineHealth);
        assert!(app.flat_rows().is_empty());
        assert_eq!(app.selection, Selection::default());
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn flat_rows_lane_invariant_holds_after_cycle_focus_across_all_lanes() {
        // AC2.3: After cycle_focus, app.flat_rows() must contain only rows
        // whose store_lane_for_row matches app.focused_store. Verified across
        // all five lanes by exercising cycle_focus as the state transition.
        let mut app = five_lane_app();
        assert_eq!(app.focused_store, StoreLane::Tasks);

        // Walk forward through all five lanes via cycle_focus(1) and assert
        // the invariant after each transition.
        let mut visited_forward: Vec<StoreLane> = Vec::new();
        for _ in 0..StoreLane::ALL.len() {
            let lane = app.focused_store;
            let flat = app.flat_rows();
            for fr in &flat {
                let row = &app.rows[fr.abs];
                assert_eq!(
                    store_lane_for_row(row),
                    lane,
                    "flat_rows must only emit rows whose store_lane_for_row matches focused_store ({:?})",
                    lane
                );
            }
            visited_forward.push(lane);
            app.cycle_focus(1);
        }
        // All five distinct lanes were visited and invariant held for each.
        let mut sorted_forward = visited_forward.clone();
        sorted_forward.sort_by_key(|l| StoreLane::ALL.iter().position(|x| x == l).unwrap());
        let mut all_lanes: Vec<StoreLane> = StoreLane::ALL.to_vec();
        all_lanes.sort_by_key(|l| StoreLane::ALL.iter().position(|x| x == l).unwrap());
        assert_eq!(sorted_forward, all_lanes);

        // And reverse: cycle_focus(-1) preserves the invariant too.
        let mut visited_backward: Vec<StoreLane> = Vec::new();
        for _ in 0..StoreLane::ALL.len() {
            let lane = app.focused_store;
            let flat = app.flat_rows();
            for fr in &flat {
                let row = &app.rows[fr.abs];
                assert_eq!(
                    store_lane_for_row(row),
                    lane,
                    "flat_rows lane invariant must hold after cycle_focus(-1) on lane {:?}",
                    lane
                );
            }
            visited_backward.push(lane);
            app.cycle_focus(-1);
        }
        let mut sorted_backward = visited_backward.clone();
        sorted_backward.sort_by_key(|l| StoreLane::ALL.iter().position(|x| x == l).unwrap());
        assert_eq!(sorted_backward, all_lanes);
    }
}
