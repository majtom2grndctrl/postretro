// In-tree BC6H (`Bc6hRgbUfloat`) encoder for the irradiance lightmap atlas.
// See: context/lib/build_pipeline.md §PRL Compilation (Lightmap section id 22),
//      context/lib/build_pipeline.md §Build Cache (determinism / BC6H exemption)

//! BC6H encodes 4×4 HDR RGB texel blocks as 128-bit (16-byte) payloads. The
//! format supports 14 mode variants (one or two subsets, deltas, varying
//! endpoint precision); we emit **only Mode 11** — the simplest single-subset,
//! non-delta mode that pairs full 10-bit-per-channel endpoints with 16
//! four-bit indices. Smooth low-frequency HDR irradiance reproduces cleanly
//! under a single endpoint pair; this is the same min/max-endpoint bet
//! `bc5.rs` already wins on normal maps, and it keeps the compiler
//! dependency-free per the lean northstar.
//!
//! Mode 11 wire layout (128 bits, little-endian), bit indices low-to-high:
//!
//! ```text
//!   [ 4: 0]  mode = 0b00011  (mode index 3, "Mode 11")
//!   [14: 5]  r0  (10 bits)
//!   [24:15]  g0
//!   [34:25]  b0
//!   [44:35]  r1
//!   [54:45]  g1
//!   [64:55]  b1
//!   [67:65]  index 0           (3 bits — first index is the fixup; high bit forced 0)
//!   [71:68]  index 1           (4 bits)
//!   ...
//!   [127:124] index 15
//! ```
//!
//! Block index 0 is the implicit "fixup index" — the BC6H spec stores it with
//! one bit elided (top bit forced 0). This caps texel 0's selector at 7; the
//! encoder enforces that by reordering endpoints if texel 0 maps closer to
//! `ep1` than `ep0` (swap endpoints so its selector lands in `[0, 7]`).
//!
//! Hardware decode for `Bc6hRgbUfloat` (DirectX / Khronos BC6H UFloat spec,
//! endpoint precision `epb = 10`):
//!   1. `unq(x)` lifts a 10-bit endpoint to a 16-bit "internal" value:
//!      `unq(0) = 0`, `unq(0x3ff) = 0xffff`, and otherwise
//!      `unq(x) = ((x << 15) + 0x4000) >> (epb - 1)` which for `epb = 10`
//!      reduces to the exact integer form `unq(x) = 64·x + 32`.
//!      Note this is **not** BC7-style bit-replication (`(x << 6) | (x >> 4)`);
//!      the two formulas agree at the endpoints but diverge mid-range (at
//!      `x = 1` the spec gives 96, bit-rep gives 64).
//!   2. `interp = ((64 - w) * unq0 + w * unq1 + 32) >> 6` for index weight `w`
//!      drawn from the BC6H 4-bit weight table.
//!   3. Final f16 bit pattern: `(interp * 31) >> 6` (unsigned-float
//!      finalization). This is the value the shader reads.
//!
//! The encoder picks endpoints by quantizing each channel's `[min, max]` in
//! internal space to 10-bit values via the inverse of `unq` (mirroring
//! `bc5.rs`'s min/max approach), then picks per-texel selectors by minimizing
//! squared error in the same internal 16-bit space against the input texel's
//! internal value. Comparing in internal space is equivalent to comparing in
//! output-f16 space because the finalize step is a monotonic linear scale.
//! No cluster-fit refinement; the round-trip tolerance gates the result.
//!
//! The CPU decoder ([`decode_bc6h_block`], plus a whole-payload reader the
//! tests use) reproduces that hardware decode for Mode 11 and refuses every
//! other mode: it reads only what this encoder writes, such as id 22 lightmap
//! texels a later compiler stage shades with.

use postretro_level_format::lightmap::f32_to_f16_bits;
use thiserror::Error;

