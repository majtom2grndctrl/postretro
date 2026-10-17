// Fixed-command client weapon prediction, rendered-pose presentation, switch
// settlement and the active-slot re-sync.
// See: context/lib/networking.md §Combat authority · context/lib/input.md
mod frame;
mod reconcile;
mod slot_resync;
pub(crate) use frame::{ClientWeaponFrame, TickAim};
#[cfg(test)]
pub(crate) use slot_resync::ACTIVE_SLOT_RESYNC_WINDOW_SECS;
pub(crate) use slot_resync::{ActiveSlotResync, ActiveSlotResyncObservation};

use crate::{App, PresentedAimPose, input, netcode, sim, weapon};
use postretro_entities::EntityRegistry;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;

/// Settle the host's answer to this client's newest switch declaration on the
/// local inventory and selection. A refusal rolls the inventory back and resets
/// the selection to the restored slot. A same-slot refusal that finds the
/// player steadily holding that slot (the active-slot re-sync) changed nothing
/// they hold, so an in-progress cursor, dwell or commit survives it.
pub(crate) fn settle_switch_resolution(
    registry: &mut EntityRegistry,
    latch: &mut input::GameplayInputLatch,
    resolution: netcode::CurrentSwitchResolution,
) {
    let (target_slot, rollback_slot, last_weapon_slot) = match resolution {
        netcode::CurrentSwitchResolution::None => return,
        netcode::CurrentSwitchResolution::Accepted { last_weapon_slot } => {
            latch
                .wieldable_selection_mut()
                .confirm_latest_declaration(last_weapon_slot);
            return;
        }
        netcode::CurrentSwitchResolution::Refused {
            target_slot,
            rollback_slot,
            last_weapon_slot,
        } => (target_slot, rollback_slot, last_weapon_slot),
    };
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return;
    };
    let held_steadily = target_slot == rollback_slot
        && slot_resync::local_selection(registry, pawn) == Some((target_slot, false));
    if !sim::refuse_local_wieldable_switch(registry, pawn, target_slot, rollback_slot) {
        return;
    }
    if held_steadily {
        return;
    }
    let active_slot = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .map(|inventory| inventory.active_slot);
    latch
        .wieldable_selection_mut()
        .reset_to_active_with_last(active_slot, last_weapon_slot);
}

impl App {
    /// Re-declare the local active slot when the host's owner-private weapon
    /// projection has named a different slot for a full
    /// `ACTIVE_SLOT_RESYNC_WINDOW_SECS` with nothing in flight.
    /// The client owns selection, and the player sees the weapon they believe
    /// they hold, so the host converges to it; a false positive from a lagging
    /// projection is an idempotent accept on the host. The declaration rides
    /// the ordinary path with its own slot as rollback, so it starts no local
    /// lower or raise, and either outcome leaves the held weapon unchanged.
    pub(crate) fn client_observe_active_slot_resync(&mut self, client_tick: u32, dt: f32) {
        let Some(session) = self.session.as_mut() else {
            self.active_slot_resync.reset();
            return;
        };
        let Some(endpoint) = session.net_endpoint.as_mut() else {
            self.active_slot_resync.reset();
            return;
        };
        let projected_slot = netcode::client_weapon_projection(Some(&*endpoint)).agreed_slot();
        let observation = {
            let registry = session.scripting.script_ctx.registry.borrow();
            registry.local_player_movement_pawn().and_then(|pawn| {
                ActiveSlotResyncObservation::read(
                    &registry,
                    pawn,
                    endpoint.has_pending_switch_declarations(),
                    projected_slot,
                )
            })
        };
        let Some(observation) = observation else {
            self.active_slot_resync.reset();
            return;
        };
        let Some(slot) = self.active_slot_resync.observe(observation, dt) else {
            return;
        };
        // Inventory slots index a ten-entry array, so this always fits; a
        // corrupt slot skips the declaration rather than naming a wrong one.
        let Ok(declared) = u8::try_from(slot) else {
            return;
        };
        log::info!(
            "[Net] host projects weapon slot {} but this client holds slot {slot}; re-declaring slot {slot}",
            projected_slot.map_or_else(|| "?".to_string(), |slot| slot.to_string()),
        );
        let last_weapon_slot = session
            .gameplay_input_latch
            .wieldable_selection()
            .last_weapon_slot();
        endpoint.send_client_switch_declaration(declared, client_tick, slot, last_weapon_slot);
    }

