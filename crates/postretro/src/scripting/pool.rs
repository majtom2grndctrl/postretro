// Ephemeral script context pools, one per language.
// See: context/lib/scripting.md
//
// Pooled contexts are NOT isolation boundaries. Scripts writing to `globalThis`
// (QuickJS) can leave state for the next acquirer. All persistent entity state
// must flow through Rust via `setComponent`/`getComponent`. Luau's
// `sandbox(true)` already blocks new global writes at the VM level.
//
// The shared behavior context is NEVER pooled — it carries event-handler
// globals for the level's lifetime. This pool is for future per-entity
// ephemeral contexts.
//
// Deliberately `!Send`: `Rc<RefCell<_>>` + `!Send` runtimes. `RefCell` rather
// than `RwLock` because `RefCell` does not poison and every FFI crossing is
// `catch_unwind`-wrapped.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use rquickjs::{Context, Runtime};

use super::error::ScriptError;
use super::primitives_registry::{ContextScope, ScriptPrimitive};

/// 32 is generous for day-one usage but cheap — a pre-warmed QuickJS `Context`
/// is a few KB; mlua's `Lua` scales similarly.
pub(crate) const DEFAULT_POOL_SIZE: usize = 32;

struct QuickJsPoolInner {
    runtime: Runtime,
    idle: VecDeque<Context>,
    in_flight: usize,
    primitives: Vec<ScriptPrimitive>,
}

pub(crate) struct QuickJsContextPool {
    inner: Rc<RefCell<QuickJsPoolInner>>,
}

impl QuickJsContextPool {
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

    /// Returns `None` when fully occupied. Takes `&self` because drop-returning
    /// handles need interior mutability to borrow on release.
    pub(crate) fn acquire(&self) -> Option<PooledQuickJsContext> {
        let mut inner = self.inner.borrow_mut();
        let ctx = inner.idle.pop_front()?;
        inner.in_flight += 1;
        Some(PooledQuickJsContext {
            ctx: Some(ctx),
            pool: self.inner.clone(),
        })
    }

    /// Falls back to synchronous creation when exhausted; fallback grows the
    /// pool on drop. Logs a warning on the fallback path.
    pub(crate) fn acquire_or_create(&self) -> Result<PooledQuickJsContext, ScriptError> {
        if let Some(h) = self.acquire() {
            return Ok(h);
        }
        log::warn!(
            target: "script/pool",
            "QuickJsContextPool exhausted; creating a fallback context synchronously",
        );
        // Clone before building: `build_pool_context` borrows `&Runtime` and
        // executes user primitives, so we can't hold the borrow across it.
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

    pub(crate) fn idle_len(&self) -> usize {
        self.inner.borrow().idle.len()
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.inner.borrow().in_flight
    }

    /// Grows when `acquire_or_create` allocates past the initial size.
    pub(crate) fn capacity(&self) -> usize {
        let inner = self.inner.borrow();
        inner.idle.len() + inner.in_flight
    }
}

pub(crate) struct PooledQuickJsContext {
    ctx: Option<Context>,  // Option so Drop can move out before returning
    pool: Rc<RefCell<QuickJsPoolInner>>,
}

impl PooledQuickJsContext {
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
            inner.in_flight = inner.in_flight.saturating_sub(1); // double-drop prevented by Option::take
        }
    }
}

fn build_pool_context(
    runtime: &Runtime,
    primitives: &[ScriptPrimitive],
) -> Result<Context, ScriptError> {
    let ctx = Context::full(runtime).map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })?;
    ctx.with(|ctx| -> Result<(), ScriptError> {
        install_pool_primitives(&ctx, primitives)?;
        super::quickjs::evaluate_prelude(&ctx)?;
        ctx.eval::<(), _>("Object.freeze(globalThis);")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: e.to_string(),
            })?;
        Ok(())
    })?;
    Ok(ctx)
}

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
        installer(ctx).map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

/// No per-entity globals yet; extend this when they land. Stray `globalThis`
/// writes by the current acquirer are NOT cleared — tolerated until then.
/// No `Runtime::run_gc` here: per-release GC serializes a full-heap pass in
/// spawn-burst scenarios; GC scheduling belongs in the frame loop.
fn reset_quickjs_context(ctx: &Context) {
    ctx.with(|ctx| {
        let _ = ctx;
    });
}

struct LuauPoolInner {
    idle: VecDeque<mlua::Lua>,
    in_flight: usize,
    primitives: Vec<ScriptPrimitive>,
}

