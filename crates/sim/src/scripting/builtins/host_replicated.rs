// Host descriptor classification and connected-client placement suppression.
// See: context/lib/networking.md

use std::collections::HashSet;

use glam::Vec3;
use postretro_entities::components::mesh::capsule_center_to_feet_origin_offset;
use postretro_entities::provenance::{DescriptorComponentKind, ReplicationRole};
use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::MapEntity;
use super::data_archetype::{DEFAULT_AGENT_PARAMS, find_descriptor};

pub fn ai_capsule_center_from_feet_offset(
    descriptor: &EntityTypeDescriptor,
    agent_params: Option<NavAgentParams>,
) -> Vec3 {
    if !descriptor_carries_brain(descriptor) {
        return Vec3::ZERO;
    }
    let params = agent_params.unwrap_or(DEFAULT_AGENT_PARAMS);
    -capsule_center_to_feet_origin_offset(params.radius, params.height)
}

/// Whether this descriptor authors a behavior-graph brain.
pub(super) fn descriptor_carries_brain(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.behavior.is_some()
}

/// Whether this descriptor's behavior block causes the engine-owned AI pair
/// (`Brain` + `Agent`) to be attached. The AI-only spawner resolver uses this
/// pre-materialization gate; generic host replication also includes descriptors
/// declaring `Mesh`, `Health`, or `Touchable` and uses
/// [`descriptor_is_host_replicated`].
pub fn descriptor_materializes_ai_enemy(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor_carries_brain(descriptor)
}

/// Whether a world placement declares host-owned gameplay or mesh state.
/// Inventory ownership is a live-entity concern; the placement classifier sees
/// only the descriptor, before materialization or acquisition.
pub fn descriptor_is_host_replicated(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor_carries_brain(descriptor)
        || DescriptorComponentKind::ALL.into_iter().any(|kind| {
            kind.replication_role() == ReplicationRole::NetworksEntity && descriptor.declares(kind)
        })
}

