// A mover ghost's placed shape as the ghost sort reads it: its oriented
// bounds for contact, and its mover's own triangles for what lies between
// the eye and a rider.
// See: context/lib/rendering_pipeline.md §7.9

use glam::{Mat4, Vec3};
use postretro_render_data::cone_frustum::Aabb;

/// How close to a rider's body point, in world metres, a mover's surface
/// may cross the eye's line and still not count as between the eye and the
/// rider. The body point is inside the rider, and falls to foot height when
/// its envelope reaches no higher; there it sits a collision skin (2 cm)
/// above the support face. A centimetre absorbs float error at that end and
/// stays under the skin, so a floor under a foot-level body point still
/// counts when the eye looks up through it.
const RIDER_BODY_END_TOLERANCE: f32 = 0.01;

/// Growth of the box early-out, in metres: rounding at a box face never
/// rejects a triangle lying on that face, as a flat floor's triangles do.
const EARLY_OUT_MARGIN: f32 = 1e-3;

/// Square of the sine below which a segment counts as parallel to a
/// triangle's plane, and a triangle's edges as collinear: either way the
/// crossing is ill-conditioned, and the triangle is skipped.
const DEGENERATE_SINE_SQUARED: f32 = 1e-12;

/// A mover ghost's geometry as placed: its mover's local geometry bounds
/// under the ghost's model transform. The frame sorts the ghost by the box's
/// centre and asks which skinned ghosts stand on or in the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MoverGhostBounds {
    local: Aabb,
    transform: Mat4,
    /// World to mover-local, when the box can hold a point: `None` for a
    /// transform with no inverse, or a box that is not finite and ordered.
    local_from_world: Option<Mat4>,
    /// World length of one local unit along each local axis.
    metres_per_unit: Vec3,
}

impl MoverGhostBounds {
    pub(super) fn new(transform: Mat4, local: Aabb) -> Self {
        let ordered =
            local.min.is_finite() && local.max.is_finite() && local.min.cmple(local.max).all();
        let local_from_world = if ordered {
            transform.try_inverse()
        } else {
            None
        };
        Self {
            local,
            transform,
            local_from_world,
            metres_per_unit: Vec3::new(
                transform.x_axis.truncate().length(),
                transform.y_axis.truncate().length(),
                transform.z_axis.truncate().length(),
            ),
        }
    }

    /// World-space centre of the box; not finite when the box or transform
    /// is not.
    pub(super) fn center(&self) -> Vec3 {
        self.transform
            .transform_point3((self.local.min + self.local.max) * 0.5)
    }

    /// Whether world `point` lies in the oriented box grown by `margin`
    /// metres along each of its axes. Tested in the mover's own frame, so a
    /// point beside a turned box but inside its world-axis enclosure is out.
    /// Never true for a box with no inverse or bounds, or a non-finite point.
    /// The sort asks only about segments; tests state contact with this.
    #[cfg(test)]
    pub(super) fn contains(&self, point: Vec3, margin: f32) -> bool {
        self.meets_segment(point, point, margin)
    }

    /// Whether the world segment from `start` to `end` touches the oriented
    /// box grown by `margin` metres along each of its axes: a slab test in
    /// the mover's own frame, ends included. A segment of zero length asks
    /// whether its point lies in the box. Never true for a box with no
    /// inverse or bounds, or a non-finite end.
    pub(super) fn meets_segment(&self, start: Vec3, end: Vec3, margin: f32) -> bool {
        let Some(local_from_world) = self.local_from_world else {
            return false;
        };
        let start = local_from_world.transform_point3(start);
        let end = local_from_world.transform_point3(end);
        if !(start.is_finite() && end.is_finite()) {
            return false;
        }
        let reach = Vec3::splat(margin) / self.metres_per_unit;
        let low = self.local.min - reach;
        let high = self.local.max + reach;
        let step = end - start;
        // The part of the segment, as a fraction of its length, inside every
        // slab seen so far.
        let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
        for axis in 0..3 {
            let (from, along) = (start[axis], step[axis]);
            if along == 0.0 {
                // Parallel to this slab: inside it everywhere or nowhere.
                if from < low[axis] || from > high[axis] {
                    return false;
                }
                continue;
            }
            let near = (low[axis] - from) / along;
            let far = (high[axis] - from) / along;
            enter = enter.max(near.min(far));
            leave = leave.min(near.max(far));
            if enter > leave {
                return false;
            }
        }
        true
    }
}

