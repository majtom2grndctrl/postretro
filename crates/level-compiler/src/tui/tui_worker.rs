//! Worker-thread orchestration, panic bridging, and terminal input for the bake TUI.
//! See: `context/lib/build_pipeline.md`.

use std::any::Any;
use std::io::{self, Stdout};
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::governor::Governor;
use crate::logger::LogSink;
use crate::pipeline::StageDescriptor;
use crate::reporter::Reporter;

use super::tui_render::{TuiPhase, draw};
use super::tui_terminal::TerminalSession;
use super::{LogScrollAction, TuiReporter};

const TICK: Duration = Duration::from_millis(80);

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Temporarily silence the process panic hook while the alternate screen owns
/// stdout. The panic payload is retained by `catch_unwind`/`JoinHandle`, then
/// reported once after terminal restoration. This is process-global, so its
/// lifetime is deliberately limited to the interactive session.
struct DeferredPanicHook {
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

/// Run a compiler bake on a worker while the caller thread exclusively owns
/// terminal input and rendering.
///
/// The caller owns CLI gating and supplies the parsed map's planned stages.
/// The closure keeps terminal orchestration independent from compile argument
/// ownership while making the worker boundary explicit.
pub fn run_tui<F>(
    planned: Vec<StageDescriptor>,
    logs: LogSink,
    governor: Arc<Governor>,
    bake: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    let panic_hook = DeferredPanicHook::install();
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        run_tui_session(planned, logs, governor, bake)
    }));
    drop(panic_hook);
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            eprintln!(
                "prl-build TUI panicked: {}",
                panic_message(payload.as_ref())
            );
            std::panic::resume_unwind(payload)
        }
    }
}

fn run_tui_session<F>(
    planned: Vec<StageDescriptor>,
    logs: LogSink,
    governor: Arc<Governor>,
    bake: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    let reporter = Arc::new(TuiReporter::new(&planned, logs));
    let mut session = TerminalSession::enter()?;
    let worker_reporter: Arc<dyn Reporter> = reporter.clone();
    let worker_governor = Arc::clone(&governor);
    let mut worker = Some(
        thread::Builder::new()
            .name("prl-build-tui-worker".to_owned())
            .spawn(move || bake(worker_reporter, worker_governor))?,
    );

    let render_outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        render_loop(
            &mut session.terminal,
            &reporter,
            &governor,
            worker.as_ref().expect("worker is available"),
        )
    }));

    let mut completed_outcome = None;
    let render_outcome = match render_outcome {
        Ok(Ok(RenderLoopExit::WorkerFinished)) => {
            let outcome = join_worker(worker.take().expect("finished worker is available"));
            let phase = match &outcome {
                WorkerOutcome::Completed(Ok(())) => TuiPhase::Done,
                WorkerOutcome::Completed(Err(_)) | WorkerOutcome::Panicked(_) => {
                    reporter.finalize_failure();
                    TuiPhase::Failed
                }
            };
            completed_outcome = Some(outcome);
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                wait_for_completion_key(&mut session.terminal, &reporter, &governor, phase)
            }))
        }
        other => other.map(|result| result.map(|_| ())),
    };
    // A quit or terminal error must never leave cooperative workers parked.
    governor.set_paused(false);
    drop(session);

    let worker_outcome = if let Some(outcome) = completed_outcome {
        outcome
    } else {
        // The interactive UI is gone but the worker is still baking. Announce the
        // detach on the restored normal screen so the blocking join below does not
        // read as a silent hang; cooked mode is back, so Ctrl-C aborts again.
        println!("UI detached — bake continues in the background; press Ctrl-C to abort.");
        join_worker(worker.take().expect("worker is available"))
    };
    finish_run(worker_outcome, render_outcome, &reporter)
}

