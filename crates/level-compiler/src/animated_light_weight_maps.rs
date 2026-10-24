// Animated-light weight-map baker.
// See: context/lib/build_pipeline.md §Build Cache

use std::time::Duration;

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
use crate::chart_raster::ChartPlacement;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{BlockLayout, Chart, CutParent};
use crate::map_data::MapLight;
use crate::texel_footprint::{EmbeddedEmitters, ProbeEarlyOut};
use crate::texel_step_detector::{CutSites, TexelStepReport, merge_chart_steps};

mod face_bake;
mod static_atlas_frame;

#[cfg(test)]
use static_atlas_frame::layer_rects_may_overlap;
#[cfg(test)]
pub(crate) use static_atlas_frame::rebase_to_cell_block;
use static_atlas_frame::{assert_no_overlapping_rects_per_layer, static_frame_blocks};

/// Dropped as numerical noise; prevents per-texel lists from inflating on
/// contributions too dim to matter.
pub(crate) const WEIGHT_EPSILON: f32 = 1.0e-6;

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
/// v9: soft-visibility seeds key on the chart's frame and texel
/// (`chart_raster::chart_texel_seed`), not bake-layer coordinates.
///
/// v10: visibility is filtered over the static bake's receiver footprint
/// unless the penumbra floor is off, and the entry stores the texel-step
/// report ahead of the section.
///
/// v11: clear footprint rays test their skipped lead-in, clip segments are
/// tested from the texel centre itself, and the probe ring sits at a fixed
/// 0.6 texel at every count; the detector's surface test stretches 1 mm past
/// both centres and a cut pair is owned even beside a missed cut piece.
///
/// v12: a light whose emitter sphere reaches into geometry
/// (`texel_footprint::embedded_emitter_fraction`) traces every footprint
/// sample, with no probe early-out.
///
/// v13: such a light is flagged only when at least
/// `EMBEDDED_EMITTER_MIN_DIRECTIONS` of the test's directions are embedded,
/// and a flagged light keeps the all-blocked probe early-out, skipping only
/// the all-clear one.
///
/// Bump when the baked weights, the section or entry layout, or the
/// texel-step detector's verdicts change: the entry stores the detector's
/// report, and only its threshold is keyed, so a verdict change alone would
/// replay stale warnings.
///
/// Pipeline orchestration caches this bake under the `animated_lm_weight_maps`
/// key, which folds this `STAGE_VERSION` in alongside the input hash — the same
/// per-stage version-constant pattern every cached stage uses. Bumping this
/// constant invalidates every prior cache entry for the stage on the next
/// build. The `CacheKey`/STAGE_VERSION contract is exercised by
/// `stage_version_bump_misses_then_hits` and `stage_version_bump_changes_cache_key`
/// in this module's test suite.
pub const STAGE_VERSION: u32 = 13;

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
    /// Cut faces' parents, so the texel-step detector checks pairs across a
    /// cut once and names the face the author drew.
    pub cut_parents: &'a [CutParent],
    /// Filter visibility over the receiver footprint (`--no-penumbra-floor`
    /// turns it off); keyed.
    pub penumbra_floor: bool,
    /// Largest stored visibility difference between axis-adjacent texels
    /// that is no step: `TEXEL_STEP_THRESHOLD` outside tests; keyed.
    pub step_threshold: f32,
}

