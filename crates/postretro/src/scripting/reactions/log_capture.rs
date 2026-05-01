// Test-only log capture: install a thread-local logger, run a closure, and
// return every record produced inside that closure. Lets reaction tests
// assert on `log::warn!` output without pulling in an extra dev-dep.
// See: context/lib/scripting.md §11 (Emitter and Particles — Reaction primitives)

use std::cell::RefCell;
use std::sync::OnceLock;

use log::{Level, Log, Metadata, Record};

thread_local! {
    static BUFFER: RefCell<Option<Vec<(Level, String)>>> = const { RefCell::new(None) };
}

struct CaptureLogger;

impl Log for CaptureLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        BUFFER.with(|b| {
            if let Some(buf) = b.borrow_mut().as_mut() {
                buf.push((record.level(), format!("{}", record.args())));
            }
        });
    }

    fn flush(&self) {}
}

static LOGGER: OnceLock<()> = OnceLock::new();

fn install() {
    LOGGER.get_or_init(|| {
        // Ignore the result: the real engine binary may have installed a
        // logger first. In test runs the global logger slot is empty unless
        // another `capture()` call already installed ours, in which case the
        // `OnceLock` short-circuits before reaching here.
        let _ = log::set_logger(&CaptureLogger);
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// Run `f` with a fresh thread-local capture buffer; return the records
/// emitted during the call. Records emitted on other threads or outside the
/// closure are not captured.
pub(crate) fn capture<F: FnOnce()>(f: F) -> Vec<(Level, String)> {
    install();
    BUFFER.with(|b| *b.borrow_mut() = Some(Vec::new()));
    f();
    BUFFER.with(|b| b.borrow_mut().take().unwrap_or_default())
}
