// Descriptor materialization helpers used by netcode replication.
// See: context/lib/networking.md

use super::MapEntity;
use super::data_archetype::{
    ai_capsule_center_from_feet_offset, compose_wieldable_inventory,
    compose_wieldable_inventory_from_slots, descriptor_light_component, descriptor_mesh_component,
    find_descriptor, spawn_descriptor_instance,
};
use super::wieldable_inventory::{acquire_wieldable_at, release_wieldable};
use postretro_combat_model::{
    CarriedState, TuningPayload, WieldableTuningPayload, restore_carried_health,
};
use postretro_entities::components::inventory::Inventory;
#[cfg(test)]
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::{MAX_EFFECTIVE_SPREAD_DEGREES, WeaponComponent};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::{MAX_PELLET_COUNT, NavAgentParams};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

/// Spawn ONE descriptor-backed networked-slot player pawn from a `player_spawn`
/// placement (M15 Phase 3 Task 4). This is the host-authoritative remote-pawn
/// counterpart to [`super::data_archetype::spawn_from_player_starts`]: it reuses the
/// same descriptor materialization internals ([`spawn_descriptor_instance`]) and the
/// same `entity_class` KVP → `"player"`-default descriptor lookup, but it is
/// deliberately NOT the local-player path:
///
/// - it does NOT call `mark_local_player_pawn` (a remote pawn is never the host's
///   local player).
///
/// The pawn's `components.inventory.loadout` still materializes host-side sibling
/// wieldable instances, so active-weapon resolution has the same shape as the
/// player-start path. Inventory owns its active slot; no global active-wieldable
/// holder exists.
///
/// Provenance is stamped [`DescriptorSpawnPath::NetworkSlot`] so these pawns are
/// distinguishable from map-start single-player spawns. The per-placement KVP bag is
/// forwarded with `entity_class` stripped, matching `spawn_from_player_starts`.
///
/// Returns the spawned pawn `EntityId`, or `None` if the pawn descriptor is
/// unregistered or the registry is exhausted (logged, like the player-start path).
pub(crate) fn spawn_net_slot_pawn(
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
) -> Option<EntityId> {
    spawn_net_slot_pawn_with_carried_loadout(placement, descriptors, registry, agent_params, None)
}

/// Network-slot materialization with an already-resolved carried record. Seat
/// ownership remains with the host lifecycle; this helper receives no seat or
/// seat table.
pub fn spawn_net_slot_pawn_with_carried_loadout(
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
    carried_loadout: Option<&CarriedState>,
) -> Option<EntityId> {
    let entity_class = placement
        .key_values
        .get("entity_class")
        .map(String::as_str)
        .unwrap_or("player");

    let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
        log::warn!(
            "[Net] {origin}: entity_class `{entity_class}` not registered; skipping net-slot spawn",
            origin = placement.diagnostic_origin(),
        );
        return None;
    };

    let Some(id) = spawn_descriptor_instance(
        registry,
        descriptor,
        placement,
        // Keep generic descriptor weapon attachment enabled for parity with the
        // player-start path. Inventory composition independently materializes
        // live siblings and selects the carried active slot, falling back to
        // the first populated slot.
        true,
        DescriptorSpawnPath::NetworkSlot,
        agent_params,
    ) else {
        log::warn!(
            "[Net] {origin}: entity registry exhausted; dropping net-slot pawn `{entity_class}`",
            origin = placement.diagnostic_origin(),
        );
        return None;
    };

    restore_carried_health(carried_loadout, registry, id);

    // Forward the per-placement KVP bag (sans `entity_class`, a routing hint) so
    // `getEntityProperty` works uniformly for net-slot pawns, matching the
    // player-start path. Deliberately NO `mark_local_player_pawn` here.
    let mut kvps = placement.key_values.clone();
    kvps.remove("entity_class");
    let _ = registry.set_map_kvps(id, kvps);

    // The host materializes every remote pawn's owned instances. Consumers resolve
    // the selected instance from the pawn inventory; no sibling id escapes this
    // spawn boundary.
    let _ = compose_wieldable_inventory(
        registry,
        id,
        descriptor,
        placement,
        descriptors,
        carried_loadout,
    );

    Some(id)
}

/// Materialize the descriptor-derived `PlayerMovementComponent` for a client's LOCAL
/// network pawn (M15 Phase 3 Task 7), reusing the same descriptor → component
/// internals as the host's net-slot spawn (`PlayerMovementComponent::from_descriptor`,
/// the body of `attach_descriptor_components`). This is the client counterpart to
/// the host's [`spawn_net_slot_pawn`]: the host spawns the authoritative pawn from a
/// descriptor, while the client materializes descriptor movement tuning locally before
/// replicated movement state merges onto it. The wire carries current movement state,
/// not descriptor-immutable tuning, so prediction/reconciliation needs the matching
/// local component first.
///
/// `entity_class` is the descriptor class the host stamped on the wire (default
/// `"player"` if the record carried none). The component is built from that class's
/// `movement` block. Idempotent: a re-baseline / re-arm must not reset the live tick
/// state, so an entity already carrying a `PlayerMovementComponent` is left untouched.
///
/// Returns `true` if a component is now present (materialized this call or already
/// there), `false` if the descriptor is unregistered or has no movement block (logged)
/// — in which case prediction stays inert for that pawn, exactly as before this path.
///
/// Deliberately does NOT call `mark_local_player_pawn` (the client's apply path owns
/// that marker, set in `maybe_arm_local_pawn`) and attaches nothing but the movement
/// component — no weapon, no provenance, no KVPs. It is a narrow local-state seam, not
/// a full descriptor spawn.
pub fn materialize_net_local_movement_component(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
) -> bool {
    // Idempotent: never clobber a live component on a re-arm.
    if matches!(
        registry.has_component_kind(id, ComponentKind::PlayerMovement),
        Ok(true)
    ) {
        return true;
    }

    let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
        log::warn!(
            "[Net] local pawn entity_class `{entity_class}` not registered; movement \
             prediction stays inert for this pawn"
        );
        return false;
    };
    let Some(movement_desc) = descriptor.movement.as_ref() else {
        log::warn!(
            "[Net] local pawn entity_class `{entity_class}` has no movement block; movement \
             prediction stays inert for this pawn"
        );
        return false;
    };

    let component = PlayerMovementComponent::from_descriptor(movement_desc);
    // `set_component` only fails on a stale id; the caller proved the pawn live.
    let _ = registry.set_component(id, component);
    true
}

/// Materialize a connected client's local movement component from the host's
/// replicated tuning payload. `view_feel` has already been restored from the
/// local descriptor by the caller; every simulation field remains host-owned.
pub fn materialize_net_local_movement_component_from_tuning(
    movement: &postretro_foundation::PlayerMovementDescriptor,
    registry: &mut EntityRegistry,
    id: EntityId,
    rebuild: bool,
) -> bool {
    if rebuild {
        let _ = registry.remove_component::<PlayerMovementComponent>(id);
    } else if matches!(
        registry.has_component_kind(id, ComponentKind::PlayerMovement),
        Ok(true)
    ) {
        return true;
    }
    let _ = registry.set_component(id, PlayerMovementComponent::from_descriptor(movement));
    true
}

/// Materialize a connected client's local wieldable inventory and merge the
/// host's replicated weapon tuning. The inventory composes once, from the first
/// installed payload's fixed slot array and spawn active slot, whichever of the
/// pawn baseline and the payload lands first; a pawn armed before its payload
/// holds no inventory, as its movement already waits. It never composes from
/// the client's own descriptor (`networking.md` §What gates, and what replicates
/// instead). Later payloads preserve and retune a same-canonical instance,
/// including its magazine and live timers; a changed canonical name replaces
/// the local instance at the host-named slot. Returns whether the pawn holds an
/// inventory.
pub fn materialize_net_local_wieldable_inventory_from_tuning(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
    tuning: Option<&TuningPayload>,
) -> bool {
    let has_inventory = matches!(
        registry.has_component_kind(id, ComponentKind::Inventory),
        Ok(true)
    );
    if !has_inventory {
        let Some(tuning) = tuning else {
            return false;
        };
        let placement = MapEntity {
            map_ordinal: None,
            classname: entity_class.to_string(),
            origin: registry
                .get_component::<Transform>(id)
                .map_or(glam::Vec3::ZERO, |transform| transform.position),
            angles: glam::Vec3::ZERO,
            key_values: Default::default(),
            tags: vec![],
        };
        let slots = std::array::from_fn(|slot| {
            tuning.wieldables[slot]
                .as_ref()
                .map(|weapon| weapon.canonical_name.clone())
        });
        let _ = compose_wieldable_inventory_from_slots(
            registry,
            id,
            &placement,
            descriptors,
            &slots,
            None,
            Some(tuning.spawn_active_slot),
        );
    }

    if registry.get_component::<Inventory>(id).is_err() {
        return false;
    }
    let Some(tuning) = tuning else {
        return true;
    };

    for (slot, tuning) in tuning.wieldables.iter().enumerate() {
        let weapon_id = registry
            .get_component::<Inventory>(id)
            .ok()
            .and_then(|inventory| inventory.wieldables[slot]);
        match (weapon_id, tuning) {
            (None, Some(tuning)) => {
                let _ = materialize_net_local_wieldable_at_slot(
                    descriptors,
                    registry,
                    id,
                    slot,
                    tuning,
                );
            }
            (Some(_), None) => {
                if let Some(released) = release_wieldable(registry, id, slot) {
                    let _ = registry.despawn(released);
                }
            }
            (Some(weapon_id), Some(tuning)) => {
                let same_canonical_name = registry
                    .get_component::<DescriptorProvenance>(weapon_id)
                    .is_ok_and(|provenance| provenance.canonical_name == tuning.canonical_name);
                if !same_canonical_name {
                    if let Some(released) = release_wieldable(registry, id, slot) {
                        let _ = registry.despawn(released);
                    }
                    let _ = materialize_net_local_wieldable_at_slot(
                        descriptors,
                        registry,
                        id,
                        slot,
                        tuning,
                    );
                    continue;
                }
                apply_net_wieldable_tuning(registry, weapon_id, tuning);
            }
            (None, None) => {}
        };
    }

    true
}

fn materialize_net_local_wieldable_at_slot(
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    pawn: EntityId,
    slot: usize,
    tuning: &WieldableTuningPayload,
) -> bool {
    let canonical_name = tuning.canonical_name.as_str();
    let Some(descriptor) = find_descriptor(descriptors, canonical_name) else {
        log::warn!(
            "[Net] local pawn tuning names unknown wieldable `{canonical_name}`; slot {slot} stays empty"
        );
        return false;
    };
    let weapon_entity = MapEntity {
        map_ordinal: None,
        classname: canonical_name.to_string(),
        origin: registry
            .get_component::<Transform>(pawn)
            .map_or(glam::Vec3::ZERO, |transform| transform.position),
        angles: glam::Vec3::ZERO,
        key_values: Default::default(),
        tags: vec![],
    };
    let Some(weapon_id) = spawn_descriptor_instance(
        registry,
        descriptor,
        &weapon_entity,
        true,
        DescriptorSpawnPath::DefaultWeapon,
        None,
    ) else {
        return false;
    };
    let _ = registry.set_map_kvps(weapon_id, Default::default());
    if !acquire_wieldable_at(registry, pawn, slot, weapon_id) {
        let _ = registry.despawn(weapon_id);
        return false;
    }
    apply_net_wieldable_tuning(registry, weapon_id, tuning);
    true
}

