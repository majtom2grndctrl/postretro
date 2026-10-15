// Fluid reflection style: where a fluid fragment evaluates its probe
// reflection from. Smooth reflects the flat face per fragment; pixel reflects
// once per water texel cell, from the cell's world-space centre, about a
// normal tilted by the cell's displayed offset. GPU-free CPU reference for
// `fluid_surface.wgsl`'s style functions, which mirror it and a test pins.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection → Style)

use glam::{IVec2, Vec2, Vec3};

use super::motion::{displayed_offset, texel_cell};

/// `FluidSceneInputs.reflection_style` for the pixel style: the uniform's
/// zero, so a renderer before any write, and frame capture, draw pixel.
pub const FLUID_REFLECTION_STYLE_PIXEL: u32 = 0;
/// `FluidSceneInputs.reflection_style` for the smooth style. Any other value
/// reads as pixel.
pub const FLUID_REFLECTION_STYLE_SMOOTH: u32 = 1;
/// Normal tilt per texel of a cell's displayed offset, as the tangent of the
/// tilt angle: 0.05 tilts the normal about 2.9° per texel, so the reflected
/// ray turns about 5.7°, four to five texels of a 128² probe tile. A cell
/// whose shimmer or ring moves it one texel therefore reads a visibly
/// different part of the room, while the largest displayed offset (two
/// texels on each axis, about 8° of tilt) still reflects that room rather
/// than scrambling it. Engine-owned, like every motion constant.
pub const FLUID_REFLECTION_PIXEL_TILT: f32 = 0.05;
/// Below this squared sine between the two screen-axis texel derivatives the
/// UV footprint has no usable inverse (the face is seen edge-on, or the
/// derivatives vanish): the fragment keeps its own position and flat normal.
pub const FLUID_REFLECTION_AXES_EPSILON: f32 = 1.0e-8;

/// The author's reflection look, `render.reflections.style`. Renderer
/// vocabulary: the app maps the mod profile onto it at one chokepoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FluidReflectionStyle {
    /// One reflection per water texel cell. The default, and frame capture's.
    #[default]
    Pixel,
    /// The flat face per fragment: a still mirror.
    Smooth,
}

impl FluidReflectionStyle {
    /// Every style.
    pub const ALL: [Self; 2] = [Self::Pixel, Self::Smooth];

    /// The value the shader reads from `FluidSceneInputs.reflection_style`.
    pub const fn uniform_value(self) -> u32 {
        match self {
            Self::Pixel => FLUID_REFLECTION_STYLE_PIXEL,
            Self::Smooth => FLUID_REFLECTION_STYLE_SMOOTH,
        }
    }
}

/// The face's world-space step per texel along its texture's `u` and `v`:
/// the texel-to-world Jacobian, columns in metres per texel. Zero on a
/// degenerate footprint.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FluidTexelAxes {
    pub u: Vec3,
    pub v: Vec3,
}

/// Where a fragment's reflection is evaluated from, and about which normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionPoint {
    pub position: Vec3,
    pub normal: Vec3,
}

/// The texel axes from one fragment's screen derivatives of its unwarped
/// texel UV and its world position: `[dP/dx dP/dy] · [dT/dx dT/dy]⁻¹`. The
/// face is planar and its UV map affine, so every fragment of a face recovers
/// the same axes, perspective or not.
pub fn reflection_texel_axes(
    ddx_texel: Vec2,
    ddy_texel: Vec2,
    ddx_world: Vec3,
    ddy_world: Vec3,
) -> FluidTexelAxes {
    let det = ddx_texel.x * ddy_texel.y - ddy_texel.x * ddx_texel.y;
    let scale = ddx_texel.dot(ddx_texel) * ddy_texel.dot(ddy_texel);
    if det * det <= FLUID_REFLECTION_AXES_EPSILON * scale {
        return FluidTexelAxes {
            u: Vec3::ZERO,
            v: Vec3::ZERO,
        };
    }
    FluidTexelAxes {
        u: (ddx_world * ddy_texel.y - ddy_world * ddx_texel.y) / det,
        v: (ddy_world * ddx_texel.x - ddx_world * ddy_texel.x) / det,
    }
}

/// The world-space centre of the texel cell `texel_uv` lies in: the cell the
/// warp floors from the unwarped UV, so drift and the wave never move it.
pub fn reflection_cell_centre(texel_uv: Vec2, world_position: Vec3, axes: FluidTexelAxes) -> Vec3 {
    let to_centre = texel_uv.floor() + Vec2::splat(0.5) - texel_uv;
    world_position + axes.u * to_centre.x + axes.v * to_centre.y
}

/// `v` at unit length, or zero for a zero vector.
pub fn reflection_unit_or_zero(v: Vec3) -> Vec3 {
    let length_sq = v.dot(v);
    if length_sq > 0.0 {
        v * length_sq.sqrt().recip()
    } else {
        Vec3::ZERO
    }
}

/// The cell's reflection normal: the flat outward normal tilted along the
/// texture's axes by the cell's whole-texel displayed offset.
pub fn reflection_cell_normal(outward: Vec3, axes: FluidTexelAxes, offset: IVec2) -> Vec3 {
    let tilt = reflection_unit_or_zero(axes.u) * offset.x as f32
        + reflection_unit_or_zero(axes.v) * offset.y as f32;
    (outward + tilt * FLUID_REFLECTION_PIXEL_TILT).normalize()
}

/// Where the fragment reflects from. Smooth: the fragment itself, about the
/// flat outward normal. Pixel (any other style value): its cell's centre,
/// about the cell's tilted normal, the offset being the albedo's own
/// shimmer-and-ring displayed offset (never drift or the wave), so every
/// fragment of a cell reflects one direction.
pub fn reflection_point(
    style: u32,
    texel_uv: Vec2,
    world_position: Vec3,
    outward: Vec3,
    axes: FluidTexelAxes,
    time: f32,
    texels_per_pixel: f32,
) -> FluidReflectionPoint {
    if style == FLUID_REFLECTION_STYLE_SMOOTH {
        return FluidReflectionPoint {
            position: world_position,
            normal: outward,
        };
    }
    let offset = displayed_offset(texel_cell(texel_uv), time, texels_per_pixel);
    FluidReflectionPoint {
        position: reflection_cell_centre(texel_uv, world_position, axes),
        normal: reflection_cell_normal(outward, axes, offset),
    }
}

#[cfg(test)]
#[path = "reflection_style_tests.rs"]
mod tests;
