use std::io;

use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub(in crate::tui) struct TerminalGuard {
    disarmed: bool,
}

impl TerminalGuard {
    pub(in crate::tui) fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self { disarmed: false })
    }

    pub(in crate::tui) fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
    }
}
