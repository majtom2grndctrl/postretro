// Script context handle.
// See: context/lib/scripting.md
//
// `ScriptCtx` captures-by-Rc into every primitive closure at registration time.
// `!Send + !Sync` by design: the frame loop is single-threaded and both
// runtimes are `!Send`. `Rc<RefCell<_>>` over `Arc<RwLock<_>>` because
// `RwLock` poisons on panic — every primitive runs inside `catch_unwind`,
// so a poisoned lock would wedge the scripting surface after the first caught
// panic. `RefCell` has no poisoning.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::data_registry::DataRegistry;
use super::registry::EntityRegistry;

/// Handle every primitive closure captures. Cloning is cheap — it bumps two
/// `Rc`s. The fields behind `Rc<RefCell<_>>` are the subsystems the scripting
/// surface is allowed to touch. Extend by adding one field per subsystem.
#[derive(Clone)]
pub(crate) struct ScriptCtx {
    pub(crate) registry: Rc<RefCell<EntityRegistry>>,
    /// Engine-wide entity-type registry written by the `registerEntity`
    /// primitive (data context) and consumed by the level-load spawn sweep.
    /// Survives level unload — entity-type descriptors are global, not
    /// per-level. `App` reaches this registry via `script_ctx.data_registry`;
    /// no separate handle is held on `App`.
    pub(crate) data_registry: Rc<RefCell<DataRegistry>>,
    /// Engine frame counter, incremented once at the start of the Game logic
    /// phase. No current consumers — reserved for future primitives that need
    /// a per-frame ordering stamp.
    pub(crate) frame: Rc<Cell<u64>>,
}

impl ScriptCtx {
    pub(crate) fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            data_registry: Rc::new(RefCell::new(DataRegistry::new())),
            frame: Rc::new(Cell::new(0)),
        }
    }
}