enum WorkerOutcome {
    Completed(anyhow::Result<()>),
    Panicked(Box<dyn Any + Send + 'static>),
}

fn join_worker(worker: thread::JoinHandle<anyhow::Result<()>>) -> WorkerOutcome {
    match worker.join() {
        Ok(result) => WorkerOutcome::Completed(result),
        Err(payload) => WorkerOutcome::Panicked(payload),
    }
}

/// Arbitrate worker and terminal outcomes only after restoring the normal
/// screen. A compiler failure is the primary result; terminal failures are
/// secondary diagnostics and never replace the compiler's root cause.
fn finish_run(
    worker: WorkerOutcome,
    render: Result<io::Result<()>, Box<dyn Any + Send>>,
    reporter: &TuiReporter,
) -> anyhow::Result<()> {
    match worker {
        WorkerOutcome::Completed(result) => {
            if result.is_err() {
                reporter.finalize_failure();
            }
            reporter.print_final_summary();
            if let Err(error) = result {
                match render {
                    Ok(Err(terminal_error)) => {
                        eprintln!("TUI also encountered a terminal error: {terminal_error}");
                    }
                    Err(payload) => {
                        eprintln!(
                            "TUI also panicked while presenting the compiler failure: {}",
                            panic_message(payload.as_ref())
                        );
                    }
                    Ok(Ok(())) => {}
                }
                return Err(error);
            }

            match render {
                Ok(result) => result.map_err(Into::into),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        WorkerOutcome::Panicked(payload) => {
            reporter.finalize_failure();
            reporter.print_final_summary();
            std::panic::resume_unwind(payload)
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
}

fn render_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    reporter: &TuiReporter,
    governor: &Governor,
    worker: &thread::JoinHandle<anyhow::Result<()>>,
) -> io::Result<RenderLoopExit> {
    let max_permits = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    clamp_governor_permits(governor, max_permits);

    while !worker.is_finished() {
        reporter.drain_logs();
        terminal.draw(|frame| draw(frame, reporter, governor, max_permits, TuiPhase::Running))?;

        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Raw mode disables ISIG, so Ctrl-C arrives as a key event
                    // rather than SIGINT. Treat it as the same "leave the UI"
                    // request as q/Esc so the operator is never trapped.
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(RenderLoopExit::LeftEarly);
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(RenderLoopExit::LeftEarly);
                        }
                        KeyCode::Char(' ') | KeyCode::Char('p') => {
                            governor.set_paused(!governor.is_paused());
                        }
                        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right | KeyCode::Up => {
                            change_permits(governor, max_permits, PermitChange::Increase);
                        }
                        KeyCode::Char('-') | KeyCode::Left | KeyCode::Down => {
                            change_permits(governor, max_permits, PermitChange::Decrease);
                        }
                        KeyCode::PageUp => {
                            reporter.lock().log_scroll.scroll(LogScrollAction::PageUp)
                        }
                        KeyCode::PageDown => {
                            reporter.lock().log_scroll.scroll(LogScrollAction::PageDown)
                        }
                        KeyCode::Home => reporter.lock().log_scroll.scroll(LogScrollAction::Home),
                        KeyCode::End => reporter.lock().log_scroll.scroll(LogScrollAction::End),
                        _ => {}
                    }
                }
                // The next draw uses the new frame area and clears stale cells.
                Event::Resize(_, _) => terminal.clear()?,
                _ => {}
            }
        }
    }
    reporter.drain_logs();
    Ok(RenderLoopExit::WorkerFinished)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderLoopExit {
    WorkerFinished,
    LeftEarly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionInput {
    Exit,
    Redraw,
    Ignore,
}

fn completion_input(event: &Event) -> CompletionInput {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => CompletionInput::Exit,
        Event::Resize(_, _) => CompletionInput::Redraw,
        _ => CompletionInput::Ignore,
    }
}

fn wait_for_completion_key(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    reporter: &TuiReporter,
    governor: &Governor,
    phase: TuiPhase,
) -> io::Result<()> {
    let max_permits = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    reporter.drain_logs();

    loop {
        terminal.draw(|frame| draw(frame, reporter, governor, max_permits, phase))?;
        match completion_input(&event::read()?) {
            CompletionInput::Exit => return Ok(()),
            CompletionInput::Redraw => terminal.clear()?,
            CompletionInput::Ignore => {}
        }
    }
}

#[derive(Clone, Copy)]
enum PermitChange {
    Increase,
    Decrease,
}

fn clamp_governor_permits(governor: &Governor, max_permits: usize) {
    let permits = governor.permits();
    let clamped = permits.clamp(1, max_permits.max(1));
    if clamped != permits {
        governor.set_permits(clamped);
    }
}

fn change_permits(governor: &Governor, max_permits: usize, change: PermitChange) {
    let max_permits = max_permits.max(1);
    let current = governor.permits().clamp(1, max_permits);
    let permits = match change {
        PermitChange::Increase => current.saturating_add(1).min(max_permits),
        PermitChange::Decrease => current.saturating_sub(1).max(1),
    };
    governor.set_permits(permits);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_capacity_text(permits: usize, max_permits: usize, width: u16) -> String {
        super::super::tui_render::core_capacity_spans(permits, max_permits, width)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn completion_input_exits_only_for_key_presses_and_redraws_on_resize() {
        use crossterm::event::KeyEvent;

        let pressed = Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let released = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        assert_eq!(completion_input(&pressed), CompletionInput::Exit);
        assert_eq!(completion_input(&released), CompletionInput::Ignore);
        assert_eq!(
            completion_input(&Event::Resize(100, 40)),
            CompletionInput::Redraw
        );
        assert_eq!(
            completion_input(&Event::FocusGained),
            CompletionInput::Ignore
        );
    }

    #[test]
    fn core_controls_clamp_high_initial_permits_and_saturate_at_bounds() {
        let governor = Governor::new(usize::MAX, false);
        clamp_governor_permits(&governor, 8);
        assert_eq!(governor.permits(), 8);

        change_permits(&governor, 8, PermitChange::Increase);
        assert_eq!(governor.permits(), 8);
        change_permits(&governor, 8, PermitChange::Decrease);
        assert_eq!(governor.permits(), 7);

        governor.set_permits(1);
        change_permits(&governor, 8, PermitChange::Decrease);
        assert_eq!(governor.permits(), 1);
        change_permits(&governor, 8, PermitChange::Increase);
        assert_eq!(governor.permits(), 2);
    }

    #[test]
    fn core_capacity_indicator_changes_when_arrow_controls_change_permits() {
        let governor = Governor::new(2, false);
        let before = core_capacity_text(governor.permits(), 4, 40);
        change_permits(&governor, 4, PermitChange::Increase);
        let after = core_capacity_text(governor.permits(), 4, 40);

        assert_eq!(before.matches('\u{25a0}').count(), 2);
        assert_eq!(after.matches('\u{25a0}').count(), 3);
        assert_ne!(before, after);
    }

    #[test]
    fn panic_payload_is_available_for_normal_screen_diagnostics() {
        let owned: Box<dyn Any + Send> = Box::new(String::from("worker failed"));
        let borrowed: Box<dyn Any + Send> = Box::new("borrowed failure");
        let opaque: Box<dyn Any + Send> = Box::new(7_u32);

        assert_eq!(panic_message(owned.as_ref()), "worker failed");
        assert_eq!(panic_message(borrowed.as_ref()), "borrowed failure");
        assert_eq!(panic_message(opaque.as_ref()), "non-string panic payload");
    }
}