/// BC6H 4-bit index weight table. Hardware uses these to interpolate between
/// endpoints `unq0`/`unq1`: `weight w / 64` applied as
/// `((64 - w) * unq0 + w * unq1 + 32) >> 6`.
const BC6H_WEIGHTS_4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Encode an HDR RGB atlas (row-major, 4 f32 per texel — A is dropped) to a
/// `Bc6hRgbUfloat` block payload.
///
/// `width` and `height` must be ≥ 4 and a multiple of 4. The atlas builder
/// already power-of-two-rounds both axes ≥ 64, so this holds for every real
/// bake; the debug bypass path skips this encoder entirely. The input slice
/// must be exactly `width·height·4` f32s.
pub fn encode_bc6h_rgb_from_f32_rgba(rgba: &[f32], width: u32, height: u32) -> Vec<u8> {
    debug_assert!(
        width >= 4 && height >= 4 && width.is_multiple_of(4) && height.is_multiple_of(4),
        "BC6H input must be ≥4 and a multiple of 4 in both dimensions (got {width}×{height})"
    );
    debug_assert_eq!(
        rgba.len(),
        (width * height * 4) as usize,
        "BC6H input f32 length does not match width·height·4"
    );

    let blocks_x = width / 4;
    let blocks_y = height / 4;
    let mut out = Vec::with_capacity((blocks_x * blocks_y * 16) as usize);

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut texels = [[0u16; 3]; 16];
            for ty in 0..4 {
                for tx in 0..4 {
                    let px = bx * 4 + tx;
                    let py = by * 4 + ty;
                    let base = ((py * width + px) * 4) as usize;
                    let i = (ty * 4 + tx) as usize;
                    // Quantize input to the same f16 representation the runtime
                    // will reconstruct to. Negative HDR values are not produced
                    // by the irradiance bake (it's a non-negative accumulator),
                    // but clamp at zero defensively — `Bc6hRgbUfloat` is
                    // unsigned and decode would clamp those anyway.
                    texels[i][0] = f32_to_f16_bits(rgba[base].max(0.0));
                    texels[i][1] = f32_to_f16_bits(rgba[base + 1].max(0.0));
                    texels[i][2] = f32_to_f16_bits(rgba[base + 2].max(0.0));
                }
            }
            out.extend_from_slice(&encode_bc6h_block(&texels));
        }
    }

    out
}

/// Encode a single 4×4 block (16 RGB f16 texels) to one 16-byte BC6H Mode 11
/// payload.
fn encode_bc6h_block(texels: &[[u16; 3]; 16]) -> [u8; 16] {
    // Per-channel f16 → "internal" 16-bit form. Decode runs
    // `output_f16 = (interp * 31) >> 6`, so encode lifts the input f16 into
    // the pre-finalize internal range by inverting that step. Picking
    // endpoints and assessing selector error in internal space keeps the
    // encoder consistent with what hardware reconstructs; we re-verify
    // against the original f16 only as the final tolerance gate (the test).
    let mut internals = [[0u32; 3]; 16];
    for (i, t) in texels.iter().enumerate() {
        for c in 0..3 {
            internals[i][c] = f16_to_internal(t[c]);
        }
    }

    let mut min = [u32::MAX; 3];
    let mut max = [0u32; 3];
    for t in &internals {
        for c in 0..3 {
            min[c] = min[c].min(t[c]);
            max[c] = max[c].max(t[c]);
        }
    }

    // Initial endpoint guess: quantize each channel's internal-space
    // [min, max] to a 10-bit endpoint via the inverse of the `unq` formula.
    let mut ep0 = [0u16; 3];
    let mut ep1 = [0u16; 3];
    for c in 0..3 {
        ep0[c] = quantize_internal_to_10(min[c]);
        ep1[c] = quantize_internal_to_10(max[c]);
    }

    let palette = build_internal_palette(ep0, ep1);
    let mut selectors = [0u8; 16];
    for (i, t) in internals.iter().enumerate() {
        selectors[i] = best_index_internal(t, &palette) as u8;
    }

    // Mode 11's fixup index (texel 0) stores 3 bits, not 4 — its top bit is
    // implicitly 0. If the best selector for texel 0 is ≥ 8, swap the
    // endpoints (and complement every selector) so texel 0 lands in [0, 7]
    // while every block sample reconstructs identically. The swap is sound
    // because BC6H_WEIGHTS_4 is symmetric around 32 (w[15-i] = 64 - w[i]),
    // so swapping ep0↔ep1 and replacing every selector s with 15-s leaves
    // the interpolated value unchanged.
    if selectors[0] >= 8 {
        std::mem::swap(&mut ep0, &mut ep1);
        for s in &mut selectors {
            *s = 15 - *s;
        }
    }

    pack_mode11_block(ep0, ep1, selectors)
}

