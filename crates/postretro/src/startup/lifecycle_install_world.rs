//! Level install's world stages: segment B (bridges, dispatch, data script,
//! archetypes, model sweep, `levelLoad`) and the passes that follow it.
//! See: context/lib/boot_sequence.md §3

use super::*;

/// Bind PRL member-light links only after the complete mover spawn result is
/// available. This is intentionally windowed: [`App::install_level_world_stages`],
/// shared by the commit and the main-thread oracle, owns both the map-light
/// bridge and the mover spawn result, while the renderer-free installer owns
/// neither bridge nor a reason to bind lights.
///
/// A failed all-or-nothing mover spawn leaves every member unbound. The next
/// level install builds a new bridge and registry, then calls this pass again;
/// raw entity ids never survive a reload.
pub(super) fn resolve_carried_light_bindings_after_mover_spawn(
    geometry: &postretro_level_loader::KinematicGeometry,
    spawned_mover_entities: &[postretro_entities::EntityId],
    light_bridge: &crate::scripting_systems::light_bridge::LightBridge,
    registry: &mut postretro_entities::EntityRegistry,
) {
    use postretro_entities::components::light::{LightCarrier, LightComponent};

    // The spawner is all-or-nothing and returns ids order-aligned to mover
    // records. Never zip a partial result: that could bind a light to the
    // wrong mover after a load fault.
    let mover_entity_by_id = if spawned_mover_entities.len() == geometry.movers.len() {
        geometry
            .movers
            .iter()
            .zip(spawned_mover_entities.iter().copied())
            .map(|(mover, entity)| (mover.mover_id, entity))
            .collect::<std::collections::HashMap<_, _>>()
    } else {
        log::warn!(
            "[Loader] kinematic mover spawn count did not match level geometry; carried lights remain unbound"
        );
        std::collections::HashMap::new()
    };

    for mover in &geometry.movers {
        for member in &mover.carried_lights {
            let Some(&mover_entity) = mover_entity_by_id.get(&mover.mover_id) else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: mover {} was not spawned",
                    member.alpha_light_index,
                    mover.mover_id,
                );
                continue;
            };
            let Some(light_entity) =
                light_bridge.entity_for_map_index(member.alpha_light_index as usize)
            else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: map light entity is unavailable",
                    member.alpha_light_index,
                );
                continue;
            };
            let Ok(mut light) = registry
                .get_component::<LightComponent>(light_entity)
                .cloned()
            else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: light component is unavailable",
                    member.alpha_light_index,
                );
                continue;
            };
            light.carrier = Some(LightCarrier {
                mover_entity,
                local_offset: member.local_offset,
            });
            if let Err(error) = registry.set_component(light_entity, light) {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: {error}",
                    member.alpha_light_index,
                );
            }
        }
    }
}

impl App {
    /// Segment B, then the bindings that read its products while the session
    /// borrow is still live: teleporters, ghost links, carried lights.
    /// `models` says where the model sweep's models come from: parsed and
    /// uploaded here (the main-thread path), or a load's prepared products.
    pub(super) fn install_level_segment_b(
        &mut self,
        script_ctx: &postretro_entities::ScriptCtx,
        prm_cache_root: &Path,
        static_collision: crate::collision::CollisionWorld,
        suppress: bool,
        models: super::lifecycle_model_sweep::LevelModels<'_>,
    ) -> WorldInstallProducts {
        // Segment B of the CPU world install: fog-volume entities, trigger-volume
        // entities, kinematic movers, classname dispatch, the data
        // script, the data-archetype sweep (incl. player spawn), the mesh sweep's
        // CPU half, and the `levelLoad` fire — all renderer-free. The one
        // renderer-coupled step (each model's renderer entry + clip-table build)
        // is injected as the model sweep so it stays windowed; a headless caller
        // passes a no-op and its clip tables stay empty (the documented headless
        // shape).
        // Cloned so the segment-B handles do not alias a `self.content_root`
        // borrow held across the call.
        let install_content_root = self.content_root.clone();
        let renderer = self
            .renderer
            .as_mut()
            .expect("renderer installed before level install");
        let model_sweep = super::lifecycle_model_sweep::WindowedModelSweep {
            renderer,
            prm_cache_root,
            source: models,
        };

        // Collision is ready before the first game tick; segment B's mover
        // build and spawn sweep follow it.
        self.collision_world = static_collision;
        let session = self
            .session
            .as_mut()
            .expect("session installed before level install");
        let handles = WorldInstallHandles {
            world: self
                .level
                .as_ref()
                .expect("level installed before segment B"),
            script_ctx,
            command_diagnostics: session.scripting.command_diagnostics.clone(),
            mover_auto_close_ms: session.scripting.mover_auto_close_ms,
            spawn_context: session.scripting.spawn_context.clone(),
            content_root: install_content_root.as_path(),
            active_level_tags: &self.active_level_tags,
            nav_graph: self.nav_graph.as_ref(),
            fog_volume_bridge: &mut session.fog_volume_bridge,
            trigger_volume_bridge: &mut session.trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut session.modal_stack,
            progress_tracker: &mut session.progress_tracker,
            crossing_detector: &mut session.crossing_detector,
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            system_reaction_ir_bindings: &session.scripting.system_reaction_ir_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            trigger_pool_policy: self.session_boot_config.windowed_trigger_pool_policy(),
            suppress_host_replicated: suppress,
            suppress_boot_pawn: suppress,
            local_carried_loadout: session
                .seat_table
                .as_ref()
                .and_then(|seats| seats.carried_state(postretro_foundation::Seat(0)))
                .cloned(),
        };
        let mut first_player_placement = None;
        let products = install_world_cpu_with_models(
            handles,
            &mut self.level_timings,
            model_sweep,
            |first_player| {
                first_player_placement = first_player.map(|spawned| spawned.placement);
                if let Some(seats) = session.seat_table.as_mut() {
                    bind_installed_first_player(
                        seats,
                        &mut script_ctx.registry.borrow_mut(),
                        first_player,
                    );
                }
            },
        );
        // Publish after the first-player bind, so seat zero's respawn placement
        // is the spawn its pawn actually took and respawn agrees with the join
        // placement. The captured index also keeps the actual spawn when no
        // seat records it.
        crate::respawn_placements::publish(
            script_ctx,
            &products.spawn_points,
            &script_ctx.data_registry.borrow().entities,
            session.seat_table.as_ref(),
            first_player_placement,
        );
        crate::runtime_teleporters::bind_loaded_teleporters(
            &mut self.collision_world.teleporters,
            &script_ctx.registry.borrow(),
        );
        self.ghost_links = crate::ghost_links::GhostLinks::install(
            self.level
                .as_ref()
                .expect("level installed before ghost links bind"),
            &script_ctx.registry.borrow(),
        );
        crate::ghost_links::warn_if_links_without_opt_ins(
            self.level
                .as_ref()
                .map_or(0, |level| level.ghost_links.links.len()),
            self.ghost_links.ghosts_movers(),
            &script_ctx.data_registry.borrow(),
        );
        self.ghost_links.bind_styles(
            self.level
                .as_ref()
                .expect("level installed before ghost styles bind"),
            &script_ctx.data_registry.borrow().ghost_styles,
        );
        // The level's trees joined the tree set.
        session.check_accessibility_entry();

        // Lights are installed before movers. This synchronous pass is the
        // only windowed binding funnel, so it runs before the first
        // LightBridge::update on both initial loads and later reloads.
        {
            let level = self
                .level
                .as_ref()
                .expect("level installed before carried-light resolution");
            let mut registry = script_ctx.registry.borrow_mut();
            resolve_carried_light_bindings_after_mover_spawn(
                &level.kinematic_geometry,
                &products.spawned_mover_entities,
                &session.light_bridge,
                &mut registry,
            );
        }
        products
    }

