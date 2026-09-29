// Animated-light weight-map baker.
// See: context/lib/build_pipeline.md §Build Cache

use bvh::bvh::Bvh;
use glam::Vec3;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use postretro_level_format::animated_lightmap_atlas::{
    ANIMATED_ATLAS_VRAM_BUDGET_BYTES, animated_atlas_byte_estimate, animated_atlas_fits_budget,
};
use rayon::prelude::*;
use thiserror::Error;

use crate::animated_atlas_layout::apply_identity_layout;
use crate::bake_control::BakeControl;

use crate::bvh_build::BvhPrimitive;
use crate::chart_raster::{
    CHART_PADDING_TEXELS, ChartPlacement, chart_interior_dims, chart_texel_world_position,
};
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{
    BlockLayout, Chart, light_contribution_and_direction, segment_clear, soft_visibility,
};
use crate::map_data::MapLight;

mod static_atlas_frame;

#[cfg(test)]
pub(crate) use static_atlas_frame::rebase_to_cell_block;
use static_atlas_frame::{
    assert_no_overlapping_rects_per_layer, chunk_atlas_rect, static_frame_blocks,
};

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
/// v6 adds the static-atlas-layer slot table and bakes animated weights for
/// every packed static layer, replacing the former layer-0 skip sentinel.
///
/// v7 caches section 25 v4 in the identity layout: one block per animated
/// face, chunk rects in compact coordinates equal to their static ones.
///
/// v8 caches section 25 v5: blocks keyed by lightmap cell block and
/// block-local texels, identity pages the internal bake layers.
///
/// Pipeline orchestration caches this bake under the `animated_lm_weight_maps`
/// key, which folds this `STAGE_VERSION` in alongside the input hash — the same
/// per-stage version-constant pattern every cached stage uses. Bumping this
/// constant invalidates every prior cache entry for the stage on the next
/// build. The `CacheKey`/STAGE_VERSION contract is exercised by
/// `stage_version_bump_misses_then_hits` and `stage_version_bump_changes_cache_key`
/// in this module's test suite.
pub const STAGE_VERSION: u32 = 8;

pub struct WeightMapInputs<'a> {
    pub bvh: &'a Bvh<f32, 3>,
    pub primitives: &'a [BvhPrimitive],
    pub geometry: &'a GeometryResult,
    pub chunk_section: &'a AnimatedLightChunksSection,
    /// Filtered `!is_dynamic && animation.is_some()` — same filter as
    /// `sh_bake.rs` for `animation_descriptors`, so indices agree without remap.
    pub lights: &'a [MapLight],
    pub face_charts: &'a [Chart],
    /// Bake-layer placements, parallel to `face_charts`.
    pub face_placements: &'a [ChartPlacement],
    /// Cell blocks the placements sit in; animated blocks are keyed in their
    /// texels.
    pub layout: &'a BlockLayout,
    /// Bake layer size; layers are square, so this is also the identity
    /// layout's page size.
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Bake layers, for the bake log.
    pub static_atlas_layer_count: u32,
    /// Area-sample count for soft-shadow penumbra visibility.
    /// `pipeline.rs` folds this value into the `animated_lm_weight_maps` cache
    /// key's input hash, so changing it produces a cache miss and full re-bake.
    /// Defaults to `lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT`.
    pub area_sample_count: u32,
}

/// Failure from the animated-light weight-map bake.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnimatedWeightMapBakeError {
    #[error(
        "animated lightmap atlas is over budget: budget {budget_bytes} bytes, found {found_bytes} bytes"
    )]
    AtlasOverBudget { budget_bytes: u64, found_bytes: u64 },
    #[error(
        "face {face}'s animated rect (layer, x, y, w, h) {rect:?} leaves its lightmap cell \
         block {lightmap_block} at {cell_block:?}; its block-local key would sample another block"
    )]
    BlockOutsideCellBlock {
        face: usize,
        lightmap_block: u32,
        rect: [u32; 5],
        cell_block: [u32; 5],
    },
}

