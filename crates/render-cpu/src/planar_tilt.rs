// Planar pixel tilt, GPU-free: the planar resolve's cell ray, mirror reprojection and lookup.
// See: context/lib/rendering_pipeline.md §7.3a

// The CPU reference for `planar_resolve.wgsl`'s tilt: `planar_tilt_ray`,
// `planar_mirror_texel`, `planar_mirror_surface`, `planar_covered` and
// `planar_tilted_point` mirror the WGSL functions of the same names statement
// for statement, and `planar_resolve_texel` mirrors the fragment's style
// switch and lookup; a renderer test pins both texts. A pixel-style cell
// composes the shared fluid helpers, so its offset, tilted normal and ray are
// SSR's and the probe term's, from the unpulled cell centre: the planar layer
// reads no probe box, capture point, Fresnel, capture shadow or probe colour.
// The ray enters only the reflection law and the reprojection, which clamps
// (clip w to a floor, the texel to the target). Nothing here branches on,
// selects by or weights a direction quantity. The composed shared helpers
// hold the only such branch and select, both placements SSR and the probe
// term share that never route, cut or weight: the horizon floor in
// `fluid_reflection_above_horizon` (`reflection_above_horizon` here) and the
// zero-length guard on the texel axes in `fluid_reflection_cell_normal`. The
// tilt moves where the layer samples: c is 0 where the untilted texel holds
// no content, else the sampled texel's coverage times its D fade times the
// fragment's R fade.

use glam::{IVec2, Mat4, Vec2, Vec3, Vec4};

use crate::fluid_surface::motion::{displayed_offset, texel_cell};
use crate::fluid_surface::reflection::{reflection_above_horizon, reflection_mirror};
use crate::fluid_surface::reflection_style::{
    FLUID_REFLECTION_STYLE_SMOOTH, FluidTexelAxes, reflection_cell_centre, reflection_cell_normal,
};

/// The smallest clip w a reprojection keeps: a placement clamp, never a test.
pub const PLANAR_W_FLOOR: f32 = 1.0e-6;

/// The mirror view as the resolve reads it from its inputs uniform: the
/// raster view-projection (a real-world point to the mirror texel showing
/// it), its inverse, and the mirror target's extent in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarMirrorView {
    pub raster_vp: Mat4,
    pub inverse_raster_vp: Mat4,
    pub extent: Vec2,
}

impl PlanarMirrorView {
    pub fn new(raster_vp: Mat4, extent: (u32, u32)) -> Self {
        Self {
            raster_vp,
            inverse_raster_vp: raster_vp.inverse(),
            extent: Vec2::new(extent.0 as f32, extent.1 as f32),
        }
    }
}

/// A pixel-style cell's reflection: its unpulled centre, its tilted normal
/// and its ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanarTiltRay {
    pub cell_centre: Vec3,
    pub cell_normal: Vec3,
    pub tilted_ray: Vec3,
}

/// The cell's reflection from the shared helpers: the displayed offset (the
/// albedo's hashed shimmer and rings, never drift or the wave) times
/// `offset_scale` (1; 0 only where a test forces the offset to zero), the
/// unpulled cell centre, the flat normal tilted by the offset, and the
/// mirrored view kept above the horizon, exactly as SSR's pass composes them.
#[allow(clippy::too_many_arguments)]
pub fn planar_tilt_ray(
    texel_uv: Vec2,
    world_position: Vec3,
    flat_normal: Vec3,
    axes: FluidTexelAxes,
    eye: Vec3,
    time: f32,
    texels_per_pixel: f32,
    offset_scale: i32,
) -> PlanarTiltRay {
    let offset = displayed_offset(texel_cell(texel_uv), time, texels_per_pixel) * offset_scale;
    let cell_centre = reflection_cell_centre(texel_uv, world_position, axes);
    let cell_normal = reflection_cell_normal(flat_normal, axes, offset);
    let view_vector = (eye - cell_centre).normalize();
    let reflected_ray = reflection_mirror(view_vector, cell_normal);
    let tilted_ray = reflection_above_horizon(reflected_ray);
    PlanarTiltRay {
        cell_centre,
        cell_normal,
        tilted_ray,
    }
}

