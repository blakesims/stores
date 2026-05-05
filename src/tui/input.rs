//! Keyboard dispatcher: routes a `KeyEvent` through the `Mode` state machine
//! and mutates `App`. Returns `KeyOutcome::Quit` when the user pressed `q`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Mode};
use super::filter::FilterPalette;
use super::search::SearchState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Continue,
    Quit,
}

pub fn on_key(app: &mut App, ev: KeyEvent) -> KeyOutcome {
    match app.mode {
        Mode::Normal => normal(app, ev),
        Mode::Filter => filter_mode(app, ev),
        Mode::Search => search_mode(app, ev),
    }
}

fn normal(app: &mut App, ev: KeyEvent) -> KeyOutcome {
    match (ev.code, ev.modifiers) {
        (KeyCode::Char('q'), _) => KeyOutcome::Quit,
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => KeyOutcome::Quit,

        // Navigation.
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
            app.move_selection(1);
            KeyOutcome::Continue
        }
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
            app.move_selection(-1);
            KeyOutcome::Continue
        }
        (KeyCode::PageDown, _) => {
            app.move_page(1);
            KeyOutcome::Continue
        }
        (KeyCode::PageUp, _) => {
            app.move_page(-1);
            KeyOutcome::Continue
        }
        (KeyCode::Tab, _) => {
            app.toggle_collapse_current();
            KeyOutcome::Continue
        }

        // Sort.
        (KeyCode::Char(','), _) => {
            app.cycle_sort();
            KeyOutcome::Continue
        }

        // Filter palette.
        (KeyCode::Char('f'), _) => {
            app.mode = Mode::Filter;
            app.filter_palette = Some(FilterPalette::open());
            KeyOutcome::Continue
        }
        (KeyCode::Char('F'), _) => {
            app.clear_filter();
            KeyOutcome::Continue
        }

        // Search.
        (KeyCode::Char('/'), _) => {
            app.mode = Mode::Search;
            app.search = SearchState::open();
            KeyOutcome::Continue
        }
        (KeyCode::Char('n'), _) => {
            app.search_next();
            KeyOutcome::Continue
        }
        (KeyCode::Char('N'), _) => {
            app.search_prev();
            KeyOutcome::Continue
        }

        // Help — no-op stub for now (Phase 2 only routes the key).
        (KeyCode::Char('?'), _) => KeyOutcome::Continue,

        // Saved-view shortcuts.
        (KeyCode::Char(c @ '1'), _)
        | (KeyCode::Char(c @ '2'), _)
        | (KeyCode::Char(c @ '3'), _) => {
            app.apply_preset(c);
            KeyOutcome::Continue
        }

        _ => KeyOutcome::Continue,
    }
}

fn filter_mode(app: &mut App, ev: KeyEvent) -> KeyOutcome {
    let palette = match app.filter_palette.as_mut() {
        Some(p) => p,
        None => {
            app.mode = Mode::Normal;
            return KeyOutcome::Continue;
        }
    };
    match ev.code {
        KeyCode::Esc => {
            app.cancel_filter();
        }
        KeyCode::Enter => {
            app.commit_filter();
        }
        KeyCode::Backspace => {
            palette.backspace();
        }
        KeyCode::Char(c) => {
            palette.push(c);
        }
        _ => {}
    }
    KeyOutcome::Continue
}

fn search_mode(app: &mut App, ev: KeyEvent) -> KeyOutcome {
    match ev.code {
        KeyCode::Esc => {
            app.search.close();
            app.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            // Leave hits intact so n/N keep working.
        }
        KeyCode::Backspace => {
            app.search.backspace();
            app.recompute_search();
        }
        KeyCode::Char(c) => {
            app.search.push(c);
            app.recompute_search();
        }
        _ => {}
    }
    KeyOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, TuiOpts};
    use crate::tui::data::{ObsRow, Row, Section, TaskRow};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn tasks_and_obs() -> Vec<Row> {
        vec![
            Row::Task(TaskRow {
                display_id: "T001".to_string(),
                status: "plan_review".to_string(),
                title: "needs review".to_string(),
                claimed_by: None,
                updated_at: "2026-05-05".to_string(),
            }),
            Row::Task(TaskRow {
                display_id: "T002".to_string(),
                status: "executing".to_string(),
                title: "in flight".to_string(),
                claimed_by: None,
                updated_at: "2026-05-04".to_string(),
            }),
            Row::Task(TaskRow {
                display_id: "T003".to_string(),
                status: "deploy_blocked".to_string(),
                title: "blocked".to_string(),
                claimed_by: None,
                updated_at: "2026-05-03".to_string(),
            }),
            Row::Obs(ObsRow {
                display_id: "L001".to_string(),
                status: "open".to_string(),
                priority: "high".to_string(),
                summary: "ratifiable".to_string(),
                updated_at: "2026-05-02".to_string(),
                contract_state: Some("ready".to_string()),
            }),
        ]
    }

    fn fresh_app() -> App {
        let mut app = App::new(TuiOpts::default());
        app.rows = tasks_and_obs();
        app.sections = crate::tui::data::classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 10;
        // Land selection on the first visible row.
        let flat = app.flat_rows();
        assert!(!flat.is_empty());
        app.selection = crate::tui::app::Selection {
            section: flat[0].section,
            row: flat[0].row,
        };
        app
    }

    #[test]
    fn nav() {
        // AC2.1: j/k/arrows/PgUp/PgDn move selection across sections.
        let mut app = fresh_app();
        let flat = app.flat_rows();
        // 4 selectable rows across 4 non-empty sections.
        assert_eq!(flat.len(), 4);

        // Start: first row.
        assert_eq!(app.current_flat(), Some(0));

        // j → second row (crosses sections).
        on_key(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.current_flat(), Some(1));

        // Down → third row.
        on_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.current_flat(), Some(2));

        // k → second row.
        on_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.current_flat(), Some(1));

        // PgDn (viewport=10) → clamps to last row.
        on_key(&mut app, key(KeyCode::PageDown));
        assert_eq!(app.current_flat(), Some(3));

        // PgUp → clamps to first row.
        on_key(&mut app, key(KeyCode::PageUp));
        assert_eq!(app.current_flat(), Some(0));

        // Tab on first row collapses TasksNeedsReview → flat list shrinks.
        let before = app.flat_rows().len();
        on_key(&mut app, key(KeyCode::Tab));
        let after = app.flat_rows().len();
        assert!(after < before, "tab should collapse the section");
        assert!(app.collapsed.contains(&Section::TasksNeedsReview));

        // Tab again → expand.
        on_key(&mut app, key(KeyCode::Tab));
        assert!(!app.collapsed.contains(&Section::TasksNeedsReview));
    }

    #[test]
    fn quits_on_q() {
        let mut app = fresh_app();
        assert_eq!(on_key(&mut app, key(KeyCode::Char('q'))), KeyOutcome::Quit);
    }
}
