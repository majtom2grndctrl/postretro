// Script context handle and the engine-side event queue.
// See: context/lib/scripting.md
//
// `ScriptCtx` captures-by-Rc into every primitive closure at registration time.
// `!Send + !Sync` by design: the frame loop is single-threaded and both
// runtimes are `!Send`. `Rc<RefCell<_>>` over `Arc<RwLock<_>>` because
// `RwLock` poisons on panic — every primitive runs inside `catch_unwind`,
// so a poisoned lock would wedge the scripting surface after the first caught
// panic. `RefCell` has no poisoning.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use super::data_registry::DataRegistry;
use super::event_dispatch::{HandlerTable, SharedHandlerTable};
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

/// A single observable game-event record drained at the end of the Game
/// logic phase. Kept structurally separate from `ScriptEvent` because the
/// ring buffer carries an extra `frame` stamp (the engine frame counter at
/// emission time) that the broadcast-handler path does not need.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GameEvent {
    pub(crate) kind: String,
    /// Engine frame counter at emit time; incremented once per redraw, not per fixed tick.
    pub(crate) frame: u64,
    pub(crate) payload: serde_json::Value,
}

/// Soft cap on the in-flight `GameEvent` ring buffer. Old entries are
/// `pop_front`-ed when capacity is exceeded so the queue is bounded but the
/// most recent emissions always survive to the drain at end-of-tick.
pub(crate) const GAME_EVENTS_CAPACITY: usize = 1024;

/// In-flight events: broadcasts go into one queue, targeted events into the
/// other. The two queues have different drain points:
/// - `broadcast` is drained by `event_dispatch` immediately after each tick
///   handler fires — not at end-of-frame.
/// - `game_events` (on `ScriptCtx`) is the ring buffer drained at the end of
///   the Game logic phase by `main.rs`.
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
    /// Per-level handler table populated by the `registerHandler` primitive
    /// and drained by level unload. See: `scripting::event_dispatch`.
    pub(crate) handlers: SharedHandlerTable,
    /// Engine-wide entity-type registry written by the `registerEntity`
    /// primitive (data context) and consumed by the level-load spawn sweep.
    /// Survives level unload — entity-type descriptors are global, not
    /// per-level. `App` reaches this registry via `script_ctx.data_registry`;
    /// no separate handle is held on `App`.
    pub(crate) data_registry: Rc<RefCell<DataRegistry>>,
    /// Bounded ring buffer of `GameEvent`s emitted by `emitEvent`. Drained at
    /// the end of the Game logic phase by main.rs (each entry surfaces as a
    /// `log::info!` on the `game_events` target; observable with
    /// `RUST_LOG=game_events=info`). Distinct from the `events` broadcast
    /// queue — the two cannot be unified: `events` items are removed on
    /// delivery to registered handlers, while entries here persist until the
    /// end-of-tick drain regardless of whether any handler is registered,
    /// providing a reliable observability tap independent of handler state.
    pub(crate) game_events: Rc<RefCell<VecDeque<GameEvent>>>,
    /// Engine frame counter, incremented once at the start of the Game logic
    /// phase. `emitEvent` stamps `GameEvent.frame` from this so each drain log
    /// line is ordered.
    pub(crate) frame: Rc<Cell<u64>>,
}

impl ScriptCtx {
    pub(crate) fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            events: Rc::new(RefCell::new(EventQueue::new())),
            handlers: Rc::new(RefCell::new(HandlerTable::new())),
            data_registry: Rc::new(RefCell::new(DataRegistry::new())),
            game_events: Rc::new(RefCell::new(VecDeque::with_capacity(GAME_EVENTS_CAPACITY))),
            frame: Rc::new(Cell::new(0)),
        }
    }
}
