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

/// Validated `setupMod()` return value. Today the engine reads only `name`;
/// additional fields will appear here as the mod system grows. Construct via
/// [`ScriptRuntime::run_mod_init`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModManifestResult {
    pub(crate) name: String,
}

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

    /// Run the mod entry point at `mod_root`.
    ///
    /// Looks for `start-script.js` (TypeScript-compiled) or `start-script.luau`
    /// at the mod root. In debug builds, a missing/stale `start-script.js`
    /// is regenerated from `start-script.ts` if present (skipped in release).
    /// Both engines run in a short-lived VM context that is created and
    /// dropped within this call.
    ///
    /// Errors:
    /// - both `start-script.js` and `start-script.luau` exist
    /// - in release builds, no `start-script.{js,luau}` exists
    /// - `setupMod` is not exported by the script
    /// - `setupMod()` throws or returns a non-object value
    /// - the returned object is missing the required `name` field
    ///
    /// Returns `Ok(None)` only in debug builds when no start-script is found.
    ///
    /// See: context/lib/scripting.md §2 (Mod-init context lifecycle)
    pub(crate) fn run_mod_init(
        &self,
        mod_root: &Path,
    ) -> Result<Option<ModManifestResult>, ScriptError> {
        let js_path = mod_root.join("start-script.js");
        let ts_path = mod_root.join("start-script.ts");
        let luau_path = mod_root.join("start-script.luau");

        // In debug, ensure `start-script.js` is up-to-date with `start-script.ts`.
        // This mirrors the freshness check used by the level-load TS path.
        #[cfg(debug_assertions)]
        {
            if ts_path.is_file() {
                if let Err(e) = compile_start_script_if_stale(&ts_path, &js_path) {
                    return Err(ScriptError::InvalidArgument {
                        reason: format!("mod-init: failed to compile `{}`: {e}", ts_path.display()),
                    });
                }
            }
        }
        // `ts_path` is only consulted in debug builds; suppress the unused
        // binding warning otherwise.
        #[cfg(not(debug_assertions))]
        let _ = ts_path;

        let has_js = js_path.is_file();
        let has_luau = luau_path.is_file();

        if has_js && has_luau {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: both `start-script.js` and `start-script.luau` exist at `{}`; \
                     pick one (the TS->JS compile path is preferred)",
                    mod_root.display(),
                ),
            });
        }

        if !has_js && !has_luau {
            #[cfg(debug_assertions)]
            {
                log::info!(
                    "[Mod-init] no start-script at `{}` — skipping (debug)",
                    mod_root.display(),
                );
                return Ok(None);
            }
            #[cfg(not(debug_assertions))]
            {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: no `start-script.{{js,luau}}` found at `{}`; \
                         release builds require a pre-compiled start-script",
                        mod_root.display(),
                    ),
                });
            }
        }

        let manifest = if has_js {
            let source =
                fs::read_to_string(&js_path).map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("mod-init: failed to read `{}`: {e}", js_path.display()),
                })?;
            run_mod_init_quickjs(&self.quickjs, &source, &js_path.to_string_lossy())?
        } else {
            let source =
                fs::read_to_string(&luau_path).map_err(|e| ScriptError::InvalidArgument {
                    reason: format!("mod-init: failed to read `{}`: {e}", luau_path.display()),
                })?;
            run_mod_init_luau(
                self.luau.primitives(),
                &source,
                &luau_path.to_string_lossy(),
                mod_root,
            )?
        };

        log::info!("[Mod-init] mod `{}` initialized", manifest.name);
        Ok(Some(manifest))
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

// ---------------------------------------------------------------------------
// Mod-init helpers.

