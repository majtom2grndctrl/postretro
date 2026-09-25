// In-tree BC5 encoder shared by normal-map mips and the shadowmask atlas.
// See: context/lib/build_pipeline.md §Baked texture mips, §PRL section IDs

//! BC5 encodes two independent channels as two back-to-back BC4 blocks per
//! 4×4 texel block (16 bytes total): block 0 = R channel, block 1 = G channel.
//! Each BC4 block is `[ep0: u8, ep1: u8, 48 bits of 3-bit-per-texel selectors]`.
//! Only R and G of each RGBA texel are read; B and A are ignored.
//!
//! Endpoints come from a trivial per-block min/max search (no cluster-fit
//! refinement). `encode_bc5_rg` always uses the 8-interpolated-value BC4 mode
//! (`ep0 > ep1`), which spends all eight palette entries on `[min, max]` — the
//! most precise mode for smooth data. Normal maps are low-frequency relative to
//! pixel-art diffuse, and the round-trip tolerance (unit length within 1/127,
//! within 2° of the input direction) is met without refinement. Tangent-space
//! normals store `(n.x, n.y)` in R and G; the shader reconstructs
//! `n.z = sqrt(max(0, 1 - x*x - y*y))`.
//!
//! `encode_bc5_rg_masks` serves visibility masks, whose blocks often mix hard
//! 0 / 255 texels with penumbra values. The 8-value ladder then spans the whole
//! `[0, 255]` range and rounds the penumbra coarsely, so each BC4 block also
//! tries the 6-value mode (`ep0 <= ep1`: four interpolants between the
//! non-extreme texels, plus explicit 0 and 255) and keeps it only when it
//! lowers the block's error.

/// BC4 palette modes an encode may choose between, per block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bc4Modes {
    /// 8-value mode only. The normal-map payload's bytes depend on this.
    EightValueOnly,
    /// 8-value mode, or 6-value mode when it has strictly lower squared error.
    EightOrSixValue,
}

/// Encode an Rgba8Unorm normal-map level into a BC5 RG byte payload.
///
/// `rgba` is row-major, tightly packed (no row padding), 4 bytes/texel.
/// `width` and `height` must be ≥ 4 and multiples of 4 — the caller handles
/// padding/skipping of sub-4 mips per the per-mip rule. Blocks are emitted in
/// row-major 4×4 order, 16 bytes each (BC4 R block then BC4 G block).
pub fn encode_bc5_rg(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    encode_bc5_rg_with(rgba, width, height, Bc4Modes::EightValueOnly)
}

/// Encode an Rgba8Unorm image of visibility masks (R and G) into a BC5 RG
/// byte payload, choosing the 6-value BC4 mode per block where it is more
/// accurate. Layout and preconditions match [`encode_bc5_rg`].
pub fn encode_bc5_rg_masks(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    encode_bc5_rg_with(rgba, width, height, Bc4Modes::EightOrSixValue)
}

fn encode_bc5_rg_with(rgba: &[u8], width: u32, height: u32, modes: Bc4Modes) -> Vec<u8> {
    debug_assert!(
        width >= 4 && height >= 4 && width % 4 == 0 && height % 4 == 0,
        "BC5 input must be ≥4 and a multiple of 4 in both dimensions (got {width}×{height})"
    );
    debug_assert_eq!(
        rgba.len(),
        (width * height * 4) as usize,
        "BC5 input byte length does not match width×height×4"
    );

    let blocks_x = width / 4;
    let blocks_y = height / 4;
    let mut out = Vec::with_capacity((blocks_x * blocks_y * 16) as usize);

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            // Gather the 16 texels' R and G channels for this block.
            let mut r = [0u8; 16];
            let mut g = [0u8; 16];
            for ty in 0..4 {
                for tx in 0..4 {
                    let px = bx * 4 + tx;
                    let py = by * 4 + ty;
                    let base = ((py * width + px) * 4) as usize;
                    let i = (ty * 4 + tx) as usize;
                    r[i] = rgba[base];
                    g[i] = rgba[base + 1];
                }
            }
            out.extend_from_slice(&encode_bc4_block(&r, modes));
            out.extend_from_slice(&encode_bc4_block(&g, modes));
        }
    }

    out
}

