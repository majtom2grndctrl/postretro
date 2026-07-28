//! Per-thread `log` capture for tests that treat diagnostics as contracts.
//! See: `context/lib/testing_guide.md` §3.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use log::{Level, LevelFilter, Log, Metadata, Record};

/// An owned log record retained after `log::Log::log` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedRecord {
    pub level: Level,
    pub target: String,
    pub message: String,
}

struct SequencedRecord {
    sequence: u64,
    record: CapturedRecord,
}

struct TestLogger;

static LOGGER: TestLogger = TestLogger;
static LOGGER_INSTALLER: Once = Once::new();
static LOGGER_INSTALLED: AtomicBool = AtomicBool::new(false);
static NEXT_RECORD_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static ORPHAN_RECORDS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CAPTURE_BUFFER: RefCell<Option<Arc<Mutex<Vec<SequencedRecord>>>>> = const { RefCell::new(None) };
}

impl Log for TestLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        let sequence = NEXT_RECORD_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        // Formatting can call a `Display` implementation that logs. Do it before
        // entering the thread-local state so that re-entrant record has no live
        // `RefCell` borrow to contend with.
        let captured = CapturedRecord {
            level: record.level(),
            target: record.target().to_owned(),
            message: record.args().to_string(),
        };

        let buffer = CAPTURE_BUFFER
            .try_with(|slot| {
                let slot = slot.borrow();
                slot.as_ref().cloned()
            })
            .ok()
            .flatten();

        if let Some(buffer) = buffer {
            // The `Ref` above has dropped before taking this mutex. Holding it
            // across the lock can deadlock when a formatter emits another log.
            let mut records = buffer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let insert_at = records.partition_point(|existing| existing.sequence < sequence);
            records.insert(
                insert_at,
                SequencedRecord {
                    sequence,
                    record: captured,
                },
            );
        } else {
            record_orphan(&captured);
        }
    }

    fn flush(&self) {}
}

fn record_orphan(record: &CapturedRecord) {
    ORPHAN_RECORDS.fetch_add(1, Ordering::Relaxed);
    if std::env::var_os("POSTRETRO_LOG_CAPTURE_ORPHANS").is_some() {
        eprintln!(
            "[postretro-test-log-capture orphan] {} {}: {}",
            record.level, record.target, record.message
        );
    }
}

fn install_logger() {
    LOGGER_INSTALLER.call_once(|| {
        let installed = log::set_logger(&LOGGER).is_ok();
        LOGGER_INSTALLED.store(installed, Ordering::Release);
        log::set_max_level(LevelFilter::Trace);
    });

    assert!(
        LOGGER_INSTALLED.load(Ordering::Acquire),
        "postretro-test-log-capture cannot start: a different process-global logger is already installed"
    );
}

fn active_buffer() -> Arc<Mutex<Vec<SequencedRecord>>> {
    CAPTURE_BUFFER
        .try_with(|slot| {
            let slot = slot.borrow();
            slot.as_ref().cloned()
        })
        .expect("postretro-test-log-capture thread-local state is unavailable")
        .expect("postretro-test-log-capture has no active capture on this thread")
}

/// A guard that captures records emitted by its thread until it is dropped.
///
/// The guard deliberately is not `Send`: dropping it must detach the buffer
/// from the same thread that attached it.
pub struct LogCapture {
    orphan_records_at_start: u64,
    _not_send: PhantomData<*const ()>,
}

impl LogCapture {
    /// Install the test logger once and attach a fresh buffer to this thread.
    pub fn start() -> Self {
        install_logger();

        CAPTURE_BUFFER
            .try_with(|slot| {
                let mut slot = slot.borrow_mut();
                assert!(
                    slot.is_none(),
                    "postretro-test-log-capture cannot start: a capture is already active on this thread"
                );
                *slot = Some(Arc::new(Mutex::new(Vec::new())));
            })
            .expect("postretro-test-log-capture thread-local state is unavailable");

        Self {
            orphan_records_at_start: ORPHAN_RECORDS.load(Ordering::Relaxed),
            _not_send: PhantomData,
        }
    }

