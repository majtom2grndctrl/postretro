// Animated-light weight-map baker.
// See: context/lib/build_pipeline.md §Build Cache

use bvh::bvh::Bvh;
use glam::Vec3;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use rayon::prelude::*;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{
    CHART_PADDING_TEXELS, ChartPlacement, chart_interior_dims, chart_texel_world_position,
};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{
    Chart, light_contribution_and_direction, segment_clear, soft_visibility,
};
use crate::map_data::MapLight;

/// Dropped as numerical noise; prevents per-texel lists from inflating on
/// contributions too dim to matter.
const WEIGHT_EPSILON: f32 = 1.0e-6;

/// Cache stage version for the animated-light weight-map bake. Bumped to 2 on
/// the sdf-static-occluder-shadows branch when the bake started retaining the
/// per-light per-texel incoming direction (Task 2b). v3 bump
/// (sdf-per-light-shadows Task 3): the direct weight-map now drops `sdf`-typed
/// lights (their direct term resolves at runtime), so a stale entry could carry
/// a baked direct weight the runtime double-counts. Bumps invalidate any prior
/// cache entries (the `animated_lm_weight_maps` cache key folds this in
/// alongside the input hash — the same per-stage version-constant pattern every
/// cached stage uses).
///
/// v4 bump (baked-soft-lightmap-shadows Task 4): the binary `shadow_visible`
/// membership gate was replaced with a soft area-light visibility fraction
/// multiplied into `TexelLight.weight` (penumbra texels now carry fractional
/// weight instead of a 0/1 include).
///
/// v5 bump (baked-soft-lightmap-shadows F1): `soft_visibility`'s probe set became
/// a strided emitter subset, shifting the soft weight for some probe geometry.
/// Bumped for consistency with `lightmap_bake`/`sh_bake`.
///
/// This stage is now cached: `main.rs` wraps the bake in a `StageCache`
/// get/insert round-trip under the `animated_lm_weight_maps` cache key, which
/// folds this `STAGE_VERSION` in alongside the input hash — the same per-stage
/// version-constant pattern every cached stage uses. Bumping this constant
/// invalidates every prior cache entry for the stage on the next build. The
/// `CacheKey`/STAGE_VERSION contract is exercised by
/// `stage_version_bump_misses_then_hits` and `stage_version_bump_changes_cache_key`
/// in this module's test suite.
pub const STAGE_VERSION: u32 = 5;

pub struct WeightMapInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub chunk_section: &'a AnimatedLightChunksSection,
    /// Filtered `!is_dynamic && animation.is_some()` — same filter as
    /// `sh_bake.rs` for `animation_descriptors`, so indices agree without remap.
    pub lights: &'a [MapLight],
    pub face_charts: &'a [Chart],
    pub face_placements: &'a [ChartPlacement],
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Area-sample count for soft-shadow penumbra visibility (Task 6 knob).
    /// Folded into the stage's `wm_input_hash` in `main.rs` (via
    /// `args.soft_shadow_samples.to_le_bytes()`), so changing this value
    /// produces a cache miss and triggers a full re-bake. Default
    /// `lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT`.
    pub area_sample_count: u32,
}

struct ChunkBakeResult {
    rect: ChunkAtlasRect, // texel_offset filled by concatenation
    /// chunk-local offsets; concatenation pass rewrites to global offsets.
    offset_counts: Vec<TexelLightEntry>,
    texel_lights: Vec<TexelLight>,
}

pub fn bake_animated_light_weight_maps(
    inputs: &WeightMapInputs<'_>,
) -> AnimatedLightWeightMapsSection {
    if inputs.chunk_section.chunks.is_empty() {
        return AnimatedLightWeightMapsSection::empty();
    }

    let chunks = &inputs.chunk_section.chunks;
    let light_indices_pool = &inputs.chunk_section.light_indices;

    let per_chunk: Vec<ChunkBakeResult> = chunks
        .par_iter()
        .map(|chunk| bake_one_chunk(inputs, chunk, light_indices_pool))
        .collect();

    assert_no_overlapping_rects_per_face(chunks, &per_chunk);

    let mut chunk_rects: Vec<ChunkAtlasRect> = Vec::with_capacity(per_chunk.len());
    let mut offset_counts: Vec<TexelLightEntry> = Vec::new();
    let mut texel_lights: Vec<TexelLight> = Vec::new();

    let mut running_texel_offset: u32 = 0;
    for result in per_chunk {
        let ChunkBakeResult {
            mut rect,
            offset_counts: chunk_oc,
            texel_lights: chunk_tl,
        } = result;

        rect.texel_offset = running_texel_offset;
        running_texel_offset += rect.width * rect.height;

        let light_base = texel_lights.len() as u32;
        for entry in chunk_oc.into_iter() {
            offset_counts.push(TexelLightEntry {
                offset: entry.offset + light_base,
                count: entry.count,
            });
        }
        texel_lights.extend(chunk_tl);
        chunk_rects.push(rect);
    }

    // Byte formula mirrors the section encoder. TexelLight grew to 12 bytes
    // when the per-texel direction was added (Task 2b).
    const HEADER_SIZE: usize = 16;
    const CHUNK_RECT_SIZE: usize = 20;
    const OFFSET_ENTRY_SIZE: usize = 8;
    const TEXEL_LIGHT_SIZE: usize = 12;
    let byte_size = HEADER_SIZE
        + chunk_rects.len() * CHUNK_RECT_SIZE
        + offset_counts.len() * OFFSET_ENTRY_SIZE
        + texel_lights.len() * TEXEL_LIGHT_SIZE;

    let covered_texels: u32 = offset_counts.iter().filter(|e| e.count > 0).count() as u32;
    let mean_lights_per_covered = if covered_texels == 0 {
        0.0
    } else {
        texel_lights.len() as f64 / covered_texels as f64
    };
    let peak_texels_per_chunk = chunk_rects
        .iter()
        .map(|r| r.width * r.height)
        .max()
        .unwrap_or(0);

    log::info!(
        "[AnimatedLightWeightMaps] {} chunks, {} byte section, {} covered texels, \
         mean {:.2} lights / covered texel, peak {} texels / chunk",
        chunk_rects.len(),
        byte_size,
        covered_texels,
        mean_lights_per_covered,
        peak_texels_per_chunk,
    );

    AnimatedLightWeightMapsSection {
        chunk_rects,
        offset_counts,
        texel_lights,
    }
}

