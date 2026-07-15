//! Interactive terminal presentation for compiler progress.

use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::governor::Governor;
use crate::logger::{CapturedRecord, LogSink};
use crate::pipeline::{StageDescriptor, StageId};
use crate::reporter::{Reporter, StageProgress};

mod tui_progress;
mod tui_terminal;
use tui_progress::progress_text;
use tui_terminal::TerminalSession;

const TICK: Duration = Duration::from_millis(80);
const MIN_FULL_WIDTH: u16 = 68;
const MIN_FULL_HEIGHT: u16 = 18;
const MAX_LOG_RECORDS: usize = 500;
const PRIMARY: Color = Color::Cyan;
const SECONDARY: Color = Color::Blue;
const DIVIDER: Color = Color::DarkGray;
const SPINNER: [char; 4] = ['\u{25d0}', '\u{25d3}', '\u{25d1}', '\u{25d2}'];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepStatus {
    Pending,
    Active,
    Done,
    Skipped,
}

#[derive(Clone, Debug)]
struct StepState {
    id: StageId,
    label: &'static str,
    status: StepStatus,
    progress: Option<StageProgress>,
    started: Option<Instant>,
    last_completed: usize,
    spinner_index: usize,
}

impl StepState {
    fn new(descriptor: StageDescriptor) -> Self {
        Self {
            id: descriptor.id,
            label: descriptor.label,
            status: StepStatus::Pending,
            progress: None,
            started: None,
            last_completed: 0,
            spinner_index: 0,
        }
    }

    fn observe_liveness(&mut self) {
        let Some(progress) = &self.progress else {
            return;
        };
        let completed = progress.completed();
        if completed != self.last_completed {
            self.last_completed = completed;
            self.spinner_index = (self.spinner_index + 1) % SPINNER.len();
        }
    }
}

#[derive(Default)]
struct FinalSummary {
    timings: Vec<(&'static str, Duration)>,
    total: Option<Duration>,
}

struct TuiState {
    steps: Vec<StepState>,
    logs: Vec<CapturedRecord>,
    final_summary: FinalSummary,
}

impl TuiState {
    fn new(planned: &[StageDescriptor]) -> Self {
        Self {
            steps: planned
                .iter()
                .copied()
                .filter(|stage| stage.predicted_present)
                .map(StepState::new)
                .collect(),
            logs: Vec::new(),
            final_summary: FinalSummary::default(),
        }
    }

    fn step_mut(&mut self, id: StageId) -> Option<&mut StepState> {
        self.steps.iter_mut().find(|step| step.id == id)
    }

    fn active_index(&self) -> Option<usize> {
        self.steps
            .iter()
            .position(|step| step.status == StepStatus::Active)
    }

