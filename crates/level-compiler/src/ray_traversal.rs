// Bounded BVH traversal for bake rays: skip nodes entered beyond a distance bound.
// See: context/plans/in-progress/bake-parallelism-large-maps/index.md (lever 5)

use std::cell::Cell;
use std::ops::ControlFlow;

use bvh::aabb::{Aabb, Bounded, IntersectsAabb};
use bvh::bvh::{Bvh, BvhNode};
use bvh::ray::Ray;

/// Bound used for pruning: a hit inside a box can compute a box entry a few
/// ulps past its own distance, so a node is skipped only when it starts
/// clearly beyond the bound.
fn prune_limit(bound: f32) -> f32 {
    bound + bound.abs() * 1e-5 + 1e-5
}

/// Distance along `ray` at which it enters `aabb`. Only meaningful for a box
/// the stock slab test accepted; that test already rejects the NaN case (a ray
/// lying in a box face plane).
fn entry_distance(ray: &Ray<f32, 3>, aabb: &Aabb<f32, 3>) -> f32 {
    let mut entry = f32::NEG_INFINITY;
    for axis in 0..3 {
        let near = (aabb.min[axis] - ray.origin[axis]) * ray.inv_direction[axis];
        let far = (aabb.max[axis] - ray.origin[axis]) * ray.inv_direction[axis];
        entry = entry.max(near.min(far));
    }
    entry
}

/// Whether the stock slab test accepts `aabb` and the ray enters it within
/// `bound`.
fn accepts(ray: &Ray<f32, 3>, aabb: &Aabb<f32, 3>, bound: f32) -> Option<f32> {
    if !ray.intersects_aabb(aabb) {
        return None;
    }
    let entry = entry_distance(ray, aabb);
    (entry <= prune_limit(bound)).then_some(entry)
}

/// A ray query for `Bvh::traverse_iterator` that also rejects nodes entered
/// beyond `bound`. The iterator's visit order is unchanged, so the first hit
/// a caller keeps on a distance tie is unchanged too. Callers shrink `bound`
/// as nearer hits land.
pub(crate) struct BoundedRay<'a> {
    pub(crate) ray: &'a Ray<f32, 3>,
    pub(crate) bound: &'a Cell<f32>,
}

impl IntersectsAabb<f32, 3> for BoundedRay<'_> {
    fn intersects_aabb(&self, aabb: &Aabb<f32, 3>) -> bool {
        accepts(self.ray, aabb, self.bound.get()).is_some()
    }
}

/// Visit the leaves `ray` reaches within `bound`, nearer child first. The
/// visitor gets each leaf's shape index and its node index; node indices are
/// assigned in pre-order with the left child first, so they rank leaves in the
/// stock iterator's visit order and break distance ties the way it does.
/// Returning `Break` stops the walk.
pub(crate) fn walk_near_first<S, F>(
    bvh: &Bvh<f32, 3>,
    shapes: &[S],
    ray: &Ray<f32, 3>,
    bound: &Cell<f32>,
    mut visit: F,
) where
    S: Bounded<f32, 3>,
    F: FnMut(usize, usize) -> ControlFlow<()>,
{
    match bvh.nodes.first() {
        None => return,
        Some(BvhNode::Leaf { shape_index, .. }) => {
            if accepts(ray, &shapes[*shape_index].aabb(), bound.get()).is_some() {
                let _ = visit(*shape_index, 0);
            }
            return;
        }
        Some(BvhNode::Node { .. }) => {}
    }

    // One pending sibling per level; the stock iterator's own stack is 32 deep.
    let mut stack = [(0usize, 0.0f32); 64];
    let mut len = 1;
    stack[0] = (0, f32::NEG_INFINITY);
    while len > 0 {
        len -= 1;
        let (node_index, entry) = stack[len];
        if entry > prune_limit(bound.get()) {
            continue;
        }
        match &bvh.nodes[node_index] {
            BvhNode::Leaf { shape_index, .. } => {
                if visit(*shape_index, node_index).is_break() {
                    return;
                }
            }
            BvhNode::Node {
                child_l_index,
                child_l_aabb,
                child_r_index,
                child_r_aabb,
                ..
            } => {
                let limit = bound.get();
                let left = accepts(ray, child_l_aabb, limit).map(|t| (*child_l_index, t));
                let right = accepts(ray, child_r_aabb, limit).map(|t| (*child_r_index, t));
                let (first, second) = match (left, right) {
                    // Equal entries keep the left child first.
                    (Some(l), Some(r)) if r.1 < l.1 => (Some(r), Some(l)),
                    (l, r) => (l.or(r), l.and(r)),
                };
                // Push the farther child first so the nearer one pops next.
                for child in [second, first].into_iter().flatten() {
                    stack[len] = child;
                    len += 1;
                }
            }
        }
    }
}
