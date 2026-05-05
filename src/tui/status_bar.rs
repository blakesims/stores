//! Always-visible bottom status bar: db path, clock, cap free/total, daemon
//! liveness, sidecars-today counter.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::app::{App, Mode};
use super::daemon::Liveness;

/// Render the status bar into `area`.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let line = build_line(app);
    f.render_widget(Paragraph::new(line), area);
}

fn build_line(app: &App) -> Line<'static> {
    let text = render_text(app);
    Line::from(vec![
        Span::styled(
            " stores watch ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {text}")),
    ])
}

/// Plain-text rendering of the status bar (snapshot tests assert against this).
pub fn render_text(app: &App) -> String {
    let db = app.status_bar.db_path.as_deref().unwrap_or("?");
    let clock = app.status_bar.clock.as_str();
    let (active, total) = app.status_bar.cap;
    let free = total.saturating_sub(active);
    let daemon = match app.status_bar.daemon_liveness {
        Liveness::Live { pid } => format!("daemon:LIVE pid={pid}"),
        Liveness::Dead => "daemon:DEAD".to_string(),
    };
    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Filter => "FILTER",
        Mode::Search => "SEARCH",
        Mode::ObsDraftConfirm => "OBS-CONFIRM",
    };
    format!(
        "db={db}  clock={clock}  cap {free}/{total} (active {active})  {daemon}  sidecars:{sc}  [{mode}]",
        sc = app.sidecars_today,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, StatusBar, TuiOpts};
    use crate::tui::daemon::Liveness;

    #[test]
    fn snapshot() {
        // AC3.3: db path, clock, cap free/total, daemon liveness all present.
        let mut app = App::new(TuiOpts::default());
        app.status_bar = StatusBar {
            db_path: Some(".stores/db.sqlite".to_string()),
            clock: "12:00:00 UTC".to_string(),
            cap: (3, 12),
            daemon_liveness: Liveness::Live { pid: 4242 },
            ..Default::default()
        };
        app.sidecars_today = 2;
        let text = render_text(&app);
        assert_eq!(
            text,
            "db=.stores/db.sqlite  clock=12:00:00 UTC  cap 9/12 (active 3)  daemon:LIVE pid=4242  sidecars:2  [NORMAL]"
        );
    }

    #[test]
    fn snapshot_dead_daemon() {
        let mut app = App::new(TuiOpts::default());
        app.status_bar = StatusBar {
            db_path: Some(".stores/db.sqlite".to_string()),
            clock: "00:00:00 UTC".to_string(),
            cap: (0, 0),
            daemon_liveness: Liveness::Dead,
            ..Default::default()
        };
        let text = render_text(&app);
        assert!(text.contains("daemon:DEAD"));
        assert!(text.contains("cap 0/0"));
    }
}
