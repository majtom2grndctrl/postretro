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
/// work outweighs its earlier pruning. A walk over a packed copy of the nodes,
/// in this same order, measured no faster on warren-mini once this node test
/// was one pass, and slower when it tested both children's boxes up front.
pub(crate) struct BoundedRay {
    origin: [f32; 3],
    inv_direction: [f32; 3],
    bound: Cell<f32>,
}

impl BoundedRay {
    pub(crate) fn new(ray: &Ray<f32, 3>, bound: f32) -> Self {
        Self {
            origin: [ray.origin.x, ray.origin.y, ray.origin.z],
            inv_direction: [
                ray.inv_direction.x,
                ray.inv_direction.y,
                ray.inv_direction.z,
            ],
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

    /// The stock slab test alone: the entry distance when the stock ray
    /// accepts the box `[min, max]`, `None` when it rejects it. A NaN slab
    /// value (the ray lies in a box face plane) rejects, as the stock test
    /// does; with NaN excluded, the per-axis min and max are unambiguous up to
    /// the sign of zero, which no comparison here distinguishes. Entry is
    /// negative when the origin is inside.
    #[inline]
    fn stock_entry(&self, min: &[f32; 3], max: &[f32; 3]) -> Option<f32> {
        let mut entry = f32::NEG_INFINITY;
        let mut exit = f32::INFINITY;
        let mut nan = false;
        for axis in 0..3 {
            let a = (min[axis] - self.origin[axis]) * self.inv_direction[axis];
            let b = (max[axis] - self.origin[axis]) * self.inv_direction[axis];
            nan |= a.is_nan() | b.is_nan();
            entry = entry.max(a.min(b));
            exit = exit.min(a.max(b));
        }
        (!nan && exit >= entry.max(0.0)).then_some(entry)
    }

    /// Whether a box entered at `entry` lies within the current bound.
    #[inline]
    fn within_bound(&self, entry: f32) -> bool {
        entry <= prune_limit(self.bound.get())
    }
}

impl IntersectsAabb<f32, 3> for BoundedRay {
    /// The stock test and the bound prune in one pass over the six slab values.
    #[inline]
    fn intersects_aabb(&self, aabb: &Aabb<f32, 3>) -> bool {
        let min = [aabb.min.x, aabb.min.y, aabb.min.z];
        let max = [aabb.max.x, aabb.max.y, aabb.max.z];
        self.stock_entry(&min, &max)
            .is_some_and(|entry| self.within_bound(entry))
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

#[cfg(test)]
mod tests {
    use bvh::aabb::{Aabb, IntersectsAabb};
    use bvh::ray::Ray;
    use nalgebra::{Point3, Vector3};
    use proptest::prelude::*;

    use super::{BoundedRay, prune_limit};

    /// The two-pass test the fused one replaced: the stock slab test, then the
    /// entry distance recomputed against the bound.
    fn two_pass_accepts(ray: &Ray<f32, 3>, aabb: &Aabb<f32, 3>, bound: f32) -> bool {
        let mut entry = f32::NEG_INFINITY;
        for axis in 0..3 {
            let near = (aabb.min[axis] - ray.origin[axis]) * ray.inv_direction[axis];
            let far = (aabb.max[axis] - ray.origin[axis]) * ray.inv_direction[axis];
            entry = entry.max(near.min(far));
        }
        ray.intersects_aabb(aabb) && entry <= prune_limit(bound)
    }

    /// Coordinates drawn from a small lattice so origins land on box faces and
    /// directions are often axis-parallel, which exercises the NaN and
    /// infinite slab cases a uniform draw would almost never hit.
    fn coord() -> impl Strategy<Value = f32> {
        prop_oneof![(-4i32..=4).prop_map(|v| v as f32), -8.0f32..8.0,]
    }

    /// Lattice components include both signed zeros, so an axis-parallel ray
    /// carries an inverse component of `+inf` or `-inf`.
    fn direction() -> impl Strategy<Value = Vector3<f32>> {
        let lattice = || prop_oneof![Just(-1.0f32), Just(-0.0f32), Just(0.0f32), Just(1.0f32)];
        prop_oneof![
            (lattice(), lattice(), lattice()).prop_map(|(x, y, z)| Vector3::new(x, y, z)),
            (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0).prop_map(|(x, y, z)| Vector3::new(x, y, z)),
        ]
        .prop_filter("non-zero", |d| d.norm() > 1e-3)
    }

    fn bound() -> impl Strategy<Value = f32> {
        prop_oneof![
            Just(f32::INFINITY),
            Just(0.0f32),
            (0i32..=12).prop_map(|v| v as f32),
            0.0f32..20.0,
        ]
    }

    #[test]
    fn ray_in_a_box_face_plane_is_rejected_like_the_stock_test() {
        // Origin on the box's x = 0 face, direction along +y: the x slab is
        // 0 * inf = NaN.
        let ray = Ray::new(Point3::new(0.0, -1.0, 0.5), Vector3::new(0.0, 1.0, 0.0));
        let aabb = Aabb::with_bounds(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        assert!(!ray.intersects_aabb(&aabb));
        assert!(!BoundedRay::new(&ray, f32::INFINITY).intersects_aabb(&aabb));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20_000))]

        #[test]
        fn fused_test_accepts_exactly_the_two_pass_set(
            origin in (coord(), coord(), coord()),
            dir in direction(),
            corner_a in (coord(), coord(), coord()),
            corner_b in (coord(), coord(), coord()),
            bound in bound(),
        ) {
            let ray = Ray::new(Point3::new(origin.0, origin.1, origin.2), dir);
            let aabb = Aabb::with_bounds(
                Point3::new(corner_a.0.min(corner_b.0), corner_a.1.min(corner_b.1), corner_a.2.min(corner_b.2)),
                Point3::new(corner_a.0.max(corner_b.0), corner_a.1.max(corner_b.1), corner_a.2.max(corner_b.2)),
            );
            let fused = BoundedRay::new(&ray, bound).intersects_aabb(&aabb);
            prop_assert_eq!(fused, two_pass_accepts(&ray, &aabb, bound));
        }
    }
}
