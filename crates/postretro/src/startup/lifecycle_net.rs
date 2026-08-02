//! Network-parity and level-unload lifecycle operations.

use glam::Vec3;

use crate::App;
use crate::camera::Camera;
use crate::frame_timing::InterpolableState;
use crate::startup::BootState;
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_pools::TriggerPoolInstallReport;

impl App {
    /// Install the immutable admission identity and current mod-parity digest on
    /// an already-constructed endpoint. This is deliberately a no-op for
    /// single-player, keeping hash work out of the ordinary boot path.
    pub(crate) fn install_network_mod_content(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if session.net_endpoint.is_none() {
            return;
        }
        let identity = session
            .scripting
            .script_runtime
            .committed_mod_identity()
            .map(|(id, version)| (id.to_owned(), version.to_owned()));
        let digest = {
            let registry = session.scripting.script_ctx.data_registry.borrow();
            crate::mod_digest::mod_compatibility_digest_from_registry(&registry)
        };
        if let Some(endpoint) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        {
            if let Some((id, version)) = identity {
                endpoint.set_mod_identity(id, version);
            }
            endpoint.set_mod_digest(digest);
        }
        self.refresh_host_tuning();
    }

    /// Re-resolve participating pawns after a committed manifest changes. The
    /// retained payload map makes this cheap on reloads that did not alter movement
    /// or live wieldable tuning.
    fn refresh_host_tuning(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let descriptors = session
            .scripting
            .script_ctx
            .data_registry
            .borrow()
            .entities
            .clone();
        let registry = session.scripting.script_ctx.registry.borrow();
        let Some(crate::netcode::NetEndpoint::Host {
            server,
            slot_pawns,
            last_sent_tuning,
            ..
        }) = session.net_endpoint.as_mut()
        else {
            return;
        };
        for client_id in server.participating_clients() {
            let Some(pawn) = slot_pawns.pawn_for(client_id) else {
                continue;
            };
            let payload = crate::netcode::tuning_payload_for_pawn(&registry, pawn, &descriptors);
            crate::netcode::host_send_tuning_if_changed(
                server,
                last_sent_tuning,
                client_id,
                payload,
            );
        }
    }

    pub(crate) fn clear_surface_lifetime_level_state(&mut self) {
        // Fog- and trigger-volume entities live in the script registry;
        // clearing their bridge id tables and trigger state prevents stale
        // slots or bindings if a future surface re-creation re-runs
        // `populate_from_level`. collision_world is reset for the same
        // reason — it must be a clean placeholder before resume populates it.
        // Called both from `unload_level` (session installed) and the suspend
        // path (session may be absent if suspend arrives pre-install), so the
        // session-owned state clears are guarded — a no-op with no session yet.
        if let Some(session) = self.session.as_mut() {
            session.scripting.command_diagnostics.clear();
            session.scripting.spawn_context.clear();
            session.scripting.slot_accumulator_bindings.clear();
            session.scripting.impact_policy_runtime.clear_level_events();
            session.fog_volume_bridge.clear();
            session.trigger_volume_bridge.clear();
            session.trigger_system.clear();
            // The selection holder shares the gameplay input latch's clear path.
            // Surface-level teardown includes level unload, so no cursor, dwell,
            // last-slot memory, or unconsumed declaration can reach the next level.
            session.gameplay_input_latch.clear();
            session
                .scripting
                .player_hud_state
                .set_pending_weapon_slot(None);
        }
        self.collision_world.clear();
        self.kinematic_mover_colliders.clear();
        self.kinematic_mover_tick_states.clear();
        self.mover_yaw_carry_ground = postretro_foundation::GroundRef::Airborne;
        self.kinematic_mover_render.clear();
        self.trigger_bindings = TriggerBindingTable::default();
        self.trigger_pool_report = TriggerPoolInstallReport::default();
        self.client_fire_resolutions.clear();
        self.client_predicted_shots.clear();
    }

    /// Unload the active level without dropping renderer/window ownership.
    ///
    /// | Cleared on unload | Kept across unload |
    /// |---|---|
    /// | `self.level` (LevelWorld) | renderer device/queue, window |
    /// | per-level GPU resources (textures, geometry) | `script_ctx`, `ScriptRuntime` |
    /// | light bridge, fog bridge, trigger-volume bridge, trigger system, trigger bindings, collision world | slot table (no clear method — engine-global) |
    /// | level sounds, sprite collections, `emitter_bridge`, `mesh_render`, `mesh_clip_tables`, `hit_zone_store`, seat pawn bindings | entity-type registry (`data_registry.entities`), mod map catalog (`data_registry.maps`), carried per-seat state |
    /// | `data_registry` reactions + crossings, accumulator bindings, presentation cells | persisted-state save path |
    /// | level-scope UI trees (`modal_stack` `ScopeTier::Level`) | |
    /// | progress tracker, death-event carryover, active wieldable, client weapon prediction state, camera pose | |
    pub(crate) fn unload_level(&mut self) {
        self.clear_net_level_parity();
        // `net_endpoint` and `audio` are session-owned; reset/release them through
        // the session borrow.
        if let Some(session) = self.session.as_mut() {
            if let Some(endpoint) = session.net_endpoint.as_mut() {
                endpoint.reset_level_scoped_client_state();
            }
            if let Some(audio) = session.audio.as_mut() {
                audio.release_level_sounds();
            }
        }

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.release_level_resources();
        }

        self.level = None;
        self.clear_surface_lifetime_level_state();
        self.nav_graph = None;
        // The registry is cleared below, retiring the chase agent's entity slot;
        // drop the handle so a stale id is never re-targeted after unload.
        #[cfg(feature = "dev-tools")]
        {
            self.debug_chase_agent = None;
        }
        self.particle_live_counts.clear();
        if let Some(session) = self.session.as_mut() {
            session.light_bridge.clear();
            session.particle_render.reset_for_level();
            session.mesh_clip_tables.clear();
            session.hit_zone_store.clear();
            session.mesh_render.clear();
            session.emitter_bridge.clear();
            session.progress_tracker.clear();
            session.pending_death_events.clear();
            session.crossing_detector.clear();
            session
                .scripting
                .script_ctx
                .data_registry
                .borrow_mut()
                .clear();
            {
                let registry = session.scripting.script_ctx.registry.borrow();
                if let Some(seats) = session.seat_table.as_mut() {
                    seats.harvest_bound_pawns(&registry);
                }
            }
            session
                .scripting
                .script_ctx
                .registry
                .borrow_mut()
                .clear_for_level_unload();
            if let Some(seats) = session.seat_table.as_mut() {
                seats.clear_pawn_bindings_for_level_unload();
            }
            session.presentation_cells.clear();
            session
                .modal_stack
                .clear_script_tree_tier(postretro_ui::modal_stack::ScopeTier::Level);
        }
        self.active_level_tags.clear();
        self.active_level_source = None;

        self.pending_level_log = false;
        self.camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        self.frame_timing
            .push_state(InterpolableState::new(Vec3::ZERO));
        self.script_time = 0.0;
        self.anim_time = 0.0;
        self.boot_state = BootState::Frontend;
    }

    /// Forget the installed level on a still-live endpoint. Both unload and
    /// platform suspend reach this helper so neither leaves peers participating
    /// against a torn-down world.
    pub(crate) fn clear_net_level_parity(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(endpoint) = session.net_endpoint.as_mut() else {
            return;
        };
        endpoint.set_level_parity(None);
        endpoint.set_relevel_catalog_id(None);
        endpoint.reset_level_scoped_host_state();
    }
}
