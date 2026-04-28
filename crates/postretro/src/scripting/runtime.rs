// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. One construction path, one reload path, one fan-out point.
// See: context/lib/scripting.md
//
// Deliberately shallow — no abstraction over "script engine" since there are
// exactly two runtimes and they aren't pluggable.

use std::fs;
use std::path::Path;

use super::call_context::ScriptCallContext;
use super::ctx::ScriptCtx;
use super::error::ScriptError;
use super::event_dispatch::{self, SharedHandlerTable};
use super::luau::{LuauConfig, LuauSubsystem, Which as LuauWhich};
use super::pool::{LuauContextPool, QuickJsContextPool};
use super::primitives_registry::PrimitiveRegistry;
use super::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};
#[cfg(debug_assertions)]
use super::typedef;

/// Which scripting scope a given call targets. The subsystem-level `Which`
/// types (QuickJS, Luau) are private to their modules; this is the
/// engine-facing selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Definition,
    Behavior,
}

impl From<Which> for LuauWhich {
    fn from(w: Which) -> Self {
        match w {
            Which::Definition => LuauWhich::Definition,
            Which::Behavior => LuauWhich::Behavior,
        }
    }
}

/// Configuration for [`ScriptRuntime`]. Composes the two subsystem configs
/// by value.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScriptRuntimeConfig {
    pub(crate) quickjs: QuickJsConfig,
    pub(crate) luau: LuauConfig,
}

/// The unified scripting runtime.
pub(crate) struct ScriptRuntime {
    quickjs: QuickJsSubsystem,
    luau: LuauSubsystem,
    /// Ephemeral-context pool for future per-entity QuickJS scripting. The
    /// shared behavior context is NOT part of this pool (see `scripting::pool`).
    quickjs_pool: QuickJsContextPool,
    /// Ephemeral-context pool for future per-entity Luau scripting.
    luau_pool: LuauContextPool,
    /// Handler table populated by `registerHandler` and drained on level
    /// unload. Shared with the `ScriptCtx` the primitive registry captured.
    handlers: SharedHandlerTable,
    /// Dev-mode hot-reload watcher. Debug builds only; release builds omit
    /// the field so `drain_reload_requests` is a no-op with no extra code.
    #[cfg(debug_assertions)]
    watcher: Option<super::watcher::ScriptWatcher>,
}