/// In debug builds: compile `ts_path` to `js_path` if `js_path` is missing or
/// older than `ts_path`. Reuses the `scripts-build` sidecar detection cascade
/// from the watcher.
#[cfg(debug_assertions)]
fn compile_start_script_if_stale(ts_path: &Path, js_path: &Path) -> Result<(), String> {
    let ts_mtime = fs::metadata(ts_path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("stat `{}`: {e}", ts_path.display()))?;
    let needs_build = match fs::metadata(js_path).and_then(|m| m.modified()) {
        Ok(js_mtime) => js_mtime < ts_mtime,
        Err(_) => true,
    };
    if !needs_build {
        return Ok(());
    }
    let compiler = super::watcher::TsCompilerPath::detect().ok_or_else(|| {
        "scripts-build not found — install it on PATH or ship it next to the engine binary"
            .to_string()
    })?;
    super::watcher::run_ts_compiler(&compiler, ts_path, js_path)
}

fn run_mod_init_quickjs(
    subsys: &QuickJsSubsystem,
    source: &str,
    source_path: &str,
) -> Result<ModManifestResult, ScriptError> {
    let ctx = JsContext::full(subsys.runtime()).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: failed to create context: {e}"),
    })?;

    let primitives = subsys.primitives();
    let mut out: Result<ModManifestResult, ScriptError> = Err(ScriptError::InvalidArgument {
        reason: "mod-init: setupMod did not produce a manifest".to_string(),
    });

    ctx.with(|ctx| {
        for p in primitives {
            if let Err(e) = (p.quickjs_installer)(&ctx) {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: failed to install primitive `{}`: {e}", p.name),
                });
                return;
            }
        }

        if let Err(e) = super::quickjs::evaluate_prelude(&ctx) {
            out = Err(e);
            return;
        }

        if let Err(e) = run_script::<()>(&ctx, source, source_path) {
            out = Err(e);
            return;
        }

        let globals = ctx.globals();
        let func: JsFunction = match globals.get("setupMod") {
            Ok(f) => f,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` did not export `setupMod`: {e}"),
                });
                return;
            }
        };

        let returned: JsValue = match func.call(()).catch(&ctx) {
            Ok(v) => v,
            Err(caught) => {
                let msg = caught.to_string();
                log::error!(
                    target: "script/quickjs",
                    "mod-init: `{source_path}` setupMod threw: {msg}",
                );
                out = Err(ScriptError::ScriptThrew {
                    msg,
                    source_name: source_path.to_string(),
                });
                return;
            }
        };

        let obj = match JsObject::from_value(returned) {
            Ok(o) => o,
            Err(_) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!("mod-init: `{source_path}` setupMod must return an object"),
                });
                return;
            }
        };

        let name: String = match obj.get("name") {
            Ok(s) => s,
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` setupMod return value missing `name`: {e}"
                    ),
                });
                return;
            }
        };

        out = Ok(ModManifestResult { name });
    });

    out
}

