// Bake occlusion segments over the compiler BVH, with the lightmap's remembered occluder.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::bvh::Bvh;
use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};

use super::BoundedRay;
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;

/// Shadow ray self-intersection offset: a segment's trace starts this far
/// along it and stops this far short of its end. The lightmap and animated
/// weight-map bakers share it; they must agree or chunk boundaries show seams.
pub(crate) const RAY_EPSILON: f32 = 1.0e-3;

pub(crate) fn segment_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    segment_clear_remembering(bvh, primitives, geometry, from, to, &Cell::new(None))
}

/// [`segment_clear`] that tests `last_occluder` first and records the triangle
/// that blocks this segment. Neighbouring texels lit by one light are mostly
/// blocked by the same triangle, so a sequential walk over one `(light, chart)`
/// skips most traversals in shadow. The cached triangle is tested with the same
/// ray and hit predicate as the traversal, so it changes only which triangle is
/// found first, not the answer — barring a hit the traversal's box test misses
/// by rounding, which the cache then reports as blocked. That miss needs the
/// hit on the box's boundary: at an axis-aligned edge or a vertex of the
/// triangle, at any ray angle. `last_occluder` holds the triangle's first
/// index-buffer offset into `geometry`.
pub(crate) fn segment_clear_remembering(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
    last_occluder: &Cell<Option<usize>>,
) -> bool {
    #[cfg(test)]
    {
        crate::bake_counters::bump(|counts| counts.shadow_rays += 1);
        crate::bake_counters::begin_ray(crate::bake_counters::RayKind::Shadow);
    }
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    let blocks = |tri: usize| {
        let p0 = Vec3::from(geom.vertices[geom.indices[tri] as usize].position);
        let p1 = Vec3::from(geom.vertices[geom.indices[tri + 1] as usize].position);
        let p2 = Vec3::from(geom.vertices[geom.indices[tri + 2] as usize].position);
        ray_triangle_hit(origin, dir, p0, p1, p2)
            .is_some_and(|dist| dist > 0.0 && dist < max_distance)
    };
    if last_occluder.get().is_some_and(blocks) {
        return false;
    }
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    let query = BoundedRay::new(&ray, max_distance);
    for prim in bvh.traverse_iterator(&query, primitives) {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.leaf_visits += 1);
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            if blocks(tri) {
                last_occluder.set(Some(tri));
                return false;
            }
            tri += 3;
        }
    }
    true
}

/// The unbounded reference scan: the occlusion answer [`segment_clear`] must
/// match.
#[cfg(test)]
pub(crate) fn segment_clear_full_scan(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return true;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let ray = Ray::new(
        Point3::new(origin.x, origin.y, origin.z),
        Vector3::new(dir.x, dir.y, dir.z),
    );
    let max_distance = length - RAY_EPSILON;
    let geom = &geometry.geometry;
    for prim in bvh.traverse_iterator(&ray, primitives) {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        let mut tri = start;
        while tri + 3 <= end {
            let i0 = geom.indices[tri] as usize;
            let i1 = geom.indices[tri + 1] as usize;
            let i2 = geom.indices[tri + 2] as usize;
            tri += 3;
            let p0 = Vec3::from(geom.vertices[i0].position);
            let p1 = Vec3::from(geom.vertices[i1].position);
            let p2 = Vec3::from(geom.vertices[i2].position);
            if let Some(dist) = ray_triangle_hit(origin, dir, p0, p1, p2)
                && dist > 0.0
                && dist < max_distance
            {
                return false;
            }
        }
    }
    true
}

/// Double-sided Möller-Trumbore intersection.
pub(crate) fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    #[cfg(test)]
    crate::bake_counters::bump_trace(|trace| trace.triangle_tests += 1);
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < 1.0e-8 {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.det_rejections += 1);
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    if t <= 0.0 { None } else { Some(t) }
}
