// In-tree BC5 (two-channel block compression) encoder for normal-map slots.
// See: context/lib/build_pipeline.md §Baked texture mips

//! BC5 encodes two independent channels as two back-to-back BC4 blocks per
//! 4×4 texel block (16 bytes total): block 0 = R channel, block 1 = G channel.
//! Each BC4 block is `[ep0: u8, ep1: u8, 48 bits of 3-bit-per-texel selectors]`.
//!
//! We use the 8-interpolated-value BC4 mode (`ep0 > ep1`), which spends all
//! eight palette entries on the `[min, max]` interval — the most precise mode,
//! and the right choice for smooth normal-map data. Endpoints come from a
//! trivial per-block min/max search (no cluster-fit refinement); the plan
//! sanctions this because normal maps are low-frequency and the round-trip
//! tolerance is generous.
//!
//! Tangent-space encoding stores `(n.x, n.y)` in the R and G channels; the
//! shader reconstructs `n.z = sqrt(max(0, 1 - x*x - y*y))`. Only R and G of
//! each RGBA texel are read here; B and A are ignored.

/// Encode an Rgba8Unorm normal-map level into a BC5 RG byte payload.
///
/// `rgba` is row-major, tightly packed (no row padding), 4 bytes/texel.
/// `width` and `height` must be ≥ 4 and multiples of 4 — the caller handles
/// padding/skipping of sub-4 mips per the per-mip rule. Blocks are emitted in
/// row-major 4×4 order, 16 bytes each (BC4 R block then BC4 G block).
pub fn encode_bc5_rg(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
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
            out.extend_from_slice(&encode_bc4_block(&r));
            out.extend_from_slice(&encode_bc4_block(&g));
        }
    }

    out
}

/// Build the eight-entry palette for the 8-interpolated-value BC4 mode, where
/// `ep0 > ep1`. Index 0 = ep0 (max), index 1 = ep1 (min), indices 2..=7 are
/// six evenly-spaced interpolants between them.
fn bc4_palette(ep0: u8, ep1: u8) -> [u8; 8] {
    let hi = ep0 as f32;
    let lo = ep1 as f32;
    let mut palette = [0u8; 8];
    palette[0] = ep0;
    palette[1] = ep1;
    for k in 1..=6u32 {
        // Interpolant k sits at fraction k/7 from ep0 toward ep1.
        let t = k as f32 / 7.0;
        palette[(k + 1) as usize] = (hi + (lo - hi) * t).round() as u8;
    }
    palette
}

/// Encode one 4×4 single-channel block into 8 BC4 bytes:
/// `[ep0, ep1, 6 bytes of packed 3-bit selectors]`.
fn encode_bc4_block(texels: &[u8; 16]) -> [u8; 8] {
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
    let ep0 = max;
    let ep1 = min;
    let palette = bc4_palette(ep0, ep1);

    // Pick, per texel, the palette index whose value is closest.
    let mut selectors = [0u8; 16];
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
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode one BC4 block (8 bytes) back to 16 channel values, mirroring the
    /// GPU's BC5 channel decode for the 8-interpolated-value / 6-interpolated
    /// modes. Used by the round-trip test to read BC5 payloads back.
    fn decode_bc4_block(block: &[u8; 8]) -> [u8; 16] {
        let ep0 = block[0];
        let ep1 = block[1];

        let mut palette = [0u8; 8];
        palette[0] = ep0;
        palette[1] = ep1;
        let hi = ep0 as f32;
        let lo = ep1 as f32;
        if ep0 > ep1 {
            // 8 interpolated values.
            for k in 1..=6u32 {
                let t = k as f32 / 7.0;
                palette[(k + 1) as usize] = (hi + (lo - hi) * t).round() as u8;
            }
        } else {
            // 6 interpolated values plus explicit 0.0 / 1.0 endpoints.
            for k in 1..=4u32 {
                let t = k as f32 / 5.0;
                palette[(k + 1) as usize] = (hi + (lo - hi) * t).round() as u8;
            }
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
    fn decode_bc5_rg(blocks: &[u8], width: u32, height: u32) -> Vec<u8> {
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
        let mut input_dirs: Vec<[f32; 3]> = Vec::with_capacity((w * h) as usize);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let nx = (x as f32 / (w - 1) as f32 - 0.5) * 0.8;
                let ny = (y as f32 / (h - 1) as f32 - 0.5) * 0.8;
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
}
