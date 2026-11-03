//! Request-driven relocation before command admission; state names resolve at drain.
//! See: context/lib/scripting.md §12 · context/lib/movement.md §6.
use glam::{EulerRot, Quat, Vec3};
use postretro_entities::components::health::{HealthComponent, set_health_absolute};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, RespawnPlacement, ScriptCtx, Transform};
use postretro_foundation::{PlayerMovementComponent, enter_life_state};

use crate::placement_occupancy::{PlacementOccupancy, standing_centre};

pub(super) struct Arrival {
    pub pawn: EntityId,
    pub position: Vec3,
    pub yaw: f32,
    pub old_yaw: f32,
}

pub(super) fn drain(ctx: &ScriptCtx, registry: &mut EntityRegistry) -> Vec<Arrival> {
    use postretro_scripting_core::group_resolution::group_commands_apply_here;
    if !group_commands_apply_here(ctx) {
        return Vec::new();
    }
    let requests = std::mem::take(&mut *ctx.pending_respawns.borrow_mut());
    if requests.is_empty() {
        return Vec::new();
    }
    let mut requests: Vec<_> = requests.into_values().collect();
    requests.sort_by_key(|request| (registry.seat_for_pawn(request.pawn), request.pawn));
    let data = ctx.data_registry.borrow();
    let placements = ctx.respawn_placements.borrow();
    let mut arrivals = Vec::with_capacity(requests.len());
    for request in requests {
        let pawn = request.pawn;
        if !registry.exists(pawn)
            || !postretro_scripting_core::group_resolution::is_player_pawn(registry, pawn)
            || crate::scripting_systems::health::is_terminally_committed_to_removal(registry, pawn)
        {
            log::warn!("[Respawn] queued pawn {pawn:?} no longer participates; skipping");
            continue;
        }
        let descriptor = registry
            .get_component::<DescriptorProvenance>(pawn)
            .ok()
            .and_then(|provenance| {
                data.entities.iter().find(|descriptor| {
                    descriptor.canonical_name.as_deref() == Some(provenance.canonical_name.as_str())
                })
            });
        let (Some(descriptor), Ok(live), Ok(transform)) = (
            descriptor,
            registry.get_component::<PlayerMovementComponent>(pawn),
            registry.get_component::<Transform>(pawn),
        ) else {
            log::warn!("[Respawn] target {pawn:?} has no player movement descriptor; skipping");
            continue;
        };
        let Some(movement_descriptor) = descriptor.movement.as_ref() else {
            log::warn!("[Respawn] target {pawn:?} descriptor has no player movement; skipping");
            continue;
        };
        let old_teleport = live.teleport.clone();
        let old_entry = live.life_state_entry;
        let mut movement = PlayerMovementComponent::from_descriptor(movement_descriptor);
        movement.teleport = old_teleport;
        // Carried so the entry below counts on from the live pawn's: a respawn
        // into the state it already holds still reads as a re-entry.
        movement.life_state_entry = old_entry;
        if let Some(states) = descriptor.life_states.as_ref() {
            let target = request.state.as_deref().and_then(|name| states.state(name));
            let (id, profile) = target.unwrap_or_else(|| {
                if let Some(name) = request.state.as_deref() {
                    log::warn!("[Respawn] pawn {pawn:?} state `{name}` removed before drain; entering initial");
                }
                states.initial_state()
            });
            enter_life_state(&mut movement, id, profile, states.table_revision);
        } else {
            postretro_foundation::clear_life_state(&mut movement, data.life_state_table_revision);
            if let Some(name) = request.state.as_deref() {
                log::warn!(
                    "[Respawn] pawn {pawn:?} no longer declares state `{name}`; respawning undeclared"
                );
            }
        }
        crate::sim::player_life::apply_spawn_life_posture(&mut movement);
        let seat = registry.seat_for_pawn(pawn);
        let seat_placement = seat.and_then(|seat| placements.seats.get(&seat).copied());
        let preferred_index = seat_placement.or(placements.first_player);
        let preferred = preferred_index
            .and_then(|index| {
                placements
                    .map_order
                    .iter()
                    .find(|placement| placement.map_index == index)
            })
            .or_else(|| placements.map_order.first());
        // With nowhere to relocate, no part of the respawn applies.
        let Some(preferred) = preferred else {
            log::warn!(
                "[Respawn] dropping respawn of pawn {pawn:?}: the level published no joinable \
                 player_spawn placement; health, life state and position stay unchanged"
            );
            continue;
        };
        let chosen = {
            let held_elsewhere = placements
                .seats
                .iter()
                .filter(|(other, _)| Some(**other) != seat)
                .map(|(_, &index)| index);
            let occupancy = PlacementOccupancy::new(registry, Some(pawn), held_elsewhere);
            let capsule = Some(&movement.capsule);
            let free = |placement: &&RespawnPlacement| {
                occupancy.is_free(placement.map_index, placement.foot_anchor, capsule)
            };
            let live_unoccupied = |placement: &&RespawnPlacement| {
                !occupancy.live_pawn_overlaps(placement.foot_anchor, capsule)
            };
            // Degraded: with every placement held or occupied, a held seat's
            // empty placement still beats landing inside a live pawn.
            if let Some(chosen) = Some(preferred)
                .filter(free)
                .or_else(|| placements.map_order.iter().find(free))
                .or_else(|| placements.map_order.iter().find(live_unoccupied))
            {
                chosen
            } else {
                log::warn!(
                    "[Respawn] no free placement for pawn {pawn:?}; using its seat placement"
                );
                preferred
            }
        };
        let position = standing_centre(chosen.foot_anchor, &movement.capsule);
        let old_yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let pose = Transform {
            position,
            rotation: Quat::from_rotation_y(chosen.yaw),
            ..*transform
        };
        let _ = registry.set_component(pawn, movement);
        let _ = registry.set_component(
            pawn,
            postretro_foundation::ImmersionComponent { immersion: 0.0 },
        );
        let _ = registry.set_presentation_transform(pawn, pose);
        if let Ok(health) = registry.get_component::<HealthComponent>(pawn) {
            let max = health.max;
            let health = match request.health {
                Some(value) if value.is_finite() && value > 0.0 && value <= max => value,
                Some(_) => {
                    // Loading may lower the authored number (a non-finite
                    // value arrives as 0), so name the rule, not the value.
                    log::warn!(
                        "[Respawn] pawn {pawn:?} invalid health (non-finite or out of range); \
                         using max {max}"
                    );
                    max
                }
                None => max,
            };
            set_health_absolute(registry, pawn, health);
        }
        crate::scripting_systems::health::cancel_owned_weapon_activations(registry, pawn);
        arrivals.push(Arrival {
            pawn,
            position,
            yaw: chosen.yaw,
            old_yaw,
        });
    }
    arrivals
}

#[cfg(test)]
#[path = "respawn_tests.rs"]
mod tests;
