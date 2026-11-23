// Glossy world faces' reflection weight and colour, and the overlay's composite: the GPU-free reference for `glossy_overlay.wgsl`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces) · context/lib/resource_management.md §4.7

use glam::{Vec3, Vec4};

pub use postretro_level_format::glossy_opt_in::{
    GLOSSY_FULL_WEIGHT_EXPONENT, GLOSSY_SHARPNESS_FLOOR_EXPONENT, glossy_sharpness_fade,
};

use crate::fluid_surface::reflection::{FLUID_REFLECTIVITY_MAX, reflection_fresnel};
use crate::specular_texel::{SPECULAR_EXPONENT_MIN, texel_exponent, tint_amount, tint_color};

// `glossy_overlay.wgsl` mirrors each function and constant below statement for
// statement; `render/glossy_overlay_tests.rs` pins them. The floor layer
// (screen-space, up-facing faces) reuses `glossy_texel`, `glossy_fresnel` and
// `glossy_composite` with its own (c·X, c).

/// An untinted texel's reflectivity on water's reflectance scale
/// (`FLUID_REFLECTION_BASE_F0` × reflectivity): a head-on base of 0.04.
pub const GLOSSY_REFLECTIVITY_UNTINTED: f32 = 2.0;
/// A tint-1 texel's: water's mirror cap, a head-on base of 1.
pub const GLOSSY_REFLECTIVITY_FULL_TINT: f32 = FLUID_REFLECTIVITY_MAX;

/// What one read of a texel's material says about its reflection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlossyTexel {
    /// `_s` × the sharpness fade: the share of the Fresnel weight the texel
    /// takes, 0 for a texel without `_s` or at or below the sharpness floor.
    pub mask: f32,
    /// Water's reflectivity for this texel: untinted (or without `_t`) at
    /// [`GLOSSY_REFLECTIVITY_UNTINTED`], raised by the tint toward
    /// [`GLOSSY_REFLECTIVITY_FULL_TINT`].
    pub reflectivity: f32,
    /// The colour the reflection takes, as the texel's highlight does: the
    /// tint rule's colour for a tinted texel, the raw albedo without `_t`.
    pub colour: Vec3,
    /// The texel's Blinn-Phong exponent, the one its highlight shades at:
    /// it picks the colour mips the probe term reads
    /// (`reflection_probe_mips::reflection_probe_mips`).
    pub exponent: f32,
}

/// A texel's reflection inputs from its specular slot sample `slot` (R `_s`,
/// B smoothness, A tint) and linear albedo, under the tint rule
/// `specular-texel-sharpness` owns. A texel without smoothness shades at the
/// minimum exponent, below the floor, so it reflects nothing.
pub fn glossy_texel(slot: Vec4, albedo: Vec3) -> GlossyTexel {
    let exponent = texel_exponent(slot.z, SPECULAR_EXPONENT_MIN);
    let tint = tint_amount(slot.w);
    GlossyTexel {
        mask: slot.x * glossy_sharpness_fade(exponent),
        reflectivity: match tint {
            // WGSL `mix(e1, e2, t)` = e1·(1 − t) + e2·t.
            Some(t) => GLOSSY_REFLECTIVITY_UNTINTED * (1.0 - t) + GLOSSY_REFLECTIVITY_FULL_TINT * t,
            None => GLOSSY_REFLECTIVITY_UNTINTED,
        },
        colour: match tint {
            Some(t) => Vec3::from(tint_color(albedo.to_array(), t)),
            None => albedo,
        },
        exponent,
    }
}

/// F, the texel's unshadowed weight: Schlick Fresnel on water's reflectance
/// scale at the texel's reflectivity, times its mask (`_s` × sharpness fade).
/// `cos_theta` is between the fragment-to-eye vector and the face normal.
pub fn glossy_fresnel(texel: &GlossyTexel, cos_theta: f32) -> f32 {
    reflection_fresnel(cos_theta, texel.reflectivity) * texel.mask
}

/// The overlay's signed addition over forward's colour at one glossy texel:
/// `F·colour·[c·X + (1 − c)·s·R] − W·D_static`, with `W = F·[c + (1 − c)·s]`
/// capped at 1, so the output is forward + W·(R − D_static) with R coloured as
/// the highlight. `static_diffuse` is forward's D_static at the pixel,
/// `probe` the probe term's capture (live-lit), `shadow` its capture-shadow
/// weight s, `screen` the floor layer's premultiplied `(c·X, c)` (zero where
/// no screen-space layer runs: the probe term alone at weight F·s).
///
/// Where s = 0 and c = 0 it adds exactly nothing: the capture shadow is
/// withheld, never filled. W ≤ 1 and non-negative R keep the output at or
/// above forward − D_static, which is at or above 0 while D_static is stored
/// no larger than forward's colour.
///
/// IP rule (owner review to change, rendering_pipeline.md §7.3a): s scales the
/// probe's share alone; the screen-space contribution `F·colour·c·X` is the
/// same at every s.
pub fn glossy_composite(
    static_diffuse: Vec3,
    probe: Vec3,
    shadow: f32,
    screen: Vec4,
    fresnel: f32,
    colour: Vec3,
) -> Vec3 {
    let confidence = screen.w;
    let probe_share = (1.0 - confidence) * shadow;
    let reflected = colour * (fresnel * (screen.truncate() + probe * probe_share));
    let weight = (fresnel * (confidence + probe_share)).min(1.0);
    reflected - static_diffuse * weight
}

#[cfg(test)]
#[path = "glossy_reflection_tests.rs"]
mod tests;
