//! Panic-safe ownership of crossterm's alternate-screen and raw-mode state.
//! See: `context/lib/build_pipeline.md`.

use std::any::Any;
use std::io::{self, Stdout, Write};
use std::panic::PanicHookInfo;

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

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Temporarily silence the process panic hook while the alternate screen owns
/// stdout. A caught panic is reported after this session restores the normal
/// terminal. This is process-global, so the guard travels with the one
/// interactive session rather than each individual screen within it.
pub(super) struct DeferredPanicHook {
    previous: Option<PanicHook>,
}

impl DeferredPanicHook {
    fn install() -> Self {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self {
            previous: Some(previous),
        }
    }
}

impl Drop for DeferredPanicHook {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(previous);
        }
    }
}

pub(super) fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
}

/// One contiguous interactive lifetime: setup, bake progress, and terminal
/// cleanup. Keeping this value alive between screens prevents crossterm from
/// briefly restoring the normal terminal before the bake view begins.
pub(crate) struct TuiSession {
    pub(super) terminal: TerminalSession,
    // Field order matters: restore the terminal before re-installing the
    // caller's panic hook when the session is dropped.
    pub(super) _panic_hook: DeferredPanicHook,
}

impl TuiSession {
    pub(crate) fn enter() -> io::Result<Self> {
        let terminal = TerminalSession::enter()?;
        // Terminal acquisition is fallible, so install the process-wide hook
        // only after it succeeds. This keeps the hook guard out of any
        // potential acquisition unwind.
        let panic_hook = DeferredPanicHook::install();
        Ok(Self {
            terminal,
            _panic_hook: panic_hook,
        })
    }
}
