// Script context handle and the engine-side event queue.
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 2
//
// `ScriptCtx` captures-by-Rc into every primitive closure at registration time.
// It is deliberately `!Send + !Sync`: `rquickjs::Context` is `!Send`,
// `mlua::Lua` is `!Send` (the `send` feature is off), and the engine frame
// loop is single-threaded. `Rc<RefCell<_>>` over `Arc<RwLock<_>>` because
// `std::sync::RwLock` poisons on panic and every primitive body runs inside
// `catch_unwind` — a poisoned lock would wedge the entire scripting surface
// after the first caught panic. `RefCell` has no poisoning.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::registry::{EntityId, EntityRegistry};

/// A single event flowing through the engine-owned event queue.
///
/// `kind` is the discriminant (matches `ComponentValue`'s `kind` convention);
/// `payload` is an arbitrary serde-serialized value. Both script runtimes
/// encode/decode against this shape at the FFI boundary.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScriptEvent {
    pub(crate) kind: String,
    pub(crate) payload: serde_json::Value,
}

/// In-flight events: broadcasts go into one queue, targeted events into the
/// other. The frame loop drains both at the end of game logic.
#[derive(Default)]
pub(crate) struct EventQueue {
    pub(crate) broadcast: VecDeque<ScriptEvent>,
    pub(crate) targeted: VecDeque<(EntityId, ScriptEvent)>,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// Handle every primitive closure captures. Cloning is cheap — it bumps two
/// `Rc`s. The fields behind `Rc<RefCell<_>>` are the subsystems the scripting
/// surface is allowed to touch. Extend by adding one field per subsystem.
#[derive(Clone)]
pub(crate) struct ScriptCtx {
    pub(crate) registry: Rc<RefCell<EntityRegistry>>,
    pub(crate) events: Rc<RefCell<EventQueue>>,
}

impl ScriptCtx {
    pub(crate) fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            events: Rc::new(RefCell::new(EventQueue::new())),
        }
    }
}
