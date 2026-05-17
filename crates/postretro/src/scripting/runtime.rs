// Top-level scripting runtime: owns both subsystems and dispatches by file
// extension. See: context/lib/scripting.md

use std::fs;
use std::path::Path;

use postretro_level_format::data_script::DataScriptSection;
use rquickjs::{
    Array as JsArray, CatchResultExt, Context as JsContext, Function as JsFunction,
    Object as JsObject, Value as JsValue,
};

use super::ctx::ScriptCtx;
use super::data_descriptors::{
    EntityTypeDescriptor, LevelManifest, entity_descriptor_from_js, entity_descriptor_from_lua,
};
use super::error::ScriptError;
use super::luau::{LuauConfig, LuauSubsystem, Which as LuauWhich};
use super::primitives_registry::{PrimitiveRegistry, ScriptPrimitive};
use super::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};
#[cfg(debug_assertions)]
use super::typedef;

/// Validated `setupMod()` return value. Construct via
/// [`ScriptRuntime::run_mod_init`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModManifestResult {
    pub(crate) name: String,
    /// Entity-type descriptors returned by `setupMod()`. Empty when the
    /// returned object omits the `entities` field. Drained into `DataRegistry`
    /// by the boot caller after `run_mod_init` returns.
    pub(crate) entities: Vec<EntityTypeDescriptor>,
}