/// Compare only the host fields installed on this instance. Sound keys remain
/// local, placement belongs to the owner projection, and resource balances and
/// activation/recovery state are live values rather than descriptor tuning.
fn net_wieldable_gameplay_matches(
    weapon: &WeaponComponent,
    tuning: &WieldableTuningPayload,
) -> bool {
    // Keep this exhaustive so a new host field cannot silently skip retuning.
    let WieldableTuningPayload {
        canonical_name: _,
        placement: _,
        muzzle_offset,
        range,
        primary,
        secondary,
        damage,
        knockback,
        projectile,
        splash,
        resource,
        pellet_count,
        spread_degrees,
        bloom_per_shot_degrees,
        bloom_max_degrees,
        bloom_decay_degrees_per_second,
        bloom_decay_delay_ms,
        movement_spread_degrees,
        spread_vertical_bias,
        resolution,
        lower_ms,
        raise_ms,
        block_during_reload,
    } = tuning;
    let activation_matches =
        |installed: &postretro_foundation::WeaponActivationDescriptor,
         incoming: &postretro_foundation::WeaponActivationDescriptor| {
            let postretro_foundation::WeaponActivationDescriptor {
                trigger,
                recovery_ms,
                steps,
                charge,
                sounds: _,
                emits,
            } = incoming;
            installed.trigger == *trigger
                && installed.recovery_ms == *recovery_ms
                && &installed.steps == steps
                && &installed.charge == charge
                && &installed.emits == emits
        };
    let resource_matches = match (
        resource,
        weapon.ammo.as_ref(),
        weapon.heat.as_ref(),
        weapon.cell.as_ref(),
    ) {
        (None, None, None, None) => true,
        (Some(postretro_foundation::WeaponResource::Ammo(incoming)), Some(ammo), None, None) => {
            let postretro_foundation::AmmoResource {
                ammo_type,
                magazine,
                cost_per_shot,
                // Initial grants are not installed on the weapon component.
                reserve: _,
                reload_ms,
                reload_style,
            } = incoming;
            ammo.ammo_type == *ammo_type
                && ammo.capacity == *magazine
                && ammo.cost_per_shot == *cost_per_shot
                && ammo.reload_ms == *reload_ms
                && ammo.reload_style == *reload_style
        }
        (Some(postretro_foundation::WeaponResource::Heat(incoming)), None, Some(heat), None) => {
            heat.tuning == *incoming
        }
        (Some(postretro_foundation::WeaponResource::Cell(incoming)), None, None, Some(cell)) => {
            cell.tuning == *incoming
        }
        _ => false,
    };
    weapon.damage == *damage
        && weapon.knockback == *knockback
        && weapon.range == *range
        && &weapon.projectile == projectile
        && &weapon.splash == splash
        && weapon.muzzle_offset.map(|offset| offset.to_array()) == *muzzle_offset
        && weapon.pellet_count == (*pellet_count).clamp(1, MAX_PELLET_COUNT)
        && weapon.spread_degrees == clamp_tuning_degrees(*spread_degrees)
        && weapon.bloom_per_shot_degrees == clamp_tuning_degrees(*bloom_per_shot_degrees)
        && weapon.bloom_max_degrees == clamp_tuning_degrees(*bloom_max_degrees)
        && weapon.bloom_decay_degrees_per_second
            == clamp_tuning_nonnegative(*bloom_decay_degrees_per_second)
        && weapon.bloom_decay_delay_ms == clamp_tuning_nonnegative(*bloom_decay_delay_ms)
        && weapon.movement_spread_degrees == clamp_tuning_degrees(*movement_spread_degrees)
        && weapon.spread_vertical_bias
            == if spread_vertical_bias.is_finite() {
                spread_vertical_bias.clamp(0.0, 1.0)
            } else {
                0.0
            }
        && weapon.resolution == *resolution
        && weapon.lower_ms == *lower_ms
        && weapon.raise_ms == *raise_ms
        && weapon.block_during_reload == *block_during_reload
        && resource_matches
        && activation_matches(&weapon.primary, primary)
        && match (weapon.secondary.as_deref(), secondary.as_ref()) {
            (None, None) => true,
            (Some(installed), Some(incoming)) => activation_matches(installed, incoming),
            _ => false,
        }
}

fn apply_net_wieldable_tuning(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    tuning: &WieldableTuningPayload,
) {
    let Ok(weapon) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return;
    };
    // Ordinary owner snapshots revisit every slot. An unchanged row must leave
    // captured activation work and installed programs on this instance intact.
    if net_wieldable_gameplay_matches(weapon, tuning) {
        return;
    }
    let mut weapon = weapon.clone();
    weapon.cancel_activation();
    weapon.range = tuning.range;
    weapon.pellet_count = tuning.pellet_count.clamp(1, MAX_PELLET_COUNT);
    weapon.spread_degrees = if tuning.spread_degrees.is_finite() {
        tuning
            .spread_degrees
            .clamp(0.0, MAX_EFFECTIVE_SPREAD_DEGREES)
    } else {
        0.0
    };
    weapon.bloom_per_shot_degrees = clamp_tuning_degrees(tuning.bloom_per_shot_degrees);
    weapon.bloom_max_degrees = clamp_tuning_degrees(tuning.bloom_max_degrees);
    weapon.bloom_decay_degrees_per_second =
        clamp_tuning_nonnegative(tuning.bloom_decay_degrees_per_second);
    weapon.bloom_decay_delay_ms = clamp_tuning_nonnegative(tuning.bloom_decay_delay_ms);
    weapon.movement_spread_degrees = clamp_tuning_degrees(tuning.movement_spread_degrees);
    weapon.spread_vertical_bias = if tuning.spread_vertical_bias.is_finite() {
        tuning.spread_vertical_bias.clamp(0.0, 1.0)
    } else {
        0.0
    };
    weapon.resolution = tuning.resolution;
    weapon.lower_ms = tuning.lower_ms;
    weapon.raise_ms = tuning.raise_ms;
    let mut descriptor = tuning.weapon_descriptor();
    // Sound keys stay local presentation; every gameplay action field comes
    // from the host row. Preserve only lanes that exist in the host action set.
    descriptor.sounds = weapon.sounds.as_deref().cloned();
    descriptor.primary.sounds = weapon.primary.sounds.clone();
    if let Some(secondary) = &mut descriptor.secondary {
        secondary.sounds = weapon
            .secondary
            .as_ref()
            .and_then(|local| local.sounds.clone());
    }
    // Keep the established clamp boundary for spread while validating new action/stat data.
    descriptor.pellet_count = weapon.pellet_count;
    descriptor.spread_degrees = weapon.spread_degrees;
    descriptor.bloom_per_shot_degrees = weapon.bloom_per_shot_degrees;
    descriptor.bloom_max_degrees = weapon.bloom_max_degrees;
    descriptor.bloom_decay_degrees_per_second = weapon.bloom_decay_degrees_per_second;
    descriptor.bloom_decay_delay_ms = weapon.bloom_decay_delay_ms;
    descriptor.movement_spread_degrees = weapon.movement_spread_degrees;
    descriptor.spread_vertical_bias = weapon.spread_vertical_bias;
    let descriptor = match descriptor.validate() {
        Ok(descriptor) => descriptor,
        Err(error) => {
            log::warn!("[Net] rejected weapon activation tuning: {error}");
            return;
        }
    };
    weapon.refresh_from_descriptor(&descriptor);
    let _ = registry.set_component(weapon_id, weapon);
}

fn clamp_tuning_degrees(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_EFFECTIVE_SPREAD_DEGREES)
    } else {
        0.0
    }
}

fn clamp_tuning_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// Materialize the presentation-only components for a client's remote descriptor
/// entity. A connected client does not simulate the remote entity's authoritative
/// state: the host owns its movement, AI (when any), combat, health, and despawn,
/// while the client receives a finite Transform plus optional mesh animation state.
/// The client attaches only the descriptor's presentation surface locally.
///
/// This attaches ONLY the descriptor's mesh block (`MeshComponent`, including its
/// declared animation states + default state and any descriptor-driven render-origin
/// offset, via the same path `attach_descriptor_components` uses). It deliberately
/// attaches NONE of `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement`: those
/// are host-authoritative and a client carries no shadow copy.
///
/// `entity_class` is the descriptor class the host stamped on the wire. An
/// unregistered class, or a descriptor with no mesh block, leaves the entity
/// transform-only (logged, not rejected) — a remote entity with no mesh simply does
/// not render, exactly as a stateless transform.
///
/// Idempotent: an entity already carrying a `MeshComponent` is left untouched, so a
/// re-baseline / re-apply does not duplicate or reset the live mesh animation state.
///
/// Returns `true` if a mesh presentation is now present (materialized this call or
/// already there), `false` if the descriptor is unregistered or has no mesh block.
pub fn materialize_net_mesh_presentation(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
    agent_params: Option<NavAgentParams>,
) -> bool {
    // Idempotent: never clobber a live mesh component (and its animation state) on
    // a re-apply.
    if matches!(
        registry.has_component_kind(id, ComponentKind::Mesh),
        Ok(true)
    ) {
        return true;
    }

    let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
        log::warn!(
            "[Net] remote entity_class `{entity_class}` not registered; \
             leaving remote entity transform-only (will not render)"
        );
        return false;
    };
    if descriptor.mesh.is_none() {
        log::debug!(
            "[Net] remote entity_class `{entity_class}` has no mesh block; \
             leaving remote entity transform-only (will not render)"
        );
        return false;
    };

    let component = descriptor_mesh_component(descriptor, agent_params)
        .expect("descriptor mesh presence checked above");
    // `set_component` only fails on a stale id; the caller proved the pawn live.
    let _ = registry.set_component(id, component);
    true
}