/// The weight maps and what the texel-step detector found while baking them.
pub(crate) struct WeightMapBake {
    pub(crate) section: AnimatedLightWeightMapsSection,
    /// Every stepped (animated light, face), not yet logged.
    pub(crate) texel_steps: TexelStepReport,
    /// Detector time summed over workers. Never keyed.
    pub(crate) texel_step_elapsed: Duration,
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
/// layout, and check each (light, face) for texel steps. The atlas budget is
/// not checked here: it depends on the page count the compact repack reaches
/// after [`cull_unlit_chunks`]. Rejects an animated face whose chart leaves
/// its lightmap cell block.
#[cfg(test)]
pub(crate) fn bake_animated_light_weight_maps(
    inputs: &WeightMapInputs<'_>,
) -> Result<WeightMapBake, AnimatedWeightMapBakeError> {
    bake_animated_light_weight_maps_controlled(inputs, &BakeControl::unrestricted())
}

#[cfg(test)]
pub(crate) fn bake_animated_light_weight_maps_controlled(
    inputs: &WeightMapInputs<'_>,
    control: &BakeControl,
) -> Result<WeightMapBake, AnimatedWeightMapBakeError> {
    bake_animated_light_weight_maps_with(inputs, &embedded_emitters(inputs, control), control)
}

/// The weight-map lights' emitter tests, keyed by position in
/// `inputs.lights`, with the floor on; with it off nothing reads the flags.
/// The test reads only the lights and geometry, which the
/// `animated_lm_weight_maps` key folds whole, so the flags need no key.
pub(crate) fn embedded_emitters(
    inputs: &WeightMapInputs<'_>,
    control: &BakeControl,
) -> EmbeddedEmitters {
    if !inputs.penumbra_floor {
        return EmbeddedEmitters::default();
    }
    EmbeddedEmitters::classify(
        inputs.lights.iter().enumerate(),
        inputs.bvh,
        inputs.primitives,
        inputs.geometry,
        control,
    )
}

/// `bake_animated_light_weight_maps_controlled` with the lights' emitter
/// flags already tested ([`embedded_emitters`]).
pub(crate) fn bake_animated_light_weight_maps_with(
    inputs: &WeightMapInputs<'_>,
    embedded_emitters: &EmbeddedEmitters,
    control: &BakeControl,
) -> Result<WeightMapBake, AnimatedWeightMapBakeError> {
    bake_weight_maps_on(
        inputs,
        |light| ProbeEarlyOut::for_light(embedded_emitters.is_embedded(light)),
        control,
    )
}

/// The weight maps floored with the probe early-out off
/// ([`ProbeEarlyOut::Off`]) for every light: the full-set reference a
/// measurement holds the floored bake against, as
/// [`bake_light_layer_full_sets`](crate::lightmap_layer::bake_light_layer_full_sets)
/// is for the static layers.
#[cfg(test)]
pub(crate) fn bake_animated_light_weight_maps_full_sets(
    inputs: &WeightMapInputs<'_>,
    control: &BakeControl,
) -> Result<WeightMapBake, AnimatedWeightMapBakeError> {
    assert!(inputs.penumbra_floor, "the full set is a floored bake");
    bake_weight_maps_on(inputs, |_| ProbeEarlyOut::Off, control)
}

/// The bake, with each light's footprint early-out (by position in
/// `inputs.lights`) chosen by `early_out`; the floor-off sampler ignores it.
fn bake_weight_maps_on(
    inputs: &WeightMapInputs<'_>,
    early_out: impl Fn(usize) -> ProbeEarlyOut + Sync,
    control: &BakeControl,
) -> Result<WeightMapBake, AnimatedWeightMapBakeError> {
    if inputs.chunk_section.chunks.is_empty() {
        return Ok(WeightMapBake {
            section: AnimatedLightWeightMapsSection::empty(),
            texel_steps: TexelStepReport::default(),
            texel_step_elapsed: Duration::ZERO,
        });
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

    // A face's chunks in section order: a light's visibility grid spans
    // every chunk of the face that lists it, so pairs across a chunk split
    // are checked.
    let mut by_face: Vec<usize> = (0..chunks.len()).collect();
    by_face.sort_by_key(|&index| chunks[index].face_index);
    let faces: Vec<&[usize]> = by_face
        .chunk_by(|&a, &b| chunks[a].face_index == chunks[b].face_index)
        .collect();
    let sites = CutSites::new(inputs.cut_parents, inputs.face_charts);
    // One governed item per face for its light-independent setup, whose clip
    // pre-pass every light on the face then reads.
    let setups: Vec<face_bake::FaceSetup<'_>> = faces
        .par_iter()
        .map(|chunk_indices| {
            let _permit = control.governor().enter();
            face_bake::FaceSetup::new(inputs, chunk_indices)
        })
        .collect();
    // Then one governed item per (face, light). A face lists more lights
    // than a chunk holds only by splitting into many chunks, so the faces
    // with the most lights cost the most; split by light, they spread over
    // every worker instead of serializing on one. Each light's grid and
    // entries depend on nothing else on the face.
    let tasks: Vec<(usize, u32)> = setups
        .iter()
        .enumerate()
        .flat_map(|(face, setup)| setup.lights.iter().map(move |&light| (face, light)))
        .collect();
    control.publish_total(tasks.len());
    let mut light_bakes = tasks
        .par_iter()
        .map(|&(face, light)| {
            let _permit = control.governor().enter();
            let baked = setups[face].bake_light(inputs, &sites, light, early_out(light as usize));
            control.advance(1);
            baked
        })
        .collect::<Vec<_>>()
        .into_iter();

    // Merged in (face, light) order, so section bytes and the report never
    // depend on which worker baked what.
    let mut per_chunk: Vec<Option<ChunkBakeResult>> = (0..chunks.len()).map(|_| None).collect();
    let mut found = Vec::new();
    let mut texel_step_elapsed = Duration::ZERO;
    for (chunk_indices, setup) in faces.iter().zip(setups) {
        let face_bakes: Vec<face_bake::LightBake> =
            light_bakes.by_ref().take(setup.lights.len()).collect();
        for (&light, bake) in setup.lights.iter().zip(&face_bakes) {
            if let Some(steps) = bake.steps {
                found.push((light as usize, steps));
            }
            texel_step_elapsed += bake.detector;
        }
        for (&index, result) in chunk_indices.iter().zip(setup.into_chunks(face_bakes)) {
            per_chunk[index] = Some(result);
        }
    }
    let per_chunk: Vec<ChunkBakeResult> = per_chunk
        .into_iter()
        .map(|result| result.expect("every chunk belongs to one face"))
        .collect();
    let started = std::time::Instant::now();
    let texel_steps = merge_chart_steps(found, &sites);
    texel_step_elapsed += started.elapsed();

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

    Ok(WeightMapBake {
        section,
        texel_steps,
        texel_step_elapsed,
    })
}

/// Strips base color and intensity so the weight is a neutral Lambert × falloff
/// × cone scalar; runtime compose re-applies color/intensity from the descriptor.
/// Picks the dominant color channel to avoid divide-by-near-zero on weak channels.
pub(crate) fn contribution_to_weight(contribution: Vec3, color: [f32; 3], intensity: f32) -> f32 {
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