/// Reject an animated atlas whose combined irradiance and direction targets do
/// not fit the production VRAM budget. Checked after the compact repack on
/// both cache paths, so the page count is the one the renderer will allocate.
pub(crate) fn validate_animated_atlas_budget(
    page_size: u32,
    page_count: u32,
) -> Result<(), AnimatedWeightMapBakeError> {
    validate_animated_atlas_budget_with_limit(
        page_size,
        page_count,
        ANIMATED_ATLAS_VRAM_BUDGET_BYTES,
    )
}

fn validate_animated_atlas_budget_with_limit(
    page_size: u32,
    page_count: u32,
    atlas_budget_bytes: u64,
) -> Result<(), AnimatedWeightMapBakeError> {
    let found_bytes = animated_atlas_byte_estimate(page_size, page_size, page_count);
    if animated_atlas_fits_budget(page_size, page_size, page_count, atlas_budget_bytes) {
        Ok(())
    } else {
        Err(AnimatedWeightMapBakeError::AtlasOverBudget {
            budget_bytes: atlas_budget_bytes,
            found_bytes,
        })
    }
}

struct ChunkBakeResult {
    /// Bake-layer rect; `block` and `texel_offset` are filled by concatenation.
    rect: ChunkAtlasRect,
    /// Bake layer of `rect`.
    layer: u32,
    /// chunk-local offsets; concatenation pass rewrites to global offsets.
    offset_counts: Vec<TexelLightEntry>,
    texel_lights: Vec<TexelLight>,
}

/// Chunk, weight-map, and BVH-leaf chunk-range tables after
/// [`cull_unlit_chunks`]. The three stay mutually consistent: rect `i` pairs
/// with chunk `i`, and every leaf range indexes the surviving chunk array.
#[derive(Debug, Clone, PartialEq)]
pub struct CulledAnimatedChunks {
    pub chunk_section: AnimatedLightChunksSection,
    pub weight_maps: AnimatedLightWeightMapsSection,
    pub leaf_chunk_ranges: Vec<(u32, u32)>,
}

