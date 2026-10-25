// Per-chart light-layer walk: the clipped `(light, chart)` bake with its tile
// shaft cull, and the shared chart texel iterator.
// See: context/lib/build_pipeline.md (Lightmap id 22)

use std::cell::Cell;

use glam::Vec3;

#[cfg(test)]
use super::CoveredChartTexel;
use super::chart_cull::ExactSkips;
use super::shaft_cull::{SHAFT_TILE_TEXELS, ShaftCull, ShaftScene};
#[cfg(test)]
use super::skip_stats::{ChartTally, TileTally};
use super::{LayerTexel, SharedAtlas, sparse_layer_texel};
use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{chart_interior_dims, chart_texel_seed, chart_texel_world_position};
use crate::geometry::GeometryResult;
#[cfg(test)]
use crate::lightmap_bake::light_texel_is_covered;
use crate::lightmap_bake::{
    light_texel_contribution_and_visibility_with, segment_clear_remembering,
};
use crate::map_data::MapLight;
use crate::texel_footprint::{
    ClipBand, ClipReach, ProbeEarlyOut, TexelFrame, TexelSampler, TexelSite, lead_in_clear,
};

/// Bake one `(light, chart)` contribution in placement order.
///
/// This is the outermost governed unit for every light-layer bake path. Keeping
/// admission here lets callers choose a single Rayon level without nesting a
/// governor entry around the established per-chart work.
///
/// `embedded_emitter` is the light's flag from the stage's
/// [`EmbeddedEmitters`](crate::texel_footprint::EmbeddedEmitters), tested
/// once per light, never per chart. `shaft_scene` is the bake's
/// [`ShaftScene`], built once over its geometry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer_chart_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    penumbra_floor: bool,
    embedded_emitter: bool,
    exact_skips: ExactSkips,
    shaft_scene: &ShaftScene,
    control: &BakeControl,
) -> Vec<LayerTexel> {
    bake_light_layer_chart_clipped(
        light,
        atlas,
        face_idx,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        penumbra_floor,
        ProbeEarlyOut::for_light(embedded_emitter),
        true,
        exact_skips,
        shaft_scene,
        control,
    )
}

