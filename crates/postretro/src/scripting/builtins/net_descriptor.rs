// Descriptor materialization paths that `crate::netcode` (the engine's sole
// registry-touching replication path) calls: host-side net-slot pawn spawn,
// client-side local-movement materialization, and client-side remote-enemy
// presentation. These reuse the same descriptor → component internals as the
// data-archetype map sweep (`data_archetype.rs`) rather than reinventing them.
//
// See: context/lib/networking.md (replication ownership / role model)
//      context/lib/build_pipeline.md §Built-in Classname Routing

use super::MapEntity;
use super::data_archetype::{find_descriptor, spawn_descriptor_instance};
use crate::nav::NavAgentParams;
use crate::scripting::components::mesh::{MeshAnimation, MeshComponent};
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::data_descriptors::EntityTypeDescriptor;
use crate::scripting::provenance::DescriptorSpawnPath;
use crate::scripting::registry::{ComponentKind, EntityId, EntityRegistry};

/// Spawn ONE descriptor-backed networked-slot player pawn from a `player_spawn`
/// placement (M15 Phase 3 Task 4). This is the host-authoritative remote-pawn
/// counterpart to [`super::data_archetype::spawn_from_player_starts`]: it reuses the
/// same descriptor materialization internals ([`spawn_descriptor_instance`]) and the
/// same `entity_class` KVP → `"player"`-default descriptor lookup, but it is
/// deliberately NOT the local-player path:
///
/// - it does NOT call `mark_local_player_pawn` (a remote pawn is never the host's
///   local player), and
/// - it does NOT assign a global `active_wieldable` (the host does not wield a
///   remote client's weapon).
///
/// The pawn's `defaultWeapon` still materializes a sibling weapon instance when the
/// descriptor declares one (so the remote pawn is armed and replicates a weapon),
/// but that weapon is never promoted to the host's active wieldable.
///
/// Provenance is stamped [`DescriptorSpawnPath::NetworkSlot`] so these pawns are
/// distinguishable from map-start single-player spawns. The per-placement KVP bag is
/// forwarded with `entity_class` stripped, matching `spawn_from_player_starts`.
///
/// Returns the spawned pawn `EntityId`, or `None` if the descriptor is unregistered
/// or the registry is exhausted (logged, like the player-start path).
pub(crate) fn spawn_net_slot_pawn(
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    agent_params: Option<NavAgentParams>,
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
        // Attach the descriptor's own weapon component to the pawn just like the
        // player-start path (so the remote pawn is armed); the sibling
        // `defaultWeapon` instance below is what `spawn_from_player_starts` would
        // promote to active — here it is spawned but never promoted.
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

    // Forward the per-placement KVP bag (sans `entity_class`, a routing hint) so
    // `getEntityProperty` works uniformly for net-slot pawns, matching the
    // player-start path. Deliberately NO `mark_local_player_pawn` here.
    let mut kvps = placement.key_values.clone();
    kvps.remove("entity_class");
    let _ = registry.set_map_kvps(id, kvps);

    // Materialize the sibling defaultWeapon instance if the descriptor declares one,
    // mirroring `spawn_from_player_starts` — but NEVER promote it to a global active
    // wieldable. The host does not wield a remote client's weapon.
    if let Some(default_weapon) = descriptor.default_weapon.as_deref() {
        match find_descriptor(descriptors, default_weapon) {
            Some(weapon_descriptor) if weapon_descriptor.weapon.is_some() => {
                let weapon_entity = MapEntity {
                    classname: default_weapon.to_string(),
                    origin: placement.origin,
                    angles: placement.angles,
                    key_values: Default::default(),
                    tags: vec![],
                };
                match spawn_descriptor_instance(
                    registry,
                    weapon_descriptor,
                    &weapon_entity,
                    true,
                    DescriptorSpawnPath::DefaultWeapon,
                    None,
                ) {
                    Some(weapon_id) => {
                        let _ = registry.set_map_kvps(weapon_id, Default::default());
                    }
                    None => log::warn!(
                        "[Net] {origin}: entity registry exhausted; dropping net-slot defaultWeapon `{default_weapon}`",
                        origin = placement.diagnostic_origin(),
                    ),
                }
            }
            _ => log::warn!(
                "[Net] {origin}: defaultWeapon `{default_weapon}` not registered or has no weapon component; net-slot pawn spawned unarmed",
                origin = placement.diagnostic_origin(),
            ),
        }
    }

    Some(id)
}

/// Materialize the descriptor-derived `PlayerMovementComponent` for a client's LOCAL
/// network pawn (M15 Phase 3 Task 7), reusing the same descriptor → component
/// internals as the host's net-slot spawn (`PlayerMovementComponent::from_descriptor`,
/// the body of `attach_descriptor_components`). This is the client counterpart to
/// the host's [`spawn_net_slot_pawn`]: the host spawns the authoritative pawn from a
/// descriptor and replicates only the mutable movement subset; the client receives a
/// Transform-only baseline (the wire never carries descriptor-immutable tuning) and
/// must materialize the matching component locally so the wire subset has something to
/// merge onto and prediction/reconciliation can run.
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

