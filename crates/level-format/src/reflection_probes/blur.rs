// Reflection probe colour mips: the one exponent → mip function the compiler prefilter, the glossy overlay and render-cpu share.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces) · context/lib/resource_management.md §5

use super::{REFLECTION_PROBE_MIN_MIP_DIMENSION, REFLECTION_PROBE_TILE_DIMENSION};

/// α₀, the mip-0 interior texel's angle in radians: 180° over the 64 texels
/// from pole to antipode of the engine tile. Mip m's texel is α₀·2^m.
pub const REFLECTION_PROBE_MIP0_TEXEL_ANGLE: f32 =
    std::f32::consts::PI / (REFLECTION_PROBE_TILE_DIMENSION / 2) as f32;
/// m_min, the minimum blur: no glossy world face reads sharper than this
/// mip (the 64² tile, 5.625° texels; n ≥ ~576 reads it), so no world face is
/// a mirror. Water reads mip 0 and never this function.
pub const REFLECTION_PROBE_MIN_BLUR_MIP: f32 = 1.0;

/// The reflected-direction half-width at half maximum of a Blinn-Phong lobe
/// of `exponent`, in radians: θ(n) = 2·acos(0.5^(1/n)). Rotating the lookup
/// by φ turns the half vector by φ/2, so the reflected lobe is twice the
/// half-vector one.
pub fn reflection_lobe_half_width(exponent: f32) -> f32 {
    2.0 * 0.5f32.powf(1.0 / exponent).acos()
}

/// The colour mip a texel of `exponent` reads, fractional between two mips:
/// m(n) = log₂(θ(n) / α₀), at least [`REFLECTION_PROBE_MIN_BLUR_MIP`] and at
/// most the last of `colour_mip_count` mips, the last winning when the chain
/// is shorter than the minimum blur. Blur grows monotonically as the
/// exponent falls. `glossy_overlay.wgsl`'s `reflection_probe_exponent_mip`
/// mirrors it operation for operation.
pub fn reflection_probe_exponent_mip(exponent: f32, colour_mip_count: u32) -> f32 {
    let last = colour_mip_count.saturating_sub(1) as f32;
    let mip = (reflection_lobe_half_width(exponent) / REFLECTION_PROBE_MIP0_TEXEL_ANGLE).log2();
    mip.max(REFLECTION_PROBE_MIN_BLUR_MIP).min(last)
}

/// The half-width mip `mip` is prefiltered to: α₀·2^m, the inverse of
/// [`reflection_probe_exponent_mip`] before its clamp.
pub fn reflection_probe_mip_half_width(mip: u32) -> f32 {
    REFLECTION_PROBE_MIP0_TEXEL_ANGLE * (mip as f32).exp2()
}

/// The probe term's blur at `exponent`, as a reflected-direction half-width
/// at half maximum in radians: the half-width of the (fractional) mip the
/// texel reads, α₀·2^m(n). Between the minimum blur and the last mip it is
/// the texel's own lobe θ(n); sharper texels read the minimum blur's, and
/// rougher ones the last mip's. The floor screen-space layer's spread is
/// held against it (acceptance row 36).
pub fn reflection_probe_term_half_width(exponent: f32, colour_mip_count: u32) -> f32 {
    REFLECTION_PROBE_MIP0_TEXEL_ANGLE
        * reflection_probe_exponent_mip(exponent, colour_mip_count).exp2()
}

/// The exponent whose lobe the compiler prefilters mip `mip` with: the n
/// with θ(n) = α₀·2^m, so `reflection_probe_exponent_mip` maps it back to
/// `mip` (unclamped). From 0.5^(1/n) = cos(θ/2): n = ln 0.5 / ln cos(θ/2).
/// Defined while θ < 180°, every mip of the engine chain.
pub fn reflection_probe_mip_exponent(mip: u32) -> f32 {
    let half = 0.5 * reflection_probe_mip_half_width(mip);
    0.5f32.ln() / half.cos().ln()
}

/// Mips of a `tile_dimension` tile's full chain: halving down to one 4×4
/// block (six for the engine's 128² tile).
pub fn reflection_probe_full_colour_mip_count(tile_dimension: u32) -> u32 {
    (tile_dimension / REFLECTION_PROBE_MIN_MIP_DIMENSION).ilog2() + 1
}

#[cfg(test)]
#[path = "blur_tests.rs"]
mod tests;
