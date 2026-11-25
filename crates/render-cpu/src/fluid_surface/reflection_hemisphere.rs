// The probe lookup's hemisphere: the half of the sphere a probe's faces
// read, water's and glossy floors' above the horizon, a wall's, ceiling's or
// slope's about its face normal. GPU-free reference for
// `fluid_reflection_ray.wgsl`'s and `fluid_reflection.wgsl`'s hemisphere
// helpers, which mirror it and a test pins. Tested beside the search
// (`reflection_search_up_tests.rs`).
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::Vec3;

use super::reflection::{FLUID_REFLECTION_DIRECTION_EPSILON, FLUID_REFLECTION_RAY_MIN_Y};

/// The hemisphere axis of an up-facing probe, water's or a glossy floor's:
/// +Y exactly, so every lookup clamps to the horizon as water's always did.
pub const FLUID_REFLECTION_UP: Vec3 = Vec3::Y;

/// The cosine of water's up-facing test (1°, the compiler's
/// `UP_FACING_MIN_COS`): a face normal at least this close to +Y faces up,
/// and its probe's lookups clamp to the horizon.
pub const REFLECTION_UP_FACING_MIN_COS: f32 = 0.999_847_7;

/// The hemisphere axis a bound face's lookups read: [`FLUID_REFLECTION_UP`]
/// for an up-facing face, else the face's own unit normal (a wall's,
/// ceiling's or slope's facing, within the compiler's 1° facing tolerance).
/// A property of the face and so of the probe it binds by position, never of
/// a lookup direction.
pub fn reflection_hemisphere_axis(normal: Vec3) -> Vec3 {
    if normal.y >= REFLECTION_UP_FACING_MIN_COS {
        FLUID_REFLECTION_UP
    } else {
        normal
    }
}

/// `v` with its part below the hemisphere's horizon removed: unchanged on or
/// above the plane through the origin normal to `axis`, else projected onto
/// it. About [`FLUID_REFLECTION_UP`] this is exactly `(x, max(y, 0), z)`.
pub fn reflection_level(v: Vec3, axis: Vec3) -> Vec3 {
    v - axis * v.dot(axis).min(0.0)
}

/// The reflected unit ray kept in the hemisphere about `axis`: unchanged at
/// or above [`FLUID_REFLECTION_RAY_MIN_Y`] over the horizon, else lifted to
/// that height and renormalized, its bearing in the plane kept. About
/// [`FLUID_REFLECTION_UP`] this is exactly water's horizon floor.
pub fn reflection_above_hemisphere(ray: Vec3, axis: Vec3) -> Vec3 {
    let height = ray.dot(axis);
    if height >= FLUID_REFLECTION_RAY_MIN_Y {
        return ray;
    }
    (ray - axis * height + axis * FLUID_REFLECTION_RAY_MIN_Y).normalize()
}

/// The tile lookup direction: from the capture point to the reflected point
/// (the distance search's hit, or the box hit), clamped to the hemisphere
/// about `axis` when it lies behind the capture point's plane. A point on
/// the capture point itself has no direction and takes the reflected ray's.
pub fn reflection_lookup_about(hit: Vec3, capture_point: Vec3, ray: Vec3, axis: Vec3) -> Vec3 {
    let level = reflection_level(hit - capture_point, axis);
    if level.dot(level) < FLUID_REFLECTION_DIRECTION_EPSILON {
        reflection_level(ray, axis)
    } else {
        level
    }
}
