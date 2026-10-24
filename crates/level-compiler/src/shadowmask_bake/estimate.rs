// Cut-face owner estimate: a sparse shadowed sample of each cut face, traced
// before the walk, fixes the one owner set its sub-faces share.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use std::cell::Cell;
use std::ops::Range;

use glam::Vec3;
use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{chart_interior_dims, chart_texel_seed, chart_texel_world_position};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{
    CutParent, light_contribution_and_direction, light_texel_contribution_and_visibility_with,
    segment_clear_remembering,
};
use crate::lightmap_layer::SharedAtlas;
use crate::map_data::{LightType, MapLight};
use crate::texel_footprint::{ClipReach, TexelFrame, TexelSampler, TexelSite};

use super::eligibility::EligibleLight;
use super::fill::quantize_visibility;
use super::ownership::{Candidate, FaceChannels, LIT_TEXEL_FLOOR};

/// Parent-grid texels between samples on each axis: one sample per
/// `ESTIMATE_STRIDE²` texels, so a light lighting the floor's worth of texels
/// rarely falls between samples.
pub(crate) const ESTIMATE_STRIDE: u32 = LIT_TEXEL_FLOOR.isqrt();

/// Each sample runs the walk's own visibility sampler (receiver footprint,
/// seed and sample count), so a sample reads exactly what the walk bakes at
/// that texel. A thin own segment with no lattice coordinate is sampled at its
/// centre; the tag names that placement so a build without it never shares a
/// memo entry.
const ESTIMATE_MODE: &[u8] = b"walk-area-samples;thin-segment-centres";

/// The estimate's inputs that can change id 42 while every partition stays
/// the same: stride, sampling mode, and the cut map. Folded into the memo key.
pub(super) fn estimate_params(cut_parents: &[CutParent]) -> Vec<u8> {
    if cut_parents.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend_from_slice(&ESTIMATE_STRIDE.to_le_bytes());
    out.extend_from_slice(&(ESTIMATE_MODE.len() as u32).to_le_bytes());
    out.extend_from_slice(ESTIMATE_MODE);
    for cut in cut_parents {
        for word in [cut.parent, cut.faces.start as u32, cut.faces.end as u32] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        for [u, v] in &cut.own {
            for word in [u.start, u.end, v.start, v.end] {
                out.extend_from_slice(&word.to_le_bytes());
            }
        }
    }
    out
}

/// Every cut face's fixed owners, and how much the estimate traced over all of
/// them.
pub(super) struct CutEstimate {
    /// Per cut parent, in `cut_parents` order, its ranked owners.
    pub(super) owners: Vec<FaceChannels>,
    /// Sample texels over every cut face.
    pub(super) samples: usize,
    /// Sample × light visibility evaluations over every cut face: lights whose
    /// unshadowed term reached the sample.
    pub(super) traces: usize,
}

/// Parent-grid coordinates one own segment is sampled at on one axis: its
/// stride-lattice coordinates, ascending. A segment too thin to hold one (a
/// sub-face one to three texels across) is sampled at its centre instead, so it
/// still gets samples, and they stay inside its own segment.
fn axis_samples(segment: &Range<u32>) -> Vec<u32> {
    let half = ESTIMATE_STRIDE / 2;
    let lattice: Vec<u32> = segment
        .clone()
        .filter(|g| g % ESTIMATE_STRIDE == half)
        .collect();
    if lattice.is_empty() && !segment.is_empty() {
        return vec![segment.start + (segment.end - segment.start) / 2];
    }
    lattice
}

/// Sample texels of `cut`, in sub-face then row-major order, each through the
/// emitted sub-face whose own segment holds it. Own segments are disjoint, so
/// no parent-grid texel is sampled twice. A piece the polygon missed has no
/// sub-face and no sample.
pub(super) fn sample_points(cut: &CutParent, shared: &SharedAtlas<'_>) -> Vec<(usize, i32, i32)> {
    let mut points = Vec::new();
    for (k, [u, v]) in cut.own.iter().enumerate() {
        let face = cut.faces.start + k;
        let chart = &shared.charts[face];
        let window = chart
            .window
            .expect("a cut sub-face's chart windows its parent");
        let (interior_w, interior_h) = chart_interior_dims(chart);
        let columns = axis_samples(u);
        for gy in axis_samples(v) {
            for &gx in &columns {
                let tx = gx as i32 - window.origin[0] as i32;
                let ty = gy as i32 - window.origin[1] as i32;
                if (0..interior_w).contains(&tx) && (0..interior_h).contains(&ty) {
                    points.push((face, tx, ty));
                }
            }
        }
    }
    points
}

/// World bounds of every sub-face's chart interior.
fn parent_bounds(cut: &CutParent, shared: &SharedAtlas<'_>) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for face in cut.faces.clone() {
        let chart = &shared.charts[face];
        let (w, h) = chart_interior_dims(chart);
        for (tx, ty) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            let p = chart_texel_world_position(chart, tx, ty);
            min = min.min(p);
            max = max.max(p);
        }
    }
    (min, max)
}

