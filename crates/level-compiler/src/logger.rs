//! Collecting logger used by both plain and interactive compiler reporters.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

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
    pending: Vec<CapturedRecord>,
    warnings: Vec<CapturedRecord>,
}

/// Shared destination for live records and the end-of-build warning history.
#[derive(Clone, Debug, Default)]
pub struct LogSink {
    state: Arc<Mutex<SinkState>>,
    warning_count: Arc<AtomicUsize>,
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
        state.pending.push(record);
    }

    /// Remove and return all live records collected since the previous drain.
    pub fn drain(&self) -> Vec<CapturedRecord> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut state.pending)
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
        metadata.level() <= self.filter.filter()
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
pub fn install(verbose: bool) -> Result<LogSink, log::SetLoggerError> {
    let default = if verbose { "info" } else { "warn" };
    let directives = std::env::var("RUST_LOG").unwrap_or_else(|_| default.to_owned());
    let mut builder = env_filter::Builder::new();
    builder.parse(&directives);
    let filter = builder.build();
    let max_level = filter.filter();
    let sink = LogSink::default();
    log::set_boxed_logger(Box::new(CollectingLogger::new(filter, sink.clone())))?;
    log::set_max_level(max_level);
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
}