/// Inverse of the BC6H UFloat finalize step. Decode runs
/// `output_f16 = (interp * 31) >> 6`, so to land on a given `output_f16` the
/// pre-finalize internal value must satisfy `interp ≈ output_f16 * 64 / 31`.
/// Saturate at the 16-bit internal max (`0xffff`); larger f16 values (infinity
/// and beyond f16 max-normal `0x7bff`) are already meaningless for the
/// irradiance bake and decoded with the saturated endpoint.
fn f16_to_internal(v: u16) -> u32 {
    let raised = (v as u32 * 64 + 15) / 31; // round-to-nearest
    raised.min(0xffff)
}

/// Round-to-nearest quantization from a 16-bit internal value to a 10-bit
/// endpoint — the exact inverse of `unq`. With `unq(0) = 0`,
/// `unq(0x3ff) = 0xffff`, and `unq(x) = 64·x + 32` for `x ∈ 1..=0x3fe`, the
/// midpoint between `unq(0) = 0` and `unq(1) = 96` is 48, and the midpoint
/// between `unq(0x3fe) = 65440` and `unq(0x3ff) = 65535` is 65487.5. Inside
/// the regular linear range, `round((v - 32) / 64) = v >> 6`.
///
/// The `quantize_inverts_unq_across_10bit_range` test asserts this matches a
/// brute-force nearest-neighbor search across the full 0..=0x3ff grid.
fn quantize_internal_to_10(v: u32) -> u16 {
    if v <= 48 {
        return 0;
    }
    if v >= 65488 {
        return 0x3ff;
    }
    (v >> 6).clamp(1, 0x3fe) as u16
}

/// Unquantize a 10-bit endpoint to the 16-bit internal value used in the
/// hardware interpolation. Implements the DirectX / Khronos `Bc6hRgbUfloat`
/// spec for `epb = 10`: endpoint specials at `x = 0` (→ 0) and `x = 0x3ff`
/// (→ `0xffff`); for the rest, `((x << 15) + 0x4000) >> 9`, which is the
/// exact integer `64·x + 32`. (Earlier code here used BC7-style
/// bit-replication `(x << 6) | (x >> 4)` — that matches at the endpoints
/// but diverges mid-range and would optimize the encoder against the wrong
/// reconstruction.)
fn unq(x: u16) -> u32 {
    let x = x as u32;
    if x == 0 {
        0
    } else if x == 0x3ff {
        0xffff
    } else {
        64 * x + 32
    }
}

/// Build the 16-entry per-channel internal-space palette the hardware
/// interpolator produces from `(ep0, ep1)` and the 4-bit index weights, *before*
/// the UFloat finalize step. Selector picking compares in this space because
/// the finalize step is a monotonic linear scale — picking the closest
/// internal value also picks the closest output f16.
fn build_internal_palette(ep0: [u16; 3], ep1: [u16; 3]) -> [[u32; 16]; 3] {
    let mut palette = [[0u32; 16]; 3];
    for c in 0..3 {
        let u0 = unq(ep0[c]);
        let u1 = unq(ep1[c]);
        for (i, &w) in BC6H_WEIGHTS_4.iter().enumerate() {
            palette[c][i] = ((64 - w) * u0 + w * u1 + 32) >> 6;
        }
    }
    palette
}

