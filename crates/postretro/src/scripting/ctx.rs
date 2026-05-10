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
    /// Runtime world gravity in m/s² (negative = downward; Earth = -9.81).
    /// Sole owner — `App` does not hold a parallel handle. Seeded from the
    /// worldspawn `initialGravity` PRL field on every level load via
    /// `self.script_ctx.gravity.set(...)`, mutated by the `worldSetGravity`
    /// primitive through the captured `ScriptCtx` clone, and read each frame
    /// by `App` (`script_ctx.gravity.get()`) to pass into `particle_sim::tick`
    /// for buoyancy integration. The `Cell` lets the primitive closures mutate
    /// without a `&mut ScriptCtx` borrow.
    pub(crate) gravity: Rc<Cell<f32>>,
}

impl ScriptCtx {
    pub(crate) fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            data_registry: Rc::new(RefCell::new(DataRegistry::new())),
            frame: Rc::new(Cell::new(0)),
            // Seeded to NaN so any code path that constructs `ScriptCtx`
            // without going through `prl::load_prl` (which seeds from
            // `LevelWorld.initial_gravity` via the `!level_load_fired` cold
            // path before the first frame) surfaces immediately — NaN
            // propagates through `particle_sim::tick` and is visually
            // obvious. The `worldSetGravity` primitive rejects non-finite
            // writes, so scripts cannot reintroduce this sentinel.
            gravity: Rc::new(Cell::new(f32::NAN)),
        }
    }
}
