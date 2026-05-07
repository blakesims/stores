//! Widget tree: rows pane (top), 1-line selected-row footer, 1-line
//! status bar, plus optional filter palette / search bar overlays.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use super::app::{App, FlatRow, Mode};
use super::data::{blocked_reason_class, surface_counts, Row};

pub fn draw(f: &mut Frame, app: &mut App) {
    // Search-mode adds an extra 1-line input bar above the status bar.
    let search_bar = if app.mode == Mode::Search { 1 } else { 0 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),             // rows pane
            Constraint::Length(1),          // selected-row footer
            Constraint::Length(search_bar), // search input
            Constraint::Length(1),          // hint line
            Constraint::Length(1),          // status bar
        ])
        .split(f.area());

    let rows_area = chunks[0];
    // Reserve one line of the rows pane per non-empty section header (and
    // collapsed sections still get a header). For viewport math we count
    // the row body lines, not headers.
    let viewport = rows_area.height.saturating_sub(0) as usize;
    app.viewport_height = viewport.max(1);

    let flat = app.flat_rows();
    clamp_scroll(app, flat.len());

    draw_rows(f, app, &flat, rows_area);
    super::footer::render(f, app, chunks[1]);

    if app.mode == Mode::Search {
        draw_search_bar(f, app, chunks[2]);
    }

    let hint = super::help::hint_for(app.mode);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[3],
    );

    super::status_bar::render(f, app, chunks[4]);

    if app.mode == Mode::Filter {
        draw_filter_palette(f, app);
    }
    if app.mode == Mode::ObsDraftConfirm {
        draw_obs_draft_popup(f, app);
    }
    if app.show_help {
        super::help::render_popup(f, app);
    }
}

fn draw_obs_draft_popup(f: &mut Frame, app: &App) {
    let draft = match &app.obs_draft_pending {
        Some(d) => d,
        None => return,
    };
    let area = centered_rect(70, 10, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" obs-draft (y to file, n/Esc to discard) ")
        .style(Style::default().fg(Color::Yellow));
    let body = Paragraph::new(vec![
        Line::from(format!("summary: {}", draft.summary)),
        Line::from(""),
        Line::from(truncate(&draft.body, 200)),
    ])
    .block(block);
    f.render_widget(body, area);
}

/// Slice the flat-row list to the current viewport window.
pub fn visible_window<'a>(app: &App, flat: &'a [FlatRow]) -> &'a [FlatRow] {
    let start = app.scroll_offset.min(flat.len());
    let end = (start + app.viewport_height).min(flat.len());
    &flat[start..end]
}

fn clamp_scroll(app: &mut App, total: usize) {
    if total == 0 {
        app.scroll_offset = 0;
        return;
    }
    let max_offset = total.saturating_sub(app.viewport_height.max(1));
    if app.scroll_offset > max_offset {
        app.scroll_offset = max_offset;
    }
}

fn draw_rows(f: &mut Frame, app: &App, flat: &[FlatRow], area: Rect) {
    // Render only the viewport window (virtualized). Section headers are
    // emitted lazily as the window crosses a section boundary.
    let window = visible_window(app, flat);
    let mut items: Vec<ListItem> = Vec::with_capacity(window.len());
    let mut last_section: Option<usize> = None;

    // Highlight the row currently under the cursor.
    let cursor = app.current_flat();

    for (i, fr) in window.iter().enumerate() {
        if last_section != Some(fr.section) {
            if let Some((sec, indices)) = app.sections.get(fr.section) {
                let collapsed = app.collapsed.contains(sec);
                let glyph = if collapsed { "▸" } else { "▾" };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("{glyph} {} ({})", sec.label(), indices.len()),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))));
            }
            last_section = Some(fr.section);
        }
        let absolute_idx = app.scroll_offset + i;
        let selected = cursor == Some(absolute_idx);
        items.push(ListItem::new(format_row_line(&app.rows[fr.abs], selected)));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no rows)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    let ((ta, tt), (oa, ot)) = surface_counts(&app.rows, app.opts.all_history);
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(
                "stores watch · TASKS {ta} actionable / {tt} total (use --all) · OBSERVATIONS {oa} actionable / {ot} total (use --all)"
            )),
    );
    f.render_widget(list, area);
}

fn format_row_line(row: &Row, selected: bool) -> Line<'static> {
    let base = match row {
        Row::Task(t) => vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<6}", t.display_id),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{:<24}", task_status_label(t)),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::raw(truncate(&t.title, 60)),
        ],
        Row::Obs(o) => {
            let status = if o.status == "investigation_failed" {
                match o.investigation_failure_reason.as_deref() {
                    Some(reason) if !reason.trim().is_empty() => {
                        format!("investigation_failed:{}", truncate(reason.trim(), 40))
                    }
                    _ => "investigation_failed:unknown".to_string(),
                }
            } else {
                o.status.clone()
            };
            vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<6}", o.display_id),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    format!("{:<24}", status),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::raw(truncate(&o.summary, 60)),
            ]
        }
    };
    if selected {
        let mut spans = base;
        for s in spans.iter_mut() {
            s.style = s.style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
        }
        Line::from(spans)
    } else {
        Line::from(base)
    }
}

