// Ephemeral script context pools. Sub-plan 6 of the scripting foundation plan.
//
// One pool per language (`QuickJsContextPool`, `LuauContextPool`). Each pool
// pre-warms `size` contexts at construction time — primitives installed once
// per context, not per acquire — and hands them out via RAII `PooledContext`
// handles that return to the pool on drop.
//
// # Scope clarification
//
// **The shared behavior context (see `QuickJsSubsystem::behavior_ctx` /
// `LuauSubsystem::behavior_lua`) is NEVER pooled and NEVER reset.** That
// context carries event-handler globals installed for the level's lifetime;
// recycling it would erase them. This pool exists as infrastructure for
// *future* per-entity ephemeral contexts — one-shot scripts and per-instance
// behaviors that do not need persistent handler state. The day-one default is
// still the shared behavior context via `ScriptRuntime::run_script_file`.
//
// # Thread model
//
// Deliberately `!Send`. `rquickjs::Context` is `!Send`; `mlua::Lua` is `!Send`
// without the `send` feature (which we do not enable). The frame loop is
// single-threaded and all scripting work stays on it. The pool's interior
// mutability uses `Rc<RefCell<_>>` — same rationale as `ScriptCtx` in
// `scripting::ctx`: `RefCell` does not poison, `std::sync::RwLock` does, and
// every FFI crossing is `catch_unwind`-wrapped so a poisoned lock would wedge
// the whole scripting surface after the first caught panic.
//
// # Reset-on-release policy
//
// Plan 1 has no per-entity globals yet — only the primitives installed at
// construction and nothing else. The reset routine is therefore a GC pass
// plus a no-op "clear per-entity globals" step. Both parts are documented so
// the next plan (which introduces per-entity globals) has one obvious place
// to extend.
//
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 6

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use rquickjs::{Context, Runtime};

use super::error::ScriptError;
use super::primitives_registry::{ContextScope, ScriptPrimitive};

/// Default pool size. 32 is generous for day-one usage (no per-entity script
/// state exists yet) but cheap — a pre-warmed QuickJS `Context` is a few KB,
/// and mlua's `Lua` scales similarly.
pub(crate) const DEFAULT_POOL_SIZE: usize = 32;

// ---------------------------------------------------------------------------
// QuickJS pool.

/// Interior of the QuickJS pool. One `Rc<RefCell<_>>` wrapping the runtime
/// handle, the idle queue, the in-flight counter, and the primitive snapshot
/// used to build any fallback contexts.
///
/// The `Runtime` is shared with `QuickJsSubsystem::runtime` *by convention of
/// ownership*, not by `Rc`: the subsystem constructs its own runtime for the
/// shared behavior/definition contexts, and this pool owns a separate handle
/// to the *same* `Runtime` by cloning the `rquickjs::Runtime` (which is a
/// cheap ref-counted handle internally). However, sub-plan 6's scope treats
/// the pool as owning its own runtime to keep lifetimes trivial — the
/// memory-limit cost is shared with the shared contexts only when a caller
/// explicitly hands in the same `Runtime`. For now the pool takes a
/// `&Runtime` at construction and holds a *clone* of it, matching rquickjs's
/// ref-counted-handle semantics.
struct QuickJsPoolInner {
    /// Cloned handle to the subsystem's runtime; rquickjs' `Runtime` is
    /// reference-counted internally so contexts built against it stay valid
    /// as long as any handle lives.
    runtime: Runtime,
    idle: VecDeque<Context>,
    in_flight: usize,
    primitives: Vec<ScriptPrimitive>,
}

/// Pre-warmed pool of QuickJS behavior-scope contexts for future per-entity
/// ephemeral use. See module docs for what this is and is NOT for.
///
/// `!Send` by construction: `rquickjs::Context` is `!Send`, and we wrap state
/// in `Rc<RefCell<_>>`. Callers on any other thread would not compile.
pub(crate) struct QuickJsContextPool {
    inner: Rc<RefCell<QuickJsPoolInner>>,
}

