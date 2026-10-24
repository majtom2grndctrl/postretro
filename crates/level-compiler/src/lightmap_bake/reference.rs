// Frozen monolithic lightmap oracle used by byte-identity tests.
// See: context/lib/build_pipeline.md §Build Cache

use std::cell::Cell;
use std::sync::Mutex;

use bvh::bvh::Bvh;
use glam::Vec3;
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{
    CHART_PADDING_TEXELS, ChartPlacement, chart_texel_seed, chart_texel_world_position,
};
use crate::geometry::GeometryResult;
use crate::map_data::MapLight;
use crate::texel_footprint::{
    ClipBand, ClipReach, TexelFrame, TexelSampler, TexelSite, lead_in_clear,
};

use super::{
    Chart, CompositedAtlas, light_texel_contribution, scatter_chart_into_atlas,
    segment_clear_remembering,
};

/// Bake the full static-light atlas and dilate — the independent monolithic
/// side of the byte-identity comparison seam.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn bake_monolithic_atlas(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    charts: &[Chart],
    placements: &[ChartPlacement],
    atlas_w: u32,
    atlas_h: u32,
    layer_count: u32,
    area_sample_count: u32,
    penumbra_floor: bool,
) -> CompositedAtlas {
    bake_monolithic_atlas_controlled(
        bvh,
        primitives,
        geometry,
        static_lights,
        charts,
        placements,
        atlas_w,
        atlas_h,
        layer_count,
        area_sample_count,
        penumbra_floor,
        &BakeControl::unrestricted(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_monolithic_atlas_controlled(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    charts: &[Chart],
    placements: &[ChartPlacement],
    atlas_w: u32,
    atlas_h: u32,
    layer_count: u32,
    area_sample_count: u32,
    penumbra_floor: bool,
    control: &BakeControl,
) -> CompositedAtlas {
    let atlas = Mutex::new(CompositedAtlas::zeroed(atlas_w, atlas_h, layer_count));

    placements
        .par_iter()
        .enumerate()
        .for_each(|(face_idx, placement)| {
            let _permit = control.governor().enter();
            let chart = &charts[face_idx];
            let chart_atlas = bake_face_chart(
                bvh,
                primitives,
                geometry,
                static_lights,
                chart,
                area_sample_count,
                penumbra_floor,
            );

            if chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0 {
                let mut atlas = atlas
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                scatter_chart_into_atlas(&chart_atlas, placement, &mut atlas);
            }
            control.advance(1);
        });

    let mut atlas = atlas
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    atlas.dilate();
    atlas
}

pub(super) fn bake_face_chart(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    static_lights: &[&MapLight],
    chart: &Chart,
    area_sample_count: u32,
    penumbra_floor: bool,
) -> CompositedAtlas {
    let mut chart_atlas = CompositedAtlas::zeroed(chart.width_texels, chart.height_texels, 1);
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return chart_atlas;
    }
    let padding = CHART_PADDING_TEXELS as i32;
    let (interior_w, interior_h) = crate::chart_raster::chart_interior_dims(chart);
    // One occluder cache per light, walked in texel order: each light sees the
    // ray sequence its `(light, chart)` layer bake traces, so the cache answers
    // match and the composite stays bit-for-bit. The footprint's wall-clip
    // segments keep their own per-light cache for the same reason.
    let last_occluders: Vec<Cell<Option<usize>>> =
        static_lights.iter().map(|_| Cell::new(None)).collect();
    let clip_occluders: Vec<Cell<Option<usize>>> =
        static_lights.iter().map(|_| Cell::new(None)).collect();
    let samplers: Vec<TexelSampler> = static_lights
        .iter()
        .map(|light| TexelSampler::new(light, area_sample_count, penumbra_floor))
        .collect();
    let frame = TexelFrame::for_chart(chart);
    let band = ClipBand::for_chart(chart);
    // Clip reach is light-independent; every light reads the same answers.
    let clip_reach = samplers
        .iter()
        .any(|sampler| matches!(sampler, TexelSampler::Footprint(_)))
        .then(|| ClipReach::rasterized(chart, bvh, primitives, geometry));

    for ty in 0..interior_h {
        for tx in 0..interior_w {
            let local_x = padding + tx;
            let local_y = padding + ty;
            let idx = (local_y as u32 * chart.width_texels + local_x as u32) as usize;
            let surface_normal = chart.normal;
            let site = TexelSite {
                world_p: chart_texel_world_position(chart, tx, ty),
                normal: surface_normal,
                seed: chart_texel_seed(chart, tx, ty),
                frame,
                may_clip: clip_reach
                    .as_ref()
                    .is_none_or(|reach| reach.may_clip(tx, ty)),
            };

            let mut irr = Vec3::ZERO;
            let mut weighted_dir = Vec3::ZERO;
            for (((light, sampler), last_occluder), clip_occluder) in static_lights
                .iter()
                .zip(&samplers)
                .zip(&last_occluders)
                .zip(&clip_occluders)
            {
                let (irr_contrib, dir_contrib) = light_texel_contribution(
                    light,
                    &site,
                    sampler,
                    |from, to| {
                        segment_clear_remembering(
                            bvh,
                            primitives,
                            geometry,
                            from,
                            to,
                            last_occluder,
                        )
                    },
                    |from, to| {
                        segment_clear_remembering(
                            bvh,
                            primitives,
                            geometry,
                            from,
                            to,
                            clip_occluder,
                        )
                    },
                    |from, to| lead_in_clear(bvh, primitives, geometry, &band, from, to),
                );
                irr += irr_contrib;
                weighted_dir += dir_contrib;
            }
            chart_atlas.irradiance[idx * 4] = irr.x;
            chart_atlas.irradiance[idx * 4 + 1] = irr.y;
            chart_atlas.irradiance[idx * 4 + 2] = irr.z;
            chart_atlas.irradiance[idx * 4 + 3] = 1.0;
            chart_atlas.direction[idx] = if weighted_dir.length_squared() > 1.0e-8 {
                weighted_dir.normalize()
            } else {
                surface_normal
            };
            chart_atlas.coverage[idx] = true;
        }
    }

    chart_atlas
}

#[cfg(test)]
mod tests {
    #[test]
    fn frozen_oracle_has_no_fused_walk_dependency() {
        let source = include_str!("reference.rs");
        let fused_walk_name = concat!("for_each_light_layer", "_chart_texel");
        assert!(!source.contains(fused_walk_name));
    }
}