fn task_status_label(t: &super::data::TaskRow) -> String {
    match t.status.as_str() {
        "closed_out_of_band" | "accepted" | "complete" | "cargo_installed" | "schema_migrated" => {
            "recovered/done".to_string()
        }
        "rejected" => "terminal-unless-amended".to_string(),
        "blocked" => format!(
            "blocked:{}",
            t.blocked_reason_class
                .as_deref()
                .unwrap_or_else(|| blocked_reason_class(t.blocked_reason.as_deref()))
        ),
        other => other.to_string(),
    }
}

fn draw_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        Span::styled("/", Style::default().fg(Color::Yellow)),
        Span::raw(app.search.query.clone()),
        Span::raw("  "),
        Span::styled(
            format!("{} hit(s)", app.search.hits.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_filter_palette(f: &mut Frame, app: &App) {
    let palette = match &app.filter_palette {
        Some(p) => p,
        None => return,
    };
    let area = centered_rect(60, 7, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" filter (state= priority= tier= since=, Enter applies, Esc cancels) ")
        .style(Style::default().fg(Color::Yellow));
    let body = Paragraph::new(vec![
        Line::from(format!("> {}", palette.buffer)),
        Line::from(""),
        Line::from(format!(
            "draft: state={:?} priority={:?} tier={:?} since={:?}",
            palette.draft.state, palette.draft.priority, palette.draft.tier, palette.draft.since
        )),
    ])
    .block(block);
    f.render_widget(body, area);
}

fn centered_rect(width_pct: u16, height: u16, area: Rect) -> Rect {
    let popup_w = area.width * width_pct / 100;
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x: popup_x,
        y: popup_y,
        width: popup_w,
        height,
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, TuiOpts};
    use crate::tui::data::{ObsRow, Row, TaskRow};

    fn line_text(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn task_row(status: &str, reason: Option<&str>) -> Row {
        Row::Task(TaskRow {
            display_id: "T999".to_string(),
            status: status.to_string(),
            title: "render status".to_string(),
            blocked_reason: reason.map(str::to_string),
            blocked_reason_class: Some(crate::tui::data::blocked_reason_class(reason).to_string()),
            ..Default::default()
        })
    }

    fn synthetic_rows(n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| {
                Row::Task(TaskRow {
                    display_id: format!("T{:03}", i),
                    status: "executing".to_string(),
                    title: format!("synthetic task {i}"),
                    claimed_by: None,
                    updated_at: format!("2026-05-{:02}", (i % 28) + 1),
                    ..Default::default()
                })
            })
            .collect()
    }

    /// AC2.5: with 200 synthetic rows and viewport=10, only 10 row widgets
    /// are rendered into the list, and selection stays in view on PgDn.
    #[test]
    fn task_terminal_and_blocked_status_labels_render_operator_actionably() {
        let closed = line_text(format_row_line(
            &task_row("closed_out_of_band", None),
            false,
        ));
        assert!(closed.contains("recovered/done"));
        assert!(!closed.contains("in flight"));

        let rejected = line_text(format_row_line(&task_row("rejected", None), false));
        assert!(rejected.contains("terminal-unless-amended"));
        assert!(!rejected.contains("in flight"));

        let blocked = line_text(format_row_line(
            &task_row("blocked", Some("rate limit 429")),
            false,
        ));
        assert!(blocked.contains("blocked:rate_limit"));
        let unknown = line_text(format_row_line(&task_row("blocked", Some("opaque")), false));
        assert!(unknown.contains("blocked:unknown"));
    }

    #[test]
    fn investigation_failed_observation_row_renders_reason() {
        let row = Row::Obs(ObsRow {
            display_id: "L065".to_string(),
            status: "investigation_failed".to_string(),
            priority: "high".to_string(),
            summary: "investigator failed".to_string(),
            investigation_failure_reason: Some("rate_limit: reset later".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(&row, false));
        assert!(text.contains("investigation_failed:rate_limit"), "{text}");
        assert!(text.contains("investigator failed"), "{text}");
    }

    #[test]
    fn virtual_scroll() {
        let mut app = App::new(TuiOpts::default());
        app.rows = synthetic_rows(200);
        app.sections = crate::tui::data::classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 10;
        app.selection = crate::tui::app::Selection {
            section: 0, /* TasksActionableCurrentWork */
            row: 0,
        };

        let flat = app.flat_rows();
        assert_eq!(flat.len(), 200);

        let window = visible_window(&app, &flat);
        assert_eq!(
            window.len(),
            10,
            "only viewport-height widgets should be emitted"
        );

        // PgDn moves selection by viewport_height and the scroll offset
        // tracks it (selection always lands within the visible window).
        crate::tui::input::on_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::PageDown,
                crossterm::event::KeyModifiers::NONE,
            ),
        );
        let cursor = app.current_flat().expect("cursor in flat list");
        assert!(
            cursor >= app.scroll_offset && cursor < app.scroll_offset + app.viewport_height,
            "selection (idx={cursor}) must stay inside viewport window \
             [{}, {}) after PgDn",
            app.scroll_offset,
            app.scroll_offset + app.viewport_height,
        );
    }
}