/// A mover ghost as the sort reads it: its placed bounds, and its mover's
/// triangles in the space the ghost's model transform maps from. The bounds
/// answer contact (standing on or in the mover); the triangles answer what
/// lies between the eye and a rider.
#[derive(Debug, Clone, Copy)]
pub(super) struct MoverGhostShape<'a> {
    bounds: MoverGhostBounds,
    triangles: &'a [[Vec3; 3]],
}

impl<'a> MoverGhostShape<'a> {
    pub(super) fn new(bounds: MoverGhostBounds, triangles: &'a [[Vec3; 3]]) -> Self {
        Self { bounds, triangles }
    }

    pub(super) fn bounds(&self) -> &MoverGhostBounds {
        &self.bounds
    }

    /// Whether any of the mover's surface lies between world `eye` and a
    /// rider's `body` point: the segment between them crosses one of its
    /// triangles, either side facing, short of the last
    /// [`RIDER_BODY_END_TOLERANCE`] metres before `body`. So railings, a
    /// post or a cabin beside the rider's line leave it clear, a closed cab's
    /// wall or a bridge's deck do not, and an eye inside a cab sees a rider
    /// inside it clear and one on its roof through the roof.
    ///
    /// The segment runs in mover-local space, where its fraction along its
    /// length is the world one. Cost: one box test, then, when the segment
    /// meets the box, one segment–triangle test per triangle; a bounding
    /// volume hierarchy over the triangles can replace the scan here without
    /// touching callers. False for a box with no inverse or bounds, a
    /// non-finite end, or a segment no longer than the tolerance; a triangle
    /// with no area, or one the segment runs parallel to, is skipped.
    pub(super) fn surface_between(&self, eye: Vec3, body: Vec3) -> bool {
        if !self.bounds.meets_segment(eye, body, EARLY_OUT_MARGIN) {
            return false;
        }
        let Some(local_from_world) = self.bounds.local_from_world else {
            return false;
        };
        let start = local_from_world.transform_point3(eye);
        let step = local_from_world.transform_point3(body) - start;
        let last = 1.0 - RIDER_BODY_END_TOLERANCE / eye.distance(body);
        if !(start.is_finite() && step.is_finite() && last > 0.0) {
            return false;
        }
        self.triangles
            .iter()
            .any(|triangle| segment_crosses_triangle(start, step, triangle, last))
    }
}

/// Whether the segment from `start` along `step` crosses `triangle` at a
/// fraction of its length in `0..=last` (Möller–Trumbore, either side
/// facing). A triangle with no area, a segment parallel to its plane, or
/// any non-finite step counts as no crossing.
fn segment_crosses_triangle(start: Vec3, step: Vec3, triangle: &[Vec3; 3], last: f32) -> bool {
    let [corner, second, third] = *triangle;
    let edge1 = second - corner;
    let edge2 = third - corner;
    let normal_sq = edge1.cross(edge2).length_squared();
    // Written as positive tests, so a NaN fails them.
    let has_area =
        normal_sq > DEGENERATE_SINE_SQUARED * edge1.length_squared() * edge2.length_squared();
    if !has_area {
        return false;
    }
    let p = step.cross(edge2);
    let det = edge1.dot(p);
    let crosses_plane = det * det > DEGENERATE_SINE_SQUARED * normal_sq * step.length_squared();
    if !crosses_plane {
        return false;
    }
    let inverse = det.recip();
    let offset = start - corner;
    let u = offset.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = offset.cross(edge1);
    let v = step.dot(q) * inverse;
    if !(v >= 0.0 && u + v <= 1.0) {
        return false;
    }
    (0.0..=last).contains(&(edge2.dot(q) * inverse))
}

#[cfg(test)]
#[path = "kinematic_ghost_shape_tests.rs"]
pub(crate) mod tests;
