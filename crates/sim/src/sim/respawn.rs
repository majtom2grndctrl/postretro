//! Request-driven relocation before command admission; state names resolve at drain.
use glam::{EulerRot, Quat, Vec3};
use postretro_entities::components::health::{HealthComponent, set_health_absolute};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, RespawnPlacement, ScriptCtx, Transform,
};
use postretro_foundation::{
    LifeStatePosture, MovementState, PlayerMovementComponent, enter_life_state,
};

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
        let mut movement = PlayerMovementComponent::from_descriptor(movement_descriptor);
        movement.teleport = old_teleport;
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
        if movement.life_posture == LifeStatePosture::Crouch
            && let Some(crouch) = movement.crouch.as_ref()
        {
            movement.capsule.half_height = crouch.half_height;
            movement.capsule.eye_height = crouch.eye_height;
            movement.movement_state = MovementState::Crouching {
                eye_current: crouch.eye_height,
            };
        }
        let seat_placement = registry
            .seat_for_pawn(pawn)
            .and_then(|seat| placements.seats.get(&seat).copied());
        let preferred_index = seat_placement.or(placements.first_player);
        let preferred = preferred_index
            .and_then(|index| {
                placements
                    .map_order
                    .iter()
                    .find(|placement| placement.map_index == index)
            })
            .or_else(|| placements.map_order.first());
        let Some(preferred) = preferred else {
            log::warn!("[Respawn] no published placement for pawn {pawn:?}; skipping");
            continue;
        };
        let position_for = |placement: &RespawnPlacement| {
            placement.foot_anchor
                + Vec3::Y * (movement.capsule.radius + movement.capsule.half_height)
        };
        let free = |placement: &RespawnPlacement| {
            !overlaps_pawn(registry, pawn, position_for(placement), &movement)
        };
        let chosen = if free(preferred) {
            preferred
        } else if let Some(free) = placements
            .map_order
            .iter()
            .find(|placement| free(placement))
        {
            free
        } else {
            log::warn!("[Respawn] no free placement for pawn {pawn:?}; using its seat placement");
            preferred
        };
        let position = position_for(chosen);
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
                Some(value) => {
                    log::warn!("[Respawn] pawn {pawn:?} invalid health {value}; using max {max}");
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

fn overlaps_pawn(
    registry: &EntityRegistry,
    subject: EntityId,
    position: Vec3,
    movement: &PlayerMovementComponent,
) -> bool {
    registry
        .query_by_component_and_tag(ComponentKind::PlayerMovement, None)
        .any(|(pawn, _)| {
            if pawn == subject
                || crate::scripting_systems::health::is_terminally_committed_to_removal(
                    registry, pawn,
                )
            {
                return false;
            }
            let (Ok(other), Ok(transform)) = (
                registry.get_component::<PlayerMovementComponent>(pawn),
                registry.get_component::<Transform>(pawn),
            ) else {
                return false;
            };
            let radius = movement.capsule.radius + other.capsule.radius;
            let lower = movement.capsule.half_height + movement.capsule.radius;
            let other_lower = other.capsule.half_height + other.capsule.radius;
            let delta = position - transform.position;
            delta.x * delta.x + delta.z * delta.z < radius * radius
                && delta.y.abs() < lower + other_lower
        })
}

#[cfg(test)]
#[path = "respawn_tests.rs"]
mod tests;