/// Drop every chunk whose baked weight map has no lit texel.
///
/// The chunk builder selects receivers by influence-sphere overlap alone — no
/// occlusion, facing, or cone test — so on maps with long-range animated lights
/// most chunks sit behind walls or face away and bake all-zero weights. Kept,
/// they still cost atlas texels and compose dispatch tiles every frame;
/// dropped, their texels are never written and stay at the atlas's zero
/// initialization, which is exactly what compose would have written.
///
/// A block whose every chunk drops is dropped too, and the survivors are laid
/// out again in the identity layout over the bake layers still holding a
/// chunk; the compact repack runs afterwards. Runs outside the weight-map
/// cache so hit and miss paths cull identically. Unlit chunks own no
/// `texel_lights` entries, so that pool and every surviving
/// `offset_counts.offset` carry over unchanged; only rect texel offsets and
/// block indices, the chunk light-index pool, and leaf ranges are rebased.
pub fn cull_unlit_chunks(
    chunk_section: &AnimatedLightChunksSection,
    weight_maps: AnimatedLightWeightMapsSection,
    leaf_chunk_ranges: &[(u32, u32)],
    layout: &BlockLayout,
    bake_layer_size: u32,
) -> CulledAnimatedChunks {
    assert_eq!(
        chunk_section.chunks.len(),
        weight_maps.chunk_rects.len(),
        "weight-map rects must pair 1:1 with animated chunks",
    );

    let rect_texels = |rect: &ChunkAtlasRect| {
        let start = rect.texel_offset as usize;
        start..start + (rect.width * rect.height) as usize
    };
    let lit: Vec<bool> = weight_maps
        .chunk_rects
        .iter()
        .map(|rect| {
            weight_maps.offset_counts[rect_texels(rect)]
                .iter()
                .any(|entry| entry.count > 0)
        })
        .collect();

    // New index of each block that keeps at least one lit chunk.
    let mut block_kept = vec![false; weight_maps.blocks.len()];
    for (rect, &is_lit) in weight_maps.chunk_rects.iter().zip(&lit) {
        if is_lit {
            block_kept[rect.block as usize] = true;
        }
    }
    let mut block_remap = vec![u32::MAX; weight_maps.blocks.len()];
    let mut blocks = Vec::new();
    for (index, block) in weight_maps.blocks.iter().enumerate() {
        if block_kept[index] {
            block_remap[index] = blocks.len() as u32;
            blocks.push(*block);
        }
    }

    let mut chunks = Vec::new();
    let mut light_indices = Vec::new();
    let mut chunk_rects = Vec::new();
    let mut offset_counts = Vec::new();
    let mut running_texel_offset: u32 = 0;
    let mut dropped_texels: u64 = 0;
    for ((chunk, rect), &is_lit) in chunk_section
        .chunks
        .iter()
        .zip(&weight_maps.chunk_rects)
        .zip(&lit)
    {
        if !is_lit {
            dropped_texels += u64::from(rect.width * rect.height);
            continue;
        }
        let pool_start = chunk.index_offset as usize;
        let pool_end = pool_start + chunk.index_count as usize;
        chunks.push(
            postretro_level_format::animated_light_chunks::AnimatedLightChunk {
                index_offset: light_indices.len() as u32,
                ..*chunk
            },
        );
        light_indices.extend_from_slice(&chunk_section.light_indices[pool_start..pool_end]);

        offset_counts.extend_from_slice(&weight_maps.offset_counts[rect_texels(rect)]);
        chunk_rects.push(ChunkAtlasRect {
            texel_offset: running_texel_offset,
            block: block_remap[rect.block as usize],
            ..*rect
        });
        running_texel_offset += rect.width * rect.height;
    }

    // `kept_before[i]` = surviving chunks among the first `i` originals. Leaves
    // own contiguous chunk ranges, so each range maps to a contiguous prefix
    // difference. Empty ranges keep their position, as the builder emits them.
    let mut kept_before = Vec::with_capacity(lit.len() + 1);
    kept_before.push(0u32);
    for &is_lit in &lit {
        kept_before.push(kept_before[kept_before.len() - 1] + u32::from(is_lit));
    }
    let leaf_chunk_ranges = leaf_chunk_ranges
        .iter()
        .map(|&(start, count)| {
            let start = start as usize;
            let end = start + count as usize;
            (kept_before[start], kept_before[end] - kept_before[start])
        })
        .collect();

    let pages_before = weight_maps.compact_layers;
    let mut culled = AnimatedLightWeightMapsSection {
        page_size: weight_maps.page_size,
        compact_layers: weight_maps.compact_layers,
        blocks,
        chunk_rects,
        offset_counts,
        texel_lights: weight_maps.texel_lights,
    };
    if culled.chunk_rects.is_empty() {
        culled = AnimatedLightWeightMapsSection {
            texel_lights: culled.texel_lights,
            ..AnimatedLightWeightMapsSection::empty()
        };
    } else {
        apply_identity_layout(&mut culled, layout, bake_layer_size);
    }

    let dropped = lit.len() - chunks.len();
    if dropped > 0 {
        log::info!(
            "[AnimatedLightWeightMaps] culled {dropped} of {} chunks with no lit texel \
             ({dropped_texels} texels); animated blocks {} -> {}, identity pages {} -> {}",
            lit.len(),
            weight_maps.blocks.len(),
            culled.blocks.len(),
            pages_before,
            culled.compact_layers,
        );
    }

    CulledAnimatedChunks {
        chunk_section: AnimatedLightChunksSection {
            chunks,
            light_indices,
        },
        weight_maps: culled,
        leaf_chunk_ranges,
    }
}

