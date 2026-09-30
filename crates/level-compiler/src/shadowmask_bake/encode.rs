// Raw RGBA shadowmask fill → per-block BC5 group planes.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::shadowmask_atlas::{SHADOWMASK_GROUP_COUNT, group_plane_len};

use crate::bc5::encode_bc5_rg_masks;
use crate::lightmap_bake::{CellBlock, copy_unit_rect};

const BC_EDGE: u32 = 4;
const BC5_BLOCK_BYTES: usize = 16;

/// Encode a layer-major raw `Rgba8Unorm` mask fill as BC5 and slice each
/// block's group A and group B planes out of its bake layer, in block order.
///
/// Per layer, R/G become group 0 in the left half of a `2W × H` image and
/// B/A become group 1 in the right half; that image is encoded once with the
/// mask encoder, which may pick BC4's 6-value mode per 4×4 block. Block
/// origins are 4-aligned, so each slice equals encoding the block alone.
/// Scratch is one `2W × H` RGBA image (two raw layers) plus the encoder's
/// returned blocks for that layer (half a raw layer), reused layer to layer.
///
/// Panics in every profile on dimensions that are not multiples of 4: the
/// encoder would otherwise drop the remainder blocks and emit a truncated
/// payload. `prepare_fused_shadowmask` rejects such atlases with an error
/// first; the test-reference bake entry points reach this panic instead.
pub(super) fn encode_blocks_bc5(
    raw: &[u8],
    width: u32,
    height: u32,
    layer_count: u32,
    blocks: &[CellBlock],
) -> Vec<[Vec<u8>; 2]> {
    assert!(
        width % 4 == 0 && height % 4 == 0,
        "shadowmask atlas {width}x{height} is not 4-aligned; BC5 would truncate it"
    );
    let plane_texels = width as usize * height as usize;
    assert_eq!(
        raw.len(),
        plane_texels * 4 * layer_count as usize,
        "shadowmask raw fill length does not match {width}x{height}x{layer_count}"
    );
    let texture_width = width * SHADOWMASK_GROUP_COUNT;

    let mut out: Vec<[Vec<u8>; 2]> = vec![[Vec::new(), Vec::new()]; blocks.len()];
    let mut image = vec![0u8; plane_texels * 4 * SHADOWMASK_GROUP_COUNT as usize];
    for (layer, raw_layer) in raw.chunks_exact(plane_texels * 4).enumerate() {
        if !blocks.iter().any(|block| block.layer == layer as u32) {
            continue;
        }
        for (row_index, raw_row) in raw_layer.chunks_exact(width as usize * 4).enumerate() {
            let image_row =
                &mut image[row_index * texture_width as usize * 4..][..texture_width as usize * 4];
            let (left, right) = image_row.split_at_mut(width as usize * 4);
            for ((texel, left), right) in raw_row
                .chunks_exact(4)
                .zip(left.chunks_exact_mut(4))
                .zip(right.chunks_exact_mut(4))
            {
                left[0] = texel[0];
                left[1] = texel[1];
                right[0] = texel[2];
                right[1] = texel[3];
            }
        }
        let encoded = encode_bc5_rg_masks(&image, texture_width, height);
        for (slot, block) in out.iter_mut().zip(blocks) {
            if block.layer != layer as u32 {
                continue;
            }
            let rect = |group: u32| {
                [
                    (group * width + block.x) / BC_EDGE,
                    block.y / BC_EDGE,
                    block.width / BC_EDGE,
                    block.height / BC_EDGE,
                ]
            };
            *slot = [
                copy_unit_rect(&encoded, texture_width / BC_EDGE, BC5_BLOCK_BYTES, rect(0)),
                copy_unit_rect(&encoded, texture_width / BC_EDGE, BC5_BLOCK_BYTES, rect(1)),
            ];
        }
        // Measured after this layer's slices land, while the encoded layer is
        // still alive: the per-layer peak.
        #[cfg(test)]
        record_encode_residency(EncodeResidency {
            raw_fill: raw.len(),
            output_capacity: out.iter().map(|[a, b]| a.capacity() + b.capacity()).sum(),
            scratch: image.len() + encoded.len(),
        });
    }
    out
}

/// Bytes the encoder holds at its per-layer peak: the caller's raw fill, the
/// block planes sliced so far, and the `2W × H` image plus that layer's
/// returned blocks.
/// `raw_fill` is the length of the slice passed in, not a live measurement;
/// the raw fill's allocation and release are counted in the parent module.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct EncodeResidency {
    pub(super) raw_fill: usize,
    pub(super) output_capacity: usize,
    pub(super) scratch: usize,
}

