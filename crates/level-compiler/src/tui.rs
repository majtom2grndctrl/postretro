//! TUI state model and the reporter that feeds compiler progress into it.
//! See: `context/lib/build_pipeline.md`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::logger::{CapturedRecord, LogSink};
use crate::pipeline::{StageDescriptor, StageId};
use crate::reporter::{Reporter, StageProgress};

mod tui_config;
mod tui_progress;
mod tui_render;
mod tui_steps;
mod tui_terminal;
mod tui_worker;

use tui_progress::RemainingEstimate;

pub(crate) use tui_config::{ConfigOutcome, run_config_screen};
pub(crate) use tui_worker::{run_tui, run_tui_after_config};

const MAX_LOG_RECORDS: usize = 500;
const ACTIVITY_FRAMES: [&str; 4] = [
    "\u{00b7}  ",
    "\u{00b7}\u{00b7} ",
    "\u{00b7}\u{00b7}\u{00b7}",
    "\u{00b7}\u{00b7} ",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepStatus {
    Pending,
    Active,
    Done,
    Skipped,
    Failed,
}

#[derive(Clone, Debug)]
struct StepState {
    id: StageId,
    label: &'static str,
    status: StepStatus,
    progress: Option<StageProgress>,
    started: Option<Instant>,
    last_completed: usize,
    activity_index: usize,
    remaining_estimate: RemainingEstimate,
    // Monotonic order in which this step most recently became active. Lets the
    // progress readout follow the newest active stage when stages overlap.
    begin_seq: u64,
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
            activity_index: 0,
            remaining_estimate: RemainingEstimate::default(),
            begin_seq: 0,
        }
    }

    fn observe_liveness(&mut self) {
        let Some(progress) = &self.progress else {
            return;
        };
        let completed = progress.completed();
        if completed != self.last_completed {
            self.last_completed = completed;
            self.activity_index = (self.activity_index + 1) % ACTIVITY_FRAMES.len();
        }
        if let Some(started) = self.started {
            self.remaining_estimate
                .observe(Instant::now(), started, completed, progress.total());
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
    log_revision: usize,
    log_scroll: LogScroll,
    final_summary: FinalSummary,
    next_begin_seq: u64,
}

#[derive(Default)]
struct LogScroll {
    lines_from_tail: usize,
    max_lines_from_tail: usize,
    page_size: usize,
    last_line_count: usize,
    last_revision: usize,
}

impl LogScroll {
    fn scroll(&mut self, action: LogScrollAction) {
        match action {
            LogScrollAction::PageUp => {
                self.lines_from_tail = self
                    .lines_from_tail
                    .saturating_add(self.page_size.max(1))
                    .min(self.max_lines_from_tail);
            }
            LogScrollAction::PageDown => {
                self.lines_from_tail = self.lines_from_tail.saturating_sub(self.page_size.max(1));
            }
            LogScrollAction::Home => self.lines_from_tail = self.max_lines_from_tail,
            LogScrollAction::End => self.lines_from_tail = 0,
        }
    }

    fn paragraph_offset(
        &mut self,
        line_count: usize,
        viewport_height: usize,
        revision: usize,
    ) -> u16 {
        let max_offset = line_count.saturating_sub(viewport_height);
        if self.lines_from_tail > 0 && revision != self.last_revision {
            self.lines_from_tail = self
                .lines_from_tail
                .saturating_add(line_count.saturating_sub(self.last_line_count));
        }
        self.lines_from_tail = self.lines_from_tail.min(max_offset);
        self.max_lines_from_tail = max_offset;
        self.page_size = viewport_height.max(1);
        self.last_line_count = line_count;
        self.last_revision = revision;

        max_offset
            .saturating_sub(self.lines_from_tail)
            .min(u16::MAX as usize) as u16
    }
}

#[derive(Clone, Copy)]
enum LogScrollAction {
    PageUp,
    PageDown,
    Home,
    End,
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
            log_revision: 0,
            log_scroll: LogScroll::default(),
            final_summary: FinalSummary::default(),
            next_begin_seq: 0,
        }
    }

    fn step_mut(&mut self, id: StageId) -> Option<&mut StepState> {
        self.steps.iter_mut().find(|step| step.id == id)
    }

    /// Mark a stage active and stamp it with the next begin sequence. The
    /// orchestrator can hold several stages open at once (an outer selection
    /// stage begins before its inner bake finishes), so the stamp records
    /// which became active last.
    fn begin_step(&mut self, id: StageId) {
        let seq = self.next_begin_seq;
        let mut stamped = false;
        if let Some(step) = self.step_mut(id) {
            step.status = StepStatus::Active;
            step.started = Some(Instant::now());
            step.activity_index = 0;
            step.last_completed = 0;
            step.remaining_estimate = RemainingEstimate::default();
            step.begin_seq = seq;
            stamped = true;
        }
        if stamped {
            self.next_begin_seq += 1;
        }
    }

    /// The active step the progress readout follows: the most-recently-begun
    /// one, so an overlapping inner bake takes the foot from its still-open
    /// outer stage rather than the outer stage pinning it.
    fn active_index(&self) -> Option<usize> {
        self.steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.status == StepStatus::Active)
            .max_by_key(|(_, step)| step.begin_seq)
            .map(|(index, _)| index)
    }

    fn append_logs(&mut self, records: Vec<CapturedRecord>) {
        self.logs.extend(records);
        let overflow = self.logs.len().saturating_sub(MAX_LOG_RECORDS);
        if overflow > 0 {
            self.logs.drain(..overflow);
        }
        self.log_revision = self.log_revision.wrapping_add(1);
    }
}

