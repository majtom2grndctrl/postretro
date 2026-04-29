// Sequenced-primitive dispatch table.
//
// A `sequence` reaction is an ordered list of (entity, primitive, args)
// triples that fire one after another when the reaction's named event is
// dispatched. This is **separate** from the script-facing primitive registry
// — sequenced primitives are Rust-only handlers keyed by name, invoked by the
// reaction dispatcher rather than by behavior scripts.
//
// See: context/lib/scripting.md §4 (primitives) and §5 (shared engine state).

use std::collections::HashMap;

use thiserror::Error;

use super::registry::EntityId;

/// Errors produced when a sequenced-primitive handler runs. Distinct from
/// `ScriptError` because sequenced primitives are not script-facing — their
/// failures surface as warnings inside the reaction dispatcher rather than
/// being thrown into a script runtime.
#[derive(Debug, Error)]
pub(crate) enum SequenceError {
    /// The handler rejected the supplied arguments (shape, range, type).
    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },
    /// The handler ran but the engine state it touched failed (component
    /// missing, etc.). Treated as non-fatal at dispatch time.
    #[error("execution failed: {reason}")]
    ExecutionFailed { reason: String },
}

/// Boxed handler for a sequenced primitive. Receives the resolved entity ID
/// and a serde_json payload carrying primitive-specific arguments.
///
/// No `Send + Sync` bound: the scripting subsystem is single-threaded by
/// design (`ScriptCtx` captures `Rc<RefCell<_>>` into every primitive
/// closure), and the reaction dispatcher runs on the main thread alongside
/// the rest of the frame loop. Adding cross-thread bounds here would force
/// every handler to give up its `Rc`-shared engine state.
pub(crate) type SequencedPrimitiveFn =
    Box<dyn Fn(EntityId, &serde_json::Value) -> Result<(), SequenceError>>;

/// Lookup table: primitive name → handler. Populated before level load and
/// consulted by both the registration-time validator (to reject sequences
/// naming unknown primitives) and the dispatcher.
#[derive(Default)]
pub(crate) struct SequencedPrimitiveRegistry {
    handlers: HashMap<String, SequencedPrimitiveFn>,
}

impl SequencedPrimitiveRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a handler under `name`. Overwrites any existing entry — the
    /// caller is responsible for ensuring names are unique.
    pub(crate) fn register<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(EntityId, &serde_json::Value) -> Result<(), SequenceError> + 'static,
    {
        self.handlers.insert(name.into(), Box::new(handler));
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&SequencedPrimitiveFn> {
        self.handlers.get(name)
    }
}

impl std::fmt::Debug for SequencedPrimitiveRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SequencedPrimitiveRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn register_and_lookup() {
        let mut r = SequencedPrimitiveRegistry::new();
        r.register("noop", |_id, _args| Ok(()));
        assert!(r.contains("noop"));
        assert!(!r.contains("missing"));
        assert!(r.get("noop").is_some());
    }

    #[test]
    fn handler_receives_entity_and_args() {
        let mut r = SequencedPrimitiveRegistry::new();
        let counter = Arc::new(AtomicU32::new(0));
        let counter_cl = Arc::clone(&counter);
        r.register("count", move |id, args| {
            assert_eq!(id.to_raw(), 0x0001_0000);
            assert_eq!(args["k"].as_i64(), Some(7));
            counter_cl.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let h = r.get("count").unwrap();
        h(
            EntityId::from_raw(0x0001_0000),
            &serde_json::json!({ "k": 7 }),
        )
        .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
