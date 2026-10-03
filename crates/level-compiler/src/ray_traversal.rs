// Bounded BVH traversal for bake rays: skip nodes entered beyond a distance bound.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::aabb::{Aabb, IntersectsAabb};
use bvh::ray::Ray;

/// A ray query for `Bvh::traverse_iterator` that accepts exactly the boxes
/// the stock ray accepts, minus those it enters clearly beyond `bound`. The
/// iterator's visit order is unchanged, so the hit a caller keeps on a distance
/// tie is unchanged too. A closest-hit caller shrinks the bound as nearer hits
/// land; an occlusion caller fixes it at the segment end.
///
/// A box is pruned only past [`prune_limit`]'s slack, so the answer should
/// equal the unbounded traversal's. That equality is verified by the fixture
/// digest gate and the full-scan parity tests, not proven for every float
/// input: a pathological grazing sliver can exceed any finite slack.
///
/// Faster on large maps than a near-child-first walk, whose extra child-entry
/// work outweighs its earlier pruning.
pub(crate) struct BoundedRay<'a> {
    ray: &'a Ray<f32, 3>,
    bound: Cell<f32>,
}

impl<'a> BoundedRay<'a> {
    pub(crate) fn new(ray: &'a Ray<f32, 3>, bound: f32) -> Self {
        Self {
            ray,
            bound: Cell::new(bound),
        }
    }

    /// Lower the bound to `distance`. Callers pass only hits they keep, so a
    /// rejected hit (behind the origin epsilon, past `max_distance`) never
    /// prunes anything.
    pub(crate) fn shrink_to(&self, distance: f32) {
        if distance < self.bound.get() {
            self.bound.set(distance);
        }
    }
}

impl IntersectsAabb<f32, 3> for BoundedRay<'_> {
    fn intersects_aabb(&self, aabb: &Aabb<f32, 3>) -> bool {
        // The stock test first: it rejects the NaN case (a ray lying in a box
        // face plane), so `entry_distance` only sees boxes with a defined slab.
        self.ray.intersects_aabb(aabb)
            && entry_distance(self.ray, aabb) <= prune_limit(self.bound.get())
    }
}

/// Bound used for pruning. A box is skipped only when it starts clearly beyond
/// the bound, because two rounding errors can put a hit's computed distance
/// below the computed entry of the box holding it: the slab test's entry
/// rounding (a few ulps, covered by the relative term), and Möller–Trumbore's
/// own `t` error, which grows with the distance from the ray origin to the
/// triangle, as the ray grazes it, and as its narrowest angle shrinks. The
/// 1 cm absolute term covers that error for well-shaped triangles at ordinary
/// angles, at little pruning cost: it keeps only boxes that start within about
/// 1 cm plus 0.01% of the bound. Long grazing rays near a sliver's ends can exceed it. An
/// infinite bound prunes nothing.
fn prune_limit(bound: f32) -> f32 {
    bound + bound.abs() * 1e-4 + 1e-2
}

/// Distance along `ray` at which it enters `aabb`; negative when the origin
/// is inside.
fn entry_distance(ray: &Ray<f32, 3>, aabb: &Aabb<f32, 3>) -> f32 {
    let mut entry = f32::NEG_INFINITY;
    for axis in 0..3 {
        let near = (aabb.min[axis] - ray.origin[axis]) * ray.inv_direction[axis];
        let far = (aabb.max[axis] - ray.origin[axis]) * ray.inv_direction[axis];
        entry = entry.max(near.min(far));
    }
    entry
}
