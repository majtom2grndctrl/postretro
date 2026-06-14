// Staged mod-init manifest builds for debug hot reload.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use rquickjs::{
    Array as JsArray, CatchResultExt, Context as JsContext, Function as JsFunction,
    Object as JsObject, Runtime as JsRuntime, Value as JsValue,
};

use super::data_descriptors::{
    EntityTypeDescriptor, drain_fonts_js, drain_fonts_lua, drain_theme_js, drain_theme_lua,
    drain_ui_trees_js, drain_ui_trees_lua, entity_descriptor_from_js,
};
use super::error::ScriptError;
use super::luau::{LuauConfig, LuauRequireTracker};
use super::primitives::store::{
    SharedStoreDeclarationAttempt, StoreDeclarationAttempt, store_declaration_primitive,
};
use super::quickjs::{QuickJsConfig, run_script};
use super::runtime::ModManifestResult;
use super::slot_table::StoreDeclarationSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StagedManifestDiagnosticSeverity {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StagedManifestDiagnostic {
    pub(crate) severity: StagedManifestDiagnosticSeverity,
    pub(crate) message: String,
}

impl StagedManifestDiagnostic {
    fn info(message: impl Into<String>) -> Self {
        Self {
            severity: StagedManifestDiagnosticSeverity::Info,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: StagedManifestDiagnosticSeverity::Error,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StagedManifest {
    pub(crate) name: String,
    pub(crate) entities: Vec<EntityTypeDescriptor>,
    pub(crate) store_declarations: StoreDeclarationSet,
    /// Canonical mod-init source dependencies carried across the worker→main
    /// thread boundary. The descriptor registry write and watcher classifier
    /// update both happen on the main thread in `commit_staged_manifest_result`,
    /// where the engine registry is mutably owned; this field is what makes the
    /// dependency set available at that commit point.
    pub(crate) dependency_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StagedManifestBuildStatus {
    Built(StagedManifest),
    NoStartScript,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StagedManifestBuildResult {
    pub(crate) generation: u64,
    pub(crate) mod_root: PathBuf,
    pub(crate) status: StagedManifestBuildStatus,
    pub(crate) diagnostics: Vec<StagedManifestDiagnostic>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StagedManifestBuildConfig {
    pub(crate) quickjs: QuickJsConfig,
    pub(crate) luau: LuauConfig,
}

#[derive(Clone, Debug)]
struct BuildRequest {
    generation: u64,
    mod_root: PathBuf,
    cfg: StagedManifestBuildConfig,
}

enum LaneRequest {
    Build(BuildRequest),
    Shutdown,
}

pub(crate) struct StagedManifestBuildLane {
    request_tx: Sender<LaneRequest>,
    result_rx: Receiver<StagedManifestBuildResult>,
    next_generation: u64,
    latest_requested_generation: u64,
    worker: Option<JoinHandle<()>>,
}

impl StagedManifestBuildLane {
    pub(crate) fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || worker_loop(request_rx, result_tx));
        Self {
            request_tx,
            result_rx,
            next_generation: 0,
            latest_requested_generation: 0,
            worker: Some(worker),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_latest(generation: u64) -> Self {
        let mut lane = Self::new();
        lane.next_generation = generation;
        lane.latest_requested_generation = generation;
        lane
    }

    pub(crate) fn enqueue(
        &mut self,
        mod_root: impl Into<PathBuf>,
        cfg: StagedManifestBuildConfig,
    ) -> Result<u64, ScriptError> {
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        self.latest_requested_generation = generation;
        self.request_tx
            .send(LaneRequest::Build(BuildRequest {
                generation,
                mod_root: mod_root.into(),
                cfg,
            }))
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("staged manifest lane is not available: {e}"),
            })?;
        Ok(generation)
    }

    pub(crate) fn latest_requested_generation(&self) -> u64 {
        self.latest_requested_generation
    }

    pub(crate) fn poll_completed(&mut self) -> Vec<StagedManifestBuildResult> {
        let mut completed = Vec::new();
        while let Ok(result) = self.result_rx.try_recv() {
            completed.push(result);
        }
        completed
    }
}

impl Drop for StagedManifestBuildLane {
    fn drop(&mut self) {
        let _ = self.request_tx.send(LaneRequest::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(request_rx: Receiver<LaneRequest>, result_tx: Sender<StagedManifestBuildResult>) {
    while let Ok(msg) = request_rx.recv() {
        let mut request = match msg {
            LaneRequest::Build(request) => request,
            LaneRequest::Shutdown => break,
        };

        while let Ok(msg) = request_rx.try_recv() {
            match msg {
                LaneRequest::Build(next) => request = next,
                LaneRequest::Shutdown => return,
            }
        }

        let result = build_staged_manifest(&request.mod_root, request.generation, &request.cfg);
        if result_tx.send(result).is_err() {
            break;
        }
    }
}

pub(crate) fn build_staged_manifest(
    mod_root: &Path,
    generation: u64,
    cfg: &StagedManifestBuildConfig,
) -> StagedManifestBuildResult {
    let mut diagnostics = Vec::new();

    match run_staged_manifest_build(mod_root, cfg, &mut diagnostics) {
        Ok(Some(manifest)) => StagedManifestBuildResult {
            generation,
            mod_root: mod_root.to_path_buf(),
            status: StagedManifestBuildStatus::Built(manifest),
            diagnostics,
        },
        Ok(None) => StagedManifestBuildResult {
            generation,
            mod_root: mod_root.to_path_buf(),
            status: StagedManifestBuildStatus::NoStartScript,
            diagnostics,
        },
        Err(err) => {
            diagnostics.push(StagedManifestDiagnostic::error(err.to_string()));
            StagedManifestBuildResult {
                generation,
                mod_root: mod_root.to_path_buf(),
                status: StagedManifestBuildStatus::Failed,
                diagnostics,
            }
        }
    }
}

fn run_staged_manifest_build(
    mod_root: &Path,
    cfg: &StagedManifestBuildConfig,
    diagnostics: &mut Vec<StagedManifestDiagnostic>,
) -> Result<Option<StagedManifest>, ScriptError> {
    let js_path = mod_root.join("start-script.js");
    let ts_path = mod_root.join("start-script.ts");
    let luau_path = mod_root.join("start-script.luau");

    let has_luau = luau_path.is_file();
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

    #[cfg(debug_assertions)]
    let mut ts_dependency_paths: Option<Vec<PathBuf>> = None;
    #[cfg(debug_assertions)]
    {
        if ts_path.is_file() {
            let compiler = super::watcher::TsCompilerPath::detect().ok_or_else(|| {
                ScriptError::InvalidArgument {
                    reason: "mod-init: failed to compile start-script.ts: scripts-build not found"
                        .to_string(),
                }
            })?;
            let report = super::watcher::run_ts_compiler_with_dependency_report(
                &compiler, &ts_path, &js_path,
            )
            .map_err(|e| ScriptError::InvalidArgument {
                reason: format!("mod-init: failed to compile `{}`: {e}", ts_path.display()),
            })?;
            let output = canonical_or_original(&report.output);
            let mut dependencies = report.dependencies;
            dependencies.push(report.entry);
            dependencies.retain(|path| canonical_or_original(path) != output);
            dependencies.sort();
            dependencies.dedup();
            ts_dependency_paths = Some(dependencies);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = ts_path;

    let has_js = js_path.is_file();
    if !has_js && !has_luau {
        #[cfg(debug_assertions)]
        {
            diagnostics.push(StagedManifestDiagnostic::info(format!(
                "[Mod-init] no start-script at `{}` - skipping (debug)",
                mod_root.display(),
            )));
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

    let (manifest, dependency_paths) = if has_js {
        let source = fs::read_to_string(&js_path).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: failed to read `{}`: {e}", js_path.display()),
        })?;
        let manifest =
            run_staged_mod_init_quickjs(&source, &js_path.to_string_lossy(), &cfg.quickjs)?;
        #[cfg(debug_assertions)]
        let entry = if mod_root.join("start-script.ts").is_file() {
            mod_root.join("start-script.ts")
        } else {
            js_path.clone()
        };
        #[cfg(not(debug_assertions))]
        let entry = js_path.clone();
        #[cfg(debug_assertions)]
        let dependencies =
            ts_dependency_paths.unwrap_or_else(|| vec![canonical_or_original(&entry)]);
        #[cfg(not(debug_assertions))]
        let dependencies = vec![canonical_or_original(&entry)];
        (manifest, dependencies)
    } else {
        let source = fs::read_to_string(&luau_path).map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: failed to read `{}`: {e}", luau_path.display()),
        })?;
        let tracker = LuauRequireTracker::new(mod_root)?;
        let manifest = run_staged_mod_init_luau(
            &source,
            &luau_path.to_string_lossy(),
            mod_root,
            &cfg.luau,
            Some(&tracker),
        )?;
        let mut dependencies = tracker.dependency_paths();
        dependencies.push(canonical_or_original(&luau_path));
        dependencies.sort();
        dependencies.dedup();
        (manifest, dependencies)
    };

    diagnostics.push(StagedManifestDiagnostic::info(format!(
        "[Mod-init] staged mod `{}`",
        manifest.name,
    )));

    Ok(Some(StagedManifest {
        name: manifest.name,
        entities: manifest.entities,
        store_declarations: manifest.store_declarations,
        dependency_paths,
    }))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn run_staged_mod_init_quickjs(
    source: &str,
    source_path: &str,
    cfg: &QuickJsConfig,
) -> Result<ModManifestResult, ScriptError> {
    let runtime = JsRuntime::new().map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: failed to create runtime: {e}"),
    })?;
    runtime.set_memory_limit(cfg.memory_limit_bytes);

    let ctx = JsContext::full(&runtime).map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: failed to create context: {e}"),
    })?;
    let declaration_attempt: SharedStoreDeclarationAttempt =
        std::rc::Rc::new(std::cell::RefCell::new(StoreDeclarationAttempt::default()));
    let declaration_primitive = store_declaration_primitive(declaration_attempt.clone());

    let mut out: Result<ModManifestResult, ScriptError> = Err(ScriptError::InvalidArgument {
        reason: "mod-init: setupMod did not produce a manifest".to_string(),
    });

    ctx.with(|ctx| {
        if let Err(e) = (declaration_primitive.quickjs_installer)(&ctx) {
            out = Err(ScriptError::InvalidArgument {
                reason: format!("mod-init: failed to install primitive `defineStore`: {e}"),
            });
            return;
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

        out = manifest_from_js_value(&ctx, source_path, returned);
    });

    let mut manifest = out?;
    manifest.store_declarations = declaration_attempt.borrow().clone().finish()?;
    Ok(manifest)
}

fn manifest_from_js_value<'js>(
    ctx: &rquickjs::Ctx<'js>,
    source_path: &str,
    returned: JsValue<'js>,
) -> Result<ModManifestResult, ScriptError> {
    let obj = JsObject::from_value(returned).map_err(|_| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod must return an object"),
    })?;

    let name: String = obj.get("name").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod return value missing `name`: {e}"),
    })?;

    let entities = match obj.contains_key("entities") {
        Ok(false) => Vec::new(),
        Ok(true) => match obj.get::<_, JsArray>("entities") {
            Ok(arr) => {
                let mut parsed = Vec::with_capacity(arr.len());
                for i in 0..arr.len() {
                    let v: JsValue = arr.get(i).map_err(|e| ScriptError::InvalidArgument {
                        reason: format!(
                            "mod-init: `{source_path}` setupMod `entities[{i}]` could not be read: {e}"
                        ),
                    })?;
                    let descriptor = entity_descriptor_from_js(ctx, v).map_err(|e| {
                        ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` setupMod `entities[{i}]` invalid: {e}"
                            ),
                        }
                    })?;
                    parsed.push(descriptor);
                }
                parsed
            }
            Err(e) => {
                return Err(ScriptError::InvalidArgument {
                    reason: format!(
                        "mod-init: `{source_path}` setupMod `entities` field must be an array: {e}"
                    ),
                });
            }
        },
        Err(e) => {
            return Err(ScriptError::InvalidArgument {
                reason: format!(
                    "mod-init: `{source_path}` setupMod return value `entities` lookup failed: {e}"
                ),
            });
        }
    };

    // UI fields drain via the G1a bridge fns; malformed entries log+skip inside
    // the drains (ui.md §1.1). This is the hot-reload twin of `run_mod_init_quickjs`.
    let ui_trees =
        drain_ui_trees_js(ctx, &obj, "setupMod").map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` setupMod `uiTrees` invalid: {e}"),
        })?;
    let theme = drain_theme_js(&obj, "setupMod").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod `theme` invalid: {e}"),
    })?;
    let fonts = drain_fonts_js(&obj, "setupMod").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod `fonts` invalid: {e}"),
    })?;

    Ok(ModManifestResult {
        name,
        entities,
        ui_trees,
        theme,
        fonts,
        store_declarations: StoreDeclarationSet::default(),
    })
}

fn run_staged_mod_init_luau(
    source: &str,
    source_path: &str,
    mod_root: &Path,
    _cfg: &LuauConfig,
    require_tracker: Option<&LuauRequireTracker>,
) -> Result<ModManifestResult, ScriptError> {
    let declaration_attempt: SharedStoreDeclarationAttempt =
        std::rc::Rc::new(std::cell::RefCell::new(StoreDeclarationAttempt::default()));
    let declaration_primitive = store_declaration_primitive(declaration_attempt.clone());
    let lua = super::luau::build_lua_state_with_require_tracking(
        &[declaration_primitive],
        None,
        Some(mod_root),
        require_tracker,
    )?;

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

    let entities = if table
        .contains_key("entities")
        .map_err(|e| ScriptError::InvalidArgument {
            reason: format!(
                "mod-init: `{source_path}` setupMod return value `entities` lookup failed: {e}"
            ),
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
                    let descriptor = super::data_descriptors::entity_descriptor_from_lua(item)
                        .map_err(|e| ScriptError::InvalidArgument {
                            reason: format!(
                                "mod-init: `{source_path}` setupMod `entities[{i}]` invalid: {e}"
                            ),
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

    // UI fields drain via the G1a bridge fns; malformed entries log+skip inside
    // the drains (ui.md §1.1). This is the hot-reload twin of `run_mod_init_luau`.
    let ui_trees =
        drain_ui_trees_lua(&table, "setupMod").map_err(|e| ScriptError::InvalidArgument {
            reason: format!("mod-init: `{source_path}` setupMod `uiTrees` invalid: {e}"),
        })?;
    let theme = drain_theme_lua(&table, "setupMod").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod `theme` invalid: {e}"),
    })?;
    let fonts = drain_fonts_lua(&table, "setupMod").map_err(|e| ScriptError::InvalidArgument {
        reason: format!("mod-init: `{source_path}` setupMod `fonts` invalid: {e}"),
    })?;

    Ok(ModManifestResult {
        name,
        entities,
        ui_trees,
        theme,
        fonts,
        store_declarations: declaration_attempt.borrow().clone().finish()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct TempModRoot(PathBuf);

    impl std::ops::Deref for TempModRoot {
        type Target = Path;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for TempModRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_mod_root(name: &str) -> TempModRoot {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "postretro_staged_manifest_test_{}_{}_{name}",
            std::process::id(),
            n,
        ));
        fs::create_dir_all(&p).unwrap();
        TempModRoot(p)
    }

    fn assert_send<T: Send>() {}

    #[test]
    fn staged_manifest_output_is_send() {
        assert_send::<StagedManifest>();
        assert_send::<StagedManifestBuildResult>();
    }

    #[test]
    fn staged_manifest_build_quickjs_returns_owned_descriptor_data() {
        let dir = temp_mod_root("js_success");
        fs::write(
            dir.join("start-script.js"),
            r#"
            const handles = defineStore("staged", {
                count: { type: "number", default: 1 },
            });
            if (handles.count !== "staged.count") throw new Error("bad store handle");
            globalThis.setupMod = function() {
                return {
                    name: "StagedMod",
                    entities: [{ canonicalName: "smoke_pillar" }],
                };
            };
            "#,
        )
        .unwrap();

        let result = build_staged_manifest(&dir, 7, &StagedManifestBuildConfig::default());
        assert_eq!(result.generation, 7);
        let StagedManifestBuildStatus::Built(manifest) = result.status else {
            panic!("expected built result, got {:?}", result.status);
        };
        assert_eq!(manifest.name, "StagedMod");
        assert_eq!(manifest.entities.len(), 1);
        assert_eq!(manifest.store_declarations.len(), 1);
        assert_eq!(
            manifest.entities[0].canonical_name.as_deref(),
            Some("smoke_pillar")
        );
        assert_eq!(manifest.dependency_paths.len(), 1);
    }

    #[test]
    fn staged_manifest_worker_builds_successful_snapshot() {
        let dir = temp_mod_root("worker_success");
        fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.setupMod = function() {
                return {
                    name: "WorkerBuilt",
                    entities: [{ canonicalName: "worker_grunt" }],
                };
            };
            "#,
        )
        .unwrap();

        let mut lane = StagedManifestBuildLane::new();
        let generation = lane
            .enqueue(dir.0.clone(), StagedManifestBuildConfig::default())
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut result = None;
        while Instant::now() < deadline {
            result = lane
                .poll_completed()
                .into_iter()
                .find(|r| r.generation == generation);
            if result.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let result = result.expect("worker should complete successful staged build");
        let StagedManifestBuildStatus::Built(manifest) = result.status else {
            panic!("expected worker built result, got {:?}", result.status);
        };
        assert_eq!(manifest.name, "WorkerBuilt");
        assert_eq!(manifest.entities.len(), 1);
        assert_eq!(
            manifest.entities[0].canonical_name.as_deref(),
            Some("worker_grunt")
        );
    }

    #[test]
    fn staged_manifest_build_luau_reports_entry_and_required_dependencies() {
        let dir = temp_mod_root("luau_dependencies");
        fs::create_dir_all(dir.join("actors")).unwrap();
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::write(
            dir.join("actors/player.luau"),
            "return { descriptor = { canonicalName = 'player' } }\n",
        )
        .unwrap();
        fs::write(
            dir.join("actors/weapons.luau"),
            "return { descriptor = { canonicalName = 'pulse_rifle' } }\n",
        )
        .unwrap();
        fs::write(
            dir.join("data/level.luau"),
            "local ignored = require('../actors/player')\nreturn {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("start-script.luau"),
            r#"
            local player = require("./actors/player")
            local player_again = require("./actors/player.luau")
            local weapons = require("actors/weapons")
            local handles = defineStore("staged", {
                count = { type = "number", default = 1 },
            })
            assert(handles.count == "staged.count")
            function setupMod()
                return {
                    name = "LuauDeps",
                    entities = {
                        player.descriptor,
                        player_again.descriptor,
                        weapons.descriptor,
                    },
                }
            end
            "#,
        )
        .unwrap();

        let result = build_staged_manifest(&dir, 11, &StagedManifestBuildConfig::default());
        let StagedManifestBuildStatus::Built(manifest) = result.status else {
            panic!("expected built result, got {:?}", result.status);
        };

        let expected = vec![
            dir.join("actors/player.luau").canonicalize().unwrap(),
            dir.join("actors/weapons.luau").canonicalize().unwrap(),
            dir.join("start-script.luau").canonicalize().unwrap(),
        ];
        assert_eq!(manifest.name, "LuauDeps");
        assert_eq!(manifest.store_declarations.len(), 1);
        assert_eq!(manifest.dependency_paths, expected);
        assert!(
            !manifest
                .dependency_paths
                .contains(&dir.join("data/level.luau").canonicalize().unwrap()),
            "data-script files must not enter mod-init dependency output"
        );
    }

    #[test]
    fn staged_manifest_build_failure_returns_diagnostics() {
        let dir = temp_mod_root("js_failure");
        fs::write(
            dir.join("start-script.js"),
            "globalThis.setupMod = function() { return {}; };\n",
        )
        .unwrap();

        let result = build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default());
        assert_eq!(result.status, StagedManifestBuildStatus::Failed);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.severity == StagedManifestDiagnosticSeverity::Error
                    && d.message.contains("name")),
            "expected missing-name diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[cfg(unix)]
    #[test]
    fn staged_manifest_build_luau_fails_when_require_resolves_outside_mod_root() {
        use std::os::unix::fs::symlink;

        let dir = temp_mod_root("luau_outside_root");
        let outside = temp_mod_root("luau_outside_target");
        fs::write(outside.join("module.luau"), "return {}\n").unwrap();
        symlink(outside.join("module.luau"), dir.join("linked.luau")).unwrap();
        fs::write(
            dir.join("start-script.luau"),
            r#"
            local linked = require("./linked")
            function setupMod()
                return { name = "Escaped" }
            end
            "#,
        )
        .unwrap();

        let result = build_staged_manifest(&dir, 12, &StagedManifestBuildConfig::default());
        assert_eq!(result.status, StagedManifestBuildStatus::Failed);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.severity == StagedManifestDiagnosticSeverity::Error
                    && d.message.contains("outside mod root")),
            "expected outside-root diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn staged_manifest_lane_coalesces_pending_generations() {
        let first = temp_mod_root("lane_first");
        fs::write(
            first.join("start-script.js"),
            "globalThis.setupMod = function() { return { name: 'First' }; };\n",
        )
        .unwrap();
        let latest = temp_mod_root("lane_latest");
        fs::write(
            latest.join("start-script.js"),
            "globalThis.setupMod = function() { return { name: 'Latest' }; };\n",
        )
        .unwrap();

        let mut lane = StagedManifestBuildLane::new();
        let first_generation = lane
            .enqueue(first.0.clone(), StagedManifestBuildConfig::default())
            .unwrap();
        let second_generation = lane
            .enqueue(latest.0.clone(), StagedManifestBuildConfig::default())
            .unwrap();
        let third_generation = lane
            .enqueue(latest.0.clone(), StagedManifestBuildConfig::default())
            .unwrap();
        assert_eq!(first_generation, 1);
        assert_eq!(second_generation, 2);
        assert_eq!(third_generation, 3);
        assert_eq!(lane.latest_requested_generation(), 3);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut completed = Vec::new();
        while Instant::now() < deadline {
            completed.extend(lane.poll_completed());
            if completed.iter().any(|r| r.generation == 3) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            completed.iter().any(|r| r.generation == 3),
            "expected latest generation to complete, got {completed:?}"
        );
        assert!(
            !completed.iter().any(|r| r.generation == 2),
            "middle pending generation should be coalesced, got {completed:?}"
        );
    }

    // --- G1b Task 1: hot-reload mod-init UI field drains --------------------

    /// Hot-reload JS parser (`manifest_from_js_value`) drains `uiTrees` / `theme`
    /// / `fonts` via the G1a bridge fns, the twin of the cold-boot path.
    #[test]
    fn manifest_from_js_value_drains_ui_fields() {
        let rt = JsRuntime::new().unwrap();
        let ctx = JsContext::full(&rt).unwrap();
        ctx.with(|ctx| {
            let returned: JsValue = ctx
                .eval(
                    r#"({
                        name: "UiMod",
                        uiTrees: [
                            { name: "hud", alwaysOn: true,
                              tree: { anchor: "topLeft", offset: [0.0, 0.0],
                                      root: { kind: "spacer", flexGrow: 1.0 } } },
                        ],
                        theme: { colors: { critical: [1.0, 0.0, 0.0, 1.0] } },
                        fonts: { body: "fonts/inter.ttf" },
                    })"#,
                )
                .unwrap();
            let manifest = manifest_from_js_value(&ctx, "/mod/start-script.js", returned)
                .expect("hot-reload JS parser must drain UI fields");
            assert_eq!(manifest.ui_trees.len(), 1);
            assert_eq!(manifest.ui_trees[0].name, "hud");
            assert!(manifest.ui_trees[0].always_on);
            assert_eq!(manifest.theme.colors["critical"], [1.0, 0.0, 0.0, 1.0]);
            assert_eq!(manifest.fonts.families["body"], "fonts/inter.ttf");
        });
    }

    /// Hot-reload JS parser skips a malformed `uiTrees` entry rather than failing.
    #[test]
    fn manifest_from_js_value_skips_malformed_ui_tree() {
        let rt = JsRuntime::new().unwrap();
        let ctx = JsContext::full(&rt).unwrap();
        ctx.with(|ctx| {
            let returned: JsValue = ctx
                .eval(
                    r#"({
                        name: "UiMod",
                        uiTrees: [
                            { name: "bad", tree: { anchor: "topLeft", offset: [0.0, 0.0], root: { kind: "carousel" } } },
                            { name: "good", tree: { anchor: "topLeft", offset: [0.0, 0.0], root: { kind: "spacer", flexGrow: 1.0 } } },
                        ],
                    })"#,
                )
                .unwrap();
            let manifest = manifest_from_js_value(&ctx, "/mod/start-script.js", returned)
                .expect("malformed UI tree must not abort the hot-reload parse");
            assert_eq!(manifest.ui_trees.len(), 1);
            assert_eq!(manifest.ui_trees[0].name, "good");
        });
    }

    /// Hot-reload Luau parser (`run_staged_mod_init_luau`) drains the UI fields,
    /// the twin of the cold-boot Luau path.
    #[test]
    fn run_staged_mod_init_luau_drains_ui_fields() {
        let dir = temp_mod_root("staged_luau_ui");
        let source = r#"
            function setupMod()
                return {
                    name = "UiMod",
                    uiTrees = {
                        { name = "hud", alwaysOn = true,
                          tree = { anchor = "topLeft", offset = { 0, 0 },
                                   root = { kind = "spacer", flexGrow = 1 } } },
                    },
                    theme = { colors = { critical = {1, 0, 0, 1} } },
                    fonts = { body = "fonts/inter.ttf" },
                }
            end
        "#;
        let manifest = run_staged_mod_init_luau(
            source,
            &dir.join("start-script.luau").to_string_lossy(),
            &dir,
            &LuauConfig::default(),
            None,
        )
        .expect("hot-reload Luau parser must drain UI fields");
        assert_eq!(manifest.ui_trees.len(), 1);
        assert_eq!(manifest.ui_trees[0].name, "hud");
        assert!(manifest.ui_trees[0].always_on);
        assert_eq!(manifest.theme.colors["critical"], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(manifest.fonts.families["body"], "fonts/inter.ttf");
    }

    /// Hot-reload Luau parser skips a malformed `uiTrees` entry.
    #[test]
    fn run_staged_mod_init_luau_skips_malformed_ui_tree() {
        let dir = temp_mod_root("staged_luau_ui_bad");
        let source = r#"
            function setupMod()
                return {
                    name = "UiMod",
                    uiTrees = {
                        { name = "bad", tree = { anchor = "topLeft", offset = { 0, 0 }, root = { kind = "carousel" } } },
                        { name = "good", tree = { anchor = "topLeft", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } },
                    },
                }
            end
        "#;
        let manifest = run_staged_mod_init_luau(
            source,
            &dir.join("start-script.luau").to_string_lossy(),
            &dir,
            &LuauConfig::default(),
            None,
        )
        .expect("malformed UI tree must not abort the hot-reload parse");
        assert_eq!(manifest.ui_trees.len(), 1);
        assert_eq!(manifest.ui_trees[0].name, "good");
    }
}