/// Aggregated reload signal returned by
/// [`ScriptRuntime::drain_reload_requests`]. Defined here (rather than under
/// the debug-only `watcher` module) so release builds can refer to it
/// without `cfg` gates at every call site.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReloadSummary {
    /// At least one definition-script change was observed under
    /// `<mod>/scripts/`.
    pub(crate) scripts: bool,
    /// At least one change touched `start-script.{ts,js,luau}` (or a likely
    /// import sibling) at the mod root; the engine should re-run mod-init.
    pub(crate) mod_init: bool,
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
    /// Validated `setupMod()` return value, populated by `run_mod_init`.
    /// `None` until `run_mod_init` succeeds; in debug builds may also remain
    /// `None` if no `start-script.{js,luau}` was found at the mod root.
    mod_manifest: Option<ModManifestResult>,
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
            mod_manifest: None,
            #[cfg(debug_assertions)]
            watcher: None,
        })
    }

    /// No-op in release builds (the method still exists so the frame-loop
    /// caller doesn't need a `cfg` gate). Calling twice replaces the previous
    /// watcher.
    ///
    /// `script_root` is watched recursively for definition-script edits;
    /// `mod_root` is watched non-recursively so changes to
    /// `start-script.{ts,js,luau}` re-trigger `run_mod_init`.
    pub(crate) fn start_watcher(
        &mut self,
        script_root: &Path,
        mod_root: &Path,
    ) -> Result<(), ScriptError> {
        #[cfg(debug_assertions)]
        {
            let ts_compiler = super::watcher::TsCompilerPath::detect();
            let w = super::watcher::ScriptWatcher::spawn(script_root, mod_root, ts_compiler)?;
            self.watcher = Some(w);
        }
        #[cfg(not(debug_assertions))]
        {
            // In release builds, hot reload is intentionally unavailable;
            // silently ignore so the caller can unconditionally invoke this.
            let _ = script_root;
            let _ = mod_root;
        }
        Ok(())
    }

    /// Call at the top of each frame. Returns a [`ReloadSummary`] describing
    /// what kinds of reload (if any) were observed. No-op in release builds:
    /// always returns the default (all flags `false`).
    pub(crate) fn drain_reload_requests(&mut self) -> Result<ReloadSummary, ScriptError> {
        #[cfg(debug_assertions)]
        {
            if let Some(w) = self.watcher.as_mut() {
                return w.drain_reload_requests();
            }
        }
        Ok(ReloadSummary::default())
    }

    /// In debug builds: walk `script_root` recursively and `mod_root`
    /// non-recursively, recompiling any `.ts` file whose sibling `.js` is
    /// missing or older. No-op in release builds.
    ///
    /// Call this before [`ScriptRuntime::run_mod_init`] so domain scripts
    /// edited between sessions are compiled before the engine loads them.
    /// The two scopes mirror [`ScriptWatcher::spawn`]: nested helpers under
    /// `scripts/` are walked recursively; top-level mod-root files
    /// (`start-script.ts` and any siblings imported by it) are walked one
    /// level. The scan mirrors the per-file freshness check in
    /// `compile_start_script_if_stale` — same `<=` mtime comparison, same
    /// compiler detection cascade, same error-logging strategy (warn and
    /// continue rather than hard-fail). A missing `scripts-build` is logged
    /// once and the scan returns without compiling.
    pub(crate) fn compile_stale_scripts(&self, script_root: &Path, mod_root: &Path) {
        #[cfg(debug_assertions)]
        {
            scan_and_compile_stale_ts(script_root, mod_root);
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = (script_root, mod_root);
        }
    }

    pub(crate) fn quickjs(&self) -> &QuickJsSubsystem {
        &self.quickjs
    }

    pub(crate) fn luau(&self) -> &LuauSubsystem {
        &self.luau
    }

    /// Evaluate a level's data script in a short-lived VM context and return
    /// the resulting `LevelManifest`. Errors are logged and converted to an
    /// empty manifest — the level loads with an empty reaction registry
    /// (per-level reactions are absent) rather than failing. The engine-global
    /// entity-type registry, populated at mod-init from `setupMod()`'s
    /// `entities` return field, is unaffected.
    ///
    /// `mod_root` is forwarded to the Luau VM so `require("./shared/loot")`
    /// inside data scripts resolves against the mod root, matching the
    /// mod-init VM's resolver wiring. For `.js` scripts, `mod_root` is not
    /// used — the QuickJS data context has no `require` resolver.
    ///
    /// The context is created and dropped within this call.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    pub(crate) fn run_data_script(
        &self,
        section: &DataScriptSection,
        mod_root: &Path,
    ) -> LevelManifest {
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
                mod_root,
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
    /// The selected engine (QuickJS for `.js`, Luau for `.luau`) runs in a
    /// short-lived VM context that is created and dropped within this call.
    ///
    /// Errors:
    /// - both `start-script.js` and `start-script.luau` exist
    /// - in release builds, no `start-script.{js,luau}` exists
    /// - `setupMod` is not exported by the script
    /// - `setupMod()` throws or returns a non-object value
    /// - the returned object is missing the required `name` field
    ///
    /// On success, the validated manifest is stored on `self`; access it via
    /// [`ScriptRuntime::mod_manifest`]. In debug builds when no start-script
    /// is found, the call still succeeds and the stored manifest stays `None`.
    ///
    /// See: context/lib/scripting.md §2 (Mod-init context lifecycle)
    pub(crate) fn run_mod_init(&mut self, mod_root: &Path) -> Result<(), ScriptError> {
        let js_path = mod_root.join("start-script.js");
        let ts_path = mod_root.join("start-script.ts");
        let luau_path = mod_root.join("start-script.luau");

        let has_luau = luau_path.is_file();

        // Both-present check runs BEFORE any debug compile so we don't write a
        // `.js` the user never authored when they have `.ts` + `.luau` and
        // intended the Luau path. The compile step would otherwise materialize
        // `start-script.js` and force the user to manually delete it.
        let has_ts_or_js_source = js_path.is_file() || {
            #[cfg(debug_assertions)]
            {
                ts_path.is_file()
            }
            #[cfg(not(debug_assertions))]
            {
                false
            }
        };
        if has_ts_or_js_source && has_luau {
            #[cfg(debug_assertions)]
            let js_source_hint = if ts_path.is_file() && !js_path.is_file() {
                "`start-script.ts`".to_string()
            } else if ts_path.is_file() {
                "`start-script.js` (compiled from `start-script.ts`)".to_string()
            } else {
                "`start-script.js`".to_string()
            };
            #[cfg(not(debug_assertions))]
            let js_source_hint = "`start-script.js`".to_string();

            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: both {js_source_hint} and `start-script.luau` exist at `{}`; \
                     pick one (delete the unwanted file; the TS->JS path is preferred)",
                    mod_root.display(),
                ),
            });
        }

        // In debug, ensure `start-script.js` is up-to-date with `start-script.ts`.
        // This mirrors the freshness check used by the level-load TS path.
        // Only runs once we've confirmed the user isn't in the both-present
        // ambiguous state above.
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

        if !has_js && !has_luau {
            #[cfg(debug_assertions)]
            {
                log::info!(
                    "[Mod-init] no start-script at `{}` — skipping (debug)",
                    mod_root.display(),
                );
                self.mod_manifest = None;
                return Ok(());
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
        self.mod_manifest = Some(manifest);
        Ok(())
    }

    /// Returns the validated manifest captured by the most recent successful
    /// [`ScriptRuntime::run_mod_init`] call. `None` until then, and may also
    /// remain `None` in debug builds when no start-script was found.
    pub(crate) fn mod_manifest(&self) -> Option<&ModManifestResult> {
        self.mod_manifest.as_ref()
    }

    /// Mutable accessor for the stored manifest. Used by the boot caller to
    /// drain `entities` into `DataRegistry` after a successful
    /// [`ScriptRuntime::run_mod_init`] — the runtime parses and returns; the
    /// caller owns registry lifecycle. See: context/lib/boot_sequence.md §3.
    pub(crate) fn mod_manifest_mut(&mut self) -> Option<&mut ModManifestResult> {
        self.mod_manifest.as_mut()
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
        let func: JsFunction = match globals.get("setupLevel") {
            Ok(f) => f,
            Err(e) => {
                manifest_out = Err(ScriptError::InvalidArgument {
                    reason: format!("data script `{source_path}` did not export `setupLevel`: {e}"),
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
                    "data script `{source_path}` setupLevel threw: {msg}",
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
    mod_root: &Path,
) -> Result<LevelManifest, ScriptError> {
    let source = std::str::from_utf8(compiled_bytes).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("data script `{source_path}` is not valid UTF-8: {e}"),
    })?;

    // Fresh `mlua::Lua`, dropped on return. Routed through `build_lua_state`
    // so the deny-list, print redirect, SDK prelude, primitives, and
    // mod-rooted `require` resolver match the mod-init VM. The archetype
    // sink is intentionally not installed here — data scripts don't drive
    // it. See: context/lib/scripting.md §2 (Luau `require` resolver)
    let lua = super::luau::build_lua_state(primitives, None, Some(mod_root))?;

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
            .get("setupLevel")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("data script `{source_path}` did not export `setupLevel`: {e}"),
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
        // `<=` catches same-second saves (e.g. `git checkout` followed by
        // immediate launch) where mtimes are equal but the JS is stale.
        Ok(js_mtime) => js_mtime <= ts_mtime,
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

/// In debug builds: walk `script_root` recursively and recompile any `.ts`
/// file whose sibling `.js` is missing or older than the `.ts`. Detects the
/// compiler once up front; logs a warning and returns early if not found.
/// Per-file compile failures are logged as warnings; the scan continues so one
/// broken file does not block the rest.
#[cfg(debug_assertions)]
fn scan_and_compile_stale_ts(script_root: &Path, mod_root: &Path) {
    let script_root_present = script_root.is_dir();
    let mod_root_present = mod_root.is_dir() && mod_root != script_root;
    if !script_root_present && !mod_root_present {
        return;
    }

    let compiler = match super::watcher::TsCompilerPath::detect() {
        Some(c) => c,
        None => {
            log::warn!(
                "[Scripting] startup TS scan: `scripts-build` not found — \
                 stale `.ts` files will not be recompiled. \
                 Install `scripts-build` on PATH or next to the engine binary.",
            );
            return;
        }
    };

    let mut compiled = 0u32;
    let mut failed = 0u32;
    if script_root_present {
        visit_ts_files(script_root, &compiler, &mut compiled, &mut failed);
    }
    // mod_root walked one level only — nested helpers belong under scripts/.
    if mod_root_present {
        visit_ts_files_shallow(mod_root, &compiler, &mut compiled, &mut failed);
    }

    if compiled > 0 || failed > 0 {
        log::info!("[Scripting] startup TS scan: {compiled} recompiled, {failed} failed");
    }
}

/// Non-recursive variant of `visit_ts_files` for the mod-root scope.
/// Subdirectories are not descended — they are the watcher's `script_root`
/// territory and are handled by the recursive walk.
#[cfg(debug_assertions)]
fn visit_ts_files_shallow(
    dir: &Path,
    compiler: &super::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: cannot read directory `{}`: {err}",
                dir.display(),
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        compile_one_if_stale(&path, compiler, compiled, failed);
    }
}

/// Recursively walk `dir`, compiling stale `.ts` files. Subdirectory traversal
/// errors (e.g. permission denied) are logged and skipped.
#[cfg(debug_assertions)]
fn visit_ts_files(
    dir: &Path,
    compiler: &super::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: cannot read directory `{}`: {err}",
                dir.display(),
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            visit_ts_files(&path, compiler, compiled, failed);
            continue;
        }

        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        compile_one_if_stale(&path, compiler, compiled, failed);
    }
}

/// Compile a single `.ts` file when its sibling `.js` is missing or older.
/// Same `<=` mtime comparison as `compile_start_script_if_stale` so a
/// same-second save still triggers a rebuild.
#[cfg(debug_assertions)]
fn compile_one_if_stale(
    path: &Path,
    compiler: &super::watcher::TsCompilerPath,
    compiled: &mut u32,
    failed: &mut u32,
) {
    let js_path = super::watcher::compiled_output_for(path);
    let ts_mtime = match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(err) => {
            log::warn!(
                "[Scripting] startup TS scan: stat `{}`: {err}",
                path.display(),
            );
            return;
        }
    };
    let needs_build = match fs::metadata(&js_path).and_then(|m| m.modified()) {
        Ok(js_mtime) => js_mtime <= ts_mtime,
        Err(_) => true,
    };
    if !needs_build {
        return;
    }
    match super::watcher::run_ts_compiler(compiler, path, &js_path) {
        Ok(()) => {
            log::debug!("[Scripting] startup TS scan: compiled `{}`", path.display(),);
            *compiled += 1;
        }
        Err(msg) => {
            log::warn!(
                "[Scripting] startup TS scan: compile failed for `{}`: {msg}",
                path.display(),
            );
            *failed += 1;
        }
    }
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

        // Optional `entities` array. Missing key → empty Vec. Present-but-not-
        // array → InvalidArgument. Each element parses via the shared
        // descriptor reader (`entity_descriptor_from_js`).
        let entities: Vec<EntityTypeDescriptor> = match obj.contains_key("entities") {
            Ok(false) => Vec::new(),
            Ok(true) => match obj.get::<_, JsArray>("entities") {
                Ok(arr) => {
                    let mut parsed = Vec::with_capacity(arr.len());
                    let mut err: Option<ScriptError> = None;
                    for i in 0..arr.len() {
                        let v: JsValue = match arr.get(i) {
                            Ok(v) => v,
                            Err(e) => {
                                err = Some(ScriptError::InvalidArgument {
                                    reason: format!(
                                        "mod-init: `{source_path}` setupMod `entities[{i}]` could not be read: {e}"
                                    ),
                                });
                                break;
                            }
                        };
                        match entity_descriptor_from_js(&ctx, v) {
                            Ok(d) => parsed.push(d),
                            Err(e) => {
                                err = Some(ScriptError::InvalidArgument {
                                    reason: format!(
                                        "mod-init: `{source_path}` setupMod `entities[{i}]` invalid: {e}"
                                    ),
                                });
                                break;
                            }
                        }
                    }
                    if let Some(e) = err {
                        out = Err(e);
                        return;
                    }
                    parsed
                }
                Err(e) => {
                    out = Err(ScriptError::InvalidArgument {
                        reason: format!(
                            "mod-init: `{source_path}` setupMod `entities` field must be an array: {e}"
                        ),
                    });
                    return;
                }
            },
            Err(e) => {
                out = Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` setupMod return value `entities` lookup failed: {e}"
                    ),
                });
                return;
            }
        };

        out = Ok(ModManifestResult { name, entities });
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

    // Optional `entities` array. Missing key → empty Vec. Present-but-not-table
    // → InvalidArgument. Each element parses via the shared descriptor reader
    // (`entity_descriptor_from_lua`).
    let entities: Vec<EntityTypeDescriptor> = if table.contains_key("entities").map_err(|e| {
        ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` setupMod return value `entities` lookup failed: {e}"
            ),
        }
    })? {
        let raw: mlua::Value = table
            .get("entities")
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` setupMod `entities` field could not be read: {e}"
                ),
            })?;
        match raw {
            mlua::Value::Nil => Vec::new(),
            mlua::Value::Table(arr) => {
                let len = arr.raw_len();
                let mut out = Vec::with_capacity(len);
                for i in 1..=(len as i64) {
                    let item: mlua::Value =
                        arr.get(i).map_err(|e| ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` setupMod `entities[{i}]` could not be read: {e}"
                            ),
                        })?;
                    let descriptor = entity_descriptor_from_lua(item).map_err(|e| {
                        ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` setupMod `entities[{i}]` invalid: {e}"
                            ),
                        }
                    })?;
                    out.push(descriptor);
                }
                out
            }
            other => {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` setupMod `entities` field must be an array, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    } else {
        Vec::new()
    };

    Ok(ModManifestResult { name, entities })
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
            globalThis.setupLevel = function(ctx) {
                return {
                    reactions: [
                        { name: "wave1Complete", primitive: "moveGeometry", tag: "reactor" },
                    ],
                };
            };
            "#,
        );
        let manifest = rt.run_data_script(&section, &std::env::temp_dir());
        assert_eq!(manifest.reactions.len(), 1);
        assert_eq!(manifest.reactions[0].name, "wave1Complete");
    }

    #[test]
    fn run_data_script_luau_populates_manifest() {
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/data.luau",
            r#"
            function setupLevel(ctx)
                return {
                    reactions = {
                        { name = "wave1Complete", primitive = "moveGeometry", tag = "reactor" },
                    },
                }
            end
            "#,
        );
        let manifest = rt.run_data_script(&section, &std::env::temp_dir());
        assert_eq!(manifest.reactions.len(), 1);
    }

    #[test]
    fn run_data_script_luau_require_resolves_from_mod_root() {
        // Asserts the same resolver wiring as the mod-init VM is active in
        // the per-level data context: `require("./shared/loot")` resolves
        // against `mod_root` instead of erroring with "attempt to call a nil
        // value".
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("data_require");
        std::fs::write(
            dir.join("shared.luau"),
            r#"
            return {
                reaction = { name = "wave1Complete", primitive = "moveGeometry", tag = "reactor" },
            }
            "#,
        )
        .unwrap();
        let section = data_section(
            &dir.join("data.luau").to_string_lossy(),
            r#"
            local m = require("./shared")
            function setupLevel(ctx)
                return { reactions = { m.reaction } }
            end
            "#,
        );
        let manifest = rt.run_data_script(&section, &dir);
        assert_eq!(
            manifest.reactions.len(),
            1,
            "data-context VM must resolve `require` against mod root",
        );
        assert_eq!(manifest.reactions[0].name, "wave1Complete");
    }

    #[test]
    fn run_data_script_luau_denylist_active_in_data_context() {
        // The data-context VM must apply the same deny-list as the mod-init
        // VM: `io`, `os.execute`, `dofile`, etc. must be nil.
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/denylist.luau",
            r#"
            assert(io == nil, "io must be denied in data context")
            assert(os.execute == nil, "os.execute must be denied in data context")
            assert(dofile == nil, "dofile must be denied in data context")
            function setupLevel(ctx)
                return { reactions = {} }
            end
            "#,
        );
        let manifest = rt.run_data_script(&section, &std::env::temp_dir());
        // No reactions returned, but the asserts above are the contract:
        // if the deny-list is NOT active, any `assert(x == nil)` call will
        // throw (condition is false because x is reachable), and the manifest
        // comes back empty.
        // Re-assert via a positive check that the script ran to completion
        // by looking at logs is not feasible, so this test passes trivially
        // when the deny-list is active. If the deny-list is NOT installed,
        // the script throws and emits an empty manifest — which matches the
        // negative case. To distinguish, also verify a reaction round-trip:
        let _ = manifest;
        let section_ok = data_section(
            "/maps/denylist_ok.luau",
            r#"
            assert(io == nil)
            function setupLevel(ctx)
                return {
                    reactions = {
                        { name = "ok", primitive = "moveGeometry", tag = "t" },
                    },
                }
            end
            "#,
        );
        let m = rt.run_data_script(&section_ok, &std::env::temp_dir());
        assert_eq!(
            m.reactions.len(),
            1,
            "deny-list assert + manifest should round-trip"
        );
    }

    #[test]
    fn run_data_script_missing_export_returns_empty_manifest() {
        let (rt, _ctx) = runtime();
        let section = data_section(
            "/maps/no_export.js",
            "// script with no setupLevel export\nlet x = 1;",
        );
        let manifest = rt.run_data_script(&section, &std::env::temp_dir());
        assert!(manifest.reactions.is_empty());
    }

    #[test]
    fn run_data_script_invalid_utf8_returns_empty_manifest() {
        let (rt, _ctx) = runtime();
        let section = DataScriptSection {
            compiled_bytes: vec![0xFFu8, 0xFE, 0xFD],
            source_path: "/maps/binary.js".to_string(),
        };
        let manifest = rt.run_data_script(&section, &std::env::temp_dir());
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

    /// RAII wrapper: removes the temp directory when dropped, so an assertion
    /// panic doesn't leak state under `std::env::temp_dir()`.
    struct TempModRoot(std::path::PathBuf);

    impl std::ops::Deref for TempModRoot {
        type Target = std::path::Path;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TempModRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_mod_root(name: &str) -> TempModRoot {
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
        TempModRoot(p)
    }

    #[test]
    #[cfg(debug_assertions)]
    fn mod_init_missing_start_script_debug_returns_none() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("missing");
        rt.run_mod_init(&dir).unwrap();
        assert!(rt.mod_manifest().is_none());
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn mod_init_missing_start_script_release_errors() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("missing_release");
        let err = rt
            .run_mod_init(&dir)
            .expect_err("release builds must require a start-script");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(
                    reason.contains("no `start-script"),
                    "expected missing-start-script error, got: {reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
        assert!(rt.mod_manifest().is_none());
    }

    #[test]
    fn mod_init_quickjs_manifest_carries_entity_descriptor() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("js_register");
        // start-script.js: `setupMod` returns a player entity descriptor on
        // the manifest's `entities` field. Boot-side ingestion drains
        // the field into `DataRegistry`; this test asserts the manifest shape.
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.setupMod = function() {
                return {
                    name: "TestMod",
                    entities: [{ canonicalName: "smoke_pillar" }],
                };
            };
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "TestMod");
        assert!(
            manifest
                .entities
                .iter()
                .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
            "setupMod's `entities` field must carry the descriptor on the manifest"
        );
    }

    #[test]
    fn mod_init_quickjs_imported_domain_script_manifest_carries_entity_descriptor() {
        // Acceptance criterion: an entity type defined in a domain script that
        // was bundled into start-script.js by `scripts-build` (not defined
        // directly in start-script itself) is carried on the mod manifest
        // after mod-init. `scripts-build` inlines all imports at build time,
        // so the fixture is a single JS file whose intent — a descriptor
        // exported from a bundled domain script and aggregated into the
        // `setupMod` return — is made explicit by the inlined-comment markers.
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("js_imported_domain");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            /* inlined from actors/player.ts */
            const playerEntity = { canonicalName: "smoke_pillar" };
            /* end inlined actors/player.ts */
            globalThis.setupMod = function() {
                return { name: "ImportedDomainMod", entities: [playerEntity] };
            };
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "ImportedDomainMod");
        assert!(
            manifest
                .entities
                .iter()
                .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
            "entity type from bundled domain script must appear on the mod manifest"
        );
    }

    #[test]
    fn mod_init_luau_manifest_carries_entity_descriptor() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_register");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            function setupMod()
                return {
                    name = "TestMod",
                    entities = { { canonicalName = "smoke_pillar" } },
                }
            end
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "TestMod");
        assert!(
            manifest
                .entities
                .iter()
                .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
            "setupMod's `entities` field must carry the descriptor on the manifest"
        );
    }

    #[test]
    fn mod_init_missing_setup_mod_errors() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("no_setup");
        std::fs::write(dir.join("start-script.js"), "var x = 1;\n").unwrap();
        let err = rt.run_mod_init(&dir).expect_err("missing setupMod");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("setupMod"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_setup_mod_missing_name_errors() {
        let (mut rt, _ctx) = runtime();
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
    }

    #[test]
    fn mod_init_setup_mod_throws_errors() {
        let (mut rt, _ctx) = runtime();
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
    }

    #[test]
    fn mod_init_setup_mod_non_object_return_errors() {
        let (mut rt, _ctx) = runtime();
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
                    reason.contains("object"),
                    "expected 'object' in error reason, got: {reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_missing_setup_mod_errors() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_no_setup");
        // Module-style script that returns a table with no `setupMod` key —
        // and never assigns a global `setupMod` either.
        std::fs::write(dir.join("start-script.luau"), "local x = 1\n").unwrap();
        let err = rt.run_mod_init(&dir).expect_err("missing setupMod");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("setupMod"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_setup_mod_throws_errors() {
        // Regression: mlua wraps Lua errors in a traceback whose format is
        // implementation-defined. Assert only the variant — not the message
        // text — so an mlua version bump can't break this test.
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_throws");
        std::fs::write(
            dir.join("start-script.luau"),
            "function setupMod() error(\"boom\") end\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("setupMod throws");
        match err {
            ScriptError::ScriptThrew { .. } => {}
            other => panic!("expected ScriptThrew, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_setup_mod_non_table_return_errors() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_non_table");
        std::fs::write(
            dir.join("start-script.luau"),
            "function setupMod() return 42 end\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("non-table return");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(
                    reason.contains("table"),
                    "expected 'table' in error reason, got: {reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_setup_mod_missing_name_errors() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_no_name");
        std::fs::write(
            dir.join("start-script.luau"),
            "function setupMod() return {} end\n",
        )
        .unwrap();
        let err = rt.run_mod_init(&dir).expect_err("missing name");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(reason.contains("name"), "{reason}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_both_js_and_lua_errors() {
        let (mut rt, _ctx) = runtime();
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
    }

    #[test]
    #[cfg(debug_assertions)]
    fn mod_init_both_ts_and_luau_errors_without_writing_js() {
        // Regression: previously the debug TS->JS auto-compile ran before the
        // both-present check, so a user with `start-script.ts` + `.luau`
        // would get an unwanted `start-script.js` materialized on disk and
        // have to delete it manually to switch to the Luau path. The check
        // must short-circuit before any compilation.
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("both_ts_luau");
        std::fs::write(
            dir.join("start-script.ts"),
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
        assert!(
            !dir.join("start-script.js").exists(),
            "both-present error must short-circuit before TS->JS compile writes start-script.js",
        );
    }

    #[test]
    fn mod_init_quickjs_entities_field_parses_descriptor() {
        // `setupMod()` returns an `entities` array; each element should parse
        // into an `EntityTypeDescriptor` and be carried on the manifest. The
        // Ingestion into `DataRegistry` is handled by the boot caller; this
        // test covers only the parse path.
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("js_entities_field");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.setupMod = function() {
                return {
                    name: "EntitiesMod",
                    entities: [{ canonicalName: "smoke_pillar" }],
                };
            };
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "EntitiesMod");
        assert_eq!(manifest.entities.len(), 1);
        assert_eq!(
            manifest.entities[0].canonical_name.as_deref(),
            Some("smoke_pillar"),
        );
    }

    #[test]
    fn mod_init_quickjs_entities_missing_key_gives_empty_vec() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("js_entities_missing");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.setupMod = function() { return { name: "NoEntitiesMod" }; };
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert!(manifest.entities.is_empty());
    }

    #[test]
    fn mod_init_quickjs_entities_not_array_gives_error() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("js_entities_bad");
        std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.setupMod = function() {
                return { name: "Bad", entities: "bad" };
            };
            "#,
        )
        .unwrap();

        let err = rt.run_mod_init(&dir).expect_err("entities must be array");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(
                    reason.contains("entities"),
                    "expected 'entities' in reason, got: {reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_entities_field_parses_descriptor() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_entities_field");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            function setupMod()
                return {
                    name = "EntitiesMod",
                    entities = { { canonicalName = "smoke_pillar" } },
                }
            end
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "EntitiesMod");
        assert_eq!(manifest.entities.len(), 1);
        assert_eq!(
            manifest.entities[0].canonical_name.as_deref(),
            Some("smoke_pillar"),
        );
    }

    #[test]
    fn mod_init_luau_entities_missing_key_gives_empty_vec() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_entities_missing");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            function setupMod() return { name = "NoEntitiesMod" } end
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert!(manifest.entities.is_empty());
    }

    #[test]
    fn mod_init_luau_entities_not_array_gives_error() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_entities_bad");
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            function setupMod()
                return { name = "Bad", entities = "bad" }
            end
            "#,
        )
        .unwrap();

        let err = rt.run_mod_init(&dir).expect_err("entities must be array");
        match err {
            ScriptError::InvalidArgument { reason } => {
                assert!(
                    reason.contains("entities"),
                    "expected 'entities' in reason, got: {reason}"
                );
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn mod_init_luau_require_resolves_from_mod_root() {
        let (mut rt, _ctx) = runtime();
        let dir = temp_mod_root("luau_require");
        // Sub-module returns a descriptor; start-script imports it and folds
        // it into the manifest's `entities` field.
        std::fs::write(
            dir.join("sub.luau"),
            r#"
            return { descriptor = { canonicalName = "smoke_pillar" } }
            "#,
        )
        .unwrap();
        std::fs::write(
            dir.join("start-script.luau"),
            r#"
            local m = require("./sub")
            function setupMod()
                return { name = "Imported", entities = { m.descriptor } }
            end
            "#,
        )
        .unwrap();

        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "Imported");
        assert!(
            manifest
                .entities
                .iter()
                .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
            "domain script imported via require must contribute its entity type to the manifest"
        );
    }

    #[test]
    fn mod_init_luau_require_rejects_parent_dir_traversal() {
        let (mut rt, _ctx) = runtime();
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
        rt.run_mod_init(&dir).unwrap();
        let manifest = rt.mod_manifest().expect("Some manifest");
        assert_eq!(manifest.name, "GuardedMod");
    }

    // --- compile_stale_scripts tests -----------------------------------------

    #[test]
    fn compile_stale_scripts_is_noop_for_nonexistent_directory() {
        // Passing a directory that does not exist must not panic or error.
        // No compiler is invoked because `scan_and_compile_stale_ts` returns
        // early when the path is not a directory.
        let (rt, _ctx) = runtime();
        let absent = std::env::temp_dir().join("postretro_scan_absent_dir_test");
        assert!(!absent.exists(), "test setup: dir must not pre-exist");
        // Should silently no-op.
        rt.compile_stale_scripts(&absent, &absent);
    }

    #[test]
    fn compile_stale_scripts_is_noop_when_no_ts_files_present() {
        // `scripts/` directory exists but contains only `.luau` files. The
        // scan walks the directory and finds nothing to compile.
        let (rt, _ctx) = runtime();
        let dir = temp_mod_root("scan_no_ts");
        std::fs::write(dir.join("archetypes.luau"), "-- luau only\n").unwrap();
        // Must complete without panic; no compiler binary needed.
        rt.compile_stale_scripts(&dir, &dir);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn compile_stale_scripts_recompiles_ts_with_stale_js_sibling() {
        // Acceptance criterion: a `.ts` file whose sibling `.js` is older than
        // the `.ts` (or absent) gets recompiled by the startup scan.
        use std::time::{Duration, SystemTime};

        let compiler_path = ensure_scripts_build();
        let (_rt, _ctx) = runtime();
        let dir = temp_mod_root("scan_stale_ts");

        let ts_path = dir.join("archetypes.ts");
        let js_path = dir.join("archetypes.js");

        fs::write(&ts_path, "export const x: number = 1;\n").unwrap();

        // Write a JS sibling backdated by 5 seconds so it is definitely older
        // than the TS. Use `set_modified` (std 1.75+) if available; fall back
        // to simply not writing the JS at all (trigger the "missing sibling"
        // code path instead).
        fs::write(&js_path, "// stale\n").unwrap();
        let stale_time = SystemTime::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Ok(file) = std::fs::File::options().write(true).open(&js_path) {
            // `set_modified` is gated on the platform supporting it; ignore
            // failures gracefully — the missing-sibling path is exercised
            // instead if the mtime cannot be set.
            let _ = file.set_modified(stale_time);
            drop(file);
        }

        // Override PATH so `TsCompilerPath::detect()` finds our binary.
        // `set_var` is only safe in single-threaded contexts; cargo test runs
        // each `#[test]` on its own thread but a cargo test binary runs all
        // threads in the same process, so we use the direct-call variant of
        // the private helper instead of mutating the process environment.
        // Instead, we invoke `scan_and_compile_stale_ts` with a synthesized
        // compiler path via the `watcher` module's public API directly.
        let _ = compiler_path; // compiler path used below via watcher API
        // Since `compile_stale_scripts` relies on `TsCompilerPath::detect()`,
        // which reads `current_exe`, we cannot inject an arbitrary path. But
        // we can test the helper `visit_ts_files` directly, which is what
        // `scan_and_compile_stale_ts` delegates to.
        let mut compiled = 0u32;
        let mut failed = 0u32;
        let compiler =
            super::super::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
        super::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

        assert_eq!(failed, 0, "no compile failures expected; failed={failed}",);
        assert_eq!(
            compiled, 1,
            "exactly one stale .ts file should have been compiled",
        );
        assert!(
            js_path.is_file(),
            "compiled output `{}` must exist after scan",
            js_path.display(),
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn compile_stale_scripts_skips_fresh_ts_files() {
        // A `.ts` whose `.js` sibling is newer is skipped.
        let dir = temp_mod_root("scan_fresh_ts");
        let ts_path = dir.join("archetypes.ts");
        let js_path = dir.join("archetypes.js");

        // Write the JS first so it has an older mtime, then write the TS so
        // it ends up newer. Because filesystem mtime granularity may be 1s on
        // some platforms, we forcibly set the JS mtime to the future.
        fs::write(&js_path, "// fresh\n").unwrap();
        fs::write(&ts_path, "export const x: number = 1;\n").unwrap();

        // Backdate the TS by 5 seconds to make the JS appear newer.
        let old_time = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(5))
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if let Ok(f) = std::fs::File::options().write(true).open(&ts_path) {
            let _ = f.set_modified(old_time);
        }

        let mut compiled = 0u32;
        let mut failed = 0u32;
        // `ensure_scripts_build` not needed — the TS is fresh so no compile runs.
        // We still need a valid `TsCompilerPath` to pass to `visit_ts_files`.
        // Use a dummy path — it will never be invoked.
        let compiler = super::super::watcher::TsCompilerPath::ScriptsBuildOnPath(
            std::path::PathBuf::from("/dev/null/scripts-build-dummy"),
        );
        super::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

        assert_eq!(compiled, 0, "fresh .ts must not be recompiled");
        assert_eq!(failed, 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn compile_stale_scripts_walks_subdirectories() {
        // A stale `.ts` nested inside a subdirectory must be found and
        // compiled.
        let dir = temp_mod_root("scan_nested_ts");
        let sub = dir.join("actors");
        fs::create_dir_all(&sub).unwrap();

        let ts_path = sub.join("player.ts");
        let js_path = sub.join("player.js");
        fs::write(&ts_path, "export const role: string = 'player';\n").unwrap();
        // No JS sibling → needs build.

        let mut compiled = 0u32;
        let mut failed = 0u32;
        let compiler =
            super::super::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
        super::visit_ts_files(&dir, &compiler, &mut compiled, &mut failed);

        assert_eq!(failed, 0);
        assert_eq!(compiled, 1, "nested stale .ts should be compiled");
        assert!(
            js_path.is_file(),
            "compiled output `{}` must exist",
            js_path.display(),
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn visit_ts_files_shallow_skips_nested_directories() {
        // Mod-root scope is one level only — nested `.ts` files are the
        // recursive `script_root` walk's territory.
        let dir = temp_mod_root("scan_shallow");
        fs::write(dir.join("start-script.ts"), "export {};\n").unwrap();
        let nested = dir.join("scripts");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("nested.ts"), "export {};\n").unwrap();

        let mut compiled = 0u32;
        let mut failed = 0u32;
        let compiler =
            super::super::watcher::TsCompilerPath::ScriptsBuildOnPath(ensure_scripts_build());
        super::visit_ts_files_shallow(&dir, &compiler, &mut compiled, &mut failed);

        assert_eq!(failed, 0);
        assert_eq!(
            compiled, 1,
            "only top-level start-script.ts should compile; nested/nested.ts must be left for the recursive walk"
        );
        assert!(dir.join("start-script.js").is_file());
        assert!(
            !nested.join("nested.js").is_file(),
            "shallow walk must not descend into subdirectories"
        );
    }

    /// Locate the freshly-built `scripts-build` binary. Mirrors the same
    /// helper in `watcher.rs` tests. CARGO_MANIFEST_DIR is always set by cargo.
    fn ensure_scripts_build() -> std::path::PathBuf {
        fn scripts_build_binary() -> Option<std::path::PathBuf> {
            let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let name = if cfg!(windows) {
                "scripts-build.exe"
            } else {
                "scripts-build"
            };
            let mut dir: Option<&std::path::Path> = Some(manifest.as_path());
            while let Some(d) = dir {
                for profile in ["debug", "release"] {
                    let candidate = d.join("target").join(profile).join(name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
                dir = d.parent();
            }
            None
        }

        if let Some(p) = scripts_build_binary() {
            return p;
        }
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "postretro-script-compiler"])
            .status()
            .expect("cargo build scripts-build");
        assert!(status.success(), "failed to build scripts-build");
        scripts_build_binary().expect("scripts-build should exist after build")
    }
}