    /// Return an ordered snapshot of records captured by this thread.
    pub fn records(&self) -> Vec<CapturedRecord> {
        let buffer = active_buffer();
        buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|captured| captured.record.clone())
            .collect()
    }

    /// Remove records captured by this thread without affecting other threads.
    pub fn clear(&self) {
        active_buffer()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Assert that at least one record matches an exact level and body substring.
    pub fn assert_logged(&self, level: Level, body: &str) {
        let count = self.match_count(level, None, body);
        if count == 0 {
            self.fail(
                format!("at least one {}", expectation(level, None, body)),
                count,
            );
        }
    }

    /// Assert that exactly one record matches an exact level and body substring.
    pub fn assert_logged_once(&self, level: Level, body: &str) {
        let count = self.match_count(level, None, body);
        if count != 1 {
            self.fail(
                format!("exactly one {}", expectation(level, None, body)),
                count,
            );
        }
    }

    /// Assert that no record matches an exact level and body substring.
    pub fn assert_not_logged(&self, level: Level, body: &str) {
        let count = self.match_count(level, None, body);
        if count != 0 {
            self.fail(format!("no {}", expectation(level, None, body)), count);
        }
    }

    /// Assert that at least one record also has a target beginning with `target_prefix`.
    pub fn assert_logged_from(&self, level: Level, target_prefix: &str, body: &str) {
        let count = self.match_count(level, Some(target_prefix), body);
        if count == 0 {
            self.fail(
                format!(
                    "at least one {}",
                    expectation(level, Some(target_prefix), body)
                ),
                count,
            );
        }
    }

    fn match_count(&self, level: Level, target_prefix: Option<&str>, body: &str) -> usize {
        self.records()
            .iter()
            .filter(|record| {
                record.level == level
                    && record.message.contains(body)
                    && target_prefix.is_none_or(|prefix| record.target.starts_with(prefix))
            })
            .count()
    }

    fn fail(&self, expectation: String, count: usize) -> ! {
        let records = self.records();
        let mut message = format!(
            "log capture assertion failed: expected {expectation}; match count: {count}\nCaptured records:"
        );
        for record in records {
            message.push_str(&format!(
                "\n  [{} {}] {}",
                record.level, record.target, record.message
            ));
        }
        let orphans = ORPHAN_RECORDS
            .load(Ordering::Relaxed)
            .saturating_sub(self.orphan_records_at_start);
        message.push_str(&format!("\norphan records (process-wide): {orphans}"));
        panic!("{message}");
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        let _ = CAPTURE_BUFFER.try_with(|slot| slot.borrow_mut().take());
    }
}