#[cfg(test)]
thread_local! {
    static PEAK_ENCODE_RESIDENCY: std::cell::Cell<Option<EncodeResidency>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
fn record_encode_residency(residency: EncodeResidency) {
    PEAK_ENCODE_RESIDENCY.with(|peak| {
        let total = |r: EncodeResidency| r.raw_fill + r.output_capacity + r.scratch;
        if peak
            .get()
            .is_none_or(|current| total(residency) > total(current))
        {
            peak.set(Some(residency));
        }
    });
}

#[cfg(test)]
pub(super) fn take_peak_encode_residency() -> Option<EncodeResidency> {
    PEAK_ENCODE_RESIDENCY.with(std::cell::Cell::take)
}

/// Every block's groups for a fully visible raw fill, built without the raw
/// fill: a constant BC4 block is `[v, v, 0, 0, 0, 0, 0, 0]`.
pub(super) fn all_visible_blocks(blocks: &[CellBlock]) -> Vec<[Vec<u8>; 2]> {
    const FULLY_VISIBLE_BLOCK: [u8; 16] = [255, 255, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0];
    blocks
        .iter()
        .map(|block| {
            assert!(
                block.width % 4 == 0 && block.height % 4 == 0,
                "shadowmask block {}x{} is not 4-aligned; BC5 would truncate it",
                block.width,
                block.height
            );
            let len = group_plane_len(block.width, block.height)
                .expect("shadowmask group length exceeds addressable memory")
                as usize;
            let plane: Vec<u8> = FULLY_VISIBLE_BLOCK
                .iter()
                .copied()
                .cycle()
                .take(len)
                .collect();
            [plane.clone(), plane]
        })
        .collect()
}

/// Decode every block's groups back into a layer-major raw fill of the bake
/// layers, per texel `[g0.r, g0.g, g1.r, g1.g]`. Texels outside every block
/// read fully visible, as the raw fill initializes them.
#[cfg(test)]
pub(crate) fn decode_blocks_to_layers(
    section: &postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection,
    blocks: &[CellBlock],
    width: u32,
    height: u32,
    layer_count: u32,
) -> Vec<u8> {
    let plane = width as usize * height as usize;
    let mut masks = vec![255u8; plane * layer_count as usize * 4];
    for (block, [a, b]) in blocks.iter().zip(&section.blocks) {
        let group_a = crate::bc5::decode_bc5_rg(a, block.width, block.height);
        let group_b = crate::bc5::decode_bc5_rg(b, block.width, block.height);
        for y in 0..block.height as usize {
            for x in 0..block.width as usize {
                let local = (y * block.width as usize + x) * 2;
                let global = block.layer as usize * plane
                    + (block.y as usize + y) * width as usize
                    + block.x as usize
                    + x;
                masks[global * 4..global * 4 + 4].copy_from_slice(&[
                    group_a[local],
                    group_a[local + 1],
                    group_b[local],
                    group_b[local + 1],
                ]);
            }
        }
    }
    masks
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

    fn block(layer: u32, x: u32, y: u32, width: u32, height: u32) -> CellBlock {
        CellBlock {
            cell_id: layer * 100 + x,
            width,
            height,
            layer,
            x,
            y,
        }
    }

    #[test]
    fn block_encode_places_rg_in_group_a_and_ba_in_group_b_per_block() {
        let (width, height, layer_count) = (8u32, 4u32, 2u32);
        // Each channel constant within one 4×4 block, so BC4 reproduces it exactly
        // and any group, block or layer misplacement shows as a wrong value.
        let mut raw = Vec::new();
        for layer in 0..layer_count {
            for _y in 0..height {
                for x in 0..width {
                    let unit = (x / 4) as u8;
                    let base = layer as u8 * 100 + unit * 10;
                    raw.extend_from_slice(&[base + 1, base + 2, base + 3, base + 4]);
                }
            }
        }
        // Layer 1 is split into two 4×4 blocks, listed out of layer order.
        let blocks = [
            block(1, 4, 0, 4, 4),
            block(0, 0, 0, 8, 4),
            block(1, 0, 0, 4, 4),
        ];
        let groups = encode_blocks_bc5(&raw, width, height, layer_count, &blocks);
        assert_eq!(groups[0][0].len(), 16);
        assert_eq!(groups[1][1].len(), 32);
        let section = ShadowmaskAtlasSection {
            channels: vec![0],
            blocks: groups,
        };
        assert_eq!(
            decode_blocks_to_layers(&section, &blocks, width, height, layer_count),
            raw
        );
    }

    #[test]
    fn all_visible_blocks_equal_encoding_an_all_visible_fill_and_decode_fully_lit() {
        let (width, height, layer_count) = (8u32, 8u32, 3u32);
        let blocks = [block(0, 0, 0, 8, 8), block(2, 4, 4, 4, 4)];
        let raw = vec![255u8; (width * height * layer_count * 4) as usize];
        let encoded = encode_blocks_bc5(&raw, width, height, layer_count, &blocks);
        assert_eq!(all_visible_blocks(&blocks), encoded);
        let section = ShadowmaskAtlasSection {
            channels: vec![],
            blocks: encoded,
        };
        assert!(
            decode_blocks_to_layers(&section, &blocks, width, height, layer_count)
                .iter()
                .all(|&mask| mask == 255)
        );
    }

    #[test]
    #[should_panic(expected = "not 4-aligned")]
    fn block_encode_refuses_misaligned_dimensions() {
        encode_blocks_bc5(&[255; 5 * 4 * 4], 5, 4, 1, &[]);
    }
}
