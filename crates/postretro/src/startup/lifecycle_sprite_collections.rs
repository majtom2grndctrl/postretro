//! Owns sprite-collection registration during level installation.
//! Governing context: `boot_sequence.md`; renderer/draw contract: `rendering_pipeline.md` §7.4.

use std::collections::HashSet;
use std::path::Path;

use crate::render;
use crate::scripting::builtins::data_archetype::ProjectileSpriteCollection;
use crate::scripting::builtins::data_archetype::find_descriptor;
use crate::scripting::builtins::{PLAYER_START_CLASSNAME, descriptor_is_host_replicated};
use crate::scripting_systems::particle_render::ParticleRenderCollector;
use crate::sprite_collection::{
    DEFAULT_SPRITE_SPECULAR_EXPONENT, DEFAULT_SPRITE_SPECULAR_INTENSITY, derive_collection_id,
};
use crate::weapon;
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

pub(super) fn weapon_impact_sprite_registration() -> render::SpriteCollectionRegistration {
    render::SpriteCollectionRegistration {
        baked_sidecar_eligible: false,
        spec_intensity: weapon::impact_spec_intensity(),
        spec_exponent: weapon::impact_spec_exponent(),
        lifetime: weapon::impact_lifetime(),
        emissive: weapon::impact_emissive(),
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpriteCollectionCandidate {
    /// Texture/dedup target. It is deliberately not the render collection id.
    pub(super) asset: String,
    pub(super) lifetime: Option<f32>,
    pub(super) emissive: f32,
    pub(super) spec_intensity: Option<f32>,
    pub(super) spec_exponent: Option<f32>,
    pub(super) frame_duration_ms: Option<f32>,
    pub(super) source: String,
}

impl SpriteCollectionCandidate {
    fn emitter(component: &BillboardEmitterComponent, source: String) -> Self {
        Self {
            asset: component.sprite.clone(),
            lifetime: Some(component.lifetime),
            emissive: 0.0,
            spec_intensity: None,
            spec_exponent: None,
            frame_duration_ms: None,
            source,
        }
    }

    fn collection_id(&self) -> String {
        derive_collection_id(
            &self.asset,
            self.lifetime,
            self.frame_duration_ms,
            self.emissive,
            self.spec_intensity,
            self.spec_exponent,
        )
    }
}

impl From<ProjectileSpriteCollection> for SpriteCollectionCandidate {
    fn from(value: ProjectileSpriteCollection) -> Self {
        Self {
            asset: value.collection,
            lifetime: value.lifetime,
            emissive: value.emissive,
            spec_intensity: None,
            spec_exponent: None,
            frame_duration_ms: value.frame_duration_ms,
            source: value.source,
        }
    }
}

/// Group collection candidates in first-seen order. A `HashMap` supplies the
/// index lookup only; the `Vec` owns the order that defines the fallback
/// collection when runtime content names an unregistered id.
fn group_candidates_by_collection_id(
    candidates: impl IntoIterator<Item = SpriteCollectionCandidate>,
) -> Vec<(String, Vec<SpriteCollectionCandidate>)> {
    let mut collections = Vec::new();
    let mut collection_indices = std::collections::HashMap::<String, usize>::new();

    for candidate in candidates {
        if candidate.asset.is_empty() {
            continue;
        }
        let collection_id = candidate.collection_id();
        let index = match collection_indices.get(&collection_id).copied() {
            Some(index) => index,
            None => {
                let index = collections.len();
                collection_indices.insert(collection_id.clone(), index);
                collections.push((collection_id, Vec::new()));
                index
            }
        };
        collections[index].1.push(candidate);
    }

    collections
}

/// Resolve one already-id-equal candidate into the unchanged per-collection
/// uniform contract. The collection id folds raw cadence fields; frame count is
/// used here only for the existing loop-period derivation.
fn registration_for_candidate(
    collection_id: &str,
    candidate: &SpriteCollectionCandidate,
    frame_count: usize,
    baked_sidecar_eligible: bool,
) -> Result<render::SpriteCollectionRegistration, String> {
    if let Some(spec_exponent) = candidate.spec_exponent
        && !render::sprite_specular_exponent_is_valid(spec_exponent)
    {
        return Err(format!(
            "collection id `{collection_id}` for asset `{}` has invalid specular exponent from `{}` ({spec_exponent}); expected a finite value greater than zero",
            candidate.asset, candidate.source,
        ));
    }

    let lifetime = candidate
        .frame_duration_ms
        .map_or(candidate.lifetime, |ms| {
            Some(ms / 1_000.0 * frame_count.max(1) as f32)
        });
    Ok(render::SpriteCollectionRegistration {
        baked_sidecar_eligible,
        spec_intensity: candidate
            .spec_intensity
            .unwrap_or(DEFAULT_SPRITE_SPECULAR_INTENSITY),
        spec_exponent: candidate
            .spec_exponent
            .unwrap_or(DEFAULT_SPRITE_SPECULAR_EXPONENT),
        lifetime: lifetime.unwrap_or(1.0),
        emissive: candidate.emissive,
    })
}

pub(super) fn map_billboard_sprite_collections(
    entities: &[postretro_level_format::map_entity::MapEntityRecord],
) -> HashSet<String> {
    entities
        .iter()
        .filter(|entity| entity.classname == "billboard_emitter")
        .map(|entity| {
            entity
                .key_values
                .iter()
                .rev()
                .find_map(|(key, value)| (key == "sprite").then_some(value.as_str()))
                .filter(|sprite| !sprite.is_empty())
                .unwrap_or("smoke")
                .to_string()
        })
        .collect()
}

/// Discover the contracts needed before deferred descriptor presentation exists.
/// `suppressed_placements` is empty outside connected-client installs. Resolved
/// spawners contribute only archetypes this level can actually spawn; unused mod
/// descriptors never expand the emitter upload set.
pub(super) fn discover_sprite_collection_candidates(
    registry: &EntityRegistry,
    projectile_sprites: Vec<ProjectileSpriteCollection>,
    suppressed_placements: &[MapEntityRecord],
    descriptors: &[EntityTypeDescriptor],
) -> Vec<SpriteCollectionCandidate> {
    let mut candidates = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
        let ComponentValue::BillboardEmitter(component) = value else {
            continue;
        };
        candidates.push(SpriteCollectionCandidate::emitter(
            component,
            format!("billboard emitter {id}"),
        ));
    }
    candidates.extend(projectile_sprites.into_iter().map(Into::into));

    for placement in suppressed_placements {
        // World install partitions pawn starts before its descriptor filter.
        if placement.classname == PLAYER_START_CLASSNAME {
            continue;
        }
        let Some(descriptor) = find_descriptor(descriptors, &placement.classname) else {
            continue;
        };
        if descriptor_is_host_replicated(descriptor)
            && let Some(emitter) = descriptor.emitter.as_ref()
        {
            candidates.push(SpriteCollectionCandidate::emitter(
                emitter,
                format!("suppressed descriptor {}", placement.classname),
            ));
        }
    }
    for (id, value) in registry.iter_with_kind(ComponentKind::Spawner) {
        let ComponentValue::Spawner(SpawnerComponent {
            archetype_name,
            resolved: true,
            ..
        }) = value
        else {
            continue;
        };
        if let Some(descriptor) = find_descriptor(descriptors, archetype_name)
            && let Some(emitter) = descriptor.emitter.as_ref()
        {
            candidates.push(SpriteCollectionCandidate::emitter(
                emitter,
                format!("resolved spawner {id} ({archetype_name})"),
            ));
        }
    }
    candidates
}

/// Register every level-owned sprite collection in discovery order.
pub(super) fn install_sprite_collections(
    renderer: &mut render::Renderer,
    particle_render: &mut ParticleRenderCollector,
    texture_root: &Path,
    prm_cache_root: &Path,
    candidates: Vec<SpriteCollectionCandidate>,
    map_billboard_collections: &HashSet<String>,
) {
    for (collection_id, candidates) in group_candidates_by_collection_id(candidates) {
        // Candidates with one id are bit-identical by construction, so the
        // first is also the exact draw contract for this registration.
        let candidate = &candidates[0];
        let asset = &candidate.asset;
        // Keep the draw-contract frame count sourced from the runtime sprite
        // loader. A direct `.png` remains one frame here; baked collection
        // sidecars never become a second shader-facing count.
        let frame_count = postretro_render_cpu::smoke::load_sprite_frames(texture_root, asset)
            .map_or(1, |frames| frames.len());
        let registration = match registration_for_candidate(
            &collection_id,
            candidate,
            frame_count,
            map_billboard_collections.contains(asset),
        ) {
            Ok(registration) => registration,
            Err(reason) => {
                log::warn!("[Loader] {reason}; rejecting only this invalid collection id");
                continue;
            }
        };
        renderer.register_smoke_collection(
            &collection_id,
            asset,
            texture_root,
            prm_cache_root,
            registration,
        );
        particle_render.register_sprite(&collection_id);
    }

    let collection_id = weapon::impact_collection_id();
    renderer.register_smoke_collection(
        collection_id,
        weapon::impact_sprite_collection(),
        texture_root,
        prm_cache_root,
        weapon_impact_sprite_registration(),
    );
    particle_render.register_sprite(collection_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitter(sprite: &str, lifetime: f32) -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate: 0.0,
            burst: Some(1),
            spread: 0.0,
            lifetime,
            velocity: [0.0; 3],
            buoyancy: 0.0,
            drag: 0.0,
            size_over_lifetime: vec![1.0].into(),
            opacity_over_lifetime: vec![1.0].into(),
            color: [1.0; 3],
            sprite: sprite.to_string(),
            spin_rate: 0.0,
            spin_animation: None,
        }
    }

    // Regression: suppressed Health/Mesh+Emitter placements registered no sprite
    // contract before their display copies emitted, so particles drew the fallback.
    #[test]
    fn suppressed_descriptor_emitters_collect_the_discovered_contract_without_fallback() {
        use crate::scripting::builtins::data_archetype::apply_data_archetype_dispatch;
        use crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor;
        use crate::scripting::builtins::filter_out_client_host_replicated_placements;
        use crate::scripting::builtins::net_descriptor::materialize_net_descriptor_presentation;
        use crate::scripting::map_entity::MapEntity;
        use crate::scripting_systems::emitter_bridge::EmitterBridge;
        use postretro_entities::Transform;
        use postretro_entities::components::health::HealthComponent;
        use postretro_entities::components::sprite_visual::SpriteVisual;
        use postretro_scripting_core::data_descriptors::HealthDescriptor;
        use postretro_test_log_capture::LogCapture;
        use postretro_visibility::VisibleCells;

        let capture = LogCapture::start();
        for health_only in [true, false] {
            let mut descriptor = mesh_descriptor("networked_plume", false);
            if health_only {
                descriptor.mesh = None;
                descriptor.health = Some(HealthDescriptor {
                    max: 30.0,
                    hitbox: None,
                    zone_multipliers: Default::default(),
                });
            }
            descriptor.emitter = Some(emitter("unique_plume", 0.45));
            let descriptors = vec![descriptor];
            let records = vec![MapEntityRecord {
                classname: "networked_plume".to_string(),
                ..Default::default()
            }];
            let placements: Vec<MapEntity> = records.iter().cloned().map(Into::into).collect();
            let kept = filter_out_client_host_replicated_placements(&placements, &descriptors);
            assert!(
                kept.is_empty(),
                "the real connected-client filter suppresses this placement"
            );
            let mut registry = EntityRegistry::new();
            apply_data_archetype_dispatch(
                &kept,
                &descriptors,
                &HashSet::new(),
                &mut registry,
                None,
            );
            assert_eq!(
                registry
                    .iter_with_kind(ComponentKind::BillboardEmitter)
                    .count(),
                0
            );

            // Sharing the texture at a different lifetime does not make the
            // suppressed descriptor's collection ready.
            let local = registry.spawn(Transform::default());
            let mut local_emitter = emitter("unique_plume", 1.75);
            local_emitter.burst = None;
            registry.set_component(local, local_emitter).unwrap();
            let groups = group_candidates_by_collection_id(discover_sprite_collection_candidates(
                &registry,
                vec![],
                &records,
                &descriptors,
            ));
            assert_eq!(groups.len(), 2, "lifetime is part of the draw contract");
            let mut collector = ParticleRenderCollector::new();
            collector.register_sprite("first-fallback-collection");
            for (collection, _) in &groups {
                collector.register_sprite(collection);
            }

            let display = registry.spawn(Transform::default());
            assert!(materialize_net_descriptor_presentation(
                "networked_plume",
                &descriptors,
                &mut registry,
                display,
                None,
            ));
            assert!(registry.get_component::<HealthComponent>(display).is_err());
            let component = registry
                .get_component::<BillboardEmitterComponent>(display)
                .unwrap();
            let expected = derive_collection_id(
                &component.sprite,
                Some(component.lifetime),
                None,
                0.0,
                None,
                None,
            );
            let mut bridge = EmitterBridge::new();
            bridge.update(
                &mut registry,
                1.0 / 60.0,
                1.0 / 60.0,
                0.0,
                &Default::default(),
            );
            let particle = registry
                .iter_with_kind(ComponentKind::ParticleState)
                .map(|(id, _)| id)
                .next()
                .expect("display copy emits its authored burst");
            assert_eq!(
                registry
                    .get_component::<SpriteVisual>(particle)
                    .unwrap()
                    .collection,
                expected
            );
            collector.collect(&registry, None, &VisibleCells::DrawAll);
            assert_eq!(
                collector
                    .iter_collections()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>(),
                vec![expected.as_str()],
            );
        }
        capture.assert_not_logged(log::Level::Warn, "falling back to default collection");
    }

    #[test]
    fn sprite_discovery_keeps_local_and_projectile_contracts_and_only_resolved_spawner_emitters() {
        use crate::scripting::builtins::data_archetype_test_fixtures::{
            behavior_enemy_descriptor, mesh_descriptor,
        };
        use postretro_entities::Transform;

        let mut registry = EntityRegistry::new();
        let local = registry.spawn(Transform::default());
        registry
            .set_component(local, emitter("local", 0.3))
            .unwrap();
        let mut resolved = behavior_enemy_descriptor("spawned_plume");
        resolved.emitter = Some(emitter("spawned", 0.4));
        let mut unused = mesh_descriptor("unused_plume", false);
        unused.emitter = Some(emitter("unused", 0.5));
        let mut unplaced = behavior_enemy_descriptor("unplaced_plume");
        unplaced.emitter = Some(emitter("unplaced", 0.7));
        let descriptors = vec![resolved, unused, unplaced];
        for name in ["spawned_plume", "unused_plume", "missing"] {
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    SpawnerComponent {
                        archetype_name: name.to_string(),
                        count: 1,
                        spawned_tags: vec![],
                        resolved: false,
                    },
                )
                .unwrap();
        }
        // Use install's actual resolver, including its AI-only gate. The
        // non-AI descriptor is deliberately present and referenced but inert.
        super::super::resolve_spawners_for_level(
            &mut registry,
            &descriptors,
            None,
            &crate::spawner::SpawnContext::default(),
        );
        let projectile = ProjectileSpriteCollection {
            collection: "projectile".to_string(),
            lifetime: Some(0.6),
            emissive: 1.0,
            frame_duration_ms: Some(50.0),
            source: "projectile body".to_string(),
        };
        let projectile_id = SpriteCollectionCandidate::from(projectile.clone()).collection_id();
        let groups = group_candidates_by_collection_id(discover_sprite_collection_candidates(
            &registry,
            vec![projectile],
            &[],
            &descriptors,
        ));
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[1].0, projectile_id);
        assert_eq!(
            groups
                .iter()
                .map(|(_, candidates)| candidates[0].asset.as_str())
                .collect::<Vec<_>>(),
            vec!["local", "projectile", "spawned"]
        );
    }

    fn sprite_candidate(
        source: &str,
        asset: &str,
        lifetime: Option<f32>,
        emissive: f32,
        spec_intensity: Option<f32>,
        spec_exponent: Option<f32>,
        frame_duration_ms: Option<f32>,
    ) -> SpriteCollectionCandidate {
        SpriteCollectionCandidate {
            asset: asset.to_string(),
            lifetime,
            emissive,
            spec_intensity,
            spec_exponent,
            frame_duration_ms,
            source: source.to_string(),
        }
    }

    #[test]
    fn differing_shared_asset_contracts_register_in_first_seen_order() {
        // The restored rifle trail is encountered before the rocket trail. It
        // used to force the entire asset collection to be skipped; it now
        // becomes the deterministic default and retains its own group.
        let rifle = sprite_candidate(
            "enemy-rifle projectile visual trail",
            "smoke_puff/smoke_puff_00.png",
            Some(0.45),
            0.0,
            None,
            None,
            None,
        );
        let rocket = sprite_candidate(
            "rocket projectile visual trail",
            "smoke_puff/smoke_puff_00.png",
            Some(1.75),
            0.0,
            None,
            None,
            None,
        );
        let expected_rifle_id = rifle.collection_id();
        let expected_rocket_id = rocket.collection_id();

        let groups = group_candidates_by_collection_id(vec![rifle, rocket]);

        assert_eq!(
            groups.len(),
            2,
            "different contracts must never skip an asset"
        );
        assert_eq!(groups[0].0, expected_rifle_id);
        assert_eq!(groups[1].0, expected_rocket_id);
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[1].1.len(), 1);
    }

    #[test]
    fn identical_contracts_over_one_asset_collapse_to_one_collection() {
        let first = sprite_candidate(
            "billboard emitter 7",
            "sprites/shared.png",
            Some(0.2),
            0.0,
            None,
            None,
            None,
        );
        let second = sprite_candidate(
            "plasma projectile visual trail",
            "sprites/shared.png",
            Some(0.2),
            0.0,
            None,
            None,
            None,
        );

        let groups = group_candidates_by_collection_id(vec![first, second]);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 2);
    }

    #[test]
    fn raw_cadence_stays_in_the_id_but_loop_period_uses_frame_count() {
        let candidate = sprite_candidate(
            "plasma projectile visual body",
            "sprites/shared.png",
            None,
            0.0,
            Some(0.75),
            Some(12.0),
            Some(50.0),
        );
        let collection_id = candidate.collection_id();
        let registration = registration_for_candidate(&collection_id, &candidate, 3, false)
            .expect("finite positive exponents are valid");

        assert!(collection_id.contains("frame_duration_ms=bits:42480000"));
        assert!((registration.lifetime - 0.15).abs() <= f32::EPSILON);
        assert!((registration.spec_intensity - 0.75).abs() <= f32::EPSILON);
        assert!((registration.spec_exponent - 12.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn invalid_specular_exponent_is_rejected_before_registration() {
        for invalid in [0.0, -0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let candidate = sprite_candidate(
                "invalid material",
                "sprites/shared.png",
                None,
                0.0,
                None,
                Some(invalid),
                None,
            );
            let error =
                registration_for_candidate(&candidate.collection_id(), &candidate, 1, false)
                    .expect_err("invalid exponents must not reach renderer registration");
            assert!(error.contains("invalid specular exponent"));
            assert!(error.contains("invalid material"));
        }
    }

    #[test]
    fn impact_registration_and_stamp_share_one_contract() {
        let registration = weapon_impact_sprite_registration();
        let expected = derive_collection_id(
            weapon::impact_sprite_collection(),
            Some(0.18),
            None,
            0.0,
            Some(0.45),
            Some(4.0),
        );

        assert_eq!(weapon::impact_collection_id(), expected);
        assert!((registration.lifetime - 0.18).abs() <= f32::EPSILON);
        assert!(registration.emissive.abs() <= f32::EPSILON);
        assert!((registration.spec_intensity - 0.45).abs() <= f32::EPSILON);
        assert!((registration.spec_exponent - 4.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn baked_sprite_eligibility_comes_only_from_map_billboard_emitters() {
        use postretro_level_format::map_entity::MapEntityRecord;

        let entities = [
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![("sprite".to_string(), "smoke".to_string())],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![
                    ("sprite".to_string(), "ignored".to_string()),
                    ("sprite".to_string(), "sparks".to_string()),
                ],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![("sprite".to_string(), String::new())],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "data_archetype".to_string(),
                key_values: vec![("sprite".to_string(), "descriptor_only".to_string())],
                ..Default::default()
            },
        ];

        let collections = map_billboard_sprite_collections(&entities);
        assert!(collections.contains("smoke"));
        assert!(collections.contains("sparks"));
        assert!(!collections.contains("ignored"));
        assert!(!collections.contains("descriptor_only"));
    }
}
