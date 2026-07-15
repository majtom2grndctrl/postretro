//! Panic-safe ownership of crossterm's alternate-screen and raw-mode state.
//! See: `context/lib/build_pipeline.md`.

use std::io::{self, Stdout, Write};

use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

fn restore_terminal(writer: &mut impl Write) {
    let _ = execute!(writer, cursor::Show);
    let _ = execute!(writer, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

struct TerminalAcquisitionGuard {
    armed: bool,
}

impl TerminalAcquisitionGuard {
    fn new() -> Self {
        Self { armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalAcquisitionGuard {
    fn drop(&mut self) {
        if self.armed {
            restore_terminal(&mut io::stdout());
        }
    }
}

pub(super) struct TerminalSession {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    pub(super) fn enter() -> io::Result<Self> {
        let mut acquisition = TerminalAcquisitionGuard::new();
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        execute!(stdout, cursor::Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        acquisition.disarm();
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal(self.terminal.backend_mut());
    }
}
