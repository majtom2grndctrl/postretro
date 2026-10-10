// Descriptor-class metadata and authoritative descriptor-world classification.
// See: context/lib/networking.md

use postretro_net::wire::ComponentPayload;

use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath, ReplicationRole,
};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry};

const DESCRIPTOR_CLASS_PREFIX: &str = "descriptor:";
const PROJECTILE_CLASS_PREFIX: &str = "projectile:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplicatedDescriptorClass<'a> {
    Descriptor(&'a str),
    Projectile(&'a str),
}

impl<'a> ReplicatedDescriptorClass<'a> {
    pub(super) fn canonical_name(self) -> &'a str {
        match self {
            Self::Descriptor(name) | Self::Projectile(name) => name,
        }
    }
}

pub(super) fn decode_replicated_descriptor_class(value: &str) -> ReplicatedDescriptorClass<'_> {
    if let Some(name) = value.strip_prefix(PROJECTILE_CLASS_PREFIX) {
        ReplicatedDescriptorClass::Projectile(name)
    } else if let Some(name) = value.strip_prefix(DESCRIPTOR_CLASS_PREFIX) {
        ReplicatedDescriptorClass::Descriptor(name)
    } else {
        // Untagged values remain readable for local fixtures and older in-memory
        // producers. Canonical-name consumers must always pass through this decoder;
        // the replicated value is a presentation classifier, not a descriptor key.
        ReplicatedDescriptorClass::Descriptor(value)
    }
}

fn descriptor_class(name: &str) -> String {
    format!("{DESCRIPTOR_CLASS_PREFIX}{name}")
}

fn projectile_class(name: &str) -> String {
    format!("{PROJECTILE_CLASS_PREFIX}{name}")
}

/// Live world-entity classification. Canonical inventory ownership takes precedence
/// over retained descriptor components; pawn and projectile paths remain separate.
pub(crate) fn is_host_replicated_descriptor_entity(
    registry: &EntityRegistry,
    id: EntityId,
) -> bool {
    let Ok(provenance) = registry.get_component::<DescriptorProvenance>(id) else {
        return false;
    };
    if !matches!(
        provenance.spawn_path,
        DescriptorSpawnPath::MapPlacement
            | DescriptorSpawnPath::RuntimeSpawn
            | DescriptorSpawnPath::DefaultWeapon
    ) || registry.is_inventory_owned(id)
    {
        return false;
    }
    DescriptorComponentKind::ALL.into_iter().any(|kind| {
        kind.replication_role() == ReplicationRole::NetworksEntity
            && registry
                .has_component_kind(id, kind.component_kind())
                .unwrap_or(false)
    }) || (registry
        .has_component_kind(id, ComponentKind::Brain)
        .unwrap_or(false)
        && registry
            .has_component_kind(id, ComponentKind::Agent)
            .unwrap_or(false))
}