/// Whether `light`'s falloff can reach the box at all: a cheap cull before any
/// per-sample analytic test. Directional and unranged lights always may.
fn light_may_reach_box(light: &MapLight, (min, max): (Vec3, Vec3)) -> bool {
    if light.light_type == LightType::Directional || light.falloff_range <= 0.0 {
        return true;
    }
    let origin = light.origin.as_vec3();
    origin.clamp(min, max).distance(origin) <= light.falloff_range
}

/// Trace the estimate for every cut face and rank each one's owners. Work is
/// one `(cut face, light)` pair per governed item; results are summed in a
/// fixed sample order and ranked by the same total order as the walk, so the
/// owners never depend on worker count.
#[allow(clippy::too_many_arguments)]
pub(super) fn estimate_cut_owners(
    cut_parents: &[CutParent],
    eligible: &[EligibleLight<'_>],
    shared: &SharedAtlas<'_>,
    bvh: &bvh::bvh::Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    area_sample_count: u32,
    penumbra_floor: bool,
    control: &BakeControl,
) -> CutEstimate {
    let samples: Vec<Vec<(usize, i32, i32)>> = cut_parents
        .iter()
        .map(|cut| sample_points(cut, shared))
        .collect();
    let pairs: Vec<(usize, usize)> = cut_parents
        .iter()
        .enumerate()
        .flat_map(|(c, cut)| {
            let bounds = parent_bounds(cut, shared);
            eligible
                .iter()
                .enumerate()
                .filter(move |(_, light)| light_may_reach_box(light.light, bounds))
                .map(move |(l, _)| (c, l))
        })
        .collect();
    // Clip reach is light-independent: one per sub-face, read by every light.
    let clip_reaches: Vec<Vec<ClipReach>> = if penumbra_floor {
        cut_parents
            .par_iter()
            .map(|cut| {
                cut.faces
                    .clone()
                    .map(|face| {
                        ClipReach::for_chart(&shared.charts[face], bvh, primitives, geometry)
                    })
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    // (lit samples, contribution, traces) per pair, in pair order.
    let results: Vec<(u32, f64, usize)> = pairs
        .par_iter()
        .map(|&(c, l)| {
            let _permit = control.governor().enter();
            let light = eligible[l].light;
            let sampler = TexelSampler::new(light, area_sample_count, penumbra_floor);
            let last_occluder = Cell::new(None);
            let clip_occluder = Cell::new(None);
            let (mut lit, mut contribution, mut traces) = (0u32, 0f64, 0usize);
            for &(face, tx, ty) in &samples[c] {
                let chart = &shared.charts[face];
                let world_p = chart_texel_world_position(chart, tx, ty);
                // A sub-chart's frame is its parent grid's, so every sample of
                // one cut face draws the walk's own footprint offsets.
                let site = TexelSite {
                    world_p,
                    normal: chart.normal,
                    seed: chart_texel_seed(chart, tx, ty),
                    frame: TexelFrame::for_chart(chart),
                    may_clip: clip_reaches.get(c).is_none_or(|reaches| {
                        reaches[face - cut_parents[c].faces.start].may_clip(tx, ty)
                    }),
                };
                let (_, _, visibility) = light_texel_contribution_and_visibility_with(
                    light,
                    world_p,
                    chart.normal,
                    || {
                        sampler.visibility(
                            light,
                            &site,
                            |from, to| {
                                segment_clear_remembering(
                                    bvh,
                                    primitives,
                                    geometry,
                                    from,
                                    to,
                                    &last_occluder,
                                )
                            },
                            |from, to| {
                                segment_clear_remembering(
                                    bvh,
                                    primitives,
                                    geometry,
                                    from,
                                    to,
                                    &clip_occluder,
                                )
                            },
                        )
                    },
                );
                let Some(visibility) = visibility else {
                    continue;
                };
                traces += 1;
                let Some(quantized) = quantize_visibility(visibility).filter(|&q| q != 0) else {
                    continue;
                };
                let (unshadowed, _) =
                    light_contribution_and_direction(light, world_p, chart.normal);
                lit += 1;
                contribution += f64::from(unshadowed.x + unshadowed.y + unshadowed.z)
                    * f64::from(quantized)
                    / 255.0;
            }
            (lit, contribution, traces)
        })
        .collect();

    let area = ESTIMATE_STRIDE * ESTIMATE_STRIDE;
    let mut owners = vec![FaceChannels::default(); cut_parents.len()];
    let mut traces = 0;
    for (&(c, l), &(lit, contribution, pair_traces)) in pairs.iter().zip(&results) {
        traces += pair_traces;
        if lit == 0 {
            continue;
        }
        let light = &eligible[l];
        // Ranking reads the estimate scaled to the face's area.
        owners[c].offer(Candidate {
            spec_index: light.spec_index,
            lit_texels: lit.saturating_mul(area),
            contribution: contribution * f64::from(area),
            promoted: light.promoted,
        });
    }
    // Counted on the caller thread: the traces themselves ran on workers.
    #[cfg(test)]
    super::record_estimate_traces(traces);
    CutEstimate {
        owners,
        samples: samples.iter().map(Vec::len).sum(),
        traces,
    }
}
