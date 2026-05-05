// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. See: context/lib/scripting.md

use std::fs;
use std::path::Path;

use postretro_level_format::data_script::DataScriptSection;
use rquickjs::{
    CatchResultExt, Context as JsContext, Function as JsFunction, Object as JsObject,
    Value as JsValue,
};

use super::ctx::ScriptCtx;
use super::data_descriptors::LevelManifest;
use super::error::ScriptError;
use super::luau::{LuauConfig, LuauSubsystem, Which as LuauWhich};
use super::primitives_registry::{PrimitiveRegistry, ScriptPrimitive};
use super::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};
#[cfg(debug_assertions)]
use super::typedef;

/// Which scripting scope a given call targets. The subsystem-level `Which`
/// types (QuickJS, Luau) are private to their modules; this is the
/// engine-facing selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Definition,
}

impl From<Which> for LuauWhich {
    fn from(w: Which) -> Self {
        match w {
            Which::Definition => LuauWhich::Definition,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScriptRuntimeConfig {
    pub(crate) quickjs: QuickJsConfig,
    pub(crate) luau: LuauConfig,
}

pub(crate) struct ScriptRuntime {
    quickjs: QuickJsSubsystem,
    luau: LuauSubsystem,
    /// Dev-mode hot-reload watcher. Debug builds only; release builds omit
    /// the field so `drain_reload_requests` is a no-op with no extra code.
    #[cfg(debug_assertions)]
    watcher: Option<super::watcher::ScriptWatcher>,
}

impl ScriptRuntime {
    /// IO failure during SDK type-definition emission is logged and swallowed —
    /// a missing `sdk/types` directory must not prevent startup.
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &ScriptRuntimeConfig,
        _ctx: &ScriptCtx,
    ) -> Result<Self, ScriptError> {
        let quickjs = QuickJsSubsystem::new(registry, &cfg.quickjs)?;
        let luau = LuauSubsystem::new(registry, &cfg.luau)?;

        #[cfg(debug_assertions)]
        typedef::emit_sdk_types_in_debug(registry);

        Ok(Self {
            quickjs,
            luau,
            #[cfg(debug_assertions)]
            watcher: None,
        })
    }


    /// No-op in release builds (the method still exists so the frame-loop
    /// caller doesn't need a `cfg` gate). Calling twice replaces the previous
    /// watcher.
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

    /// Call at the top of each frame. Returns `Ok(true)` when at least one
    /// reload request was drained. No-op in release builds: always returns
    /// `Ok(false)`.
    pub(crate) fn drain_reload_requests(&mut self) -> Result<bool, ScriptError> {
        #[cfg(debug_assertions)]
        {
            if let Some(w) = self.watcher.as_mut() {
                return w.drain_reload_requests();
            }
        }
        Ok(false)
    }

    pub(crate) fn quickjs(&self) -> &QuickJsSubsystem {
        &self.quickjs
    }

    pub(crate) fn luau(&self) -> &LuauSubsystem {
        &self.luau
    }

    /// Evaluate a level's data script in a short-lived VM context and return
    /// the resulting `LevelManifest`. Errors are logged and converted to an
    /// empty manifest — the level loads with empty registries rather than
    /// failing.
    ///
    /// The context is created and dropped within this call.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    pub(crate) fn run_data_script(&self, section: &DataScriptSection) -> LevelManifest {
        // Anything that isn't `.luau` runs through QuickJS, mirroring
        // `run_script_file`'s policy: prl-build emits `.js` from `.ts`, so the
        // on-disk extension is the only signal available at runtime.
        let is_luau = Path::new(&section.source_path)
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("luau"))
            .unwrap_or(false);

        let result = if is_luau {
            run_data_script_luau(
                self.luau.primitives(),
                &section.compiled_bytes,
                &section.source_path,
            )
        } else {
            run_data_script_quickjs(&self.quickjs, &section.compiled_bytes, &section.source_path)
        };

        match result {
            Ok(manifest) => manifest,
            Err(err) => {
                log::warn!(
                    "[Scripting] data script failed for `{}`: {err}",
                    section.source_path,
                );
                LevelManifest::default()
            }
        }
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

        match ext {
            "ts" | "js" => {
                let ctx = match which {
                    Which::Definition => self.quickjs.definition_ctx(),
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

// A short-lived data context is built fresh for each level. It uses the same
// primitive scope as the definition context.

fn run_data_script_quickjs(
    subsys: &QuickJsSubsystem,
    compiled_bytes: &[u8],
    source_path: &str,
) -> Result<LevelManifest, ScriptError> {
    let source = std::str::from_utf8(compiled_bytes).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("data script `{source_path}` is not valid UTF-8: {e}"),
    })?;

    // Fresh context against the existing runtime — shares the GC heap and
    // memory limit with the long-lived contexts. Dropped at the end of this
    // function via RAII when `ctx` goes out of scope.
    let ctx = JsContext::full(subsys.runtime()).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("failed to create data context: {e}"),
    })?;

    let primitives = subsys.primitives();

    let mut manifest_out: Result<LevelManifest, ScriptError> = Err(ScriptError::InvalidArgument {
        reason: "data script did not produce a manifest".to_string(),
    });

    ctx.with(|ctx| {
        for p in primitives {
            if let Err(e) = (p.quickjs_installer)(&ctx) {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("failed to install primitive `{}`: {e}", p.name),
                });
                return;
            }
        }

        if let Err(e) = super::quickjs::evaluate_prelude(&ctx) {
            manifest_out = Err(e);
            return;
        }

        if let Err(e) = run_script::<()>(&ctx, source, source_path) {
            manifest_out = Err(e);
            return;
        }

        let globals = ctx.globals();
        let func: JsFunction = match globals.get("registerLevelManifest") {
            Ok(f) => f,
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "data script `{source_path}` did not export `registerLevelManifest`: {e}"
                    ),
                });
                return;
            }
        };

        // Pass an empty object as the context argument — descriptor-API
        // builders read no fields from it today; the parameter is reserved
        // for forward-compat (see scripting.md §2).
        let arg = match JsObject::new(ctx.clone()) {
            Ok(o) => o,
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("failed to allocate ctx argument: {e}"),
                });
                return;
            }
        };

        let returned: JsValue = match func.call((arg,)).catch(&ctx) {
            Ok(v) => v,
            Err(caught) => {
                let msg = caught.to_string();
                log::error!(
                    target: "script/quickjs",
                    "data script `{source_path}` registerLevelManifest threw: {msg}",
                );
                manifest_out = Err(ScriptError::ScriptThrew {
                    msg,
                    source_name: source_path.to_string(),
                });
                return;
            }
        };

        match LevelManifest::from_js_value(&ctx, returned) {
            Ok(m) => manifest_out = Ok(m),
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: e.to_string(),
                });
            }
        }
    });

    manifest_out
}

