// SH bake ray generation and BVH traversal: closest hit and segment occlusion.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::{RAY_EPSILON, RaytracingCtx};
use crate::bvh_build::BvhPrimitive;
use crate::ray_traversal::{TraceRay, primitive_triangles, triangle_points};

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
    ray: &TraceRay,
    max_distance: f32,
    best: &mut Option<Hit>,
) -> bool {
    let mut changed = false;
    for tri in primitive_triangles(prim) {
        let points = triangle_points(ctx.geometry, tri);
        let Some(hit) = ray.hit(points, RAY_EPSILON, max_distance) else {
            continue;
        };
        let dist = hit.t as f32;
        if best.as_ref().is_none_or(|b| dist < b.distance) {
            *best = Some(Hit {
                texture_index: prim.material_bucket_id,
                point: ray.origin() + ray.direction() * dist,
                normal: facing_normal(points, hit.front),
                distance: dist,
                triangle: tri as u32,
            });
            changed = true;
        }
    }
    changed
}

/// The triangle's geometric normal, flipped toward a ray that meets its
/// `front` or back, so indirect illumination does not vanish at back-facing
/// walls.
fn facing_normal([a, b, c]: [Vec3; 3], front: bool) -> Vec3 {
    let normal = (b - a).cross(c - a).normalize_or_zero();
    if front { normal } else { -normal }
}

/// A probe gather ray's hit: the nearest surface along `dir` from `origin`,
/// stepped `RAY_EPSILON` off the origin. Every gather source reads its hit
/// here, so the distance the depth moments take is the same whatever the
/// hit's radiance comes from.
pub(crate) fn gather_hit(ctx: &RaytracingCtx<'_>, origin: Vec3, dir: Vec3) -> Option<Hit> {
    #[cfg(test)]
    crate::bake_counters::bump(|counts| counts.gather_rays += 1);
    closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY)
}

/// A later-bounce gather ray's hit (`surface_radiance::bounce`): the nearest
/// surface along `dir` from a patch centroid, stepped `RAY_EPSILON` off the
/// origin as [`gather_hit`] steps a probe's. Counted apart from probe
/// gather rays, so the probe-side cost counters do not move with the
/// patch count.
pub(crate) fn bounce_hit(ctx: &RaytracingCtx<'_>, origin: Vec3, dir: Vec3) -> Option<Hit> {
    #[cfg(test)]
    crate::bake_counters::bump(|counts| counts.bounce_rays += 1);
    closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY)
}

pub(super) fn closest_hit(
    ctx: &RaytracingCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    #[cfg(test)]
    crate::bake_counters::begin_ray(crate::bake_counters::RayKind::Gather);
    let ray = TraceRay::new(ray_origin, ray_dir);
    let query = ray.query(max_distance);
    let mut best: Option<Hit> = None;
    for prim in ctx.bvh.traverse_iterator(&query, ctx.primitives) {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.leaf_visits += 1);
        if closest_hit_in_leaf(ctx, prim, &ray, max_distance, &mut best)
            && let Some(hit) = &best
        {
            query.shrink_to(hit.distance);
        }
    }
    best.filter(|h| h.distance < max_distance)
}

/// Whether nothing blocks `from → to`: the lightmap tracer's segment test
/// over the SH context's tree.
pub(crate) fn segment_clear(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
    crate::ray_traversal::segment_clear(ctx.bvh, ctx.primitives, ctx.geometry, from, to)
}

/// The brute-force reference: [`closest_hit`]'s answer with every triangle
/// tested by the shipped hit test, no BVH and no box test. On a distance tie
/// it may name another triangle than the traversal.
#[cfg(test)]
pub(super) fn closest_hit_brute_force(
    ctx: &RaytracingCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_distance: f32,
) -> Option<Hit> {
    let ray = TraceRay::new(ray_origin, ray_dir);
    let mut best: Option<Hit> = None;
    for prim in ctx.primitives {
        closest_hit_in_leaf(ctx, prim, &ray, max_distance, &mut best);
    }
    best.filter(|h| h.distance < max_distance)
}

/// The brute-force reference for [`segment_clear`].
#[cfg(test)]
pub(super) fn segment_clear_brute_force(ctx: &RaytracingCtx<'_>, from: Vec3, to: Vec3) -> bool {
    crate::ray_traversal::segment_clear_brute_force(ctx.geometry, from, to)
}