    fn append_logs(&mut self, records: Vec<CapturedRecord>) {
        self.logs.extend(records);
        let overflow = self.logs.len().saturating_sub(MAX_LOG_RECORDS);
        if overflow > 0 {
            self.logs.drain(..overflow);
        }
    }
}

/// Reporter shared by the bake worker and the terminal render thread.
pub struct TuiReporter {
    state: Mutex<TuiState>,
    logs: LogSink,
}

impl TuiReporter {
    pub fn new(planned: &[StageDescriptor], logs: LogSink) -> Self {
        Self {
            state: Mutex::new(TuiState::new(planned)),
            logs,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TuiState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn drain_logs(&self) {
        let records = self.logs.drain();
        if !records.is_empty() {
            self.lock().append_logs(records);
        }
    }

    fn print_final_summary(&self) {
        self.drain_logs();
        let state = self.lock();
        if let Some(total) = state.final_summary.total {
            println!("\nBuild Summary:");
            for (name, duration) in &state.final_summary.timings {
                println!("  {: <15} {:>6.2}s", name, duration.as_secs_f32());
            }
            println!("  {: <15} {:>6.2}s", "Total", total.as_secs_f32());
        }
        drop(state);

        let warnings = self.logs.warnings();
        println!("\nWarnings: {}", self.logs.warning_count());
        for warning in warnings {
            println!("  {warning}");
        }
    }
}

impl Reporter for TuiReporter {
    fn begin_stage(&self, id: StageId, _label: &'static str) {
        let mut state = self.lock();
        if let Some(step) = state.step_mut(id) {
            step.status = StepStatus::Active;
            step.started = Some(Instant::now());
            step.spinner_index = 0;
            step.last_completed = 0;
        }
    }

    fn declare_progress(&self, id: StageId, progress: StageProgress) {
        if let Some(step) = self.lock().step_mut(id) {
            step.last_completed = progress.completed();
            step.progress = Some(progress);
        }
    }

    fn finish_stage(&self, id: StageId) {
        if let Some(step) = self.lock().step_mut(id) {
            step.status = StepStatus::Done;
        }
        self.drain_logs();
    }

    fn skip_stage(&self, id: StageId) {
        if let Some(step) = self.lock().step_mut(id) {
            step.status = StepStatus::Skipped;
        }
        self.drain_logs();
    }

    fn record_warning(&self, warning: CapturedRecord) {
        self.logs.record(warning);
        self.drain_logs();
    }

    fn finalize(&self, timings: &[(&'static str, Duration)], total: Duration) {
        self.drain_logs();
        let mut state = self.lock();
        state.final_summary.timings = timings.to_vec();
        state.final_summary.total = Some(total);
    }
}

/// Run a compiler bake on a worker while the caller thread exclusively owns
/// terminal input and rendering.
///
/// Task 5 supplies reporter selection and the parsed map's planned stages. The
/// closure keeps this entry point independent from CLI gating and argument
/// ownership while still making the thread boundary explicit.
pub fn run_tui<F>(
    planned: Vec<StageDescriptor>,
    logs: LogSink,
    governor: Arc<Governor>,
    bake: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Arc<dyn Reporter>, Arc<Governor>) -> anyhow::Result<()> + Send + 'static,
{
    let reporter = Arc::new(TuiReporter::new(&planned, logs));
    let worker_reporter: Arc<dyn Reporter> = reporter.clone();
    let worker_governor = Arc::clone(&governor);
    let worker = thread::spawn(move || bake(worker_reporter, worker_governor));

    let mut session = match TerminalSession::enter() {
        Ok(session) => session,
        Err(error) => {
            governor.set_paused(false);
            let bake_result = join_worker(worker);
            reporter.print_final_summary();
            bake_result?;
            return Err(error.into());
        }
    };

    let render_result = render_loop(&mut session.terminal, &reporter, &governor, &worker);
    // A quit or terminal error must never leave cooperative workers parked.
    governor.set_paused(false);
    drop(session);

    let bake_result = join_worker(worker);
    reporter.print_final_summary();
    render_result?;
    bake_result
}

fn join_worker(worker: thread::JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    match worker.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn render_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    reporter: &TuiReporter,
    governor: &Governor,
    worker: &thread::JoinHandle<anyhow::Result<()>>,
) -> io::Result<()> {
    let max_permits = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);

    while !worker.is_finished() {
        reporter.drain_logs();
        terminal.draw(|frame| draw(frame, reporter, governor, max_permits))?;

        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(' ') | KeyCode::Char('p') => {
                        governor.set_paused(!governor.is_paused());
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right | KeyCode::Up => {
                        governor.set_permits((governor.permits() + 1).min(max_permits));
                    }
                    KeyCode::Char('-') | KeyCode::Left | KeyCode::Down => {
                        governor.set_permits(governor.permits().saturating_sub(1).max(1));
                    }
                    _ => {}
                },
                // The next draw uses the new frame area and clears stale cells.
                Event::Resize(_, _) => terminal.clear()?,
                _ => {}
            }
        }
    }
    reporter.drain_logs();
    Ok(())
}

fn draw(
    frame: &mut ratatui::Frame<'_>,
    reporter: &TuiReporter,
    governor: &Governor,
    max_permits: usize,
) {
    let area = frame.area();
    let mut state = reporter.lock();
    for step in &mut state.steps {
        if step.status == StepStatus::Active {
            step.observe_liveness();
        }
    }

    if layout_mode(area) == LayoutMode::Compact {
        draw_compact(
            frame,
            area,
            &state,
            reporter.logs.warning_count(),
            governor,
            max_permits,
        );
    } else {
        draw_full(
            frame,
            area,
            &state,
            reporter.logs.warning_count(),
            governor,
            max_permits,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutMode {
    Full,
    Compact,
}

fn layout_mode(area: Rect) -> LayoutMode {
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        LayoutMode::Compact
    } else {
        LayoutMode::Full
    }
}

fn draw_full(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
    warning_count: usize,
    governor: &Governor,
    max_permits: usize,
) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(32), Constraint::Min(24)])
        .split(vertical[0]);

