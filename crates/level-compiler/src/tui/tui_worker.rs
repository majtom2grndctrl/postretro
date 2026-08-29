//! Worker-thread orchestration, panic bridging, and terminal input for the bake TUI.
//! See: `context/lib/build_pipeline.md`.

use std::any::Any;
use std::io::{self, Stdout};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::governor::Governor;
use crate::logger::LogSink;
use crate::pipeline::StageDescriptor;
use crate::reporter::Reporter;

use super::tui_render::{TuiPhase, draw};
use super::tui_terminal::{TerminalSession, TuiSession, panic_message};
use super::{LogScrollAction, TuiReporter};

const TICK: Duration = Duration::from_millis(80);

/// Run a compiler bake on a worker while the caller thread exclusively owns
/// terminal input and rendering.
///
/// The caller owns CLI gating and supplies the parsed map's planned stages.
/// The closure keeps terminal orchestration independent from compile argument
/// ownership while making the worker boundary explicit.
pub(crate) fn run_tui<F>(
    planned: Vec<StageDescriptor>,
    logs: LogSink,
    governor: Arc<Governor>,
    bake: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    let session = TuiSession::enter().inspect_err(|_| logs.print_warning_summary())?;
    run_tui_after_config(session, logs.clone(), move || {
        (planned, logs, governor, bake)
    })
}

/// Keep the alternate-screen session alive while constructing the bake work,
/// then run that work under the same panic boundary. The configuration path
/// uses this entry point so cache setup cannot panic with the hook suppressed
/// but outside the recovery boundary.
pub(crate) fn run_tui_after_config<P, F>(
    session: TuiSession,
    recovery_logs: LogSink,
    prepare: P,
) -> anyhow::Result<()>
where
    P: FnOnce() -> (Vec<StageDescriptor>, LogSink, Arc<Governor>, F),
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    // Keep the hook guard outside `catch_unwind`: dropping it calls
    // `std::panic::set_hook`, which must only happen after any terminal panic
    // has been caught. The terminal itself moves into the catch boundary so
    // its Drop still restores cooked mode during the unwind.
    let TuiSession {
        terminal,
        _panic_hook: panic_hook,
    } = session;
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let (planned, logs, governor, bake) = prepare();
        run_tui_session(terminal, planned, logs, governor, bake)
    }));
    drop(panic_hook);
    match outcome {
        Ok(result) => result,
        // A panic reaches here re-raised from one of two origins. `finish_run`
        // wraps a bake-worker payload in `WorkerPanicPayload`; a render/TUI
        // panic is re-raised bare. Label each honestly, then resume with the
        // original payload so the true panic still propagates. The terminal is
        // already cooked (session dropped in `run_tui_session`), so printing on
        // the normal screen here is safe.
        Err(payload) => {
            // `terminal` was moved into the catch boundary, so it has already
            // restored cooked mode; the hook was restored immediately above.
            // This clone shares the reporter's exactly-once summary claim.
            recovery_logs.print_warning_summary();
            match payload.downcast::<WorkerPanicPayload>() {
                Ok(worker) => {
                    let inner = worker.0;
                    eprintln!(
                        "prl-build panicked during the bake: {}",
                        panic_message(inner.as_ref())
                    );
                    std::panic::resume_unwind(inner)
                }
                Err(payload) => {
                    eprintln!(
                        "prl-build panicked in the TUI: {}",
                        panic_message(payload.as_ref())
                    );
                    std::panic::resume_unwind(payload)
                }
            }
        }
    }
}

fn run_tui_session<F>(
    mut terminal_session: TerminalSession,
    planned: Vec<StageDescriptor>,
    logs: LogSink,
    governor: Arc<Governor>,
    bake: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    // The interactive reporter takes ownership of the sink; keep a clone so a
    // post-detach fallback can drain the same shared queue. Capture the
    // requested `-j` before the render loop clamps it, so a detach can restore
    // the operator's throttle rather than leaving it at a UI-lowered count.
    let detach_logs = logs.clone();
    let requested_permits = governor.permits();
    let reporter = Arc::new(TuiReporter::new(&planned, logs));
    let worker_reporter: Arc<dyn Reporter> = reporter.clone();
    let worker_governor = Arc::clone(&governor);
    let mut worker = match thread::Builder::new()
        .name("prl-build-tui-worker".to_owned())
        .spawn(move || bake(worker_reporter, worker_governor))
    {
        Ok(worker) => Some(worker),
        Err(error) => {
            // `TuiReporter` prints its warning summary on Drop. Restore the
            // normal terminal first so that fallback presentation cannot land
            // in the alternate screen.
            drop(terminal_session);
            return Err(error.into());
        }
    };

    let render_outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        render_loop(
            &mut terminal_session.terminal,
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
                wait_for_completion_key(&mut terminal_session.terminal, &reporter, &governor, phase)
            }))
        }
        other => other.map(|result| result.map(|_| ())),
    };
    // A quit or terminal error must never leave cooperative workers parked.
    governor.set_paused(false);
    drop(terminal_session);

    let worker_outcome = if let Some(outcome) = completed_outcome {
        outcome
    } else {
        // The interactive UI is gone but the worker is still baking. Announce why on
        // the restored normal screen so the blocking join below does not read as a
        // silent hang; cooked mode is back, so Ctrl-C aborts again. Only an
        // operator-initiated leave is a true "detach" — a terminal error or a
        // render-loop panic must not masquerade as one. Match by reference; the
        // render outcome stays intact for finish_run, which re-raises any panic.
        let worker = worker.take().expect("worker is available");
        match &render_outcome {
            // Operator quit (q/Esc/Ctrl-C): render_loop returned LeftEarly. Restore
            // the requested `-j` so the background bake is not stuck at a permit
            // count the operator lowered in the UI, then report the honest core
            // count instead of implying an unthrottled background run.
            Ok(Ok(())) => {
                let max_permits = thread::available_parallelism()
                    .map(std::num::NonZeroUsize::get)
                    .unwrap_or(1);
                let restored = requested_permits.clamp(1, max_permits);
                governor.set_permits(restored);
                println!(
                    "UI detached — bake continues in the background at {restored} core \
                     permit(s) (throttle restored to -j); press Ctrl-C to abort."
                );
            }
            // Terminal I/O error or render-loop panic: the UI crashed, it did not detach.
            Ok(Err(_)) | Err(_) => println!(
                "TUI stopped due to an error — bake continues in the background \
                 (cooked mode restored, press Ctrl-C to abort); the error will be \
                 reported once it finishes."
            ),
        }
        // The interactive UI is gone, so nothing else drains the shared sink and
        // the background bake would otherwise run silently until it joins. Degrade
        // to PlainReporter-style discrete output: stream drained log records plus
        // occasional progress/heartbeat lines to the cooked screen while the worker
        // runs. Display-only — no counter feeds back into bake logic, so output
        // stays byte-identical. The final warning tally and build summary remain
        // owned by the finalize/Drop path; this only bridges detach to completion.
        stream_detached_bake(&reporter, &detach_logs, &worker);
        join_worker(worker)
    };
    finish_run(worker_outcome, render_outcome, &reporter)
}

