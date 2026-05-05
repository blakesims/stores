//! Initial widget tree: rows pane (top), 1-line selected-row footer, 1-line
//! status bar.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::app::App;
use super::data::Row;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // rows pane
            Constraint::Length(1), // selected-row footer
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    draw_rows(f, app, chunks[0]);
    draw_selected_footer(f, app, chunks[1]);
    draw_status_bar(f, app, chunks[2]);
}

fn draw_rows(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut items: Vec<ListItem> = Vec::new();

    for (section, indices) in &app.sections {
        let header = Line::from(Span::styled(
            format!("{} ({})", section.label(), indices.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        items.push(ListItem::new(header));

        if indices.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (no rows)",
                Style::default().fg(Color::DarkGray),
            ))));
            continue;
        }

        for &i in indices {
            let row = &app.rows[i];
            items.push(ListItem::new(format_row_line(row)));
        }
    }

    let list =
        List::new(items).block(Block::default().borders(Borders::NONE).title("stores watch"));
    f.render_widget(list, area);
}

fn format_row_line(row: &Row) -> Line<'static> {
    match row {
        Row::Task(t) => Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<6}", t.display_id),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{:<14}", t.status),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::raw(truncate(&t.title, 60)),
        ]),
        Row::Obs(o) => Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<6}", o.display_id),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                format!("{:<14}", o.status),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::raw(truncate(&o.summary, 60)),
        ]),
    }
}

fn draw_selected_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = if app.rows.is_empty() {
        "no rows".to_string()
    } else {
        // Phase 1: just show row count; selection follow-through is in P2.
        format!("{} row(s) loaded", app.rows.len())
    };
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_status_bar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let daemon = match app.status_bar.daemon_pid {
        Some(pid) => format!("daemon: pid {pid}"),
        None => "daemon: ?".to_string(),
    };
    let line = Line::from(vec![
        Span::styled(
            " stores watch ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(daemon, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(
            "[q quit]",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
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