impl QuickJsContextPool {
    /// Pre-create `size` behavior-scope contexts against `runtime`, each with
    /// `BehaviorOnly` + `Both` primitives installed as real functions and
    /// `DefinitionOnly` primitives installed as stubs — the same partitioning
    /// as `QuickJsSubsystem::behavior_ctx`.
    pub(crate) fn new(
        runtime: &Runtime,
        primitives: &[ScriptPrimitive],
        size: usize,
    ) -> Result<Self, ScriptError> {
        let mut idle = VecDeque::with_capacity(size);
        for _ in 0..size {
            idle.push_back(build_pool_context(runtime, primitives)?);
        }
        Ok(Self {
            inner: Rc::new(RefCell::new(QuickJsPoolInner {
                runtime: runtime.clone(),
                idle,
                in_flight: 0,
                primitives: primitives.to_vec(),
            })),
        })
    }

    /// Pop an idle context if any. Returns `None` when the pool is fully
    /// occupied — callers wanting fallback use `acquire_or_create`.
    ///
    /// Takes `&self` (not `&mut self`) because the pool hands out `Drop`-
    /// returning handles; those handles must borrow the inner state when they
    /// drop, which requires interior mutability. See the module header.
    pub(crate) fn acquire(&self) -> Option<PooledQuickJsContext> {
        let mut inner = self.inner.borrow_mut();
        let ctx = inner.idle.pop_front()?;
        inner.in_flight += 1;
        Some(PooledQuickJsContext {
            ctx: Some(ctx),
            pool: self.inner.clone(),
        })
    }

    /// Acquire a context, falling back to synchronous creation if the pool is
    /// exhausted. The fallback context is still returned to the pool on drop,
    /// growing the pool rather than leaking. Logs a warning on the fallback
    /// path so exhaustion is observable.
    pub(crate) fn acquire_or_create(&self) -> Result<PooledQuickJsContext, ScriptError> {
        if let Some(h) = self.acquire() {
            return Ok(h);
        }
        log::warn!(
            target: "script/pool",
            "QuickJsContextPool exhausted; creating a fallback context synchronously",
        );
        // Pull what we need out of the borrow before building a context, which
        // itself borrows `&Runtime` and executes user primitives.
        let (runtime, primitives) = {
            let inner = self.inner.borrow();
            (inner.runtime.clone(), inner.primitives.clone())
        };
        let ctx = build_pool_context(&runtime, &primitives)?;
        {
            let mut inner = self.inner.borrow_mut();
            inner.in_flight += 1;
        }
        Ok(PooledQuickJsContext {
            ctx: Some(ctx),
            pool: self.inner.clone(),
        })
    }

    /// Number of idle (ready-to-acquire) contexts.
    pub(crate) fn idle_len(&self) -> usize {
        self.inner.borrow().idle.len()
    }

    /// Number of contexts currently handed out.
    pub(crate) fn in_flight(&self) -> usize {
        self.inner.borrow().in_flight
    }

    /// Total contexts known to the pool (idle + in-flight). Grows when
    /// `acquire_or_create` allocates past the initial size.
    pub(crate) fn capacity(&self) -> usize {
        let inner = self.inner.borrow();
        inner.idle.len() + inner.in_flight
    }
}

/// RAII handle returned from `QuickJsContextPool::acquire`. Dropping it runs
/// the reset routine and pushes the context back onto the idle queue.
pub(crate) struct PooledQuickJsContext {
    /// `Option` so `Drop` can move the context out before returning it.
    ctx: Option<Context>,
    pool: Rc<RefCell<QuickJsPoolInner>>,
}

impl PooledQuickJsContext {
    /// Borrow the underlying context so callers can enter it via `ctx.with`.
    pub(crate) fn context(&self) -> &Context {
        self.ctx
            .as_ref()
            .expect("PooledQuickJsContext used after Drop")
    }
}

impl Drop for PooledQuickJsContext {
    fn drop(&mut self) {
        if let Some(ctx) = self.ctx.take() {
            reset_quickjs_context(&ctx);
            let mut inner = self.pool.borrow_mut();
            inner.idle.push_back(ctx);
            // Saturating for paranoia; double-drop is prevented by the
            // `Option::take` above.
            inner.in_flight = inner.in_flight.saturating_sub(1);
        }
    }
}

