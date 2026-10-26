// Point-to-portal distance and half-space tests for the region walk.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use crate::visibility::FrustumPlane;
use postretro_level_loader::PortalPlane;

/// Every vertex strictly behind `plane`. A vertex on the plane keeps the
/// portal, as the clipper would.
pub(crate) fn wholly_behind(plane: &FrustumPlane, polygon: &[Vec3]) -> bool {
    polygon
        .iter()
        .all(|v| plane.normal.dot(*v) + plane.dist < -crate::portal_vis::CLIP_EPSILON)
}

/// Distance from `point` to the closed convex `polygon` lying in `plane`.
/// A degenerate plane (zero normal) measures to the edges alone.
pub(crate) fn distance_to_polygon(point: Vec3, polygon: &[Vec3], plane: &PortalPlane) -> f32 {
    let n = polygon.len();
    if n == 0 {
        return f32::MAX;
    }
    if plane.normal != Vec3::ZERO && n >= 3 {
        let offset = plane.signed_distance(point);
        let projected = point - plane.normal * offset;
        // Inside when the projected point is on one side of every edge.
        let mut positive = false;
        let mut negative = false;
        for i in 0..n {
            let a = polygon[i];
            let b = polygon[(i + 1) % n];
            let side = (b - a).cross(projected - a).dot(plane.normal);
            positive |= side > 0.0;
            negative |= side < 0.0;
        }
        if !(positive && negative) {
            return offset.abs();
        }
    }
    (0..n)
        .map(|i| distance_to_segment(point, polygon[i], polygon[(i + 1) % n]))
        .fold(f32::MAX, f32::min)
}

fn distance_to_segment(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_squared();
    if length_squared == 0.0 {
        return point.distance(a);
    }
    let t = ((point - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    point.distance(a + ab * t)
}