pub(crate) struct LuauContextPool {
    inner: Rc<RefCell<LuauPoolInner>>,
}

impl LuauContextPool {
    pub(crate) fn new(primitives: &[ScriptPrimitive], size: usize) -> Result<Self, ScriptError> {
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
    let lua = mlua::Lua::new();
    apply_denylist(&lua)?;
    install_print_redirect(&lua)?;
    install_behavior_primitives(&lua, primitives)?;
    super::luau::evaluate_prelude(&lua)?;
    lua.sandbox(true)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    Ok(lua)
}

/// Duplicates `luau::DENIED_GLOBALS` / `DENIED_OS_FIELDS` (private to
/// `luau.rs`). Keep both lists in sync when either grows.
fn apply_denylist(lua: &mlua::Lua) -> Result<(), ScriptError> {
    const DENIED_GLOBALS: &[&str] = &["io", "package", "require", "dofile", "loadfile", "load"];
    const DENIED_OS_FIELDS: &[&str] = &["execute", "exit", "getenv"];

    let globals = lua.globals();
    for name in DENIED_GLOBALS {
        globals
            .set(*name, mlua::Value::Nil)
            .map_err(|e| ScriptError::InvalidArgument {
                reason: e.to_string(),
            })?;
    }
    if let Ok(os_table) = globals.get::<mlua::Table>("os") {
        for field in DENIED_OS_FIELDS {
            os_table
                .set(*field, mlua::Value::Nil)
                .map_err(|e| ScriptError::InvalidArgument {
                    reason: e.to_string(),
                })?;
        }
    }
    Ok(())
}

fn install_print_redirect(lua: &mlua::Lua) -> Result<(), ScriptError> {
    let f = lua
        .create_function(|_lua, args: mlua::MultiValue| {
            const NAME: &str = "print";
            let result = catch_unwind(AssertUnwindSafe(|| {
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
            }));
            match result {
                Ok(()) => Ok(()),
                Err(_) => {
                    log::error!(target: "script/luau", "[Scripting] print closure panicked: {NAME}");
                    Err(mlua::Error::RuntimeError(format!("panic in print: {NAME}")))
                }
            }
        })
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    lua.globals()
        .set("print", f)
        .map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
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
        installer(lua).map_err(|e| ScriptError::InvalidArgument {
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

/// Extend here when per-entity globals land (see `reset_quickjs_context`).
fn reset_lua(lua: &mlua::Lua) {
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

        handle.context().with(|ctx| {
            let v: bool = ctx.eval("entityExists(0)").unwrap();
            assert!(!v);
        });
    }

    #[test]
    fn quickjs_release_returns_same_context_and_clears_in_flight() {
        let rt = runtime();
        let pool = QuickJsContextPool::new(&rt, &primitives(), 1).unwrap();

        {
            let _h = pool.acquire().unwrap();
        }
        assert_eq!(pool.in_flight(), 0);
        assert_eq!(pool.idle_len(), 1);

        let h2 = pool.acquire().unwrap();
        h2.context().with(|ctx| {
            let v: bool = ctx.eval("entityExists(0)").unwrap();
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

        let h3 = pool.acquire_or_create().expect("fallback should succeed");
        assert_eq!(pool.in_flight(), 3);
        assert_eq!(pool.capacity(), 3);
        drop(h3); // capacity stays at 3 (fallback grew the pool)
        assert_eq!(pool.idle_len(), 1);
        assert_eq!(pool.in_flight(), 2);
        assert_eq!(pool.capacity(), 3);
    }

    #[test]
    fn quickjs_definition_primitive_is_stubbed_in_pool() {
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

    #[test]
    fn luau_pool_prewarms_with_primitives_installed() {
        let pool = LuauContextPool::new(&primitives(), 4).unwrap();
        assert_eq!(pool.idle_len(), 4);
        assert_eq!(pool.in_flight(), 0);

        let handle = pool.acquire().unwrap();
        assert_eq!(pool.idle_len(), 3);
        assert_eq!(pool.in_flight(), 1);

        let v: bool = handle.lua().load("return entityExists(0)").eval().unwrap();
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

        let h2 = pool.acquire().unwrap();
        let v: bool = h2.lua().load("return entityExists(0)").eval().unwrap();
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

        let v: bool = h3.lua().load("return entityExists(0)").eval().unwrap();
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

}