fn bake_one_chunk(
    inputs: &WeightMapInputs<'_>,
    chunk: &postretro_level_format::animated_light_chunks::AnimatedLightChunk,
    light_indices_pool: &[u32],
) -> ChunkBakeResult {
    let face_index = chunk.face_index as usize;
    let chart = &inputs.face_charts[face_index];
    let placement = inputs.face_placements[face_index];
    let (interior_w, interior_h) = chart_interior_dims(chart);

    let (atlas_x, atlas_y, width, height) = chunk_atlas_rect(
        chart,
        placement,
        chunk.uv_min,
        chunk.uv_max,
        inputs.atlas_width,
        inputs.atlas_height,
    );

    let list_start = chunk.index_offset as usize;
    let list_end = list_start + chunk.index_count as usize;
    let chunk_light_indices: &[u32] = &light_indices_pool[list_start..list_end];

    let rect = ChunkAtlasRect {
        atlas_x,
        atlas_y,
        width,
        height,
        texel_offset: 0, // filled by caller
    };

    let texel_count = (width * height) as usize;
    let mut offset_counts: Vec<TexelLightEntry> = Vec::with_capacity(texel_count);
    let mut texel_lights: Vec<TexelLight> = Vec::new();

    let padding = CHART_PADDING_TEXELS as i32;

    let chart_usable = chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0;

    // Row-major to match section encoding: chunk_rect.texel_offset + ty * width + tx.
    for ty in 0..height {
        for tx in 0..width {
            let ax = atlas_x + tx;
            let ay = atlas_y + ty;
            // Map back into the chart's interior coordinate space.
            let tx_interior = ax as i32 - placement.x as i32 - padding;
            let ty_interior = ay as i32 - placement.y as i32 - padding;

            // Texels outside the chart (artifact of outward rounding): zero-count entry.
            if !chart_usable
                || tx_interior < 0
                || ty_interior < 0
                || tx_interior >= interior_w
                || ty_interior >= interior_h
            {
                offset_counts.push(TexelLightEntry {
                    offset: texel_lights.len() as u32,
                    count: 0,
                });
                continue;
            }

            let world_p =
                chart_texel_world_position(chart, tx_interior, ty_interior, interior_w, interior_h);
            let surface_normal = chart.normal;

            // Deterministic per-texel seed for soft-visibility sampling: a fixed
            // integer hash of the texel's atlas coordinate. Same convention as the
            // static lightmap stage (texel `(x, y)` hash, never `RandomState`), so
            // the bake stays byte-identical across processes. `(ax, ay)` is the
            // texel's stable identity in this loop.
            let texel_seed = soft_visibility_texel_seed(ax, ay);

            let offset_start = texel_lights.len() as u32;
            let mut count: u32 = 0;
            for &light_index in chunk_light_indices {
                let light = &inputs.lights[light_index as usize];
                // Disjoint-direct exclusion at the direct weight-map consumer:
                // `sdf`-typed lights resolve their direct term at runtime, so
                // they contribute no baked `lm_anim` weight. They stay in
                // `inputs.lights` (index alignment with the chunk section /
                // delta-SH bake) but emit zero weight here. Dynamic-tier lights
                // are already absent — the `AnimatedBakedLights` namespace keys
                // on `!is_dynamic`. The soft-visibility multiply below composes
                // *after* this filter, so sdf-typed lights stay fully skipped.
                if light.shadow_type == crate::map_data::ShadowType::Sdf {
                    continue;
                }
                let (contribution, dir) =
                    light_contribution_and_direction(light, world_p, surface_normal);
                let weight = contribution_to_weight(contribution, light.color, light.intensity);
                if weight <= WEIGHT_EPSILON {
                    continue;
                }
                // Soft area-light visibility replaces the old binary
                // `shadow_visible` gate (baked-soft-lightmap-shadows Task 4): the
                // `[0, 1]` unoccluded fraction scales the emitted weight, so
                // penumbra texels carry a fractional weight rather than a hard
                // include/exclude. Fully occluded (`v <= 0`) emits no entry, same
                // sparsity as before. Because `weight` is a continuous multiplier
                // consumed by the compose pre-pass (no thresholding there), the
                // shadow *shape* stays fixed as the light's intensity animates —
                // intensity is a separate per-frame scalar. Wraps this module's
                // `segment_clear` as the trace closure (the helper is
                // trace-context-agnostic per Task 2).
                let v = soft_visibility(
                    world_p,
                    surface_normal,
                    light,
                    texel_seed,
                    inputs.area_sample_count,
                    |from, to| {
                        segment_clear(inputs.bvh, inputs.primitives, inputs.geometry, from, to)
                    },
                );
                if v <= 0.0 {
                    continue;
                }
                let weight = weight * v;
                // Retain the per-light per-texel incoming direction the
                // contribution calculation already computed (Task 2b of
                // sdf-static-occluder-shadows). The light's geometry is
                // static, so this direction is bakeable; the compose pass
                // weights it by the light's per-frame radiance to fuse a
                // runtime dominant-direction atlas the SDF traces toward.
                // `light_contribution_and_direction` returns `Vec3::Y` for
                // degenerate (zero-distance) cases — `weight > EPSILON`
                // already gates those out, so `dir` here is meaningful.
                let direction_oct = postretro_level_format::octahedral::encode(dir.x, dir.y, dir.z);
                texel_lights.push(TexelLight {
                    light_index,
                    weight,
                    direction_oct,
                });
                count += 1;
            }

            offset_counts.push(TexelLightEntry {
                offset: offset_start,
                count,
            });
        }
    }

    ChunkBakeResult {
        rect,
        offset_counts,
        texel_lights,
    }
}