enum WorkerOutcome {
    Completed(anyhow::Result<()>),
    Panicked(Box<dyn Any + Send + 'static>),
}

/// Wraps a panic payload re-raised from the bake worker so the outer `run_tui`
/// handler can label it a bake panic rather than a TUI panic. Only the worker
/// re-raise path wraps; a render/TUI panic propagates bare.
struct WorkerPanicPayload(Box<dyn Any + Send + 'static>);

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
            match render {
                Ok(Err(terminal_error)) => {
                    eprintln!("TUI also encountered a terminal error: {terminal_error}");
                }
                Err(payload) => {
                    eprintln!(
                        "TUI also panicked while presenting the compiler panic: {}",
                        panic_message(payload.as_ref())
                    );
                }
                Ok(Ok(())) => {}
            }
            // Tag the origin so the outer handler labels this a bake panic, not a
            // TUI panic, then resume with the original payload preserved inside.
            std::panic::resume_unwind(Box::new(WorkerPanicPayload(payload)))
        }
    }
}

/// Discrete progress snapshot for detached streaming: stage label, completed
/// count, and total once published.
type DetachProgress = (&'static str, usize, Option<usize>);

/// After an operator detach, stream the still-running bake to the restored
/// cooked screen. Drain the shared log sink and emit occasional discrete
/// progress lines so a long background bake is not silent, and so pending
/// records are not lost to the sink's cap. Poll cadence mirrors
/// `PlainReporter`'s progress monitor (500 ms) rather than busy-spinning.
///
/// Display-only: reads progress counters, never writes them, so bake output
/// stays byte-identical. The warning tally and build summary stay owned by the
/// finalize/Drop path; this only bridges detach to the worker join.
fn stream_detached_bake(
    reporter: &TuiReporter,
    logs: &LogSink,
    worker: &thread::JoinHandle<anyhow::Result<()>>,
) {
    const POLL: Duration = Duration::from_millis(500);
    const HEARTBEAT: Duration = Duration::from_secs(10);

    let mut last_progress: Option<DetachProgress> = None;
    let mut last_output = Instant::now();
    while !worker.is_finished() {
        let mut printed = drain_detached_logs(logs);
        let snapshot = detached_progress(reporter);
        if snapshot != last_progress {
            if let Some(line) = detached_progress_line(snapshot, false) {
                eprintln!("{line}");
                printed = true;
            }
            last_progress = snapshot;
        } else if last_output.elapsed() >= HEARTBEAT {
            if let Some(line) = detached_progress_line(snapshot, true) {
                eprintln!("{line}");
                printed = true;
            }
        }
        if printed {
            last_output = Instant::now();
        }
        thread::sleep(POLL);
    }
    // Flush records emitted between the final poll and worker exit before the
    // finalize path prints the summary.
    drain_detached_logs(logs);
}

/// Drain and print every pending record in the shared sink using the record's
/// existing display format. Returns whether anything was printed.
fn drain_detached_logs(logs: &LogSink) -> bool {
    let mut printed = false;
    for record in logs.drain() {
        eprintln!("{record}");
        printed = true;
    }
    printed
}

/// Snapshot the most-recently-begun active stage's progress for streaming.
fn detached_progress(reporter: &TuiReporter) -> Option<DetachProgress> {
    let state = reporter.lock();
    let index = state.active_index()?;
    let step = &state.steps[index];
    let (completed, total) = match &step.progress {
        Some(progress) => (progress.completed(), progress.total()),
        None => (0, None),
    };
    Some((step.label, completed, total))
}

/// Format a discrete progress line. `heartbeat` marks an unchanged repeat so a
/// silent stage still shows life.
fn detached_progress_line(snapshot: Option<DetachProgress>, heartbeat: bool) -> Option<String> {
    let (label, completed, total) = snapshot?;
    let body = match total {
        Some(total) => format!("  {label}: {completed}/{total}"),
        None => format!("  {label}: {completed} done"),
    };
    Some(if heartbeat {
        format!("{body}  (still working)")
    } else {
        body
    })
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
