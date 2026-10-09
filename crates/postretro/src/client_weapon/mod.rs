// Fixed-command client weapon prediction and rendered-pose presentation.
// See: context/lib/networking.md §Combat authority · context/lib/input.md
mod frame;
mod reconcile;
pub(crate) use frame::ClientWeaponFrame;

use crate::{App, PresentedAimPose, netcode, sim, weapon};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{ActivationToken, ShotId};

impl ClientWeaponFrame {
    /// Retract what a rejected activation predicted on `owner_weapon`: its shots
    /// still due this frame, and every live predicted projectile of `token`.
    pub(crate) fn retract_rejected_activation(
        &mut self,
        registry: &mut EntityRegistry,
        predicted: &mut weapon::ClientPredictedShots,
        token: ActivationToken,
        owner_weapon: EntityId,
    ) {
        self.due.retain(|queued| {
            let id = queued.shot.activation.shot_id;
            queued.weapon != owner_weapon
                || id.start_tick != token.start_tick
                || id.lane != token.lane
        });
        let shots: Vec<ShotId> = registry
            .iter_with_kind(ComponentKind::Projectile)
            .filter_map(|(_, value)| {
                let ComponentValue::Projectile(projectile) = value else {
                    return None;
                };
                projectile.predicted_shot_id.filter(|id| {
                    id.start_tick == token.start_tick
                        && id.lane == token.lane
                        && projectile.owner_weapon == owner_weapon
                })
            })
            .collect();
        for id in shots {
            let _ = predicted.apply_verdict(registry, id, false, false);
        }
    }
}

impl App {
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
        let (origin, direction) = presented_aim.aim_ray();
        for mut queued in std::mem::take(&mut self.client_weapon.due) {
            if !self
                .client_weapon
                .records
                .correct_new_shot(&mut queued.shot)
            {
                continue;
            }
            let id = queued.shot.activation.shot_id;
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
