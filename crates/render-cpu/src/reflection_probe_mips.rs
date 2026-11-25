// Reflection probe colour mips at runtime: which two mips a glossy texel's exponent reads, each mip's own tile UV, and the GPU-free colour read `glossy_overlay.wgsl` mirrors.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

use glam::{Vec2, Vec3};
use postretro_level_format::reflection_probes::reflection_probe_colour_mip_dimension;
pub use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_MIN_BLUR_MIP, reflection_lobe_half_width, reflection_probe_exponent_mip,
    reflection_probe_mip0_texel_angle, reflection_probe_term_half_width,
};

use crate::fluid_surface::reflection::reflection_tile_uv;

/// The two colour mips a texel reads and the blend between them: `lower`
/// at weight 1 − `blend`, `upper` at `blend`. Mirrors
/// `glossy_overlay_probe_colour`'s selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReflectionProbeMips {
    pub lower: u32,
    pub upper: u32,
    pub blend: f32,
}

/// The mips a texel of `exponent` reads from a `colour_mip_count`-mip chain
/// (at least 1) of `tile_dimension` tiles: the exponent → mip function's
/// value at that tile's α₀, its floor and the next mip, capped at the last.
/// Both inputs are the section's own, as the overlay reads them from the
/// installed array and probe table.
pub fn reflection_probe_mips(
    exponent: f32,
    tile_dimension: u32,
    colour_mip_count: u32,
) -> ReflectionProbeMips {
    let mip = reflection_probe_exponent_mip(
        exponent,
        reflection_probe_mip0_texel_angle(tile_dimension),
        colour_mip_count,
    );
    // WGSL `u32(f)` truncates; `mip` is never negative.
    let lower = mip as u32;
    ReflectionProbeMips {
        lower,
        upper: (lower + 1).min(colour_mip_count.saturating_sub(1)),
        blend: mip - lower as f32,
    }
}

/// Normalized UV of `direction` in colour mip `mip`: the octahedral mapping
/// over that mip's own tile (`tile_dimension >> mip`) and its own 1-texel
/// border. Interiors do not nest (126, 62, … 2 texels at 128²), so one UV
/// cannot address two mips.
pub fn reflection_tile_mip_uv(
    direction: Vec3,
    tile_dimension: u32,
    tile_border: u32,
    mip: u32,
) -> Vec2 {
    reflection_tile_uv(
        direction,
        reflection_probe_colour_mip_dimension(tile_dimension, mip),
        tile_border,
    )
}

/// One colour mip as a linear-filtered, clamp-to-edge sampler reads it at
/// its own level: bilinear between the four texel centres around `uv`.
/// `texels` is the mip's tile row-major, border included, decoded.
pub fn reflection_probe_mip_sample(texels: &[Vec3], dimension: u32, uv: Vec2) -> Vec3 {
    let last = dimension as i32 - 1;
    let position = uv * dimension as f32 - Vec2::splat(0.5);
    let base = position.floor();
    let fraction = position - base;
    let at = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, last) as usize, y.clamp(0, last) as usize);
        texels[y * dimension as usize + x]
    };
    let (x0, y0) = (base.x as i32, base.y as i32);
    let top = at(x0, y0) * (1.0 - fraction.x) + at(x0 + 1, y0) * fraction.x;
    let bottom = at(x0, y0 + 1) * (1.0 - fraction.x) + at(x0 + 1, y0 + 1) * fraction.x;
    top * (1.0 - fraction.y) + bottom * fraction.y
}

/// A glossy texel's probe colour toward `direction`: each of its two mips
/// read at that mip's own UV, blended. `chain[m]` is mip m's tile
/// (`tile_dimension >> m` square, border included); its length is the
/// colour mip count.
pub fn reflection_probe_mip_colour(
    chain: &[&[Vec3]],
    tile_dimension: u32,
    tile_border: u32,
    direction: Vec3,
    exponent: f32,
) -> Vec3 {
    let mips = reflection_probe_mips(exponent, tile_dimension, chain.len() as u32);
    let read = |mip: u32| {
        reflection_probe_mip_sample(
            chain[mip as usize],
            reflection_probe_colour_mip_dimension(tile_dimension, mip),
            reflection_tile_mip_uv(direction, tile_dimension, tile_border, mip),
        )
    };
    // WGSL `mix(a, b, t)` = a·(1 − t) + b·t.
    read(mips.lower) * (1.0 - mips.blend) + read(mips.upper) * mips.blend
}

#[cfg(test)]
#[path = "reflection_probe_mips_tests.rs"]
mod tests;