    draw_steps(frame, body[0], state);
    draw_logs(frame, body[1], state, warning_count);
    draw_controls(frame, vertical[1], governor, max_permits);
}

fn draw_steps(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let block = Block::default()
        .title(Span::styled(
            " Compile steps ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DIVIDER))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let footer_height = 3.min(inner.height);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(inner);
    let visible = sections[0].height as usize;
    let offset = active_scroll_offset(state.steps.len(), visible, state.active_index());
    let lines = state
        .steps
        .iter()
        .skip(offset)
        .take(visible)
        .map(step_line)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), sections[0]);

    if let Some(active) = state
        .active_index()
        .and_then(|index| state.steps.get(index))
    {
        let (progress, eta) = progress_text(active);
        let footer = Paragraph::new(vec![
            Line::from(Span::styled(progress, Style::default().fg(PRIMARY))),
            Line::from(Span::styled(eta, Style::default().fg(SECONDARY))),
        ]);
        frame.render_widget(footer, sections[1]);
    }
}

fn active_scroll_offset(total: usize, visible: usize, active: Option<usize>) -> usize {
    if visible == 0 || total <= visible {
        return 0;
    }
    let active = active.unwrap_or(0);
    active
        .saturating_sub(visible / 2)
        .min(total.saturating_sub(visible))
}

fn step_line(step: &StepState) -> Line<'static> {
    let (marker, style) = match step.status {
        StepStatus::Pending => ('\u{00b7}', Style::default().fg(DIVIDER)),
        StepStatus::Active => (
            SPINNER[step.spinner_index],
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        StepStatus::Done => ('\u{2713}', Style::default()),
        StepStatus::Skipped => ('\u{2013}', Style::default().fg(DIVIDER)),
    };
    Line::from(vec![
        Span::styled(format!("{marker} "), style),
        Span::styled(step.label, style),
    ])
}

fn draw_logs(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState, warning_count: usize) {
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(5)])
        .split(area);
    let log_block = Block::default()
        .title(Span::styled(
            " Build log ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(2));
    let log_inner = log_block.inner(regions[0]);
    let height = log_inner.height as usize;
    let lines = state
        .logs
        .iter()
        .skip(state.logs.len().saturating_sub(height))
        .map(log_line)
        .collect::<Vec<_>>();
    frame.render_widget(log_block, regions[0]);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), log_inner);

    let warning_block = Block::default()
        .title(Span::styled(
            format!(" Warnings ({warning_count}) "),
            Style::default().fg(SECONDARY).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(DIVIDER))
        .padding(Padding::horizontal(2));
    let warning_inner = warning_block.inner(regions[1]);
    let warning_height = warning_inner.height as usize;
    let warnings = state
        .logs
        .iter()
        .filter(|record| record.level <= log::Level::Warn)
        .rev()
        .take(warning_height)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|record| log_line(record))
        .collect::<Vec<_>>();
    frame.render_widget(warning_block, regions[1]);
    frame.render_widget(Paragraph::new(warnings), warning_inner);
}

fn log_line(record: &CapturedRecord) -> Line<'static> {
    let style = if record.level <= log::Level::Warn {
        Style::default().fg(SECONDARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(Span::styled(record.to_string(), style))
}

fn draw_controls(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    governor: &Governor,
    max_permits: usize,
) {
    let status = if governor.is_paused() {
        "PAUSED"
    } else {
        "RUNNING"
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(DIVIDER))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area).inner(Margin {
        horizontal: 0,
        vertical: 1,
    });
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{status}  "),
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("Cores  {} / {max_permits}", governor.permits())),
            Span::styled(
                "    [space] pause    [\u{2190}/\u{2192}] cores    [q] leave UI",
                Style::default().fg(SECONDARY),
            ),
        ])),
        inner,
    );
}

