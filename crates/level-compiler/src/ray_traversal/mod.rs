// The bake tracer: the watertight hit test, bounded BVH traversal and occlusion segments.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::aabb::{Aabb, IntersectsAabb};
use glam::Vec3;

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;

mod hit_test;
mod occlusion;
mod pad;

pub(crate) use hit_test::{TRACER_TOLERANCE_METERS, TraceRay, closest_point_on_triangle};
#[cfg(test)]
pub(crate) use hit_test::{with_planted_t, with_tolerance};
pub(crate) use occlusion::{
    Occluder, RAY_EPSILON, ray_clear, segment_clear, segment_clear_remembering,
};
#[cfg(test)]
pub(crate) use occlusion::{ray_clear_brute_force, segment_clear_brute_force, without_cache_gate};
#[cfg(test)]
pub(crate) use pad::assert_parents_contain_children;
pub(crate) use pad::pad_bake_tree;
#[cfg(test)]
pub(crate) use pad::{assert_padded, box_pad};

/// Epoch of the answers this tracer gives: the hit test, τ, the box test,
/// the bake tree's padding and the occluder-cache gate. Bump it on any
/// change that can move a shadow or closest-hit answer. Every cached stage
/// whose computation reaches the tracer takes it as a key input, as the
/// residency stage folds `PORTAL_WALK_EPOCH`, so one bump re-keys them all
/// (`build_pipeline.md` §Build Cache).
pub(crate) const TRACER_EPOCH: u32 = 3;

/// The first index-buffer offsets of a BVH primitive's triangles.
pub(crate) fn primitive_triangles(prim: &BvhPrimitive) -> impl Iterator<Item = usize> {
    let start = prim.index_offset as usize;
    let end = start + prim.index_count as usize;
    (start..end)
        .step_by(3)
        .take_while(move |&tri| tri + 3 <= end)
}

/// The stored `f32` corners of the triangle at index `tri`, in index order.
/// Every triangle test reads it, so it is always inlined and spelled out
/// corner by corner: an array `map` here compiled to an out-of-line call per
/// test.
#[inline(always)]
pub(crate) fn triangle_points(geometry: &GeometryResult, tri: usize) -> [Vec3; 3] {
    let geom = &geometry.geometry;
    let corner = |k: usize| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position);
    [corner(0), corner(1), corner(2)]
}

/// A ray query for `Bvh::traverse_iterator`: a conservative slab test,
/// minus boxes the ray enters clearly beyond `bound`. It accepts every box
/// the stock ray test accepts and more: the far slab distance grows by
/// [`FAR_SCALE`] (pbrt-v4, Ize 2013), so rounding never rejects a box the
/// exact ray meets, and an axis whose slab is NaN (the ray lies in a face
/// plane of the box) constrains nothing. With the bake tree's padded boxes
/// (`pad_bake_tree`), every box holding a hit the hit test accepts passes.
/// The iterator's visit order is unchanged, so the hit a caller keeps on a
/// distance tie is unchanged too. A closest-hit caller shrinks the bound as
/// nearer hits land; an occlusion caller fixes it at the segment end.
///
/// Faster on large maps than a near-child-first walk, whose extra child-entry
/// work outweighs its earlier pruning. A walk over a packed copy of the nodes,
/// in this same order, measured no faster on warren-mini once this node test
/// was one pass, and slower when it tested both children's boxes up front.
pub(crate) struct BoundedRay {
    origin: [f32; 3],
    inv_direction: [f32; 3],
    bound: Cell<f32>,
    /// [`prune_limit`] of `bound`, kept beside it so a box test reads it.
    limit: Cell<f32>,
    far_scale: f32,
}

/// 1 + 2γ₃ (γₙ = nu / (1 − nu), u = 2⁻²⁴), rounded up to the next `f32`:
/// a slab distance's rounding is within γ₃ of exact, so a far distance
/// scaled by it is no nearer than the exact one (pbrt-v4 `Bounds3::IntersectP`).
const FAR_SCALE: f32 = 1.0 + 4.0 * f32::EPSILON;

