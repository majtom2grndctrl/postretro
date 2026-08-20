// Descriptor-class metadata derivation and networked AI-enemy classification.
// See: context/lib/networking.md

use postretro_net::wire::ComponentPayload;

use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry};

/// Shared predicate: is `id` a descriptor AI enemy that the host owns as an
/// authoritative networked entity (E10 — networked enemy authority)?
///
/// True iff the entity is alive and:
/// - its `DescriptorProvenance.spawn_path` is `MapPlacement` or `RuntimeSpawn` (not a
///   player-start / net-slot / default-weapon spawn), AND
/// - its **live** registry columns carry BOTH `ComponentKind::Brain` AND
///   `ComponentKind::Agent` (the engine-owned AI brain + navigation agent a brain
///   `components.behavior` graph materializes together — see
///   `data_archetype::attach_descriptor_components`).
///
/// Contract notes for importers:
/// - It reads the **live component columns**, NOT `DescriptorProvenance.owned_components`.
///   `owned_components` only tracks the modder-declarable `DescriptorComponentKind` set
///   and never includes engine-owned AI components, so it cannot be used to detect an
///   AI enemy.
/// - It is registry-blind about role: it does not check host/client. Host registration
///   (this task) gates on the role separately; connected-client spawn suppression (Task 5)
///   imports the same predicate to decide which descriptor placements NOT to spawn locally.
/// - Both consumers (`replication.rs` host registration and `descriptor_entity_class`
///   below) reach it via the direct submodule path, not a re-export.
///
/// A stale/despawned id (`get_component` errors) returns `false`.
pub(crate) fn is_networked_ai_enemy(registry: &EntityRegistry, id: EntityId) -> bool {
    let Ok(provenance) = registry.get_component::<DescriptorProvenance>(id) else {
        return false;
    };
    if !matches!(
        provenance.spawn_path,
        DescriptorSpawnPath::MapPlacement | DescriptorSpawnPath::RuntimeSpawn
    ) {
        return false;
    }
    matches!(
        registry.has_component_kind(id, ComponentKind::Brain),
        Ok(true)
    ) && matches!(
        registry.has_component_kind(id, ComponentKind::Agent),
        Ok(true)
    )
}