/// Snapshot class for descriptor-world entities and the separate pawn/projectile
/// paths. The client resolves this name to build descriptor presentation.
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
        return Some(descriptor_class(&provenance.canonical_name));
    }

    // Projectile observer copies deliberately carry no gameplay component. Their
    // dedicated provenance path is therefore the only classifier signal; the client
    // resolves the weapon descriptor and attaches its visual-only body/trail.
    if matches!(
        provenance.spawn_path,
        DescriptorSpawnPath::ProjectilePresentation
    ) {
        return Some(projectile_class(&provenance.canonical_name));
    }

    // World descriptor records carry the class needed to build their display copy.
    if is_host_replicated_descriptor_entity(registry, id) {
        return Some(descriptor_class(&provenance.canonical_name));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::touchable::TouchableComponent;
    use postretro_foundation::TouchMode;
    use postretro_net::wire::WireTransform;

    use crate::scripting::builtins::data_archetype::{
        apply_data_archetype_dispatch, find_descriptor,
    };
    use crate::scripting::builtins::data_archetype_test_fixtures::{
        behavior_enemy_descriptor, mesh_descriptor, placement,
    };
    use crate::scripting::builtins::descriptor_is_host_replicated;

    #[test]
    fn classifier_agrees_with_live_predicate_one_source_of_truth() {
        use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
        use postretro_entities::data_descriptors::{
            HealthDescriptor, LightDescriptor, TouchableDescriptor,
        };
        let health = HealthDescriptor {
            max: 48.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        };
        let light = LightDescriptor {
            color: [1.0; 3],
            intensity: 1.0,
            range: 8.0,
            is_dynamic: true,
        };
        let emitter = BillboardEmitterComponent {
            rate: 1.0,
            burst: None,
            spread: 0.0,
            lifetime: 1.0,
            velocity: [0.0; 3],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: [1.0].into(),
            opacity_over_lifetime: [1.0].into(),
            color: [1.0; 3],
            sprite: "smoke".into(),
            spin_rate: 0.0,
            spin_animation: None,
        };
        let mut world_item = mesh_descriptor("world_item", false);
        world_item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 1.0,
        });
        let mut dummy = mesh_descriptor("dummy", false);
        dummy.health = Some(health.clone());
        let mesh_prop = mesh_descriptor("mesh_prop", false);
        let mut invisible = mesh_descriptor("invisible", false);
        invisible.mesh = None;
        invisible.health = Some(health.clone());
        let mut local_light = mesh_descriptor("local_light", false);
        local_light.mesh = None;
        local_light.light = Some(light.clone());
        let mut local_emitter = mesh_descriptor("local_emitter", false);
        local_emitter.mesh = None;
        local_emitter.emitter = Some(emitter.clone());
        let mut lit_prop = mesh_descriptor("lit_prop", false);
        lit_prop.light = Some(light);
        let mut health_emitter = mesh_descriptor("health_emitter", false);
        health_emitter.mesh = None;
        health_emitter.health = Some(health);
        health_emitter.emitter = Some(emitter);
        let cases = vec![
            (behavior_enemy_descriptor("grunt"), true),
            (world_item, true),
            (dummy, true),
            (mesh_prop, true),
            (invisible, true),
            (local_light, false),
            (local_emitter, false),
            (lit_prop, true),
            (health_emitter, true),
        ];
        let descriptors: Vec<_> = cases
            .iter()
            .map(|(descriptor, _)| descriptor.clone())
            .collect();
        let placements: Vec<_> = descriptors
            .iter()
            .map(|descriptor| placement(descriptor.canonical_name.as_deref().unwrap(), &[]))
            .collect();
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);
        assert_eq!(
            reg.iter_with_kind(ComponentKind::DescriptorProvenance)
                .count(),
            cases.len()
        );
        for (id, _) in reg.iter_with_kind(ComponentKind::DescriptorProvenance) {
            let provenance = reg.get_component::<DescriptorProvenance>(id).unwrap();
            let descriptor = find_descriptor(&descriptors, &provenance.canonical_name).unwrap();
            let expected = cases
                .iter()
                .find(|(case, _)| case.canonical_name == descriptor.canonical_name)
                .unwrap()
                .1;
            assert_eq!(
                descriptor_is_host_replicated(descriptor),
                expected,
                "descriptor {}",
                provenance.canonical_name
            );
            assert_eq!(
                is_host_replicated_descriptor_entity(&reg, id),
                expected,
                "live {}",
                provenance.canonical_name
            );
        }
    }

    #[test]
    fn runtime_spawn_stamps_descriptor_class_and_behavior_requires_live_pair() {
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

        assert!(is_host_replicated_descriptor_entity(&reg, grunt));
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
            Some("descriptor:grunt".to_string()),
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
            is_host_replicated_descriptor_entity(&reg, crate_id),
            "a runtime-spawned mesh descriptor is networked"
        );

        // Isolate behavior's signal from the independently networking mesh.
        reg.remove_component_kind(grunt, ComponentKind::Mesh)
            .unwrap();
        let agent = reg
            .get_component::<AgentComponent>(grunt)
            .expect("AI descriptor materializes an Agent")
            .clone();
        reg.remove_component_kind(grunt, ComponentKind::Agent)
            .expect("remove Agent from live AI fixture");
        assert!(
            !is_host_replicated_descriptor_entity(&reg, grunt),
            "RuntimeSpawn with a missing Agent is not a networked AI enemy"
        );

        reg.set_component(grunt, agent)
            .expect("restore Agent to live AI fixture");
        reg.remove_component_kind(grunt, ComponentKind::Brain)
            .expect("remove Brain from live AI fixture");
        assert!(
            !is_host_replicated_descriptor_entity(&reg, grunt),
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
            Some("projectile:plasma_rifle".to_string()),
            "Transform-only projectile presentation records name the weapon descriptor"
        );
    }

    #[test]
    fn replicated_descriptor_class_distinguishes_world_item_from_projectile_capability() {
        let world_item = decode_replicated_descriptor_class("descriptor:plasma_rifle");
        let projectile = decode_replicated_descriptor_class("projectile:plasma_rifle");

        assert_eq!(
            world_item,
            ReplicatedDescriptorClass::Descriptor("plasma_rifle")
        );
        assert_eq!(
            projectile,
            ReplicatedDescriptorClass::Projectile("plasma_rifle")
        );
    }

    #[test]
    fn touchable_projectile_capable_descriptor_stays_world_item_representation() {
        // Regression: the client classified solely from the descriptor's weapon
        // capability and rendered a replicated pickup as an in-flight projectile.
        let mut registry = EntityRegistry::new();
        let item = registry.spawn(postretro_entities::Transform::default());
        registry
            .set_component(
                item,
                DescriptorProvenance {
                    canonical_name: "plasma_rifle".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        registry
            .set_component(
                item,
                TouchableComponent {
                    mode: TouchMode::Auto,
                    radius: 1.0,
                },
            )
            .unwrap();

        let encoded = descriptor_entity_class(
            &registry,
            item,
            &[ComponentPayload::Transform(WireTransform {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            })],
        )
        .expect("world item carries its descriptor representation");
        assert_eq!(encoded, "descriptor:plasma_rifle");
        assert_eq!(
            decode_replicated_descriptor_class(&encoded),
            ReplicatedDescriptorClass::Descriptor("plasma_rifle"),
        );
    }
}