impl BoundedRay {
    /// The query for a ray from `origin` along `direction`, the hit test's
    /// own: the inverse is taken here, never re-normalized.
    #[inline]
    pub(crate) fn new(origin: Vec3, direction: Vec3, bound: f32) -> Self {
        Self {
            origin: origin.to_array(),
            inv_direction: direction.recip().to_array(),
            bound: Cell::new(bound),
            limit: Cell::new(prune_limit(bound)),
            far_scale: far_scale(),
        }
    }

    /// Lower the bound to `distance`. Callers pass only hits they keep, so a
    /// rejected hit (behind the origin epsilon, past `max_distance`) never
    /// prunes anything.
    pub(crate) fn shrink_to(&self, distance: f32) {
        if distance < self.bound.get() {
            self.bound.set(distance);
            self.limit.set(prune_limit(distance));
        }
    }

    /// The conservative slab test alone: the entry distance when the ray may
    /// meet the box `[min, max]`, `None` when it cannot. An axis whose slab
    /// value is NaN, `0 · ∞` for a ray lying in that face plane, is dropped
    /// whole for either sign of zero: the closed box holds the ray's line on
    /// that axis. Entry is negative when the origin is inside.
    ///
    /// A dropped axis takes the whole line as its slab rather than skipping
    /// the loop body, and with NaN masked out every min and max is a plain
    /// comparison, so the loop unrolls into straight-line code; an early
    /// `continue` here kept it a loop over spilled box values.
    #[inline]
    fn conservative_entry(&self, min: &[f32; 3], max: &[f32; 3]) -> Option<f32> {
        let mut entry = f32::NEG_INFINITY;
        let mut exit = f32::INFINITY;
        for axis in 0..3 {
            let a = (min[axis] - self.origin[axis]) * self.inv_direction[axis];
            let b = (max[axis] - self.origin[axis]) * self.inv_direction[axis];
            let dropped = a.is_nan() | b.is_nan();
            let (near, far) = if a < b { (a, b) } else { (b, a) };
            let near = if dropped { f32::NEG_INFINITY } else { near };
            let far = if dropped {
                f32::INFINITY
            } else {
                far * self.far_scale
            };
            entry = if near > entry { near } else { entry };
            exit = if far < exit { far } else { exit };
        }
        (exit >= entry.max(0.0)).then_some(entry)
    }

    /// Whether a box entered at `entry` lies within the current bound.
    #[inline]
    fn within_bound(&self, entry: f32) -> bool {
        entry <= self.limit.get()
    }
}

impl IntersectsAabb<f32, 3> for BoundedRay {
    /// The conservative test and the bound prune in one pass over the six
    /// slab values.
    #[inline]
    fn intersects_aabb(&self, aabb: &Aabb<f32, 3>) -> bool {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.box_tests += 1);
        let min = [aabb.min.x, aabb.min.y, aabb.min.z];
        let max = [aabb.max.x, aabb.max.y, aabb.max.z];
        self.conservative_entry(&min, &max)
            .is_some_and(|entry| self.within_bound(entry))
    }
}

/// Bound used for pruning. A box is skipped only when it starts clearly
/// beyond the bound. The padded box holds every point the hit test accepts,
/// so its exact entry is no later than an accepted hit's exact distance; two
/// rounding errors remain between the computed values: the slab test's entry
/// (a few ulps, covered by the relative term) and the hit test's `t`, which
/// in `f64` grows only as a ray turns into a triangle's plane. The 1 cm
/// absolute term covers both at little pruning cost: it keeps only boxes that
/// start within about 1 cm plus 0.01% of the bound. An infinite bound prunes
/// nothing.
fn prune_limit(bound: f32) -> f32 {
    bound + bound.abs() * 1e-4 + 1e-2
}