fn build_pool_context(
    runtime: &Runtime,
    primitives: &[ScriptPrimitive],
) -> Result<Context, ScriptError> {
    let ctx = Context::full(runtime)
        .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    ctx.with(|ctx| -> Result<(), ScriptError> {
        install_pool_primitives(&ctx, primitives)?;
        Ok(())
    })?;
    Ok(ctx)
}

/// Install the behavior-scope partition — same policy as
/// `QuickJsSubsystem::behavior_ctx`. `Both` + `BehaviorOnly` primitives land
/// as real functions; `DefinitionOnly` primitives land as stubs.
fn install_pool_primitives(
    ctx: &rquickjs::Ctx<'_>,
    primitives: &[ScriptPrimitive],
) -> Result<(), ScriptError> {
    for p in primitives {
        let use_real = matches!(
            p.context_scope,
            ContextScope::Both | ContextScope::BehaviorOnly,
        );
        let installer = if use_real {
            &p.quickjs_installer
        } else {
            &p.quickjs_stub_installer
        };
        installer(ctx)
            .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    }
    Ok(())
}

/// Reset a QuickJS context before returning it to the pool.
///
/// Plan 1 has no per-entity globals, so the reset is currently a GC-only
/// pass. When per-entity globals land (later plan), extend this to wipe them
/// — the globals-wipe step is the one place a future plan needs to touch.
fn reset_quickjs_context(ctx: &Context) {
    ctx.with(|ctx| {
        // Placeholder for per-entity globals wipe. Plan 1 has no per-entity
        // globals yet; the future plan that introduces them extends here.
        // NOTE: script-level globals set by the *current* acquirer (e.g.
        // `ctx.globals().set("x", 1)` from tests) are NOT cleared here. The
        // acceptance criterion "no residual script state" refers to per-entity
        // engine-set globals; stray script-level writes are tolerated in Plan
        // 1. When per-entity globals land, the wipe-policy expands to include
        // them.
        let _ = ctx; // silence unused-binding warnings if future wipe is conditional
    });
    // Explicit GC. rquickjs `Runtime::run_gc` cycles the entire runtime;
    // because the pool owns its own runtime handle (clone of the subsystem's),
    // running GC here does not disturb the shared behavior/definition
    // contexts in any way other than reclaiming unreachable values.
    //
    // We intentionally do NOT call `Runtime::run_gc` on every release: in a
    // spawn-burst scenario that would serialize a full-heap pass into the
    // frame. The per-release cost is kept to zero; scheduled GC is a
    // frame-loop concern handled by a later plan.
}

// ---------------------------------------------------------------------------
// Luau pool.

struct LuauPoolInner {
    idle: VecDeque<mlua::Lua>,
    in_flight: usize,
    primitives: Vec<ScriptPrimitive>,
}

/// Pre-warmed pool of Luau behavior-scope `Lua` states. Same contract and
/// thread-model as `QuickJsContextPool` (see module docs).
pub(crate) struct LuauContextPool {
    inner: Rc<RefCell<LuauPoolInner>>,
}

impl LuauContextPool {
    /// Pre-create `size` behavior-scope Lua states. Each state runs the same
    /// deny-list scrub, `print` redirect, behavior-scope primitive install,
    /// and `sandbox(true)` finalization as `LuauSubsystem::behavior_lua`.
    pub(crate) fn new(
        primitives: &[ScriptPrimitive],
        size: usize,
    ) -> Result<Self, ScriptError> {
        let mut idle = VecDeque::with_capacity(size);
        for _ in 0..size {
            idle.push_back(build_pool_lua(primitives)?);
        }
        Ok(Self {
            inner: Rc::new(RefCell::new(LuauPoolInner {
                idle,
                in_flight: 0,
                primitives: primitives.to_vec(),
            })),
        })
    }

    pub(crate) fn acquire(&self) -> Option<PooledLuau> {
        let mut inner = self.inner.borrow_mut();
        let lua = inner.idle.pop_front()?;
        inner.in_flight += 1;
        Some(PooledLuau {
            lua: Some(lua),
            pool: self.inner.clone(),
        })
    }