/// Partition map placements for a **connected client** install:
/// returns only the placements that should still materialize locally, dropping
/// any whose matched descriptor is host-replicated
/// ([`descriptor_is_host_replicated`]). Those entities reach the client solely
/// via host snapshots; spawning a local copy here would duplicate their
/// host-authoritative state.
///
/// Placements whose classname has no descriptor match are retained untouched so the
/// downstream dispatch handles their unknown-classname / built-in-collision diagnostics
/// exactly as it would on a host. Single-player and listen-host installs never call this
/// (they keep every placement); only the connected-client lifecycle path filters.
pub fn filter_out_client_host_replicated_placements(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<MapEntity> {
    entities
        .iter()
        .filter(
            |entity| match find_descriptor(descriptors, &entity.classname) {
                Some(descriptor) => !descriptor_is_host_replicated(descriptor),
                // No descriptor match: retain for the normal unknown-classname
                // diagnostics in dispatch.
                None => true,
            },
        )
        .cloned()
        .collect()
}

/// Collect the distinct, non-empty mesh model handles referenced by the
/// host-authoritative map placements a connected client suppresses
/// ([`filter_out_client_host_replicated_placements`]), preserving first-seen order. GPU-free:
/// this is the pure analogue of [`crate::distinct_mesh_models`] for placements
/// that never spawn a local `MeshComponent` on a connected client, so the
/// registry-driven sweep cannot see them.
///
/// Scoped to the classes the **map actually references** (the placements passed
/// in), not every descriptor in the data registry — only host-replicated map
/// entities in this level need their model on the GPU. A placement is included only
/// when its matched descriptor is host-replicated and
/// carries a `mesh` block with a non-empty `model`; client-local presentation
/// placements and meshless descriptors contribute nothing.
///
/// Regression (E10 AC #3): a connected client filtered out the AI-enemy
/// placement before dispatch, so its model was never in the registry-driven
/// upload set; when the host snapshot later materialized the remote enemy the
/// draw planner dropped it (no uploaded mesh in the model cache) and the real
/// model never rendered — only a dev-tools debug capsule showed. The level-load
/// sweep unions these handles with [`crate::distinct_mesh_models`] so the
/// suppressed entity's model is uploaded up front.
///
/// Each returned string is the VERBATIM renderer cache key (the descriptor's
/// holder `mesh.model` or attachment model), identical in shape to
/// [`crate::distinct_mesh_models`] output, so the caller can dedup the two sets
/// and upload each handle once.
pub fn suppressed_client_host_replicated_mesh_models(
    entities: &[MapEntity],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for entity in entities {
        let Some(descriptor) = find_descriptor(descriptors, &entity.classname) else {
            continue;
        };
        if !descriptor_is_host_replicated(descriptor) {
            continue;
        }
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        if !mesh.model.is_empty() && seen.insert(mesh.model.clone()) {
            ordered.push(mesh.model.clone());
        }
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            if !attachment_model.is_empty() && seen.insert(attachment_model.to_string()) {
                ordered.push(attachment_model.to_string());
            }
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::super::data_archetype::{apply_data_archetype_dispatch, descriptor_mesh_component};
    use super::super::data_archetype_test_fixtures::{
        behavior_enemy_descriptor, mesh_descriptor, placement,
    };
    use super::*;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_entities::registry::{ComponentKind, EntityRegistry};
    use postretro_scripting_core::data_descriptors::{
        LightDescriptor, ResolutionMode, TouchMode, TouchableDescriptor, WeaponDescriptor,
    };

    fn light_descriptor(classname: &str, is_dynamic: bool) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            faction: None,
            tolerance: None,
            canonical_name: Some(classname.to_string()),
            inventory: None,
            light: Some(LightDescriptor {
                color: [0.5, 0.5, 0.5],
                intensity: 1.0,
                range: 8.0,
                is_dynamic,
            }),
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn weapon_descriptor(classname: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
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
                damage: 12.0,
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
                    180.0,
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

    #[test]
    fn behavior_descriptor_materializes_ai_enemy() {
        // An authored behavior graph is the sole AI classifier; light,
        // mesh, and health-only descriptors are non-AI props.
        assert!(descriptor_materializes_ai_enemy(
            &behavior_enemy_descriptor("grunt")
        ));
        assert!(!descriptor_materializes_ai_enemy(&mesh_descriptor(
            "prop", false
        )));
        assert!(!descriptor_materializes_ai_enemy(&light_descriptor(
            "torch", true
        )));
    }

    #[test]
    fn descriptor_classifier_and_filter_cover_all_world_placement_shapes() {
        use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
        use postretro_scripting_core::data_descriptors::HealthDescriptor;

        let health = HealthDescriptor {
            max: 90.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        };
        let mut item = weapon_descriptor("item");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 1.0,
        });
        let mut mesh_health = mesh_descriptor("mesh_health", false);
        mesh_health.health = Some(health.clone());
        let mut health_only = mesh_descriptor("health", false);
        health_only.mesh = None;
        health_only.health = Some(health.clone());
        let mut emitter = light_descriptor("emitter", true);
        emitter.light = None;
        emitter.emitter = Some(BillboardEmitterComponent {
            rate: 1.0,
            burst: None,
            spread: 0.0,
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
        let mut light_mesh = light_descriptor("light_mesh", true);
        light_mesh.mesh = mesh_descriptor("light_mesh", false).mesh;
        let mut emitter_health = emitter.clone();
        emitter_health.canonical_name = Some("emitter_health".into());
        emitter_health.health = Some(health);
        let descriptors = vec![
            behavior_enemy_descriptor("enemy"),
            item,
            mesh_health,
            mesh_descriptor("mesh", false),
            health_only,
            light_descriptor("light", true),
            emitter,
            light_mesh,
            emitter_health,
        ];
        let expected = [true, true, true, true, true, false, false, true, true];
        let placements: Vec<_> = descriptors
            .iter()
            .map(|descriptor| placement(descriptor.canonical_name.as_deref().unwrap(), &[]))
            .collect();
        for (descriptor, expected) in descriptors.iter().zip(expected) {
            assert_eq!(
                descriptor_is_host_replicated(descriptor),
                expected,
                "shape {:?}",
                descriptor.canonical_name
            );
        }
        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);
        assert_eq!(
            kept.iter()
                .map(|entity| entity.classname.as_str())
                .collect::<Vec<_>>(),
            ["light", "emitter"]
        );
        assert!(!descriptor_is_host_replicated(&weapon_descriptor(
            "held_only"
        )));
    }

    #[test]
    fn descriptor_is_host_replicated_when_touchable_or_mesh() {
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });

        assert!(descriptor_is_host_replicated(&item));
        assert!(descriptor_is_host_replicated(&mesh_descriptor(
            "crate", false
        )));
    }

    #[test]
    fn client_filter_drops_host_replicated_placements_keeps_local_presentation() {
        // Gameplay and mesh placements arrive from the host; light-only content
        // remains a local placement.
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            item,
            mesh_descriptor("crate", false),
            light_descriptor("torch", true),
        ];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("torch", &[]),
            placement("grunt", &[]),
        ];

        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);

        assert_eq!(
            kept.len(),
            1,
            "host-replicated placements drop, local light stays"
        );
        assert_eq!(kept[0].classname, "torch");
    }

    #[test]
    fn suppressed_host_replicated_mesh_models_collects_filtered_models_for_upload() {
        // Regression: a connected client filters host-replicated placements out
        // before dispatch, so their models are absent from the registry-driven upload
        // set. This pins the level-load union for both AI enemies and world items.
        let mut grunt = behavior_enemy_descriptor("grunt");
        grunt.mesh.as_mut().unwrap().attachments =
            [("hand".to_string(), "models/grunt_prop.gltf".to_string())]
                .into_iter()
                .collect();
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        item.mesh = mesh_descriptor("reference_pistol", false).mesh;
        item.mesh.as_mut().expect("fixture mesh").model = "models/pistol_world.gltf".to_string();
        let mut prop = mesh_descriptor("crate", false);
        prop.mesh.as_mut().unwrap().model = "models/crate.gltf".to_string();
        let descriptors = vec![grunt, item, prop];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
            placement("mystery", &[]),
        ];

        let models = suppressed_client_host_replicated_mesh_models(&placements, &descriptors);

        // Enemies, items and mesh-only props all contribute; unknown names do not.
        assert_eq!(
            models,
            vec![
                "decraniated".to_string(),
                "models/grunt_prop.gltf".to_string(),
                "models/pistol_world.gltf".to_string(),
                "models/crate.gltf".to_string(),
            ],
            "suppressed host-replicated models must preload"
        );
    }

    #[test]
    fn suppressed_host_replicated_mesh_models_empty_without_suppressed_placements() {
        // No map-referenced host-replicated placement means the ordinary registry
        // sweep already covers every mesh.
        let descriptors = vec![light_descriptor("torch", true)];
        let placements = vec![placement("torch", &[]), placement("mystery", &[])];

        assert!(
            suppressed_client_host_replicated_mesh_models(&placements, &descriptors).is_empty()
        );
    }

    #[test]
    fn client_filter_retains_unknown_classname_placements() {
        // A placement with no descriptor match is not host-replicated; the filter
        // retains it so the dispatch's own unknown-classname diagnostics fire.
        let descriptors = vec![behavior_enemy_descriptor("grunt")];
        let placements = vec![placement("mystery", &[]), placement("grunt", &[])];

        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].classname, "mystery");
    }

    #[test]
    fn client_filtered_dispatch_spawns_no_gameplay_or_mesh_copies() {
        // End-to-end on the dispatch seam: the SAME placements + descriptors
        // produce a Brain-bearing entity through the unfiltered (host /
        // single-player) path but NONE through the connected-client filtered
        // path. Mesh-only props also arrive only from host snapshots.
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            mesh_descriptor("crate", false),
        ];
        let placements = vec![placement("grunt", &[]), placement("crate", &[])];

        // Host / single-player: every placement dispatched unfiltered.
        let mut host_reg = EntityRegistry::new();
        apply_data_archetype_dispatch(
            &placements,
            &descriptors,
            &HashSet::new(),
            &mut host_reg,
            None,
        );
        assert!(
            host_reg
                .iter_with_kind(ComponentKind::Brain)
                .next()
                .is_some(),
            "host/single-player materializes the AI enemy locally"
        );
        let host_crates = host_reg
            .iter_with_kind(ComponentKind::Mesh)
            .filter(|(id, _)| {
                host_reg
                    .get_component::<DescriptorProvenance>(*id)
                    .map(|p| p.canonical_name == "crate")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(host_crates, 1, "host materializes the non-AI prop");

        // Connected client: filter host-replicated placements before dispatch.
        let mut client_reg = EntityRegistry::new();
        let client_placements =
            filter_out_client_host_replicated_placements(&placements, &descriptors);
        apply_data_archetype_dispatch(
            &client_placements,
            &descriptors,
            &HashSet::new(),
            &mut client_reg,
            None,
        );
        assert!(
            client_reg
                .iter_with_kind(ComponentKind::Brain)
                .next()
                .is_none(),
            "connected client must NOT spawn a local authoritative AI enemy"
        );
        assert!(
            client_reg
                .iter_with_kind(ComponentKind::Agent)
                .next()
                .is_none(),
            "no Agent either — the AI pair is suppressed together"
        );
        let client_crates = client_reg
            .iter_with_kind(ComponentKind::Mesh)
            .filter(|(id, _)| {
                client_reg
                    .get_component::<DescriptorProvenance>(*id)
                    .map(|p| p.canonical_name == "crate")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            client_crates, 0,
            "connected client suppresses the host-owned mesh prop"
        );
    }

    #[test]
    fn behavior_descriptor_materialization_yields_live_brain_and_agent() {
        // The AI-only descriptor gate must match materialization: a behavior
        // block attaches both live `Brain` and `Agent` components.
        let mut descriptor = behavior_enemy_descriptor("grunt");
        descriptor.behavior.as_mut().unwrap().knockback.scale = 0.25;
        let descriptors = vec![descriptor];
        let placements = vec![placement("grunt", &[])];
        let mut reg = EntityRegistry::new();
        apply_data_archetype_dispatch(&placements, &descriptors, &HashSet::new(), &mut reg, None);

        let (id, _) = reg
            .iter_with_kind(ComponentKind::Brain)
            .next()
            .expect("behavior descriptor materializes a Brain");
        let agent = reg
            .get_component::<postretro_entities::components::agent::AgentComponent>(id)
            .unwrap();
        assert!((agent.knockback.scale - 0.25).abs() < f32::EPSILON);
        assert!(
            matches!(reg.has_component_kind(id, ComponentKind::Agent), Ok(true)),
            "behavior descriptor materializes an Agent alongside the Brain"
        );
    }

    #[test]
    fn behavior_descriptor_gets_capsule_center_to_feet_offsets() {
        // The other two branches gated on "carries a brain": the spawn transform
        // shift and the mesh render-origin offset. They are equal-and-opposite.
        use postretro_foundation::NavAgentParams;

        let params = Some(NavAgentParams {
            radius: 0.4,
            height: 1.6,
            step_height: 0.3,
            max_slope_deg: 45.0,
        });
        let descriptor = behavior_enemy_descriptor("grunt");
        let shift = ai_capsule_center_from_feet_offset(&descriptor, params);
        assert_eq!(
            shift,
            Vec3::new(0.0, 0.8, 0.0),
            "the spawn transform lifts feet-authored origins to capsule center"
        );
        let mesh = descriptor_mesh_component(&descriptor, params)
            .expect("behavior descriptor carries a mesh");
        assert_eq!(
            mesh.origin_offset, -shift,
            "the mesh renders back down at the feet"
        );
        assert_eq!(
            descriptor_mesh_component(&mesh_descriptor("crate", false), params)
                .unwrap()
                .origin_offset,
            Vec3::ZERO,
            "a brain-less prop takes no capsule offset at all"
        );
    }
}