fn expectation(level: Level, target_prefix: Option<&str>, body: &str) -> String {
    match target_prefix {
        Some(prefix) => format!(
            "{level} record from target prefix {prefix:?} containing body substring {body:?}"
        ),
        None => format!("{level} record containing body substring {body:?}"),
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::{Arc, Barrier, Mutex, OnceLock, mpsc};
    use std::thread;

    use super::*;

    const TEST_TARGET: &str = "postretro_test_log_capture::tests";

    static PANIC_HOOK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn panic_hook_lock() -> &'static Mutex<()> {
        PANIC_HOOK_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn catch_with_suppressed_hook<T>(action: impl FnOnce() -> T) -> T {
        let _serial = panic_hook_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let result = action();
        panic::set_hook(previous);
        result
    }

    fn panic_message(payload: Box<dyn Any + Send>) -> String {
        if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&'static str>() {
            (*message).to_owned()
        } else {
            "non-string panic payload".to_owned()
        }
    }

    #[test]
    fn assertions_match_exact_levels_bodies_and_target_prefixes() {
        let capture = LogCapture::start();
        log::error!(target: TEST_TARGET, "[Test] level-only");
        log::warn!(target: TEST_TARGET, "[Test] body-only");
        log::warn!(target: TEST_TARGET, "[Test] target-filter");
        log::warn!(target: "other_test_target", "[Test] target-filter");

        capture.assert_logged(Level::Warn, "[Test] body-only");
        capture.assert_not_logged(Level::Warn, "[Test] level-only");
        capture.assert_not_logged(Level::Warn, "[Test] another body");
        capture.assert_logged_from(Level::Warn, TEST_TARGET, "[Test] target-filter");

        let failure = catch_with_suppressed_hook(|| {
            panic::catch_unwind(|| {
                capture.assert_logged_from(Level::Warn, "missing_target", "[Test] target-filter");
            })
        });
        let message = panic_message(failure.expect_err("wrong target must not satisfy assertion"));
        assert!(message.contains("match count: 0"));
        assert!(message.contains("target prefix \"missing_target\""));
    }

    #[test]
    fn records_preserve_order_and_capture_trace_and_debug() {
        let capture = LogCapture::start();
        log::trace!(target: TEST_TARGET, "[Test] trace record");
        log::debug!(target: TEST_TARGET, "[Test] debug record");
        log::warn!(target: TEST_TARGET, "[Test] warning record");

        assert_eq!(
            capture.records(),
            vec![
                CapturedRecord {
                    level: Level::Trace,
                    target: TEST_TARGET.to_owned(),
                    message: "[Test] trace record".to_owned(),
                },
                CapturedRecord {
                    level: Level::Debug,
                    target: TEST_TARGET.to_owned(),
                    message: "[Test] debug record".to_owned(),
                },
                CapturedRecord {
                    level: Level::Warn,
                    target: TEST_TARGET.to_owned(),
                    message: "[Test] warning record".to_owned(),
                },
            ]
        );
    }

    // Regression: re-entrant formatting appended a nested record before its outer record.
    #[test]
    fn reentrant_formatting_preserves_logger_entry_order() {
        struct ReentrantFormatter;

        impl std::fmt::Display for ReentrantFormatter {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                log::warn!(target: TEST_TARGET, "[Test] nested formatting record");
                formatter.write_str("formatted body")
            }
        }

        let capture = LogCapture::start();
        log::info!(
            target: TEST_TARGET,
            "[Test] outer record with {}",
            ReentrantFormatter
        );

        assert_eq!(
            capture.records(),
            vec![
                CapturedRecord {
                    level: Level::Info,
                    target: TEST_TARGET.to_owned(),
                    message: "[Test] outer record with formatted body".to_owned(),
                },
                CapturedRecord {
                    level: Level::Warn,
                    target: TEST_TARGET.to_owned(),
                    message: "[Test] nested formatting record".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn exactly_once_and_negative_assertions_report_failures() {
        let capture = LogCapture::start();
        log::warn!(target: TEST_TARGET, "[Test] duplicate");
        capture.assert_logged_once(Level::Warn, "[Test] duplicate");
        capture.assert_not_logged(Level::Warn, "[Test] absent");
        log::warn!(target: TEST_TARGET, "[Test] duplicate");

        let exact_once_failure = catch_with_suppressed_hook(|| {
            panic::catch_unwind(|| capture.assert_logged_once(Level::Warn, "[Test] duplicate"))
        });
        let exact_once_message = panic_message(
            exact_once_failure.expect_err("two matching records must fail exactly-once assertion"),
        );
        assert!(exact_once_message.contains("expected exactly one"));
        assert!(exact_once_message.contains("match count: 2"));

        let negative_failure = catch_with_suppressed_hook(|| {
            panic::catch_unwind(|| capture.assert_not_logged(Level::Warn, "[Test] duplicate"))
        });
        let negative_message = panic_message(
            negative_failure.expect_err("matching record must fail negative assertion"),
        );
        assert!(negative_message.contains("expected no"));
        assert!(negative_message.contains("match count: 2"));
    }

    #[test]
    fn parallel_captures_are_isolated_by_thread() {
        let barrier = Arc::new(Barrier::new(2));
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            let capture = LogCapture::start();
            log::info!(target: "parallel_first", "[Test] first thread");
            first_barrier.wait();
            capture.records()
        });
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            let capture = LogCapture::start();
            log::info!(target: "parallel_second", "[Test] second thread");
            second_barrier.wait();
            capture.records()
        });

        let first_records = first.join().expect("first capture thread must finish");
        let second_records = second.join().expect("second capture thread must finish");
        assert_eq!(first_records.len(), 1);
        assert_eq!(first_records[0].message, "[Test] first thread");
        assert_eq!(second_records.len(), 1);
        assert_eq!(second_records[0].message, "[Test] second thread");
    }

    #[test]
    fn unwind_drops_the_buffer_before_the_next_capture_starts() {
        let panic = catch_with_suppressed_hook(|| {
            panic::catch_unwind(AssertUnwindSafe(|| {
                let _capture = LogCapture::start();
                log::warn!(target: TEST_TARGET, "[Test] discarded by unwind");
                panic!("[Test] simulate test unwind");
            }))
        });
        assert!(panic.is_err(), "the simulated test panic must unwind");

        let capture = LogCapture::start();
        assert!(
            capture.records().is_empty(),
            "the fresh buffer must not retain the unwound guard's records"
        );
    }

    #[test]
    fn nested_capture_on_one_thread_panics_without_detaching_the_first() {
        let capture = LogCapture::start();
        let nested = catch_with_suppressed_hook(|| panic::catch_unwind(LogCapture::start));
        let nested_panic = match nested {
            Ok(_) => panic!("nested capture must panic"),
            Err(payload) => payload,
        };
        let message = panic_message(nested_panic);
        assert!(message.contains("capture is already active on this thread"));

        log::info!(target: TEST_TARGET, "[Test] first capture remains active");
        capture.assert_logged(Level::Info, "[Test] first capture remains active");
    }

    #[test]
    fn failure_messages_include_all_records_and_orphan_count() {
        let capture = LogCapture::start();
        log::warn!(target: TEST_TARGET, "[Test] first captured record");
        log::error!(target: TEST_TARGET, "[Test] second captured record");
        thread::spawn(|| log::warn!(target: "orphan_thread", "[Test] orphan record"))
            .join()
            .expect("orphan logging thread must finish");

        let failure = catch_with_suppressed_hook(|| {
            panic::catch_unwind(|| capture.assert_logged(Level::Warn, "[Test] missing record"))
        });
        let message = panic_message(failure.expect_err("missing record must fail assertion"));
        assert!(message.contains("expected at least one"));
        assert!(message.contains("match count: 0"));
        assert!(
            message
                .contains("[WARN postretro_test_log_capture::tests] [Test] first captured record")
        );
        assert!(
            message.contains(
                "[ERROR postretro_test_log_capture::tests] [Test] second captured record"
            )
        );
        let orphan_line = message
            .lines()
            .find(|line| line.starts_with("orphan records (process-wide):"))
            .expect("failure must report process-wide orphan records");
        assert!(
            orphan_line
                .trim_start_matches("orphan records (process-wide):")
                .trim()
                .parse::<u64>()
                .is_ok(),
            "orphan count must be formatted as an integer"
        );
    }

    #[test]
    fn clear_only_empties_the_calling_threads_buffer() {
        let capture = LogCapture::start();
        log::info!(target: TEST_TARGET, "[Test] parent record");
        let (ready, ready_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let child = thread::spawn(move || {
            let child_capture = LogCapture::start();
            log::info!(target: "clear_child", "[Test] child record");
            ready.send(()).expect("parent must wait for child record");
            continue_rx
                .recv()
                .expect("parent must release child record check");
            child_capture.records()
        });
        ready_rx
            .recv()
            .expect("child must attach and log before parent clears");

        capture.clear();
        assert!(capture.records().is_empty());
        continue_tx
            .send(())
            .expect("child must still be waiting for its snapshot");
        let child_records = child.join().expect("child capture thread must finish");
        assert_eq!(child_records.len(), 1);
        assert_eq!(child_records[0].message, "[Test] child record");
    }
}
