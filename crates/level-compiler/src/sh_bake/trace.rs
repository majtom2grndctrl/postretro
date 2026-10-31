// SH bake ray generation and BVH traversal: closest hit and segment occlusion.
// See: context/lib/build_pipeline.md §Compiler pipeline

use bvh::ray::Ray;
use glam::Vec3;
use nalgebra::{Point3, Vector3};

use super::{RAY_EPSILON, RaytracingCtx};
use crate::bvh_build::BvhPrimitive;
use crate::ray_traversal::BoundedRay;

/// Fibonacci-sphere directions: evenly-spaced, no RNG, identical input → identical output.
pub(super) fn sphere_directions(count: u32, seed: u64) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(count as usize);
    let phi = std::f32::consts::PI * (3.0 - (5.0_f32).sqrt()); // golden angle
    let seed_offset = ((seed & 0xFFFF_FFFF) as f32) / (u32::MAX as f32);
    for i in 0..count {
        let t = (i as f32 + 0.5) / count as f32;
        let y = 1.0 - 2.0 * t;
        let radius = (1.0 - y * y).max(0.0).sqrt();
        let theta = phi * i as f32 + seed_offset * std::f32::consts::TAU;
        let x = theta.cos() * radius;
        let z = theta.sin() * radius;
        out.push(Vec3::new(x, y, z).normalize());
    }
    out
}

pub(crate) struct Hit {
    pub(crate) texture_index: u32,
    pub(crate) point: Vec3,
    /// The triangle's geometric normal, flipped toward the ray.
    pub(crate) normal: Vec3,
    pub(crate) distance: f32,
    /// Index-buffer offset of the hit triangle's first index: names the
    /// pre-cut face through `GeometryResult::face_index_ranges`.
    pub(crate) triangle: u32,
}

/// Test one leaf's triangles against the ray; keeps the nearest hit in
/// `best`, with a later equal-distance hit never replacing an earlier one.
/// Returns whether `best` changed.
fn closest_hit_in_leaf(
    ctx: &RaytracingCtx<'_>,
    prim: &BvhPrimitive,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
    best: &mut Option<Hit>,
) -> bool {
    let geom = &ctx.geometry.geometry;
    let start = prim.index_offset as usize;
    let end = start + prim.index_count as usize;
    let mut tri = start;
    let mut changed = false;
    while tri + 3 <= end {
        let triangle = tri as u32;
        let i0 = geom.indices[tri] as usize;
        let i1 = geom.indices[tri + 1] as usize;
        let i2 = geom.indices[tri + 2] as usize;
        tri += 3;

        let p0 = Vec3::from(geom.vertices[i0].position);
        let p1 = Vec3::from(geom.vertices[i1].position);
        let p2 = Vec3::from(geom.vertices[i2].position);

        if let Some((dist, normal)) = ray_triangle_hit(ray_origin, ray_dir, p0, p1, p2)
            && dist > RAY_EPSILON
            && dist < max_distance
        {
            let update = best.as_ref().map(|b| dist < b.distance).unwrap_or(true);
            if update {
                *best = Some(Hit {
                    texture_index: prim.material_bucket_id,
                    point: ray_origin + ray_dir * dist,
                    normal,
                    distance: dist,
                    triangle,
                });
                changed = true;
            }
        }
    }
    changed
}

/// A probe gather ray's hit: the nearest surface along `dir` from `origin`,
/// stepped `RAY_EPSILON` off the origin. Every gather source reads its hit
/// here, so the distance the depth moments take is the same whatever the
/// hit's radiance comes from.
pub(crate) fn gather_hit(ctx: &RaytracingCtx<'_>, origin: Vec3, dir: Vec3) -> Option<Hit> {
    closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY)
}

pub(super) fn closest_hit(
    ctx: &RaytracingCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let ray = Ray::new(
        Point3::new(ray_origin.x, ray_origin.y, ray_origin.z),
        Vector3::new(ray_dir.x, ray_dir.y, ray_dir.z),
    );
    let query = BoundedRay::new(&ray, max_distance);
    let mut best: Option<Hit> = None;
    for prim in ctx.bvh.traverse_iterator(&query, ctx.primitives) {
        if closest_hit_in_leaf(ctx, prim, ray_origin, ray_dir, max_distance, &mut best)
            && let Some(hit) = &best
        {
            query.shrink_to(hit.distance);
        }
    }

    if let Some(h) = best.as_mut()
        && h.distance >= max_distance
    {
        return None;
    }
    best
}

/// Double-sided Möller-Trumbore. Normal is flipped toward the incoming ray so
/// indirect illumination does not vanish at back-facing walls.
fn ray_triangle_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec3)> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < 1.0e-8 {
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
    if t <= 0.0 {
        return None;
    }
    let mut normal = edge1.cross(edge2).normalize_or_zero();
    if normal.dot(dir) > 0.0 {
        normal = -normal;
    }
    Some((t, normal))
}

pub(crate) fn segment_clear(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
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
    let geom = &ctx.geometry.geometry;
    let blocks = |prim: &BvhPrimitive| {
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
            if let Some((dist, _)) = ray_triangle_hit(origin, dir, p0, p1, p2)
                && dist > 0.0
                && dist < max_distance
            {
                return true;
            }
        }
        false
    };
    let query = BoundedRay::new(&ray, max_distance);
    !ctx.bvh
        .traverse_iterator(&query, ctx.primitives)
        .any(blocks)
}

/// The unbounded reference scan: the closest hit [`closest_hit`] must match.
#[cfg(test)]
pub(super) fn closest_hit_full_scan(
    ctx: &RaytracingCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let ray = Ray::new(
        Point3::new(ray_origin.x, ray_origin.y, ray_origin.z),
        Vector3::new(ray_dir.x, ray_dir.y, ray_dir.z),
    );
    let mut best: Option<Hit> = None;
    for prim in ctx.bvh.traverse_iterator(&ray, ctx.primitives) {
        closest_hit_in_leaf(ctx, prim, ray_origin, ray_dir, max_distance, &mut best);
    }
    best.filter(|h| h.distance < max_distance)
}

/// The unbounded reference scan: the occlusion answer [`segment_clear`] must
/// match.
#[cfg(test)]
pub(super) fn segment_clear_full_scan(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
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
    let geom = &ctx.geometry.geometry;
    !ctx.bvh.traverse_iterator(&ray, ctx.primitives).any(|prim| {
        let start = prim.index_offset as usize;
        let end = start + prim.index_count as usize;
        (start..end)
            .step_by(3)
            .filter(|tri| tri + 3 <= end)
            .any(|tri| {
                let p =
                    |k: usize| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position);
                ray_triangle_hit(origin, dir, p(0), p(1), p(2))
                    .is_some_and(|(dist, _)| dist > 0.0 && dist < max_distance)
            })
    })
}
