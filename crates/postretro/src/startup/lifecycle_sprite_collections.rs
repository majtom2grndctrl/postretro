//! Sprite-collection registration during level installation.

use std::collections::HashSet;
use std::path::Path;

use crate::render;
use crate::scripting::builtins::data_archetype::ProjectileSpriteCollection;
use crate::scripting_systems::particle_render::ParticleRenderCollector;
use crate::weapon;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};

pub(super) const DEFAULT_SPRITE_SPECULAR_INTENSITY: f32 = 2.0;
pub(super) const DEFAULT_SPRITE_SPECULAR_EXPONENT: f32 = 4.0;

pub(super) fn weapon_impact_sprite_registration() -> render::SpriteCollectionRegistration {
    render::SpriteCollectionRegistration {
        baked_sidecar_eligible: false,
        spec_intensity: 0.45,
        spec_exponent: DEFAULT_SPRITE_SPECULAR_EXPONENT,
        lifetime: weapon::impact_lifetime(),
        emissive: 0.0,
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpriteCollectionCandidate {
    pub(super) collection: String,
    pub(super) lifetime: Option<f32>,
    pub(super) emissive: f32,
    pub(super) spec_intensity: Option<f32>,
    pub(super) spec_exponent: Option<f32>,
    pub(super) frame_duration_ms: Option<f32>,
    pub(super) source: String,
}

impl From<ProjectileSpriteCollection> for SpriteCollectionCandidate {
    fn from(value: ProjectileSpriteCollection) -> Self {
        Self {
            collection: value.collection,
            lifetime: value.lifetime,
            emissive: value.emissive,
            spec_intensity: None,
            spec_exponent: None,
            frame_duration_ms: value.frame_duration_ms,
            source: value.source,
        }
    }
}

pub(super) fn resolve_sprite_collection_draw_contract(
    collection: &str,
    candidates: &[SpriteCollectionCandidate],
    frame_count: usize,
) -> Result<(f32, f32, f32, f32), String> {
    let mut lifetime: Option<(f32, &str)> = None;
    let mut emissive: Option<(f32, &str)> = None;
    let mut spec_intensity: Option<(f32, &str)> = None;
    let mut spec_exponent: Option<(f32, &str)> = None;

    for candidate in candidates {
        let required_lifetime = candidate
            .frame_duration_ms
            .map_or(candidate.lifetime, |ms| {
                Some(ms / 1_000.0 * frame_count.max(1) as f32)
            });
        if let Some(required) = required_lifetime {
            if let Some((chosen, chosen_source)) = lifetime
                && chosen.to_bits() != required.to_bits()
            {
                return Err(format!(
                    "collection `{collection}` has conflicting loop periods from `{chosen_source}` ({chosen}s) and `{}` ({required}s)",
                    candidate.source,
                ));
            }
            lifetime.get_or_insert((required, &candidate.source));
        }

        if let Some((chosen, chosen_source)) = emissive
            && chosen.to_bits() != candidate.emissive.to_bits()
        {
            return Err(format!(
                "collection `{collection}` has conflicting emissive strengths from `{chosen_source}` ({chosen}) and `{}` ({})",
                candidate.source, candidate.emissive,
            ));
        }
        emissive.get_or_insert((candidate.emissive, &candidate.source));

        if let Some(required) = candidate.spec_intensity {
            if let Some((chosen, chosen_source)) = spec_intensity
                && chosen.to_bits() != required.to_bits()
            {
                return Err(format!(
                    "collection `{collection}` has conflicting specular intensities from `{chosen_source}` ({chosen}) and `{}` ({required})",
                    candidate.source,
                ));
            }
            spec_intensity.get_or_insert((required, &candidate.source));
        }

        if let Some(required) = candidate.spec_exponent {
            if !render::sprite_specular_exponent_is_valid(required) {
                return Err(format!(
                    "collection `{collection}` has invalid specular exponent from `{}` ({required}); expected a finite value greater than zero",
                    candidate.source,
                ));
            }
            if let Some((chosen, chosen_source)) = spec_exponent
                && chosen.to_bits() != required.to_bits()
            {
                return Err(format!(
                    "collection `{collection}` has conflicting specular exponents from `{chosen_source}` ({chosen}) and `{}` ({required})",
                    candidate.source,
                ));
            }
            spec_exponent.get_or_insert((required, &candidate.source));
        }
    }

    Ok((
        lifetime.map_or(1.0, |(value, _)| value),
        emissive.map_or(0.0, |(value, _)| value),
        spec_intensity.map_or(DEFAULT_SPRITE_SPECULAR_INTENSITY, |(value, _)| value),
        spec_exponent.map_or(DEFAULT_SPRITE_SPECULAR_EXPONENT, |(value, _)| value),
    ))
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

/// Register every level-owned sprite collection in discovery order.
pub(super) fn install_sprite_collections(
    renderer: &mut render::Renderer,
    particle_render: &mut ParticleRenderCollector,
    texture_root: &Path,
    prm_cache_root: &Path,
    registry: &EntityRegistry,
    projectile_sprites: Vec<ProjectileSpriteCollection>,
    map_billboard_collections: &HashSet<String>,
) {
    let mut collections: Vec<(String, Vec<SpriteCollectionCandidate>)> = Vec::new();
    let mut collection_indices = std::collections::HashMap::<String, usize>::new();
    {
        let mut add_candidate = |candidate: SpriteCollectionCandidate| {
            if candidate.collection.is_empty() {
                return;
            }
            let index = match collection_indices.get(&candidate.collection).copied() {
                Some(index) => index,
                None => {
                    let index = collections.len();
                    collection_indices.insert(candidate.collection.clone(), index);
                    collections.push((candidate.collection.clone(), Vec::new()));
                    index
                }
            };
            collections[index].1.push(candidate);
        };
        for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
            let ComponentValue::BillboardEmitter(c) = value else {
                continue;
            };
            add_candidate(SpriteCollectionCandidate {
                collection: c.sprite.clone(),
                lifetime: Some(c.lifetime),
                emissive: 0.0,
                spec_intensity: None,
                spec_exponent: None,
                frame_duration_ms: None,
                source: format!("billboard emitter {id}"),
            });
        }
        for sprite in projectile_sprites {
            add_candidate(sprite.into());
        }
    }

    for (collection, candidates) in collections {
        // Keep the draw-contract frame count sourced from the runtime sprite
        // loader. A direct `.png` remains one frame here; baked collection
        // sidecars never become a second shader-facing count.
        let frame_count =
            postretro_render_cpu::smoke::load_sprite_frames(texture_root, &collection)
                .map_or(1, |frames| frames.len());
        let (lifetime, emissive, spec_intensity, spec_exponent) =
            match resolve_sprite_collection_draw_contract(&collection, &candidates, frame_count) {
                Ok(contract) => contract,
                Err(reason) => {
                    log::warn!(
                        "[Loader] {reason}; skipping the collection so no accepted descriptor is silently overridden"
                    );
                    continue;
                }
            };
        renderer.register_smoke_collection(
            &collection,
            texture_root,
            prm_cache_root,
            render::SpriteCollectionRegistration {
                baked_sidecar_eligible: map_billboard_collections.contains(&collection),
                spec_intensity,
                spec_exponent,
                lifetime,
                emissive,
            },
        );
        particle_render.register_sprite(&collection);
    }

    let collection = weapon::impact_sprite_collection();
    renderer.register_smoke_collection(
        collection,
        texture_root,
        prm_cache_root,
        weapon_impact_sprite_registration(),
    );
    particle_render.register_sprite(collection);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite_candidate(
        source: &str,
        lifetime: Option<f32>,
        emissive: f32,
        spec_intensity: Option<f32>,
        spec_exponent: Option<f32>,
        frame_duration_ms: Option<f32>,
    ) -> SpriteCollectionCandidate {
        SpriteCollectionCandidate {
            collection: "sprites/shared".to_string(),
            lifetime,
            emissive,
            spec_intensity,
            spec_exponent,
            frame_duration_ms,
            source: source.to_string(),
        }
    }

    #[test]
    fn shared_projectile_collection_rejects_conflicting_draw_contracts() {
        let candidates = [
            sprite_candidate(
                "plasma.projectile.visual.body",
                None,
                2.0,
                None,
                None,
                Some(50.0),
            ),
            sprite_candidate(
                "rocket.projectile.visual.body",
                None,
                1.0,
                None,
                None,
                Some(80.0),
            ),
        ];

        let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
            .expect_err("conflicting projectile consumers must reject the collection");

        assert!(error.contains("conflicting loop periods"));
        assert!(error.contains("plasma.projectile.visual.body"));
        assert!(error.contains("rocket.projectile.visual.body"));
    }

    #[test]
    fn projectile_and_emitter_collection_rejects_conflicting_emissive_contracts() {
        let candidates = [
            sprite_candidate("billboard emitter 7", Some(0.2), 0.0, None, None, None),
            sprite_candidate(
                "plasma.projectile.visual.trail",
                Some(0.2),
                3.0,
                None,
                None,
                None,
            ),
        ];

        let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
            .expect_err("emitter and projectile conflicts must not depend on collection order");

        assert!(error.contains("conflicting emissive strengths"));
        assert!(error.contains("billboard emitter 7"));
        assert!(error.contains("plasma.projectile.visual.trail"));
    }

    #[test]
    fn compatible_shared_sprite_consumers_resolve_one_draw_contract() {
        let candidates = [
            sprite_candidate("billboard emitter 7", Some(0.2), 0.0, None, None, None),
            sprite_candidate(
                "plasma.projectile.visual.trail",
                Some(0.2),
                0.0,
                None,
                None,
                None,
            ),
            sprite_candidate("plasma.projectile.visual.body", None, 0.0, None, None, None),
        ];

        let (lifetime, emissive, spec_intensity, spec_exponent) =
            resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect("identical consumers and a cadence-less body are compatible");

        assert!((lifetime - 0.2).abs() <= f32::EPSILON);
        assert!(emissive.abs() <= f32::EPSILON);
        assert!((spec_intensity - DEFAULT_SPRITE_SPECULAR_INTENSITY).abs() <= f32::EPSILON);
        assert!((spec_exponent - DEFAULT_SPRITE_SPECULAR_EXPONENT).abs() <= f32::EPSILON);
    }

    #[test]
    fn sprite_collection_draw_contract_resolves_specular_overrides_and_zero_candidate_defaults() {
        let (_, _, default_intensity, default_exponent) =
            resolve_sprite_collection_draw_contract("sprites/shared", &[], 4)
                .expect("no consumers retain the default draw contract");
        assert!((default_intensity - DEFAULT_SPRITE_SPECULAR_INTENSITY).abs() <= f32::EPSILON);
        assert!((default_exponent - DEFAULT_SPRITE_SPECULAR_EXPONENT).abs() <= f32::EPSILON);

        let candidates = [
            sprite_candidate(
                "plasma.projectile.visual.trail",
                Some(0.2),
                0.0,
                Some(0.75),
                Some(12.0),
                None,
            ),
            sprite_candidate(
                "plasma.projectile.visual.body",
                None,
                0.0,
                Some(0.75),
                Some(12.0),
                None,
            ),
        ];

        let (_, _, spec_intensity, spec_exponent) =
            resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect("matching authored specular values resolve one draw contract");
        assert!((spec_intensity - 0.75).abs() <= f32::EPSILON);
        assert!((spec_exponent - 12.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn sprite_collection_draw_contract_rejects_invalid_specular_exponents() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let candidates = [sprite_candidate(
                "invalid material",
                None,
                0.0,
                None,
                Some(invalid),
                None,
            )];

            let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect_err("invalid exponents must reject the collection before registration");

            assert!(error.contains("invalid specular exponent"));
            assert!(error.contains("invalid material"));
            assert!(error.contains("finite value greater than zero"));
        }
    }

    #[test]
    fn weapon_impact_sprite_registration_retains_authored_specular_defaults() {
        // Regression: weapon impacts bypass shared candidate resolution and need their own guard.
        let registration = weapon_impact_sprite_registration();

        assert!((registration.spec_intensity - 0.45).abs() <= f32::EPSILON);
        assert!(
            (registration.spec_exponent - DEFAULT_SPRITE_SPECULAR_EXPONENT).abs() <= f32::EPSILON
        );
    }

    #[test]
    fn sprite_collection_draw_contract_rejects_conflicting_specular_intensity_in_both_orders() {
        for candidates in [
            [
                sprite_candidate("first", None, 0.0, Some(0.3), None, None),
                sprite_candidate("second", None, 0.0, Some(0.6), None, None),
            ],
            [
                sprite_candidate("second", None, 0.0, Some(0.6), None, None),
                sprite_candidate("first", None, 0.0, Some(0.3), None, None),
            ],
        ] {
            let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect_err("conflicting intensity overrides must reject the collection");

            assert!(error.contains("conflicting specular intensities"));
            assert!(error.contains("first"));
            assert!(error.contains("second"));
        }
    }

    #[test]
    fn sprite_collection_draw_contract_rejects_conflicting_specular_exponent_in_both_orders() {
        for candidates in [
            [
                sprite_candidate("first", None, 0.0, None, Some(4.0), None),
                sprite_candidate("second", None, 0.0, None, Some(8.0), None),
            ],
            [
                sprite_candidate("second", None, 0.0, None, Some(8.0), None),
                sprite_candidate("first", None, 0.0, None, Some(4.0), None),
            ],
        ] {
            let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect_err("conflicting exponent overrides must reject the collection");

            assert!(error.contains("conflicting specular exponents"));
            assert!(error.contains("first"));
            assert!(error.contains("second"));
        }
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

    #[test]
    fn explicit_sprite_cadence_uses_the_normalized_frame_count() {
        // Regression: three decoded frames produced a three-frame loop period
        // even when only two shared the renderer's array extent.
        let frames = vec![
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 4],
                width: 1,
                height: 1,
            },
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
        ];
        let frames = postretro_render_cpu::smoke::normalize_sprite_frames(frames)
            .expect("two frames share the collection extent");
        let candidates = [sprite_candidate(
            "plasma.projectile.visual.body",
            None,
            0.0,
            None,
            None,
            Some(50.0),
        )];

        let (lifetime, _, _, _) =
            resolve_sprite_collection_draw_contract("sprites/shared", &candidates, frames.len())
                .expect("one consumer resolves");

        assert!((lifetime - 0.1).abs() <= f32::EPSILON);
        assert!((lifetime / frames.len() as f32 - 0.05).abs() <= f32::EPSILON);
    }
}
