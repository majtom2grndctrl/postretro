// Bake occlusion segments over the compiler BVH, with the lightmap's remembered occluder.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::aabb::{Aabb, IntersectsAabb};
use bvh::bounding_hierarchy::BHShape;
use bvh::bvh::Bvh;
use glam::Vec3;

use super::pad::leaf_box;
use super::{TraceRay, primitive_triangles, triangle_points};
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;

/// Shadow ray self-intersection offset: a segment's trace starts this far
/// along it and tests to its end. The lightmap and animated
/// weight-map bakers share it; they must agree or chunk boundaries show seams.
pub(crate) const RAY_EPSILON: f32 = 1.0e-3;

/// Whether nothing blocks the segment `from → to`, either face of a triangle
/// counting. The trace starts [`RAY_EPSILON`] along the segment and tests to
/// its end.
pub(crate) fn segment_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    segment_clear_remembering(bvh, primitives, geometry, from, to, &Cell::new(None))
}

/// A remembered occluder: the triangle that last blocked a segment, by its
/// first index-buffer offset, and the box the tree stores for the leaf that
/// holds it, copied when it is remembered so the gate reads no tree nodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Occluder {
    leaf_box: Aabb<f32, 3>,
    triangle: u32,
}

impl Occluder {
    /// Triangle `triangle` of `prim`, a primitive of `bvh`.
    pub(crate) fn new(
        bvh: &Bvh<f32, 3>,
        primitives: &[BvhPrimitive],
        prim: &BvhPrimitive,
        triangle: usize,
    ) -> Self {
        Self {
            leaf_box: leaf_box(bvh, primitives, prim.bh_node_index()),
            triangle: triangle as u32,
        }
    }
}

/// [`segment_clear`] that tests `last_occluder` first and records the
/// triangle that blocks this segment. Neighbouring texels lit by one light
/// are mostly blocked by the same triangle, so a sequential walk over one
/// `(light, chart)` skips most traversals in shadow.
///
/// The remembered triangle is tested only when its leaf box passes the
/// traversal's own query, the box the iterator tests before yielding that
/// leaf. A leaf box that passes has ancestors that pass (each stored box
/// holds the ones below it, and the predicate is monotone in containment),
/// so the triangle is one of the traversal's own candidates: the cache
/// reorders the traversal's work and never changes the answer.
pub(crate) fn segment_clear_remembering(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    from: Vec3,
    to: Vec3,
    last_occluder: &Cell<Option<Occluder>>,
) -> bool {
    #[cfg(test)]
    {
        crate::bake_counters::bump(|counts| counts.shadow_rays += 1);
        crate::bake_counters::begin_ray(crate::bake_counters::RayKind::Shadow);
    }
    let Some((ray, max_distance)) = segment_ray(from, to) else {
        return true;
    };
    let blocks = |tri: usize| {
        ray.hit(triangle_points(geometry, tri), 0.0, max_distance)
            .is_some()
    };
    let query = ray.query(max_distance);
    if let Some(occluder) = last_occluder.get()
        && (cache_gate_off() || query.intersects_aabb(&occluder.leaf_box))
        && blocks(occluder.triangle as usize)
    {
        return false;
    }
    for prim in bvh.traverse_iterator(&query, primitives) {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.leaf_visits += 1);
        if let Some(tri) = primitive_triangles(prim).find(|&tri| blocks(tri)) {
            last_occluder.set(Some(Occluder::new(bvh, primitives, prim, tri)));
            return false;
        }
    }
    true
}

/// Whether no triangle crosses `ray` strictly between 0 and `max_distance`,
/// either face counting: the occlusion walk behind every segment tracer whose
/// segment is not [`segment_clear`]'s.
pub(crate) fn ray_clear(
    bvh: &Bvh<f32, 3>,
    primitives: &[BvhPrimitive],
    geometry: &GeometryResult,
    ray: &TraceRay,
    max_distance: f32,
) -> bool {
    let query = ray.query(max_distance);
    !bvh.traverse_iterator(&query, primitives).any(|prim| {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.leaf_visits += 1);
        primitive_triangles(prim).any(|tri| {
            ray.hit(triangle_points(geometry, tri), 0.0, max_distance)
                .is_some()
        })
    })
}

#[cfg(not(test))]
#[inline(always)]
fn cache_gate_off() -> bool {
    false
}

#[cfg(test)]
thread_local! {
    static CACHE_GATE_OFF: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
fn cache_gate_off() -> bool {
    CACHE_GATE_OFF.with(Cell::get)
}

/// Run `work` with the remembered occluder tested before its leaf box: the
/// cache as it was before the gate, for tests that show what the gate fixes.
#[cfg(test)]
pub(crate) fn without_cache_gate<R>(work: impl FnOnce() -> R) -> R {
    let previous = CACHE_GATE_OFF.with(|cell| cell.replace(true));
    let result = work();
    CACHE_GATE_OFF.with(|cell| cell.set(previous));
    result
}

/// The ray [`segment_clear`] traces for `from → to` and its far bound, or
/// `None` for a segment too short to hold one.
#[inline]
fn segment_ray(from: Vec3, to: Vec3) -> Option<(TraceRay, f32)> {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return None;
    }
    let dir = delta / length;
    Some((
        TraceRay::new(from + dir * RAY_EPSILON, dir),
        length - RAY_EPSILON,
    ))
}

/// The brute-force reference: [`segment_clear`]'s answer with every triangle
/// of `geometry` tested by the shipped hit test, no BVH and no box test.
#[cfg(test)]
pub(crate) fn segment_clear_brute_force(geometry: &GeometryResult, from: Vec3, to: Vec3) -> bool {
    segment_ray(from, to)
        .is_none_or(|(ray, max_distance)| ray_clear_brute_force(geometry, &ray, 0.0, max_distance))
}

/// [`ray_clear`]'s brute-force reference, from `t_min` on.
#[cfg(test)]
pub(crate) fn ray_clear_brute_force(
    geometry: &GeometryResult,
    ray: &TraceRay,
    t_min: f32,
    max_distance: f32,
) -> bool {
    !every_triangle(geometry).any(|tri| {
        ray.hit(triangle_points(geometry, tri), t_min, max_distance)
            .is_some()
    })
}

/// Every triangle of `geometry`'s faces, by first index-buffer offset, in
/// index order: the set the BVH holds.
#[cfg(test)]
pub(crate) fn every_triangle(geometry: &GeometryResult) -> impl Iterator<Item = usize> + '_ {
    geometry.face_index_ranges.iter().flat_map(|range| {
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        (start..end)
            .step_by(3)
            .take_while(move |&tri| tri + 3 <= end)
    })
}
