// Without a consumer, logger setup is exactly the inner logger's own: the
// global max level stays its filter. Its own test binary, so no other logger
// is installed in the process.
// See: context/lib/development_guide.md §6.5

use log::{LevelFilter, Log, Metadata, Record};

struct WarnOnly;

impl Log for WarnOnly {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= LevelFilter::Warn
    }
    fn log(&self, _record: &Record<'_>) {}
    fn flush(&self) {}
}

#[test]
fn with_no_consumer_the_global_max_level_is_the_inner_filter() {
    postretro_telemetry::install_logger(Box::new(WarnOnly), LevelFilter::Warn, false)
        .expect("no other logger in this test binary");
    assert_eq!(log::max_level(), LevelFilter::Warn);
}
