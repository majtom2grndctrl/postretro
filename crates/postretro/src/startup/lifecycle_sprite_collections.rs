//! Sprite-collection registration during level installation.

use std::collections::HashSet;
use std::path::Path;

use crate::render;
use crate::scripting::builtins::data_archetype::ProjectileSpriteCollection;
use crate::scripting_systems::particle_render::ParticleRenderCollector;
use crate::sprite_collection::{
    DEFAULT_SPRITE_SPECULAR_EXPONENT, DEFAULT_SPRITE_SPECULAR_INTENSITY, derive_collection_id,
};
use crate::weapon;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};

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
    let mut candidates = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
        let ComponentValue::BillboardEmitter(component) = value else {
            continue;
        };
        candidates.push(SpriteCollectionCandidate {
            asset: component.sprite.clone(),
            lifetime: Some(component.lifetime),
            emissive: 0.0,
            spec_intensity: None,
            spec_exponent: None,
            frame_duration_ms: None,
            source: format!("billboard emitter {id}"),
        });
    }
    candidates.extend(projectile_sprites.into_iter().map(Into::into));

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