/// The mirror texel showing real-world `point`: its clip w floored and its
/// position clamped to the target, a placement, never a decision, so a point
/// off the mirror frustum or behind the mirror eye lands on the target's edge.
pub fn planar_mirror_texel(point: Vec3, mirror: &PlanarMirrorView) -> IVec2 {
    let mirror_clip = mirror.raster_vp * point.extend(1.0);
    let mirror_ndc = Vec2::new(mirror_clip.x, mirror_clip.y) / mirror_clip.w.max(PLANAR_W_FLOOR);
    let size = mirror.extent;
    let at = (Vec2::new(mirror_ndc.x * 0.5 + 0.5, 0.5 - mirror_ndc.y * 0.5) * size).floor();
    at.clamp(Vec2::ZERO, size - Vec2::ONE).as_ivec2()
}

/// The real-world point mirror texel `texel` shows at its stored `depth`.
pub fn planar_mirror_surface(texel: IVec2, depth: f32, mirror: &PlanarMirrorView) -> Vec3 {
    let centre = (texel.as_vec2() + Vec2::splat(0.5)) / mirror.extent;
    let surface_h = mirror.inverse_raster_vp
        * Vec4::new(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0, depth, 1.0);
    surface_h.truncate() / surface_h.w
}

/// Coverage: the clear (unmasked) and the mask's far value (masked, nothing
/// drawn) hold no mirror content.
pub fn planar_covered(depth: f32) -> bool {
    depth > 0.0 && depth < 1.0
}

/// X = P + r′·d: the tilted ray from the cell centre, out to the distance the
/// mirror depth gives at the untilted texel, never a preset distance.
pub fn planar_tilted_point(
    tilt: &PlanarTiltRay,
    untilted: IVec2,
    untilted_depth: f32,
    mirror: &PlanarMirrorView,
) -> Vec3 {
    let reach = planar_mirror_surface(untilted, untilted_depth, mirror).distance(tilt.cell_centre);
    tilt.cell_centre + tilt.tilted_ray * reach
}

/// The mirror texel one resolve fragment samples, after its derivatives:
/// smooth samples the fragment's own mirror texel and tilts nothing; pixel
/// samples where its cell's tilted ray reaches at the untilted texel's
/// mirror depth, or nothing (c = 0) where the untilted texel holds no
/// content. `depth_at` reads the mirror depth. The layer value is then that
/// texel's coverage, D at the surface it shows and R at the fragment.
#[allow(clippy::too_many_arguments)]
pub fn planar_resolve_texel(
    style: u32,
    texel_uv: Vec2,
    world_position: Vec3,
    world_normal: Vec3,
    axes: FluidTexelAxes,
    eye: Vec3,
    time: f32,
    texels_per_pixel: f32,
    offset_scale: i32,
    mirror: &PlanarMirrorView,
    depth_at: impl Fn(IVec2) -> f32,
) -> Option<IVec2> {
    if style == FLUID_REFLECTION_STYLE_SMOOTH {
        return Some(planar_mirror_texel(world_position, mirror));
    }
    let tilt = planar_tilt_ray(
        texel_uv,
        world_position,
        world_normal.normalize(),
        axes,
        eye,
        time,
        texels_per_pixel,
        offset_scale,
    );
    let untilted = planar_mirror_texel(tilt.cell_centre, mirror);
    let untilted_depth = depth_at(untilted);
    if !planar_covered(untilted_depth) {
        return None;
    }
    Some(planar_mirror_texel(
        planar_tilted_point(&tilt, untilted, untilted_depth, mirror),
        mirror,
    ))
}

#[cfg(test)]
#[path = "planar_tilt_tests.rs"]
mod tests;
