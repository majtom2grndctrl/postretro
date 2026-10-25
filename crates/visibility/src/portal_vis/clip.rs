// Portal polygon clipping and frustum narrowing.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use crate::visibility::{Frustum, FrustumPlane};

// Half-space boundary epsilon for Sutherland-Hodgman. Over-inclusion at the
// boundary is safe: the next narrowing iteration will discard any slop, so the
// strict-subset invariant holds.
pub(crate) const CLIP_EPSILON: f32 = 1e-4;

/// Clip a convex polygon against every frustum plane (Sutherland-Hodgman).
///
/// Returns a slice into whichever scratch buffer held the final output; the
/// `'a` lifetime ties both scratch buffers to the return value so the borrow
/// checker prevents reuse of either until the slice is dropped. Callers that
/// recurse with the same scratches (e.g. `flood`) must confine this slice to
/// an inner scope.
///
/// Planes use Hessian normal form pointing inward; `CLIP_EPSILON` tilts
/// boundary cases toward "inside" without violating the strict-subset
/// invariant — slop kept here is outside the next narrowing's edge planes.
pub(crate) fn clip_polygon_to_frustum<'a>(
    polygon: &[Vec3],
    frustum: &Frustum,
    scratch_a: &'a mut Vec<Vec3>,
    scratch_b: &'a mut Vec<Vec3>,
) -> &'a [Vec3] {
    clip_polygon_to_planes(polygon, &frustum.planes, scratch_a, scratch_b)
}

/// [`clip_polygon_to_frustum`] over a bare plane list, for callers that build
/// their clip planes per step instead of holding a `Frustum`.
pub(crate) fn clip_polygon_to_planes<'a>(
    polygon: &[Vec3],
    planes: &[FrustumPlane],
    scratch_a: &'a mut Vec<Vec3>,
    scratch_b: &'a mut Vec<Vec3>,
) -> &'a [Vec3] {
    scratch_a.clear();
    scratch_b.clear();

    if polygon.len() < 3 {
        return &scratch_a[..];
    }

    scratch_a.extend_from_slice(polygon);

    let mut input_is_a = true;
    for plane in planes {
        let (input, output) = if input_is_a {
            (&*scratch_a, &mut *scratch_b)
        } else {
            (&*scratch_b, &mut *scratch_a)
        };
        if input.is_empty() {
            break;
        }
        output.clear();
        clip_polygon_to_plane(input, plane, output);
        input_is_a = !input_is_a;
    }

    if input_is_a {
        &scratch_a[..]
    } else {
        &scratch_b[..]
    }
}

/// Clip a convex polygon against a single half-space (one Sutherland-Hodgman
/// step), using the three-state classifier from Doom 3's `idWinding::Split`
/// (RBDOOM-3-BFG `neo/idlib/geometry/Winding.cpp` L115-200). The same
/// algorithm ships in id's 1999 Quake `qbsp/winding.c` and in ericw-tools'
/// `polylib::winding_base_t::clip` today — a ~30-year-battle-tested lineage.
///
/// Each vertex is classified `FRONT`, `BACK`, or `ON` relative to the plane,
/// using `CLIP_EPSILON` as the on-plane tolerance. Both `FRONT` and `ON`
/// vertices are emitted to the output. The crucial predicate is the split-
/// point skip: when the *next* vertex is `ON` or on the same side as the
/// current one, no intersection vertex is generated — otherwise a vertex
/// within `CLIP_EPSILON` of the plane would get both emitted directly (as an
/// `ON` vertex) *and* have an intersection vertex generated adjacent to it
/// from the bracketing edge, producing the near-duplicate leading pair that
/// makes `narrow_frustum`'s cross-product normal collapse. That is the
/// mechanism behind the `test-2.prl` S-maze missing-panels bug; see the
/// regression probe below.
///
/// Writes the clipped vertices into `output` (which is cleared on entry by
/// the caller). The input polygon must be closed in winding order; vertex
/// order is preserved in the output.
fn clip_polygon_to_plane(input: &[Vec3], plane: &FrustumPlane, output: &mut Vec<Vec3>) {
    let n = input.len();
    if n < 3 {
        return;
    }

    let classify = |d: f32| -> i8 {
        if d > CLIP_EPSILON {
            1 // FRONT — strictly inside the half-space
        } else if d < -CLIP_EPSILON {
            -1 // BACK — strictly outside
        } else {
            0 // ON — within epsilon of the plane
        }
    };

    for i in 0..n {
        let p1 = input[i];
        let d1 = plane.normal.dot(p1) + plane.dist;
        let s1 = classify(d1);

        // Emit `p1` if it is FRONT or ON. ON vertices are emitted to both
        // sides in a full front-and-back split; since we only keep the
        // front side here, ON still belongs in the output.
        if s1 >= 0 {
            output.push(p1);
        }

        // If `p1` is ON, do not generate a split point for the outgoing
        // edge: the ON vertex itself is already the geometric split point,
        // so emitting another one adjacent to it would produce a near-
        // duplicate. The next vertex is handled by its own iteration.
        if s1 == 0 {
            continue;
        }

        let next_idx = (i + 1) % n;
        let p2 = input[next_idx];
        let d2 = plane.normal.dot(p2) + plane.dist;
        let s2 = classify(d2);

        // Skip the split point when:
        //   - `p2` is ON: it will be emitted verbatim in the next
        //     iteration as the geometric split point (Doom 3/Quake rule).
        //   - `p2` is on the same side as `p1`: the edge does not cross
        //     the plane, so there is no split point to generate.
        if s2 == 0 || s2 == s1 {
            continue;
        }

        output.push(compute_split_point_on_plane(p1, p2, d1, d2, plane));
    }
}

