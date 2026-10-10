// Descriptor classification and connected-client placement suppression for
// entities whose authoritative state arrives through host snapshots.

use std::collections::HashSet;

use glam::Vec3;
use postretro_entities::components::mesh::capsule_center_to_feet_origin_offset;
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

/// Whether materializing this descriptor would attach the engine-owned AI pair
/// (`ComponentKind::Brain` + `ComponentKind::Agent`) — i.e. whether the
/// descriptor carries a brain block. This is the *pre-materialization* mirror of
/// the live-component predicate
/// `postretro-netcode`'s live networked-enemy predicate,
/// which can only inspect those components AFTER an entity exists: a brain block
/// is the sole thing `attach_descriptor_components` keys the `Brain` + `Agent`
/// attachment on, so [`descriptor_carries_brain`] holds exactly when that
/// predicate would later return `true` for an eligible descriptor spawn of this
/// descriptor.
///
/// Used by the connected-client install path to drop AI-enemy map
/// placements *before* dispatch, since those enemies must arrive only as
/// host-authoritative snapshots — never as locally-spawned authoritative copies.
/// Keying on the brain block (not classname strings, not
/// `DescriptorProvenance.owned_components`, which never tracks AI) keeps the same
/// single definition of "AI map enemy" the live predicate enforces.
pub fn descriptor_materializes_ai_enemy(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor_carries_brain(descriptor)
}

/// Whether this descriptor materializes a host-authoritative world item. A world
/// item is defined solely by its `touchable` block; the host derives outbound
/// replication membership from the corresponding live component.
pub fn descriptor_materializes_world_item(descriptor: &EntityTypeDescriptor) -> bool {
    descriptor.touchable.is_some()
}

/// Partition map placements for a **connected client** install:
/// returns only the placements that should still materialize locally, dropping
/// any whose matched descriptor would materialize a host-authoritative AI enemy
/// ([`descriptor_materializes_ai_enemy`]) or world item
/// ([`descriptor_materializes_world_item`]). Those entities reach the client solely
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
                Some(descriptor) => {
                    !descriptor_materializes_ai_enemy(descriptor)
                        && !descriptor_materializes_world_item(descriptor)
                }
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
/// when its matched descriptor materializes either an AI enemy or world item and
/// carries a `mesh` block with a non-empty `model`; ordinary placements and
/// meshless descriptors contribute nothing.
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
        if !descriptor_materializes_ai_enemy(descriptor)
            && !descriptor_materializes_world_item(descriptor)
        {
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
    fn descriptor_materializes_world_item_when_touchable() {
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });

        assert!(descriptor_materializes_world_item(&item));
        assert!(!descriptor_materializes_world_item(&mesh_descriptor(
            "crate", false
        )));
    }

    #[test]
    fn client_filter_drops_host_replicated_placements_keeps_props() {
        // The connected-client pre-dispatch filter drops behavior-authored AI enemies
        // and touchable world items, while keeping ordinary local props.
        let mut item = weapon_descriptor("reference_pistol");
        item.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 32.0,
        });
        let descriptors = vec![
            behavior_enemy_descriptor("grunt"),
            item,
            mesh_descriptor("crate", false),
        ];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
        ];

        let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);

        assert_eq!(
            kept.len(),
            1,
            "host-replicated placements drop, crate stays"
        );
        assert_eq!(kept[0].classname, "crate");
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
        let descriptors = vec![grunt, item, mesh_descriptor("crate", false)];
        let placements = vec![
            placement("grunt", &[]),
            placement("reference_pistol", &[]),
            placement("crate", &[]),
            placement("grunt", &[]),
            placement("mystery", &[]),
        ];

        let models = suppressed_client_host_replicated_mesh_models(&placements, &descriptors);

        // Both host-replicated categories contribute their models, while the ordinary
        // crate and unknown classname add nothing.
        assert_eq!(
            models,
            vec![
                "decraniated".to_string(),
                "models/grunt_prop.gltf".to_string(),
                "models/pistol_world.gltf".to_string(),
            ],
            "suppressed host-replicated models must preload"
        );
    }

    #[test]
    fn suppressed_host_replicated_mesh_models_empty_without_suppressed_placements() {
        // No map-referenced host-replicated placement means the ordinary registry
        // sweep already covers every mesh.
        let descriptors = vec![mesh_descriptor("crate", false)];
        let placements = vec![placement("crate", &[]), placement("mystery", &[])];

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
    fn client_filtered_dispatch_spawns_no_brain_but_host_dispatch_does() {
        // End-to-end on the dispatch seam: the SAME placements + descriptors
        // produce a Brain-bearing entity through the unfiltered (host /
        // single-player) path but NONE through the connected-client filtered
        // path. The non-AI prop materializes in BOTH.
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
            client_crates, 1,
            "connected client still materializes the non-AI prop"
        );
    }

    #[test]
    fn behavior_descriptor_materialization_yields_live_brain_and_agent() {
        // Invariant the suppression mechanism rests on: a brain descriptor
        // materialization attaches BOTH live `Brain` and `Agent` columns.
        // Without this, `is_networked_ai_enemy` (which reads those live columns)
        // and the pre-materialization `descriptor_materializes_ai_enemy` could
        // disagree.
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
