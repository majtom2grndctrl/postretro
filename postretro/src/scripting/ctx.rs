// Script context handle and the engine-side event queue.
// See: context/lib/scripting.md
//
// `ScriptCtx` captures-by-Rc into every primitive closure at registration time.
// `!Send + !Sync` by design: the frame loop is single-threaded and both
// runtimes are `!Send`. `Rc<RefCell<_>>` over `Arc<RwLock<_>>` because
// `RwLock` poisons on panic — every primitive runs inside `catch_unwind`,
// so a poisoned lock would wedge the scripting surface after the first caught
// panic. `RefCell` has no poisoning.

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