/// Pick the palette index whose reconstructed RGB internal value minimizes
/// squared error against the input texel's internal value. Internal-space
/// distance is the right metric for selector choice (see `build_internal_palette`).
fn best_index_internal(texel: &[u32; 3], palette: &[[u32; 16]; 3]) -> usize {
    let mut best_idx = 0usize;
    let mut best_err = u64::MAX;
    for (i, ((&r, &g), &b)) in palette[0]
        .iter()
        .zip(palette[1].iter())
        .zip(palette[2].iter())
        .enumerate()
    {
        let dr = r as i64 - texel[0] as i64;
        let dg = g as i64 - texel[1] as i64;
        let db = b as i64 - texel[2] as i64;
        let err = (dr * dr + dg * dg + db * db) as u64;
        if err < best_err {
            best_err = err;
            best_idx = i;
        }
    }
    best_idx
}

/// Pack Mode 11 endpoint + selector data into the 128-bit BC6H block.
fn pack_mode11_block(ep0: [u16; 3], ep1: [u16; 3], selectors: [u8; 16]) -> [u8; 16] {
    let mut bits = 0u128;
    let mut cursor = 0u32;
    // mode = 0b00011 (5 bits)
    write_bits(&mut bits, &mut cursor, 0b00011, 5);
    // Endpoints, RGB pairs interleaved as (r0, g0, b0, r1, g1, b1), 10 bits each.
    for &v in &ep0 {
        write_bits(&mut bits, &mut cursor, v as u128, 10);
    }
    for &v in &ep1 {
        write_bits(&mut bits, &mut cursor, v as u128, 10);
    }
    // Indices: texel 0 is 3 bits (fixup), texels 1..=15 are 4 bits each.
    write_bits(&mut bits, &mut cursor, (selectors[0] & 0x7) as u128, 3);
    for sel in &selectors[1..] {
        write_bits(&mut bits, &mut cursor, (*sel & 0xf) as u128, 4);
    }
    debug_assert_eq!(
        cursor, 128,
        "BC6H Mode 11 block must pack to exactly 128 bits"
    );

    bits.to_le_bytes()
}

fn write_bits(bits: &mut u128, cursor: &mut u32, value: u128, count: u32) {
    *bits |= (value & ((1u128 << count) - 1)) << *cursor;
    *cursor += count;
}

/// Frozen regression guard for the BC6H round-trip test. Measured peak
/// relative error on the synthetic gradient is ~0.055; the tolerance is set
/// with headroom under the "re-pin when measured drops below ½× current"
/// convention, so harmless encoder noise won't trip the test but a real
/// quality regression will.
///
/// Lifted to module scope (out of `mod tests`) so the determinism test in
/// `lightmap_bake.rs` can gate against the *same* frozen threshold rather
/// than picking its own — keeping the plan's "Both ACs gate against the same
/// frozen threshold" contract on a single constant.
#[cfg(test)]
pub(crate) const BC6H_ROUNDTRIP_TOLERANCE_REL: f32 = 0.08;

/// The 5-bit mode field of the one BC6H mode this module encodes ("Mode 11").
const MODE_11_BITS: u8 = 0b00011;

/// A BC6H payload the CPU decoder cannot reproduce. The bytes come from disk
/// (lightmap section id 22), so a malformed or foreign payload is an error to
/// surface, not a panic.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Bc6hDecodeError {
    #[error(
        "BC6H block uses mode bits {mode_bits:#07b}; only Mode 11 (0b00011), \
         the mode prl-build encodes, decodes on the CPU"
    )]
    UnsupportedMode { mode_bits: u8 },
    #[error(
        "BC6H payload for {width}×{height} texels must be {expected} bytes \
         (16 per 4×4 block), found {actual}"
    )]
    PayloadLength {
        width: u32,
        height: u32,
        expected: usize,
        actual: usize,
    },
}