/// [`bake_light_layer_chart_controlled`] with the clip pre-pass optional:
/// without it every texel traces its clip segments, the reference the
/// pre-pass must reproduce bit for bit.
///
/// With the shaft cull on, the chart splits into [`SHAFT_TILE_TEXELS`]-square
/// tiles from its first interior texel, and each tile is tested once, on its
/// first covered texel ([`ShaftCull::tile_clear`]). A clear tile's covered
/// texels store visibility exactly 1.0, the ratio an all-clear sample set
/// gives, and trace nothing: no shadow, clip or lead-in segment. The occluder
/// caches change only on a hit, so they leave a clear tile as the unculled
/// walk would. The direct term and the records are unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_light_layer_chart_clipped(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    penumbra_floor: bool,
    early_out: ProbeEarlyOut,
    skip_unreachable_clips: bool,
    exact_skips: ExactSkips,
    shaft_scene: &ShaftScene,
    control: &BakeControl,
) -> Vec<LayerTexel> {
    // One permit covers the clip pre-pass and the walk, so pause and the
    // concurrency cap apply to both.
    let _permit = control.governor().enter();
    let chart = &atlas.charts[face_idx];
    let capacity = if chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0 {
        let (width, height) = chart_interior_dims(chart);
        (width * height) as usize
    } else {
        0
    };
    let mut texels = Vec::with_capacity(capacity);
    let sampler = TexelSampler::with_early_out(light, area_sample_count, penumbra_floor, early_out);
    let frame = TexelFrame::for_chart(chart);
    let band = ClipBand::for_chart(chart);
    // Light-independent, but partitions bake per light; it costs little
    // beside the light's rays.
    let clip_reach = (skip_unreachable_clips && sampler.is_footprint())
        .then(|| ClipReach::rasterized(chart, bvh, primitives, geometry));
    let shaft = if exact_skips.shaft_cull {
        ShaftCull::new(
            light,
            chart,
            &sampler,
            &frame,
            shaft_scene,
            bvh,
            primitives,
            geometry,
        )
    } else {
        None
    };
    let (width, height) = chart_interior_dims(chart);
    let tiles_wide = (width + SHAFT_TILE_TEXELS - 1) / SHAFT_TILE_TEXELS;
    let tile_count = (tiles_wide * ((height + SHAFT_TILE_TEXELS - 1) / SHAFT_TILE_TEXELS)) as usize;
    // Each tile's verdict once tested. One thread walks the chart, so the
    // verdicts sit beside the occluder caches with no synchronisation.
    let mut verdicts: Vec<Option<bool>> = vec![None; if shaft.is_some() { tile_count } else { 0 }];
    #[cfg(test)]
    let mut tallies = exact_skips
        .stats
        .map(|_| vec![TileTally::default(); tile_count]);
    // One chart's texels are walked in order on one thread, so the occluder
    // caches stay local to this `(light, chart)` unit. Wall-clip segments keep
    // their own: they run between the light's rays and would otherwise evict
    // the triangle that shadows the next texel.
    let last_occluder = Cell::new(None);
    let clip_occluder = Cell::new(None);
    for_each_light_layer_chart_texel(atlas, face_idx, |sample| {
        let site = TexelSite {
            world_p: sample.world_p,
            normal: sample.surface_normal,
            seed: sample.seed,
            frame,
            may_clip: clip_reach
                .as_ref()
                .is_none_or(|reach| reach.may_clip(sample.tx, sample.ty)),
        };
        let (tile_x, tile_y) = (sample.tx / SHAFT_TILE_TEXELS, sample.ty / SHAFT_TILE_TEXELS);
        let tile = (tile_y * tiles_wide + tile_x) as usize;
        #[cfg(test)]
        let traced = TracedSegments::default();
        #[cfg(test)]
        let tested = Cell::new(false);
        let trace = |from, to| {
            #[cfg(test)]
            traced.shadow.set(traced.shadow.get() + 1);
            segment_clear_remembering(bvh, primitives, geometry, from, to, &last_occluder)
        };
        let reach = |from, to| {
            #[cfg(test)]
            traced.clip.set(traced.clip.get() + 1);
            segment_clear_remembering(bvh, primitives, geometry, from, to, &clip_occluder)
        };
        let lead_in = |from, to| {
            #[cfg(test)]
            traced.lead_in.set(traced.lead_in.get() + 1);
            lead_in_clear(bvh, primitives, geometry, &band, from, to)
        };
        let (_, _, raw_visibility) =
            light_texel_contribution_and_visibility_with(light, site.world_p, site.normal, || {
                if let Some(shaft) = &shaft {
                    let clear = *verdicts[tile].get_or_insert_with(|| {
                        #[cfg(test)]
                        tested.set(true);
                        shaft.tile_clear(tile_x, tile_y)
                    });
                    if clear {
                        #[cfg(test)]
                        if exact_skips.verify {
                            verify_clear_texel(
                                light,
                                &site,
                                &sampler,
                                (bvh, primitives, geometry),
                                &band,
                                [&last_occluder, &clip_occluder],
                            );
                            if let Some(stats) = exact_skips.stats {
                                stats.add_verified_texel();
                            }
                        }
                        return 1.0;
                    }
                }
                sampler.visibility(light, &site, trace, reach, lead_in)
            });
        #[cfg(test)]
        if let Some(tallies) = tallies.as_mut() {
            let tally = &mut tallies[tile];
            if raw_visibility.is_some() {
                tally.covered += 1;
                tally.first_covered.get_or_insert((sample.tx, sample.ty));
            }
            tally.tests += u32::from(tested.get());
            tally.clear = verdicts.get(tile).copied().flatten();
            tally.shadow += traced.shadow.get();
            tally.clip += traced.clip.get();
            tally.lead_in += traced.lead_in.get();
        }
        if let Some(texel) = sparse_layer_texel(sample.idx, raw_visibility) {
            texels.push(texel);
        }
    });
    #[cfg(test)]
    if let (Some(stats), Some(tiles)) = (exact_skips.stats, tallies) {
        stats.push_chart(ChartTally {
            light_origin: light.origin.as_vec3().to_array(),
            face: face_idx,
            dims: (width, height),
            tiles_wide,
            tiles,
        });
    }
    control.advance(1);
    texels
}

/// Segments one texel traced, by kind.
#[cfg(test)]
#[derive(Default)]
struct TracedSegments {
    shadow: Cell<u32>,
    clip: Cell<u32>,
    lead_in: Cell<u32>,
}