/// Bake per-texel animated-light weights for every chunk, in the identity
/// layout. The atlas budget is not checked here: it depends on the page count
/// the compact repack reaches after [`cull_unlit_chunks`]. Rejects an animated
/// face whose chart leaves its lightmap cell block.
pub fn bake_animated_light_weight_maps(
    inputs: &WeightMapInputs<'_>,
) -> Result<AnimatedLightWeightMapsSection, AnimatedWeightMapBakeError> {
    bake_animated_light_weight_maps_controlled(inputs, &BakeControl::unrestricted())
}

pub fn bake_animated_light_weight_maps_controlled(
    inputs: &WeightMapInputs<'_>,
    control: &BakeControl,
) -> Result<AnimatedLightWeightMapsSection, AnimatedWeightMapBakeError> {
    if inputs.chunk_section.chunks.is_empty() {
        return Ok(AnimatedLightWeightMapsSection::empty());
    }

    let chunks = &inputs.chunk_section.chunks;
    assert_eq!(
        inputs.atlas_width, inputs.atlas_height,
        "bake layers are square",
    );
    let (block_faces, blocks) = static_frame_blocks(
        chunks,
        inputs.face_charts,
        inputs.face_placements,
        inputs.layout,
    )?;

    control.publish_total(chunks.len());
    let light_indices_pool = &inputs.chunk_section.light_indices;

    let per_chunk: Vec<ChunkBakeResult> = chunks
        .par_iter()
        .map(|chunk| {
            let _permit = control.governor().enter();
            let result = bake_one_chunk(inputs, chunk, light_indices_pool);
            control.advance(1);
            result
        })
        .collect();

    assert_no_overlapping_rects_per_layer(chunks, &per_chunk);

    let mut chunk_rects: Vec<ChunkAtlasRect> = Vec::with_capacity(per_chunk.len());
    let mut offset_counts: Vec<TexelLightEntry> = Vec::new();
    let mut texel_lights: Vec<TexelLight> = Vec::new();

    let mut running_texel_offset: u32 = 0;
    for (chunk, result) in chunks.iter().zip(per_chunk) {
        let ChunkBakeResult {
            mut rect,
            layer: _,
            offset_counts: chunk_oc,
            texel_lights: chunk_tl,
        } = result;

        rect.block = block_faces
            .binary_search(&chunk.face_index)
            .expect("every chunk face owns a block") as u32;
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

    let mut section = AnimatedLightWeightMapsSection {
        page_size: 0,
        compact_layers: 0,
        blocks,
        chunk_rects,
        offset_counts,
        texel_lights,
    };
    // Chunk rects hold bake-layer coordinates and every block sits at its
    // bake-layer placement, so this only assigns pages: the identity layout,
    // which the stage cache stores.
    apply_identity_layout(&mut section, inputs.layout, inputs.atlas_width);

    let covered_texels: u32 = section.offset_counts.iter().filter(|e| e.count > 0).count() as u32;
    let mean_lights_per_covered = if covered_texels == 0 {
        0.0
    } else {
        section.texel_lights.len() as f64 / covered_texels as f64
    };
    let peak_texels_per_chunk = section
        .chunk_rects
        .iter()
        .map(|r| r.width * r.height)
        .max()
        .unwrap_or(0);

    log::info!(
        "[AnimatedLightWeightMaps] {} bake layers, {} animated blocks on {} identity \
         pages, {} chunks, {} byte section, {} covered texels, \
         mean {:.2} lights / covered texel, peak {} texels / chunk",
        inputs.static_atlas_layer_count,
        section.blocks.len(),
        section.compact_layers,
        section.chunk_rects.len(),
        section.byte_len(),
        covered_texels,
        mean_lights_per_covered,
        peak_texels_per_chunk,
    );

    Ok(section)
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
        compact_x: atlas_x,
        compact_y: atlas_y,
        width,
        height,
        texel_offset: 0, // filled by caller
        block: 0,        // filled by caller
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
        layer: placement.layer,
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

#[cfg(test)]
mod tests;
