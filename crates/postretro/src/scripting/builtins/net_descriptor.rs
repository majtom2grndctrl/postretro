// Descriptor materialization helpers used by netcode replication.
// See: context/lib/networking.md

use super::MapEntity;
use super::data_archetype::{
    compose_wieldable_inventory, compose_wieldable_inventory_from_slots, descriptor_mesh_component,
    find_descriptor, spawn_descriptor_instance,
};
use super::wieldable_inventory::{acquire_wieldable_at, release_wieldable};
use postretro_entities::components::inventory::Inventory;
#[cfg(test)]
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::registry::{ComponentKind, EntityId, EntityRegistry, Transform};
use postretro_foundation::{MAX_PELLET_COUNT, NavAgentParams};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use crate::netcode::{TuningPayload, WieldableTuningPayload};

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
pub(crate) fn spawn_net_slot_pawn_with_carried_loadout(
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
    carried_loadout: Option<&crate::netcode::CarriedState>,
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
        // live siblings and makes its first populated slot the active instance.
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

    crate::netcode::restore_carried_health(carried_loadout, registry, id);

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
pub(crate) fn materialize_net_local_movement_component(
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
pub(crate) fn materialize_net_local_movement_component_from_tuning(
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
/// host's replicated weapon tuning. A local-player baseline normally arrives
/// first, so descriptor defaults make the pawn responsive until Control arrives.
/// If Control wins the race, its fixed slot array supplies the composition before
/// the instances are created. Later payloads preserve and retune a same-canonical
/// instance, including its magazine and live timers; a changed canonical name
/// replaces the local instance at the host-named slot.
pub(crate) fn materialize_net_local_wieldable_inventory_from_tuning(
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
        let placement = MapEntity {
            classname: entity_class.to_string(),
            origin: registry
                .get_component::<Transform>(id)
                .map_or(glam::Vec3::ZERO, |transform| transform.position),
            angles: glam::Vec3::ZERO,
            key_values: Default::default(),
            tags: vec![],
        };
        if let Some(tuning) = tuning {
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
            );
        } else if let Some(descriptor) = find_descriptor(descriptors, entity_class) {
            let _ = compose_wieldable_inventory(
                registry,
                id,
                descriptor,
                &placement,
                descriptors,
                None,
            );
        } else {
            log::warn!(
                "[Net] local pawn entity_class `{entity_class}` not registered; wieldable inventory stays inert"
            );
            return false;
        }
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

fn apply_net_wieldable_tuning(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    tuning: &WieldableTuningPayload,
) {
    let Ok(mut weapon) = registry
        .get_component::<WeaponComponent>(weapon_id)
        .cloned()
    else {
        return;
    };
    weapon.range = tuning.range;
    weapon.cooldown_ms = tuning.cooldown_ms;
    weapon.pellet_count = tuning.pellet_count.clamp(1, MAX_PELLET_COUNT);
    weapon.spread_degrees = if tuning.spread_degrees.is_finite() {
        tuning.spread_degrees.clamp(0.0, 45.0)
    } else {
        0.0
    };
    weapon.fire_mode = tuning.fire_mode;
    weapon.resolution = tuning.resolution;
    weapon.lower_ms = tuning.lower_ms;
    weapon.raise_ms = tuning.raise_ms;
    let _ = registry.set_component(weapon_id, weapon);
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
pub(crate) fn materialize_net_mesh_presentation(
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use log::Level;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
    use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_scripting_core::data_descriptors::{
        AirParams, AmmoResource, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
        BehaviorGraphEnvelope, CapsuleParams, FallParams, FireMode, GroundParams, MeshDescriptor,
        MotionVerb, PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams, TouchMode,
        TouchableDescriptor, WeaponDescriptor, WeaponResource,
    };
    use postretro_test_log_capture::LogCapture;
    use std::collections::HashMap;

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
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: std::collections::BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
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
            view_feel: None,
        }
    }

    fn player_with_movement(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
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
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 64.0,
                cooldown_ms: 100.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
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

        let carried = crate::netcode::CarriedState {
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

    fn tuning_for_slot(
        slot: usize,
        canonical_name: &str,
        range: f32,
        cooldown_ms: f32,
        lower_ms: u32,
        raise_ms: u32,
    ) -> TuningPayload {
        let mut wieldables = std::array::from_fn(|_| None);
        wieldables[slot] = Some(crate::netcode::WieldableTuningPayload {
            canonical_name: canonical_name.to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range,
            cooldown_ms,
            pellet_count: 1,
            spread_degrees: 0.0,
            fire_mode: FireMode::Auto,
            resolution: ResolutionMode::Hitscan,
            lower_ms,
            raise_ms,
        });
        TuningPayload::new(None, wieldables)
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
        apply_net_wieldable_tuning(&mut registry, weapon_id, payload);

        let mut weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let resolution = crate::weapon::resolve_client_fire(
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
        assert_eq!(
            reg.get_component::<DescriptorProvenance>(weapon_id)
                .unwrap()
                .canonical_name,
            "host_ion_rifle"
        );
        let weapon = reg.get_component::<WeaponComponent>(weapon_id).unwrap();
        assert_eq!(weapon.range, 220.0);
        assert_eq!(weapon.cooldown_ms, 340.0);
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
            None,
        ));
        let original = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("descriptor inventory materializes its default wieldable");

        let mut tuning = tuning_for_slot(2, "host_ion_rifle", 220.0, 340.0, 55, 80);
        tuning.wieldables[0] = Some(crate::netcode::WieldableTuningPayload {
            canonical_name: "local_pistol".to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: 64.0,
            cooldown_ms: 100.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 0,
            raise_ms: 0,
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
            None,
        ));
        let weapon_id = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("descriptor inventory materializes its default wieldable");

        let empty_tuning = TuningPayload::new(None, std::array::from_fn(|_| None));
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
            None,
        ));
        let weapon_id = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("descriptor inventory materializes its default wieldable");
        let mut live_weapon = reg
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        live_weapon.magazine = 3;
        let tuning_weapon = live_weapon.clone();
        reg.set_component(weapon_id, live_weapon).unwrap();

        let mut wieldables = std::array::from_fn(|_| None);
        wieldables[0] = Some(crate::netcode::WieldableTuningPayload {
            canonical_name: "reference_pistol".to_string(),
            placement: postretro_foundation::WeaponPlacementDescriptor::default(),
            muzzle_offset: None,
            range: tuning_weapon.range,
            cooldown_ms: tuning_weapon.cooldown_ms,
            pellet_count: tuning_weapon.pellet_count,
            spread_degrees: tuning_weapon.spread_degrees,
            fire_mode: tuning_weapon.fire_mode,
            resolution: tuning_weapon.resolution,
            lower_ms: tuning_weapon.lower_ms,
            raise_ms: tuning_weapon.raise_ms,
        });
        let tuning = TuningPayload::new(None, wieldables);
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
            None,
        ));
        let old_weapon = reg.get_component::<Inventory>(pawn).unwrap().wieldables[0]
            .expect("local descriptor inventory materializes");

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
            None,
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
            None,
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
        assert_eq!(after.cooldown_ms, 215.0);
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
}
