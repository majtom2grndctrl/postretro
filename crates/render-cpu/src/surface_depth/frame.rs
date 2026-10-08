//! The per-fragment surface frame and the two ray directions the marches walk.

use super::{SURFACE_DEPTH_EPS, above, within};
use glam::Vec3;

/// Floor on the UV Jacobian's determinant.
///
/// It is a PRODUCT of two UV derivatives, so it is naturally tiny — at close
/// range a 1 k texture can put it near `1e-9`, which is why this is NOT
/// `SURFACE_DEPTH_EPS`. It only has to catch a genuinely singular chart (which
/// makes the solve `0/0`); the honest check is the scale range below, which
/// also traps the NaN and infinity a near-singular solve produces.
pub const SURFACE_DEPTH_DET_EPS: f32 = 1.0e-20;
/// Sanity range for world meters per UV unit. Outside it the solved frame is
/// not describing a real brush face.
pub const SURFACE_DEPTH_MAX_UV_SCALE_M: f32 = 1.0e6;
/// Cosine floor between the view ray and the surface normal. Below it the
/// fragment is a sub-pixel sliver seen edge-on and the march has nothing to say.
pub const SURFACE_DEPTH_MIN_DESCENT: f32 = 1.0e-3;

/// Orthonormal surface frame derived from screen-space derivatives rather than
/// from the baked tangent.
///
/// Deriving it here rather than from `world_tangent` is deliberate: the march
/// walks the UV grid, so its axes must be the UV axes and its scale must be the
/// same world-units-per-UV-unit the meters→UV conversion uses. Taking both from
/// one Jacobian makes that consistency structural. The baked tangent still owns
/// normal mapping, which is a different (and authored) tangent space.
///
/// Assumes the UV parameterization is locally orthogonal, which holds for brush
/// faces (TrenchBroom emits scale/rotate/offset UV axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthBasis {
    /// Unit world direction of increasing `u`, projected into the tangent plane.
    pub tangent: Vec3,
    /// Unit world direction of increasing `v`, projected into the tangent plane.
    pub bitangent: Vec3,
    /// Geometric surface normal (never the shading normal).
    pub normal: Vec3,
    /// UV units per world meter along `u` and `v`.
    pub uv_per_meter: [f32; 2],
}

/// Solve `∂W/∂u` and `∂W/∂v` from the screen-space derivative pairs and build
/// the surface frame. Returns `None` for a degenerate fragment (collapsed UV
/// chart, vanishing derivatives) — the caller then stays flat.
pub fn surface_depth_basis(
    ddx_uv: [f32; 2],
    ddy_uv: [f32; 2],
    ddx_world: Vec3,
    ddy_world: Vec3,
    normal: Vec3,
) -> Option<SurfaceDepthBasis> {
    // [Wx] = [ux vx][Wu]
    // [Wy]   [uy vy][Wv]
    // See `above` / `within`: NaN must fall out, not slip through.
    let det = ddx_uv[0] * ddy_uv[1] - ddx_uv[1] * ddy_uv[0];
    if !above(det.abs(), SURFACE_DEPTH_DET_EPS) {
        return None;
    }
    let w_u = (ddx_world * ddy_uv[1] - ddy_world * ddx_uv[1]) / det;
    let w_v = (ddy_world * ddx_uv[0] - ddx_world * ddy_uv[0]) / det;

    let scale_u = w_u.length();
    let scale_v = w_v.length();
    let usable = |s: f32| within(s, SURFACE_DEPTH_EPS, SURFACE_DEPTH_MAX_UV_SCALE_M);
    if !usable(scale_u) || !usable(scale_v) {
        return None;
    }

    // Project out the normal: interpolation leaves the derivatives slightly
    // off the tangent plane, and a side-wall normal that is not perpendicular
    // to the surface normal reads as a lighting seam.
    let t = w_u - normal * w_u.dot(normal);
    let b = w_v - normal * w_v.dot(normal);
    if !above(t.length_squared(), SURFACE_DEPTH_EPS)
        || !above(b.length_squared(), SURFACE_DEPTH_EPS)
    {
        return None;
    }

    Some(SurfaceDepthBasis {
        tangent: t.normalize(),
        bitangent: b.normalize(),
        normal,
        uv_per_meter: [1.0 / scale_u, 1.0 / scale_v],
    })
}

/// UV advance per meter of descent for the view ray.
///
/// `view_to_eye` points from the surface toward the camera. Returns `None` for
/// an edge-on fragment, where the ray never descends and the march is
/// meaningless.
pub fn surface_depth_view_ray(basis: &SurfaceDepthBasis, view_to_eye: Vec3) -> Option<[f32; 2]> {
    let descent = view_to_eye.dot(basis.normal);
    if !above(descent, SURFACE_DEPTH_MIN_DESCENT) {
        return None;
    }
    let into = -view_to_eye;
    Some([
        into.dot(basis.tangent) * basis.uv_per_meter[0] / descent,
        into.dot(basis.bitangent) * basis.uv_per_meter[1] / descent,
    ])
}

/// UV advance per meter of *rise* for a shadow ray toward `to_light`.
/// `None` when the light is at or below the surface plane.
pub fn surface_depth_light_ray(basis: &SurfaceDepthBasis, to_light: Vec3) -> Option<[f32; 2]> {
    let rise = to_light.dot(basis.normal);
    if !above(rise, SURFACE_DEPTH_EPS) {
        return None;
    }
    Some([
        to_light.dot(basis.tangent) * basis.uv_per_meter[0] / rise,
        to_light.dot(basis.bitangent) * basis.uv_per_meter[1] / rise,
    ])
}