/// Reporter shared by the bake worker and the terminal render thread.
pub struct TuiReporter {
    state: Mutex<TuiState>,
    logs: LogSink,
    finalized: AtomicBool,
    summary_printed: AtomicBool,
}

impl TuiReporter {
    pub fn new(planned: &[StageDescriptor], logs: LogSink) -> Self {
        Self {
            state: Mutex::new(TuiState::new(planned)),
            logs,
            finalized: AtomicBool::new(false),
            summary_printed: AtomicBool::new(false),
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
        if self.summary_printed.swap(true, Ordering::AcqRel) {
            return;
        }
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

        self.logs.print_warning_summary();
    }
}

impl Reporter for TuiReporter {
    fn begin_stage(&self, id: StageId) {
        self.lock().begin_step(id);
    }

    fn declare_progress(&self, id: StageId, progress: StageProgress) {
        if let Some(step) = self.lock().step_mut(id) {
            step.last_completed = progress.completed();
            step.remaining_estimate = RemainingEstimate::default();
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

    fn finalize(&self, timings: &[(&'static str, Duration)], total: Duration) {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        self.drain_logs();
        let mut state = self.lock();
        state.final_summary.timings = timings.to_vec();
        state.final_summary.total = Some(total);
    }

    fn finalize_failure(&self) {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        self.drain_logs();
        let mut state = self.lock();
        // Resolve every still-Active step so none render as a live spinner
        // under a FAILED banner. Any Active stage -- the one that actually
        // failed, or an outer stage left open around it -- didn't complete,
        // so all of them are marked Failed rather than left spinning.
        for step in state.steps.iter_mut() {
            if step.status == StepStatus::Active {
                step.status = StepStatus::Failed;
            }
        }
        state.final_summary = FinalSummary::default();
    }
}

impl Drop for TuiReporter {
    /// Backstop mirroring `PlainReporter`'s: if a panic or early return
    /// unwinds past `run_tui_session` without ever reaching `finalize` /
    /// `finalize_failure` + the worker's `print_final_summary()` call, make
    /// sure the warning tally still gets printed. `print_final_summary`
    /// self-guards on `summary_printed`, so this is a no-op on the normal
    /// path where the summary was already printed.
    fn drop(&mut self) {
        self.print_final_summary();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        reporter.begin_stage(StageId::Parsing);
        reporter.finish_stage(StageId::Parsing);
        reporter.begin_stage(StageId::DataScript);
        reporter.skip_stage(StageId::DataScript);

        let state = reporter.lock();
        assert_eq!(state.steps[0].status, StepStatus::Done);
        assert_eq!(state.steps[1].status, StepStatus::Skipped);
    }

    #[test]
    fn active_readout_follows_the_most_recently_begun_overlapping_stage() {
        let reporter = TuiReporter::new(
            &[
                descriptor(StageId::EntityShadowLights),
                descriptor(StageId::DirectShDeltaBake),
            ],
            LogSink::default(),
        );
        // The outer selection stage stays open while the inner delta bake runs.
        reporter.begin_stage(StageId::EntityShadowLights);
        reporter.begin_stage(StageId::DirectShDeltaBake);

        let state = reporter.lock();
        let active = state.active_index().expect("an active step");
        assert_eq!(state.steps[active].id, StageId::DirectShDeltaBake);
    }

    #[test]
    fn log_scroll_pages_and_clamps_at_both_ends() {
        let mut scroll = LogScroll::default();
        assert_eq!(scroll.paragraph_offset(30, 5, 0), 25);

        scroll.scroll(LogScrollAction::PageUp);
        assert_eq!(scroll.lines_from_tail, 5);
        assert_eq!(scroll.paragraph_offset(30, 5, 0), 20);

        for _ in 0..10 {
            scroll.scroll(LogScrollAction::PageUp);
        }
        assert_eq!(scroll.lines_from_tail, 25);
        assert_eq!(scroll.paragraph_offset(30, 5, 0), 0);

        for _ in 0..10 {
            scroll.scroll(LogScrollAction::PageDown);
        }
        assert_eq!(scroll.lines_from_tail, 0);
        assert_eq!(scroll.paragraph_offset(30, 5, 0), 25);
    }

    #[test]
    fn first_terminal_finalization_wins() {
        let reporter = TuiReporter::new(&[], LogSink::default());
        reporter.finalize(&[("parse", Duration::from_secs(1))], Duration::from_secs(1));
        reporter.finalize_failure();
        reporter.finalize_failure();

        assert!(reporter.finalized.load(Ordering::Acquire));
        assert_eq!(
            reporter.lock().final_summary.total,
            Some(Duration::from_secs(1))
        );

        let failed = TuiReporter::new(&[], LogSink::default());
        failed.finalize_failure();
        failed.finalize(&[("parse", Duration::from_secs(1))], Duration::from_secs(1));
        assert!(failed.lock().final_summary.total.is_none());
    }
}
