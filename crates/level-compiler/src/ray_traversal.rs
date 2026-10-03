// Bounded BVH traversal for bake rays: skip nodes entered beyond a distance bound.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::aabb::{Aabb, IntersectsAabb};
use bvh::ray::Ray;

/// A ray query for `Bvh::traverse_iterator` that accepts exactly the boxes
/// the stock ray accepts, minus those it enters beyond `bound`. The iterator's
/// visit order is unchanged, so the hit a caller keeps on a distance tie is
/// unchanged too. A closest-hit caller shrinks the bound as nearer hits land;
/// an occlusion caller fixes it at the segment end.
///
/// Measured on the hallway's SH groups (bake-parallelism-large-maps Task 10):
/// 19% faster than the unbounded iterator, and faster than a near-child-first
/// walk, whose extra child-entry work outweighed its earlier pruning.
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

/// Bound used for pruning: a hit inside a box can compute a box entry a few
/// ulps past its own distance, so a box is skipped only when it starts clearly
/// beyond the bound. An infinite bound prunes nothing.
fn prune_limit(bound: f32) -> f32 {
    bound + bound.abs() * 1e-5 + 1e-5
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
