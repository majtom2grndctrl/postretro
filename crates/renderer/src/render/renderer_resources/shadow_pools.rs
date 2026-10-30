// Level-boundary shadow state: the spot pool's tier rebuild and the promoted
// and dynamic depth caches every install resets.
// See: context/lib/rendering_pipeline.md §4

use super::super::renderer_types::{FullRenderer, PromotedBakedLightState};
use super::lights::LevelLightSelection;
use super::*;

fn spot_shadow_pool_needs_rebuild(current: u32, requested: u32) -> bool {
    current != requested
}

pub(super) fn rebuild_spot_shadow_pool_for_tier(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    requested_spot_resolution: u32,
) {
    // Shadow quality is a level-boundary setting: changing the option only
    // updates renderer boot state, and the next geometry install rebuilds
    // the live spot pool and all resolution-coupled caches together.
    if spot_shadow_pool_needs_rebuild(full.spot_shadow_pool.resolution, requested_spot_resolution) {
        let cube_array_supported = full.cube_shadow_pool.is_some();
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(device, cube_array_supported);
        let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        full.spot_shadow_pool = SpotShadowPool::new(
            device,
            &spot_shadow_bgl,
            &full.sdf_shadow_pass.shadow_view,
            &full.depth_view,
            cube_sampling_view,
            requested_spot_resolution,
        );
        // Reallocated below using the new resolution if this level has a
        // promoted-cache source. Dropping it first prevents a same-shape
        // reset from retaining the previous texture dimensions.
        full.promoted_depth_cache = None;
    }
}

/// Reset the promoted-light state and the promoted/dynamic depth caches for the
/// level's shadow candidates, and reallocate the promoted static weights.
pub(super) fn reset_level_shadow_caches(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
    selection: &LevelLightSelection<'_>,
) {
    full.promoted_static_states =
        vec![PromotedBakedLightState::default(); geometry.entity_shadow_lights.len()];
    // Preserve raw section-45 index space even when a roster row lacks a
    // runtime MapLight candidate (for example, a bake-only entry).
    full.promoted_animated_states =
        vec![PromotedBakedLightState::default(); selection.animated_baked_descriptor_indices.len()];
    // A level reload must not let the prior level's window maxima keep a
    // newly-loaded animated candidate alive before its bridge update.
    full.animated_light_window_brightness =
        vec![0.0; selection.animated_baked_descriptor_indices.len()];
    full.promoted_baked_records.clear();
    full.promoted_baked_cache_layers.clear();
    full.promoted_static_weights = vec![0.0; geometry.entity_shadow_lights.len()];
    full.promoted_static_weight_scratch.clear();
    full.promoted_baked_last_update_time = None;
    full.promotions_start_whole = false;
    // Match the init-time policy: selected-static and section-45 animated
    // candidates share the fixed-projection cache. A level with neither
    // source frees it; either source allocates/reuses it and clears every
    // cache layer on reload.
    let has_promoted_cache_source = !geometry.entity_shadow_lights.is_empty()
        || !selection.animated_baked_candidates.is_empty();
    if !has_promoted_cache_source {
        full.promoted_depth_cache = None;
    } else if let Some(cache) = &mut full.promoted_depth_cache {
        cache.reset_level();
    } else {
        full.promoted_depth_cache = Some(PromotedDepthCache::new(
            device,
            full.cube_shadow_pool.is_some(),
            full.spot_shadow_pool.resolution,
        ));
    }
    full.promoted_depth_cache_frame_plan = PromotedDepthCacheFramePlan::default();
    full.promoted_depth_cache_missing_layer_warned = false;
    full.promoted_depth_cache_promoted_count = 0;
    full.promoted_depth_cache_world_render_skips = 0;
    full.promoted_depth_cache_timing_open = false;
    full.dynamic_depth_cache.reset_level(
        device,
        dynamic_depth_cache::DynamicCacheAllocation::for_lights(
            &selection.shadow_candidate_lights,
            full.cube_shadow_pool.is_some(),
        ),
        full.spot_shadow_pool.resolution,
    );
    full.dynamic_depth_cache_frame_plan = DynamicDepthCachePlan::default();
    full.dynamic_depth_cache_diagnostics = Default::default();
    full.promoted_entity_occluders_submitted = 0;
    full.promoted_static_weight_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Promoted Static Light Weights"),
        size: (geometry.entity_shadow_lights.len().max(1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
}

#[cfg(test)]
mod quality_boundary_tests {
    use super::spot_shadow_pool_needs_rebuild;

    #[test]
    fn spot_shadow_quality_rebuilds_only_when_level_boundary_resolution_changes() {
        assert!(!spot_shadow_pool_needs_rebuild(1024, 1024));
        assert!(spot_shadow_pool_needs_rebuild(1024, 512));
        assert!(spot_shadow_pool_needs_rebuild(512, 768));
    }
}
