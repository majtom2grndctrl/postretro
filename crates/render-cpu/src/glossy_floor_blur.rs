// The glossy floor layer's blur at the march's hit: the scene-colour chain level a hit reads, the GPU-free reference for `glossy_floor_blur.wgsl`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors) · context/plans/in-progress/glossy-world-reflections/ip-check.md §7

use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_MIP0_TEXEL_ANGLE, reflection_probe_exponent_mip,
};

// `glossy_floor_blur.wgsl` mirrors each function and constant below statement
// for statement; `render/glossy_floor_march_tests.rs` pins them.
//
// IP rule (owner review to change, `ip-check.md` B1): the level reads the
// texel's exponent, the minimum blur (inside the probe term's exponent → mip
// function), the hit length and the hit's view depth alone, with the camera's
// focal length and the chain's scale. Never s, the distance tile, a
// capture-shadow flag, probe colour or the hit's confidence: it never widens,
// renormalises or fills where the probe term is withheld.

/// Colour mips of the engine probe tile's full chain, the chain every probe
/// of a section that binds a world face carries: the probe term's blur is
/// read over it, its last mip included.
pub const GLOSSY_FLOOR_PROBE_MIPS: u32 = 6;

/// The widest cone half-angle a hit's footprint takes, radians (about 86°):
/// keeps its tangent finite and positive at the probe chain's last mip
/// (90°). Every texel that reflects (exponent above the sharpness floor,
/// half-width under 48°) lies below it.
pub const GLOSSY_FLOOR_WIDEST_HALF_ANGLE: f32 = 1.5;

/// The nearest hit view depth the footprint divides by, metres: a hit is
/// in front of the near plane, so this only keeps the division finite.
pub const GLOSSY_FLOOR_NEAREST_DEPTH: f32 = 1.0e-3;

/// The cone half-angle a texel of `exponent` blurs its hit by: the probe
/// term's blur at that exponent (`reflection_probe_term_half_width` over the
/// full chain, its minimum blur included), capped at
/// [`GLOSSY_FLOOR_WIDEST_HALF_ANGLE`].
pub fn glossy_floor_blur_half_angle(exponent: f32) -> f32 {
    (REFLECTION_PROBE_MIP0_TEXEL_ANGLE
        * reflection_probe_exponent_mip(exponent, GLOSSY_FLOOR_PROBE_MIPS).exp2())
    .min(GLOSSY_FLOOR_WIDEST_HALF_ANGLE)
}

/// The scene-colour chain level a hit reads, fractional: the footprint of a
/// cone of the texel's blur half-angle ([`glossy_floor_blur_half_angle`])
/// over `hit_length` metres of the reflected ray, projected to the screen at
/// the hit's view depth `hit_depth`, so the blur widens with hit length as the
/// probe term's angular blur does. `focal` is scene pixels per unit of
/// tangent at depth 1 (the projection's y scale × half the scene height),
/// `chain_scale` the chain's level-0 texels per scene pixel, `levels` its
/// level count. A level's texel spans the footprint's full width at half
/// maximum (twice the half-width), clamped to the chain.
pub fn glossy_floor_blur_level(
    exponent: f32,
    hit_length: f32,
    hit_depth: f32,
    focal: f32,
    chain_scale: f32,
    levels: u32,
) -> f32 {
    let half_width = glossy_floor_blur_half_angle(exponent).tan() * hit_length * focal
        / hit_depth.max(GLOSSY_FLOOR_NEAREST_DEPTH);
    // WGSL `clamp(e, low, high)` = min(max(e, low), high); log2(0) is −∞.
    (2.0 * half_width * chain_scale)
        .log2()
        .max(0.0)
        .min(levels.saturating_sub(1) as f32)
}

#[cfg(test)]
#[path = "glossy_floor_blur_tests.rs"]
mod tests;