/// Decode a `Bc6hRgbUfloat` payload written by
/// [`encode_bc6h_rgb_from_f32_rgba`] back to row-major RGBA f32 (A = 1.0, as
/// hardware returns for BC6H), reproducing hardware decode bit for bit.
///
/// Blocks are row-major, `ceil(width/4)·ceil(height/4)` of them; texels a
/// partial edge block covers past `width`/`height` are dropped. Every block
/// must be Mode 11 — the only mode the encoder emits — or the decode fails.
#[cfg(test)]
pub fn decode_bc6h_rgb_to_f32_rgba(
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<f32>, Bc6hDecodeError> {
    let blocks_x = width.div_ceil(4) as usize;
    let blocks_y = height.div_ceil(4) as usize;
    let expected = blocks_x * blocks_y * 16;
    if bytes.len() != expected {
        return Err(Bc6hDecodeError::PayloadLength {
            width,
            height,
            expected,
            actual: bytes.len(),
        });
    }

    let (width, height) = (width as usize, height as usize);
    let mut out = vec![0.0f32; width * height * 4];
    for (block_index, block) in bytes.as_chunks::<16>().0.iter().enumerate() {
        let texels = decode_bc6h_block(block)?;
        let bx = (block_index % blocks_x) * 4;
        let by = (block_index / blocks_x) * 4;
        for (k, texel) in texels.iter().enumerate() {
            let (px, py) = (bx + k % 4, by + k / 4);
            if px >= width || py >= height {
                continue;
            }
            let base = (py * width + px) * 4;
            out[base..base + 3].copy_from_slice(texel);
            out[base + 3] = 1.0;
        }
    }
    Ok(out)
}

/// Decode one 16-byte BC6H Mode 11 block to its 16 RGB texels, row-major
/// within the block (texel `k` sits at `(k % 4, k / 4)`). Exact to hardware:
/// `unq` endpoints, the 4-bit weight table, then the UFloat finalize to f16.
/// The capture stage reads a bilinear footprint per hit and decodes only the
/// blocks its taps land in.
pub fn decode_bc6h_block(block: &[u8; 16]) -> Result<[[f32; 3]; 16], Bc6hDecodeError> {
    let bits = u128::from_le_bytes(*block);
    let mut cursor = 0u32;
    // Two-bit modes (low bits 0b00 / 0b01) read as their 5-bit prefix here
    // too; none of them is Mode 11, so one compare rejects every other mode.
    let mode_bits = read_bits(bits, &mut cursor, 5) as u8;
    if mode_bits != MODE_11_BITS {
        return Err(Bc6hDecodeError::UnsupportedMode { mode_bits });
    }
    let mut ep0 = [0u16; 3];
    let mut ep1 = [0u16; 3];
    for slot in &mut ep0 {
        *slot = read_bits(bits, &mut cursor, 10) as u16;
    }
    for slot in &mut ep1 {
        *slot = read_bits(bits, &mut cursor, 10) as u16;
    }
    let mut indices = [0u8; 16];
    indices[0] = read_bits(bits, &mut cursor, 3) as u8;
    for slot in indices.iter_mut().skip(1) {
        *slot = read_bits(bits, &mut cursor, 4) as u8;
    }
    let palette = build_internal_palette(ep0, ep1);
    let mut out = [[0.0f32; 3]; 16];
    for (texel_out, &idx) in out.iter_mut().zip(indices.iter()) {
        for (c, channel_out) in texel_out.iter_mut().enumerate() {
            // UFloat finalize: at most (0xffff · 31) >> 6 = 0x7bff, the largest
            // finite f16, so the decode never yields infinity or NaN.
            let half = ((palette[c][idx as usize] * 31) >> 6) as u16;
            *channel_out = f16_bits_to_f32(half);
        }
    }
    Ok(out)
}

fn read_bits(bits: u128, cursor: &mut u32, count: u32) -> u128 {
    let value = (bits >> *cursor) & ((1u128 << count) - 1);
    *cursor += count;
    value
}

/// Half-float bit pattern → f32, exact for every f16 value. Shared by the BC6H
/// decoder and the reflection-probe samplers that read f16 sections.
pub(crate) fn f16_bits_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 0x1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            let mut m = mant;
            let mut e: i32 = -14;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x3ff;
            (sign << 31) | (((e + 127) as u32) << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (mant << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Color-correlated HDR penumbra: RGB share an intensity ramp tinted by
    /// fixed scaling factors — the same shape a single static light's
    /// soft-shadow ramp produces in irradiance space. Per-texel variation is
    /// held under ~0.15 units (the rate real lightmap content varies at the
    /// default 0.04 m/texel density), so per-block [min, max] stays under ~0.6
    /// units and the channels co-vary, letting the single-mode encoder's
    /// shared-selector picks resolve each block within tolerance. (Synthetic
    /// per-channel independent gradients would push higher error because the
    /// 16 shared selectors can't simultaneously approximate diverging
    /// per-channel fractions — a known limitation of the no-partition mode
    /// and exactly why BC6H also defines 2-partition modes. Real irradiance
    /// does not exercise that case.) `scale(y)` multiplies each row.
    fn penumbra_gradient(w: u32, h: u32, scale: impl Fn(u32) -> f32) -> Vec<f32> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let intensity = (0.5 + (x as f32) * 0.1 + (y as f32) * 0.05) * scale(y);
                rgba.extend_from_slice(&[intensity * 1.0, intensity * 0.85, intensity * 1.2, 1.0]);
            }
        }
        rgba
    }

    /// Encode then decode through the production decoder; every RGB channel
    /// must reproduce within `BC6H_ROUNDTRIP_TOLERANCE_REL` and A reads 1.0.
    fn assert_roundtrip_within_frozen_tolerance(rgba: &[f32], w: u32, h: u32) {
        let blocks = encode_bc6h_rgb_from_f32_rgba(rgba, w, h);
        assert_eq!(blocks.len(), ((w / 4) * (h / 4) * 16) as usize);
        let decoded = decode_bc6h_rgb_to_f32_rgba(&blocks, w, h).expect("encoder emits Mode 11");
        assert_eq!(decoded.len(), rgba.len());
        for py in 0..h {
            for px in 0..w {
                let base = ((py * w + px) * 4) as usize;
                for c in 0..3 {
                    let original = rgba[base + c].max(0.0);
                    let decoded_f = decoded[base + c];
                    let denom = original.abs().max(1.0e-3);
                    let rel = (decoded_f - original).abs() / denom;
                    assert!(
                        rel <= BC6H_ROUNDTRIP_TOLERANCE_REL,
                        "texel ({px}, {py}) c{c}: relative error {rel} > tolerance {BC6H_ROUNDTRIP_TOLERANCE_REL} (orig {original}, decoded {decoded_f})",
                    );
                }
                assert_eq!(decoded[base + 3], 1.0, "texel ({px}, {py}) alpha");
            }
        }
    }

    /// Synthetic HDR gradient: smooth low-frequency irradiance, the case the
    /// bake actually produces. Encoded to BC6H and decoded through the
    /// production hardware-matching decoder, every texel must reproduce
    /// within `BC6H_ROUNDTRIP_TOLERANCE_REL`.
    #[test]
    fn bc6h_roundtrip_within_frozen_relative_tolerance() {
        let (w, h) = (16, 16);
        assert_roundtrip_within_frozen_tolerance(&penumbra_gradient(w, h, |_| 1.0), w, h);
    }

    /// The same penumbra at irradiance far above 1.0: each 4×4 block row is
    /// scaled by a power of two up to 4096 (peak ≈ 13.5k, under f16's 65504),
    /// so every block exercises a higher f16 exponent range. A power-of-two
    /// scale shifts f16 exponents only, so the frozen tolerance carries over
    /// unchanged (measured peak equals the unscaled gradient's ~0.055).
    #[test]
    fn bc6h_roundtrip_reproduces_hdr_irradiance_within_frozen_tolerance() {
        const ROW_SCALES: [f32; 4] = [1.0, 16.0, 256.0, 4096.0];
        let (w, h) = (16, 16);
        let rgba = penumbra_gradient(w, h, |y| ROW_SCALES[(y / 4) as usize]);
        assert_roundtrip_within_frozen_tolerance(&rgba, w, h);
    }

    /// A constant-value block must reproduce its input within the tolerance.
    /// Degenerate-endpoint blocks were a likely failure mode pre-fixup-handling;
    /// keeping the test as a quick visible sanity gate for that corner.
    #[test]
    fn bc6h_constant_block_within_tolerance() {
        let mut rgba = Vec::with_capacity(64);
        for _ in 0..16 {
            rgba.push(1.5_f32);
            rgba.push(0.75);
            rgba.push(2.25);
            rgba.push(1.0);
        }
        let blocks = encode_bc6h_rgb_from_f32_rgba(&rgba, 4, 4);
        let block: [u8; 16] = blocks[..16].try_into().unwrap();
        let decoded = decode_bc6h_block(&block).expect("encoder emits Mode 11");
        for &[r, g, b] in decoded.iter() {
            assert!((r - 1.5).abs() / 1.5 <= BC6H_ROUNDTRIP_TOLERANCE_REL);
            assert!((g - 0.75).abs() / 0.75 <= BC6H_ROUNDTRIP_TOLERANCE_REL);
            assert!((b - 2.25).abs() / 2.25 <= BC6H_ROUNDTRIP_TOLERANCE_REL);
        }
    }

    /// Lightmap bytes come from disk; a block in any mode the encoder never
    /// writes is refused, by the block decoder and by the payload decoder.
    /// Covers a two-bit mode (Mode 1, low bits 0b00) and a five-bit mode
    /// (Mode 12, 0b00111) beside the supported 0b00011.
    #[test]
    fn bc6h_decode_refuses_blocks_outside_mode_11() {
        let mode_1 = [0u8; 16];
        let mut mode_12 = [0u8; 16];
        mode_12[0] = 0b00111;
        assert_eq!(
            decode_bc6h_block(&mode_1),
            Err(Bc6hDecodeError::UnsupportedMode { mode_bits: 0b00000 })
        );
        assert_eq!(
            decode_bc6h_block(&mode_12),
            Err(Bc6hDecodeError::UnsupportedMode { mode_bits: 0b00111 })
        );

        // One foreign block among valid ones fails the whole payload.
        let rgba = vec![0.5_f32; 8 * 4 * 4];
        let mut payload = encode_bc6h_rgb_from_f32_rgba(&rgba, 8, 4);
        assert!(decode_bc6h_rgb_to_f32_rgba(&payload, 8, 4).is_ok());
        payload[16..32].copy_from_slice(&mode_12);
        assert_eq!(
            decode_bc6h_rgb_to_f32_rgba(&payload, 8, 4),
            Err(Bc6hDecodeError::UnsupportedMode { mode_bits: 0b00111 })
        );
    }

    /// A payload whose length disagrees with its stated extent (truncated on
    /// disk, or paired with the wrong block's dimensions) is refused rather
    /// than read short or past its end.
    #[test]
    fn bc6h_decode_refuses_payload_length_mismatch() {
        let rgba = vec![0.5_f32; 8 * 8 * 4];
        let payload = encode_bc6h_rgb_from_f32_rgba(&rgba, 8, 8);
        assert_eq!(
            decode_bc6h_rgb_to_f32_rgba(&payload[..48], 8, 8),
            Err(Bc6hDecodeError::PayloadLength {
                width: 8,
                height: 8,
                expected: 64,
                actual: 48,
            })
        );
    }

    /// Anchors the `unq` spec formula at a representative table of inputs:
    /// the two endpoint specials, the first non-trivial value (where BC7-style
    /// bit-replication would diverge most visibly), the mid-range, and the
    /// last regular value before the upper special. Reference values use the
    /// spec's bit expression `((x << 15) + 0x4000) >> 9` directly; the closed
    /// integer form `64·x + 32` is what `unq` implements.
    #[test]
    fn unq_matches_bc6h_ufloat_spec_table() {
        assert_eq!(super::unq(0x000), 0x0000);
        assert_eq!(super::unq(0x001), 96);
        assert_eq!(super::unq(0x002), 160);
        assert_eq!(super::unq(512), 64 * 512 + 32);
        assert_eq!(super::unq(1022), 65440);
        assert_eq!(super::unq(0x3ff), 0xffff);
        // Spot-check the closed form against the spec's bit expression for
        // every x in the regular range; cheap and pins the algebra.
        for x in 1u16..0x3ff {
            let spec = (((x as u32) << 15) + 0x4000) >> 9;
            assert_eq!(super::unq(x), spec, "spec mismatch at x={x}");
        }
    }

    /// `quantize_internal_to_10` must be the exact inverse of `unq` on every
    /// 10-bit input — i.e. round-trip through the encoder's endpoint
    /// quantization is lossless on the lattice the hardware actually uses.
    #[test]
    fn quantize_inverts_unq_across_10bit_range() {
        for x in 0u16..=0x3ff {
            let v = super::unq(x);
            assert_eq!(
                super::quantize_internal_to_10(v),
                x,
                "quantize_internal_to_10(unq({x})) != {x} (v={v})"
            );
        }
    }

    /// `quantize_internal_to_10`'s choice must be among the brute-force
    /// nearest-neighbor minima across the 10-bit grid for every reachable
    /// internal value (0..=0xffff). Pins the closed-form midpoint logic
    /// against an obviously correct reference.
    ///
    /// The spec's `unq` grid has two kinds of exact midpoints where multiple
    /// 10-bit endpoints tie for minimum absolute error: boundary ties at
    /// v=48 (`|unq(0)-48| = |unq(1)-48| = 48`) and v=65487
    /// (`|unq(0x3fe)-v| = |unq(0x3ff)-v| ≈ 47`), and interior ties at every
    /// v = 64·x + 64 for x ∈ 1..=0x3fd (`unq(x) = 64x+32`,
    /// `unq(x+1) = 64x+96`, midpoint exactly between). Choice among tied
    /// candidates is loss-equivalent — both produce the same hardware
    /// reconstruction error — so the test asserts membership in the tie set
    /// rather than a specific tie-break, while still pinning the closed form
    /// against an external nearest-neighbor reference.
    #[test]
    fn quantize_matches_brute_force_nearest_neighbor() {
        fn brute_min_err(v: u32) -> i64 {
            let mut best_err = i64::MAX;
            for x in 0u16..=0x3ff {
                let err = (super::unq(x) as i64 - v as i64).abs();
                if err < best_err {
                    best_err = err;
                }
            }
            best_err
        }
        for v in 0u32..=0xffff {
            let chosen = super::quantize_internal_to_10(v);
            let chosen_err = (super::unq(chosen) as i64 - v as i64).abs();
            let min_err = brute_min_err(v);
            assert_eq!(
                chosen_err, min_err,
                "closed-form chose x={chosen} (err {chosen_err}) at v={v}; brute force min err {min_err}",
            );
        }
    }

    /// Block size matches the spec's `ceil(w/4)·ceil(h/4)·16`. The bake's
    /// `irr_len` accounting (`crates/level-format/src/lightmap.rs` header) and
    /// the runtime upload both consume this sizing; a wrong block byte count
    /// here would mis-size the on-disk blob.
    #[test]
    fn bc6h_block_count_matches_block_math() {
        let w = 16u32;
        let h = 8u32;
        let rgba = vec![0.5_f32; (w * h * 4) as usize];
        let blocks = encode_bc6h_rgb_from_f32_rgba(&rgba, w, h);
        let expected = ((w / 4) * (h / 4) * 16) as usize;
        assert_eq!(blocks.len(), expected);
    }
}
