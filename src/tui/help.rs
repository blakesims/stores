//! Modal cheat-sheet popup (`?`) and context-aware bottom hint line.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::app::{App, Mode};

const HELP_LINES: &[&str] = &[
    "stores watch — keyboard reference",
    "",
    "  j / k / ↑ / ↓     move selection",
    "  PgUp / PgDn       page selection",
    "  Tab               collapse/expand current section",
    "  ,                 cycle sort key",
    "  f / F             open filter palette / clear filter",
    "  / n N             search · next · prev",
    "  1 2 3             preset views (ratifiable / in-flight / deploy-blocked)",
    "  D                 start agents daemon (detached)",
    "  s / S             spawn per-row sidecar (resume / fresh)",
    "  g / o             general / obs-drafting sidecar",
    "  ?                 toggle this help",
    "  q / Ctrl-C        quit",
];

/// Render the help popup centred on the frame. Caller decides when to call
/// based on `app.show_help`.
pub fn render_popup(f: &mut Frame, _app: &App) {
    let area = centered_rect(60, (HELP_LINES.len() as u16) + 2, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" ? help ")
        .style(Style::default().fg(Color::Yellow));
    let body = Paragraph::new(
        HELP_LINES
            .iter()
            .map(|l| Line::from(*l))
            .collect::<Vec<_>>(),
    )
    .block(block);
    f.render_widget(body, area);
}

/// Context-aware single-line bottom hint, chosen from current mode.
pub fn hint_for(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "j/k nav  , sort  f filter  / search  D daemon  s/S/g/o sidecar  ? help  q quit",
        Mode::Filter => "type k=v (state= priority= tier= since=)  Enter apply  Esc cancel",
        Mode::Search => "type to search  Enter accept  Esc cancel  n/N next/prev",
        Mode::ObsDraftConfirm => "y file obs  n discard  Esc discard",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_differ_per_mode() {
        let n = hint_for(Mode::Normal);
        let f = hint_for(Mode::Filter);
        let s = hint_for(Mode::Search);
        assert!(n.contains("nav"));
        assert!(f.contains("Enter apply"));
        assert!(s.contains("next/prev"));
        assert_ne!(n, f);
        assert_ne!(f, s);
    }
}