fn run_data_script_luau(
    primitives: &[ScriptPrimitive],
    compiled_bytes: &[u8],
    source_path: &str,
) -> Result<LevelManifest, ScriptError> {
    let source = std::str::from_utf8(compiled_bytes).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("data script `{source_path}` is not valid UTF-8: {e}"),
    })?;

    // Fresh `mlua::Lua`, dropped on return. We don't go through
    // `LuauSubsystem::new` because it would also build the archetype sink we
    // don't need here.
    let lua = mlua::Lua::new();

    for p in primitives {
        (p.luau_installer)(&lua).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to install primitive `{}`: {e}", p.name),
        })?;
    }

    super::luau::evaluate_prelude(&lua)?;

    // Mirror `LuauSubsystem::run_source`'s compile+load shape so traceback
    // formatting stays consistent.
    let bytecode = mlua::Compiler::new()
        .compile(source)
        .map_err(|e| ScriptError::ScriptThrew {
            msg: e.to_string(),
            source_name: source_path.to_string(),
        })?;
    lua.load(&bytecode)
        .set_name(source_path)
        .set_mode(mlua::ChunkMode::Binary)
        .exec()
        .map_err(|e| ScriptError::ScriptThrew {
            msg: e.to_string(),
            source_name: source_path.to_string(),
        })?;

    let func: mlua::Function =
        lua.globals()
            .get("registerLevelManifest")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!(
                    "data script `{source_path}` did not export `registerLevelManifest`: {e}"
                ),
            })?;

    let arg = lua
        .create_table()
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("failed to allocate ctx argument: {e}"),
        })?;

    let returned: mlua::Value = func.call(arg).map_err(|e| ScriptError::ScriptThrew {
        msg: e.to_string(),
        source_name: source_path.to_string(),
    })?;

    LevelManifest::from_lua_value(returned).map_err(|e| ScriptError::InvalidArgument {
        reason: e.to_string(),
    })
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
    fn run_script_file_rejects_unknown_extension() {
        let (rt, _ctx) = runtime();
        let path = temp_script("dispatch.py", "print('nope')\n");
        let err = rt.run_script_file(Which::Definition, &path).unwrap_err();
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains(".py"), "reason: {reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        fs::remove_file(&path).ok();
    }

    // Perf budgets (20 ms / 5 ms) are release-build targets — debug builds
    // will exceed them. Assertions gate on `!cfg!(debug_assertions)` so the
    // tests still run and print timing in debug without failing CI.

    #[test]
    fn shared_definition_context_primitive_install_under_20ms_release() {
        use std::time::Instant;
        let ctx = ScriptCtx::new();
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());

        let cfg = ScriptRuntimeConfig {
            quickjs: crate::scripting::quickjs::QuickJsConfig {
                memory_limit_bytes: 100 * 1024 * 1024,
            },
            luau: crate::scripting::luau::LuauConfig::default(),
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

    fn data_section(source_path: &str, body: &str) -> DataScriptSection {
        DataScriptSection {
            compiled_bytes: body.as_bytes().to_vec(),
            source_path: source_path.to_string(),
        }
    }

    #[test]
    fn run_data_script_quickjs_populates_manifest() {
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/data.js",
            r#"
            globalThis.registerLevelManifest = function(ctx) {
                return {
                    reactions: [
                        { name: "wave1Complete", primitive: "moveGeometry", tag: "reactor" },
                    ],
                };
            };
            "#,
        );
        let manifest = rt.run_data_script(&section);
        assert_eq!(manifest.reactions.len(), 1);
        assert_eq!(manifest.reactions[0].name, "wave1Complete");
    }

    #[test]
    fn run_data_script_luau_populates_manifest() {
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/data.luau",
            r#"
            function registerLevelManifest(ctx)
                return {
                    reactions = {
                        { name = "wave1Complete", primitive = "moveGeometry", tag = "reactor" },
                    },
                }
            end
            "#,
        );
        let manifest = rt.run_data_script(&section);
        assert_eq!(manifest.reactions.len(), 1);
    }

    #[test]
    fn run_data_script_missing_export_returns_empty_manifest() {
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/no_export.js",
            "// script with no registerLevelManifest export\nlet x = 1;",
        );
        let manifest = rt.run_data_script(&section);
        assert!(manifest.reactions.is_empty());
    }

    #[test]
    fn run_data_script_invalid_utf8_returns_empty_manifest() {
        let (rt, _ctx) = runtime();
        let section = DataScriptSection {
            compiled_bytes: vec![0xFFu8, 0xFE, 0xFD],
            source_path: "/maps/binary.js".to_string(),
        };
        let manifest = rt.run_data_script(&section);
        assert!(manifest.reactions.is_empty());
    }

    #[test]
    fn thousand_primitive_calls_under_5ms_release() {
        use std::time::Instant;
        let (rt, _ctx) = runtime();

        let start = Instant::now();
        rt.quickjs().definition_ctx().with(|ctx| {
            ctx.eval::<(), _>(
                r#"
                for (let i = 0; i < 1000; i++) {
                    entityExists(i);
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
