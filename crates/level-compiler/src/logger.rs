//! Collecting log backend: warning sink for plain and interactive compiler reporters.
//! See: `context/lib/build_pipeline.md`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

const MAX_PENDING_RECORDS: usize = 4_096;

/// An owned log record safe to retain after `log::Log::log` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedRecord {
    pub level: Level,
    pub target: String,
    pub message: String,
}

impl fmt::Display for CapturedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "[{} {}] {}",
            self.level, self.target, self.message
        )
    }
}

#[derive(Debug, Default)]
struct SinkState {
    pending: VecDeque<CapturedRecord>,
    // Intentionally uncapped, unlike `pending`: the end-of-run tally must
    // list every warning, so history can't be dropped under a warn-flood.
    warnings: Vec<CapturedRecord>,
}

/// Shared destination for live records and the end-of-build warning history.
#[derive(Clone, Debug, Default)]
pub struct LogSink {
    state: Arc<Mutex<SinkState>>,
    warning_count: Arc<AtomicUsize>,
    summary_printed: Arc<std::sync::atomic::AtomicBool>,
}

impl LogSink {
    /// Capture an owned record. Warn-or-higher records remain in history even
    /// after the live queue is drained.
    pub fn record(&self, record: CapturedRecord) {
        let is_warning = record.level <= Level::Warn;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_warning {
            self.warning_count.fetch_add(1, Ordering::Relaxed);
            state.warnings.push(record.clone());
        }
        if state.pending.len() >= MAX_PENDING_RECORDS {
            state.pending.pop_front();
        }
        state.pending.push_back(record);
    }

    /// Remove and return all live records collected since the previous drain.
    pub fn drain(&self) -> Vec<CapturedRecord> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.pending.drain(..).collect()
    }

    /// Return the number of warn-or-error records observed by the logger.
    pub fn warning_count(&self) -> usize {
        self.warning_count.load(Ordering::Relaxed)
    }

    /// Snapshot warn-or-error records, including records already drained live.
    pub fn warnings(&self) -> Vec<CapturedRecord> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .warnings
            .clone()
    }

    /// Print the retained warn-or-higher history once across all sink clones.
    pub fn print_warning_summary(&self) {
        if self
            .summary_printed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        println!("\nWarnings: {}", self.warning_count());
        for warning in self.warnings() {
            println!("  {warning}");
        }
    }

    #[cfg(test)]
    pub(crate) fn summary_printed(&self) -> bool {
        self.summary_printed.load(Ordering::Acquire)
    }
}

/// A `log` backend with env_logger-compatible directive filtering.
pub struct CollectingLogger {
    filter: env_filter::Filter,
    sink: LogSink,
}

impl CollectingLogger {
    pub fn new(filter: env_filter::Filter, sink: LogSink) -> Self {
        Self { filter, sink }
    }
}

impl Log for CollectingLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if !self.filter.matches(record) {
            return;
        }
        self.sink.record(CapturedRecord {
            level: record.level(),
            target: record.target().to_owned(),
            message: record.args().to_string(),
        });
    }

    fn flush(&self) {}
}

/// Install the process logger, honoring `RUST_LOG` and the verbose default.
/// With a telemetry consumer, the telemetry tee wraps it: a record reaches
/// the sink only when this logger's own filter accepts it, so the TUI log
/// pane and the warning tally are unchanged.
pub fn install(verbose: bool, telemetry: bool) -> Result<LogSink, log::SetLoggerError> {
    let default = if verbose { "info" } else { "warn" };
    let directives = std::env::var("RUST_LOG").unwrap_or_else(|_| default.to_owned());
    let mut builder = env_filter::Builder::new();
    builder.parse(&directives);
    let filter = builder.build();
    let max_level = filter.filter();
    let sink = LogSink::default();
    postretro_telemetry::install_logger(
        Box::new(CollectingLogger::new(filter, sink.clone())),
        max_level,
        telemetry,
    )?;
    Ok(sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logger(spec: &str) -> (CollectingLogger, LogSink) {
        let mut builder = env_filter::Builder::new();
        builder.parse(spec);
        let sink = LogSink::default();
        (CollectingLogger::new(builder.build(), sink.clone()), sink)
    }

    fn emit(logger: &CollectingLogger, level: Level, target: &str, message: &str) {
        logger.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .build(),
        );
    }

    // Telemetry: behind the tee, an info record the warn filter hides never
    // reaches the sink, so it never joins the TUI warnings list.
    #[test]
    fn behind_the_tee_an_info_record_reaches_neither_the_sink_nor_the_warnings() {
        let (logger, sink) = logger("warn");
        let tee = postretro_telemetry::TeeLogger::new(Box::new(logger));
        tee.log(
            &Record::builder()
                .level(Level::Info)
                .target("compiler")
                .args(format_args!("an info"))
                .build(),
        );
        tee.log(
            &Record::builder()
                .level(Level::Warn)
                .target("compiler")
                .args(format_args!("a warning"))
                .build(),
        );
        let drained = sink.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message, "a warning");
        assert_eq!(sink.warning_count(), 1);
    }

    #[test]
    fn filters_records_and_retains_warning_history_after_drain() {
        let (logger, sink) = logger("warn,compiler::detail=info");
        emit(&logger, Level::Info, "other", "hidden");
        emit(&logger, Level::Info, "compiler::detail", "visible detail");
        emit(&logger, Level::Warn, "compiler", "warning");
        emit(&logger, Level::Error, "compiler", "error");

        let drained = sink.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(sink.warning_count(), 2);
        assert_eq!(sink.warnings().len(), 2);
        assert!(sink.drain().is_empty());
        assert_eq!(sink.warnings().len(), 2);
    }

    #[test]
    fn warning_summary_claim_is_shared_by_sink_clones() {
        let sink = LogSink::default();
        assert!(!sink.summary_printed.load(Ordering::Acquire));
        sink.print_warning_summary();
        assert!(sink.summary_printed.load(Ordering::Acquire));
        let clone = sink.clone();
        clone.print_warning_summary();
        assert!(clone.summary_printed.load(Ordering::Acquire));
    }

    #[test]
    fn live_pending_queue_caps_and_drops_oldest_records() {
        let sink = LogSink::default();
        for index in 0..MAX_PENDING_RECORDS + 2 {
            sink.record(CapturedRecord {
                level: Level::Info,
                target: "compiler".to_owned(),
                message: index.to_string(),
            });
        }

        let drained = sink.drain();
        assert_eq!(drained.len(), MAX_PENDING_RECORDS);
        assert_eq!(drained.first().unwrap().message, "2");
        assert_eq!(
            drained.last().unwrap().message,
            (MAX_PENDING_RECORDS + 1).to_string()
        );
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn warning_history_remains_complete_after_live_queue_overflow_and_drain() {
        let sink = LogSink::default();
        let warning_total = MAX_PENDING_RECORDS + 2;
        for index in 0..warning_total {
            sink.record(CapturedRecord {
                level: Level::Warn,
                target: "compiler".to_owned(),
                message: index.to_string(),
            });
        }

        let drained = sink.drain();
        assert_eq!(drained.len(), MAX_PENDING_RECORDS);
        assert_eq!(drained.first().unwrap().message, "2");

        let warnings = sink.warnings();
        assert_eq!(sink.warning_count(), warning_total);
        assert_eq!(warnings.len(), warning_total);
        assert_eq!(warnings.first().unwrap().message, "0");
        assert_eq!(
            warnings.last().unwrap().message,
            (warning_total - 1).to_string()
        );
        assert!(sink.drain().is_empty());
        assert_eq!(sink.warnings(), warnings);
    }
}
