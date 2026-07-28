use std::any::Any;
use std::panic;

use log::{LevelFilter, Log, Metadata, Record};
use postretro_test_log_capture::LogCapture;

struct ForeignLogger;

impl Log for ForeignLogger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, _record: &Record<'_>) {}

    fn flush(&self) {}
}

static FOREIGN_LOGGER: ForeignLogger = ForeignLogger;

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
fn start_panics_when_a_foreign_logger_owns_the_process() {
    log::set_logger(&FOREIGN_LOGGER).expect("foreign logger must install first");
    log::set_max_level(LevelFilter::Trace);

    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(LogCapture::start);
    panic::set_hook(previous);

    let panic = match result {
        Ok(_) => panic!("capture must reject a foreign logger"),
        Err(payload) => payload,
    };
    let message = panic_message(panic);
    assert!(message.contains("different process-global logger"));
}