/// Materialize the presentation-only components for a client's REMOTE enemy pawn
/// (E10). A connected client does not simulate host-owned enemies: the host owns
/// their AI, steering, damage, death, and despawn, and replicates only their
/// position. The client receives a Transform-only baseline from the snapshot and
/// must attach the descriptor's *presentation* surface locally so the remote enemy
/// renders — but it must carry NO hidden authoritative state.
///
/// This attaches ONLY the descriptor's mesh block (`MeshComponent`, including its
/// declared animation states + default state, via the same path
/// `attach_descriptor_components` uses). It deliberately attaches NONE of
/// `Brain`, `Agent`, `Health`, `Weapon`, or `PlayerMovement`: those are
/// host-authoritative and a client carries no shadow copy.
///
/// `entity_class` is the descriptor class the host stamped on the wire. An
/// unregistered class, or a descriptor with no mesh block, leaves the entity
/// transform-only (logged, not rejected) — a remote enemy with no mesh simply does
/// not render, exactly as a stateless transform.
///
/// Idempotent: an entity already carrying a `MeshComponent` is left untouched, so a
/// re-baseline / re-apply does not duplicate or reset the live mesh animation state.
///
/// Returns `true` if a mesh presentation is now present (materialized this call or
/// already there), `false` if the descriptor is unregistered or has no mesh block.
pub(crate) fn materialize_net_remote_enemy_presentation(
    entity_class: &str,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
    id: EntityId,
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
            "[Net] remote enemy entity_class `{entity_class}` not registered; \
             leaving remote pawn transform-only (will not render)"
        );
        return false;
    };
    let Some(mesh_desc) = descriptor.mesh.as_ref() else {
        log::debug!(
            "[Net] remote enemy entity_class `{entity_class}` has no mesh block; \
             leaving remote pawn transform-only (will not render)"
        );
        return false;
    };

    // Same materialization the data-archetype mesh path uses: no `animations` block
    // ⇒ stateless mesh (model handle only); otherwise copy the declared state map in
    // via `MeshAnimation::new` (current = default state, entry stamp pending). Parse
    // validation guarantees `default_state` is `Some` exactly when the map is
    // non-empty and names a declared state.
    let component = match &mesh_desc.default_state {
        Some(default_state) => MeshComponent {
            model: mesh_desc.model.clone(),
            animation: Some(MeshAnimation::new(
                mesh_desc.animations.clone(),
                default_state.clone(),
            )),
        },
        None => MeshComponent::stateless(mesh_desc.model.clone()),
    };
    // `set_component` only fails on a stale id; the caller proved the pawn live.
    let _ = registry.set_component(id, component);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::mesh::{AnimationState, InterruptPolicy};
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, MeshDescriptor,
        PlayerMovementDescriptor, SpeedParams,
    };
    use crate::scripting::provenance::DescriptorProvenance;
    use crate::scripting::registry::Transform;
    use glam::{Quat, Vec3};
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
                    clip_index: None,
                },
            );
            (states, Some("idle".to_string()))
        } else {
            (HashMap::new(), None)
        };

        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: Some(MeshDescriptor {
                model: "decraniated".to_string(),
                animations,
                default_state,
            }),
            health: None,
            ai: None,
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

        let attached = materialize_net_remote_enemy_presentation(
            "decraniated_mob",
            &descriptors,
            &mut reg,
            id,
        );
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
    }

    #[test]
    fn remote_enemy_presentation_attaches_stateless_mesh() {
        let descriptors = vec![enemy_mesh_descriptor("prop_enemy", false)];
        let mut reg = EntityRegistry::new();
        let id = spawn_transform_only(&mut reg);

        assert!(materialize_net_remote_enemy_presentation(
            "prop_enemy",
            &descriptors,
            &mut reg,
            id
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

        materialize_net_remote_enemy_presentation("decraniated_mob", &descriptors, &mut reg, id);

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

        assert!(materialize_net_remote_enemy_presentation(
            "decraniated_mob",
            &descriptors,
            &mut reg,
            id
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
            materialize_net_remote_enemy_presentation(
                "decraniated_mob",
                &descriptors,
                &mut reg,
                id
            ),
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

        let attached =
            materialize_net_remote_enemy_presentation("not_a_class", &descriptors, &mut reg, id);
        assert!(!attached, "unknown class attaches nothing");
        assert_eq!(
            reg.has_component_kind(id, ComponentKind::Mesh),
            Ok(false),
            "unknown class leaves the entity transform-only"
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
            view_feel: None,
        }
    }

    fn player_with_movement(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(classname.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: Some(movement_descriptor()),
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
        }
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

    // The net-slot path defaults `entity_class` to "player", matching
    // spawn_from_player_starts; an unregistered entity_class is skipped (None).
    #[test]
    fn net_slot_pawn_defaults_to_player_and_skips_unknown_class() {
        let mut reg = EntityRegistry::new();
        let descriptors = vec![player_with_movement("player")];

        // Default entity_class -> "player".
        let default_placement = spawn_point(&[]);
        assert!(spawn_net_slot_pawn(&default_placement, &descriptors, &mut reg, None).is_some());

        // Explicit unknown entity_class -> skipped.
        let unknown = spawn_point(&[("entity_class", "no_such_class")]);
        assert!(spawn_net_slot_pawn(&unknown, &descriptors, &mut reg, None).is_none());
    }
}