/// Deterministic per-texel seed for `soft_visibility`'s sample-lattice rotation,
/// derived from a fixed integer hash of the texel's atlas coordinate `(x, y)`.
/// No `RandomState`, no hash-order dependence — same `(x, y)` always yields the
/// same seed, so the bake is byte-identical across processes. Mixing follows the
/// SplitMix64 finalizer so adjacent texels decorrelate.
///
/// The static lightmap stage seeds the same way (deterministic per-texel) but with
/// a different mixer (FNV-1a). The two need not match: each stage bakes into its
/// own INDEPENDENT atlas, so per-stage determinism is all that's required.
fn soft_visibility_texel_seed(x: u32, y: u32) -> u64 {
    let mut z = ((x as u64) << 32) | (y as u64);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Strips base color and intensity so the weight is a neutral Lambert × falloff
/// × cone scalar; runtime compose re-applies color/intensity from the descriptor.
/// Picks the dominant color channel to avoid divide-by-near-zero on weak channels.
fn contribution_to_weight(contribution: Vec3, color: [f32; 3], intensity: f32) -> f32 {
    let (c_contrib, c_color) = if color[0] >= color[1] && color[0] >= color[2] {
        (contribution.x, color[0])
    } else if color[1] >= color[2] {
        (contribution.y, color[1])
    } else {
        (contribution.z, color[2])
    };
    let denom = c_color * intensity;
    if denom <= 1.0e-6 {
        return 0.0;
    }
    (c_contrib / denom).max(0.0)
}

/// Center-based half-open ownership: a chart-interior texel `t` (whose center
/// projects to UV `chart.uv_min + (t + 0.5) / interior * uv_extent`) belongs to
/// a chunk iff its center UV is in `[chunk.uv_min, chunk.uv_max)`. Siblings
/// share UV boundaries exactly (A.uv_max == B.uv_min) and under this rule pack
/// into adjacent atlas rects with no overlap and no gap. The
/// `assert_no_overlapping_rects_per_face` postcondition guards the invariant.
fn chunk_atlas_rect(
    chart: &Chart,
    placement: ChartPlacement,
    chunk_uv_min: [f32; 2],
    chunk_uv_max: [f32; 2],
    atlas_width: u32,
    atlas_height: u32,
) -> (u32, u32, u32, u32) {
    let (interior_w, interior_h) = chart_interior_dims(chart);
    let padding = CHART_PADDING_TEXELS as f32;

    // Degenerate chart — one-texel rect so downstream width × height ≥ 1.
    if chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <= 0.0 {
        return (placement.x, placement.y, 1, 1);
    }

    let scale_u = interior_w as f32 / chart.uv_extent[0];
    let scale_v = interior_h as f32 / chart.uv_extent[1];

    // Interior-relative texel coord `tx = (chunk_uv - chart.uv_min) * scale - 0.5`
    // is the center-space position of the chunk boundary; texels owned by the
    // chunk are integer indices `>= ceil(fx_min_interior)` (and `< ceil(fx_max)`
    // for the exclusive max).
    //
    // Shared boundaries on siblings are not bit-exact: recursive halving of a
    // non-dyadic chart extent leaves the two sides drifting by ~1e-7 in f32.
    // When that drift straddles an integer, the two `ceil`s disagree by one
    // and adjacent atlas rects overlap by a texel row/column. Snap to absorb
    // the drift before rounding.
    //
    // Epsilon is in interior-texel units; observed drift is ~1e-5 there.
    // The nearest a genuine (non-shared) split can land to an integer in
    // interior-texel space is 0.5: the subdivider only cuts at UV midpoints,
    // and a midpoint of any sub-range maps to the midpoint between two adjacent
    // texel-boundary integers — so 1e-4 is above the noise floor but at least
    // 5000x clear of any real boundary. See
    // `sibling_chunks_with_drifted_shared_uv_edge_pack_without_overlap` for a
    // worked example with the precise drift values the subdivider produces.
    const BOUNDARY_SNAP_EPS: f32 = 1.0e-4;
    let snap_to_int = |x: f32| -> f32 {
        let r = x.round();
        if (x - r).abs() < BOUNDARY_SNAP_EPS {
            r
        } else {
            x
        }
    };
    let fx_min_interior = snap_to_int((chunk_uv_min[0] - chart.uv_min[0]) * scale_u - 0.5);
    let fx_max_interior = snap_to_int((chunk_uv_max[0] - chart.uv_min[0]) * scale_u - 0.5);
    let fy_min_interior = snap_to_int((chunk_uv_min[1] - chart.uv_min[1]) * scale_v - 0.5);
    let fy_max_interior = snap_to_int((chunk_uv_max[1] - chart.uv_min[1]) * scale_v - 0.5);

    let fx_min_unclamped = placement.x as f32 + padding + fx_min_interior.ceil();
    let fx_max_unclamped = placement.x as f32 + padding + fx_max_interior.ceil();
    let fy_min_unclamped = placement.y as f32 + padding + fy_min_interior.ceil();
    let fy_max_unclamped = placement.y as f32 + padding + fy_max_interior.ceil();

    // Clamp before `f32 as u32`: a misplaced chart can put coordinates below 0,
    // and `(-n as u32)` saturates to 0 (wrong). Clamping pins rogue rects to
    // the atlas edge; the interior-check loop in `bake_one_chunk` skips out-of-range texels.
    let atlas_w_f = atlas_width as f32;
    let atlas_h_f = atlas_height as f32;
    let fx_min = fx_min_unclamped.clamp(0.0, atlas_w_f);
    let fx_max = fx_max_unclamped.clamp(0.0, atlas_w_f);
    let fy_min = fy_min_unclamped.clamp(0.0, atlas_h_f);
    let fy_max = fy_max_unclamped.clamp(0.0, atlas_h_f);

    let ax_min_raw = fx_min as u32;
    let ay_min_raw = fy_min as u32;
    let ax_max_raw = (fx_max as u32).max(ax_min_raw + 1);
    let ay_max_raw = (fy_max as u32).max(ay_min_raw + 1);

    // Clamp the min corner: without this, a chart past the atlas bound can make
    // `ax_max - ax_min` underflow, handing a 1-texel rect to `textureStore` at
    // an out-of-bounds coord. Pin to the last valid texel column/row.
    let ax_min = ax_min_raw.min(atlas_width.saturating_sub(1));
    let ay_min = ay_min_raw.min(atlas_height.saturating_sub(1));
    let ax_max = ax_max_raw.min(atlas_width).max(ax_min + 1);
    let ay_max = ay_max_raw.min(atlas_height).max(ay_min + 1);
    let width = ax_max - ax_min;
    let height = ay_max - ay_min;

    (ax_min, ay_min, width, height)
}

/// If this fires, the UV packer violated the required 1-atlas-texel gap between
/// adjacent chunk boundaries within a face — fix the packer, not this baker.
fn assert_no_overlapping_rects_per_face(
    chunks: &[postretro_level_format::animated_light_chunks::AnimatedLightChunk],
    per_chunk: &[ChunkBakeResult],
) {
    use std::collections::HashMap;
    let mut by_face: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, c) in chunks.iter().enumerate() {
        by_face.entry(c.face_index).or_default().push(i);
    }
    for (face_index, indices) in &by_face {
        for (i_idx, &i) in indices.iter().enumerate() {
            let a = &per_chunk[i].rect;
            for &j in &indices[i_idx + 1..] {
                let b = &per_chunk[j].rect;
                let overlap_x = a.atlas_x < b.atlas_x + b.width && b.atlas_x < a.atlas_x + a.width;
                let overlap_y =
                    a.atlas_y < b.atlas_y + b.height && b.atlas_y < a.atlas_y + a.height;
                if overlap_x && overlap_y {
                    let ca = &chunks[i];
                    let cb = &chunks[j];
                    panic!(
                        "animated-light chunks {i} and {j} on face {face_index} produced \
                         overlapping atlas rects under center-based half-open ownership \
                         ({}x{}+{}+{} vs {}x{}+{}+{}); chunk UVs [{:?}..{:?}] vs \
                         [{:?}..{:?}]. Likely causes: subdivider emitted truly \
                         overlapping UV ranges, or shared-boundary float drift exceeded \
                         `chunk_atlas_rect`'s BOUNDARY_SNAP_EPS.",
                        a.width,
                        a.height,
                        a.atlas_x,
                        a.atlas_y,
                        b.width,
                        b.height,
                        b.atlas_x,
                        b.atlas_y,
                        ca.uv_min,
                        ca.uv_max,
                        cb.uv_min,
                        cb.uv_max,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh_build::build_bvh;
    use crate::geometry::FaceIndexRange;
    use crate::map_data::{FalloffModel, LightAnimation, LightType};
    use glam::DVec3;
    use postretro_level_format::animated_light_chunks::{
        AnimatedLightChunk, AnimatedLightChunksSection,
    };
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn xz_quad_face(y: f32, normal_y: f32, vertex_base: f32) -> Vec<Vertex> {
        let n = [0.0, normal_y, 0.0];
        let t = [1.0, 0.0, 0.0];
        vec![
            Vertex::new([vertex_base, y, 0.0], [0.0, 0.0], n, t, true, [0.0, 0.0], 0),
            Vertex::new(
                [vertex_base + 1.0, y, 0.0],
                [1.0, 0.0],
                n,
                t,
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [vertex_base + 1.0, y, 1.0],
                [1.0, 1.0],
                n,
                t,
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new([vertex_base, y, 1.0], [0.0, 1.0], n, t, true, [0.0, 0.0], 0),
        ]
    }

    fn unit_floor_geometry() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: xz_quad_face(0.0, 1.0, 0.0),
                indices: vec![0, 1, 2, 0, 2, 3],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![FaceIndexRange {
                index_offset: 0,
                index_count: 6,
            }],
        }
    }

    /// Floor (face 0) at y=0; ceiling blocker (face 1) at y=0.5 between light and floor.
    fn floor_plus_blocker_geometry() -> GeometryResult {
        let mut vertices = xz_quad_face(0.0, 1.0, 0.0);
        // Ceiling larger than the floor so the shadow ray always hits it.
        let ceiling = vec![
            Vertex::new(
                [-1.0, 0.5, -1.0],
                [0.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [2.0, 0.5, -1.0],
                [1.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [2.0, 0.5, 2.0],
                [1.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [-1.0, 0.5, 2.0],
                [0.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
        ];
        vertices.extend(ceiling);
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
                faces: vec![
                    FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    },
                    FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    },
                ],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![
                FaceIndexRange {
                    index_offset: 0,
                    index_count: 6,
                },
                FaceIndexRange {
                    index_offset: 6,
                    index_count: 6,
                },
            ],
        }
    }

    /// Floor (face 0) at y=0; a *partial* blocker (face 1) at y=0.5 covering
    /// only x ∈ [-1, 0.5] (z ∈ [-1, 2]). With a large-`light_size` point light at
    /// (0.5, 1, 0.5), floor texels near the blocker edge see part of the light's
    /// area disk past the edge and part occluded → fractional soft visibility.
    fn floor_plus_partial_blocker_geometry() -> GeometryResult {
        let mut vertices = xz_quad_face(0.0, 1.0, 0.0);
        let blocker = vec![
            Vertex::new(
                [-1.0, 0.5, -1.0],
                [0.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [0.5, 0.5, -1.0],
                [1.0, 0.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [0.5, 0.5, 2.0],
                [1.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
            Vertex::new(
                [-1.0, 0.5, 2.0],
                [0.0, 1.0],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ),
        ];
        vertices.extend(blocker);
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
                faces: vec![
                    FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    },
                    FaceMeta {
                        leaf_index: 0,
                        texture_index: 0,
                    },
                ],
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: vec![
                FaceIndexRange {
                    index_offset: 0,
                    index_count: 6,
                },
                FaceIndexRange {
                    index_offset: 6,
                    index_count: 6,
                },
            ],
        }
    }

    fn animated_point_light_above() -> MapLight {
        MapLight {
            origin: DVec3::new(0.5, 1.0, 0.5),
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 5.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: Some(LightAnimation {
                period: 1.0,
                phase: 0.0,
                brightness: Some(vec![1.0, 0.5]),
                color: None,
                direction: None,
                start_active: true,
            }),
            bake_only: false,
            is_dynamic: false,
            is_animated: false,
            casts_entity_shadows: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
        }
    }

    /// Same as `animated_point_light_above` but with a large `light_size` so the
    /// soft-visibility path activates (the disk subtends enough that a partial
    /// occluder yields a fractional unoccluded fraction rather than a hard 0/1).
    fn soft_animated_point_light_above() -> MapLight {
        MapLight {
            light_size: 0.5,
            ..animated_point_light_above()
        }
    }

    fn bake_with_geometry_and_chunks<F>(
        geo: GeometryResult,
        lights: Vec<MapLight>,
        build_chunks: F,
    ) -> AnimatedLightWeightMapsSection
    where
        F: FnOnce(&[Chart]) -> AnimatedLightChunksSection,
    {
        bake_with_sample_count(
            geo,
            lights,
            crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
            build_chunks,
        )
    }

    fn bake_with_sample_count<F>(
        mut geo: GeometryResult,
        lights: Vec<MapLight>,
        area_sample_count: u32,
        build_chunks: F,
    ) -> AnimatedLightWeightMapsSection
    where
        F: FnOnce(&[Chart]) -> AnimatedLightChunksSection,
    {
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let static_lights = crate::light_namespaces::StaticBakedLights::from_lights(&lights);
        let mut lm_ctx = crate::lightmap_bake::LightmapBakeCtx {
            bvh: &bvh,
            primitives: &prims,
            geometry: &mut geo,
            lights: &static_lights,
        };
        let lm_output = crate::lightmap_bake::bake_lightmap(
            &mut lm_ctx,
            &crate::lightmap_bake::LightmapConfig {
                lightmap_density: 0.25,
                area_sample_count: crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
                uncompressed_irradiance: false,
            },
        )
        .unwrap();

        let chunk_section = build_chunks(&lm_output.charts);

        let inputs = WeightMapInputs {
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
            chunk_section: &chunk_section,
            lights: &lights,
            face_charts: &lm_output.charts,
            face_placements: &lm_output.placements,
            atlas_width: lm_output.atlas_width,
            atlas_height: lm_output.atlas_height,
            area_sample_count,
        };
        bake_animated_light_weight_maps(&inputs)
    }

    fn full_face_chunk(
        charts: &[Chart],
        face_index: u32,
        light_indices: Vec<u32>,
    ) -> AnimatedLightChunksSection {
        let chart = &charts[face_index as usize];
        let uv_min = chart.uv_min;
        let uv_max = [
            chart.uv_min[0] + chart.uv_extent[0],
            chart.uv_min[1] + chart.uv_extent[1],
        ];
        let index_count = light_indices.len() as u32;
        AnimatedLightChunksSection {
            chunks: vec![AnimatedLightChunk {
                aabb_min: [0.0, 0.0, 0.0],
                face_index,
                aabb_max: [1.0, 0.0, 1.0],
                index_offset: 0,
                uv_min,
                uv_max,
                index_count,
                _padding: 0,
            }],
            light_indices,
        }
    }

    #[test]
    fn single_chunk_single_light_emits_one_light_per_covered_texel() {
        let section = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );

        assert_eq!(section.chunk_rects.len(), 1);
        let rect = &section.chunk_rects[0];
        assert!(rect.width >= 1 && rect.height >= 1);
        assert!(section.is_consistent());

        let mut covered = 0;
        for entry in &section.offset_counts {
            if entry.count > 0 {
                covered += 1;
                assert_eq!(
                    entry.count, 1,
                    "expected exactly one light per covered texel"
                );
                let tl = &section.texel_lights[entry.offset as usize];
                assert_eq!(tl.light_index, 0);
                assert!(
                    tl.weight > 0.0 && tl.weight <= 1.0 + 1.0e-4,
                    "unexpected weight {}",
                    tl.weight
                );
            }
        }
        assert!(covered > 0, "expected at least one covered texel");
    }

    #[test]
    fn parallel_plate_occluder_zeros_shadowed_texels() {
        let section = bake_with_geometry_and_chunks(
            floor_plus_blocker_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );

        for entry in &section.offset_counts {
            assert_eq!(
                entry.count, 0,
                "expected every floor texel to be fully occluded by the ceiling",
            );
        }
        assert!(
            section.texel_lights.is_empty(),
            "no weights should be emitted when every texel is shadowed",
        );
    }

    /// Task 4: a partially-occluded soft light produces *fractional* weights in
    /// penumbra texels, not just the old binary 0/1 include. At least one covered
    /// texel must carry a weight strictly between the noise floor and the fully-
    /// lit value, and that fractional texel must be a covered (emitted) entry.
    #[test]
    fn soft_light_partial_occluder_emits_fractional_weight() {
        // Hard reference: a zero-size light over the same partial blocker bakes a
        // crisp 0/1 mask. Its maximum (fully-lit) weight bounds "fully lit".
        let hard = bake_with_geometry_and_chunks(
            floor_plus_partial_blocker_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        let hard_max = hard
            .texel_lights
            .iter()
            .map(|tl| tl.weight)
            .fold(0.0_f32, f32::max);
        assert!(hard_max > 0.0, "hard reference produced no lit texels");

        let soft = bake_with_geometry_and_chunks(
            floor_plus_partial_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        assert!(soft.is_consistent());

        // A penumbra weight is one strictly below the fully-lit value at the same
        // geometric falloff (scaled down by soft visibility < 1) but above the
        // numerical noise floor — i.e. a continuous gradient, not 0/1.
        let mut penumbra_count = 0;
        for entry in &soft.offset_counts {
            for i in 0..entry.count {
                let tl = &soft.texel_lights[(entry.offset + i) as usize];
                assert!(
                    tl.weight > WEIGHT_EPSILON,
                    "emitted entries must clear the noise floor; got {}",
                    tl.weight,
                );
                if tl.weight < hard_max * 0.95 {
                    penumbra_count += 1;
                }
            }
        }
        assert!(
            penumbra_count > 0,
            "expected at least one penumbra texel with fractional (< fully-lit) \
             weight under soft visibility, found none",
        );
    }

    /// Task 4: fully-occluded texels under a soft light still emit *no* entry —
    /// soft visibility of 0 means "drop", same sparsity as the old binary gate.
    /// The full (large) blocker covers the whole floor, so every disk sample is
    /// occluded for every texel.
    #[test]
    fn soft_light_full_occluder_emits_no_entry() {
        let section = bake_with_geometry_and_chunks(
            floor_plus_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        for entry in &section.offset_counts {
            assert_eq!(
                entry.count, 0,
                "a fully-occluded soft light must emit no entry (v <= 0)",
            );
        }
        assert!(section.texel_lights.is_empty());
    }

    /// Task 4: the soft-visibility multiply composes *after* the sdf-typed
    /// `continue`, so `sdf`-typed lights remain fully skipped (no baked weight).
    #[test]
    fn sdf_typed_soft_light_is_skipped() {
        let mut light = soft_animated_point_light_above();
        light.shadow_type = crate::map_data::ShadowType::Sdf;
        let section = bake_with_geometry_and_chunks(unit_floor_geometry(), vec![light], |charts| {
            full_face_chunk(charts, 0, vec![0])
        });
        for entry in &section.offset_counts {
            assert_eq!(entry.count, 0, "sdf-typed lights emit no baked weight");
        }
        assert!(section.texel_lights.is_empty());
    }

    /// Task 6: `area_sample_count` is folded into `wm_input_hash` in `main.rs`,
    /// so changing it produces a cache miss and re-bake. This test verifies the
    /// field actually reaches `soft_visibility` — raising it shifts penumbra
    /// weights at the higher stratification resolution. The cache-miss contract
    /// is covered separately by `stage_version_bump_misses_then_hits`.
    #[test]
    fn area_sample_count_field_changes_penumbra_weights() {
        let low = bake_with_sample_count(
            floor_plus_partial_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            16,
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        let high = bake_with_sample_count(
            floor_plus_partial_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            64,
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        assert!(low.is_consistent() && high.is_consistent());

        // Collect penumbra weights (those strictly below the per-build max) at
        // each sample count. The finer stratification at 64 quantizes the
        // fraction differently, so the multisets must differ.
        let low_weights: Vec<u32> = low
            .texel_lights
            .iter()
            .map(|t| t.weight.to_bits())
            .collect();
        let high_weights: Vec<u32> = high
            .texel_lights
            .iter()
            .map(|t| t.weight.to_bits())
            .collect();
        assert_ne!(
            low_weights, high_weights,
            "raising the area-sample-count knob must change baked penumbra weights",
        );
    }

    /// Task 4: the soft bake stays deterministic — the per-texel seed is a fixed
    /// hash of `(x, y)`, no RNG / hash-order, so two builds are byte-identical.
    #[test]
    fn soft_light_determinism_two_builds_byte_identical() {
        let bytes_a = bake_with_geometry_and_chunks(
            floor_plus_partial_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        )
        .to_bytes();
        let bytes_b = bake_with_geometry_and_chunks(
            floor_plus_partial_blocker_geometry(),
            vec![soft_animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        )
        .to_bytes();
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn determinism_two_builds_byte_identical() {
        let bytes_a = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        )
        .to_bytes();
        let bytes_b = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        )
        .to_bytes();
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn empty_chunk_section_yields_empty_output() {
        let section = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |_charts| AnimatedLightChunksSection::empty(),
        );
        assert!(section.chunk_rects.is_empty());
        assert!(section.offset_counts.is_empty());
        assert!(section.texel_lights.is_empty());
    }

    /// Two chunks with a 1-texel UV gap (matching chart packer guarantee) so their
    /// rounded rects don't overlap. Verifies prefix-sum texel_offset invariant.
    #[test]
    fn texel_offsets_form_prefix_sum_partition() {
        let section = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| {
                let chart = &charts[0];
                let u0 = chart.uv_min[0];
                let u1 = chart.uv_min[0] + chart.uv_extent[0];
                let v0 = chart.uv_min[1];
                let v1 = chart.uv_min[1] + chart.uv_extent[1];
                let u_mid_lo = u0 + 0.4 * chart.uv_extent[0];
                let u_mid_hi = u0 + 0.6 * chart.uv_extent[0];
                AnimatedLightChunksSection {
                    chunks: vec![
                        AnimatedLightChunk {
                            aabb_min: [0.0, 0.0, 0.0],
                            face_index: 0,
                            aabb_max: [0.5, 0.0, 1.0],
                            index_offset: 0,
                            uv_min: [u0, v0],
                            uv_max: [u_mid_lo, v1],
                            index_count: 1,
                            _padding: 0,
                        },
                        AnimatedLightChunk {
                            aabb_min: [0.5, 0.0, 0.0],
                            face_index: 0,
                            aabb_max: [1.0, 0.0, 1.0],
                            index_offset: 1,
                            uv_min: [u_mid_hi, v0],
                            uv_max: [u1, v1],
                            index_count: 1,
                            _padding: 0,
                        },
                    ],
                    light_indices: vec![0, 0],
                }
            },
        );
        assert!(section.is_consistent());
        let mut running = 0u32;
        for chunk in &section.chunk_rects {
            assert_eq!(chunk.texel_offset, running);
            running += chunk.width * chunk.height;
        }
        assert_eq!(section.offset_counts.len() as u32, running);
    }

    #[test]
    fn byte_size_under_8_mib_budget() {
        let section = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        let bytes = section.to_bytes();
        assert!(
            bytes.len() < 8 * 1024 * 1024,
            "section exceeded 8 MiB budget ({} bytes)",
            bytes.len(),
        );
    }

    #[test]
    fn mean_lights_per_covered_texel_under_2_5() {
        let section = bake_with_geometry_and_chunks(
            unit_floor_geometry(),
            vec![animated_point_light_above()],
            |charts| full_face_chunk(charts, 0, vec![0]),
        );
        let covered: usize = section.offset_counts.iter().filter(|e| e.count > 0).count();
        assert!(covered > 0, "expected at least one covered texel");
        let mean = section.texel_lights.len() as f64 / covered as f64;
        assert!(
            mean <= 2.5,
            "mean lights per covered texel {mean} exceeded 2.5 target",
        );
    }

    /// Task 2b: a static-geometry animated light's per-texel incoming
    /// direction is bakeable (it never changes). This anchors that the
    /// weight-map baker retains what `light_contribution_and_direction`
    /// computes rather than discarding it.
    #[test]
    fn weight_map_retains_per_light_per_texel_incoming_direction() {
        let light = animated_point_light_above();
        let section =
            bake_with_geometry_and_chunks(unit_floor_geometry(), vec![light.clone()], |charts| {
                full_face_chunk(charts, 0, vec![0])
            });

        assert!(
            !section.texel_lights.is_empty(),
            "expected at least one covered texel — fixture is broken otherwise",
        );

        // Every covered entry must decode to a roughly-unit vector pointing
        // upward (the light sits above a floor with normal +Y).
        for tl in &section.texel_lights {
            let decoded = postretro_level_format::octahedral::decode(tl.direction_oct);
            let len = (decoded[0] * decoded[0] + decoded[1] * decoded[1] + decoded[2] * decoded[2])
                .sqrt();
            assert!(
                (len - 1.0).abs() < 1.0e-3,
                "decoded direction {:?} not unit-length (len={})",
                decoded,
                len,
            );
            assert!(
                decoded[1] > 0.0,
                "expected +Y dominant direction (light above floor), got {:?}",
                decoded,
            );
        }
    }

    /// Anchors the cache-bump contract: bumping `STAGE_VERSION` invalidates
    /// the prior `animated_lm_weight_maps` cache entry. Mirrors
    /// `sh_volume_stage_version_bump_misses_then_hits` in `main.rs`.
    #[test]
    fn stage_version_bump_changes_cache_key() {
        use crate::cache::CacheKey;

        let input_hash = b"fake-animated-weight-maps-input-fingerprint";
        let stale = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION - 1, input_hash);
        let current = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION, input_hash);

        assert_ne!(
            stale.as_filename(),
            current.as_filename(),
            "a STAGE_VERSION bump must change the cache key — a stale entry \
             must not be reachable under the bumped version",
        );
    }

    #[test]
    fn stage_version_bump_misses_then_hits() {
        use crate::cache::{CacheKey, StageCache};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        // Unique temp dir so parallel test runs don't collide.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "postretro_anim_lm_stage_bump_{stamp}_{nonce}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = StageCache::new(&dir).expect("create cache dir");

        let input_hash = b"anim-lm-weight-maps-input-fingerprint";

        // A pre-bump entry baked by the previous version (no direction field).
        let stale_key = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION - 1, input_hash);
        cache.put(&stale_key, b"old-baked-weight-maps-without-direction");

        // Same inputs, current version: the version is folded into the key,
        // so the stale entry must not be reachable — first build is a miss.
        let current_key = CacheKey::new("animated_lm_weight_maps", STAGE_VERSION, input_hash);
        assert!(
            cache.get(&current_key).is_none(),
            "first build after STAGE_VERSION bump must miss and rebake",
        );

        // The rebake stores the direction-bearing section; the second build hits.
        let rebaked = b"weight-maps-with-direction".to_vec();
        cache.put(&current_key, &rebaked);
        let loaded = cache
            .get(&current_key)
            .expect("second build under the bumped version must hit the cache");
        assert_eq!(loaded, rebaked);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: with outward rounding (floor min, ceil max), two sibling
    /// chunks sharing a UV boundary inflated outward into the same atlas texel
    /// column/row, tripping `assert_no_overlapping_rects_per_face`. Center-based
    /// half-open ownership packs them adjacent with no overlap.
    #[test]
    fn sibling_chunks_with_shared_uv_edge_pack_without_overlap() {
        let chart = Chart {
            origin: glam::Vec3::ZERO,
            u_axis: glam::Vec3::X,
            v_axis: glam::Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: glam::Vec3::Y,
            width_texels: 8,
            height_texels: 8,
        };
        let placement = ChartPlacement { x: 0, y: 0 };
        let atlas_size = 64u32;

        // Splits the chart's U range at uv=0.5 — what `recurse` does on a
        // U-split. Pre-fix, both rects shared the same atlas texel column.
        let (ax_a, _ay_a, w_a, _h_a) = chunk_atlas_rect(
            &chart,
            placement,
            [0.0, 0.0],
            [0.5, 1.0],
            atlas_size,
            atlas_size,
        );
        let (ax_b, _ay_b, _w_b, _h_b) = chunk_atlas_rect(
            &chart,
            placement,
            [0.5, 0.0],
            [1.0, 1.0],
            atlas_size,
            atlas_size,
        );
        assert!(
            ax_a + w_a <= ax_b,
            "sibling chunks must not overlap: A ends at {} but B starts at {}",
            ax_a + w_a,
            ax_b,
        );

        // Non-integer-texel boundary at uv=0.4 (i.e. fx = 1.6, on a 4-texel
        // interior) — the original failure mode where outward rounding put A's
        // ax_max=2 and B's ax_min=1 into the same column.
        let (ax_a2, _, w_a2, _) = chunk_atlas_rect(
            &chart,
            placement,
            [0.0, 0.0],
            [0.4, 1.0],
            atlas_size,
            atlas_size,
        );
        let (ax_b2, _, _w_b2, _) = chunk_atlas_rect(
            &chart,
            placement,
            [0.4, 0.0],
            [1.0, 1.0],
            atlas_size,
            atlas_size,
        );
        assert!(
            ax_a2 + w_a2 <= ax_b2,
            "sibling chunks split at uv=0.4 must not overlap: A ends at {} but B starts at {}",
            ax_a2 + w_a2,
            ax_b2,
        );
    }

    /// Sibling chunks share a UV boundary that can drift by ~1e-7 in f32 due to
    /// recursive halving of a non-dyadic chart extent. Without the boundary
    /// snap in `chunk_atlas_rect`, the two sides straddle an integer in
    /// interior-texel space, `ceil` disagrees by one, and the atlas rects
    /// overlap by a single texel row.
    #[test]
    fn sibling_chunks_with_drifted_shared_uv_edge_pack_without_overlap() {
        // Chart geometry chosen so V has a fine interior pitch (~0.0254 m),
        // which is where the drift surfaces in practice.
        let chart = Chart {
            origin: glam::Vec3::ZERO,
            u_axis: glam::Vec3::X,
            v_axis: glam::Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [0.0508, 8.128],
            normal: glam::Vec3::Y,
            width_texels: 3,
            height_texels: 322,
        };
        let placement = ChartPlacement { x: 0, y: 0 };
        let atlas_size = 4096u32;

        // A.uv_max and B.uv_min are intended-equal but drift apart by ~2e-7;
        // this is the pattern recursive halving produces at sibling seams.
        let a_uv_min = [0.0, 2.3876];
        let a_uv_max = [0.0508, 2.4384];
        let b_uv_min = [0.0, 2.4383998];
        let b_uv_max = [0.0508, 2.4891999];

        let (ax_a, ay_a, w_a, h_a) = chunk_atlas_rect(
            &chart, placement, a_uv_min, a_uv_max, atlas_size, atlas_size,
        );
        let (ax_b, ay_b, w_b, h_b) = chunk_atlas_rect(
            &chart, placement, b_uv_min, b_uv_max, atlas_size, atlas_size,
        );

        let overlap_x = ax_a < ax_b + w_b && ax_b < ax_a + w_a;
        let overlap_y = ay_a < ay_b + h_b && ay_b < ay_a + h_a;
        assert!(
            !(overlap_x && overlap_y),
            "drifted-boundary siblings must not overlap: A={ax_a}+{ay_a}+{w_a}x{h_a} \
             vs B={ax_b}+{ay_b}+{w_b}x{h_b}",
        );
    }

    /// Regression: previously only ax_max/ay_max were clamped; a misplaced chart
    /// could put ax_min >= atlas_width, underflowing the width, and textureStore
    /// wrote past the atlas edge. Both corners are now clamped.
    #[test]
    fn chunk_atlas_rect_handles_placement_at_and_beyond_atlas_bound() {
        let chart = Chart {
            origin: glam::Vec3::ZERO,
            u_axis: glam::Vec3::X,
            v_axis: glam::Vec3::Z,
            uv_min: [0.0, 0.0],
            uv_extent: [1.0, 1.0],
            normal: glam::Vec3::Y,
            width_texels: 8,
            height_texels: 8,
        };
        let atlas_size = 64u32;

        // Case 1: placement exactly at the atlas edge.
        let placement = ChartPlacement {
            x: atlas_size - 1,
            y: atlas_size - 1,
        };
        let (ax, ay, w, h) = chunk_atlas_rect(
            &chart,
            placement,
            [0.0, 0.0],
            [1.0, 1.0],
            atlas_size,
            atlas_size,
        );
        assert!(ax < atlas_size, "ax_min {ax} overflowed atlas {atlas_size}");
        assert!(ay < atlas_size, "ay_min {ay} overflowed atlas {atlas_size}");
        assert!(w >= 1 && h >= 1);
        assert!(
            ax + w <= atlas_size,
            "rect ends at {}; atlas size {atlas_size}",
            ax + w
        );
        assert!(
            ay + h <= atlas_size,
            "rect ends at {}; atlas size {atlas_size}",
            ay + h
        );

        // Case 2: placement past the atlas bound; both corners must be pinned inside.
        let placement_past = ChartPlacement {
            x: atlas_size + 100,
            y: atlas_size + 100,
        };
        let (ax2, ay2, w2, h2) = chunk_atlas_rect(
            &chart,
            placement_past,
            [0.0, 0.0],
            [1.0, 1.0],
            atlas_size,
            atlas_size,
        );
        assert!(
            ax2 < atlas_size,
            "ax_min {ax2} must be clamped inside atlas (was past-bound)",
        );
        assert!(
            ay2 < atlas_size,
            "ay_min {ay2} must be clamped inside atlas (was past-bound)",
        );
        assert!(ax2 + w2 <= atlas_size);
        assert!(ay2 + h2 <= atlas_size);
    }
}
