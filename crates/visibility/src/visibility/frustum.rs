// View-frustum planes: extraction from a view-projection and AABB culling.
// See: context/lib/rendering_pipeline.md §2

use glam::{Mat4, Vec3, Vec4};

/// A plane in Hessian normal form: dot(normal, point) + dist >= 0 for points on the inside.
#[derive(Debug, Clone, Copy)]
pub struct FrustumPlane {
    pub normal: Vec3,
    pub dist: f32,
}

/// The planes of a view frustum, extracted from a view-projection matrix.
///
/// The initial camera frustum always contains exactly 6 planes in
/// left/right/bottom/top/near/far order. A frustum narrowed through a portal
/// may contain more planes (one per portal edge plus near/far).
#[derive(Debug, Clone)]
pub struct Frustum {
    pub planes: Vec<FrustumPlane>,
}

/// Canonical plane indices for a 6-plane frustum produced by
/// [`extract_frustum_planes`]. Kept next to the extraction so any future
/// reordering has exactly one place to update.
pub(crate) const NEAR_PLANE_INDEX: usize = 4;

impl Frustum {
    /// Slide the near plane so it passes exactly through `apex`, keeping
    /// the inward normal unchanged. Intended for the initial camera frustum
    /// before any portal narrowing.
    ///
    /// Fixes the tight-corridor blank-frame bug: the render pipeline's
    /// 0.1-unit near clip is depth-precision only, and when the camera sits
    /// closer than that to a portal plane, every portal vertex lies between
    /// camera and near plane — Sutherland-Hodgman clips the polygon to
    /// empty, the neighbor is rejected, and the frame flashes the clear
    /// color.
    ///
    /// Assumes the canonical 6-plane layout from [`extract_frustum_planes`]
    /// (Left, Right, Bottom, Top, Near, Far). **Do not call on a narrowed
    /// sub-frustum**: those replace the near plane with the portal plane,
    /// and sliding it to the camera apex would defeat the narrowing.
    pub(crate) fn slide_near_plane_to(&mut self, apex: Vec3) {
        debug_assert_eq!(
            self.planes.len(),
            6,
            "slide_near_plane_to expects the canonical 6-plane extraction; \
             narrowed sub-frustums must not call this"
        );
        if let Some(near) = self.planes.get_mut(NEAR_PLANE_INDEX) {
            slide_plane_to(near, apex);
        }
    }
}

/// Move `plane` along its normal so it passes through `point`.
pub(crate) fn slide_plane_to(plane: &mut FrustumPlane, point: Vec3) {
    plane.dist = -plane.normal.dot(point);
}

/// Extract the six frustum planes from a combined view-projection matrix.
///
/// Uses the Griess-Hartmann method for a right-handed projection:
/// each plane is a combination of rows from the 4x4 matrix. The resulting
/// planes point inward (a point satisfying all six is inside the frustum).
pub fn extract_frustum_planes(view_proj: Mat4) -> Frustum {
    Frustum {
        planes: frustum_plane_array(view_proj).to_vec(),
    }
}

/// [`extract_frustum_planes`] without the allocation, for the per-frame walk.
pub(crate) fn frustum_plane_array(view_proj: Mat4) -> [FrustumPlane; 6] {
    // glam stores matrices column-major. To get row N, we read element N from each column.
    let row = |n: usize| -> Vec4 {
        Vec4::new(
            view_proj.col(0)[n],
            view_proj.col(1)[n],
            view_proj.col(2)[n],
            view_proj.col(3)[n],
        )
    };

    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);

    let raw_planes = [
        r3 + r0, // Left
        r3 - r0, // Right
        r3 + r1, // Bottom
        r3 - r1, // Top
        r3 + r2, // Near
        r3 - r2, // Far
    ];

    raw_planes.map(|raw| {
        let normal = Vec3::new(raw.x, raw.y, raw.z);
        let length = normal.length();
        if length > 0.0 {
            let inv_len = 1.0 / length;
            FrustumPlane {
                normal: normal * inv_len,
                dist: raw.w * inv_len,
            }
        } else {
            FrustumPlane {
                normal: Vec3::ZERO,
                dist: 0.0,
            }
        }
    })
}

/// Test whether an axis-aligned bounding box is completely outside the frustum.
///
/// Uses the "positive vertex" (p-vertex) test: for each frustum plane, find the AABB
/// corner most in the direction of the plane normal. If that corner is behind the plane,
/// the entire AABB is outside. This is conservative — partially-outside boxes pass.
pub(crate) fn is_aabb_outside_frustum(mins: Vec3, maxs: Vec3, frustum: &Frustum) -> bool {
    is_aabb_outside_planes(mins, maxs, &frustum.planes)
}

/// [`is_aabb_outside_frustum`] over a bare plane list.
pub(crate) fn is_aabb_outside_planes(mins: Vec3, maxs: Vec3, planes: &[FrustumPlane]) -> bool {
    planes.iter().any(|plane| {
        // Select the AABB vertex farthest along the plane normal (positive vertex).
        let p_vertex = Vec3::select(plane.normal.cmpge(Vec3::ZERO), maxs, mins);
        // If the positive vertex is behind the plane, the AABB is fully outside.
        plane.normal.dot(p_vertex) + plane.dist < 0.0
    })
}