/// Build the eight-entry palette for the 8-interpolated-value BC4 mode, where
/// `ep0 > ep1`. Index 0 = ep0 (max), index 1 = ep1 (min), indices 2..=7 are
/// the 6 interpolated entries between them using the D3D/wgpu integer
/// formulas.
// The `1 *` coefficients keep the rows aligned with the D3D BC4 coefficient ladder (6:1 … 1:6).
#[allow(clippy::identity_op)]
fn bc4_palette(ep0: u8, ep1: u8) -> [u8; 8] {
    let e0 = ep0 as u32;
    let e1 = ep1 as u32;
    let mut palette = [0u8; 8];
    palette[0] = ep0;
    palette[1] = ep1;
    // Indices 2..=7 are the 6 interpolated entries between ep0 (index 0) and ep1 (index 1).
    // Hardware interpolates in float; integer division truncates, so an
    // interpolant sits at most 1 LSB below what the GPU reconstructs. Endpoints
    // and constant blocks are exact.
    palette[2] = ((6 * e0 + 1 * e1) / 7) as u8;
    palette[3] = ((5 * e0 + 2 * e1) / 7) as u8;
    palette[4] = ((4 * e0 + 3 * e1) / 7) as u8;
    palette[5] = ((3 * e0 + 4 * e1) / 7) as u8;
    palette[6] = ((2 * e0 + 5 * e1) / 7) as u8;
    palette[7] = ((1 * e0 + 6 * e1) / 7) as u8;
    palette
}

/// Build the eight-entry palette for the 6-interpolated-value BC4 mode, where
/// `ep0 <= ep1`: the endpoints, four interpolants between them (same integer
/// truncation as [`bc4_palette`]), then explicit 0 and 255.
// The `1 *` coefficients keep the rows aligned with the D3D BC4 coefficient ladder (4:1 … 1:4).
#[allow(clippy::identity_op)]
fn bc4_palette_six(ep0: u8, ep1: u8) -> [u8; 8] {
    let e0 = ep0 as u32;
    let e1 = ep1 as u32;
    [
        ep0,
        ep1,
        ((4 * e0 + 1 * e1) / 5) as u8,
        ((3 * e0 + 2 * e1) / 5) as u8,
        ((2 * e0 + 3 * e1) / 5) as u8,
        ((1 * e0 + 4 * e1) / 5) as u8,
        0,
        255,
    ]
}

/// Encode one 4×4 single-channel block into 8 BC4 bytes:
/// `[ep0, ep1, 6 bytes of packed 3-bit selectors]`.
fn encode_bc4_block(texels: &[u8; 16], modes: Bc4Modes) -> [u8; 8] {
    let min = *texels.iter().min().expect("16 texels");
    let max = *texels.iter().max().expect("16 texels");

    // Degenerate block (all equal): emit ep0 == ep1 with zero selectors. With
    // ep0 == ep1 the palette is constant, so any selector reproduces the value;
    // we keep selectors at 0 (index 0 = ep0 = the value).
    if min == max {
        return [max, min, 0, 0, 0, 0, 0, 0];
    }

    // 8-value mode requires ep0 > ep1. Use max as ep0, min as ep1 so all eight
    // palette entries cover the [min, max] interval.
    let eight = fit_bc4_block(texels, max, min, &bc4_palette(max, min));
    if modes == Bc4Modes::EightValueOnly {
        return eight.block;
    }

    // 6-value mode spends its interpolants on the texels that are not already
    // exact through the explicit 0 / 255 entries. A block of only 0 and 255
    // is exact in 8-value mode (ep0 = 255, ep1 = 0), so it has nothing to gain.
    let mut interior = texels.iter().copied().filter(|&v| v != 0 && v != 255);
    let Some(first) = interior.next() else {
        return eight.block;
    };
    let (lo, hi) = interior.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let six = fit_bc4_block(texels, lo, hi, &bc4_palette_six(lo, hi));
    // Ties keep the 8-value block, so the choice is deterministic and only a
    // strictly better fit changes mode.
    if six.squared_error < eight.squared_error {
        six.block
    } else {
        eight.block
    }
}

