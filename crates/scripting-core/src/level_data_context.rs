// The level data context marker: set while a level's data script runs
// (`ScriptRuntime::run_data_script`), so a primitive that only makes sense
// inside a level — the map-member query behind `getMapEntities` — can raise
// everywhere else (a mod start script, mod init, the definition context).
// Primitive scope is otherwise advisory (`scripting.md` §3); this is the one
// enforced check, keyed on the data-context lifetime rather than on which VM
// installed the primitive, because both contexts install every primitive.
// See: context/lib/scripting.md §2 (Data context lifecycle), §12 (Entity addressing)

use std::cell::Cell;
use std::marker::PhantomData;

thread_local! {
    // Script VMs run on the thread that owns the runtime, and the data context
    // is created and dropped within one `run_data_script` call, so a
    // thread-scoped depth is exactly the data context's lifetime.
    static LEVEL_DATA_CONTEXT_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Marks the current thread as running a level's data script until dropped.
/// Nests: the marker clears when the outermost guard drops.
#[must_use = "the level data context ends when the guard drops"]
pub struct LevelDataContext {
    // `!Send`: the marker is thread-scoped, so the guard must drop on the
    // thread that entered it.
    _thread_bound: PhantomData<*const ()>,
}

impl LevelDataContext {
    pub fn enter() -> Self {
        LEVEL_DATA_CONTEXT_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self {
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for LevelDataContext {
    fn drop(&mut self) {
        LEVEL_DATA_CONTEXT_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Whether a level's data script is running on this thread.
pub fn in_level_data_context() -> bool {
    LEVEL_DATA_CONTEXT_DEPTH.with(|depth| depth.get() > 0)
}
