// The tee logger: forwards to the process's own logger with its filter
// unchanged, and captures info and above for telemetry.
// See: context/lib/development_guide.md §6.5

use std::cell::Cell;

use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};

/// Wraps a process's logger. A record reaches `inner` only when `inner`'s
/// own filter accepts it; info and above are also captured, so a record the
/// stderr filter hides still reaches the file and the ring.
pub struct TeeLogger {
    inner: Box<dyn Log>,
}

impl TeeLogger {
    pub fn new(inner: Box<dyn Log>) -> Self {
        Self { inner }
    }
}

thread_local! {
    /// Set while this thread captures, so a record logged from inside the
    /// capture (a sink warning, say) is forwarded but never re-captured.
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
}

impl Log for TeeLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Info || self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }
        if record.level() <= Level::Info {
            capture(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn capture(record: &Record<'_>) {
    if CAPTURING.with(Cell::get) {
        return;
    }
    CAPTURING.with(|capturing| capturing.set(true));
    let message = record.args().to_string();
    crate::with_hub(|hub| hub.log_record(record.level(), record.target(), &message));
    CAPTURING.with(|capturing| capturing.set(false));
}

/// Install the process logger. With a consumer (the sink variable, or a
/// live ring), the tee wraps `inner` and the max level rises to at least
/// info so info records reach capture. Without one, `inner` is installed
/// exactly as its own `init` would: boxed, with its filter as the max level.
pub fn install_logger(
    inner: Box<dyn Log>,
    inner_max: LevelFilter,
    consumer: bool,
) -> Result<(), SetLoggerError> {
    if consumer {
        log::set_boxed_logger(Box::new(TeeLogger::new(inner)))?;
        log::set_max_level(inner_max.max(LevelFilter::Info));
    } else {
        log::set_boxed_logger(inner)?;
        log::set_max_level(inner_max);
    }
    Ok(())
}