/// Attach a descriptor display copy's complete presentation set independently.
/// Existing components retain applied network state across retries and re-baselines.
pub fn materialize_net_descriptor_presentation(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
    agent_params: Option<NavAgentParams>,
) -> bool {
    use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::hit_volume::HitVolumeComponent;
    use postretro_entities::components::light::LightComponent;
    use postretro_entities::components::mesh::MeshComponent;

    let Ok(transform) = registry.get_component::<Transform>(id).copied() else {
        return false;
    };
    let Some(descriptor) = find_descriptor(descriptors, entity_class) else {
        log::warn!(
            "[Net] remote entity_class `{entity_class}` not registered; presentation incomplete"
        );
        return false;
    };
    let mut complete = true;
    if registry.get_component::<MeshComponent>(id).is_err()
        && let Some(component) = descriptor_mesh_component(descriptor, agent_params)
    {
        complete &= registry.set_component(id, component).is_ok();
    }
    if registry.get_component::<LightComponent>(id).is_err()
        && let Some(light) = descriptor.light.as_ref()
    {
        complete &= registry
            .set_component(id, descriptor_light_component(light, transform.position))
            .is_ok();
    }
    if registry
        .get_component::<BillboardEmitterComponent>(id)
        .is_err()
        && let Some(emitter) = descriptor.emitter.as_ref()
    {
        complete &= registry.set_component(id, emitter.clone()).is_ok();
    }
    if registry.get_component::<HitVolumeComponent>(id).is_err()
        && let Some(health) = descriptor.health.as_ref()
        && let Some(mut hitbox) = HealthComponent::from_descriptor(health).hitbox
    {
        hitbox.offset -= ai_capsule_center_from_feet_offset(descriptor, agent_params);
        complete &= registry
            .set_component(id, HitVolumeComponent { hitbox })
            .is_ok();
    }
    complete
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use log::Level;
    use postretro_entities::AmmoReserve;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_scripting_core::data_descriptors::{
        AirParams, AmmoResource, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
        BehaviorGraphEnvelope, CapsuleParams, FallParams, GroundParams, MeshDescriptor, MotionVerb,
        PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams, TouchMode,
        TouchableDescriptor, WeaponDescriptor, WeaponResource,
    };
    use postretro_test_log_capture::LogCapture;
    use std::collections::HashMap;

    /// These fixtures exercise pawn tuning; the level state is incidental.
    fn test_environment() -> postretro_combat_model::EnvironmentTuning {
        postretro_combat_model::EnvironmentTuning {
            level_gravity: -9.81,
            fluids: Default::default(),
        }
    }

    /// Minimal in-memory descriptor carrying only a mesh block. `animated` selects
    /// between a stateless mesh (model only) and a two-state animated mesh
    /// (`idle` default + `attack`), mirroring the validated descriptor shape.
    fn enemy_mesh_descriptor(classname: &str, animated: bool) -> EntityTypeDescriptor {
        let (animations, default_state) = if animated {
            let mut states = HashMap::new();
            states.insert(
                "idle".to_string(),
                AnimationState {
                    clip: "idle_clip".to_string(),
                    looping: true,
                    crossfade_ms: 150.0,
                    interrupt: InterruptPolicy::Smooth,
                    travel_speed: None,
                    clip_index: None,
                },
            );
            states.insert(
                "attack".to_string(),
                AnimationState {
                    clip: "attack_clip".to_string(),
                    looping: false,
                    crossfade_ms: 0.0,
                    interrupt: InterruptPolicy::Snap,
                    travel_speed: None,
                    clip_index: None,
                },
            );
            (states, Some("idle".to_string()))
        } else {
            (HashMap::new(), None)
        };

        EntityTypeDescriptor {
            ghost: false,
            faction: None,
            tolerance: None,
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: Some(MeshDescriptor {
                model: "decraniated".to_string(),
                shadow_only: false,
                attachments: Default::default(),
                shadow_bias_scale: 1.0,
                animations,
                default_state,
                locomotion: None,
            }),
            health: None,
            behavior: None,
        }
    }

    /// Spawn a bare transform-only entity, the wire baseline a remote enemy starts
    /// from before presentation is materialized.
    fn spawn_transform_only(reg: &mut EntityRegistry) -> EntityId {
        reg.try_spawn(
            Transform {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            &[],
        )
        .expect("registry has room for one entity")
    }

    fn display_descriptor(mesh: bool, hitbox: bool) -> EntityTypeDescriptor {
        let mut descriptor = enemy_mesh_descriptor("display", true);
        if !mesh {
            descriptor.mesh = None;
        }
        descriptor.health = Some(
            postretro_scripting_core::data_descriptors::HealthDescriptor {
                max: 100.0,
                hitbox: hitbox.then_some(
                    postretro_scripting_core::data_descriptors::HitboxDescriptor {
                        half_extents: [0.5; 3],
                        offset: Some([0.0, 1.0, 0.0]),
                    },
                ),
                zone_multipliers: Default::default(),
            },
        );
        descriptor
    }

    #[test]
    fn descriptor_display_set_attaches_independently_and_preserves_live_state() {
        use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
        use postretro_entities::components::hit_volume::HitVolumeComponent;
        use postretro_entities::components::light::LightComponent;
        let mut descriptor = display_descriptor(true, true);
        descriptor.light = Some(
            postretro_scripting_core::data_descriptors::LightDescriptor {
                color: [0.2, 0.4, 0.6],
                intensity: 3.0,
                range: 7.0,
                is_dynamic: false,
            },
        );
        descriptor.emitter = Some(BillboardEmitterComponent {
            rate: 2.0,
            burst: None,
            spread: 0.5,
            lifetime: 1.0,
            velocity: [0.0; 3],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![1.0].into(),
            opacity_over_lifetime: vec![1.0].into(),
            color: [1.0; 3],
            sprite: "spark".into(),
            spin_rate: 0.0,
            spin_animation: None,
        });
        descriptor.behavior = Some(BehaviorGraphDescriptor {
            knockback: Default::default(),
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: std::collections::BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        sound: None,
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: std::collections::BTreeMap::new(),
                    },
                )]),
                transitions: std::collections::BTreeMap::new(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed: 3.5,
        });
        descriptor.weapon = weapon_descriptor("unused").weapon;
        descriptor.movement = player_with_movement("unused").movement;
        descriptor.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Press,
            radius: 1.0,
        });
        let mut registry = EntityRegistry::new();
        let id = spawn_transform_only(&mut registry);
        let descriptors = [descriptor];
        assert!(materialize_net_descriptor_presentation(
            "display",
            &descriptors,
            &mut registry,
            id,
            None
        ));
        let light = registry.get_component::<LightComponent>(id).unwrap();
        assert!(Vec3::from_array(light.origin).distance(Vec3::new(1.0, 2.0, 3.0)) < 1e-5);
        assert!(light.is_dynamic);
        assert!((light.intensity - 3.0).abs() < 1e-5);
        assert!(
            registry
                .get_component::<HitVolumeComponent>(id)
                .unwrap()
                .hitbox
                .offset
                .distance(Vec3::Y - ai_capsule_center_from_feet_offset(&descriptors[0], None))
                < 1e-5
        );
        for kind in [
            ComponentKind::Health,
            ComponentKind::Brain,
            ComponentKind::Agent,
            ComponentKind::Weapon,
            ComponentKind::Touchable,
            ComponentKind::PlayerMovement,
            ComponentKind::DescriptorProvenance,
        ] {
            assert!(
                !registry.has_component_kind(id, kind).unwrap(),
                "unexpected {kind:?}"
            );
        }
        let mut light = light.clone();
        light.intensity = 8.0;
        registry.set_component(id, light).unwrap();
        let mut emitter = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .clone();
        emitter.rate = 0.0;
        registry.set_component(id, emitter).unwrap();
        let mut mesh = registry.get_component::<MeshComponent>(id).unwrap().clone();
        mesh.animation.as_mut().unwrap().current_state = "attack".into();
        registry.set_component(id, mesh.clone()).unwrap();
        registry.remove_component::<HitVolumeComponent>(id).unwrap();
        assert!(materialize_net_descriptor_presentation(
            "display",
            &descriptors,
            &mut registry,
            id,
            None
        ));
        assert!(
            (registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .intensity
                - 8.0)
                .abs()
                < 1e-5
        );
        assert!(
            registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .rate
                .abs()
                < 1e-5
        );
        assert_eq!(registry.get_component::<MeshComponent>(id).unwrap(), &mesh);
        assert!(registry.get_component::<HitVolumeComponent>(id).is_ok());
    }

    #[test]
    fn descriptor_display_set_completes_empty_and_meshless_but_rejects_unknown_or_stale() {
        let mut registry = EntityRegistry::new();
        let id = spawn_transform_only(&mut registry);
        let mut descriptor = display_descriptor(false, false);
        descriptor.health = None;
        assert!(materialize_net_descriptor_presentation(
            "display",
            &[descriptor.clone()],
            &mut registry,
            id,
            None
        ));
        assert!(!materialize_net_descriptor_presentation(
            "unknown",
            &[descriptor.clone()],
            &mut registry,
            id,
            None
        ));
        registry.despawn(id).unwrap();
        assert!(!materialize_net_descriptor_presentation(
            "display",
            &[descriptor],
            &mut registry,
            id,
            None
        ));
    }

    #[test]
    fn descriptor_display_hit_volume_targets_ray_without_damage_splash_or_policy() {
        use crate::scripting_systems::hit_zones::{HitZoneStore, nearest_entity_hit};
        use crate::sim::splash::entities_in_sphere;
        use crate::weapon::{ActivationOutcome, DamagePayload, WeaponImpact};
        use postretro_entities::components::health::{
            DamageContext, DamageProducer, apply_damage_with_context,
        };
        use postretro_entities::components::hit_volume::HitVolumeComponent;
        for mesh in [false, true] {
            for authored_hitbox in [false, true] {
                let mut registry = EntityRegistry::new();
                let id = registry.spawn(Transform::default());
                let descriptor = display_descriptor(mesh, authored_hitbox);
                assert!(materialize_net_descriptor_presentation(
                    "display",
                    &[descriptor],
                    &mut registry,
                    id,
                    None
                ));
                let store = HitZoneStore::new();
                let hit = nearest_entity_hit(
                    &registry,
                    &store,
                    0.0,
                    Vec3::new(0.0, 1.0, -3.0),
                    Vec3::Z,
                    10.0,
                    0.0,
                );
                assert_eq!(hit.map(|hit| hit.target), authored_hitbox.then_some(id));
                assert!(
                    nearest_entity_hit(
                        &registry,
                        &store,
                        0.0,
                        Vec3::new(2.0, 1.0, -3.0),
                        Vec3::Z,
                        10.0,
                        0.0
                    )
                    .is_none()
                );
                assert!(
                    entities_in_sphere(&registry, &store, Vec3::Y, 5.0, |_| false, None, Vec3::Y)
                        .is_empty()
                );
                let payload = DamagePayload {
                    amount: 10.0,
                    impulse: Vec3::ZERO,
                };
                assert!(!apply_damage_with_context(
                    &mut registry,
                    id,
                    &payload,
                    DamageContext::new("test", DamageProducer::InTick)
                ));
                let impact = WeaponImpact {
                    point: Vec3::Y,
                    normal: -Vec3::Z,
                    target: Some(id),
                    zone: None,
                    outcome: ActivationOutcome::Hit(payload),
                };
                crate::sim::weapon_stage::apply_authorized_weapon_impact_damage(
                    &mut registry,
                    id,
                    None,
                    &impact,
                    "test".into(),
                    10.0,
                );
                assert!(registry.take_impact_dispatches().is_empty());
                assert!(registry.get_component::<HealthComponent>(id).is_err());
                assert!(
                    registry
                        .get_component::<postretro_entities::components::brain::BrainComponent>(id)
                        .is_err()
                );
                assert_eq!(
                    registry.get_component::<HitVolumeComponent>(id).is_ok(),
                    authored_hitbox
                );
            }
        }
    }

    #[test]
    fn remote_enemy_presentation_attaches_animated_mesh_only() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob", true)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        let attached =
            materialize_net_mesh_presentation("decraniated_mob", &descriptors, &mut reg, id, None);
        assert!(
            attached,
            "mesh-bearing descriptor materializes presentation"
        );

        let mesh = reg
            .get_component::<MeshComponent>(id)
            .expect("remote enemy renders its descriptor mesh");
        assert_eq!(mesh.model, "decraniated");
        let animation = mesh
            .animation
            .as_ref()
            .expect("animated descriptor carries declared animation states");
        assert_eq!(
            animation.default_state, "idle",
            "default animation state copied from the descriptor"
        );
        assert_eq!(
            animation.current_state, "idle",
            "spawn enters the default state"
        );
        assert_eq!(
            animation.states.len(),
            2,
            "both declared states are copied in"
        );
        assert_eq!(
            mesh.origin_offset,
            glam::Vec3::ZERO,
            "non-AI mesh-only descriptors keep a zero render-origin offset"
        );
    }

    #[test]
    fn remote_enemy_presentation_materializes_locomotion_contract() {
        use postretro_scripting_core::data_descriptors::LocomotionDescriptor;

        let mut descriptor = enemy_mesh_descriptor("decraniated_mob", true);
        let mesh_desc = descriptor.mesh.as_mut().unwrap();
        mesh_desc.locomotion = Some(LocomotionDescriptor { speed_scale: false });
        mesh_desc.animations.get_mut("idle").unwrap().travel_speed = Some(2.75);
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        assert!(materialize_net_mesh_presentation(
            "decraniated_mob",
            &[descriptor],
            &mut reg,
            id,
            None,
        ));

        let animation = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert!(!animation.speed_scale);
        assert_eq!(animation.states["idle"].travel_speed, Some(2.75));
    }

    #[test]
    fn remote_enemy_presentation_offsets_ai_mesh_from_capsule_center_to_feet() {
        let mut descriptor = enemy_mesh_descriptor("decraniated_mob", true);
        descriptor.behavior = Some(BehaviorGraphDescriptor {
            knockback: Default::default(),
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: std::collections::BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        sound: None,
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: std::collections::BTreeMap::new(),
                    },
                )]),
                transitions: std::collections::BTreeMap::new(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed: 3.5,
        });
        let descriptors = vec![descriptor];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let params = NavAgentParams {
            radius: 0.4,
            height: 1.6,
            step_height: 0.3,
            max_slope_deg: 45.0,
        };

        assert!(materialize_net_mesh_presentation(
            "decraniated_mob",
            &descriptors,
            &mut reg,
            id,
            Some(params)
        ));

        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(
            mesh.origin_offset,
            postretro_entities::components::mesh::capsule_center_to_feet_origin_offset(
                params.radius,
                params.height,
            ),
            "client remote AI presentation uses the same capsule-center to feet offset as host materialization"
        );
    }

    #[test]
    fn remote_enemy_presentation_attaches_stateless_mesh() {
        let descriptors = vec![enemy_mesh_descriptor("prop_enemy", false)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        assert!(materialize_net_mesh_presentation(
            "prop_enemy",
            &descriptors,
            &mut reg,
            id,
            None
        ));
        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        assert_eq!(mesh.model, "decraniated");
        assert!(
            mesh.animation.is_none(),
            "descriptor with no animations yields a stateless mesh"
        );
    }

    #[test]
    fn remote_enemy_presentation_never_attaches_authoritative_components() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob", true)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        materialize_net_mesh_presentation("decraniated_mob", &descriptors, &mut reg, id, None);

        // A connected client carries no hidden authoritative state for a remote
        // enemy: only presentation (mesh) is attached.
        for kind in [
            ComponentKind::Brain,
            ComponentKind::Agent,
            ComponentKind::Health,
            ComponentKind::Weapon,
            ComponentKind::PlayerMovement,
        ] {
            assert_eq!(
                reg.has_component_kind(id, kind),
                Ok(false),
                "remote enemy presentation must not attach {kind:?}"
            );
        }
    }

    #[test]
    fn remote_enemy_presentation_is_idempotent() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob", true)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        assert!(materialize_net_mesh_presentation(
            "decraniated_mob",
            &descriptors,
            &mut reg,
            id,
            None
        ));

        // Mutate the live animation state so a second call that reset it would be
        // observable. (A re-apply must NOT clobber runtime state.)
        {
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            let animation = mesh.animation.as_mut().unwrap();
            animation.current_state = "attack".to_string();
            reg.set_component(id, mesh).unwrap();
        }

        assert!(
            materialize_net_mesh_presentation("decraniated_mob", &descriptors, &mut reg, id, None),
            "a second apply reports presentation present"
        );

        let mesh = reg.get_component::<MeshComponent>(id).unwrap();
        let animation = mesh.animation.as_ref().unwrap();
        assert_eq!(
            animation.current_state, "attack",
            "second apply must not reset live animation state"
        );
    }

    #[test]
    fn remote_enemy_presentation_unknown_class_leaves_transform_only() {
        let descriptors = vec![enemy_mesh_descriptor("decraniated_mob", true)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let capture = LogCapture::start();

        let attached =
            materialize_net_mesh_presentation("not_a_class", &descriptors, &mut reg, id, None);
        assert!(!attached, "unknown class attaches nothing");
        assert_eq!(
            reg.has_component_kind(id, ComponentKind::Mesh),
            Ok(false),
            "unknown class leaves the entity transform-only"
        );
        capture.assert_logged(
            Level::Warn,
            "[Net] remote entity_class `not_a_class` not registered",
        );
    }

    #[test]
    fn remote_enemy_presentation_meshless_descriptor_leaves_transform_only() {
        let descriptors = vec![EntityTypeDescriptor {
            ghost: false,
            touchable: None,
            mesh: None,
            ..enemy_mesh_descriptor("meshless_enemy", false)
        }];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);
        let capture = LogCapture::start();

        let attached =
            materialize_net_mesh_presentation("meshless_enemy", &descriptors, &mut reg, id, None);

        assert!(!attached, "meshless descriptor attaches nothing");
        assert_eq!(
            reg.has_component_kind(id, ComponentKind::Mesh),
            Ok(false),
            "meshless descriptor leaves the entity transform-only"
        );
        capture.assert_logged(
            Level::Debug,
            "[Net] remote entity_class `meshless_enemy` has no mesh block",
        );
    }

    // --- spawn_net_slot_pawn (M15 Phase 3 Task 4) ----------------------------

    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            sounds: None,
            knockback: Default::default(),
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        }
    }

    fn player_with_movement(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            ghost: false,
            faction: None,
            tolerance: None,
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: Some(movement_descriptor()),
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn player_with_default_weapon(classname: &str, default_weapon: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            ghost: false,
            faction: None,
            tolerance: None,
            canonical_name: Some(classname.to_string()),
            inventory: Some(postretro_entities::InventoryDescriptor {
                loadout: vec![default_weapon.to_string()],
            }),
            light: None,
            emitter: None,
            movement: Some(movement_descriptor()),
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            ghost: false,
            faction: None,
            tolerance: None,
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                sounds: None,
                knockback: None,
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                bloom_per_shot_degrees: 0.0,
                bloom_max_degrees: 0.0,
                bloom_decay_degrees_per_second: 0.0,
                bloom_decay_delay_ms: 0.0,
                movement_spread_degrees: 0.0,
                spread_vertical_bias: 0.0,
                range: 64.0,
                primary: postretro_foundation::WeaponActivationDescriptor::single(
                    postretro_foundation::ActivationTrigger::Press,
                    100.0,
                ),
                secondary: None,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                splash: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn touchable_mesh_weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = weapon_descriptor(classname);
        descriptor.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 1.0,
        });
        descriptor.mesh = Some(MeshDescriptor {
            model: format!("models/{classname}/world.gltf"),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: Default::default(),
            default_state: None,
            locomotion: None,
        });
        descriptor
    }

    fn ammo_weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        let mut descriptor = weapon_descriptor(classname);
        descriptor.weapon.as_mut().unwrap().resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "bullets.light".to_string(),
            magazine: 12,
            cost_per_shot: 1,
            reserve: 48,
            reload_ms: 900,
            reload_style: ReloadStyle::Magazine,
        }));
        descriptor
    }

    fn spawn_point(kvps: &[(&str, &str)]) -> MapEntity {
        let mut kv = HashMap::new();
        for (k, v) in kvps {
            kv.insert((*k).to_string(), (*v).to_string());
        }
        MapEntity {
            map_ordinal: None,
            classname: "player_spawn".to_string(),
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            key_values: kv,
            tags: vec![],
        }
    }

    fn spawn_point_at(origin: Vec3, angles: Vec3, kvps: &[(&str, &str)]) -> MapEntity {
        let mut e = spawn_point(kvps);
        e.origin = origin;
        e.angles = angles;
        e
    }

    #[test]
    fn net_slot_pawn_restores_carried_health_after_descriptor_spawn() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_scripting_core::data_descriptors::HealthDescriptor;

        let mut player = player_with_movement("player");
        player.health = Some(HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        let mut registry = EntityRegistry::new();

        let carried = CarriedState {
            health_current: Some(29.0),
            ..Default::default()
        };
        let pawn = spawn_net_slot_pawn_with_carried_loadout(
            &spawn_point(&[]),
            &[player],
            &mut registry,
            None,
            Some(&carried),
        )
        .expect("descriptor-backed net slot pawn spawns");

        let health = registry
            .get_component::<HealthComponent>(pawn)
            .expect("descriptor materialized health")
            .current;
        assert!(
            (health - 29.0).abs() <= 1.0e-6,
            "expected carried health 29.0, got {health}"
        );
    }

    /// Host and client tuning rows for slot 0 `shotgun` and slot 2 `plasma`,
    /// with slot 1 deliberately empty.
    fn shotgun_and_plasma_tuning(spawn_active_slot: usize) -> TuningPayload {
        let mut tuning = tuning_for_slot(0, "shotgun", 40.0, 600.0, 50, 50);
        tuning.wieldables[2] =
            tuning_for_slot(2, "plasma", 90.0, 120.0, 50, 50).wieldables[2].take();
        tuning.with_spawn_active_slot(spawn_active_slot)
    }

    fn shotgun_and_plasma_descriptors() -> Vec<EntityTypeDescriptor> {
        vec![
            player_with_movement("player"),
            weapon_descriptor("shotgun"),
            weapon_descriptor("plasma"),
        ]
    }

    // Regression: after a level change the host restored the carried selection
    // (plasma, slot 2) while the client composed from the same slot layout and
    // started on the first populated slot (shotgun). The client then fired and
    // reloaded "the shotgun" against a host projecting the plasma rifle.
    #[test]
    fn client_install_composes_the_host_carried_spawn_active_slot() {
        let descriptors = shotgun_and_plasma_descriptors();
        let mut host = EntityRegistry::new();
        let carried = CarriedState {
            wieldables: std::array::from_fn(|slot| match slot {
                0 => Some("shotgun".to_string()),
                2 => Some("plasma".to_string()),
                _ => None,
            }),
            active_slot: 2,
            ..Default::default()
        };
        let host_pawn = spawn_net_slot_pawn_with_carried_loadout(
            &spawn_point(&[]),
            &descriptors,
            &mut host,
            None,
            Some(&carried),
        )
        .expect("descriptor-backed net slot pawn spawns");
        let host_inventory = host.get_component::<Inventory>(host_pawn).unwrap();
        assert_eq!(host_inventory.active_slot, 2);
        assert_eq!(host_inventory.spawn_active_slot, 2);

        let tuning = shotgun_and_plasma_tuning(host_inventory.spawn_active_slot);
        let mut client = EntityRegistry::new();
        let client_pawn = client.spawn(Transform::default());
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut client,
            client_pawn,
            Some(&tuning),
        ));
        let client_inventory = client.get_component::<Inventory>(client_pawn).unwrap();
        assert_eq!(
            client_inventory.active_slot, 2,
            "client starts on the host's slot"
        );
        assert_eq!(
            client
                .get_component::<DescriptorProvenance>(client_inventory.active_wieldable().unwrap())
                .unwrap()
                .canonical_name,
            "plasma"
        );
        assert_eq!(client_inventory.spawn_active_slot, 2);
    }

    // A spawn slot naming an empty or out-of-range position composes like a
    // fresh inventory: the first populated slot.
    #[test]
    fn client_install_with_an_unpopulated_spawn_slot_starts_on_the_first_populated_slot() {
        let descriptors = shotgun_and_plasma_descriptors();
        for spawn_active_slot in [1, WIELDABLE_SLOT_CAPACITY, usize::MAX] {
            let mut client = EntityRegistry::new();
            let pawn = client.spawn(Transform::default());
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut client,
                pawn,
                Some(&shotgun_and_plasma_tuning(spawn_active_slot)),
            ));
            let inventory = client.get_component::<Inventory>(pawn).unwrap();
            assert_eq!(inventory.active_slot, 0, "spawn slot {spawn_active_slot}");
            assert_eq!(inventory.spawn_active_slot, 0);
        }
    }

    fn ammo_weapon_descriptor_of(
        classname: &str,
        ammo_type: &str,
        magazine: u32,
        reserve: u32,
    ) -> EntityTypeDescriptor {
        let mut descriptor = weapon_descriptor(classname);
        descriptor.weapon.as_mut().unwrap().resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: ammo_type.to_string(),
            magazine,
            cost_per_shot: 1,
            reserve,
            reload_ms: 900,
            reload_style: ReloadStyle::Magazine,
        }));
        descriptor
    }

    /// A host payload naming `slots`, each row carrying its descriptor's resource.
    fn payload_for(
        descriptors: &[EntityTypeDescriptor],
        slots: &[(usize, &str)],
        spawn_active_slot: usize,
    ) -> TuningPayload {
        let mut payload =
            TuningPayload::new(None, std::array::from_fn(|_| None), test_environment());
        for &(slot, name) in slots {
            let mut row = tuning_for_slot(slot, name, 64.0, 100.0, 0, 0).wieldables[slot]
                .take()
                .unwrap();
            row.resource = find_descriptor(descriptors, name)
                .and_then(|descriptor| descriptor.weapon.as_ref())
                .and_then(|weapon| weapon.resource.clone());
            payload.wieldables[slot] = Some(row);
        }
        payload.with_spawn_active_slot(spawn_active_slot)
    }

    /// A client whose own player loadout is a shell weapon the host never
    /// names, beside the host's two bullet weapons.
    fn differing_loadout_descriptors() -> Vec<EntityTypeDescriptor> {
        vec![
            player_with_default_weapon("player", "local_shotgun"),
            ammo_weapon_descriptor_of("local_shotgun", "shells.buck", 8, 32),
            ammo_weapon_descriptor_of("host_pistol", "bullets.light", 12, 48),
            ammo_weapon_descriptor_of("host_rifle", "bullets.rifle", 30, 120),
        ]
    }

    const HOST_SLOTS: [(usize, &str); 2] = [(0, "host_pistol"), (1, "host_rifle")];

    fn magazines(reg: &EntityRegistry, pawn: EntityId) -> Vec<Option<u32>> {
        reg.get_component::<Inventory>(pawn)
            .unwrap()
            .wieldables
            .iter()
            .map(|weapon| {
                weapon.map(|weapon| {
                    reg.get_component::<WeaponComponent>(weapon)
                        .unwrap()
                        .magazine
                })
            })
            .collect()
    }

    // Regression: a pawn baseline that armed before the host's payload composed
    // from the client's own descriptor's loadout. It now composes nothing until
    // the payload arrives, which composes the host's layout on its spawn slot.
    #[test]
    fn client_arm_before_the_payload_composes_nothing_then_the_payload_composes_the_host_layout() {
        let descriptors = differing_loadout_descriptors();
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());

        assert!(!materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            None,
        ));
        assert_eq!(
            reg.has_component_kind(pawn, ComponentKind::Inventory),
            Ok(false)
        );
        assert_eq!(
            reg.iter_with_kind(ComponentKind::Weapon).count(),
            0,
            "nothing from the client's own loadout"
        );
        assert!(reg.get_component::<AmmoReserve>(pawn).is_err());

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &HOST_SLOTS, 1)),
        ));
        let inventory = reg.get_component::<Inventory>(pawn).unwrap();
        assert_eq!((inventory.active_slot, inventory.spawn_active_slot), (1, 1));
        let names: Vec<_> = inventory
            .wieldables
            .iter()
            .map(|weapon| {
                weapon.map(|weapon| {
                    reg.get_component::<DescriptorProvenance>(weapon)
                        .unwrap()
                        .canonical_name
                        .clone()
                })
            })
            .collect();
        let mut expected = vec![None; WIELDABLE_SLOT_CAPACITY];
        expected[0] = Some("host_pistol".to_string());
        expected[1] = Some("host_rifle".to_string());
        assert_eq!(names, expected, "the host's slot layout");
    }

    // Regression: own-descriptor composition seeded reserve for the client's
    // own loadout, and the late payload's merge never removed it. A late
    // payload now composes exactly what the payload-first path does.
    #[test]
    fn late_payload_seeds_reserve_and_magazines_as_the_payload_first_path() {
        let descriptors = differing_loadout_descriptors();
        let payload = payload_for(&descriptors, &HOST_SLOTS, 1);

        let mut first = EntityRegistry::new();
        let first_pawn = first.spawn(Transform::default());
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut first,
            first_pawn,
            Some(&payload),
        ));

        let mut late = EntityRegistry::new();
        let late_pawn = late.spawn(Transform::default());
        materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut late,
            late_pawn,
            None,
        );
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut late,
            late_pawn,
            Some(&payload),
        ));

        let reserve = late.get_component::<AmmoReserve>(late_pawn).unwrap();
        assert_eq!(
            reserve,
            first.get_component::<AmmoReserve>(first_pawn).unwrap()
        );
        assert_eq!(reserve.available("bullets.light"), 48);
        assert_eq!(reserve.available("bullets.rifle"), 120);
        assert_eq!(reserve.available("shells.buck"), 0, "no own-loadout ammo");
        assert_eq!(magazines(&late, late_pawn), magazines(&first, first_pawn));
        assert_eq!(&magazines(&late, late_pawn)[..2], [Some(12), Some(30)]);
    }

    fn tuning_for_slot(
        slot: usize,
        canonical_name: &str,
        range: f32,
        cooldown_ms: f32,
        lower_ms: u32,
        raise_ms: u32,
    ) -> TuningPayload {
        let mut wieldables = std::array::from_fn(|_| None);
        wieldables[slot] = Some(WieldableTuningPayload {
            canonical_name: canonical_name.to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Hold,
                cooldown_ms,
            ),
            secondary: None,
            damage: 10.0,
            knockback: None,
            projectile: None,
            splash: None,
            resource: None,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            resolution: ResolutionMode::Hitscan,
            lower_ms,
            raise_ms,
            block_during_reload: None,
        });
        TuningPayload::new(None, wieldables, test_environment())
    }

    #[test]
    fn net_wieldable_repeat_tuning_preserves_burst_and_charge_without_allocations() {
        use crate::weapon::FireButtonState;
        use crate::weapon::execution::{ActivationCommand, advance_weapon_activation};
        use postretro_foundation::{
            ActivationCharge, ActivationInput, ActivationLane, ActivationRelease, ActivationToken,
            ActivationTrigger, WeaponActivationDescriptor,
        };
        use std::sync::Arc;

        let burst: WeaponActivationDescriptor = serde_json::from_value(serde_json::json!({
            "trigger": "press", "recoveryMs": 300.0,
            "steps": [
                {"kind": "shot"}, {"kind": "wait", "durationMs": 80.0},
                {"kind": "shot"}, {"kind": "wait", "durationMs": 80.0},
                {"kind": "shot"}
            ],
            "emits": {"activate": "burst.activate"}
        }))
        .unwrap();
        let mut charge = WeaponActivationDescriptor::single(ActivationTrigger::Press, 300.0);
        charge.charge = Some(ActivationCharge {
            min_ms: 200.0,
            full_ms: 1000.0,
            interrupted_by: None,
        });
        let token = ActivationToken {
            start_tick: 10,
            lane: ActivationLane::Secondary,
        };

        for (action, release_tick, expected_shots) in [(burst, None, 3), (charge, Some(25), 1)] {
            let mut registry = EntityRegistry::new();
            let weapon_id = registry.spawn(Transform::default());
            let mut descriptor = ammo_weapon_descriptor("reference_pistol");
            let local = descriptor.weapon.as_mut().unwrap();
            local.secondary = Some(action.clone());
            local.secondary.as_mut().unwrap().sounds =
                Some(postretro_foundation::ActivationSounds {
                    fire: Some("sfx/local-secondary".into()),
                    impact: None,
                });
            let mut weapon = WeaponComponent::from_descriptor(local);
            weapon.magazine = 5;
            registry.set_component(weapon_id, weapon).unwrap();
            let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);
            let row = tuning.wieldables[0].as_mut().unwrap();
            row.secondary = Some(action);
            row.resource = local.resource.clone();
            row.bloom_per_shot_degrees = 1.0;
            row.bloom_max_degrees = 6.0;
            apply_net_wieldable_tuning(&mut registry, weapon_id, row);
            let mut shots = Vec::new();

            for tick in 10..=30 {
                let mut weapon = registry
                    .get_component::<WeaponComponent>(weapon_id)
                    .unwrap()
                    .clone();
                let advance = advance_weapon_activation(
                    &mut weapon,
                    ActivationCommand {
                        tick,
                        pawn: 7,
                        real_command: true,
                        input: ActivationInput {
                            initiation: (tick == 10).then_some(token),
                            release: (release_tick == Some(tick)).then_some(ActivationRelease {
                                token,
                                release_tick: tick,
                            }),
                            cancel: None,
                        },
                        controller_starts: false,
                        primary: FireButtonState {
                            pressed: false,
                            active: false,
                        },
                        secondary: FireButtonState {
                            pressed: tick == 10,
                            active: release_tick.is_none_or(|release| tick < release),
                        },
                    },
                    1000.0 / 60.0,
                    true,
                    true,
                );
                if tick == 10 {
                    assert!(weapon.state.activation_cursor().is_some());
                }
                if let Some(shot) = advance.shot {
                    shots.push(shot);
                }
                let before = weapon.clone();
                registry.set_component(weapon_id, weapon).unwrap();
                let probe = crate::alloc_probe::AllocSnapshot::arm();
                apply_net_wieldable_tuning(&mut registry, weapon_id, row);
                apply_net_wieldable_tuning(&mut registry, weapon_id, row);
                let allocations = probe.allocs_since();
                assert_eq!(
                    allocations, 0,
                    "unchanged tuning must not rebuild installed data"
                );
                let installed = registry
                    .get_component::<WeaponComponent>(weapon_id)
                    .unwrap();
                assert_eq!(
                    installed, &before,
                    "repeat install changed live state at tick {tick}"
                );
                assert!(Arc::ptr_eq(&installed.primary, &before.primary));
                assert!(Arc::ptr_eq(
                    installed.secondary.as_ref().unwrap(),
                    before.secondary.as_ref().unwrap(),
                ));
                assert!(Arc::ptr_eq(
                    installed.activation_programs.primary.as_ref().unwrap(),
                    before.activation_programs.primary.as_ref().unwrap(),
                ));
                assert!(Arc::ptr_eq(
                    installed.activation_programs.secondary.as_ref().unwrap(),
                    before.activation_programs.secondary.as_ref().unwrap(),
                ));
            }
            assert_eq!(shots.len(), expected_shots);
            for (ordinal, shot) in shots.iter().enumerate() {
                assert_eq!(shot.shot_id.start_tick, token.start_tick);
                assert_eq!(shot.shot_id.lane, token.lane);
                assert_eq!(shot.shot_id.ordinal, ordinal as u8);
            }
            if release_tick.is_some() {
                assert!((shots[0].charge - 0.25).abs() < f32::EPSILON);
            }
        }
    }

    #[test]
    fn net_wieldable_changed_tuning_cancels_future_work_after_atomic_validation() {
        use crate::weapon::FireButtonState;
        use crate::weapon::execution::{ActivationCommand, advance_weapon_activation};
        use postretro_foundation::{ActivationInput, ActivationLane, ActivationToken};
        use std::sync::Arc;

        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);
        let row = tuning.wieldables[0].as_mut().unwrap();
        row.secondary = Some(
            serde_json::from_value(serde_json::json!({
                "trigger": "press", "recoveryMs": 300.0,
                "steps": [
                    {"kind": "shot"}, {"kind": "wait", "durationMs": 80.0}, {"kind": "shot"}
                ]
            }))
            .unwrap(),
        );
        row.resource = ammo_weapon_descriptor("reference_pistol")
            .weapon
            .unwrap()
            .resource;
        let mut weapon =
            WeaponComponent::from_descriptor(&row.weapon_descriptor().validate().unwrap());
        let advance = advance_weapon_activation(
            &mut weapon,
            ActivationCommand {
                tick: 10,
                pawn: 7,
                real_command: true,
                input: ActivationInput {
                    initiation: Some(ActivationToken {
                        start_tick: 10,
                        lane: ActivationLane::Secondary,
                    }),
                    ..Default::default()
                },
                controller_starts: false,
                primary: FireButtonState {
                    pressed: false,
                    active: false,
                },
                secondary: FireButtonState {
                    pressed: true,
                    active: true,
                },
            },
            1000.0 / 60.0,
            false,
            true,
        );
        let frozen_shot = advance.shot.unwrap();
        assert_eq!(weapon.magazine, 11);
        assert!(weapon.state.activation_cursor().is_some());
        let before = weapon.clone();
        registry.set_component(weapon_id, weapon).unwrap();

        row.damage = f32::NAN;
        apply_net_wieldable_tuning(&mut registry, weapon_id, row);
        let installed = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap();
        assert_eq!(
            installed, &before,
            "rejected tuning must not cancel live execution"
        );
        assert!(Arc::ptr_eq(
            installed.activation_programs.secondary.as_ref().unwrap(),
            before.activation_programs.secondary.as_ref().unwrap(),
        ));

        row.damage = 25.0;
        apply_net_wieldable_tuning(&mut registry, weapon_id, row);
        let installed = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap();
        assert_eq!(installed.state, WieldableState::Idle);
        assert_eq!(installed.damage, 25.0);
        assert_eq!(installed.magazine, before.magazine);
        assert_eq!(
            installed.cooldown_remaining_ms,
            before.cooldown_remaining_ms
        );
        assert!(!Arc::ptr_eq(
            installed.activation_programs.secondary.as_ref().unwrap(),
            &frozen_shot.program,
        ));
        assert_eq!(frozen_shot.values.damage, 10.0);
        assert_eq!(frozen_shot.shot_id.ordinal, 0);
    }

    #[test]
    fn net_wieldable_tuning_clamps_untrusted_pellet_stats_on_apply() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        let descriptor = weapon_descriptor("reference_pistol");
        registry
            .set_component(
                weapon_id,
                WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
            )
            .unwrap();
        let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);

        for (pellet_count, spread_degrees, expected_count, expected_spread) in [
            (0, -1.0, 1, 0.0),
            (MAX_PELLET_COUNT + 1, f32::NAN, MAX_PELLET_COUNT, 0.0),
            (u32::MAX, f32::INFINITY, MAX_PELLET_COUNT, 0.0),
            (8, f32::NEG_INFINITY, 8, 0.0),
            (8, 90.0, 8, 45.0),
        ] {
            let payload = tuning.wieldables[0].as_mut().unwrap();
            payload.pellet_count = pellet_count;
            payload.spread_degrees = spread_degrees;
            apply_net_wieldable_tuning(&mut registry, weapon_id, payload);

            let weapon = registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap();
            assert_eq!(weapon.pellet_count, expected_count);
            assert!(
                (weapon.spread_degrees - expected_spread).abs() <= f32::EPSILON,
                "spread_degrees {} differs from expected {}",
                weapon.spread_degrees,
                expected_spread
            );
        }
    }

    #[test]
    fn net_wieldable_tuning_clamps_untrusted_dynamic_accuracy_on_apply() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        let descriptor = weapon_descriptor("reference_pistol");
        registry
            .set_component(
                weapon_id,
                WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
            )
            .unwrap();
        let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);

        for (bloom_per_shot, bloom_max, decay_rate, decay_delay, movement, bias, expected) in [
            (
                -1.0,
                -2.0,
                -3.0,
                -4.0,
                -5.0,
                -0.1,
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            ),
            (
                90.0,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                60.0,
                2.0,
                (45.0, 0.0, 0.0, 0.0, 45.0, 1.0),
            ),
            (
                1.5,
                6.0,
                1_000_000.0,
                175.0,
                3.0,
                0.25,
                (1.5, 6.0, 1_000_000.0, 175.0, 3.0, 0.25),
            ),
        ] {
            let payload = tuning.wieldables[0].as_mut().unwrap();
            payload.bloom_per_shot_degrees = bloom_per_shot;
            payload.bloom_max_degrees = bloom_max;
            payload.bloom_decay_degrees_per_second = decay_rate;
            payload.bloom_decay_delay_ms = decay_delay;
            payload.movement_spread_degrees = movement;
            payload.spread_vertical_bias = bias;
            apply_net_wieldable_tuning(&mut registry, weapon_id, payload);

            let weapon = registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap();
            let (
                expected_bloom_per_shot,
                expected_bloom_max,
                expected_decay_rate,
                expected_decay_delay,
                expected_movement,
                expected_bias,
            ) = expected;
            assert!((weapon.bloom_per_shot_degrees - expected_bloom_per_shot).abs() < f32::EPSILON);
            assert!((weapon.bloom_max_degrees - expected_bloom_max).abs() < f32::EPSILON);
            assert!(
                (weapon.bloom_decay_degrees_per_second - expected_decay_rate).abs() < f32::EPSILON
            );
            assert!((weapon.bloom_decay_delay_ms - expected_decay_delay).abs() < f32::EPSILON);
            assert!((weapon.movement_spread_degrees - expected_movement).abs() < f32::EPSILON);
            assert!((weapon.spread_vertical_bias - expected_bias).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn net_wieldable_tuning_drives_predicted_pellet_ray_count() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        let descriptor = weapon_descriptor("reference_pistol");
        registry
            .set_component(
                weapon_id,
                WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
            )
            .unwrap();
        let target = registry.spawn(Transform {
            position: Vec3::new(0.0, 0.0, -5.0),
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents: Vec3::splat(0.5),
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);
        let payload = tuning.wieldables[0].as_mut().unwrap();
        payload.pellet_count = 8;
        payload.spread_degrees = 0.0;
        payload.bloom_per_shot_degrees = 1.5;
        payload.bloom_max_degrees = 6.0;
        payload.bloom_decay_degrees_per_second = 2.5;
        payload.bloom_decay_delay_ms = 175.0;
        payload.movement_spread_degrees = 3.0;
        payload.spread_vertical_bias = 0.2;
        apply_net_wieldable_tuning(&mut registry, weapon_id, payload);

        let mut weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        assert!((weapon.bloom_per_shot_degrees - 1.5).abs() < f32::EPSILON);
        assert!((weapon.bloom_max_degrees - 6.0).abs() < f32::EPSILON);
        assert!((weapon.bloom_decay_degrees_per_second - 2.5).abs() < f32::EPSILON);
        assert!((weapon.bloom_decay_delay_ms - 175.0).abs() < f32::EPSILON);
        assert!((weapon.movement_spread_degrees - 3.0).abs() < f32::EPSILON);
        assert!((weapon.spread_vertical_bias - 0.2).abs() < f32::EPSILON);
        let resolution = crate::weapon::resolve_test_client_shot(
            None,
            &mut weapon,
            "reference_pistol",
            0,
            crate::weapon::FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &postretro_foundation::WeaponPlacementDescriptor::default(),
            None,
            7,
            &[0.0],
            &[],
            &crate::collision::CollisionWorld::new(),
            &registry,
            &crate::scripting_systems::hit_zones::HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("an applied eight-pellet tuning resolves the client shell");

        assert_eq!(resolution.hits.len(), 8);
        assert!(resolution.hits.iter().all(|hit| hit.target == target));
    }

    #[test]
    fn held_descriptor_lifecycle_client_materialization_orders_preserve_ownership_and_health() {
        use crate::scripting_systems::hit_zones::{HitZoneStore, nearest_entity_hit};
        fn health_weapon(name: &str) -> EntityTypeDescriptor {
            let mut descriptor = touchable_mesh_weapon_descriptor(name);
            descriptor.health = Some(postretro_foundation::HealthDescriptor {
                max: 75.0,
                hitbox: Some(postretro_foundation::HitboxDescriptor {
                    half_extents: [0.5; 3],
                    offset: None,
                }),
                zone_multipliers: Default::default(),
            });
            descriptor
        }
        for control_first in [false, true] {
            let mut registry = EntityRegistry::new();
            let pawn = registry.spawn(Transform::default());
            let descriptors = [
                player_with_default_weapon("player", "local_pistol"),
                health_weapon("local_pistol"),
                health_weapon("replacement_rifle"),
            ];
            let mut tuning = tuning_for_slot(0, "local_pistol", 220.0, 340.0, 55, 80);
            if !control_first {
                // The baseline arms first and composes nothing until Control.
                assert!(!materialize_net_local_wieldable_inventory_from_tuning(
                    "player",
                    &descriptors,
                    &mut registry,
                    pawn,
                    None
                ));
                assert_eq!(
                    registry.has_component_kind(pawn, ComponentKind::Inventory),
                    Ok(false)
                );
            }
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut registry,
                pawn,
                Some(&tuning)
            ));
            let item = registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0]
                .unwrap();
            assert_eq!(registry.inventory_owner(item), Some((pawn, 0)));
            assert!(registry.is_inventory_owned(item));
            let mut health = registry
                .get_component::<HealthComponent>(item)
                .unwrap()
                .clone();
            health.current = 29.0;
            registry.set_component(item, health).unwrap();
            let mut weapon = registry
                .get_component::<WeaponComponent>(item)
                .unwrap()
                .clone();
            weapon.magazine = 3;
            weapon.cooldown_remaining_ms = 47.0;
            registry.set_component(item, weapon).unwrap();
            // A later re-arm must keep the already materialized instance and live state.
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut registry,
                pawn,
                Some(&tuning)
            ));
            assert_eq!(
                registry
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .wieldables[0],
                Some(item)
            );
            assert_eq!(
                registry
                    .get_component::<HealthComponent>(item)
                    .unwrap()
                    .current,
                29.0
            );
            tuning.wieldables[0].as_mut().unwrap().range = 180.0;
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut registry,
                pawn,
                Some(&tuning)
            ));
            assert_eq!(
                registry
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .wieldables[0],
                Some(item)
            );
            assert_eq!(registry.inventory_owner(item), Some((pawn, 0)));
            assert_eq!(
                registry
                    .get_component::<HealthComponent>(item)
                    .unwrap()
                    .current,
                29.0
            );
            let weapon = registry.get_component::<WeaponComponent>(item).unwrap();
            assert_eq!(weapon.range, 180.0);
            assert_eq!(weapon.magazine, 3);
            assert_eq!(weapon.cooldown_remaining_ms, 47.0);
            let zones = HitZoneStore::new();
            assert!(
                nearest_entity_hit(&registry, &zones, 0.0, Vec3::Z * 2.0, Vec3::NEG_Z, 4.0, 0.0)
                    .is_none()
            );
            // Positive query control proves the retained hitbox is usable in world space.
            let inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            registry.set_component(pawn, Inventory::default()).unwrap();
            assert_eq!(
                nearest_entity_hit(&registry, &zones, 0.0, Vec3::Z * 2.0, Vec3::NEG_Z, 4.0, 0.0)
                    .unwrap()
                    .target,
                item
            );
            registry.set_component(pawn, inventory).unwrap();
            tuning.wieldables[0].as_mut().unwrap().canonical_name = "replacement_rifle".into();
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut registry,
                pawn,
                Some(&tuning)
            ));
            let replacement = registry
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0]
                .unwrap();
            assert_ne!(replacement, item);
            assert_eq!(replacement.to_raw() & 0xffff, item.to_raw() & 0xffff);
            assert_ne!(replacement.to_raw() >> 16, item.to_raw() >> 16);
            assert!(!registry.exists(item));
            assert_eq!(registry.inventory_owner(item), None);
            assert_eq!(registry.inventory_owner(replacement), Some((pawn, 0)));
            assert_eq!(
                registry
                    .get_component::<HealthComponent>(replacement)
                    .unwrap()
                    .current,
                75.0
            );
            tuning.wieldables[0] = None;
            assert!(materialize_net_local_wieldable_inventory_from_tuning(
                "player",
                &descriptors,
                &mut registry,
                pawn,
                Some(&tuning)
            ));
            assert!(
                registry
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .wieldables[0]
                    .is_none()
            );
            assert!(!registry.exists(replacement));
            assert_eq!(registry.inventory_owner(replacement), None);
            let reused = registry.spawn(Transform::default());
            assert_eq!(reused.to_raw() & 0xffff, replacement.to_raw() & 0xffff);
            assert_ne!(reused.to_raw() >> 16, replacement.to_raw() >> 16);
            assert_eq!(registry.inventory_owner(reused), None);
        }
    }

    #[test]
    fn tuning_first_materializes_capacity_slot_from_host_archetype() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "local_pistol"),
            weapon_descriptor("local_pistol"),
            touchable_mesh_weapon_descriptor("host_ion_rifle"),
        ];
        let tuning = tuning_for_slot(2, "host_ion_rifle", 220.0, 340.0, 55, 80);

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        let inventory = reg.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 2);
        assert!(inventory.wieldables[0].is_none());
        let weapon_id = inventory.wieldables[2].expect("host slot materialized");
        assert_eq!(reg.inventory_owner(weapon_id), Some((pawn, 2)));
        assert_eq!(
            reg.get_component::<DescriptorProvenance>(weapon_id)
                .unwrap()
                .canonical_name,
            "host_ion_rifle"
        );
        let weapon = reg.get_component::<WeaponComponent>(weapon_id).unwrap();
        assert_eq!(weapon.range, 220.0);
        assert_eq!(weapon.primary.recovery_ms, 340.0);
        assert_eq!(weapon.lower_ms, 55);
        assert_eq!(weapon.raise_ms, 80);
        assert!(reg.get_component::<MeshComponent>(weapon_id).is_err());
        assert_eq!(
            reg.has_component_kind(weapon_id, ComponentKind::Touchable),
            Ok(false),
            "tuning materialization routes through the inventory ownership chokepoint"
        );
        assert_eq!(
            inventory.wieldables.len(),
            WIELDABLE_SLOT_CAPACITY,
            "the payload preserves the engine's fixed slot capacity"
        );
    }

    #[test]
    fn tuning_merge_grows_an_existing_inventory_at_the_host_named_slot() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "local_pistol"),
            weapon_descriptor("local_pistol"),
            weapon_descriptor("host_ion_rifle"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "local_pistol")], 0)),
        ));
        let original = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("the first payload composes slot 0");

        let mut tuning = tuning_for_slot(2, "host_ion_rifle", 220.0, 340.0, 55, 80);
        tuning.wieldables[0] = Some(WieldableTuningPayload {
            canonical_name: "local_pistol".to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: 64.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                100.0,
            ),
            secondary: None,
            damage: 10.0,
            knockback: None,
            projectile: None,
            splash: None,
            resource: None,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        });
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        let inventory = reg.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.wieldables[0], Some(original));
        let grown = inventory.wieldables[2].expect("host-named slot is filled");
        assert_eq!(reg.inventory_owner(original), Some((pawn, 0)));
        assert_eq!(reg.inventory_owner(grown), Some((pawn, 2)));
        assert_eq!(
            reg.get_component::<DescriptorProvenance>(grown)
                .unwrap()
                .canonical_name,
            "host_ion_rifle"
        );
    }

    #[test]
    fn tuning_merge_releases_absent_slot_and_despawns_local_instance() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "reference_pistol")], 0)),
        ));
        let weapon_id = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("the first payload composes slot 0");

        let empty_tuning =
            TuningPayload::new(None, std::array::from_fn(|_| None), test_environment());
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&empty_tuning),
        ));

        assert!(
            reg.get_component::<Inventory>(pawn).unwrap().wieldables[0].is_none(),
            "the host's empty slot releases the local inventory entry"
        );
        assert!(
            !reg.exists(weapon_id),
            "the released client-owned instance must not leak for the level"
        );
    }

    #[test]
    fn tuning_merge_same_named_slot_preserves_the_existing_instance() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "reference_pistol")], 0)),
        ));
        let weapon_id = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("the first payload composes slot 0");
        let mut live_weapon = reg
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        live_weapon.magazine = 3;
        let tuning_weapon = live_weapon.clone();
        reg.set_component(weapon_id, live_weapon).unwrap();

        let mut wieldables = std::array::from_fn(|_| None);
        wieldables[0] = Some(WieldableTuningPayload {
            canonical_name: "reference_pistol".to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: tuning_weapon.range,
            primary: (*tuning_weapon.primary).clone(),
            secondary: tuning_weapon.secondary.as_deref().cloned(),
            damage: tuning_weapon.damage,
            knockback: tuning_weapon.knockback,
            projectile: tuning_weapon.projectile.clone(),
            splash: tuning_weapon.splash.clone(),
            resource: WieldableTuningPayload::resource_from_weapon(&tuning_weapon),
            pellet_count: tuning_weapon.pellet_count,
            spread_degrees: tuning_weapon.spread_degrees,
            bloom_per_shot_degrees: tuning_weapon.bloom_per_shot_degrees,
            bloom_max_degrees: tuning_weapon.bloom_max_degrees,
            bloom_decay_degrees_per_second: tuning_weapon.bloom_decay_degrees_per_second,
            bloom_decay_delay_ms: tuning_weapon.bloom_decay_delay_ms,
            movement_spread_degrees: tuning_weapon.movement_spread_degrees,
            spread_vertical_bias: tuning_weapon.spread_vertical_bias,
            resolution: tuning_weapon.resolution,
            lower_ms: tuning_weapon.lower_ms,
            raise_ms: tuning_weapon.raise_ms,
            block_during_reload: tuning_weapon.block_during_reload,
        });
        let tuning = TuningPayload::new(None, wieldables, test_environment());
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        assert_eq!(
            reg.get_component::<Inventory>(pawn).unwrap().wieldables[0],
            Some(weapon_id),
            "matching descriptor identity retunes in place instead of transferring an instance"
        );
        assert_eq!(
            reg.get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .magazine,
            3,
            "the tuning merge never transfers host instance state"
        );
    }

    // Regression: an occupied slot was retuned in place even when the host named
    // a different descriptor, leaving client inventory identity divergent.
    #[test]
    fn tuning_merge_replaces_occupied_slot_when_descriptor_identity_changes() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "local_pistol"),
            weapon_descriptor("local_pistol"),
            touchable_mesh_weapon_descriptor("host_ion_rifle"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "local_pistol")], 0)),
        ));
        let old_weapon = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("the first payload composes slot 0");

        let tuning = tuning_for_slot(0, "host_ion_rifle", 220.0, 340.0, 55, 80);
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        let replacement = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("host identity replaces the local slot");
        assert_ne!(replacement, old_weapon);
        assert!(
            !reg.exists(old_weapon),
            "released local instance is despawned"
        );
        assert_eq!(
            reg.get_component::<DescriptorProvenance>(replacement)
                .unwrap()
                .canonical_name,
            "host_ion_rifle"
        );
        assert_eq!(
            reg.get_component::<WeaponComponent>(replacement)
                .unwrap()
                .range,
            220.0
        );
        assert!(reg.get_component::<MeshComponent>(replacement).is_err());
        assert_eq!(
            reg.has_component_kind(replacement, ComponentKind::Touchable),
            Ok(false)
        );
    }

    #[test]
    fn tuning_merge_unknown_replacement_despawns_old_without_leaking_a_new_entity() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "local_pistol"),
            weapon_descriptor("local_pistol"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "local_pistol")], 0)),
        ));
        let old_weapon = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0].unwrap();

        let tuning = tuning_for_slot(0, "unknown_host_weapon", 1.0, 1.0, 0, 0);
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        assert!(!reg.exists(old_weapon));
        assert!(reg.get_component::<Inventory>(pawn).unwrap().wieldables[0].is_none());
        assert_eq!(
            reg.iter_with_kind(ComponentKind::Weapon).count(),
            0,
            "unknown replacement cannot leave an unowned local instance"
        );
    }

    #[test]
    fn o41_tuning_arrival_mid_switch_merges_without_rematerializing_live_state() {
        let mut reg = EntityRegistry::new();
        let pawn = reg.spawn(Transform::default());
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];

        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&payload_for(&descriptors, &[(0, "reference_pistol")], 0)),
        ));
        let weapon_id = reg
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();
        let mut before = reg
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        before.magazine = 3;
        before.cooldown_remaining_ms = 47.0;
        before.state = WieldableState::Raising;
        before.state_remaining_ms = 18;
        before.state_total_ms = 60;
        reg.set_component(weapon_id, before).unwrap();

        let tuning = tuning_for_slot(0, "reference_pistol", 144.0, 215.0, 70, 95);
        assert!(materialize_net_local_wieldable_inventory_from_tuning(
            "player",
            &descriptors,
            &mut reg,
            pawn,
            Some(&tuning),
        ));

        assert_eq!(
            reg.get_component::<Inventory>(pawn)
                .unwrap()
                .active_wieldable(),
            Some(weapon_id),
            "retuning keeps the active instance rather than composing a replacement"
        );
        let after = reg.get_component::<WeaponComponent>(weapon_id).unwrap();
        assert_eq!(after.range, 144.0);
        assert_eq!(after.primary.recovery_ms, 215.0);
        assert_eq!(after.lower_ms, 70);
        assert_eq!(after.raise_ms, 95);
        assert_eq!(after.magazine, 3);
        assert_eq!(after.cooldown_remaining_ms, 47.0);
        assert_eq!(after.state, WieldableState::Raising);
        assert_eq!(after.state_remaining_ms, 18);
        assert_eq!(after.state_total_ms, 60);
    }

    // A descriptor-backed net-slot pawn is a real PlayerMovement pawn from the
    // placement, but — unlike spawn_from_player_starts — it is NEVER marked the local
    // player and NEVER promotes a global active_wieldable. Provenance is NetworkSlot.
    #[test]
    fn net_slot_pawn_is_player_movement_without_local_marker_or_active_wieldable() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![player_with_movement("player")];
        let placement = spawn_point_at(Vec3::new(2.0, 1.0, -3.0), Vec3::ZERO, &[]);

        let id = spawn_net_slot_pawn(&placement, &descriptors, &mut reg, None)
            .expect("net-slot pawn spawns from a player descriptor");

        // It is a movement pawn at the placement origin.
        assert!(matches!(
            reg.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(true)
        ));
        assert_eq!(
            reg.get_component::<Transform>(id).unwrap().position,
            Vec3::new(2.0, 1.0, -3.0)
        );

        // Provenance distinguishes it from a map-start single-player spawn.
        let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
        assert_eq!(provenance.spawn_path, DescriptorSpawnPath::NetworkSlot);

        // It is NOT the local player — the host never marks a remote pawn local, even
        // though the player-start path would have marked the first such pawn.
        assert_ne!(
            reg.local_player_pawn(),
            Some(id),
            "a net-slot pawn is never the local player"
        );
    }

    #[test]
    fn net_slot_pawn_returns_spawned_default_weapon() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            weapon_descriptor("reference_pistol"),
        ];
        let placement = spawn_point(&[]);

        let pawn = spawn_net_slot_pawn(&placement, &descriptors, &mut reg, None)
            .expect("net-slot pawn spawns from a player descriptor");
        let weapon = reg
            .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .expect("inventory loadout materializes an active weapon entity");

        assert_ne!(
            pawn, weapon,
            "the active weapon is the sibling inventory entity, not the pawn"
        );
        assert!(matches!(
            reg.has_component_kind(weapon, ComponentKind::Weapon),
            Ok(true)
        ));
        assert_ne!(
            reg.local_player_pawn(),
            Some(pawn),
            "a net-slot pawn still is not marked as the local player"
        );
    }

    #[test]
    fn net_slot_pawn_seeds_local_reserve_and_full_sibling_weapon_magazine() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![
            player_with_default_weapon("player", "reference_pistol"),
            ammo_weapon_descriptor("reference_pistol"),
        ];

        let pawn = spawn_net_slot_pawn(&spawn_point(&[]), &descriptors, &mut reg, None).unwrap();
        let weapon = reg
            .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .expect("net-slot sibling weapon");

        assert_eq!(
            reg.get_component::<postretro_entities::AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            48
        );
        assert_eq!(
            reg.get_component::<postretro_entities::components::weapon::WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            12
        );
        assert_ne!(reg.local_player_pawn(), Some(pawn));
    }

    // The net-slot path defaults `entity_class` to "player", matching
    // spawn_from_player_starts; an unregistered entity_class is skipped (None).
    #[test]
    fn net_slot_pawn_defaults_to_player_and_skips_unknown_class() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![player_with_movement("player")];

        // Default entity_class -> "player".
        let default_placement = spawn_point(&[]);
        let _pawn = spawn_net_slot_pawn(&default_placement, &descriptors, &mut reg, None)
            .expect("default entity_class spawns a pawn");

        // Explicit unknown entity_class -> skipped.
        let unknown = spawn_point(&[("entity_class", "no_such_class")]);
        assert!(spawn_net_slot_pawn(&unknown, &descriptors, &mut reg, None).is_none());
    }
    #[test]
    fn net_wieldable_tuning_preserves_host_reload_override_and_local_presentation() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        let mut descriptor = weapon_descriptor("reference_pistol");
        let local = descriptor.weapon.as_mut().unwrap();
        local.block_during_reload = Some(true);
        local.credit_source = Some("pistol.primary".into());
        local.sounds = Some(postretro_foundation::WeaponSounds {
            fire: Some("sfx/local-default-fire".into()),
            impact: Some("sfx/local-default-impact".into()),
            dry_fire: Some("sfx/local-empty".into()),
            ..Default::default()
        });
        let local_sounds = local.sounds.clone();
        local.primary.sounds = Some(postretro_foundation::ActivationSounds {
            fire: Some("sfx/local-primary".into()),
            impact: None,
        });
        let mut secondary = postretro_foundation::WeaponActivationDescriptor::single(
            postretro_foundation::ActivationTrigger::Press,
            250.0,
        );
        secondary.sounds = Some(postretro_foundation::ActivationSounds {
            fire: Some("sfx/local-secondary".into()),
            impact: None,
        });
        local.secondary = Some(secondary);
        let mut weapon = WeaponComponent::from_descriptor(local);
        weapon.state = postretro_entities::components::wieldable_state::WieldableState::Reloading;
        registry.set_component(weapon_id, weapon).unwrap();
        let mut tuning = tuning_for_slot(0, "reference_pistol", 64.0, 100.0, 0, 0);
        let row = tuning.wieldables[0].as_mut().unwrap();
        row.block_during_reload = Some(true);
        row.damage = 30.0;
        row.primary.recovery_ms = 200.0;
        row.primary.emits = Some(postretro_foundation::ActivationEmits {
            activate: Some("host.primary".into()),
            impact: None,
        });
        let mut host_secondary = postretro_foundation::WeaponActivationDescriptor::single(
            postretro_foundation::ActivationTrigger::Press,
            450.0,
        );
        host_secondary.emits = Some(postretro_foundation::ActivationEmits {
            activate: Some("host.secondary".into()),
            impact: None,
        });
        row.secondary = Some(host_secondary.clone());
        apply_net_wieldable_tuning(&mut registry, weapon_id, row);
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap();
        assert_eq!(weapon.damage, 30.0);
        assert_eq!(weapon.credit_source, "pistol.primary");
        assert_eq!(weapon.sounds.as_deref(), local_sounds.as_ref());
        assert!(weapon.state.is_reload_activity());
        assert!(
            weapon.block_during_reload.unwrap_or(false),
            "host override blocks reload switching even with mod global false"
        );
        assert!(weapon.activation_programs.primary.is_some());
        assert_eq!(weapon.primary.recovery_ms, 200.0);
        assert_eq!(weapon.primary.emits, row.primary.emits);
        assert_eq!(
            weapon.primary.sounds.as_ref().unwrap().fire.as_deref(),
            Some("sfx/local-primary")
        );
        let secondary = weapon.secondary.as_ref().unwrap();
        assert_eq!(secondary.recovery_ms, 450.0);
        assert_eq!(secondary.emits, host_secondary.emits);
        assert_eq!(
            secondary.sounds.as_ref().unwrap().fire.as_deref(),
            Some("sfx/local-secondary")
        );
        row.secondary = None;
        apply_net_wieldable_tuning(&mut registry, weapon_id, row);
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .secondary
                .is_none()
        );
        row.secondary = Some(host_secondary);
        apply_net_wieldable_tuning(&mut registry, weapon_id, row);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .sounds
                .as_deref(),
            local_sounds.as_ref(),
            "host lane replacement preserves common local sound fallbacks"
        );
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .secondary
                .as_ref()
                .unwrap()
                .sounds
                .is_none(),
            "new host lane uses common weapon sound fallback when no local lane exists"
        );
    }
}
