//! The capture's `tracing` dispatcher: events become `log` records, spans become nothing.
//!
//! Without a global `tracing` dispatcher, `tracing`'s `log` feature turns every span
//! *creation* into a `log` record at the span's own level. Any crate in the build can
//! switch that feature on for all of them — on Linux, `calloop` 0.14 does so
//! unconditionally, and `dev-tools` pulls it in through egui-winit's Wayland
//! clipboard — and swc opens its pass spans at `ERROR`, so compiling a mod in a test
//! logged `tree-shaker; pass=1` as an error. A span is structure, not a
//! diagnostic. Setting a dispatcher turns the bridge off, and this one forwards only
//! events, so a dependency's real `tracing::warn!`/`error!` still reaches the capture
//! at its own level and target.

use std::fmt::{self, Write as _};

use tracing_core::field::{Field, Visit};
use tracing_core::span::{Attributes, Id, Record as SpanRecord};
use tracing_core::subscriber::Interest;
use tracing_core::{Event, LevelFilter, Metadata, Subscriber};

struct EventsToLog;

impl Subscriber for EventsToLog {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if metadata.is_event() {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event()
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }

    // Unreachable while `enabled` refuses spans; an id is still owed if one arrives.
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &SpanRecord<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing_core::Level::ERROR => log::Level::Error,
            tracing_core::Level::WARN => log::Level::Warn,
            tracing_core::Level::INFO => log::Level::Info,
            tracing_core::Level::DEBUG => log::Level::Debug,
            tracing_core::Level::TRACE => log::Level::Trace,
        };
        let mut body = EventBody::default();
        event.record(&mut body);
        let fields = if body.message.is_empty() {
            body.fields.trim_start()
        } else {
            &body.fields
        };
        log::logger().log(
            &log::Record::builder()
                .level(level)
                .target(metadata.target())
                .module_path(metadata.module_path())
                .file(metadata.file())
                .line(metadata.line())
                .args(format_args!("{}{fields}", body.message))
                .build(),
        );
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// `message` first, then `name=value` per other field, as `tracing`'s own bridge writes it.
#[derive(Default)]
struct EventBody {
    message: String,
    fields: String,
}

impl Visit for EventBody {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

/// Install the dispatcher, unless a test already set its own: that test owns
/// `tracing`, and the bridge is off either way.
pub(crate) fn install() {
    let _ = tracing_core::dispatcher::set_global_default(tracing_core::Dispatch::new(EventsToLog));
}
