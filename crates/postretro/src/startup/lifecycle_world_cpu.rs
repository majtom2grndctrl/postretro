//! Renderer-free CPU portion of level installation.

use super::*;

use crate::scripting::builtins::data_archetype::projectile_presentation_assets;
use crate::scripting::builtins::{
    PLAYER_START_CLASSNAME, apply_classname_dispatch, apply_data_archetype_dispatch,
    filter_out_client_host_replicated_placements, movement_descriptor_mesh_models,
    spawn_from_player_starts_with_carried_loadout, suppressed_client_host_replicated_mesh_models,
    touchable_wieldable_world_models, weapon_presentation_models,
};
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::reaction_dispatch::{
    dispatch_deferred_named_events_with_sequences, fire_named_event_with_sequences,
};

/// Attach the descriptor-authored `player.health` validation range before either
/// network role builds its replicated-state schema. The selected descriptor matches
/// `spawn_from_player_starts`: placements are visited in map order, `entity_class`
/// defaults to `"player"`, unknown/non-movement descriptors do not become the local
/// movement pawn, and the first movement descriptor is authoritative.
///
/// This deliberately resolves from shared authoring data rather than the registry.
/// Connected clients suppress their boot pawn until the host baseline arrives, but
/// must still fingerprint the same range as the listen host.
pub(crate) fn install_descriptor_player_health_range(
    slot_table: &mut postretro_entities::SlotTable,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    descriptors: &[postretro_entities::EntityTypeDescriptor],
) {
    for spawn in spawn_points {
        let entity_class = spawn
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");
        let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))
        else {
            continue;
        };
        if descriptor.movement.is_none() {
            continue;
        }
        let Some(health) = descriptor.health.as_ref() else {
            return;
        };
        if let Err(err) = slot_table.set_engine_numeric_range(
            "player.health",
            postretro_entities::NumericRange {
                min: 0.0,
                max: health.max,
            },
        ) {
            log::warn!("[Loader] failed to set player.health range: {err}");
        }
        return;
    }
}

