// Test-only log capture adapter for reaction tests.
// See: context/lib/testing_guide.md §3.

use log::Level;
use postretro_test_log_capture::LogCapture;

/// Run `f` with a fresh thread-local capture buffer; return the records
/// emitted during the call. Records emitted on other threads are not captured.
pub(crate) fn capture<F: FnOnce()>(f: F) -> Vec<(Level, String)> {
    let capture = LogCapture::start();
    f();
    capture
        .records()
        .into_iter()
        .map(|record| (record.level, record.message))
        .collect()
}
