// Level-boundary shadow state: the build's tier lookup (spot pool and the
// promoted and dynamic world-depth cache textures, shared by tier) and the
// publish-time reset of the active cache contents.
// See: context/lib/rendering_pipeline.md §4 · context/plans/in-progress/level-load-pipeline/index.md (Per-load renderer resources)

use super::super::level_resource_set::LevelShadowTier;
use super::super::renderer_types::{FullRenderer, PromotedBakedLightState};
use super::super::shadow_tier::ShadowTier;
use super::lights::LevelLightSelection;
use super::*;
use crate::render::gpu_create::GpuCreate;

/// One build's shadow textures. Shadow quality is a level-boundary setting:
/// changing the option only updates renderer boot state, and the next build
/// looks its resolution up in the tier registry, reusing a tier any live set
/// (or the active pool) still holds and building a fresh one otherwise, so a
/// resident level at another tier keeps sampling its own. The promoted cache
/// textures join when the level has a promoted source (selected-static or
/// section-45 animated candidates share the fixed-projection cache), and each
/// dynamic cache when the level holds a dynamic light of its type. Touches
/// nothing the active set or its frames read.
pub(super) fn level_shadow_tier(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
    selection: &LevelLightSelection<'_>,
    requested_spot_resolution: u32,
) -> LevelShadowTier {
    let tier = full
        .shadow_tiers
        .get_or_build(requested_spot_resolution, || {
            ShadowTier::new(device, requested_spot_resolution)
        });
    let cube_array_supported = full.cube_shadow_pool.is_some();
    let has_promoted_cache_source = !geometry.entity_shadow_lights.is_empty()
        || !selection.animated_baked_candidates.is_empty();
    let promoted = has_promoted_cache_source.then(|| tier.promoted(device, cube_array_supported));
    let dynamic = tier.dynamic(
        device,
        dynamic_depth_cache::DynamicCacheAllocation::for_lights(
            &selection.shadow_candidate_lights,
            cube_array_supported,
        ),
    );
    LevelShadowTier {
        tier,
        promoted,
        dynamic,
    }
}

/// Point the active pool and depth caches at the published set's tier, every
/// slot unoccupied and every cache layer cold, and zero the cache frame plans
/// and counters. Creates nothing: the textures were built (or shared) by the
/// set's build.
pub(in crate::render) fn publish_shadow_tier(full: &mut FullRenderer, shadow: &LevelShadowTier) {
    full.shadow_tier = shadow.tier.clone();
    full.spot_shadow_pool = SpotShadowPool::over(shadow.tier.spot.clone());
    full.promoted_depth_cache = shadow.promoted.clone().map(PromotedDepthCache::over);
    full.promoted_depth_cache_frame_plan = PromotedDepthCacheFramePlan::default();
    full.promoted_depth_cache_missing_layer_warned = false;
    full.promoted_depth_cache_promoted_count = 0;
    full.promoted_depth_cache_world_render_skips = 0;
    full.promoted_depth_cache_timing_open = false;
    full.dynamic_depth_cache = DynamicDepthCacheGpu::over(shadow.dynamic.clone());
    full.dynamic_depth_cache_frame_plan = DynamicDepthCachePlan::default();
    full.dynamic_depth_cache_diagnostics = Default::default();
    full.promoted_entity_occluders_submitted = 0;
}

/// One level's fresh promoted-light state and its static weight buffer.
pub(super) struct PromotedLevelState {
    pub(super) promoted_static_states: Vec<PromotedBakedLightState>,
    pub(super) promoted_animated_states: Vec<PromotedBakedLightState>,
    pub(super) animated_light_window_brightness: Vec<f32>,
    pub(super) promoted_static_weights: Vec<f32>,
    pub(super) promoted_static_weight_buffer: wgpu::Buffer,
}

/// Build the level's promoted-light state and static weights.
pub(super) fn level_promoted_state(
    device: &wgpu::Device,
    geometry: &LevelGeometry<'_>,
    selection: &LevelLightSelection<'_>,
) -> PromotedLevelState {
    PromotedLevelState {
        promoted_static_states: vec![
            PromotedBakedLightState::default();
            geometry.entity_shadow_lights.len()
        ],
        // Preserve raw section-45 index space even when a roster row lacks a
        // runtime MapLight candidate (for example, a bake-only entry).
        promoted_animated_states: vec![
            PromotedBakedLightState::default();
            selection.animated_baked_descriptor_indices.len()
        ],
        // A level reload must not let the prior level's window maxima keep a
        // newly-loaded animated candidate alive before its bridge update.
        animated_light_window_brightness: vec![
            0.0;
            selection.animated_baked_descriptor_indices.len()
        ],
        promoted_static_weights: vec![0.0; geometry.entity_shadow_lights.len()],
        promoted_static_weight_buffer: device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("Promoted Static Light Weights"),
            size: (geometry.entity_shadow_lights.len().max(1) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
    }
}

#[cfg(test)]
mod quality_boundary_tests {
    use super::super::super::shadow_tier::TierRegistry;
    use std::sync::Arc;

    /// The tier lookup is the shadow-quality boundary: an unchanged
    /// resolution reuses the live pool, a changed one builds a fresh one,
    /// and a resolution whose pool nothing holds any more builds again.
    #[test]
    fn spot_shadow_quality_rebuilds_only_when_level_boundary_resolution_changes() {
        let mut registry = TierRegistry::<u32>::default();
        let mut builds = 0;
        let mut tier = |registry: &mut TierRegistry<u32>, resolution: u32| {
            registry.get_or_build(resolution, || {
                builds += 1;
                resolution
            })
        };
        let first = tier(&mut registry, 1024);
        let same = tier(&mut registry, 1024);
        assert!(Arc::ptr_eq(&first, &same), "same tier reuses the pool");
        let lower = tier(&mut registry, 512);
        assert!(!Arc::ptr_eq(&first, &lower), "a new tier builds its own");
        let other = tier(&mut registry, 768);
        assert_eq!((*lower, *other), (512, 768));
        let lower_again = tier(&mut registry, 512);
        assert!(Arc::ptr_eq(&lower, &lower_again), "a live tier is reused");
        drop((first, same));
        assert!(registry.live(1024).is_none(), "the registry holds no tier");
        let rebuilt = tier(&mut registry, 1024);
        assert_eq!(*rebuilt, 1024);
        drop(tier);
        assert_eq!(builds, 4, "1024, 512, 768, then 1024 once nothing held it");
    }
}