/// Segment B of the CPU world install (renderer-free): fog-volume entities,
/// collision world + kinematic movers, classname dispatch, the data script, the
/// data-archetype sweep (incl. player-pawn spawn), the mesh sweep's CPU half
/// (hit-zone store build + clip-index resolve), and the `levelLoad` fire. The
/// sole renderer-coupled step — skinned-model upload + clip-table build — is
/// injected as `upload_mesh_models`, called between the archetype sweep and the
/// clip-index resolve: the windowed caller uploads models and fills the clip
/// tables and returns renderer ownership of model-load diagnostics; a headless
/// caller passes a no-op that returns game-side ownership, leaving clips
/// unresolved while preserving load warnings. Stage durations record into
/// `timings`, matching the windowed log-line-C labels. The caller-owned
/// `before_level_load` hook runs after player-pawn materialization and before
/// the event fire, so session state may bind a local pawn without making this
/// installer depend on the seat table.
pub(crate) fn install_world_cpu(
    handles: WorldInstallHandles<'_>,
    timings: &mut StartupTimings,
    mut upload_mesh_models: impl FnMut(
        &[String],
        &mut crate::scripting_systems::mesh_anim::MeshClipTables,
    )
        -> crate::scripting_systems::hit_zones::ModelLoadWarningOwner,
    mut before_level_load: impl FnMut(&[crate::scripting::map_entity::MapEntity]),
) -> WorldInstallProducts {
    let WorldInstallHandles {
        world,
        script_ctx,
        command_diagnostics,
        mover_auto_close_ms,
        spawn_context,
        content_root,
        active_level_tags,
        nav_graph,
        collision_world,
        fog_volume_bridge,
        trigger_volume_bridge,
        classname_dispatch,
        script_runtime,
        sequence_registry,
        reaction_registry,
        system_registry,
        modal_stack,
        progress_tracker,
        crossing_detector,
        slot_accumulator_bindings,
        impact_policy_runtime,
        mesh_clip_tables,
        hit_zone_store,
        trigger_pool_policy,
        suppress_ai_enemies,
        suppress_boot_pawn,
        local_carried_loadout,
    } = handles;

    // Fog volumes — one entity per record. Runs after the windowed light-bridge
    // populate so the first fog entity-id lands after the light entities.
    {
        let mut registry = script_ctx.registry.borrow_mut();
        fog_volume_bridge.populate_from_level(&mut registry, &world.fog_volumes);
    }

    // Trigger volumes — one entity per record. Kept directly after fog so the
    // fog → trigger → mover entity-id order matches the pre-extraction windowed
    // install. The host-authoritative trigger system evaluates these each tick
    // (windowed / listen host); a headless run populates them but passes no
    // trigger context to `simulate_tick`, so they are inert there.
    {
        let mut registry = script_ctx.registry.borrow_mut();
        trigger_volume_bridge.populate_from_level(&mut registry, &world.trigger_volumes);
    }

    // Collision + kinematic movers. Populate before the first game tick so
    // movement collision is ready.
    collision_world.populate_from_level(world);
    let mover_colliders = crate::runtime_movers::build_loaded_mover_colliders(world);
    let spawned_mover_entities = if !world.kinematic_geometry.movers.is_empty() {
        let mut registry = script_ctx.registry.borrow_mut();
        match crate::runtime_movers::spawn_loaded_kinematic_movers(
            &mut registry,
            world,
            mover_auto_close_ms,
        ) {
            Ok(spawned) => {
                log::info!(
                    "[Loader] spawned {} kinematic mover entity/entities",
                    spawned.len()
                );
                spawned
            }
            Err(err) => {
                log::warn!("[Loader] failed to spawn kinematic movers: {err}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    timings.record("bridges_populated");

    // Classname dispatch: partition player-start placements out (retained for the
    // caller), dispatch the remainder through built-in handlers. The handled set
    // feeds the data-archetype sweep below.
    let all_entities: Vec<crate::scripting::map_entity::MapEntity> =
        world.map_entities.iter().cloned().map(Into::into).collect();
    let (spawn_points, map_entities): (Vec<_>, Vec<_>) = all_entities
        .into_iter()
        .partition(|e| e.classname == PLAYER_START_CLASSNAME);
    let builtin_handled = {
        let mut registry = script_ctx.registry.borrow_mut();
        let handled = apply_classname_dispatch(&map_entities, classname_dispatch, &mut registry);
        if !map_entities.is_empty() {
            log::info!(
                "[Loader] dispatched {total} map entities; {built_in} classname(s) handled by built-in handlers",
                built_in = handled.len(),
                total = map_entities.len(),
            );
        }
        handled
    };
    timings.record("classname_dispatch");

    // Data script runs once at level open. Errors surface as an empty manifest so
    // the level still loads; even levels without one compose against mod-global
    // reactions/crossings. Composed before progress/crossing subscriber rebuild.
    {
        let mut manifest = if let Some(data_script) = &world.data_script {
            script_runtime.run_data_script(data_script, content_root)
        } else {
            LevelManifest::default()
        };
        if world.data_script.is_some() {
            manifest.reactions =
                validate_sequence_primitives(manifest.reactions, sequence_registry);
            // Register level-scope UI trees before the data-script VM context drops
            // and before the manifest is consumed by the data registry.
            modal_stack.register_script_trees(
                std::mem::take(&mut manifest.ui_trees),
                postretro_ui::modal_stack::ScopeTier::Level,
            );
        }
        impact_policy_runtime
            .replace_level_events(std::mem::take(&mut manifest.events), active_level_tags);
        script_ctx
            .data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                manifest.reactions,
                manifest.crossings,
                manifest.trigger_events,
                manifest.trigger_pools,
                active_level_tags,
            );
        // E18 validation — Pass A (V1, V4a, V6), then Pass B's rejection rows
        // (V2, V3, V4b). Both run BEFORE every consumer of the composed
        // reaction set: the subscriber/accumulator rebuilds below and
        // `build_trigger_bindings` all read the post-validation `DataRegistry`,
        // so a rejected reaction is an inert `Sequence(vec![])` before anything
        // binds or subscribes to it. Dropping after the binder would be a no-op
        // for trigger-bound content — the binder copies bodies into owned
        // commands/steps the drain never re-reads. Matches the staged-commit
        // order in `poll_staged_manifest_results`, so install and hot reload
        // agree on what the subscriber rebuild observes for a dropped body.
        // V4b reads sentinel-scoped targets directly from the composed
        // descriptors and runtime-IR inputs from a freshly built
        // `SystemReactionIrBindings`. The session's table is not rebuilt until
        // after this installer returns; that later rebuild recomputes the IR
        // input metadata identically.
        crate::startup::reaction_validation::validate_reaction_bodies_pass_a(script_ctx);
        {
            let mut system_reaction_bindings =
                crate::scripting_systems::system_reactions::SystemReactionIrBindings::default();
            system_reaction_bindings.rebuild(&script_ctx.data_registry.borrow(), script_ctx);
            crate::startup::reaction_validation::validate_trigger_coupled_pass_b(
                script_ctx,
                &system_reaction_bindings,
            );
        }
        // CROSSING-CHANNEL INSTALL ORDER (E18): the detector must capture this
        // level's local slot defaults before any connected-client network baseline is
        // applied. A late join then observes the host's persistent state as one real
        // crossing instead of silently arming at the already-replicated value.
        // Network baseline application begins only after world install returns.
        rebuild_reaction_subscribers(progress_tracker, crossing_detector, script_ctx);
        slot_accumulator_bindings.rebuild(script_ctx);
    }
    // Bind after subscriber rebuild: `populate_level` has committed the final
    // composed reaction set, so tick dispatch never re-matches a name later.
    let mut trigger_bindings = build_trigger_bindings(
        script_ctx,
        command_diagnostics.clone(),
        spawn_context.clone(),
    );
    {
        let registry = script_ctx.registry.borrow();
        let data_registry = script_ctx.data_registry.borrow();
        trigger_bindings.install_manifest_events(&registry, &data_registry, script_ctx);
    }
    // E18 V5 — derive the paired Exit edge for every surviving
    // interruptible-wait reaction. Runs AFTER `install_manifest_events` so a
    // manifest-bound Enter binding derives its edge too and the insert lands in
    // the final table (O36). The rejection rows already ran before the binder,
    // so a dropped body derives nothing here.
    crate::startup::reaction_validation::derive_interruptible_wait_exit_edges(
        script_ctx,
        &mut trigger_bindings,
    );
    timings.record("data_script");

    // Data-archetype sweep: materialize every matching map placement the built-in
    // dispatch did not handle, then spawn the boot pawn from the player starts.
    let descriptors = script_ctx.data_registry.borrow().entities.clone();
    // Read the baked navmesh agent params into an owned local BEFORE borrowing the
    // registry (dispatch borrows it mutably); `None` when the map has no navmesh —
    // the agent then falls back to an engine-default capsule and cannot path.
    let agent_params: Option<postretro_foundation::NavAgentParams> =
        nav_graph.map(|g| g.agent_params());
    // Mesh models of host-authoritative placements a connected client suppresses.
    // They never spawn a local `MeshComponent`, so the registry-driven model sweep
    // cannot see them; unioned into the mesh model list below so a host-replicated
    // remote entity is drawable. Empty off a connected client.
    let mut suppressed_host_replicated_models: Vec<String> = Vec::new();
    // Runtime net-slot materialization may select any movement descriptor on a
    // listen host, while a connected client receives the same set by snapshot.
    // Preload the whole category for every role; gameplay never uploads models.
    let movement_descriptor_models = movement_descriptor_mesh_models(&descriptors);
    // Replicated state schema must be role-invariant before the first snapshot is
    // validated. Resolve the authored player-health range from the shared map
    // placement + descriptor table, not from a role-specific materialized pawn: a
    // connected client intentionally suppresses its boot pawn.
    install_descriptor_player_health_range(
        &mut script_ctx.slot_table.borrow_mut(),
        &spawn_points,
        &descriptors,
    );
    // Wieldable weapon instances have no MeshComponent of their own. Preload every
    // declared third- and first-person model so attachment/viewmodel changes never
    // trigger runtime model loads or leave a transient placeholder.
    let weapon_presentation_models = weapon_presentation_models(&descriptors);
    // Projectile meshes materialize only when a weapon fires, after the registry
    // sweep. Enroll every descriptor-owned body model in this install-time upload
    // so gameplay never reaches across the renderer boundary.
    let (projectile_presentation_models, _) = projectile_presentation_assets(&descriptors);
    // A loadout-only touchable wieldable loses its own MeshComponent when held,
    // but drop restores that descriptor mesh during gameplay. Preload the world
    // holder and attachments now; model upload remains renderer-owned.
    let touchable_wieldable_world_models = touchable_wieldable_world_models(&descriptors);
    let first_spawn = {
        let mut registry = script_ctx.registry.borrow_mut();
        let mut map_entities = map_entities;
        if suppress_ai_enemies {
            // A connected client must not spawn local copies of host-authoritative
            // map entities. Filter before dispatch, while descriptor metadata is
            // available but live components do not exist yet.
            suppressed_host_replicated_models =
                suppressed_client_host_replicated_mesh_models(&map_entities, &descriptors);
            let kept = filter_out_client_host_replicated_placements(&map_entities, &descriptors);
            let dropped = map_entities.len() - kept.len();
            if dropped > 0 {
                log::info!(
                    "[Loader] connected client: suppressing {dropped} host-replicated map \
                     placement(s); they arrive via host snapshots"
                );
            }
            map_entities = kept;
        }
        let descriptor_handled = apply_data_archetype_dispatch(
            &map_entities,
            &descriptors,
            &builtin_handled,
            &mut registry,
            agent_params,
        );
        if !descriptor_handled.is_empty() {
            log::info!(
                "[Loader] dispatched {} map entities through descriptor archetypes",
                descriptor_handled.len(),
            );
        }

        // Camera pose is seeded from the first spawn regardless of spawn success
        // (a connected client holds it until the net baseline arms its pawn).
        let first_spawn: Option<(Vec3, Vec3)> = spawn_points.first().map(|e| (e.origin, e.angles));

        // A connected client must NOT spawn a boot pawn (its authoritative pawn
        // arrives as a host-replicated baseline); single-player and the listen host
        // keep spawning theirs.
        if suppress_boot_pawn {
            log::info!("[Loader] connected client: deferring player spawn to host baseline");
        } else if !spawn_points.is_empty() {
            let _ = spawn_from_player_starts_with_carried_loadout(
                &spawn_points,
                &descriptors,
                &mut registry,
                agent_params,
                local_carried_loadout.as_ref(),
            );
        } else {
            log::info!("[Loader] no player_spawn in map; skipping player spawn");
        }

        first_spawn
    };
    let spawner_diagnostics = {
        let mut registry = script_ctx.registry.borrow_mut();
        resolve_spawners_for_level(&mut registry, &descriptors, agent_params, &spawn_context)
    };
    if spawner_diagnostics.invalid_total() > 0 {
        log::warn!(
            "[Loader] {} entity_spawner placement(s) remain unresolved",
            spawner_diagnostics.invalid_total()
        );
    }
    // Pool arming is host-only and occurs after every trigger/spawner binding
    // exists, but before `levelLoad` so level-load reactions can override it.
    // Connected clients retain their authored trigger state and an empty report.
    let trigger_pool_report = if suppress_ai_enemies {
        TriggerPoolInstallReport::default()
    } else {
        let pools = script_ctx.data_registry.borrow().trigger_pools().to_vec();
        install_trigger_pools(
            &mut script_ctx.registry.borrow_mut(),
            &pools,
            trigger_pool_policy,
            &command_diagnostics,
            trigger_volume_bridge,
        )
    };
    // An `entity_spawner` itself has no mesh. Its resolved archetype can still
    // be the only reference to an enemy model in this level, on either the host
    // or a connected client (spawners survive the client AI-placement filter).
    // Feed those handles into the same install-time upload/clip-table sweep as
    // ordinary and client-suppressed placements; no runtime GPU upload exists.
    let spawner_models = {
        let registry = script_ctx.registry.borrow();
        resolved_spawner_mesh_models(&registry, &descriptors)
    };
    timings.record("archetype_sweep");

    // Mesh model sweep, CPU half. Runs AFTER both dispatch sweeps so it sees every
    // mesh entity. Reset the game-side tables, compute the distinct model list
    // (unioning models missing due to connected-client suppression), then the
    // renderer-coupled upload + clip-table build runs via the injected hook,
    // followed by the CPU hit-zone build, clip-index resolve, and zone-multiplier
    // cross-check.
    mesh_clip_tables.clear();
    hit_zone_store.clear();
    let models = {
        let registry = script_ctx.registry.borrow();
        let mut models = crate::distinct_mesh_models(&registry);
        let mut seen: std::collections::HashSet<String> = models.iter().cloned().collect();
        for model in &suppressed_host_replicated_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        for model in &movement_descriptor_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        for model in &weapon_presentation_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        for model in &projectile_presentation_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        for model in &touchable_wieldable_world_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        for model in &spawner_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        models
    };
    let model_load_warning_owner = upload_mesh_models(&models, mesh_clip_tables);
    for model in &models {
        // Build this model's game-side hit-zone entry by re-loading the glTF
        // independently of the renderer (CPU-only).
        hit_zone_store.insert_from_load(model, content_root, model_load_warning_owner);
    }
    crate::resolve_mesh_entity_bindings(
        &mut script_ctx.registry.borrow_mut(),
        mesh_clip_tables,
        hit_zone_store,
    );
    crate::warn_unknown_zone_multipliers(
        &script_ctx.data_registry.borrow().entities,
        hit_zone_store,
    );
    timings.record("model_load");

    // Bind caller-owned session state after the archetype sweep has created
    // player pawns but before `levelLoad` can address their owner association.
    before_level_load(&spawn_points);

    // Fire `levelLoad`. Headless fires it too so data-script reactions and
    // crossings compose identically; runs after the clip resolve so a
    // `setAnimationState` reaction sees concrete clip indices. This fire now
    // precedes the windowed light/sprite enrollment passes
    // (`absorb_dynamic_lights`, sprite-collection registration), which run after
    // this function returns — so a `levelLoad` reaction that spawns a dynamic
    // light or emitter is enrolled and renders (previously dropped). Intentional,
    // accepted improvement.
    // Capture the returned chained names (a `fire` step's target, or a fired
    // `Primitive`'s `on_complete`) and feed them into the deferred dispatcher —
    // previously discarded here, so a `fire` step in `levelLoad` dispatched
    // nothing. A `wait` step enrolls its tail and returns before this point.
    let level_load_chained = fire_named_event_with_sequences(
        "levelLoad",
        &script_ctx.data_registry.borrow(),
        sequence_registry,
        reaction_registry,
        system_registry,
        script_ctx,
        None,
    );
    if !level_load_chained.is_empty() {
        dispatch_deferred_named_events_with_sequences(
            level_load_chained,
            &script_ctx.data_registry.borrow(),
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
        );
    }
    // `levelLoad` may itself fire a spawner reaction after the install sweep.
    // Its archetype's table already exists above; fill only the newly attached
    // meshes before the first render rather than rebuilding or uploading.
    let spawned_meshes = spawn_context.take_pending_mesh_clip_resolves();
    crate::resolve_mesh_entity_bindings_for_entities(
        &mut script_ctx.registry.borrow_mut(),
        mesh_clip_tables,
        hit_zone_store,
        spawned_meshes,
    );
    timings.record("level_load_event");

    WorldInstallProducts {
        mover_colliders,
        spawned_mover_entities,
        trigger_bindings,
        trigger_pool_report,
        mover_tick_states: crate::kinematic_mover::MoverTickStateTable::default(),
        first_spawn,
        spawn_points,
    }
}
