// Raw RGBA shadowmask fill → side-by-side BC5 payload.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::shadowmask_atlas::{SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection};

use crate::bc5::encode_bc5_rg;

/// Encode a layer-major raw `Rgba8Unorm` mask fill as side-by-side BC5.
///
/// Per layer, R/G become group 0 in the left half of a `2W × H` image and
/// B/A become group 1 in the right half; that image is encoded once. Scratch
/// is one `2W × H` RGBA image (two raw layers) plus the encoder's returned
/// blocks for that layer (half a raw layer), reused layer to layer.
///
/// Panics in every profile on dimensions that are not multiples of 4: the
/// encoder would otherwise drop the remainder blocks and emit a truncated
/// payload. Bake entry points reject such atlases with an error first.
pub(super) fn encode_side_by_side_bc5(
    raw: &[u8],
    width: u32,
    height: u32,
    layer_count: u32,
) -> Vec<u8> {
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
    let payload_len = ShadowmaskAtlasSection::payload_len(width, height, layer_count)
        .expect("shadowmask payload length exceeds addressable memory");

    let mut out = Vec::with_capacity(payload_len);
    let mut image = vec![0u8; plane_texels * 4 * SHADOWMASK_GROUP_COUNT as usize];
    for layer in raw.chunks_exact(plane_texels * 4) {
        for (row_index, raw_row) in layer.chunks_exact(width as usize * 4).enumerate() {
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
        out.extend_from_slice(&encode_bc5_rg(&image, texture_width, height));
    }
    debug_assert_eq!(out.len(), payload_len);
    out
}

/// The payload a fully visible raw fill encodes to, built without the raw
/// fill: a constant BC4 block is `[v, v, 0, 0, 0, 0, 0, 0]`.
pub(super) fn all_visible_bc5_payload(width: u32, height: u32, layer_count: u32) -> Vec<u8> {
    assert!(
        width % 4 == 0 && height % 4 == 0,
        "shadowmask atlas {width}x{height} is not 4-aligned; BC5 would truncate it"
    );
    const FULLY_VISIBLE_BLOCK: [u8; 16] = [255, 255, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0];
    let payload_len = ShadowmaskAtlasSection::payload_len(width, height, layer_count)
        .expect("shadowmask payload length exceeds addressable memory");
    FULLY_VISIBLE_BLOCK
        .iter()
        .copied()
        .cycle()
        .take(payload_len)
        .collect()
}

/// Decode a side-by-side payload back to per-texel `[g0.r, g0.g, g1.r, g1.g]`,
/// layer-major, matching the raw fill's texel order.
#[cfg(test)]
pub(crate) fn decode_side_by_side(
    payload: &[u8],
    width: u32,
    height: u32,
    layer_count: u32,
) -> Vec<u8> {
    let layer_bytes = ShadowmaskAtlasSection::layer_payload_len(width, height).unwrap();
    let texture_width = width * SHADOWMASK_GROUP_COUNT;
    let mut masks = Vec::with_capacity((width * height * layer_count * 4) as usize);
    for layer in payload.chunks_exact(layer_bytes) {
        let rg = crate::bc5::decode_bc5_rg(layer, texture_width, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let left = (y * texture_width as usize + x) * 2;
                let right = left + width as usize * 2;
                masks.extend_from_slice(&[rg[left], rg[left + 1], rg[right], rg[right + 1]]);
            }
        }
    }
    masks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_by_side_encode_places_rg_left_and_ba_right_per_layer() {
        let (width, height, layer_count) = (8u32, 4u32, 2u32);
        // Each channel constant within one 4×4 block, so BC4 reproduces it exactly
        // and any group or layer misplacement shows as a wrong value.
        let mut raw = Vec::new();
        for layer in 0..layer_count {
            for _y in 0..height {
                for x in 0..width {
                    let block = (x / 4) as u8;
                    let base = layer as u8 * 100 + block * 10;
                    raw.extend_from_slice(&[base + 1, base + 2, base + 3, base + 4]);
                }
            }
        }
        let payload = encode_side_by_side_bc5(&raw, width, height, layer_count);
        assert_eq!(payload.len(), raw.len() / 2);
        assert_eq!(
            decode_side_by_side(&payload, width, height, layer_count),
            raw
        );
    }

    #[test]
    fn all_visible_payload_equals_encoding_an_all_visible_fill_and_decodes_fully_lit() {
        let (width, height, layer_count) = (8u32, 8u32, 3u32);
        let raw = vec![255u8; (width * height * layer_count * 4) as usize];
        let encoded = encode_side_by_side_bc5(&raw, width, height, layer_count);
        assert_eq!(all_visible_bc5_payload(width, height, layer_count), encoded);
        assert!(
            decode_side_by_side(&encoded, width, height, layer_count)
                .iter()
                .all(|&mask| mask == 255)
        );
    }

    #[test]
    #[should_panic(expected = "not 4-aligned")]
    fn side_by_side_encode_refuses_misaligned_dimensions() {
        encode_side_by_side_bc5(&[255; 5 * 4 * 4], 5, 4, 1);
    }
}