    pub(crate) fn predict_client_weapon_command(
        &mut self,
        command: &mut sim::SimCommand,
        tick: u32,
        dt: f32,
    ) {
        let Some(network_pawn) = netcode::client_local_pawn_network_id(
            self.session.as_ref().and_then(|s| s.net_endpoint.as_ref()),
        ) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        let Some((slot, id)) = crate::local_active_wieldable(&registry) else {
            return;
        };
        let Some(terms) = session
            .net_endpoint
            .as_ref()
            .and_then(|endpoint| match endpoint {
                netcode::NetEndpoint::Client { tuning, .. } => tuning.as_deref(),
                _ => None,
            })
            .and_then(|tuning| crate::client_fire_muzzle_terms(tuning, slot))
        else {
            return;
        };
        if let Ok(postretro_entities::ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(id, postretro_entities::ComponentKind::Weapon)
        {
            component.muzzle_offset = terms.muzzle_offset;
        }
        let projection = netcode::client_weapon_projection(session.net_endpoint.as_ref());
        let owner_alive = self.client_weapon.apply_owner_liveness(
            &mut registry,
            &session.scripting.script_ctx.slot_table.borrow(),
            &mut session.gameplay_input_latch.activation,
        );
        self.client_weapon.predict_with_liveness(
            &mut registry,
            command,
            tick,
            network_pawn.0,
            dt * 1000.0,
            terms.placement,
            &projection,
            owner_alive,
        );
        let active = registry
            .get_component::<WeaponComponent>(id)
            .ok()
            .and_then(|component| component.state.activation_cursor())
            .map(|cursor| cursor.token);
        session.gameplay_input_latch.activation.set_active(active);
    }

    pub(crate) fn run_client_fire_path_post_loop(
        &mut self,
        frame_dt: f32,
        anim_time: f64,
        presented_aim: PresentedAimPose,
        pending: &mut Vec<postretro_entities::WeaponEmission>,
    ) {
        self.client_fire_resolutions.clear();
        if !self.is_connected_client() {
            return;
        }
        let Some(ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        // Share interpolation and mover carry with the view, while keeping
        // cosmetic view feel out of fire origin and authored muzzle placement.
        // A shot whose tick preceded a teleport later in this frame keeps its own
        // tick's eye and view instead: the presented pose is the arrival.
        let presented_ray = presented_aim.aim_ray();
        let frame_teleport =
            crate::runtime_teleporters::local_teleport_identity(&ctx.registry.borrow());
        // Predicted pellets cross the same digest-bound volumes the host holds.
        // Fluid crossings read only the volumes, so the view's gravity and
        // viscosities are inert here; they are still the host's installed
        // values, never this client's own: the gravity from the payload (inert
        // zero before one installs), the viscosities from the table the
        // volumes resolved against, which on a client is the host's.
        let level_gravity = self
            .session
            .as_ref()
            .and_then(|session| session.net_endpoint.as_ref())
            .and_then(netcode::NetEndpoint::client_host_environment)
            .map_or(0.0, |environment| environment.level_gravity);
        for mut queued in std::mem::take(&mut self.client_weapon.due) {
            if !self
                .client_weapon
                .records
                .correct_new_shot(&mut queued.shot)
            {
                continue;
            }
            let id = queued.shot.activation.shot_id;
            let (origin, direction) = match queued.tick_aim {
                Some(tick) if tick.teleport != frame_teleport => tick.pose.aim_ray(),
                _ => presented_ray,
            };
            let resolution = {
                let registry = ctx.registry.borrow();
                // The original immutable data remains usable after replacement/drop.
                let component = queued.component;
                weapon::resolve_client_shot(
                    Some(queued.pawn),
                    component,
                    &queued.pellet_salt,
                    queued.slot,
                    origin,
                    direction,
                    &queued.placement,
                    &self.collision_world,
                    postretro_collision::EnvironmentView::new(
                        level_gravity,
                        &self.environment_volumes,
                        self.environment_fluids.viscosities(),
                    ),
                    &registry,
                    &self
                        .session
                        .as_ref()
                        .expect("live client session")
                        .hit_zone_store,
                    anim_time,
                    queued.shot.clone(),
                )
            };
            self.client_predicted_shots.predict(
                id,
                queued.weapon,
                &resolution,
                queued.cooldown_before,
                queued.cooldown_after,
                queued.presentation,
            );
            let (shooter, name) = {
                let registry = ctx.registry.borrow();
                (
                    postretro_sim::emission::entity_emitter(&registry, queued.pawn),
                    postretro_sim::emission::descriptor_name(&registry, queued.weapon),
                )
            };
            let contacts = resolution.impact_contacts();
            let effects = weapon::client_pull_effects(
                queued.presentation,
                resolution.projectile_launch.is_some(),
                !contacts.is_empty(),
            );
            // The burst follows the `impact` address: a dry or silent pull raises
            // neither, and a later verdict never retracts it.
            if effects.addresses.contains(&"impact") {
                weapon::spawn_impact_effects_for_contacts(
                    &mut ctx.registry.borrow_mut(),
                    &contacts,
                );
            }
            for address in effects.addresses {
                pending.push(postretro_entities::WeaponEmission {
                    sounds: Some(queued.shot.sounds.clone()),
                    action: Some(queued.shot.action().clone()),
                    shot_id: Some(id),
                    address,
                    emitter: if address == "impact" {
                        postretro_entities::Emitter::Contacts(contacts.clone())
                    } else {
                        shooter.clone()
                    },
                    weapon: name.clone(),
                });
            }
            let mut spawned = false;
            if let Some(launch) = resolution.projectile_launch.clone() {
                // One scoped lease covers spawn and visibility. A temporary
                // borrow in an if-let condition would remain live in its body.
                let mut registry = ctx.registry.borrow_mut();
                if let Some(projectile) = sim::spawn_projectile(
                    &mut registry,
                    queued.pawn,
                    queued.weapon,
                    launch,
                    Some(id),
                    sim::ProjectileSource {
                        weapon: name,
                        activation: None,
                    },
                ) {
                    sim::set_predicted_projectile_visible(
                        &mut registry,
                        projectile,
                        queued.presentation == weapon::ClientPullPresentation::Fire,
                    );
                    spawned = true;
                }
            }
            if resolution.projectile_launch.is_none() || !spawned {
                let (hits, world) = if resolution.projectile_launch.is_none() {
                    (
                        resolution.hits.as_slice(),
                        resolution.world_contacts.as_slice(),
                    )
                } else {
                    (&[][..], &[][..])
                };
                let _ = netcode::client_send_hit_declaration(
                    self.session.as_mut().and_then(|s| s.net_endpoint.as_mut()),
                    id,
                    hits,
                    world,
                );
            }
            self.client_fire_resolutions.push(resolution);
        }
        self.advance_client_predicted_projectiles(frame_dt, anim_time, pending);
    }
}

#[cfg(test)]
mod tests;