/// Compute the split point where a line segment crosses a plane, with two
/// numerical-robustness tweaks borrowed from Doom 3's `idWinding::Split`
/// (RBDOOM-3-BFG `neo/idlib/geometry/Winding.cpp` L205-224):
///
/// 1. **Direction-symmetric lerp.** Always interpolate from the FRONT
///    vertex toward the BACK vertex. This guarantees that processing edge
///    `A→B` and edge `B→A` yields bitwise-identical split points, which
///    matters when the same edge is walked from opposite directions by
///    adjacent clip steps.
/// 2. **Axis-aligned plane snap.** If the clip plane's normal is exactly a
///    unit axis (e.g. `(±1, 0, 0)`), force the split point's coordinate on
///    that axis to lie exactly on the plane instead of accepting lerp
///    drift. Frees a split-point vertex from later misclassification by
///    adjacent planes.
///
/// Caller guarantees `d1` and `d2` have opposite signs and neither is ON,
/// so the denominator is non-zero.
fn compute_split_point_on_plane(
    p1: Vec3,
    p2: Vec3,
    d1: f32,
    d2: f32,
    plane: &FrustumPlane,
) -> Vec3 {
    debug_assert!(
        d1.abs() > CLIP_EPSILON && d2.abs() > CLIP_EPSILON && d1.signum() != d2.signum(),
        "compute_split_point_on_plane requires d1/d2 to be strictly \
         opposite-sign and neither within CLIP_EPSILON — the SIDE_ON \
         filter in clip_polygon_to_plane must guarantee this"
    );

    let (front, back, d_front, d_back) = if d1 >= 0.0 {
        (p1, p2, d1, d2)
    } else {
        (p2, p1, d2, d1)
    };
    let t = d_front / (d_front - d_back);
    let mut mid = front + (back - front) * t;

    // Axis-aligned snap. Our Hessian convention is `n·v + d = 0`, so for
    // `n = +unit_j` the plane is `v[j] = -plane.dist`, and for
    // `n = -unit_j` it is `v[j] = plane.dist`.
    for j in 0..3 {
        let n_j = plane.normal[j];
        if n_j == 1.0 {
            mid[j] = -plane.dist;
        } else if n_j == -1.0 {
            mid[j] = plane.dist;
        }
    }

    mid
}

/// Tolerance for "camera lies on the portal's supporting plane".
///
/// Signed distance from the apex to the polygon's plane, measured in world
/// units. Looser than `CLIP_EPSILON` because the camera only needs to be
/// *near enough* that the view-frustum cross-section at the portal collapses
/// to something Sutherland-Hodgman can't keep convex — a sub-millimeter
/// margin around the plane is plenty to catch the "player on the portal
/// boundary" case without triggering on genuinely-frontal portals.
const APEX_ON_PORTAL_PLANE_EPSILON: f32 = 1e-3;

// Newell's method for the normal: robust against near-colinear leading
// vertices that collapse a simple (v1-v0)×(v2-v0) cross product.
pub(super) fn camera_on_polygon_plane(apex: Vec3, polygon: &[Vec3]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let n = polygon.len();
    let centroid = polygon.iter().copied().sum::<Vec3>() / n as f32;
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = polygon[i];
        let nxt = polygon[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-12 {
        return false;
    }
    let normal = normal.normalize();
    normal.dot(apex - centroid).abs() < APEX_ON_PORTAL_PLANE_EPSILON
}

/// Returns None if the portal is degenerate or the normal collapses.
pub fn narrow_frustum(
    camera_position: Vec3,
    portal_polygon: &[Vec3],
    original_frustum: &Frustum,
) -> Option<Frustum> {
    if portal_polygon.len() < 3 {
        return None;
    }

    let n = portal_polygon.len();
    let centroid = portal_polygon.iter().copied().sum::<Vec3>() / n as f32;

    // Newell's method: robust against colinear/near-duplicate vertices that would collapse a single (v1-v0)×(v2-v0) cross product.
    let mut portal_normal = Vec3::ZERO;
    for i in 0..n {
        let cur = portal_polygon[i];
        let nxt = portal_polygon[(i + 1) % n];
        portal_normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        portal_normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        portal_normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if portal_normal.length_squared() < 1e-12 {
        return None;
    }
    let portal_normal = portal_normal.normalize();

    // Orient normal away from the camera so the near plane clips the camera-side.
    let camera_side = portal_normal.dot(camera_position - centroid);
    let oriented_normal = if camera_side > 0.0 {
        -portal_normal
    } else {
        portal_normal
    };
    let portal_dist = -oriented_normal.dot(centroid);

    let mut planes = Vec::with_capacity(n + 2);

    // Portal plane as near clip.
    planes.push(FrustumPlane {
        normal: oriented_normal,
        dist: portal_dist,
    });

    // Edge planes: for each portal edge, the clip plane passes through the
    // camera and the edge, oriented to face the portal centroid. This is the
    // exact visibility cone from a point camera through the portal.
    for i in 0..n {
        let edge_a = portal_polygon[i];
        let edge_b = portal_polygon[(i + 1) % n];
        let edge_dir = edge_b - edge_a;
        let to_camera = camera_position - edge_a;

        let mut edge_normal = edge_dir.cross(to_camera);
        if edge_normal.length_squared() < 1e-12 {
            continue;
        }
        edge_normal = edge_normal.normalize();
        if edge_normal.dot(centroid - edge_a) < 0.0 {
            edge_normal = -edge_normal;
        }
        let dist = -edge_normal.dot(edge_a);

        planes.push(FrustumPlane {
            normal: edge_normal,
            dist,
        });
    }

    // Keep the far plane from the original frustum (always the last plane).
    if let Some(&far_plane) = original_frustum.planes.last() {
        planes.push(far_plane);
    }

    Some(Frustum { planes })
}