/// The descriptor class a replicable entity was materialized from, for the snapshot's
/// `entity_class` (M15 Phase 3 Task 7 / E16 Task 5). The recipient uses it to materialize
/// the matching descriptor-backed presentation locally. `None` unless the entity is one of:
///
/// - a **movement pawn** (carries a `PlayerMovementState` wire payload) spawned through
///   the net-slot or player-start descriptor path
///   (`DescriptorSpawnPath::NetworkSlot` / `DescriptorSpawnPath::PlayerSpawn`) — its
///   `canonical_name` is the resolved `entity_class` (default `"player"`); or
/// - a **networked AI enemy** ([`is_networked_ai_enemy`]) — its `canonical_name` is
///   the descriptor class the host registered it under.
/// - a **world item** (carries `ComponentKind::Touchable`) — its canonical descriptor
///   class lets the client materialize its mesh presentation from a Transform-only record.
/// - a **projectile presentation** — host-created Transform/provenance-only flight
///   entity whose canonical class names the firing weapon descriptor.
///
/// The wire allows `entity_class` on any non-despawn finite-`Transform` record (E10
/// Task 3 relaxed it off the movement-only gate), so AI enemies and world items ride
/// Transform-only snapshots. A Transform-only fixture / demo mover / map-start pawn that
/// is none of the above returns `None`.
pub(super) fn descriptor_entity_class(
    registry: &EntityRegistry,
    id: EntityId,
    components: &[ComponentPayload],
) -> Option<String> {
    let provenance = registry.get_component::<DescriptorProvenance>(id).ok()?;

    // A movement pawn from either authoritative player path: the wire historically
    // gated `entity_class` on a movement record. `PlayerSpawn` is the listen host's
    // own boot pawn; `NetworkSlot` is an accepted client pawn.
    let carries_movement = components
        .iter()
        .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
    if carries_movement
        && matches!(
            provenance.spawn_path,
            DescriptorSpawnPath::NetworkSlot | DescriptorSpawnPath::PlayerSpawn
        )
    {
        return Some(provenance.canonical_name.clone());
    }

    // Projectile observer copies deliberately carry no gameplay component. Their
    // dedicated provenance path is therefore the only classifier signal; the client
    // resolves the weapon descriptor and attaches its visual-only body/trail.
    if matches!(
        provenance.spawn_path,
        DescriptorSpawnPath::ProjectilePresentation
    ) {
        return Some(provenance.canonical_name.clone());
    }

    // A map-placed/runtime AI enemy or a live world item needs its descriptor class
    // so the client can materialize mesh presentation from the Transform-only record.
    // World-item membership deliberately keys only on the host-local touchable component.
    if is_networked_ai_enemy(registry, id)
        || registry
            .has_component_kind(id, ComponentKind::Touchable)
            .unwrap_or(false)
    {
        return Some(provenance.canonical_name.clone());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use postretro_entities::components::agent::AgentComponent;
    use postretro_net::wire::WireTransform;

    use crate::scripting::builtins::data_archetype::{
        apply_data_archetype_dispatch, descriptor_materializes_ai_enemy, find_descriptor,
    };
    use crate::scripting::builtins::data_archetype_test_fixtures::{
        behavior_enemy_descriptor, mesh_descriptor, placement,
    };

    #[test]
    fn classifier_agrees_with_live_predicate_one_source_of_truth() {
        // One source of truth: the pre-materialization descriptor classifier
        // (`descriptor_materializes_ai_enemy`, used to FILTER on the client) must
        // agree with the live-component predicate (`is_networked_ai_enemy`,
        // used to REGISTER on the host) for a `MapPlacement` spawn. Materialize
        // an AI enemy and a non-AI prop and assert each side agrees per entity.
        //
        // Lives on the netcode side because only it can see BOTH the scripting
        // classifier and the netcode live predicate — the scripting tree must not
        // reach up into `crate::netcode` (dependency arrow is netcode → scripting).
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![placement("grunt", &[]), placement("crate", &[])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        for (id, _) in reg
            .iter_with_kind(ComponentKind::DescriptorProvenance)
            .collect::<Vec<_>>()
        {
            let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
            let descriptor = find_descriptor(&descriptors, &provenance.canonical_name)
                .expect("descriptor for materialized entity");
            assert_eq!(
                descriptor_materializes_ai_enemy(descriptor),
                is_networked_ai_enemy(&reg, id),
                "pre-materialization classifier and live predicate must agree for `{}`",
                provenance.canonical_name,
            );
        }
    }

    #[test]
    fn runtime_spawn_requires_the_same_live_ai_pair_and_stamps_its_class() {
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![placement("grunt", &[]), placement("crate", &[])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let grunt = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("AI descriptor materializes a Brain")
            .0;
        let mut runtime_provenance = reg
            .get_component::<DescriptorProvenance>(grunt)
            .expect("AI descriptor has provenance")
            .clone();
        runtime_provenance.spawn_path = DescriptorSpawnPath::RuntimeSpawn;
        reg.set_component(grunt, runtime_provenance)
            .expect("live entity accepts updated provenance");

        assert!(is_networked_ai_enemy(&reg, grunt));
        assert_eq!(
            descriptor_entity_class(
                &reg,
                grunt,
                &[ComponentPayload::Transform(WireTransform {
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                })],
            ),
            Some("grunt".to_string()),
            "a Transform-only runtime-spawn snapshot carries the canonical descriptor class"
        );

        let crate_id = reg
            .iter_with_kind(ComponentKind::Mesh)
            .find(|(id, _)| {
                reg.get_component::<DescriptorProvenance>(*id)
                    .is_ok_and(|provenance| provenance.canonical_name == "crate")
            })
            .expect("non-AI prop materializes")
            .0;
        let mut prop_provenance = reg
            .get_component::<DescriptorProvenance>(crate_id)
            .expect("prop has provenance")
            .clone();
        prop_provenance.spawn_path = DescriptorSpawnPath::RuntimeSpawn;
        reg.set_component(crate_id, prop_provenance)
            .expect("live entity accepts updated provenance");
        assert!(
            !is_networked_ai_enemy(&reg, crate_id),
            "RuntimeSpawn alone never makes a non-AI descriptor networked"
        );

        let agent = reg
            .get_component::<AgentComponent>(grunt)
            .expect("AI descriptor materializes an Agent")
            .clone();
        reg.remove_component_kind(grunt, ComponentKind::Agent)
            .expect("remove Agent from live AI fixture");
        assert!(
            !is_networked_ai_enemy(&reg, grunt),
            "RuntimeSpawn with a missing Agent is not a networked AI enemy"
        );

        reg.set_component(grunt, agent)
            .expect("restore Agent to live AI fixture");
        reg.remove_component_kind(grunt, ComponentKind::Brain)
            .expect("remove Brain from live AI fixture");
        assert!(
            !is_networked_ai_enemy(&reg, grunt),
            "RuntimeSpawn with a missing Brain is not a networked AI enemy"
        );
    }

    #[test]
    fn projectile_presentation_provenance_stamps_weapon_descriptor_class() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(postretro_entities::Transform::default());
        reg.set_component(
            id,
            DescriptorProvenance {
                canonical_name: "plasma_rifle".to_string(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::ProjectilePresentation,
            },
        )
        .unwrap();

        assert_eq!(
            descriptor_entity_class(
                &reg,
                id,
                &[ComponentPayload::Transform(WireTransform {
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                })],
            ),
            Some("plasma_rifle".to_string()),
            "Transform-only projectile presentation records name the weapon descriptor"
        );
    }
}