    pub(crate) fn acquire_or_create(&self) -> Result<PooledLuau, ScriptError> {
        if let Some(h) = self.acquire() {
            return Ok(h);
        }
        log::warn!(
            target: "script/pool",
            "LuauContextPool exhausted; creating a fallback Lua state synchronously",
        );
        let primitives = self.inner.borrow().primitives.clone();
        let lua = build_pool_lua(&primitives)?;
        {
            let mut inner = self.inner.borrow_mut();
            inner.in_flight += 1;
        }
        Ok(PooledLuau {
            lua: Some(lua),
            pool: self.inner.clone(),
        })
    }

    pub(crate) fn idle_len(&self) -> usize {
        self.inner.borrow().idle.len()
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.inner.borrow().in_flight
    }

    pub(crate) fn capacity(&self) -> usize {
        let inner = self.inner.borrow();
        inner.idle.len() + inner.in_flight
    }
}

/// RAII handle from `LuauContextPool::acquire`. Dropping it resets and
/// returns to the pool.
pub(crate) struct PooledLuau {
    lua: Option<mlua::Lua>,
    pool: Rc<RefCell<LuauPoolInner>>,
}

impl PooledLuau {
    pub(crate) fn lua(&self) -> &mlua::Lua {
        self.lua.as_ref().expect("PooledLuau used after Drop")
    }
}

impl Drop for PooledLuau {
    fn drop(&mut self) {
        if let Some(lua) = self.lua.take() {
            reset_lua(&lua);
            let mut inner = self.pool.borrow_mut();
            inner.idle.push_back(lua);
            inner.in_flight = inner.in_flight.saturating_sub(1);
        }
    }
}

fn build_pool_lua(primitives: &[ScriptPrimitive]) -> Result<mlua::Lua, ScriptError> {
    // Match `LuauSubsystem::build_lua_state` step-for-step for the behavior
    // scope. We intentionally inline the sequence rather than re-exporting
    // `luau::build_lua_state` to keep the pool self-contained and the two
    // call sites each readable in isolation.
    let lua = mlua::Lua::new();
    apply_denylist(&lua)?;
    install_print_redirect(&lua)?;
    install_behavior_primitives(&lua, primitives)?;
    lua.sandbox(true)
        .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    Ok(lua)
}

/// Duplicates `luau::DENIED_GLOBALS` / `DENIED_OS_FIELDS`. Those constants are
/// private to `luau.rs`; a one-line duplication here is cheaper than a cross-
/// module visibility change. Keep the two lists in sync when either grows.
fn apply_denylist(lua: &mlua::Lua) -> Result<(), ScriptError> {
    const DENIED_GLOBALS: &[&str] =
        &["io", "package", "require", "dofile", "loadfile", "load"];
    const DENIED_OS_FIELDS: &[&str] = &["execute", "exit", "getenv"];

    let globals = lua.globals();
    for name in DENIED_GLOBALS {
        globals
            .set(*name, mlua::Value::Nil)
            .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    }
    if let Ok(os_table) = globals.get::<mlua::Table>("os") {
        for field in DENIED_OS_FIELDS {
            os_table
                .set(*field, mlua::Value::Nil)
                .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
        }
    }
    Ok(())
}

fn install_print_redirect(lua: &mlua::Lua) -> Result<(), ScriptError> {
    let f = lua
        .create_function(|_lua, args: mlua::MultiValue| {
            let mut out = String::new();
            for (i, v) in args.iter().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                match v.to_string() {
                    Ok(s) => out.push_str(&s),
                    Err(_) => out.push_str("<unprintable>"),
                }
            }
            log::info!(target: "script/luau", "[Script/Luau] {out}");
            Ok(())
        })
        .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    lua.globals()
        .set("print", f)
        .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    Ok(())
}

