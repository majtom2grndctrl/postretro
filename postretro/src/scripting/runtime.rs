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
}

impl ScriptRuntime {
    /// Construct both subsystems against the same primitive registry. Also
    /// emits the SDK type-definition files in debug builds (logs and
    /// continues on IO failure — missing `sdk/types` directory must not
    /// prevent engine startup).
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &ScriptRuntimeConfig,
    ) -> Result<Self, ScriptError> {
        let quickjs = QuickJsSubsystem::new(registry, &cfg.quickjs)?;
        let luau = LuauSubsystem::new(registry, &cfg.luau)?;

        #[cfg(debug_assertions)]
        typedef::emit_sdk_types_in_debug(registry);

        Ok(Self { quickjs, luau })
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
    pub(crate) fn run_script_file(
        &self,
        which: Which,
        path: &Path,
    ) -> Result<(), ScriptError> {
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
}