/// Verify mode: trace a clear tile's covered texel as the unculled walk
/// would, with copies of both occluder caches as the walk holds them here,
/// and require exactly 1.0 with both copies unchanged, as the skip leaves
/// the caches.
#[cfg(test)]
fn verify_clear_texel(
    light: &MapLight,
    site: &TexelSite,
    sampler: &TexelSampler,
    (bvh, primitives, geometry): (&bvh::bvh::Bvh<f32, 3>, &[BvhPrimitive], &GeometryResult),
    band: &ClipBand,
    [last_occluder, clip_occluder]: [&Cell<Option<usize>>; 2],
) {
    let shadow = Cell::new(last_occluder.get());
    let clip = Cell::new(clip_occluder.get());
    let visibility = sampler.visibility(
        light,
        site,
        |from, to| segment_clear_remembering(bvh, primitives, geometry, from, to, &shadow),
        |from, to| segment_clear_remembering(bvh, primitives, geometry, from, to, &clip),
        |from, to| lead_in_clear(bvh, primitives, geometry, band, from, to),
    );
    assert!(
        visibility == 1.0,
        "shaft cull: a clear tile's texel at {:?} traces visibility {visibility} toward a light at {:?}",
        site.world_p,
        light.origin
    );
    assert_eq!(
        (shadow.get(), clip.get()),
        (last_occluder.get(), clip_occluder.get()),
        "shaft cull: tracing a clear tile's texel at {:?} moved an occluder cache",
        site.world_p
    );
}

/// [`bake_light_layer_chart_controlled`], floored, tracing every texel's clip
/// segments: the reference the clip pre-pass is checked against, with the
/// light's `embedded_emitter` flag as the stage would pass it.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_light_layer_chart_without_clip_prepass(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    embedded_emitter: bool,
    exact_skips: ExactSkips,
    shaft_scene: &ShaftScene,
    control: &BakeControl,
) -> Vec<LayerTexel> {
    bake_light_layer_chart_clipped(
        light,
        atlas,
        face_idx,
        bvh,
        primitives,
        geometry,
        area_sample_count,
        true,
        ProbeEarlyOut::for_light(embedded_emitter),
        false,
        exact_skips,
        shaft_scene,
        control,
    )
}

/// Walk one `(light, chart)` using the exact lightmap raster path, retaining
/// only texels with a direct contribution before visibility is sampled.
#[cfg(test)]
pub(crate) fn visit_light_chart_coverage_controlled(
    light: &MapLight,
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    control: &BakeControl,
    mut visit_covered: impl FnMut(CoveredChartTexel),
) {
    let _permit = control.governor().enter();
    for_each_light_layer_chart_texel(atlas, face_idx, |sample| {
        if light_texel_is_covered(light, sample.world_p, sample.surface_normal) {
            visit_covered(CoveredChartTexel {
                idx: sample.idx,
                layer: atlas.placements[face_idx].layer,
            });
        }
    });
    control.advance(1);
}

#[derive(Clone, Copy)]
pub(crate) struct ChartWalkSample {
    pub idx: u32,
    /// Interior texel coordinates within the chart.
    pub tx: i32,
    pub ty: i32,
    pub world_p: Vec3,
    pub surface_normal: Vec3,
    pub seed: u64,
}

/// Walk one chart without acquiring a governor permit. Parallel bake work
/// enters once at its outermost boundary, so pause and the shared concurrency
/// cap apply to every chart: callers acquire the permit around this walk and
/// any setup in the same parallel item, then advance by one.
pub(crate) fn for_each_light_layer_chart_texel(
    atlas: &SharedAtlas<'_>,
    face_idx: usize,
    mut sample_texel: impl FnMut(ChartWalkSample),
) {
    let placement = &atlas.placements[face_idx];
    let chart = &atlas.charts[face_idx];
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return;
    }

    let padding = crate::chart_raster::CHART_PADDING_TEXELS as i32;
    let (interior_w, interior_h) = chart_interior_dims(chart);
    for ty in 0..interior_h {
        for tx in 0..interior_w {
            let atlas_x = placement.x as i32 + padding + tx;
            let atlas_y = placement.y as i32 + padding + ty;
            // Within-layer index — the atlas layer lives in the enclosing
            // `LightmapLayer.target_layer`, not folded into `idx`.
            let idx = atlas_y as u32 * atlas.atlas_width + atlas_x as u32;

            let world_p = chart_texel_world_position(chart, tx, ty);
            let surface_normal = chart.normal;
            let seed = chart_texel_seed(chart, tx, ty);

            let sample = ChartWalkSample {
                idx,
                tx,
                ty,
                world_p,
                surface_normal,
                seed,
            };
            sample_texel(sample);
        }
    }
}