impl ScriptRuntime {
    /// Construct both subsystems, pre-warm context pools, and emit SDK
    /// type-definition files in debug builds. IO failure is logged and
    /// swallowed — a missing `sdk/types` directory must not prevent startup.
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &ScriptRuntimeConfig,
        ctx: &ScriptCtx,
    ) -> Result<Self, ScriptError> {
        let quickjs = QuickJsSubsystem::new(registry, &cfg.quickjs)?;
        let luau = LuauSubsystem::new(registry, &cfg.luau)?;

        let quickjs_pool = QuickJsContextPool::new(
            quickjs.runtime(),
            quickjs.primitives(),
            cfg.quickjs.pool_size,
        )?;
        let luau_pool = LuauContextPool::new(luau.primitives(), cfg.luau.pool_size)?;

        #[cfg(debug_assertions)]
        typedef::emit_sdk_types_in_debug(registry);

        Ok(Self {
            quickjs,
            luau,
            quickjs_pool,
            luau_pool,
            handlers: ctx.handlers.clone(),
            #[cfg(debug_assertions)]
            watcher: None,
        })
    }

    /// Fire the `levelLoad` event. Iterates registered handlers in
    /// registration order; a throwing handler is logged and swallowed.
    /// See: context/plans/ready/scripting-foundation/plan-2-light-entity.md §Sub-plan 5
    pub(crate) fn fire_level_load(&self) {
        event_dispatch::fire_level_load(
            &self.handlers,
            self.quickjs.behavior_ctx(),
            self.luau.behavior_lua(),
        );
    }

    /// Fire the `tick` event. `ctx` carries `delta` and `time` from the engine
    /// frame timer. A throwing handler is logged and swallowed.
    pub(crate) fn fire_tick(&self, ctx: ScriptCallContext) {
        event_dispatch::fire_tick(
            &self.handlers,
            self.quickjs.behavior_ctx(),
            self.luau.behavior_lua(),
            ctx,
        );
    }

    /// Drop every registered handler. Called on level unload and hot reload —
    /// the handler registry is strictly per-level (see `scripting.md` §10
    /// Non-Goals); on hot reload, scripts re-register from a clean slate so
    /// handlers don't accumulate across reloads.
    pub(crate) fn clear_level_handlers(&self) {
        self.handlers.borrow_mut().clear();
    }

    /// Access the QuickJS ephemeral-context pool.
    pub(crate) fn quickjs_pool(&self) -> &QuickJsContextPool {
        &self.quickjs_pool
    }

    /// Access the Luau ephemeral-context pool.
    pub(crate) fn luau_pool(&self) -> &LuauContextPool {
        &self.luau_pool
    }

    /// Start the dev-mode file watcher. No-op in release builds (the method
    /// still exists so the frame-loop caller doesn't need a `cfg` gate).
    /// Calling twice replaces the previous watcher.
    pub(crate) fn start_watcher(&mut self, script_root: &Path) -> Result<(), ScriptError> {
        #[cfg(debug_assertions)]
        {
            let ts_compiler = super::watcher::TsCompilerPath::detect();
            let w = super::watcher::ScriptWatcher::spawn(script_root, ts_compiler)?;
            self.watcher = Some(w);
        }
        #[cfg(not(debug_assertions))]
        {
            // In release builds, hot reload is intentionally unavailable;
            // silently ignore so the caller can unconditionally invoke this.
            let _ = script_root;
        }
        Ok(())
    }

    /// Drain any pending reload requests produced by the watcher. Call at the
    /// top of each frame. Returns `Ok(true)` when at least one reload request
    /// was drained — the caller is responsible for the actual reload sequence
    /// (clear handlers, re-run behavior scripts, re-fire `levelLoad` if the
    /// level is loaded). No-op in release builds: always returns `Ok(false)`.
    pub(crate) fn drain_reload_requests(&mut self) -> Result<bool, ScriptError> {
        #[cfg(debug_assertions)]
        {
            if let Some(w) = self.watcher.as_mut() {
                return w.drain_reload_requests();
            }
        }
        Ok(false)
    }

    /// Access the QuickJS subsystem.
    pub(crate) fn quickjs(&self) -> &QuickJsSubsystem {
        &self.quickjs
    }

    /// Access the Luau subsystem.
    pub(crate) fn luau(&self) -> &LuauSubsystem {
        &self.luau
    }

    /// Reload both definition contexts. Called from the dev-mode hot-reload path.
    pub(crate) fn reload_definition_context(&mut self) -> Result<(), ScriptError> {
        self.quickjs.reload_definition_context()?;
        self.luau.reload_definition_context()?;
        Ok(())
    }

    /// Read `path` from disk and run it in the appropriate subsystem, chosen
    /// by extension:
    ///
    ///   * `.ts`, `.js`  → QuickJS
    ///   * `.luau`       → Luau
    ///
    /// `.ts` is accepted as a convenience for upstream layers that strip types
    /// before passing the file in; QuickJS parses it as plain JS. Unknown
    /// extensions return `ScriptError::InvalidArgument`.
    pub(crate) fn run_script_file(&self, which: Which, path: &Path) -> Result<(), ScriptError> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let source = fs::read_to_string(path).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to read script `{}`: {e}", path.display()),
        })?;
        let name = path.to_string_lossy().into_owned();

        // Publish the current script name so `registerHandler` can stamp it
        // onto any handlers this script installs. Always cleared at scope exit,
        // even on failure, so a thrown script cannot leak a stale name onto a
        // later file's handlers.
        self.handlers
            .borrow_mut()
            .set_current_source(Some(name.clone()));
        let _source_guard = SourceGuard {
            handlers: &self.handlers,
        };

        match ext {
            "ts" | "js" => {
                let ctx = match which {
                    Which::Definition => self.quickjs.definition_ctx(),
                    Which::Behavior => self.quickjs.behavior_ctx(),
                };
                ctx.with(|ctx| run_script::<()>(&ctx, &source, &name))?;
                Ok(())
            }
            "luau" => {
                self.luau.run_source::<()>(which.into(), &source, &name)?;
                Ok(())
            }
            other => Err(ScriptError::InvalidArgument {
                reason: format!(
                    "unsupported script extension `.{other}` for `{}` (expected .ts/.js/.luau)",
                    path.display(),
                ),
            }),
        }
    }
}

