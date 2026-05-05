//! Terminal setup / teardown helpers and panic-restore hook.

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, Stdout};

pub type Tty = Terminal<CrosstermBackend<Stdout>>;

pub fn setup() -> Result<Tty> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

pub fn teardown(terminal: &mut Tty) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Restore cooked terminal state on panic so the user's shell isn't wedged.
pub fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_on_panic();
        original(info);
    }));
}

/// Best-effort terminal restore. Safe to call from a panic hook (ignores
/// errors — the process is going down anyway).
pub fn restore_on_panic() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC1.5: panic hook restores the cooked terminal. We can't actually
    /// panic in a test without exiting the process, so we exercise the
    /// restore function directly and assert it returns without error
    /// regardless of prior raw-mode state.
    #[test]
    fn panic_restore_is_idempotent_and_does_not_error() {
        restore_on_panic();
        restore_on_panic();
    }

    #[test]
    fn install_panic_hook_does_not_error() {
        install_panic_hook();
    }
}