/// One BC4 block and its squared error against the source texels.
struct Bc4Fit {
    block: [u8; 8],
    squared_error: u32,
}

/// Select, per texel, the nearest palette entry (lowest index on ties) and
/// pack the block for the given endpoint order.
fn fit_bc4_block(texels: &[u8; 16], ep0: u8, ep1: u8, palette: &[u8; 8]) -> Bc4Fit {
    let mut selectors = [0u8; 16];
    let mut squared_error = 0u32;
    for (i, &v) in texels.iter().enumerate() {
        let mut best_idx = 0u8;
        let mut best_err = u16::MAX;
        for (idx, &pv) in palette.iter().enumerate() {
            let diff = (v as i16 - pv as i16).unsigned_abs();
            if diff < best_err {
                best_err = diff;
                best_idx = idx as u8;
            }
        }
        selectors[i] = best_idx;
        squared_error += u32::from(best_err) * u32::from(best_err);
    }

    // Pack 16 × 3-bit selectors (48 bits) little-endian into 6 bytes.
    let mut bits: u64 = 0;
    for (i, &sel) in selectors.iter().enumerate() {
        bits |= ((sel as u64) & 0x7) << (3 * i);
    }

    let mut block = [0u8; 8];
    block[0] = ep0;
    block[1] = ep1;
    for (i, byte) in block[2..8].iter_mut().enumerate() {
        *byte = ((bits >> (8 * i)) & 0xFF) as u8;
    }
    Bc4Fit {
        block,
        squared_error,
    }
}

/// Decode one BC4 block (8 bytes) back to 16 channel values with both D3D/wgpu
/// palette modes. Interpolants use integer division, so they match the GPU's
/// float interpolation within 1 LSB (truncated low); endpoints, the 6-value
/// mode's explicit 0 / 255, and constant blocks match exactly.
#[cfg(test)]
// The `1 *` coefficients keep the rows aligned with the D3D BC4 coefficient ladders (6:1 … 1:6 and 4:1 … 1:4).
#[allow(clippy::identity_op)]
fn decode_bc4_block(block: &[u8; 8]) -> [u8; 16] {
    let ep0 = block[0] as u32;
    let ep1 = block[1] as u32;

    let mut palette = [0u8; 8];
    palette[0] = ep0 as u8;
    palette[1] = ep1 as u8;
    if ep0 > ep1 {
        // 8-value mode: indices 2..=7 are the 6 interpolated entries between
        // ep0 (index 0) and ep1 (index 1).
        palette[2] = ((6 * ep0 + 1 * ep1) / 7) as u8;
        palette[3] = ((5 * ep0 + 2 * ep1) / 7) as u8;
        palette[4] = ((4 * ep0 + 3 * ep1) / 7) as u8;
        palette[5] = ((3 * ep0 + 4 * ep1) / 7) as u8;
        palette[6] = ((2 * ep0 + 5 * ep1) / 7) as u8;
        palette[7] = ((1 * ep0 + 6 * ep1) / 7) as u8;
    } else {
        // 6 interpolated values plus explicit 0 / 255 endpoints.
        palette[2] = ((4 * ep0 + 1 * ep1) / 5) as u8;
        palette[3] = ((3 * ep0 + 2 * ep1) / 5) as u8;
        palette[4] = ((2 * ep0 + 3 * ep1) / 5) as u8;
        palette[5] = ((1 * ep0 + 4 * ep1) / 5) as u8;
        palette[6] = 0;
        palette[7] = 255;
    }

    let mut bits: u64 = 0;
    for (i, &b) in block[2..8].iter().enumerate() {
        bits |= (b as u64) << (8 * i);
    }

    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        let sel = ((bits >> (3 * i)) & 0x7) as usize;
        *slot = palette[sel];
    }
    out
}