impl Drop for ScriptRuntime {
    /// Clear every registered handler before the QuickJS runtime is freed.
    /// Each registered handler carries a `Persistent<Function>` that pins a JS
    /// object in the QuickJS heap — letting it outlive the runtime would trip
    /// QuickJS's `list_empty(&rt->gc_obj_list)` assertion during
    /// `JS_FreeRuntime`. We also drop our own handle on the pools and the
    /// behavior context the handlers live against, but the order of field
    /// drops in `ScriptRuntime` would still free `quickjs` before the
    /// `handlers` Rc that outside code may still share with `ScriptCtx`.
    fn drop(&mut self) {
        self.handlers.borrow_mut().clear();
    }
}

/// RAII guard that clears the handler table's `current_source` when the
/// currently-loading script exits scope. Ensures a failing script does not
/// leave a stale file name visible to the next script's handlers.
struct SourceGuard<'a> {
    handlers: &'a SharedHandlerTable,
}

impl<'a> Drop for SourceGuard<'a> {
    fn drop(&mut self) {
        self.handlers.borrow_mut().set_current_source(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;

    fn runtime() -> (ScriptRuntime, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let rt = ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        (rt, ctx)
    }

    /// Write `content` to a temp file under the target test directory and
    /// return its path. Using `std::env::temp_dir` rather than an external
    /// crate keeps the test dependency-free.
    fn temp_script(name: &str, content: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        // Nonce by pid + counter to avoid cross-test collisions.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        p.push(format!(
            "postretro_runtime_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn new_constructs_both_subsystems() {
        let (_rt, _ctx) = runtime();
    }

    #[test]
    fn reload_forwards_to_both_subsystems() {
        let (mut rt, _ctx) = runtime();
        rt.reload_definition_context().unwrap();
    }

    #[test]
    fn run_script_file_dispatches_by_extension_luau() {
        let (rt, ctx) = runtime();
        let path = temp_script(
            "dispatch.luau",
            r#"
            spawn_entity({
                position = { x = 0, y = 0, z = 0 },
                rotation = { pitch = 0, yaw = 0, roll = 0 },
                scale    = { x = 1, y = 1, z = 1 },
            })
            "#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        assert!(
            ctx.registry
                .borrow()
                .exists(crate::scripting::registry::EntityId::from_raw(0)),
            "luau path should have spawned via QuickJs-symmetric primitives",
        );
        fs::remove_file(&path).ok();
    }

    /// Acceptance criterion for hot reload (Task 4): re-running the same
    /// behavior script after `clear_level_handlers` must not accumulate
    /// handlers. Three simulated reloads each settle to the cold-load count.
    #[test]
    fn hot_reload_does_not_duplicate_handlers() {
        let (rt, ctx) = runtime();
        let path = temp_script(
            "reload_dedup.js",
            r#"
            registerHandler("levelLoad", function () {});
            "#,
        );

        // Cold load.
        rt.run_script_file(Which::Behavior, &path).unwrap();
        let cold_count = ctx.handlers.borrow().len();
        assert!(
            cold_count > 0,
            "cold load should have registered at least one handler",
        );

        // Three simulated hot reloads. Mirrors main.rs's reload sequence:
        // clear handlers, then re-run all behavior scripts. The handler count
        // must equal `cold_count` after each reload — no accumulation.
        for i in 1..=3 {
            rt.clear_level_handlers();
            rt.run_script_file(Which::Behavior, &path).unwrap();
            assert_eq!(
                ctx.handlers.borrow().len(),
                cold_count,
                "after hot reload #{i}, handler count must equal cold-load count",
            );
        }

        fs::remove_file(&path).ok();
    }

    #[test]
    fn run_script_file_dispatches_by_extension_js() {
        let (rt, ctx) = runtime();
        let path = temp_script(
            "dispatch.js",
            r#"
            spawn_entity({
                position: { x: 0, y: 0, z: 0 },
                rotation: { pitch: 0, yaw: 0, roll: 0 },
                scale:    { x: 1, y: 1, z: 1 },
            });
            "#,
        );
        rt.run_script_file(Which::Behavior, &path).unwrap();
        assert!(
            ctx.registry
                .borrow()
                .exists(crate::scripting::registry::EntityId::from_raw(0)),
            "js path should have spawned through QuickJS",
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn run_script_file_rejects_unknown_extension() {
        let (rt, _ctx) = runtime();
        let path = temp_script("dispatch.py", "print('nope')\n");
        let err = rt.run_script_file(Which::Behavior, &path).unwrap_err();
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains(".py"), "reason: {reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        fs::remove_file(&path).ok();
    }

    #[test]
    fn new_prewarms_pools_with_default_size() {
        let (rt, _ctx) = runtime();
        assert_eq!(
            rt.quickjs_pool().idle_len(),
            crate::scripting::pool::DEFAULT_POOL_SIZE,
        );
        assert_eq!(
            rt.luau_pool().idle_len(),
            crate::scripting::pool::DEFAULT_POOL_SIZE,
        );
        assert_eq!(rt.quickjs_pool().in_flight(), 0);
        assert_eq!(rt.luau_pool().in_flight(), 0);
    }

    #[test]
    fn pooled_quickjs_context_calls_entity_exists() {
        let (rt, _ctx) = runtime();
        let handle = rt.quickjs_pool().acquire().unwrap();
        handle.context().with(|ctx| {
            let v: bool = ctx.eval("entity_exists(0)").unwrap();
            assert!(!v);
        });
    }

    #[test]
    fn pooled_luau_context_calls_entity_exists() {
        let (rt, _ctx) = runtime();
        let handle = rt.luau_pool().acquire().unwrap();
        let v: bool = handle.lua().load("return entity_exists(0)").eval().unwrap();
        assert!(!v);
    }

    // Perf budgets (20 ms / 5 ms) are release-build targets — debug builds
    // will exceed them. Assertions gate on `!cfg!(debug_assertions)` so the
    // tests still run and print timing in debug without failing CI.

    #[test]
    fn shared_behavior_context_primitive_install_under_20ms_release() {
        use std::time::Instant;
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());

        // Build subsystems with pool_size = 0 so we're only timing the
        // shared-context install cost, not the pool pre-warm.
        let cfg = ScriptRuntimeConfig {
            quickjs: crate::scripting::quickjs::QuickJsConfig {
                memory_limit_bytes: 100 * 1024 * 1024,
                pool_size: 0,
            },
            luau: crate::scripting::luau::LuauConfig { pool_size: 0 },
        };

        let start = Instant::now();
        let _rt = ScriptRuntime::new(&registry, &cfg, &ctx).unwrap();
        let elapsed = start.elapsed();

        if !cfg!(debug_assertions) {
            assert!(
                elapsed.as_millis() < 20,
                "shared-context install took {elapsed:?}, budget 20ms",
            );
        } else {
            eprintln!("shared-context install (debug build, not asserting): {elapsed:?}",);
        }
    }

    #[test]
    fn thousand_primitive_calls_under_5ms_release() {
        use std::time::Instant;
        let (rt, _ctx) = runtime();

        let start = Instant::now();
        rt.quickjs().behavior_ctx().with(|ctx| {
            ctx.eval::<(), _>(
                r#"
                for (let i = 0; i < 1000; i++) {
                    entity_exists(i);
                }
                "#,
            )
            .unwrap();
        });
        let elapsed = start.elapsed();

        if !cfg!(debug_assertions) {
            assert!(
                elapsed.as_millis() < 5,
                "1000 primitive calls took {elapsed:?}, budget 5ms",
            );
        } else {
            eprintln!("1000 primitive calls (debug build, not asserting): {elapsed:?}",);
        }
    }
}
