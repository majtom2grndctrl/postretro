// Fluid reflection style: where a fluid fragment evaluates its probe
// reflection from. Smooth reflects the flat face per fragment; pixel reflects
// once per water texel cell, from the cell's world-space centre (held to the
// probe's box at a wall rim), about a normal tilted by the cell's displayed
// offset. GPU-free CPU reference for `fluid_reflection_ray.wgsl`'s style
// functions, which mirror it and a test pins.
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

/// The pixel style's rim inset, relative: a pulled cell centre lands this
/// fraction of its horizontal L1 distance from the capture point inside the
/// box, never on the face. On the face the stored wall puts h at 0 give or
/// take its f16 rounding (half an ulp is ≈2.4e-4 relative; on a wall the
/// bilinear filter reconstructs exactly, the sign flips in runs along it),
/// and a ray into that wall leaves its slab at t = 0: the search either
/// takes the capture-shadow branch's box hit partway up the wall or
/// collapses onto its start. Moving the start inward by `m` moves it about
/// `m · (1 + tan φ) ≥ m` of L1 in front of a wall seen at φ from its normal,
/// so h falls to about −1e-3, four half-ulps clear of the rounding, and the
/// ray meets its wall `m` ahead, at the foot. A cell 10 m from the capture
/// point moves 1 cm, a third of a 16-texel-a-metre cell's half side. A
/// concave corner, where the filter reads the stored distance near across
/// the crease (0.1% at the canal fixture's far corners, 0.9% at the empty
/// room's), keeps its lookup at the foot but may stay inside the depth
/// margin and dim (95% in the empty room's): clearing that would take 5e-3,
/// 4.5 cm there, more than half a cell.
pub const FLUID_REFLECTION_RIM_INSET: f32 = 1.0e-3;
/// The rim inset's floor in metres: a cell within 10 cm of the capture
/// point still lands 0.1 mm inside, well above f32's step at map
/// coordinates (≈6e-5 m at 1 km) and well below the compiler's 1 cm capture
/// clearance from walls.
pub const FLUID_REFLECTION_RIM_INSET_MIN: f32 = 1.0e-4;

/// The pixel style's cell centre held to the probe's box at a wall rim:
/// pulled toward the box inset horizontally by `m`
/// ([`FLUID_REFLECTION_RIM_INSET`] of the centre's horizontal L1 distance
/// from the capture point, at least [`FLUID_REFLECTION_RIM_INSET_MIN`]). With
/// `h` half the cell's farthest overshoot, `0.5 · max(|u + v|, |u − v|)`
/// (half its longer diagonal, sheared axes included), a centre up to `h`
/// past a face (`|o|`, measured against the face) is pulled whole onto the
/// inset box, `m` inside the face, and the pull then tapers linearly to
/// nothing at `2h`, one full cell: the pull to the inset box scaled by
/// `clamp((2h − |o|) / h, 0, 1)`. Measuring the taper against the face keeps
/// every cell that straddles the face (`|o| ≤ h`) on the full pull; the
/// taper crosses the face only for a cell wholly past it, which is water
/// outside an authored box. A cell straddling a wall the box fits (a derived
/// box's wall), its centre inside the wall, starts its reflection just in
/// front of the wall's face rather than behind the surface the capture
/// stored there, so the rim keeps its term with no threshold. A centre
/// inside the box within `m` of a face moves at most `m` inward; one deeper
/// inside, below a derived box's floor (the capture plane 2 mm above the
/// water), or a cell or more past the box (water well outside an authored
/// box) stays put. While `m` is under the capture point's clearance from the
/// box (1 cm from a wall the compiler fits, so any cell within 10 m), the
/// capture point lies inside the inset box and the pull, toward a convex set
/// holding it, never reaches it; a cell farther out moves at most `h + m`,
/// metres from it. Continuous in the
/// centre, and a function of the centre, the cell's axes, the box and the
/// capture point alone, so every fragment of a cell still reads one search.
pub fn reflection_cell_within_box(
    centre: Vec3,
    axes: FluidTexelAxes,
    box_min: Vec3,
    box_max: Vec3,
    capture_point: Vec3,
) -> Vec3 {
    let past = centre - centre.clamp(box_min, box_max);
    let length_past = Vec3::new(past.x, 0.0, past.z).length();
    let half_cell = 0.5 * (axes.u + axes.v).length().max((axes.u - axes.v).length());
    let toward = centre - capture_point;
    let inset = (FLUID_REFLECTION_RIM_INSET * (toward.x.abs() + toward.z.abs()))
        .max(FLUID_REFLECTION_RIM_INSET_MIN);
    let margin = Vec3::new(inset, 0.0, inset);
    let target_point = centre.min(box_max - margin).max(box_min + margin);
    let pull = Vec3::new(centre.x - target_point.x, 0.0, centre.z - target_point.z);
    let keep = if half_cell > 0.0 {
        ((2.0 * half_cell - length_past) / half_cell).clamp(0.0, 1.0)
    } else {
        0.0
    };
    centre - pull * keep
}

/// Where the fragment reflects from. Smooth: the fragment itself, about the
/// flat outward normal. Pixel (any other style value): its cell's centre,
/// held to the probe's box at a wall rim ([`reflection_cell_within_box`]),
/// about the cell's tilted normal, the offset being the albedo's own
/// shimmer-and-ring displayed offset (never drift or the wave), so every
/// fragment of a cell reflects one direction.
#[allow(clippy::too_many_arguments)]
pub fn reflection_point(
    style: u32,
    texel_uv: Vec2,
    world_position: Vec3,
    outward: Vec3,
    axes: FluidTexelAxes,
    time: f32,
    texels_per_pixel: f32,
    box_min: Vec3,
    box_max: Vec3,
    capture_point: Vec3,
) -> FluidReflectionPoint {
    if style == FLUID_REFLECTION_STYLE_SMOOTH {
        return FluidReflectionPoint {
            position: world_position,
            normal: outward,
        };
    }
    let offset = displayed_offset(texel_cell(texel_uv), time, texels_per_pixel);
    let centre = reflection_cell_centre(texel_uv, world_position, axes);
    FluidReflectionPoint {
        position: reflection_cell_within_box(centre, axes, box_min, box_max, capture_point),
        normal: reflection_cell_normal(outward, axes, offset),
    }
}

#[cfg(test)]
#[path = "reflection_style_tests.rs"]
mod tests;