fn draw_compact(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
    warning_count: usize,
    governor: &Governor,
    max_permits: usize,
) {
    let active = state
        .active_index()
        .and_then(|index| state.steps.get(index));
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "prl-build  ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::raw(if governor.is_paused() {
            "PAUSED"
        } else {
            "RUNNING"
        }),
        Span::raw(format!(
            "  cores {}/{}  warnings {warning_count}",
            governor.permits(),
            max_permits
        )),
    ]));
    lines.push(Line::default());
    if let Some(step) = active {
        let (progress, eta) = progress_text(step);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}  ", SPINNER[step.spinner_index]),
                Style::default().fg(PRIMARY),
            ),
            Span::styled(step.label, Style::default().add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(format!("{progress}  {eta}")));
    }
    lines.push(Line::default());
    let available = area.height.saturating_sub(lines.len() as u16 + 1) as usize;
    lines.extend(
        state
            .logs
            .iter()
            .skip(state.logs.len().saturating_sub(available))
            .map(log_line),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().padding(Padding::horizontal(2)))
            .wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn descriptor(id: StageId) -> StageDescriptor {
        StageDescriptor {
            id,
            label: id.label(),
            predicted_present: true,
        }
    }

    #[test]
    fn absent_predicted_stages_are_omitted() {
        let mut absent = descriptor(StageId::SdfAtlasBake);
        absent.predicted_present = false;
        let state = TuiState::new(&[descriptor(StageId::Parsing), absent]);
        assert_eq!(state.steps.len(), 1);
        assert_eq!(state.steps[0].id, StageId::Parsing);
    }

    #[test]
    fn reporter_tracks_active_done_and_skipped_steps() {
        let reporter = TuiReporter::new(
            &[
                descriptor(StageId::Parsing),
                descriptor(StageId::DataScript),
            ],
            LogSink::default(),
        );
        reporter.begin_stage(StageId::Parsing, StageId::Parsing.label());
        reporter.finish_stage(StageId::Parsing);
        reporter.begin_stage(StageId::DataScript, StageId::DataScript.label());
        reporter.skip_stage(StageId::DataScript);

        let state = reporter.lock();
        assert_eq!(state.steps[0].status, StepStatus::Done);
        assert_eq!(state.steps[1].status, StepStatus::Skipped);
    }

    #[test]
    fn liveness_spinner_moves_only_when_progress_moves() {
        let progress = StageProgress::with_total(10);
        let mut step = StepState::new(descriptor(StageId::ShBake));
        step.status = StepStatus::Active;
        step.progress = Some(progress.clone());
        step.observe_liveness();
        assert_eq!(step.spinner_index, 0);
        progress.completed_handle().store(1, Ordering::Relaxed);
        step.observe_liveness();
        assert_eq!(step.spinner_index, 1);
        step.observe_liveness();
        assert_eq!(step.spinner_index, 1);
    }

    #[test]
    fn scrolling_keeps_active_step_visible() {
        assert_eq!(active_scroll_offset(21, 5, Some(0)), 0);
        let offset = active_scroll_offset(21, 5, Some(12));
        assert!(offset <= 12 && 12 < offset + 5);
        assert_eq!(active_scroll_offset(21, 5, Some(20)), 16);
    }

    #[test]
    fn compact_layout_activates_below_either_minimum() {
        assert_eq!(layout_mode(Rect::new(0, 0, 67, 30)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 100, 17)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 68, 18)), LayoutMode::Full);
    }

    #[test]
    fn progress_is_indeterminate_until_late_total_arrives() {
        let progress = StageProgress::indeterminate();
        let mut step = StepState::new(descriptor(StageId::DeltaShBake));
        step.progress = Some(progress.clone());
        step.started = Some(Instant::now());
        assert_eq!(progress_text(&step).0, "working  --%");
        progress.completed_handle().store(2, Ordering::Relaxed);
        progress.publish_total(8);
        assert_eq!(progress_text(&step).0, "2/8   25%");
    }
}
