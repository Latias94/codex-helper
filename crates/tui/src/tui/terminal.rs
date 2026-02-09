use std::io;
use std::io::IsTerminal;

use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub(in crate::tui) struct TerminalGuard {
    disarmed: bool,
}

impl TerminalGuard {
    pub(in crate::tui) fn enter() -> anyhow::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(anyhow::anyhow!(
                "TUI requires an interactive terminal (stdin/stdout must be a TTY)"
            ));
        }
        enable_raw_mode()?;
        if let Err(e) = crossterm::execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e.into());
        }
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
