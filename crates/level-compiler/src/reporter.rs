//! Compiler progress reporting independent of terminal presentation.
//! See: `context/lib/build_pipeline.md`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::logger::{CapturedRecord, LogSink};
use crate::pipeline::StageId;

/// Shared, display-only progress state for one quantifiable stage.
///
/// Publication is tracked separately from the value, so a late-published zero
/// means a present stage completed without work rather than indeterminate.
#[derive(Clone, Debug)]
pub struct StageProgress {
    completed: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    total_published: Arc<AtomicBool>,
}

impl StageProgress {
    pub fn indeterminate() -> Self {
        Self {
            completed: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            total_published: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_total(total: usize) -> Self {
        let progress = Self::indeterminate();
        progress.publish_total(total);
        progress
    }

    pub fn completed_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.completed)
    }

    pub fn total_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.total)
    }

    pub fn total_published_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.total_published)
    }

    pub fn publish_total(&self, total: usize) {
        self.total.store(total, Ordering::Release);
        self.total_published.store(true, Ordering::Release);
    }

    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> Option<usize> {
        if self.total_published.load(Ordering::Acquire) {
            Some(self.total.load(Ordering::Acquire))
        } else {
            None
        }
    }
}

/// Presentation contract shared by plain and TUI compiler frontends.
///
/// Exactly one terminal method must win: `finalize` commits/records a complete
/// success summary for exactly-once presentation, while `finalize_failure`
/// prints warnings without a partial success table. Implementations make
/// repeated terminal calls no-ops.
pub trait Reporter: Send + Sync {
    fn begin_stage(&self, id: StageId);
    fn declare_progress(&self, id: StageId, progress: StageProgress);
    fn finish_stage(&self, id: StageId);
    fn skip_stage(&self, id: StageId);
    /// Records a warning directly into the shared `LogSink`, bypassing the
    /// `log` facade (`CollectingLogger`). Production warnings already reach
    /// this sink via the `CollectingLogger` drain — a caller using this
    /// method must not also emit the same warning through `log`, or it will
    /// be tallied twice.
    fn record_warning(&self, warning: CapturedRecord);
    fn finalize(&self, timings: &[(&'static str, Duration)], total: Duration);
    fn finalize_failure(&self);
}

struct Monitor {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    label: &'static str,
    progress: StageProgress,
    started: Instant,
}

#[derive(Default)]
struct PlainState {
    active: Option<(StageId, Instant)>,
    monitor: Option<Monitor>,
}

/// Line-oriented reporter suitable for CI, pipes, and `xtask` output.
pub struct PlainReporter {
    started: Instant,
    logs: LogSink,
    state: Mutex<PlainState>,
    finalized: AtomicBool,
}

impl PlainReporter {
    pub fn new(started: Instant, logs: LogSink) -> Self {
        Self {
            started,
            logs,
            state: Mutex::new(PlainState::default()),
            finalized: AtomicBool::new(false),
        }
    }

    fn drain_logs(&self) {
        for record in self.logs.drain() {
            eprintln!("{record}");
        }
    }

    fn stop_monitor(state: &mut PlainState) {
        if let Some(monitor) = state.monitor.take() {
            monitor.stop.store(true, Ordering::Release);
            monitor.thread.thread().unpark();
            let _ = monitor.thread.join();
            Self::print_progress(monitor.label, &monitor.progress, monitor.started.elapsed());
        }
    }

    fn print_progress(label: &str, progress: &StageProgress, elapsed: Duration) {
        let Some(total) = progress.total() else {
            return;
        };
        let done = progress.completed().min(total);
        let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
        let eta = if total == 0 {
            Some(Duration::ZERO)
        } else if done == 0 {
            None
        } else {
            Some(elapsed.mul_f64((total - done) as f64 / done as f64))
        };
        match eta {
            Some(eta) => eprintln!("  {label}: {percent:>3}%  ETA {:.1}s", eta.as_secs_f64()),
            None => eprintln!("  {label}: {percent:>3}%  ETA --"),
        }
    }
}

impl Drop for PlainReporter {
    fn drop(&mut self) {
        let needs_failure_summary = !self.finalized.swap(true, Ordering::AcqRel);
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::stop_monitor(state);
        self.drain_logs();
        if needs_failure_summary {
            self.logs.print_warning_summary();
        }
    }
}

impl Reporter for PlainReporter {
    fn begin_stage(&self, id: StageId) {
        self.drain_logs();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::stop_monitor(&mut state);
        state.active = Some((id, Instant::now()));
        eprintln!(
            "{:>6.2}s  {}",
            self.started.elapsed().as_secs_f32(),
            id.progress_label()
        );
    }

    fn declare_progress(&self, id: StageId, progress: StageProgress) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::stop_monitor(&mut state);
        let Some((active_id, stage_started)) = state.active else {
            return;
        };
        if active_id != id {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let label = id.label();
        let worker_progress = progress.clone();
        let logs = self.logs.clone();
        state.monitor = Some(Monitor {
            stop,
            thread: thread::spawn(move || {
                let mut previous = None;
                while !worker_stop.load(Ordering::Acquire) {
                    for record in logs.drain() {
                        eprintln!("{record}");
                    }
                    let total = worker_progress.total();
                    let done = worker_progress.completed();
                    let snapshot = (done, total);
                    if total.is_some() && previous != Some(snapshot) {
                        Self::print_progress(label, &worker_progress, stage_started.elapsed());
                        previous = Some(snapshot);
                    }
                    thread::park_timeout(Duration::from_millis(500));
                }
            }),
            label,
            progress,
            started: stage_started,
        });
    }

    fn finish_stage(&self, id: StageId) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.active.is_some_and(|(active, _)| active == id) {
            Self::stop_monitor(&mut state);
            state.active = None;
        }
        drop(state);
        self.drain_logs();
    }

    fn skip_stage(&self, id: StageId) {
        self.finish_stage(id);
        eprintln!("  {}: skipped", id.label());
    }

    fn record_warning(&self, warning: CapturedRecord) {
        self.logs.record(warning);
        self.drain_logs();
    }

    fn finalize(&self, timings: &[(&'static str, Duration)], total: Duration) {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::stop_monitor(&mut state);
        drop(state);
        self.drain_logs();

        println!("\nBuild Summary:");
        for (name, duration) in timings {
            println!("  {: <15} {:>6.2}s", name, duration.as_secs_f32());
        }
        println!("  {: <15} {:>6.2}s", "Total", total.as_secs_f32());

        self.logs.print_warning_summary();
    }

    fn finalize_failure(&self) {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::stop_monitor(&mut state);
        state.active = None;
        drop(state);
        self.drain_logs();
        self.logs.print_warning_summary();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_supports_late_total_publication() {
        let progress = StageProgress::indeterminate();
        assert_eq!(progress.total(), None);
        progress.completed_handle().store(3, Ordering::Relaxed);
        progress.publish_total(8);
        assert_eq!(progress.completed(), 3);
        assert_eq!(progress.total(), Some(8));
    }

    #[test]
    fn progress_distinguishes_published_zero_from_indeterminate() {
        let progress = StageProgress::indeterminate();
        assert_eq!(progress.total(), None);
        progress.publish_total(0);
        assert_eq!(progress.total(), Some(0));
    }

    #[test]
    fn failure_finalization_is_idempotent() {
        let reporter = PlainReporter::new(Instant::now(), LogSink::default());
        reporter.finalize_failure();
        reporter.finalize_failure();

        assert!(reporter.finalized.load(Ordering::Acquire));
        assert!(reporter.logs.summary_printed());
    }

    #[test]
    fn skipped_stage_clears_active_state() {
        let reporter = PlainReporter::new(Instant::now(), LogSink::default());
        reporter.begin_stage(StageId::NavMesh);
        reporter.skip_stage(StageId::NavMesh);
        assert!(
            reporter
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active
                .is_none()
        );
    }
}
