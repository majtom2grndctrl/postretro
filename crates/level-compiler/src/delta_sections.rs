//! Carries all baked delta sections through the compiler's post-bake seam.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;

/// Default aggregate raw payload cap for ids 27, 41, and 45 on desktop maps.
pub(crate) const DEFAULT_MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Compiler-only configuration for the post-bake delta-section policy.
///
/// The cap is carried with the baked sections so the policy can enforce it
/// before packing without leaking an implementation choice into the PRL wire
/// format or runtime representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeltaSectionConfig {
    pub max_payload_bytes: u64,
}

impl Default for DeltaSectionConfig {
    fn default() -> Self {
        Self {
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
        }
    }
}

/// Delta sections after all three delta bakes have completed.
///
/// This is intentionally an owned handoff. The next compiler-only policy can
/// inspect or replace these sections before packing while the pipeline and
/// runtime continue to consume the established dense section contracts.
pub(crate) struct PostBakeDeltaSections {
    pub config: DeltaSectionConfig,
    pub indirect: Option<DeltaShVolumesSection>,
    pub entity_shadow_lights: Option<EntityShadowLightsSection>,
    pub direct: Option<DirectShDeltaVolumesSection>,
    pub animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
}

impl PostBakeDeltaSections {
    pub(crate) fn new(
        config: DeltaSectionConfig,
        indirect: Option<DeltaShVolumesSection>,
        entity_shadow_lights: Option<EntityShadowLightsSection>,
        direct: Option<DirectShDeltaVolumesSection>,
        animated_direct: Option<AnimatedDirectShDeltaVolumesSection>,
    ) -> Self {
        Self {
            config,
            indirect,
            entity_shadow_lights,
            direct,
            animated_direct,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_indirect() -> DeltaShVolumesSection {
        DeltaShVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_direct() -> DirectShDeltaVolumesSection {
        DirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    fn empty_animated_direct() -> AnimatedDirectShDeltaVolumesSection {
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0, 0, 0],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![],
            affinity_offsets: vec![0],
            affinity_lights: vec![],
            delta_subblocks: vec![],
        }
    }

    #[test]
    fn post_bake_handoff_preserves_sections_and_owns_the_cap() {
        let config = DeltaSectionConfig {
            max_payload_bytes: 123,
        };
        let indirect = empty_indirect();
        let entity_shadow_lights = EntityShadowLightsSection {
            light_indices: vec![2, 7],
        };
        let direct = empty_direct();
        let animated_direct = empty_animated_direct();

        let handoff = PostBakeDeltaSections::new(
            config,
            Some(indirect.clone()),
            Some(entity_shadow_lights.clone()),
            Some(direct.clone()),
            Some(animated_direct.clone()),
        );

        assert_eq!(handoff.config, config);
        assert_eq!(handoff.indirect, Some(indirect));
        assert_eq!(handoff.entity_shadow_lights, Some(entity_shadow_lights));
        assert_eq!(handoff.direct, Some(direct));
        assert_eq!(handoff.animated_direct, Some(animated_direct));
    }

    #[test]
    fn delta_section_config_defaults_to_desktop_cap() {
        assert_eq!(
            DeltaSectionConfig::default().max_payload_bytes,
            256 * 1024 * 1024
        );
    }
}
