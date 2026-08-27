// `ScriptRuntime` construction, hot-reload wiring, and staged-manifest commit.
// See: context/lib/scripting.md

use std::path::Path;

use crate::ctx::ScriptCtx;
use crate::error::ScriptError;
use crate::luau::LuauSubsystem;
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::QuickJsSubsystem;
use crate::sequence::SequencedPrimitiveRegistry;
#[cfg(debug_assertions)]
use crate::slot_table::StoreDeclarationSet;
use crate::staged_manifest::StagedManifestBuildResult;
#[cfg(debug_assertions)]
use crate::staged_manifest::{
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

/// Mesh and weapon-presentation descriptors are installed with the level: their
/// models are pre-uploaded, spawner caches retain resolved descriptors, and live
/// components carry transient animation and attachment bindings. Staged hot reload
/// cannot update those consumers atomically, so preserve presentation paths until
/// reload while still applying ordinary weapon tuning changes immediately.
#[cfg(debug_assertions)]
fn defer_visual_asset_descriptor_refreshes(
    old_descriptors: &[crate::data_descriptors::EntityTypeDescriptor],
    next_descriptors: &mut Vec<crate::data_descriptors::EntityTypeDescriptor>,
) -> bool {
    let mut deferred = false;
    for next in next_descriptors.iter_mut() {
        let Some(name) = next.canonical_name.as_deref() else {
            continue;
        };
        let old_descriptor = old_descriptors
            .iter()
            .find(|old| old.canonical_name.as_deref() == Some(name));
        let old_mesh = old_descriptor.and_then(|old| old.mesh.clone());
        if next.mesh == old_mesh {
            // Keep checking the weapon presentation fields below.
        } else {
            deferred = true;

            log::warn!(
                "[Scripting] staged descriptor refresh deferred components.mesh for `{name}`; active mesh bindings and model uploads remain in use until the next level load"
            );
            next.mesh = old_mesh;
        }

        let old_weapon = old_descriptor.and_then(|old| old.weapon.as_ref());
        let old_projectile_assets = projectile_presentation_asset_paths(old_weapon);
        let next_projectile_assets = projectile_presentation_asset_paths(next.weapon.as_ref());
        if next_projectile_assets != old_projectile_assets {
            deferred = true;
            log::warn!(
                "[Scripting] staged descriptor refresh deferred components.weapon projectile presentation assets for `{name}`; active renderer uploads remain in use until the next level load"
            );

            if let Some(old_weapon) = old_weapon {
                if let Some(next_weapon) = next.weapon.as_mut() {
                    match (
                        old_weapon.projectile.as_ref(),
                        next_weapon.projectile.as_mut(),
                    ) {
                        (Some(old_projectile), Some(next_projectile))
                            if old_weapon.resolution == next_weapon.resolution =>
                        {
                            next_projectile.visual = old_projectile.visual.clone();
                        }
                        _ => {
                            next_weapon.resolution = old_weapon.resolution;
                            next_weapon.projectile = old_weapon.projectile.clone();
                        }
                    }
                } else {
                    next.weapon = Some(old_weapon.clone());
                }
            } else if let Some(next_weapon) = next.weapon.as_mut() {
                next_weapon.resolution = crate::data_descriptors::ResolutionMode::Hitscan;
                next_weapon.projectile = None;
            }
        }
        let presentation_paths = |weapon: Option<&crate::data_descriptors::WeaponDescriptor>| {
            weapon.and_then(|weapon| {
                (weapon.third_person_model.is_some() || weapon.viewmodel.is_some())
                    .then(|| (weapon.third_person_model.clone(), weapon.viewmodel.clone()))
            })
        };
        let old_presentation = presentation_paths(old_weapon);
        let next_presentation = presentation_paths(next.weapon.as_ref());
        if next_presentation == old_presentation {
            continue;
        }

        deferred = true;
        log::warn!(
            "[Scripting] staged descriptor refresh deferred components.weapon presentation models for `{name}`; active model uploads and socket bindings remain in use until the next level load"
        );
        if let Some(next_weapon) = next.weapon.as_mut() {
            if let Some(old_weapon) = old_weapon {
                next_weapon.third_person_model = old_weapon.third_person_model.clone();
                next_weapon.viewmodel = old_weapon.viewmodel.clone();
            } else {
                next_weapon.third_person_model = None;
                next_weapon.viewmodel = None;
            }
        } else if let Some(old_weapon) = old_weapon {
            // Removing a weapon with live presentation models would leave its
            // holder attachment stale. Keep the full descriptor until reload.
            next.weapon = Some(old_weapon.clone());
        }
    }

    // Descriptor-backed network presentation and runtime spawners do not all
    // carry enough provenance to prove whether a removed descriptor has a live
    // holder. Treat the installed descriptor/resource snapshot as the lifetime
    // boundary: a completely removed presentation-bearing descriptor remains
    // addressable for this level even when no holder is currently observed.
    // Pure tuning descriptors have no such dependency and delete immediately.
    for old in old_descriptors {
        let Some(name) = old.canonical_name.as_deref() else {
            continue;
        };
        if next_descriptors
            .iter()
            .any(|next| next.canonical_name.as_deref() == Some(name))
        {
            continue;
        }
        let weapon_presentation_installed = old.weapon.as_ref().is_some_and(|weapon| {
            weapon.third_person_model.is_some()
                || weapon.viewmodel.is_some()
                || weapon.projectile.is_some()
        });
        if old.mesh.is_none() && !weapon_presentation_installed {
            continue;
        }

        deferred = true;
        log::warn!(
            "[Scripting] staged descriptor deletion deferred for presentation-bearing `{name}`; active model uploads and bindings remain addressable until the next level load"
        );
        next_descriptors.push(old.clone());
    }
    deferred
}

#[cfg(debug_assertions)]
fn projectile_presentation_asset_paths(
    weapon: Option<&crate::data_descriptors::WeaponDescriptor>,
) -> Option<(String, Option<String>)> {
    let projectile = weapon.and_then(|weapon| weapon.projectile.as_ref())?;
    let body = match &projectile.visual.body {
        crate::data_descriptors::ProjectileBodyVisual::Sprite { sprite, .. } => {
            format!("sprite:{sprite}")
        }
        crate::data_descriptors::ProjectileBodyVisual::Model { model } => {
            format!("model:{model}")
        }
    };
    let trail = projectile
        .visual
        .trail
        .as_ref()
        .map(|trail| trail.sprite.clone());
    Some((body, trail))
}

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
    pub fn new(
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
            store_identity: None,
            committed_store_slots: Default::default(),
            committed_mod_identity: None,
            #[cfg(debug_assertions)]
            watcher: None,
            #[cfg(debug_assertions)]
            staged_manifest_lane: None,
            #[cfg(debug_assertions)]
            active_mod_init_dependencies: None,
            #[cfg(debug_assertions)]
            deferred_mesh_descriptors: None,
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
    pub fn start_watcher(
        &mut self,
        script_root: &Path,
        mod_root: &Path,
    ) -> Result<(), ScriptError> {
        #[cfg(debug_assertions)]
        {
            self.seed_active_mod_init_dependencies(mod_root);
            let ts_compiler = crate::watcher::TsCompilerPath::detect();
            if let Some(ref c) = ts_compiler {
                c.warn_if_stale();
            }
            let w = crate::watcher::ScriptWatcher::spawn(script_root, mod_root, ts_compiler)?;
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
    pub fn drain_reload_requests(&mut self) -> Result<ReloadSummary, ScriptError> {
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
    pub fn enqueue_staged_manifest_build(
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
    pub fn poll_staged_manifest_builds(&mut self) -> Vec<StagedManifestBuildResult> {
        #[cfg(debug_assertions)]
        {
            if let Some(lane) = self.staged_manifest_lane.as_mut() {
                return lane.poll_completed();
            }
        }
        Vec::new()
    }

    pub fn latest_staged_manifest_generation(&self) -> Option<u64> {
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

    /// Promote presentation descriptor changes deferred by a staged hot reload
    /// before the next level's model, attachment, and projectile-visual preload
    /// sweeps read descriptors. Live components intentionally keep their existing
    /// bindings until then.
    pub fn install_deferred_mesh_descriptors(&mut self, ctx: &ScriptCtx) -> bool {
        #[cfg(debug_assertions)]
        {
            let Some(descriptors) = self.deferred_mesh_descriptors.take() else {
                return false;
            };
            ctx.data_registry
                .borrow_mut()
                .replace_entity_types(descriptors);
            true
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = ctx;
            false
        }
    }

    /// Commit a completed staged manifest result on the main thread.
    ///
    /// Latest successful results replace the descriptor registry snapshot,
    /// update the active dependency classifier, and apply the precomputed live
    /// refresh plan while the entity registry is mutably owned. Stale or
    /// failed results preserve the previous committed snapshot.
    pub fn commit_staged_manifest_result(
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
                next_default_weapon_placement,
                next_global_reactions,
                next_global_crossings,
                next_global_trigger_events,
                next_global_trigger_pools,
                next_store_declarations,
                next_dependencies,
                next_mod_identity,
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
                        manifest.default_weapon_placement.clone(),
                        manifest.reactions.clone(),
                        manifest.crossings.clone(),
                        manifest.trigger_events.clone(),
                        manifest.trigger_pools.clone(),
                        manifest.store_declarations.clone(),
                        dependencies,
                        Some((manifest.id.clone(), manifest.version.clone())),
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
                        None,
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        StoreDeclarationSet::default(),
                        dependencies,
                        None,
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
            let mut next_descriptors =
                crate::data_registry::DataRegistry::dedup_entity_type_snapshot(next_descriptors);
            let next_global_reactions =
                crate::reaction_dispatch::validate_scoped_sequence_primitives(
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
            let next_committed_store_slots =
                crate::store_identity::declaration_slot_names(&next_store_declarations);
            // Keep identity in the same all-or-nothing commit gate as slot
            // reconciliation. In particular this runs before deferred mesh
            // snapshots, entity refresh, and every live registry mutation.
            let next_store_identity = if self.cfg.skip_identity_enforcement {
                None
            } else {
                match crate::store_identity::read_and_validate_attempt(
                    &result.mod_root,
                    &next_store_declarations,
                    &ctx.slot_table.borrow(),
                    self.store_identity.as_ref(),
                ) {
                    Ok(validated) => {
                        for warning in &validated.warnings {
                            log::warn!("[Scripting] {warning}");
                        }
                        Some(validated)
                    }
                    Err(error) => {
                        let reason = format!("state-store identity rejected: {error}");
                        log::error!(
                            "[Scripting] staged mod-init generation {} rejected before commit: {reason}",
                            result.generation,
                        );
                        return StagedManifestCommitOutcome::Rejected {
                            generation: result.generation,
                            reason,
                        };
                    }
                }
            };

            let old_descriptors = ctx.data_registry.borrow().entities.clone();
            let incoming_descriptors = next_descriptors.clone();
            if defer_visual_asset_descriptor_refreshes(&old_descriptors, &mut next_descriptors) {
                // Preserve the unmodified, latest snapshot. A subsequent staged
                // reload replaces it, so the next level sees the final authored
                // presentation asset additions, removals, and path changes.
                self.deferred_mesh_descriptors = Some(incoming_descriptors);
            } else {
                // A later reload can revert every previously deferred presentation
                // change. Its snapshot is now safe for the active level, so do
                // not let an older deferred snapshot override it at install.
                self.deferred_mesh_descriptors = None;
            }
            let refresh_plan = {
                let registry = ctx.registry.borrow();
                crate::refresh_plan::plan_descriptor_refresh(
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
                match crate::refresh_plan::apply_descriptor_refresh_plan(
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
            if self.cfg.skip_identity_enforcement {
                self.store_identity = None;
            } else if let Some(validated) = next_store_identity {
                // A successful attempt that reads no ledger explicitly
                // discards the previous snapshot; persistence must not retain
                // an old durable mapping for live-but-undeclared slots.
                self.store_identity = validated.ledger;
            }
            self.committed_store_slots = next_committed_store_slots;

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

            // One mutable registry borrow commits the staged whole snapshot, so
            // a render frame cannot observe new descriptors without their
            // matching mod-global weapon-placement default.
            {
                let mut data_registry = ctx.data_registry.borrow_mut();
                data_registry.replace_entity_types(next_descriptors);
                data_registry.replace_maps(next_maps);
                data_registry.set_default_weapon_placement(next_default_weapon_placement);
                data_registry.replace_global_reactions(next_global_reactions);
                data_registry.replace_global_crossings(next_global_crossings);
                data_registry.replace_global_trigger_events(next_global_trigger_events);
                data_registry.replace_global_trigger_pools(next_global_trigger_pools);
            }
            let dependency_count = next_dependencies.len();
            self.active_mod_init_dependencies = Some(next_dependencies);

            if let Some((id, version)) = next_mod_identity {
                // Manifest lanes normally atomically replace on a staged commit.
                // Identity joins `fonts` as the existing non-re-committed minority:
                // admission is terminal, so changing it would invalidate live
                // decisions with no recovery path. The compatibility digest is the
                // opposite case and must re-hash on each commit because parity can
                // demote and later re-promote a connection.
                match self.committed_mod_identity.as_ref() {
                    None => self.committed_mod_identity = Some((id, version)),
                    Some((committed_id, committed_version))
                        if committed_id != &id || committed_version != &version =>
                    {
                        log::warn!("[Scripting] mod identity is frozen");
                    }
                    Some(_) => {}
                }
            }
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
    pub fn compile_stale_scripts(&self, script_root: &Path, mod_root: &Path) {
        #[cfg(debug_assertions)]
        {
            super::compile::scan_and_compile_stale_ts(script_root, mod_root);
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = (script_root, mod_root);
        }
    }

    pub fn quickjs(&self) -> &QuickJsSubsystem {
        &self.quickjs
    }

    pub fn luau(&self) -> &LuauSubsystem {
        &self.luau
    }

    /// Returns the validated manifest captured by the most recent successful
    /// [`ScriptRuntime::run_mod_init`] call. `None` until then, and may also
    /// remain `None` in debug builds when no start-script was found.
    pub fn mod_manifest(&self) -> Option<&ModManifestResult> {
        self.mod_manifest.as_ref()
    }

    /// Returns the validated identity-ledger snapshot from the latest
    /// successful declaration commit. This is intentionally the only runtime
    /// path consumers use; `identity.json` itself is never re-read after commit.
    pub fn store_identity(&self) -> Option<&crate::store_identity::StoreIdentityLedger> {
        self.store_identity.as_ref()
    }

    /// Returns authored dotted slot names from the latest successful
    /// declaration commit. Consumers use this snapshot to filter add-only live
    /// slots that are no longer declared by the current mod content.
    pub fn committed_store_slots(&self) -> &std::collections::BTreeSet<String> {
        &self.committed_store_slots
    }

    /// Mutable accessor for the stored manifest. Used by the boot caller to
    /// drain `entities` into `DataRegistry` after a successful
    /// [`ScriptRuntime::run_mod_init`] — the runtime parses and returns; the
    /// caller owns registry lifecycle. See: context/lib/boot_sequence.md §3.
    pub fn mod_manifest_mut(&mut self) -> Option<&mut ModManifestResult> {
        self.mod_manifest.as_mut()
    }

    /// Returns the id and version from the first committed manifest. Identity
    /// stays frozen across staged reloads and resume-time mod-init reruns.
    pub fn committed_mod_identity(&self) -> Option<(&str, &str)> {
        self.committed_mod_identity
            .as_ref()
            .map(|(id, version)| (id.as_str(), version.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use log::Level;
    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::components::health::HealthComponent;
    use crate::data_descriptors::{
        EntityTypeDescriptor, FireMode, HealthDescriptor, InventoryDescriptor, MeshDescriptor,
        ProjectileBodyVisual, ProjectileDescriptor, ProjectileTrailVisual, ProjectileVisual,
        ResolutionMode, WeaponDescriptor,
    };
    use crate::provenance::{DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath};
    use crate::registry::{ComponentKind, Transform};
    use crate::slot_table::SlotValue;
    use crate::staged_manifest::{StagedManifestBuildConfig, build_staged_manifest};

    fn temp_mod_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "postretro_runtime_core_test_{}_{}_{name}",
            std::process::id(),
            sequence,
        ));
        fs::create_dir_all(&path).expect("temporary mod root should be created");
        path
    }

    #[test]
    fn default_runtime_config_enforces_store_identity() {
        assert!(
            !ScriptRuntimeConfig::default().skip_identity_enforcement,
            "the shipping runtime must not silently bypass durable identity enforcement"
        );
    }

    fn write_durable_store_manifest(mod_root: &PathBuf, namespace: &str) {
        fs::write(
            mod_root.join("start-script.js"),
            format!(
                r#"
                    const modStore = defineStore("{namespace}", {{
                        score: {{ type: "number", default: 1, persist: true }},
                    }});
                    globalThis.__postretroModManifest = defineMod({{
                        name: "Identity Gate",
                        id: "identity-gate",
                        version: "1",
                        entities: [{{
                            canonicalName: "identity_guard",
                            components: {{ health: {{ max: 10 }} }},
                        }}],
                        stores: [modStore],
                    }});
                "#,
            ),
        )
        .unwrap();
    }

    fn write_empty_mod_manifest(mod_root: &PathBuf) {
        fs::write(
            mod_root.join("start-script.js"),
            r#"
                globalThis.__postretroModManifest = {
                    name: "Identity Gate",
                    id: "identity-gate",
                    version: "1",
                };
            "#,
        )
        .unwrap();
    }

    // Regression: an absent start script cleared durable identity along with
    // current declaration membership, losing changed-key protection.
    #[test]
    fn debug_no_start_resume_clears_membership_but_retains_valid_ledger() {
        let mod_root = temp_mod_root("no_start_resume_identity");
        write_durable_store_manifest(&mod_root, "story");
        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"story.score":"k0123456789abcdef"}}"#,
        )
        .unwrap();
        let ctx = ScriptCtx::new();
        let primitives = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();

        runtime.run_mod_init(&mod_root).unwrap();
        assert!(runtime.store_identity().is_some());
        assert!(runtime.committed_store_slots().contains("story.score"));

        fs::remove_file(mod_root.join("start-script.js")).unwrap();
        runtime
            .run_mod_init(&mod_root)
            .expect("debug resume without a start script commits an empty snapshot");

        assert!(runtime.mod_manifest().is_none());
        assert!(runtime.committed_store_slots().is_empty());
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("story.score")),
            Some("k0123456789abcdef"),
            "a no-start commit clears membership but retains the valid ledger snapshot"
        );
        assert!(
            ctx.slot_table.borrow().get("story.score").is_some(),
            "the empty commit must not delete add-only live slots"
        );

        fs::remove_dir_all(mod_root).unwrap();
    }

    #[test]
    fn staged_identity_gate_rejects_before_live_mutation_and_retains_snapshot() {
        let mod_root = temp_mod_root("identity_gate");
        write_durable_store_manifest(&mod_root, "story");
        let ctx = ScriptCtx::new();
        let old_health = HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        };
        let mut active_descriptor = descriptor("identity_guard", None, None);
        active_descriptor.health = Some(old_health.clone());
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![active_descriptor]);
        let guarded_entity = {
            let mut registry = ctx.registry.borrow_mut();
            let entity = registry.spawn(Transform::default());
            registry
                .set_component(entity, HealthComponent::from_descriptor(&old_health))
                .unwrap();
            registry
                .set_component(
                    entity,
                    DescriptorProvenance {
                        canonical_name: "identity_guard".to_string(),
                        owned_components: BTreeSet::from([DescriptorComponentKind::Health]),
                        map_overrides: BTreeSet::new(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            entity
        };
        let primitives = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        let deferred_before_rejection = vec![descriptor(
            "deferred_identity_guard",
            Some(mesh_descriptor("models/identity_guard.gltf")),
            None,
        )];
        runtime.deferred_mesh_descriptors = Some(deferred_before_rejection.clone());
        let slot_count_before_rejection = ctx.slot_table.borrow().len();

        let missing = build_staged_manifest(&mod_root, 1, &StagedManifestBuildConfig::default());
        assert!(
            matches!(missing.status, StagedManifestBuildStatus::Built(_)),
            "identity fixture must reach the commit gate: {missing:?}"
        );
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(1));
        let outcome = runtime.commit_staged_manifest_result(
            &missing,
            &ctx,
            &SequencedPrimitiveRegistry::new(),
        );
        let StagedManifestCommitOutcome::Rejected { reason, .. } = outcome else {
            panic!("missing durable identity must reject the staged result");
        };
        assert!(reason.contains("story.score"));
        assert!(reason.contains("add `\"story.score\": \"k"));
        assert!(ctx.slot_table.borrow().get("story.score").is_none());
        assert_eq!(
            ctx.slot_table.borrow().len(),
            slot_count_before_rejection,
            "identity rejection must leave the slot table unchanged"
        );
        assert!(runtime.store_identity().is_none());
        assert_eq!(
            runtime.deferred_mesh_descriptors.as_ref(),
            Some(&deferred_before_rejection),
            "identity rejection must run before the staged path changes deferred meshes"
        );
        let registry = ctx.registry.borrow();
        assert!(registry.exists(guarded_entity));
        assert!(
            registry
                .has_component_kind(guarded_entity, ComponentKind::Health)
                .unwrap()
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(guarded_entity)
                .unwrap()
                .max,
            100.0,
            "identity rejection must run before descriptor refresh mutates live entities"
        );
        drop(registry);

        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"story.score":"k0123456789abcdef"}}"#,
        )
        .unwrap();
        let accepted = build_staged_manifest(&mod_root, 2, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(2));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &accepted,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Committed { generation: 2, .. }
        ));
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("story.score")),
            Some("k0123456789abcdef")
        );
        assert_eq!(
            runtime.committed_store_slots(),
            &BTreeSet::from(["story.score".to_string()])
        );
        ctx.slot_table
            .borrow_mut()
            .get_mut("story.score")
            .unwrap()
            .write_value(Some(SlotValue::Number(41.0)));

        // Regression: removing the declaration must not let a fresh ledger
        // mapping re-key the still-live add-only slot.
        write_empty_mod_manifest(&mod_root);
        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"story.score":"kfedcba9876543210"}}"#,
        )
        .unwrap();
        let removed = build_staged_manifest(&mod_root, 21, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(21));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &removed,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Rejected { generation: 21, .. }
        ));

        // Regression: NoStartScript is the same empty-attempt boundary and
        // cannot replace the retained snapshot with a changed mapping either.
        fs::remove_file(mod_root.join("start-script.js")).unwrap();
        let no_start = build_staged_manifest(&mod_root, 22, &StagedManifestBuildConfig::default());
        assert!(matches!(
            no_start.status,
            StagedManifestBuildStatus::NoStartScript
        ));
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(22));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &no_start,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Rejected { generation: 22, .. }
        ));
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("story.score")),
            Some("k0123456789abcdef")
        );

        write_durable_store_manifest(&mod_root, "story");

        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"story.score":"kfedcba9876543210"}}"#,
        )
        .unwrap();
        let changed = build_staged_manifest(&mod_root, 3, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(3));
        let changed_outcome = runtime.commit_staged_manifest_result(
            &changed,
            &ctx,
            &SequencedPrimitiveRegistry::new(),
        );
        let StagedManifestCommitOutcome::Rejected { reason, .. } = changed_outcome else {
            panic!("a durable-key change for a currently live slot must reject");
        };
        assert!(
            reason.contains("changes durable key for already committed state slot `story.score`")
        );
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("story.score")),
            Some("k0123456789abcdef"),
            "a rejected hand edit must not replace the retained snapshot"
        );

        write_durable_store_manifest(&mod_root, "chapter");
        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"chapter.score":"k0123456789abcdef"}}"#,
        )
        .unwrap();
        let renamed = build_staged_manifest(&mod_root, 4, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(4));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &renamed,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Committed { generation: 4, .. }
        ));
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("story.score")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(41.0)),
            "removing a declaration must keep the existing live slot value"
        );
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("chapter.score")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(1.0)),
            "a staged authored rename is a new namespace and starts at defaults"
        );
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("chapter.score")),
            Some("k0123456789abcdef")
        );
        assert!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("story.score"))
                .is_none()
        );
        assert_eq!(
            runtime.committed_store_slots(),
            &BTreeSet::from(["chapter.score".to_string()])
        );

        fs::remove_file(mod_root.join("start-script.js")).unwrap();
        fs::remove_file(mod_root.join(crate::store_identity::IDENTITY_FILE_NAME)).unwrap();
        let discarded = build_staged_manifest(&mod_root, 5, &StagedManifestBuildConfig::default());
        assert!(matches!(
            discarded.status,
            StagedManifestBuildStatus::NoStartScript
        ));
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(5));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &discarded,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Committed { generation: 5, .. }
        ));
        assert!(
            runtime.store_identity().is_none(),
            "a successful no-ledger attempt must clear the retained snapshot"
        );
        assert!(runtime.committed_store_slots().is_empty());
        assert!(
            ctx.slot_table.borrow().get("story.score").is_some(),
            "declaration removal intentionally keeps prior live slots"
        );

        write_durable_store_manifest(&mod_root, "chapter");
        let redeclared = build_staged_manifest(&mod_root, 6, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(6));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &redeclared,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Rejected { generation: 6, .. }
        ));
        assert!(runtime.store_identity().is_none());

        // The staged worker has already produced this result. Reading the
        // appended ledger only at commit proves that each attempt re-reads the
        // file instead of carrying a stale build-time view.
        let appended = build_staged_manifest(&mod_root, 7, &StagedManifestBuildConfig::default());
        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"chapter.score":"kfedcba9876543210"}}"#,
        )
        .unwrap();
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(7));
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &appended,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Committed { generation: 7, .. }
        ));
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("chapter.score")),
            Some("kfedcba9876543210"),
            "the ledger appended between staged attempts must be accepted by the later commit"
        );

        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":2,"slots":{"chapter.score":"kfedcba9876543210"}}"#,
        )
        .unwrap();
        let unsupported_version =
            build_staged_manifest(&mod_root, 8, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(8));
        let StagedManifestCommitOutcome::Rejected { reason, .. } = runtime
            .commit_staged_manifest_result(
                &unsupported_version,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            )
        else {
            panic!("an unsupported ledger version must reject the staged result");
        };
        assert!(reason.contains("identity ledger version 2 is unsupported"));

        fs::write(
            mod_root.join(crate::store_identity::IDENTITY_FILE_NAME),
            r#"{"version":1,"slots":{"chapter.score":"kfedcba9876543210","duplicate.score":"kfedcba9876543210"}}"#,
        )
        .unwrap();
        let duplicate_key =
            build_staged_manifest(&mod_root, 9, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(9));
        let StagedManifestCommitOutcome::Rejected { reason, .. } = runtime
            .commit_staged_manifest_result(
                &duplicate_key,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            )
        else {
            panic!("a duplicate durable key must reject the staged result");
        };
        assert!(reason.contains("is assigned to more than one authored slot"));
        assert_eq!(
            runtime
                .store_identity()
                .and_then(|identity| identity.durable_key("chapter.score")),
            Some("kfedcba9876543210"),
            "file-rule rejections must retain the latest successful identity snapshot"
        );

        fs::remove_dir_all(mod_root).unwrap();
    }

    fn mesh_descriptor(attachment_model: &str) -> MeshDescriptor {
        MeshDescriptor {
            model: "models/holder.gltf".to_string(),
            shadow_only: false,
            attachments: [("hand".to_string(), attachment_model.to_string())]
                .into_iter()
                .collect(),
            shadow_bias_scale: 1.0,
            animations: HashMap::new(),
            default_state: None,
            locomotion: None,
        }
    }

    fn weapon_descriptor(
        damage: f32,
        third_person_model: Option<&str>,
        viewmodel: Option<&str>,
    ) -> WeaponDescriptor {
        WeaponDescriptor {
            damage,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: third_person_model.map(str::to_string),
            viewmodel: viewmodel.map(str::to_string),
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        }
    }

    fn projectile_descriptor(
        body: ProjectileBodyVisual,
        trail_sprite: Option<&str>,
        speed: f32,
    ) -> ProjectileDescriptor {
        ProjectileDescriptor {
            speed,
            radius: 0.1,
            lifetime_ms: 1_500.0,
            visual: ProjectileVisual {
                body,
                trail: trail_sprite.map(|sprite| ProjectileTrailVisual {
                    sprite: sprite.to_string(),
                    rate: 30.0,
                    lifetime: 0.4,
                    burst: None,
                    spread: 0.0,
                    velocity: [0.0; 3],
                    buoyancy: 0.0,
                    drag: 0.0,
                    size_over_lifetime: vec![0.2, 0.0],
                    opacity_over_lifetime: vec![0.8, 0.0],
                    color: [1.0; 3],
                    spin_rate: 0.0,
                    spin_animation: None,
                }),
                light: None,
                impact_light: None,
            },
        }
    }

    fn descriptor(
        name: &str,
        mesh: Option<MeshDescriptor>,
        inventory_weapon: Option<&str>,
    ) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            inventory: inventory_weapon.map(|name| InventoryDescriptor {
                loadout: vec![name.to_string()],
            }),
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh,
            health: None,
            behavior: None,
        }
    }

    #[test]
    fn staged_refresh_defers_visual_assets_but_commits_tuning() {
        // Regression: committing a refreshed attachment descriptor while a level
        // still holds old mesh bindings left remote materialization pointing at a
        // prop model the level never uploaded.
        let mut old = vec![
            descriptor(
                "remote_enemy",
                Some(mesh_descriptor("models/old_prop.gltf")),
                Some("old_weapon"),
            ),
            descriptor(
                "removed_mesh",
                Some(mesh_descriptor("models/removed_prop.gltf")),
                None,
            ),
        ];
        old[0].weapon = Some(weapon_descriptor(
            10.0,
            Some("models/old_weapon.gltf"),
            Some("models/old_view.gltf"),
        ));
        {
            let weapon = old[0].weapon.as_mut().unwrap();
            weapon.resolution = ResolutionMode::Projectile;
            weapon.projectile = Some(projectile_descriptor(
                ProjectileBodyVisual::Sprite {
                    sprite: "sprites/old_bolt.png".to_string(),
                    size: 0.3,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                Some("sprites/old_trail.png"),
                20.0,
            ));
        }
        let mut next = vec![
            descriptor(
                "remote_enemy",
                Some(mesh_descriptor("models/new_prop.gltf")),
                Some("new_weapon"),
            ),
            descriptor("removed_mesh", None, None),
            descriptor(
                "new_remote_enemy",
                Some(mesh_descriptor("models/new_remote_prop.gltf")),
                None,
            ),
        ];

        next[0].weapon = Some(weapon_descriptor(
            25.0,
            Some("models/new_weapon.gltf"),
            Some("models/new_view.gltf"),
        ));
        {
            let weapon = next[0].weapon.as_mut().unwrap();
            weapon.resolution = ResolutionMode::Projectile;
            weapon.projectile = Some(projectile_descriptor(
                ProjectileBodyVisual::Model {
                    model: "models/new_rocket.gltf".to_string(),
                },
                Some("sprites/new_trail.png"),
                35.0,
            ));
        }
        let incoming = next.clone();
        assert!(defer_visual_asset_descriptor_refreshes(&old, &mut next));

        assert_eq!(
            next[0].mesh, old[0].mesh,
            "the active-level descriptor snapshot keeps its uploaded attachment model"
        );
        assert_eq!(
            next[0]
                .inventory
                .as_ref()
                .map(|inventory| &inventory.loadout),
            Some(&vec!["new_weapon".to_string()]),
            "unrelated descriptor refreshes remain available"
        );
        let active_weapon = next[0].weapon.as_ref().unwrap();
        assert_eq!(
            active_weapon.damage, 25.0,
            "weapon tuning still hot reloads"
        );
        assert_eq!(
            active_weapon.third_person_model.as_deref(),
            Some("models/old_weapon.gltf")
        );
        assert_eq!(
            active_weapon.viewmodel.as_deref(),
            Some("models/old_view.gltf")
        );
        let active_projectile = active_weapon.projectile.as_ref().unwrap();
        assert!((active_projectile.speed - 35.0).abs() <= f32::EPSILON);
        assert_eq!(
            active_projectile.visual,
            old[0]
                .weapon
                .as_ref()
                .unwrap()
                .projectile
                .as_ref()
                .unwrap()
                .visual,
            "active-level launches keep only renderer-preloaded projectile visual assets"
        );
        assert_ne!(
            incoming[0]
                .weapon
                .as_ref()
                .unwrap()
                .projectile
                .as_ref()
                .unwrap()
                .visual,
            active_projectile.visual,
            "the next-level snapshot retains the newly authored body and trail assets"
        );
        assert_eq!(
            incoming[0]
                .weapon
                .as_ref()
                .unwrap()
                .third_person_model
                .as_deref(),
            Some("models/new_weapon.gltf"),
            "the next-level snapshot retains new presentation paths"
        );
        assert!(
            next[1].mesh.is_some(),
            "a removed mesh descriptor remains active until the next level"
        );
        assert!(
            next[2].mesh.is_none(),
            "a newly introduced mesh descriptor also waits for a level-load upload sweep"
        );
        assert_eq!(
            incoming[0].mesh.as_ref().unwrap().attachments["hand"],
            "models/new_prop.gltf",
            "the next-level snapshot retains attachment edits"
        );
        assert!(
            incoming[1].mesh.is_none(),
            "the next-level snapshot retains removed mesh descriptors"
        );
        assert!(
            incoming[2].mesh.is_some(),
            "the next-level snapshot retains added mesh descriptors"
        );
    }

    #[test]
    fn staged_refresh_defers_complete_presentation_deletion_without_an_observed_holder() {
        // Regression: complete snapshot deletion bypassed field-level deferral,
        // dropping descriptor lookup while the current level retained its models.
        let mesh_backed = descriptor(
            "mesh_backed",
            Some(mesh_descriptor("models/prop.gltf")),
            None,
        );
        let mut weapon_backed = descriptor("weapon_backed", None, None);
        weapon_backed.weapon = Some(weapon_descriptor(
            10.0,
            Some("models/weapon.gltf"),
            Some("models/view.gltf"),
        ));
        let mut tuning_only = descriptor("tuning_only", None, None);
        tuning_only.weapon = Some(weapon_descriptor(10.0, None, None));
        let mut projectile_backed = descriptor("projectile_backed", None, None);
        let mut projectile_weapon = weapon_descriptor(10.0, None, None);
        projectile_weapon.resolution = ResolutionMode::Projectile;
        projectile_weapon.projectile = Some(projectile_descriptor(
            ProjectileBodyVisual::Model {
                model: "models/projectile.gltf".to_string(),
            },
            Some("sprites/projectile_trail.png"),
            20.0,
        ));
        projectile_backed.weapon = Some(projectile_weapon);
        let old = vec![
            mesh_backed.clone(),
            weapon_backed.clone(),
            projectile_backed.clone(),
            tuning_only,
        ];
        let mut next = Vec::new();

        assert!(defer_visual_asset_descriptor_refreshes(&old, &mut next));
        assert_eq!(
            next,
            vec![mesh_backed, weapon_backed, projectile_backed],
            "installed presentation descriptors remain addressable even when no live holder is observable; tuning-only deletion remains immediate"
        );
    }

    #[test]
    fn staged_refresh_defers_new_projectile_assets_until_level_install() {
        let old = vec![descriptor("new_projectile", None, None)];
        let mut next = old.clone();
        let mut weapon = weapon_descriptor(18.0, None, None);
        weapon.resolution = ResolutionMode::Projectile;
        weapon.projectile = Some(projectile_descriptor(
            ProjectileBodyVisual::Sprite {
                sprite: "sprites/new_bolt.png".to_string(),
                size: 0.3,
                opacity: 1.0,
                rotation: 0.0,
                tint: [1.0; 3],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            Some("sprites/new_trail.png"),
            24.0,
        ));
        next[0].weapon = Some(weapon);

        let incoming = next.clone();
        assert!(defer_visual_asset_descriptor_refreshes(&old, &mut next));
        let active_weapon = next[0].weapon.as_ref().unwrap();
        assert_eq!(active_weapon.resolution, ResolutionMode::Hitscan);
        assert!(active_weapon.projectile.is_none());
        assert!(
            incoming[0].weapon.as_ref().unwrap().projectile.is_some(),
            "the deferred next-level snapshot keeps the new projectile assets"
        );
    }

    #[test]
    fn level_install_promotes_the_latest_deferred_mesh_snapshot() {
        let ctx = ScriptCtx::new();
        let active = vec![
            descriptor(
                "remote_enemy",
                Some(mesh_descriptor("models/old_prop.gltf")),
                None,
            ),
            descriptor(
                "deleted_remote_enemy",
                Some(mesh_descriptor("models/deleted_prop.gltf")),
                None,
            ),
        ];
        let incoming = vec![
            descriptor(
                "remote_enemy",
                Some(mesh_descriptor("models/new_prop.gltf")),
                None,
            ),
            descriptor(
                "new_remote_enemy",
                Some(mesh_descriptor("models/new_remote_prop.gltf")),
                None,
            ),
        ];
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(active.clone());

        let primitives = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        runtime.deferred_mesh_descriptors = Some(incoming.clone());

        assert!(runtime.install_deferred_mesh_descriptors(&ctx));
        assert_eq!(
            ctx.data_registry.borrow().entities,
            incoming,
            "the next level sees attachment edits, additions, and complete presentation-descriptor deletion"
        );
        assert!(
            !runtime.install_deferred_mesh_descriptors(&ctx),
            "the deferred snapshot is consumed by one level install"
        );
    }

    #[test]
    fn staged_commit_replaces_trigger_pools_and_recomposes_by_level_tags() {
        let mod_root = temp_mod_root("trigger_pools");
        fs::write(
            mod_root.join("start-script.js"),
            r#"
                globalThis.__postretroModManifest = {
                    name: "ReloadedPools",
                    id: "reloaded-pools",
                    version: "1",
                    triggerPools: [
                        { tag: "every_level", arm: 1 },
                        { tag: "campaign_only", armPercentage: 50, levels: ["campaign"] },
                        { tag: "deathmatch_only", arm: 1, levels: ["deathmatch"] },
                    ],
                };
            "#,
        )
        .expect("staged mod manifest should be written");
        let result = build_staged_manifest(&mod_root, 1, &StagedManifestBuildConfig::default());
        assert!(
            matches!(result.status, StagedManifestBuildStatus::Built(_)),
            "staged manifest must build before its replacement can commit: {result:?}",
        );

        let ctx = ScriptCtx::new();
        {
            let mut data_registry = ctx.data_registry.borrow_mut();
            data_registry.replace_global_trigger_pools(vec![
                crate::data_descriptors::TriggerPoolDescriptor {
                    tag: "stale_pool".to_string(),
                    arm: crate::data_descriptors::TriggerPoolArm::Count(1),
                    levels: Vec::new(),
                },
            ]);
            data_registry.recompose_active_sets(&["campaign".to_string()]);
            assert_eq!(data_registry.trigger_pools()[0].tag, "stale_pool");
        }

        let primitive_registry = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitive_registry, &ScriptRuntimeConfig::default(), &ctx)
                .expect("script runtime should initialize");
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(1));

        let outcome = runtime.commit_staged_manifest_result(
            &result,
            &ctx,
            &SequencedPrimitiveRegistry::new(),
        );
        assert!(matches!(
            outcome,
            StagedManifestCommitOutcome::Committed { generation: 1, .. }
        ));

        let mut data_registry = ctx.data_registry.borrow_mut();
        assert_eq!(
            data_registry
                .global_trigger_pools
                .iter()
                .map(|pool| pool.tag.as_str())
                .collect::<Vec<_>>(),
            ["every_level", "campaign_only", "deathmatch_only"],
            "a staged commit replaces the prior global-pool definition snapshot",
        );

        data_registry.recompose_active_sets(&["campaign".to_string()]);
        assert_eq!(
            data_registry
                .trigger_pools()
                .iter()
                .map(|pool| pool.tag.as_str())
                .collect::<Vec<_>>(),
            ["every_level", "campaign_only"],
            "unscoped and matching pools compose; non-matching pools remain inactive",
        );

        data_registry.recompose_active_sets(&[]);
        assert_eq!(
            data_registry
                .trigger_pools()
                .iter()
                .map(|pool| pool.tag.as_str())
                .collect::<Vec<_>>(),
            ["every_level"],
            "a direct .prl path has no catalog tags, so only unscoped pools compose",
        );

        drop(data_registry);
        fs::remove_dir_all(mod_root).expect("temporary mod root should be removed");
    }

    #[test]
    fn repeated_run_mod_init_keeps_first_committed_identity() {
        // Regression: platform resume reruns mod-init, but process-scoped
        // persistence and replication identity must remain on the first id.
        let mod_root = temp_mod_root("initial_identity");
        fs::write(
            mod_root.join("start-script.js"),
            "globalThis.__postretroModManifest = { name: 'Initial', id: 'initial.mod', version: 'display build' };",
        )
        .expect("initial manifest should be written");

        let ctx = ScriptCtx::new();
        let primitives = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        runtime
            .run_mod_init(&mod_root)
            .expect("initial manifest should commit");

        assert_eq!(
            runtime.committed_mod_identity(),
            Some(("initial.mod", "display build"))
        );

        fs::write(
            mod_root.join("start-script.js"),
            "globalThis.__postretroModManifest = { name: 'Changed', id: 'changed.mod', version: '2' };",
        )
        .expect("changed manifest should be written");
        runtime
            .run_mod_init(&mod_root)
            .expect("repeated manifest should commit");

        assert_eq!(
            runtime.mod_manifest().map(|manifest| manifest.id.as_str()),
            Some("changed.mod"),
            "the latest manifest remains mutable across resume-style init",
        );
        assert_eq!(
            runtime.committed_mod_identity(),
            Some(("initial.mod", "display build")),
            "process-scoped consumers must keep using the first committed id",
        );
        fs::remove_dir_all(mod_root).expect("temporary mod root should be removed");
    }

    #[test]
    fn staged_mod_identity_is_first_commit_wins_without_an_endpoint() {
        // Regression: the freeze belongs to ScriptRuntime, not an endpoint, so
        // staged reload obeys it in ordinary single-player too.
        let mod_root = temp_mod_root("identity_first_wins");
        fs::write(
            mod_root.join("start-script.js"),
            "globalThis.__postretroModManifest = { name: 'First', id: 'first.mod', version: '1' };",
        )
        .expect("first staged manifest should be written");
        let first = build_staged_manifest(&mod_root, 1, &StagedManifestBuildConfig::default());

        let ctx = ScriptCtx::new();
        let primitives = PrimitiveRegistry::new();
        let mut runtime =
            ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(1));
        assert!(matches!(
            runtime
                .commit_staged_manifest_result(&first, &ctx, &SequencedPrimitiveRegistry::new(),),
            StagedManifestCommitOutcome::Committed { generation: 1, .. }
        ));
        assert_eq!(runtime.committed_mod_identity(), Some(("first.mod", "1")));

        fs::write(
            mod_root.join("start-script.js"),
            "globalThis.__postretroModManifest = { name: 'Second', id: 'second.mod', version: 'not semver' };",
        )
        .expect("divergent staged manifest should be written");
        let second = build_staged_manifest(&mod_root, 2, &StagedManifestBuildConfig::default());
        runtime.staged_manifest_lane = Some(StagedManifestBuildLane::new_for_test_latest(2));

        let capture = LogCapture::start();
        assert!(matches!(
            runtime.commit_staged_manifest_result(
                &second,
                &ctx,
                &SequencedPrimitiveRegistry::new(),
            ),
            StagedManifestCommitOutcome::Committed { generation: 2, .. }
        ));
        capture.assert_logged_once(Level::Warn, "[Scripting] mod identity is frozen");
        assert_eq!(runtime.committed_mod_identity(), Some(("first.mod", "1")));
        fs::remove_dir_all(mod_root).expect("temporary mod root should be removed");
    }
}
