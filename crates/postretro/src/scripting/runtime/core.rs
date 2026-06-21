// `ScriptRuntime` construction, hot-reload wiring, and staged-manifest commit.
// See: context/lib/scripting.md

use std::path::Path;

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::error::ScriptError;
use crate::scripting::luau::LuauSubsystem;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::quickjs::QuickJsSubsystem;
use crate::scripting::sequence::SequencedPrimitiveRegistry;
#[cfg(debug_assertions)]
use crate::scripting::slot_table::StoreDeclarationSet;
use crate::scripting::staged_manifest::StagedManifestBuildResult;
#[cfg(debug_assertions)]
use crate::scripting::staged_manifest::{
    StagedManifestBuildConfig, StagedManifestBuildLane, StagedManifestBuildStatus,
};

#[cfg(debug_assertions)]
use super::data_script::follow_pawn_health_range_after_refresh;
#[cfg(debug_assertions)]
use super::types::ActiveModInitDependencies;
use super::types::ModManifestResult;
use super::types::{
    ReloadSummary, ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome,
};

impl ScriptRuntime {
    /// Construction is side-effect-free with respect to the working tree.
    ///
    /// The debug-build SDK type regeneration (`emit_sdk_types_in_debug`) was
    /// pulled out of this constructor and into the engine startup path so it
    /// runs exactly once. Constructing a runtime no longer writes
    /// `sdk/types/postretro.d.{ts,luau}`: every test that builds a runtime was
    /// otherwise racing the committed-types reader test, which intermittently
    /// observed a truncated file mid-write. The dev convenience lives at the
    /// real startup site; the `gen-script-types` bin remains the explicit
    /// regeneration entry point. See: context/lib/scripting.md §7.
    pub(crate) fn new(
        registry: &PrimitiveRegistry,
        cfg: &ScriptRuntimeConfig,
        ctx: &ScriptCtx,
    ) -> Result<Self, ScriptError> {
        let quickjs = QuickJsSubsystem::new(registry, &cfg.quickjs)?;
        let luau = LuauSubsystem::new(registry, &cfg.luau)?;

        Ok(Self {
            quickjs,
            luau,
            mod_manifest: None,
            #[cfg(debug_assertions)]
            watcher: None,
            #[cfg(debug_assertions)]
            staged_manifest_lane: None,
            #[cfg(debug_assertions)]
            active_mod_init_dependencies: None,
            script_ctx: ctx.clone(),
            cfg: *cfg,
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
            self.seed_active_mod_init_dependencies(mod_root);
            let ts_compiler = crate::scripting::watcher::TsCompilerPath::detect();
            if let Some(ref c) = ts_compiler {
                c.warn_if_stale();
            }
            let w = crate::scripting::watcher::ScriptWatcher::spawn(
                script_root,
                mod_root,
                ts_compiler,
            )?;
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
                let requests = w.drain_reload_requests()?;
                let mut mod_init = false;
                for request in &requests {
                    mod_init |= self.changed_paths_affect_active_mod_init_manifest(&request.paths);
                }
                return Ok(ReloadSummary { mod_init });
            }
        }
        Ok(ReloadSummary::default())
    }

    /// Queue a staged mod-init manifest build on the serialized debug worker
    /// lane. Release builds keep hot reload unavailable and return `None`.
    pub(crate) fn enqueue_staged_manifest_build(
        &mut self,
        mod_root: &Path,
    ) -> Result<Option<u64>, ScriptError> {
        #[cfg(debug_assertions)]
        {
            let lane = self
                .staged_manifest_lane
                .get_or_insert_with(StagedManifestBuildLane::new);
            let generation = lane.enqueue(
                mod_root.to_path_buf(),
                StagedManifestBuildConfig {
                    quickjs: self.cfg.quickjs,
                    luau: self.cfg.luau,
                },
            )?;
            Ok(Some(generation))
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = mod_root;
            Ok(None)
        }
    }

    /// Poll completed staged manifest jobs without blocking. Release builds
    /// return an empty list.
    pub(crate) fn poll_staged_manifest_builds(&mut self) -> Vec<StagedManifestBuildResult> {
        #[cfg(debug_assertions)]
        {
            if let Some(lane) = self.staged_manifest_lane.as_mut() {
                return lane.poll_completed();
            }
        }
        Vec::new()
    }