#[cfg(not(test))]
#[inline(always)]
fn far_scale() -> f32 {
    FAR_SCALE
}

#[cfg(test)]
thread_local! {
    static STOCK_FAR_BOUND: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
fn far_scale() -> f32 {
    if STOCK_FAR_BOUND.with(Cell::get) {
        1.0
    } else {
        FAR_SCALE
    }
}

/// Run `work` with box queries built on this thread unscaled: the
/// conservative factor removed, for tests that show what it catches.
#[cfg(test)]
pub(crate) fn without_far_scale<R>(work: impl FnOnce() -> R) -> R {
    let previous = STOCK_FAR_BOUND.with(|cell| cell.replace(true));
    let result = work();
    STOCK_FAR_BOUND.with(|cell| cell.set(previous));
    result
}

#[cfg(test)]
mod census;
#[cfg(test)]
mod census_tests;
#[cfg(test)]
mod cost_tests;

#[cfg(test)]
mod hit_test_tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tree_tests;

#[cfg(test)]
mod tests {
    use bvh::aabb::{Aabb, IntersectsAabb};
    use bvh::ray::Ray;
    use glam::Vec3;
    use nalgebra::{Point3, Vector3};
    use proptest::prelude::*;

    use super::{BoundedRay, prune_limit};

    /// The stock test with the bound prune: the slab test the conservative
    /// one replaced, then the entry distance against the bound.
    fn stock_accepts(ray: &Ray<f32, 3>, aabb: &Aabb<f32, 3>, bound: f32) -> bool {
        let mut entry = f32::NEG_INFINITY;
        for axis in 0..3 {
            let near = (aabb.min[axis] - ray.origin[axis]) * ray.inv_direction[axis];
            let far = (aabb.max[axis] - ray.origin[axis]) * ray.inv_direction[axis];
            entry = entry.max(near.min(far));
        }
        ray.intersects_aabb(aabb) && entry <= prune_limit(bound)
    }

    /// The conservative query for the stock ray's own origin and direction.
    fn query(ray: &Ray<f32, 3>, bound: f32) -> BoundedRay {
        BoundedRay::new(
            Vec3::new(ray.origin.x, ray.origin.y, ray.origin.z),
            Vec3::new(ray.direction.x, ray.direction.y, ray.direction.z),
            bound,
        )
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

    /// A ray lying in a box face plane, parallel to it: its slab on that axis
    /// is `0 · ∞ = NaN`. The stock test rejects the box; the conservative one
    /// drops that axis, for either sign of zero, on the min and the max face,
    /// on a flat box and on one with extent on that axis.
    #[test]
    fn ray_in_a_box_face_plane_is_accepted_for_either_zero_and_face() {
        for (min_x, max_x) in [(0.0f32, 0.0f32), (0.0, 1.0)] {
            let aabb =
                Aabb::with_bounds(Point3::new(min_x, 0.0, 0.0), Point3::new(max_x, 1.0, 1.0));
            for face_x in [min_x, max_x] {
                for zero in [0.0f32, -0.0] {
                    let origin = Vec3::new(face_x, -1.0, 0.5);
                    let direction = Vec3::new(zero, 1.0, 0.0);
                    let stock = Ray::new(
                        Point3::new(origin.x, origin.y, origin.z),
                        Vector3::new(direction.x, direction.y, direction.z),
                    );
                    assert!(
                        !stock.intersects_aabb(&aabb),
                        "stock accepts x = {face_x}, {zero:?}"
                    );
                    assert!(
                        BoundedRay::new(origin, direction, f32::INFINITY).intersects_aabb(&aabb),
                        "conservative rejects x = {face_x} on [{min_x}, {max_x}], {zero:?}"
                    );
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20_000))]

        #[test]
        fn conservative_test_accepts_every_box_the_stock_test_accepts(
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
            if stock_accepts(&ray, &aabb, bound) {
                prop_assert!(query(&ray, bound).intersects_aabb(&aabb));
            }
        }
    }
}
