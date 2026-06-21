// Mod-init orchestration: dispatches the start-script to the right VM and
// maintains the hot-reload dependency set. VM execution lives in `mod_init_exec`.
// See: context/lib/scripting.md §2 (Mod-init context lifecycle)

use std::fs;
use std::path::Path;

use crate::scripting::error::ScriptError;
#[cfg(debug_assertions)]
use crate::scripting::staged_manifest::{
    StagedManifestBuildConfig, StagedManifestBuildResult, StagedManifestBuildStatus,
    StagedManifestDiagnosticSeverity,
};

#[cfg(debug_assertions)]
use super::compile::compile_start_script;
use super::mod_init_exec::{run_mod_init_luau, run_mod_init_quickjs};
#[cfg(debug_assertions)]
use super::types::ActiveModInitDependencies;
use super::types::ScriptRuntime;

impl ScriptRuntime {
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
    /// - `.js` default manifest export is missing or not an object
    /// - `.luau` returned manifest is missing or not a table
    /// - top-level manifest initialization throws
    /// - the manifest object/table is missing the required `name` field
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
                if let Err(e) = compile_start_script(&ts_path, &js_path) {
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

        let store_plan = self
            .script_ctx
            .slot_table
            .borrow()
            .plan_reconcile(&manifest.store_declarations)
            .map_err(|error| ScriptError::InvalidArgument {
                reason: format!("mod-init: state-store declarations rejected: {error}"),
            })?;
        self.script_ctx
            .slot_table
            .borrow_mut()
            .apply_reconcile_plan(store_plan);

        log::info!("[Mod-init] mod `{}` initialized", manifest.name);
        self.mod_manifest = Some(manifest);
        Ok(())
    }

    #[cfg(debug_assertions)]
    pub(super) fn seed_active_mod_init_dependencies(&mut self, mod_root: &Path) {
        let result = crate::scripting::staged_manifest::build_staged_manifest(
            mod_root,
            0,
            &StagedManifestBuildConfig {
                quickjs: self.cfg.quickjs,
                luau: self.cfg.luau,
            },
        );
        self.log_staged_manifest_diagnostics(&result);
        self.install_active_dependencies_from_staged_result(&result);
    }

    #[cfg(debug_assertions)]
    pub(super) fn install_active_dependencies_from_staged_result(
        &mut self,
        result: &StagedManifestBuildResult,
    ) {
        match &result.status {
            StagedManifestBuildStatus::Built(manifest) => {
                match ActiveModInitDependencies::from_dependencies(
                    &result.mod_root,
                    manifest.dependency_paths.iter(),
                ) {
                    Ok(dependencies) => {
                        log::debug!(
                            "[Scripting] active mod-init dependency set now has {} paths",
                            dependencies.len(),
                        );
                        self.active_mod_init_dependencies = Some(dependencies);
                    }
                    Err(err) => {
                        log::error!(
                            "[Scripting] staged mod-init dependencies rejected; keeping previous dependency set: {err}"
                        );
                    }
                }
            }
            StagedManifestBuildStatus::NoStartScript => {
                match ActiveModInitDependencies::no_start_script(&result.mod_root) {
                    Ok(dependencies) => {
                        log::debug!(
                            "[Scripting] active mod-init dependency set is absent-start-script candidates"
                        );
                        self.active_mod_init_dependencies = Some(dependencies);
                    }
                    Err(err) => {
                        log::error!(
                            "[Scripting] failed to install absent-start-script dependency candidates: {err}"
                        );
                    }
                }
            }
            StagedManifestBuildStatus::Failed => {
                log::error!(
                    "[Scripting] initial staged mod-init build failed; hot reload is disabled. Script saves will not take effect until the build error above is fixed and the engine is restarted."
                );
            }
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn log_staged_manifest_diagnostics(&self, result: &StagedManifestBuildResult) {
        for diagnostic in &result.diagnostics {
            match diagnostic.severity {
                StagedManifestDiagnosticSeverity::Info => log::debug!(
                    "[Scripting] staged mod-init generation {} diagnostic: {:?}: {}",
                    result.generation,
                    diagnostic.severity,
                    diagnostic.message,
                ),
                StagedManifestDiagnosticSeverity::Error => log::error!(
                    "[Scripting] staged mod-init generation {} diagnostic: {:?}: {}",
                    result.generation,
                    diagnostic.severity,
                    diagnostic.message,
                ),
            }
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn changed_paths_affect_active_mod_init_manifest(
        &self,
        paths: &[std::path::PathBuf],
    ) -> bool {
        let Some(dependencies) = self.active_mod_init_dependencies.as_ref() else {
            for path in paths {
                log::debug!(
                    "[Scripting] changed path `{}` ignored: no active mod-init dependency set",
                    path.display(),
                );
            }
            return false;
        };
        dependencies.changed_paths_affect_mod_init(paths)
    }
}