    pub(crate) fn latest_staged_manifest_generation(&self) -> Option<u64> {
        #[cfg(debug_assertions)]
        {
            self.staged_manifest_lane
                .as_ref()
                .map(|lane| lane.latest_requested_generation())
        }
        #[cfg(not(debug_assertions))]
        {
            None
        }
    }

    /// Commit a completed staged manifest result on the main thread.
    ///
    /// Latest successful results replace the descriptor registry snapshot,
    /// update the active dependency classifier, and apply the precomputed live
    /// refresh plan while the entity registry is mutably owned. Stale or
    /// failed results preserve the previous committed snapshot.
    pub(crate) fn commit_staged_manifest_result(
        &mut self,
        result: &StagedManifestBuildResult,
        ctx: &ScriptCtx,
        sequence_registry: &SequencedPrimitiveRegistry,
    ) -> StagedManifestCommitOutcome {
        #[cfg(debug_assertions)]
        {
            let latest = self.latest_staged_manifest_generation();
            if latest != Some(result.generation) {
                log::info!(
                    "[Scripting] discarded stale staged mod-init generation {} (latest {:?})",
                    result.generation,
                    latest,
                );
                return StagedManifestCommitOutcome::DiscardedStale {
                    generation: result.generation,
                    latest_requested: latest,
                };
            }

            self.log_staged_manifest_diagnostics(result);

            let (
                next_descriptors,
                next_maps,
                next_global_reactions,
                next_global_crossings,
                next_store_declarations,
                next_dependencies,
                descriptor_label,
            ) = match &result.status {
                StagedManifestBuildStatus::Built(manifest) => {
                    let dependencies = match ActiveModInitDependencies::from_dependencies(
                        &result.mod_root,
                        manifest.dependency_paths.iter(),
                    ) {
                        Ok(dependencies) => dependencies,
                        Err(err) => {
                            log::error!(
                                "[Scripting] staged mod-init generation {} rejected before commit: {err}",
                                result.generation,
                            );
                            return StagedManifestCommitOutcome::Rejected {
                                generation: result.generation,
                                reason: err,
                            };
                        }
                    };
                    (
                        manifest.entities.clone(),
                        manifest.maps.clone(),
                        manifest.reactions.clone(),
                        manifest.crossings.clone(),
                        manifest.store_declarations.clone(),
                        dependencies,
                        format!("mod `{}`", manifest.name),
                    )
                }
                StagedManifestBuildStatus::NoStartScript => {
                    let dependencies = match ActiveModInitDependencies::no_start_script(
                        &result.mod_root,
                    ) {
                        Ok(dependencies) => dependencies,
                        Err(err) => {
                            log::error!(
                                "[Scripting] staged mod-init generation {} rejected before commit: {err}",
                                result.generation,
                            );
                            return StagedManifestCommitOutcome::Rejected {
                                generation: result.generation,
                                reason: err,
                            };
                        }
                    };
                    (
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        StoreDeclarationSet::default(),
                        dependencies,
                        "debug no-start-script state".to_string(),
                    )
                }
                StagedManifestBuildStatus::Failed => {
                    log::error!(
                        "[Scripting] staged mod-init generation {} failed; keeping current descriptor registry",
                        result.generation,
                    );
                    return StagedManifestCommitOutcome::FailedBuild {
                        generation: result.generation,
                    };
                }
            };

            // Dedup once up front (last-write-wins, matching startup's upsert)
            // so the warning fires a single time and both the refresh plan and
            // the registry replace observe the same deduped snapshot.
            let next_descriptors =
                crate::scripting::data_registry::DataRegistry::dedup_entity_type_snapshot(
                    next_descriptors,
                );
            let next_global_reactions =
                crate::scripting::reaction_dispatch::validate_scoped_sequence_primitives(
                    next_global_reactions,
                    sequence_registry,
                );
            let store_plan = match ctx
                .slot_table
                .borrow()
                .plan_reconcile(&next_store_declarations)
            {
                Ok(plan) => plan,
                Err(error) => {
                    let reason = format!("state-store declarations rejected: {error}");
                    log::error!(
                        "[Scripting] staged mod-init generation {} rejected before commit: {reason}",
                        result.generation,
                    );
                    return StagedManifestCommitOutcome::Rejected {
                        generation: result.generation,
                        reason,
                    };
                }
            };

            let old_descriptors = ctx.data_registry.borrow().entities.clone();
            let refresh_plan = {
                let registry = ctx.registry.borrow();
                crate::scripting::refresh_plan::plan_descriptor_refresh(
                    &old_descriptors,
                    &next_descriptors,
                    &registry,
                )
            };
            for diagnostic in &refresh_plan.diagnostics {
                log::debug!(
                    "[Scripting] descriptor refresh diagnostic for entity {} `{}`: {}",
                    diagnostic.entity,
                    diagnostic.descriptor,
                    diagnostic.message,
                );
            }

            let apply_summary = {
                let mut registry = ctx.registry.borrow_mut();
                match crate::scripting::refresh_plan::apply_descriptor_refresh_plan(
                    &refresh_plan,
                    &mut registry,
                ) {
                    Ok(summary) => summary,
                    Err(err) => {
                        let reason = err.to_string();
                        log::error!(
                            "[Scripting] staged mod-init generation {} refresh apply failed; keeping descriptor registry and dependency set active: {reason}",
                            result.generation,
                        );
                        return StagedManifestCommitOutcome::Rejected {
                            generation: result.generation,
                            reason,
                        };
                    }
                }
            };

            ctx.slot_table.borrow_mut().apply_reconcile_plan(store_plan);

            // Hot-reload range-follow: if the refresh replaced the pawn's Health
            // component (e.g. an authored `max` edit), re-attach the
            // `player.health` slot range `[0, max]` from the now-applied
            // component. Idempotent — re-set unconditionally on any pawn-health
            // replace, no `max`-delta detection. The registry borrow_mut from
            // the apply step above has already dropped; this re-borrows the
            // registry (read) and the slot table (separate `RefCell`, write).
            follow_pawn_health_range_after_refresh(
                &refresh_plan,
                &ctx.registry.borrow(),
                &mut ctx.slot_table.borrow_mut(),
            );

            ctx.data_registry
                .borrow_mut()
                .replace_entity_types(next_descriptors);
            ctx.data_registry.borrow_mut().replace_maps(next_maps);
            ctx.data_registry
                .borrow_mut()
                .replace_global_reactions(next_global_reactions);
            ctx.data_registry
                .borrow_mut()
                .replace_global_crossings(next_global_crossings);
            let dependency_count = next_dependencies.len();
            self.active_mod_init_dependencies = Some(next_dependencies);
            log::info!(
                "[Scripting] committed staged mod-init generation {} for {descriptor_label}: {} descriptor(s), {} refresh action(s), {} dropped missing target(s), {} dependency candidate(s)",
                result.generation,
                ctx.data_registry.borrow().entities.len(),
                apply_summary.applied_actions,
                apply_summary.dropped_missing_targets,
                dependency_count,
            );
            return StagedManifestCommitOutcome::Committed {
                generation: result.generation,
                descriptor_count: ctx.data_registry.borrow().entities.len(),
                applied_actions: apply_summary.applied_actions,
                dropped_missing_targets: apply_summary.dropped_missing_targets,
            };
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = result;
            let _ = ctx;
            let _ = sequence_registry;
            StagedManifestCommitOutcome::ReleaseNoop
        }
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
    /// `compile_start_script` for top-level mod-root entries (unconditional
    /// rebuild — they are bundle components) and `compile_one_if_stale` for
    /// nested `script_root` files (per-file mtime check — they compile to
    /// individual `.js` outputs). Same compiler detection cascade, same
    /// error-logging strategy (warn and continue rather than hard-fail). A
    /// missing `scripts-build` is logged once and the scan returns without
    /// compiling.
    pub(crate) fn compile_stale_scripts(&self, script_root: &Path, mod_root: &Path) {
        #[cfg(debug_assertions)]
        {
            super::compile::scan_and_compile_stale_ts(script_root, mod_root);
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
}