/// Decode a full BC5 RG payload back into an RG byte buffer (2 bytes/texel,
/// row-major). Mirrors the GPU sampler's view of BC5.
#[cfg(test)]
pub(crate) fn decode_bc5_rg(blocks: &[u8], width: u32, height: u32) -> Vec<u8> {
    let blocks_x = width / 4;
    let blocks_y = height / 4;
    let mut rg = vec![0u8; (width * height * 2) as usize];
    let mut cursor = 0usize;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let r_block: [u8; 8] = blocks[cursor..cursor + 8].try_into().unwrap();
            let g_block: [u8; 8] = blocks[cursor + 8..cursor + 16].try_into().unwrap();
            cursor += 16;
            let r = decode_bc4_block(&r_block);
            let g = decode_bc4_block(&g_block);
            for ty in 0..4 {
                for tx in 0..4 {
                    let px = bx * 4 + tx;
                    let py = by * 4 + ty;
                    let i = (ty * 4 + tx) as usize;
                    let base = ((py * width + px) * 2) as usize;
                    rg[base] = r[i];
                    rg[base + 1] = g[i];
                }
            }
        }
    }
    rg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode unorm `[0, 1]` to a u8 channel value (matches the authoring
    /// convention `byte = (n*0.5 + 0.5) * 255`).
    fn encode_axis(v: f32) -> u8 {
        ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Decode a u8 channel value back to `[-1, 1]` (shader convention).
    fn decode_axis(b: u8) -> f32 {
        (b as f32) / 255.0 * 2.0 - 1.0
    }

    /// Synthetic tangent-space normal map, encoded to BC5 and decoded with Z
    /// reconstructed via the shader formula, stays unit-length within 1/127 and
    /// within 2° of the input direction at every texel.
    #[test]
    fn bc5_rg_roundtrip_keeps_normals_unit_and_within_two_degrees() {
        let w = 8u32;
        let h = 8u32;

        // Build a smooth tangent-space normal field: directions tilt gently
        // away from (0, 0, 1) across the surface — typical normal-map content.
        // Tilt factor 0.6 keeps the worst-case BC4 quantization error well within
        // the 2° tolerance for the hardware integer palette (max observed ≈1.46°).
        let mut input_dirs: Vec<[f32; 3]> = Vec::with_capacity((w * h) as usize);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let nx = (x as f32 / (w - 1) as f32 - 0.5) * 0.6;
                let ny = (y as f32 / (h - 1) as f32 - 0.5) * 0.6;
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let dir = [nx / len, ny / len, nz / len];
                input_dirs.push(dir);
                rgba.push(encode_axis(dir[0]));
                rgba.push(encode_axis(dir[1]));
                rgba.push(encode_axis(dir[2])); // ignored by the encoder
                rgba.push(255);
            }
        }

        let blocks = encode_bc5_rg(&rgba, w, h);
        assert_eq!(blocks.len(), ((w / 4) * (h / 4) * 16) as usize);

        let rg = decode_bc5_rg(&blocks, w, h);

        let unit_tol = 1.0 / 127.0;
        let angle_tol_rad = 2.0_f32.to_radians();
        for (i, expected) in input_dirs.iter().enumerate() {
            let rx = decode_axis(rg[i * 2]);
            let ry = decode_axis(rg[i * 2 + 1]);
            // Shader-side Z reconstruction.
            let rz = (1.0 - rx * rx - ry * ry).max(0.0).sqrt();

            let len = (rx * rx + ry * ry + rz * rz).sqrt();
            assert!(
                (len - 1.0).abs() <= unit_tol,
                "texel {i}: reconstructed normal not unit-length (len = {len})"
            );

            let dot = (rx * expected[0] + ry * expected[1] + rz * expected[2]).clamp(-1.0, 1.0);
            let angle = dot.acos();
            assert!(
                angle <= angle_tol_rad,
                "texel {i}: reconstructed normal {angle} rad off (> 2°): \
                 got ({rx}, {ry}, {rz}), expected {expected:?}"
            );
        }
    }

    /// A constant block hits the `min == max` degenerate path in both BC4
    /// sub-blocks (ep0 == ep1, zero selectors). The hardware palette is then
    /// constant, so every texel decodes back to the input byte exactly,
    /// regardless of which interpolation mode the GPU infers from the endpoints.
    #[test]
    fn bc5_rg_roundtrip_reproduces_flat_block_exactly() {
        let w = 4u32;
        let h = 4u32;

        // A single off-axis tangent-space normal, identical across the block.
        let nx = 0.3_f32;
        let ny = -0.2_f32;
        let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let r = encode_axis(nx / len);
        let g = encode_axis(ny / len);
        let b = encode_axis(nz / len);

        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba.push(r);
            rgba.push(g);
            rgba.push(b); // ignored by the encoder
            rgba.push(255);
        }

        let blocks = encode_bc5_rg(&rgba, w, h);
        let rg = decode_bc5_rg(&blocks, w, h);

        for i in 0..(w * h) as usize {
            assert_eq!(rg[i * 2], r, "texel {i}: R channel not reproduced exactly");
            assert_eq!(
                rg[i * 2 + 1],
                g,
                "texel {i}: G channel not reproduced exactly"
            );
        }
    }

    fn squared_error(texels: &[u8; 16], decoded: &[u8; 16]) -> u32 {
        texels
            .iter()
            .zip(decoded)
            .map(|(&a, &b)| u32::from(a.abs_diff(b)).pow(2))
            .sum()
    }

    /// Deterministic 4×4 blocks biased toward the shadowmask shape: hard 0 and
    /// 255 texels mixed with arbitrary penumbra values.
    fn mask_like_blocks(count: usize) -> Vec<[u8; 16]> {
        let mut state = 0x2545_f491_u32;
        (0..count)
            .map(|_| {
                std::array::from_fn(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    match state >> 30 {
                        0 => 0,
                        1 => 255,
                        _ => (state >> 16) as u8,
                    }
                })
            })
            .collect()
    }

    /// 8×4 image: block 0 R is a 0..=255 ramp and G cycles 0 / 128 / 255;
    /// block 1 R is constant 77 and G a 100..=115 gradient.
    fn two_block_fixture() -> Vec<u8> {
        let mut rgba = Vec::with_capacity(8 * 4 * 4);
        for y in 0..4u8 {
            for x in 0..8u8 {
                let i = y * 4 + x % 4;
                let (r, g) = if x < 4 {
                    (i * 17, [0, 128, 255][usize::from(i % 3)])
                } else {
                    (77, 100 + i)
                };
                rgba.extend_from_slice(&[r, g, 0, 255]);
            }
        }
        rgba
    }

    #[test]
    fn bc4_masks_encode_reproduces_hard_edges_with_penumbra_exactly() {
        let texels: [u8; 16] = std::array::from_fn(|i| [0, 128, 255][i % 3]);

        // The 8-value ladder over [0, 255] has no entry near 128.
        let eight = decode_bc4_block(&encode_bc4_block(&texels, Bc4Modes::EightValueOnly));
        assert_eq!(
            texels
                .iter()
                .zip(&eight)
                .map(|(&a, &b)| a.abs_diff(b))
                .max(),
            Some(17)
        );

        let block = encode_bc4_block(&texels, Bc4Modes::EightOrSixValue);
        assert!(
            block[0] <= block[1],
            "6-value mode is ep0 <= ep1: {block:?}"
        );
        assert_eq!(decode_bc4_block(&block), texels);
    }

    #[test]
    fn bc4_masks_encode_switches_to_six_value_mode_only_when_it_lowers_error() {
        let only_hard: [u8; 16] = std::array::from_fn(|i| if i % 2 == 0 { 0 } else { 255 });
        let gradient: [u8; 16] = std::array::from_fn(|i| 100 + i as u8);
        let ramp: [u8; 16] = std::array::from_fn(|i| i as u8 * 17);
        // Only 0 / 255: both modes are exact, and the tie keeps 8-value mode.
        // No hard texel: 8-value mode's finer ladder wins. Full ramp: the hard
        // ends are present, but 8-value mode still fits the interior better.
        for texels in [only_hard, gradient, ramp] {
            let block = encode_bc4_block(&texels, Bc4Modes::EightOrSixValue);
            assert_eq!(block, encode_bc4_block(&texels, Bc4Modes::EightValueOnly));
            assert!(block[0] > block[1], "8-value mode is ep0 > ep1: {block:?}");
        }
        assert_eq!(
            decode_bc4_block(&encode_bc4_block(&only_hard, Bc4Modes::EightOrSixValue)),
            only_hard
        );

        let mut switched = 0;
        for texels in mask_like_blocks(256) {
            let eight = encode_bc4_block(&texels, Bc4Modes::EightValueOnly);
            let chosen = encode_bc4_block(&texels, Bc4Modes::EightOrSixValue);
            let eight_error = squared_error(&texels, &decode_bc4_block(&eight));
            let chosen_error = squared_error(&texels, &decode_bc4_block(&chosen));
            if chosen == eight {
                continue;
            }
            switched += 1;
            assert!(chosen[0] <= chosen[1], "a switched block is 6-value mode");
            assert!(
                chosen_error < eight_error,
                "6-value mode must lower error ({chosen_error} vs {eight_error}): {texels:?}"
            );
        }
        assert!(switched > 0, "fixture must exercise the 6-value mode");
    }

    /// Pins the normal-map payload: the mask encoder's 6-value mode must not
    /// reach `encode_bc5_rg`. Bytes are the 8-value-only encoding of the
    /// fixture, as shipped before the mask entry point existed.
    #[test]
    fn bc5_normal_map_encode_bytes_are_unchanged_by_the_mask_entry_point() {
        let rgba = two_block_fixture();
        let normal = encode_bc5_rg(&rgba, 8, 4);
        assert_eq!(
            normal,
            [
                255, 0, 201, 111, 183, 228, 38, 1, // block 0 R: ramp
                255, 0, 33, 66, 132, 8, 17, 34, // block 0 G: 0 / 128 / 255
                77, 77, 0, 0, 0, 0, 0, 0, // block 1 R: constant
                115, 100, 201, 237, 150, 220, 36, 1, // block 1 G: gradient
            ]
        );

        // The mask encoder differs only in the 0 / 128 / 255 block.
        let masks = encode_bc5_rg_masks(&rgba, 8, 4);
        assert_eq!(masks[..8], normal[..8]);
        assert_eq!(masks[8..16], [128, 128, 198, 141, 27, 55, 110, 220]);
        assert_eq!(masks[16..], normal[16..]);
    }

    #[test]
    fn bc5_masks_encode_is_deterministic() {
        let (w, h) = (16u32, 16u32);
        let blocks = mask_like_blocks((w * h / 16) as usize * 2);
        let values: Vec<u8> = blocks.iter().flatten().copied().collect();
        let rgba: Vec<u8> = values
            .chunks_exact(2)
            .flat_map(|rg| [rg[0], rg[1], 0, 255])
            .collect();
        let first = encode_bc5_rg_masks(&rgba, w, h);
        assert_eq!(first.len(), ((w / 4) * (h / 4) * 16) as usize);
        assert_eq!(encode_bc5_rg_masks(&rgba, w, h), first);
    }
}
