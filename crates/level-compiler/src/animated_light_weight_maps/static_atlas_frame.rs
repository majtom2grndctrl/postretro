// Animated blocks keyed in cell-block texels; chunk rects placed in bake-layer coordinates.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::animated_light_chunks::AnimatedLightChunk;
use postretro_level_format::animated_light_weight_maps::AnimatedBlock;

use super::{AnimatedWeightMapBakeError, ChunkBakeResult};
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement, chart_interior_dims};
use crate::lightmap_bake::{BlockLayout, Chart};

/// One block per face with a chunk: its whole chart placement, padding
/// included, so the zero gutter travels with it. Blocks are indexed in
/// ascending face order, which is cell order (faces are emitted grouped
/// by cell), the order the compact repack packs them in. `AnimatedLightChunk`s
/// keep their original order for baking and serialization.
///
/// Each block is keyed by its face's lightmap cell block and the chart's
/// block-local origin; its compact position starts at the chart's bake-layer
/// placement (the identity layout).
///
/// Returns the sorted block faces (a chunk's block index is its face's position
/// here) and the blocks themselves.
pub(super) fn static_frame_blocks(
    chunks: &[AnimatedLightChunk],
    face_charts: &[Chart],
    face_placements: &[ChartPlacement],
    layout: &BlockLayout,
) -> Result<(Vec<u32>, Vec<AnimatedBlock>), AnimatedWeightMapBakeError> {
    let mut block_faces: Vec<u32> = chunks.iter().map(|chunk| chunk.face_index).collect();
    block_faces.sort_unstable();
    block_faces.dedup();
    let blocks = block_faces
        .iter()
        .map(|&face| {
            let face = face as usize;
            rebase_to_cell_block(
                layout,
                face,
                face_placements[face],
                face_charts[face].width_texels,
                face_charts[face].height_texels,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((block_faces, blocks))
}

/// Key face `face`'s bake-layer rect `(placement, width × height)` by its
/// lightmap cell block and block-local origin. Rejects a rect that leaves the
/// cell block: the runtime resolves the key through that block's pool slot,
/// so texels outside it would sample another block.
pub(crate) fn rebase_to_cell_block(
    layout: &BlockLayout,
    face: usize,
    placement: ChartPlacement,
    width: u32,
    height: u32,
) -> Result<AnimatedBlock, AnimatedWeightMapBakeError> {
    let lightmap_block = layout.chart_blocks[face];
    let cell_block = &layout.blocks[lightmap_block as usize];
    if !cell_block.contains(placement.layer, placement.x, placement.y, width, height) {
        return Err(AnimatedWeightMapBakeError::BlockOutsideCellBlock {
            face,
            lightmap_block,
            rect: [placement.layer, placement.x, placement.y, width, height],
            cell_block: [
                cell_block.layer,
                cell_block.x,
                cell_block.y,
                cell_block.width,
                cell_block.height,
            ],
        });
    }
    let local = |bake: u32, origin: u32| {
        u16::try_from(bake - origin).expect("a cell block fits a u16-wide pool layer")
    };
    Ok(AnimatedBlock {
        lightmap_block,
        block_x: local(placement.x, cell_block.x),
        block_y: local(placement.y, cell_block.y),
        compact_x: placement.x,
        compact_y: placement.y,
        compact_layer: 0,
        width,
        height,
    })
}

/// Center-based half-open ownership: a chart-interior texel `t` (whose center
/// projects to UV `chart.uv_min + (t + 0.5) / interior * uv_extent`) belongs to
/// a chunk iff its center UV is in `[chunk.uv_min, chunk.uv_max)`. Siblings
/// share UV boundaries exactly (A.uv_max == B.uv_min) and under this rule pack
/// into adjacent atlas rects with no overlap and no gap. The
/// `assert_no_overlapping_rects_per_layer` postcondition guards the invariant.
pub(super) fn chunk_atlas_rect(
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
    // The subdivider splits chunks on whole texels, so a chunk edge maps to
    // `t - 0.5` here, half a texel from any integer, and f32 drift in the UV
    // (which grows with distance into a large chart) cannot flip a `ceil`.
    // The snap is a second guard for chunk UVs not on texel edges: when
    // shared-edge drift straddles an integer, the two `ceil`s disagree by one
    // and adjacent atlas rects overlap by a texel row/column. Epsilon is in
    // interior-texel units. See
    // `sibling_chunks_with_drifted_shared_uv_edge_pack_without_overlap` for a
    // worked example with drifted midpoint edges.
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

/// If this fires, the UV packer assigned overlapping chart space within one
/// bake layer. Different faces on the same layer share coordinates, so
/// this must not be limited to sibling chunks of one face.
///
/// Each layer is checked by marking its rects in an occupancy bitmap, linear
/// in the texels the bake already visited; a pairwise scan was quadratic in
/// chunks per layer, hundreds of seconds on a map with ~10⁵ min-extent chunks
/// on one layer. A layer the bitmap flags goes to the pairwise scan, which
/// panics naming the same first overlapping pair it always named.
pub(super) fn assert_no_overlapping_rects_per_layer(
    chunks: &[postretro_level_format::animated_light_chunks::AnimatedLightChunk],
    per_chunk: &[ChunkBakeResult],
) {
    use std::collections::BTreeMap;

    let mut by_layer: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, result) in per_chunk.iter().enumerate() {
        by_layer.entry(result.layer).or_default().push(index);
    }
    let mut occupancy: Vec<u64> = Vec::new();
    for (layer, indices) in by_layer {
        if layer_rects_may_overlap(per_chunk, &indices, &mut occupancy) {
            panic_on_first_overlap(chunks, per_chunk, layer, &indices);
        }
    }
}

/// Whether two of the layer's rects share a texel. A rect with a zero
/// dimension answers `true` without marking: the pairwise half-open test can
/// still report it inside another rect, so the scan decides that layer.
pub(super) fn layer_rects_may_overlap(
    per_chunk: &[ChunkBakeResult],
    indices: &[usize],
    occupancy: &mut Vec<u64>,
) -> bool {
    let rects = || indices.iter().map(|&i| &per_chunk[i].rect);
    if rects().any(|r| r.width == 0 || r.height == 0) {
        return true;
    }
    // `chunk_atlas_rect` clamps rects to the bake layer, so the bitmap spans
    // at most one layer's texels.
    let width = rects()
        .map(|r| r.compact_x as usize + r.width as usize)
        .max()
        .unwrap_or(0);
    let height = rects()
        .map(|r| r.compact_y as usize + r.height as usize)
        .max()
        .unwrap_or(0);
    let words_per_row = width.div_ceil(64);
    occupancy.clear();
    occupancy.resize(words_per_row * height, 0);
    for rect in rects() {
        let x0 = rect.compact_x as usize;
        let x1 = x0 + rect.width as usize;
        let (first_word, last_word) = (x0 / 64, (x1 - 1) / 64);
        let y0 = rect.compact_y as usize;
        for row in y0..y0 + rect.height as usize {
            let row_words = &mut occupancy[row * words_per_row..(row + 1) * words_per_row];
            for (word, bits) in row_words
                .iter_mut()
                .enumerate()
                .take(last_word + 1)
                .skip(first_word)
            {
                let lo = if word == first_word { x0 % 64 } else { 0 };
                let hi = if word == last_word {
                    (x1 - 1) % 64 + 1
                } else {
                    64
                };
                let mask = (u64::MAX >> (64 - (hi - lo))) << lo;
                if *bits & mask != 0 {
                    return true;
                }
                *bits |= mask;
            }
        }
    }
    false
}

/// Pairwise scan of one layer: panics on its first overlapping pair in
/// index order, and returns when no pair overlaps.
fn panic_on_first_overlap(
    chunks: &[postretro_level_format::animated_light_chunks::AnimatedLightChunk],
    per_chunk: &[ChunkBakeResult],
    layer: u32,
    indices: &[usize],
) {
    for (i_idx, &i) in indices.iter().enumerate() {
        let a = &per_chunk[i].rect;
        for &j in &indices[i_idx + 1..] {
            let b = &per_chunk[j].rect;
            let overlap_x =
                a.compact_x < b.compact_x + b.width && b.compact_x < a.compact_x + a.width;
            let overlap_y =
                a.compact_y < b.compact_y + b.height && b.compact_y < a.compact_y + a.height;
            if overlap_x && overlap_y {
                let ca = &chunks[i];
                let cb = &chunks[j];
                panic!(
                    "animated-light chunks {i} (face {}) and {j} (face {}) on bake \
                     layer {layer} produced \
                     overlapping atlas rects under center-based half-open ownership \
                     ({}x{}+{}+{} vs {}x{}+{}+{}); chunk UVs [{:?}..{:?}] vs \
                     [{:?}..{:?}]. Likely causes: subdivider emitted truly \
                     overlapping UV ranges, or shared-boundary float drift exceeded \
                     `chunk_atlas_rect`'s BOUNDARY_SNAP_EPS.",
                    ca.face_index,
                    cb.face_index,
                    a.width,
                    a.height,
                    a.compact_x,
                    a.compact_y,
                    b.width,
                    b.height,
                    b.compact_x,
                    b.compact_y,
                    ca.uv_min,
                    ca.uv_max,
                    cb.uv_min,
                    cb.uv_max,
                );
            }
        }
    }
}