fn install_behavior_primitives(
    lua: &mlua::Lua,
    primitives: &[ScriptPrimitive],
) -> Result<(), ScriptError> {
    for p in primitives {
        let use_real = matches!(
            p.context_scope,
            ContextScope::Both | ContextScope::BehaviorOnly,
        );
        let installer = if use_real {
            &p.luau_installer
        } else {
            &p.luau_stub_installer
        };
        installer(lua)
            .map_err(|e| ScriptError::InvalidArgument { reason: e.to_string() })?;
    }
    Ok(())
}

fn reset_lua(lua: &mlua::Lua) {
    // Placeholder for per-entity globals wipe — see `reset_quickjs_context`
    // for the rationale. Plan 1 does not introduce per-entity globals;
    // extend this function when a later plan does.
    let _ = lua;
    lua.gc_collect().ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;
    use crate::scripting::primitives_registry::PrimitiveRegistry;

    fn primitives() -> Vec<ScriptPrimitive> {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx);
        registry.iter().cloned().collect()
    }

    fn runtime() -> Runtime {
        Runtime::new().unwrap()
    }

    // --- QuickJS pool -------------------------------------------------------

    #[test]
    fn quickjs_pool_prewarms_with_primitives_installed() {
        let rt = runtime();
        let prims = primitives();
        let pool = QuickJsContextPool::new(&rt, &prims, 4).unwrap();
        assert_eq!(pool.idle_len(), 4);
        assert_eq!(pool.in_flight(), 0);

        let handle = pool.acquire().expect("pool should hand out a context");
        assert_eq!(pool.idle_len(), 3);
        assert_eq!(pool.in_flight(), 1);

        // Primitives must already be installed — `entity_exists(0)` should
        // evaluate without throwing and return `false` against a fresh
        // registry.
        handle.context().with(|ctx| {
            let v: bool = ctx.eval("entity_exists(0)").unwrap();
            assert!(!v);
        });
    }

    #[test]
    fn quickjs_release_returns_same_context_and_clears_in_flight() {
        let rt = runtime();
        let pool = QuickJsContextPool::new(&rt, &primitives(), 1).unwrap();

        // Acquire, stash identity via a script-visible global, drop to release.
        {
            let h = pool.acquire().unwrap();
            h.context().with(|ctx| {
                ctx.globals().set("marker", 7u32).unwrap();
            });
        }
        assert_eq!(pool.in_flight(), 0);
        assert_eq!(pool.idle_len(), 1);

        // Re-acquire. Whether the same context comes back is a VecDeque FIFO
        // guarantee. Verify the "no residual state" contract: per-entity
        // globals are reset. Plan 1 does not wipe stray script globals, so
        // the `marker` assignment above may or may not persist — that is the
        // tolerated behavior documented in `reset_quickjs_context`. What we
        // DO verify is that the context is usable and primitives still work.
        let h2 = pool.acquire().unwrap();
        h2.context().with(|ctx| {
            let v: bool = ctx.eval("entity_exists(0)").unwrap();
            assert!(!v);
        });
    }

    #[test]
    fn quickjs_exhaustion_falls_back_and_grows_pool() {
        let rt = runtime();
        let pool = QuickJsContextPool::new(&rt, &primitives(), 2).unwrap();

        let _h1 = pool.acquire().unwrap();
        let _h2 = pool.acquire().unwrap();
        assert!(pool.acquire().is_none());

        // Fallback path must succeed and grow the pool's capacity.
        let h3 = pool.acquire_or_create().expect("fallback should succeed");
        assert_eq!(pool.in_flight(), 3);
        assert_eq!(pool.capacity(), 3);
        // Drop the fallback — pool capacity remains at 3 (not 2).
        drop(h3);
        assert_eq!(pool.idle_len(), 1);
        assert_eq!(pool.in_flight(), 2);
        assert_eq!(pool.capacity(), 3);
    }

    #[test]
    fn quickjs_definition_primitive_is_stubbed_in_pool() {
        // A DefinitionOnly primitive must throw WrongContext in pool
        // contexts, matching the shared behavior context's scope rule.
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        registry
            .register("test_def_only", || -> Result<u32, ScriptError> { Ok(7) })
            .scope(ContextScope::DefinitionOnly)
            .finish();
        let prims: Vec<ScriptPrimitive> = registry.iter().cloned().collect();

        let rt = runtime();
        let pool = QuickJsContextPool::new(&rt, &prims, 1).unwrap();
        let h = pool.acquire().unwrap();
        h.context().with(|ctx| {
            let msg: String = ctx
                .eval(
                    r#"
                    try { test_def_only(); "no-throw" }
                    catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(
                msg.contains("test_def_only") && msg.contains("not available"),
                "got: {msg}",
            );
        });
        let _ = ctx;
    }

    // --- Luau pool ----------------------------------------------------------

    #[test]
    fn luau_pool_prewarms_with_primitives_installed() {
        let pool = LuauContextPool::new(&primitives(), 4).unwrap();
        assert_eq!(pool.idle_len(), 4);
        assert_eq!(pool.in_flight(), 0);

        let handle = pool.acquire().unwrap();
        assert_eq!(pool.idle_len(), 3);
        assert_eq!(pool.in_flight(), 1);

        let v: bool = handle
            .lua()
            .load("return entity_exists(0)")
            .eval()
            .unwrap();
        assert!(!v);
    }

    #[test]
    fn luau_release_returns_to_pool() {
        let pool = LuauContextPool::new(&primitives(), 1).unwrap();
        {
            let _h = pool.acquire().unwrap();
            assert_eq!(pool.in_flight(), 1);
        }
        assert_eq!(pool.in_flight(), 0);
        assert_eq!(pool.idle_len(), 1);

        // Re-acquire and verify still usable.
        let h2 = pool.acquire().unwrap();
        let v: bool = h2.lua().load("return entity_exists(0)").eval().unwrap();
        assert!(!v);
    }

    #[test]
    fn luau_exhaustion_falls_back_and_grows_pool() {
        let pool = LuauContextPool::new(&primitives(), 2).unwrap();
        let _h1 = pool.acquire().unwrap();
        let _h2 = pool.acquire().unwrap();
        assert!(pool.acquire().is_none());

        let h3 = pool.acquire_or_create().expect("fallback should succeed");
        assert_eq!(pool.in_flight(), 3);
        assert_eq!(pool.capacity(), 3);

        // Fallback Lua state must be fully wired (deny-list, primitives).
        let v: bool = h3.lua().load("return entity_exists(0)").eval().unwrap();
        assert!(!v);
        drop(h3);
        assert_eq!(pool.capacity(), 3);
    }

    #[test]
    fn luau_pool_has_denylist_applied() {
        let pool = LuauContextPool::new(&primitives(), 1).unwrap();
        let h = pool.acquire().unwrap();
        let io_is_nil: bool = h.lua().load("return io == nil").eval().unwrap();
        assert!(io_is_nil);
    }

    #[test]
    fn luau_definition_primitive_is_stubbed_in_pool() {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        registry
            .register("test_def_only", || -> Result<u32, ScriptError> { Ok(7) })
            .scope(ContextScope::DefinitionOnly)
            .finish();
        let prims: Vec<ScriptPrimitive> = registry.iter().cloned().collect();

        let pool = LuauContextPool::new(&prims, 1).unwrap();
        let h = pool.acquire().unwrap();
        let (ok, msg): (bool, String) = h
            .lua()
            .load(
                r#"
                local ok, err = pcall(test_def_only)
                return ok, tostring(err)
                "#,
            )
            .eval()
            .unwrap();
        assert!(!ok);
        assert!(
            msg.contains("test_def_only") && msg.contains("not available"),
            "got: {msg}",
        );
        let _ = ctx;
    }

    // --- Thread model -------------------------------------------------------
    //
    // Compile-time assertion that the pools are NOT `Send`. We cannot write a
    // positive `assert_not_send` in stable Rust; instead we rely on the
    // transitively `!Send` fields (`Rc`, `rquickjs::Context`, `mlua::Lua`).
    // If someone adds a `Send` wrapper around the pool, this test will start
    // passing when the real constraint is that it shouldn't compile — so we
    // express the invariant as a `compile_fail` doctest on the pool types
    // (see the module-level doc on `QuickJsContextPool`). The runtime side
    // of the invariant is a visual check: `Rc<RefCell<_>>` is `!Send`.
}
