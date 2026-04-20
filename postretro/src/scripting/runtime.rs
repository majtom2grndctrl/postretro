// Top-level scripting runtime: owns both `QuickJsSubsystem` and
// `LuauSubsystem` and dispatches by file extension. Per the sub-plan 4
// "unification" decision: one construction path, one reload path, one place
// to fan script-file inputs out to.
//
// This type is deliberately shallow. It owns two subsystem handles, forwards
// lifecycle calls to both, and routes by extension. No abstraction over
// "script engine" — that would be gold-plating given there are exactly two
// and they aren't pluggable.
//
// See: context/plans/in-progress/scripting-foundation/plan-1-runtime-foundation.md §Sub-plan 4

use std::fs;
use std::path::Path;

use super::error::ScriptError;
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
    /// shared behavior context on `quickjs` is *not* part of this pool (see
    /// `scripting::pool` module docs).
    quickjs_pool: QuickJsContextPool,
    /// Ephemeral-context pool for future per-entity Luau scripting. Mirrors
    /// `quickjs_pool`.
    luau_pool: LuauContextPool,
    /// Dev-mode hot-reload watcher. Present only in debug builds; release
    /// builds omit the field entirely so the watcher module doesn't compile
    /// in and `drain_reload_requests` becomes a cheap no-op.
    #[cfg(debug_assertions)]
    watcher: Option<super::watcher::ScriptWatcher>,
}

impl ScriptRuntime {
    /// Construct both subsystems against the same primitive registry, pre-
    /// warm per-language context pools at the sizes named in `cfg`, and
    /// emit the SDK type-definition files in debug builds (logs and
    /// continues on IO failure — missing `sdk/types` directory must not
    /// prevent engine startup).
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &ScriptRuntimeConfig,
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
            #[cfg(debug_assertions)]
            watcher: None,
        })
    }

    /// Access the QuickJS ephemeral-context pool. Test-facing; the real
    /// consumer is a later per-entity scripting plan.
    pub(crate) fn quickjs_pool(&self) -> &QuickJsContextPool {
        &self.quickjs_pool
    }

    /// Access the Luau ephemeral-context pool. Test-facing; the real
    /// consumer is a later per-entity scripting plan.
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

    /// Drain any pending reload requests produced by the watcher and apply
    /// them. Call at the top of each frame. No-op in release builds.
    ///
    /// Errors during a reload are logged (inside the watcher) and swallowed —
    /// one failed hot reload must not kill the engine. The prior archetype
    /// set stays active.
    pub(crate) fn drain_reload_requests(&mut self) -> Result<(), ScriptError> {
        #[cfg(debug_assertions)]
        {
            // Temporarily take the watcher to satisfy the borrow checker:
            // `drain_reload_requests` needs `&mut self` on both the watcher
            // and the runtime, and they can't both be borrowed from `self`
            // at once. We put it back unconditionally via `Option::replace`
            // equivalent; panics inside `drain_reload_requests` propagate but
            // don't leak the watcher because the field is simply re-set here.
            if let Some(mut w) = self.watcher.take() {
                let result = w.drain_reload_requests(self);
                self.watcher = Some(w);
                return result;
            }
        }
        Ok(())
    }

    /// Access the QuickJS subsystem. Lifecycle-heavy operations (entering a
    /// context, draining archetypes) still live on the subsystem; this just
    /// exposes the handle.
    pub(crate) fn quickjs(&self) -> &QuickJsSubsystem {
        &self.quickjs
    }

    /// Access the Luau subsystem.
    pub(crate) fn luau(&self) -> &LuauSubsystem {
        &self.luau
    }

    /// Reload both definition contexts. Called from the dev-mode hot-reload
    /// path in a later sub-plan.
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
    /// `.ts` is accepted here as a convenience for the later sub-plan that
    /// feeds TS straight through the transpile step. For now QuickJS will
    /// parse the file as JS; the upstream layer is responsible for stripping
    /// types before handing a `.ts` file to the runtime. Unknown extensions
    /// return `ScriptError::InvalidArgument`.
    pub(crate) fn run_script_file(&self, which: Which, path: &Path) -> Result<(), ScriptError> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let source = fs::read_to_string(path).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to read script `{}`: {e}", path.display()),
        })?;
        let name = path.to_string_lossy().into_owned();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::primitives::register_all;

    fn runtime() -> (ScriptRuntime, ScriptCtx) {
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        let rt = ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default()).unwrap();
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

    // Perf criteria from sub-plan 6. These are measurable numbers, but the
    // plan's 20 ms / 5 ms budgets are release-build targets — debug builds
    // will blow past them. We gate the assertion on `!cfg!(debug_assertions)`
    // so the test still runs (and prints timing) in debug without failing
    // CI on sanity-check `cargo test` runs.

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
        let _rt = ScriptRuntime::new(&registry, &cfg).unwrap();
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
