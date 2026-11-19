//! Typed telemetry for agents: one versioned JSON envelope, a JSONL file
//! sink, a bounded live ring and a log tee. Engine-blind; producers build
//! their payloads from the snapshot their log line formats and hand them
//! here. See: context/lib/development_guide.md §6.5

use std::sync::OnceLock;

use serde::Serialize;

mod envelope;
mod hub;
mod payload;
mod ring;
mod tee;

#[cfg(test)]
mod tests;

pub use envelope::{ENVELOPE_VERSION, Envelope, Kind, Level, ParseError, Source};
pub use hub::{Hub, HubConfig, MESSAGE_CAP_BYTES};
pub use payload::{ExitReason, LogRecord, ProducerEntry, ProducerState, RunEnd, RunStart};
pub use ring::{EventsError, EventsPage, RingCaps};
pub use tee::{TeeLogger, install_logger};

/// The file sink's variable: `POSTRETRO_TELEMETRY=<path>`, in every build.
pub const TELEMETRY_ENV: &str = "POSTRETRO_TELEMETRY";

static GLOBAL: OnceLock<Hub> = OnceLock::new();

/// The sink path `POSTRETRO_TELEMETRY` names; empty reads as unset.
pub fn sink_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os(TELEMETRY_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// Install the process-wide hub. The first install wins; later calls get it.
pub fn install(hub: Hub) -> &'static Hub {
    GLOBAL.get_or_init(|| hub)
}

/// The process-wide hub, if one was installed.
pub fn global() -> Option<&'static Hub> {
    GLOBAL.get()
}

/// Run `f` against this thread's scoped hub (tests) or the process-wide one.
/// `None` when neither exists: telemetry is off and nothing is built.
pub fn with_hub<R>(f: impl FnOnce(&Hub) -> R) -> Option<R> {
    #[cfg(feature = "test-support")]
    if let Some(hub) = scoped::current() {
        return Some(f(&hub));
    }
    GLOBAL.get().map(f)
}

/// Whether any hub would receive an event. Allocation-free, so per-frame
/// paths can ask before building a payload.
pub fn active() -> bool {
    with_hub(|_| ()).is_some()
}

/// Emit `kind` under the running level.
pub fn emit<P: Serialize + ?Sized>(kind: Kind, payload: &P) {
    with_hub(|hub| hub.emit(kind, payload));
}

/// Build the payload only when a hub would receive it.
pub fn emit_with<P: Serialize>(kind: Kind, payload: impl FnOnce() -> P) {
    with_hub(|hub| hub.emit(kind, &payload()));
}

/// Emit `kind` naming `level` rather than the running level.
pub fn emit_for_level<P: Serialize + ?Sized>(kind: Kind, level: Option<&Level>, payload: &P) {
    with_hub(|hub| hub.emit_for_level(kind, level, payload));
}

/// The running level, `None` between levels or without a hub.
pub fn level() -> Option<Level> {
    with_hub(Hub::level).flatten()
}

pub fn set_level(level: Option<Level>) {
    with_hub(|hub| hub.set_level(level));
}

pub fn set_producer_state(entry: ProducerEntry) {
    with_hub(|hub| hub.set_producer_state(entry));
}

/// Run a process's main body under an unwind guard: a panic that unwinds out
/// of it writes `run_end` naming the panic, without waiting on a lock, and
/// then keeps unwinding. Never a panic hook, so a panic caught inside the
/// body (a worker's) writes nothing.
pub fn run_guarded<T>(body: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(panic) => {
            let message = panic_message(panic.as_ref());
            with_hub(|hub| hub.end_run_without_waiting(ExitReason::Panic, Some(&message)));
            std::panic::resume_unwind(panic)
        }
    }
}

/// End the run once; later events from any thread write nothing.
pub fn end_run(reason: ExitReason, detail: Option<&str>) {
    with_hub(|hub| hub.end_run(reason, detail));
}

/// The text a panic payload carries, or a placeholder.
pub fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        String::from("non-string panic payload")
    }
}

/// A hub private to one test thread, so parallel tests never share the
/// process-wide one.
#[cfg(feature = "test-support")]
pub mod scoped {
    use std::cell::RefCell;
    use std::sync::Arc;

    use crate::Hub;

    thread_local! {
        static SCOPED: RefCell<Option<Arc<Hub>>> = const { RefCell::new(None) };
    }

    pub(crate) fn current() -> Option<Arc<Hub>> {
        SCOPED.try_with(|slot| slot.borrow().clone()).ok().flatten()
    }

    /// Route this thread's telemetry to `hub` until the guard drops.
    pub fn install(hub: Arc<Hub>) -> ScopedHub {
        let previous = SCOPED.with(|slot| slot.replace(Some(hub)));
        ScopedHub { previous }
    }

    pub struct ScopedHub {
        previous: Option<Arc<Hub>>,
    }

    impl Drop for ScopedHub {
        fn drop(&mut self) {
            let previous = self.previous.take();
            let _ = SCOPED.try_with(|slot| slot.replace(previous));
        }
    }
}
