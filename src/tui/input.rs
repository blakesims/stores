//! Keyboard dispatcher: routes a `KeyEvent` through the `Mode` state machine
//! and mutates `App`. Returns `KeyOutcome::Quit` when the user pressed `q`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Mode};
use super::data::Row;
use super::filter::FilterPalette;
use super::search::SearchState;
use super::sidecar::SidecarScope;

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
        Mode::ObsDraftConfirm => obs_draft_confirm_mode(app, ev),
    }
}

/// Display id of the row currently under the cursor (or None).
fn current_display_id(app: &App) -> Option<String> {
    let flat = app.flat_rows();
    let cursor = app.current_flat()?;
    let fr = flat.get(cursor)?;
    Some(match &app.rows[fr.abs] {
        Row::Task(t) => t.display_id.clone(),
        Row::Obs(o) => o.display_id.clone(),
    })
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

        // Help — toggle modal cheat-sheet.
        (KeyCode::Char('?'), _) => {
            app.toggle_help();
            KeyOutcome::Continue
        }

        // Daemon start.
        (KeyCode::Char('D'), _) => {
            match super::daemon::start_detached() {
                Ok(pid) => {
                    app.status_bar.daemon_pid = Some(pid);
                    app.status_bar.daemon_liveness = super::daemon::Liveness::Live { pid };
                    app.status_bar.message = format!("daemon started (pid {pid})");
                }
                Err(e) => {
                    app.status_bar.message = format!("daemon start failed: {e}");
                }
            }
            KeyOutcome::Continue
        }

        // Saved-view shortcuts.
        (KeyCode::Char(c @ '1'), _) | (KeyCode::Char(c @ '2'), _) | (KeyCode::Char(c @ '3'), _) => {
            app.apply_preset(c);
            KeyOutcome::Continue
        }

        // Side-car spawn keys.
        (KeyCode::Char('s'), _) => {
            if let Some(id) = current_display_id(app) {
                app.pending_spawn = Some(SidecarScope::PerRow {
                    display_id: id,
                    fresh: false,
                });
            } else {
                // No row under cursor → fall back to general orchestrator sidecar.
                app.pending_spawn = Some(SidecarScope::General);
            }
            KeyOutcome::Continue
        }
        (KeyCode::Char('S'), _) => {
            if let Some(id) = current_display_id(app) {
                app.pending_spawn = Some(SidecarScope::PerRow {
                    display_id: id,
                    fresh: true,
                });
            } else {
                app.status_bar.message = "no row selected (S requires a selected row)".to_string();
            }
            KeyOutcome::Continue
        }
        (KeyCode::Char('g'), _) => {
            app.pending_spawn = Some(SidecarScope::General);
            KeyOutcome::Continue
        }
        (KeyCode::Char('o'), _) => {
            app.pending_spawn = Some(SidecarScope::ObsDraft);
            KeyOutcome::Continue
        }

        _ => KeyOutcome::Continue,
    }
}

/// Confirm popup after an obs-drafting side-car returned with a draft on
/// disk. `y` files via `stores observations add`; `n` discards; `Esc`
/// also discards.
fn obs_draft_confirm_mode(app: &mut App, ev: KeyEvent) -> KeyOutcome {
    match ev.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(draft) = app.obs_draft_pending.take() {
                app.obs_draft_filing_request = Some(draft);
                app.last_obs_draft_action = Some("file".to_string());
            }
            app.mode = Mode::Normal;
            KeyOutcome::Continue
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            if let Some(draft) = app.obs_draft_pending.take() {
                let _ = std::fs::remove_file(&draft.draft_path);
                app.last_obs_draft_action = Some("discard".to_string());
            }
            app.mode = Mode::Normal;
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
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T002".to_string(),
                status: "executing".to_string(),
                title: "in flight".to_string(),
                claimed_by: None,
                updated_at: "2026-05-04".to_string(),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T003".to_string(),
                status: "deploy_blocked".to_string(),
                title: "blocked".to_string(),
                claimed_by: None,
                updated_at: "2026-05-03".to_string(),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L001".to_string(),
                status: "open".to_string(),
                priority: "high".to_string(),
                summary: "ratifiable".to_string(),
                updated_at: "2026-05-02".to_string(),
                contract_state: Some("ready".to_string()),
                ..Default::default()
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

        // Tab on first row collapses TasksActionableCurrentWork → flat list shrinks.
        let before = app.flat_rows().len();
        on_key(&mut app, key(KeyCode::Tab));
        let after = app.flat_rows().len();
        assert!(after < before, "tab should collapse the section");
        assert!(app.collapsed.contains(&Section::TasksActionableCurrentWork));

        // Tab again → expand.
        on_key(&mut app, key(KeyCode::Tab));
        assert!(!app.collapsed.contains(&Section::TasksActionableCurrentWork));
    }

    #[test]
    fn quits_on_q() {
        let mut app = fresh_app();
        assert_eq!(on_key(&mut app, key(KeyCode::Char('q'))), KeyOutcome::Quit);
    }

    #[test]
    fn s_spawns_per_row_when_selected() {
        // 's' with a row selected → pending_spawn is PerRow with that display_id.
        let mut app = fresh_app();
        // First row is T001 (tasks_and_obs order + sort).
        on_key(&mut app, key(KeyCode::Char('s')));
        match app.pending_spawn.take() {
            Some(SidecarScope::PerRow { display_id, fresh }) => {
                assert!(!display_id.is_empty(), "display_id must be set");
                assert!(!fresh, "'s' key must use fresh=false (resume mode)");
            }
            other => panic!("expected PerRow pending_spawn, got {other:?}"),
        }
    }

    #[test]
    fn s_falls_back_to_general_when_no_selection() {
        // 's' with no row in the list → pending_spawn is General (not a status-bar message).
        let mut app = App::new(TuiOpts::default());
        // No rows loaded; flat list is empty; current_display_id returns None.
        on_key(&mut app, key(KeyCode::Char('s')));
        assert_eq!(
            app.pending_spawn,
            Some(SidecarScope::General),
            "empty-list 's' must fall back to SidecarScope::General"
        );
    }

    #[test]
    fn g_always_spawns_general() {
        let mut app = fresh_app();
        on_key(&mut app, key(KeyCode::Char('g')));
        assert_eq!(app.pending_spawn, Some(SidecarScope::General));
    }
}