fn run_mod_init_luau(
    primitives: &[ScriptPrimitive],
    source: &str,
    source_path: &str,
    mod_root: &Path,
) -> Result<ModManifestResult, ScriptError> {
    // The mod-init Luau VM gets a working `require` resolver rooted at the
    // mod root so start-script can pull in domain scripts.
    let lua = super::luau::build_lua_state(primitives, None, Some(mod_root))?;

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
            .get("setupMod")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("mod-init: `{source_path}` did not export `setupMod`: {e}"),
            })?;

    let returned: mlua::Value = func.call(()).map_err(|e| ScriptError::ScriptThrew {
        msg: e.to_string(),
        source_name: source_path.to_string(),
    })?;

    let table = match returned {
        mlua::Value::Table(t) => t,
        other => {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` setupMod must return a table, got {}",
                    other.type_name()
                ),
            });
        }
    };

    let name: String = table
        .get("name")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` setupMod return value missing `name`: {e}"),
        })?;

    Ok(ModManifestResult { name })
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

    // --- mod-init tests ----------------------------------------------------

    fn temp_mod_root(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_mod_init_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    #[cfg(debug_assertions)]
    fn mod_init_missing_start_script_debug_returns_none() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("missing");
        let got = rt.run_mod_init(&dir).unwrap();
        assert!(got.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_quickjs_registers_entity_type() {
        let (rt, ctx) = runtime();
        let dir = temp_mod_root("js_register");
        // start-script.js: registers a player type, then exports `setupMod`.
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            registerEntity({ classname: "info_player_start" });
            globalThis.setupMod = function() { return { name: "TestMod" }; };
            "#,
        )
        .unwrap();

        let manifest = rt.run_mod_init(&dir).unwrap().expect("Some manifest");
        assert_eq!(manifest.name, "TestMod");
        let dr = ctx.data_registry.borrow();
        assert!(
            dr.entities
                .iter()
                .any(|e| e.classname == "info_player_start"),
            "registerEntity from start-script must populate the data registry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_luau_registers_entity_type() {
        let (rt, ctx) = runtime();
        let dir = temp_mod_root("luau_register");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            registerEntity({ classname = "info_player_start" })
            function setupMod()
                return { name = "TestMod" }
            end
            "#,
        )
        .unwrap();

        let manifest = rt.run_mod_init(&dir).unwrap().expect("Some manifest");
        assert_eq!(manifest.name, "TestMod");
        let dr = ctx.data_registry.borrow();
        assert!(
            dr.entities
                .iter()
                .any(|e| e.classname == "info_player_start"),
            "registerEntity from start-script.luau must populate the data registry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_missing_setup_mod_errors() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("no_setup");
        std::fs::write(dir.join("start-script.js"), "var x = 1;\n").unwrap();
        let err = rt.run_mod_init(&dir).expect_err("missing setupMod");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("setupMod"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_setup_mod_missing_name_errors() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("no_name");
        std::fs::write(
            dir.join("start-script.js"),
            "globalThis.setupMod = function() { return {}; };\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("missing name");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("name"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_setup_mod_throws_errors() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("throws");
        std::fs::write(
            dir.join("start-script.js"),
            "globalThis.setupMod = function() { throw new Error('boom'); };\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("setupMod throws");
        match err {
            ScriptError::ScriptThrew { msg, .. } => {
                assert!(msg.contains("boom"), "{msg}");
            }
            other => panic!("expected ScriptThrew, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_setup_mod_non_object_return_errors() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("non_obj");
        std::fs::write(
            dir.join("start-script.js"),
            "globalThis.setupMod = function() { return 42; };\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("non-object return");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(
                    reason.contains("object") || reason.contains("name"),
                    "{reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_both_js_and_lua_errors() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("both");
        std::fs::write(
            dir.join("start-script.js"),
            "globalThis.setupMod = function() { return { name: 'A' }; };\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("start-script.luau"),
            "function setupMod() return { name = 'A' } end\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("both present");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("both"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_luau_require_resolves_from_mod_root() {
        let (rt, ctx) = runtime();
        let dir = temp_mod_root("luau_require");
        // Sub-module returns a descriptor; start-script imports it and registers.
        std::fs::write(
            dir.join("sub.luau"),
            r#"
            return { descriptor = { classname = "info_player_start" } }
            "#,
        )
        .unwrap();
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            local m = require("./sub")
            registerEntity(m.descriptor)
            function setupMod()
                return { name = "Imported" }
            end
            "#,
        )
        .unwrap();

        let manifest = rt.run_mod_init(&dir).unwrap().expect("Some manifest");
        assert_eq!(manifest.name, "Imported");
        let dr = ctx.data_registry.borrow();
        assert!(
            dr.entities
                .iter()
                .any(|e| e.classname == "info_player_start"),
            "domain script imported via require must register its entity type"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mod_init_luau_require_rejects_parent_dir_traversal() {
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_require_traversal");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            local ok, err = pcall(require, "../escape")
            if ok then error("expected require to reject ../") end
            function setupMod()
                return { name = "GuardedMod" }
            end
            "#,
        )
        .unwrap();
        let manifest = rt.run_mod_init(&dir).unwrap().expect("Some manifest");
        assert_eq!(manifest.name, "GuardedMod");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