    /// The passes after `levelLoad`: final reaction bindings, the products the
    /// tick loop keeps, the local weapon presentation, host registration, and
    /// install-time animations. Returns the first spawn for the camera.
    pub(super) fn install_level_post_load_passes(
        &mut self,
        script_ctx: &postretro_entities::ScriptCtx,
        products: WorldInstallProducts,
    ) -> Option<(Vec3, Vec3)> {
        // The CPU install rebuilt the session's `setState` bindings before
        // `levelLoad` fired, so its writes evaluated at dispatch.

        self.kinematic_mover_colliders = products.mover_colliders;
        self.trigger_bindings = products.trigger_bindings;
        self.teleporter_bindings = products.teleporter_bindings;
        self.warn_hosted_trigger_machine_local();
        self.warn_if_local_player_cannot_swim();
        // After the boot pawn spawns, so the host reads its own descriptor's
        // terminal velocity; the surfaces drew still until now.
        self.resolve_fluid_surface_drift();
        // A level install starts every player unobserved: no edge memory
        // carries from the previous level.
        self.rebuild_player_events(false);
        self.trigger_pool_report = products.trigger_pool_report;
        // Retain spawn-point placements for the host's runtime seat-accept path:
        // each accepted client's descriptor pawn materializes from them later.
        self.host_spawn_points = products.spawn_points;

        // The boot pawn exists before the regular host snapshot cadence (and in
        // single-player there is no `WeaponOwners` table at all), so establish its
        // shadow-only third-person weapon prop at install time. The model sweep above
        // has already built both CPU tables; resolve only this changed pawn through
        // the standard socket-binding path.
        let local_pawn = script_ctx.registry.borrow().local_player_pawn();
        let descriptors = script_ctx.data_registry.borrow().entities.clone();
        if let Some(pawn) = local_pawn {
            let session = self
                .session
                .as_mut()
                .expect("session installed before local weapon presentation install");
            let mut registry = script_ctx.registry.borrow_mut();
            if crate::netcode::synchronize_weapon_attachment_for_pawn(
                &mut registry,
                pawn,
                &descriptors,
                &session.hit_zone_store,
            ) {
                crate::resolve_mesh_entity_bindings_for_entities(
                    &mut registry,
                    &session.mesh_clip_tables,
                    &session.hit_zone_store,
                    [pawn],
                );
            }
        }

        // Register host-authoritative map entities and PRL-loaded movers for outbound
        // replication. Host-gated (a no-op off a listen host) and reload-safe; each
        // takes its own registry borrow.
        self.host_register_descriptor_entities_after_install();
        self.host_register_loaded_movers_after_install();
        self.level_timings.record("host_registration");

        // Enroll descriptor lights and start install-time animations at clock zero.
        // The first running frame advances from that origin after Settling.
        {
            let registry = script_ctx.registry.borrow();
            let session = self
                .session
                .as_mut()
                .expect("session installed before level install");
            start_installed_presentation_animations(
                &registry,
                &mut session.light_bridge,
                &mut session.fog_volume_bridge,
                &mut session.emitter_bridge,
            );
        }
        products.first_spawn
    }
}
