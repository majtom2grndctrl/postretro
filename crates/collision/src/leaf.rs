// Convex leaves (plane sets with bounds) and the contents mask that filters queries over them.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use std::ops::{BitOr, BitOrAssign};

use glam::Vec3;

use crate::bvh::line_overlaps_box;
use crate::polytope;

/// Leaf bounds are widened by this relative margin so rounding in a supplied
/// AABB can never reject a point or line the planes contain.
const BOUNDS_RELATIVE_PAD: f32 = 1.0e-5;

/// Which regional fields a volume carries. A query names the fields it wants and
/// never sees a volume whose contents miss all of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ContentsMask(u32);

impl ContentsMask {
    pub const EMPTY: Self = Self(0);
    pub const FLUID: Self = Self(1 << 0);
    pub const GRAVITY: Self = Self(1 << 1);
    pub const PUSH: Self = Self(1 << 2);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Every bit of `other` is set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// At least one bit is shared.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for ContentsMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl BitOrAssign for ContentsMask {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// One bounding plane of a convex leaf, in engine space. The leaf's interior is
/// the side where `dot(p, normal) - distance <= 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafPlane {
    pub normal: Vec3,
    pub distance: f32,
}

impl LeafPlane {
    /// Signed distance of `point` from the plane (scaled by the normal's length);
    /// positive is outside.
    pub fn side(&self, point: Vec3) -> f32 {
        self.normal.dot(point) - self.distance
    }
}

/// A convex solid: the intersection of its planes' inner half-spaces. The AABB
/// encloses the solid and is used only to prefilter queries; containment is
/// decided by the planes.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvexLeaf {
    pub planes: Vec<LeafPlane>,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
}

impl ConvexLeaf {
    /// A point lies inside when it is on the inner side of every plane. A point
    /// exactly on a face counts as inside.
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes.iter().all(|plane| plane.side(point) <= 0.0)
    }

    /// Exact volume of the convex solid, in cubic metres: the AABB box clipped by
    /// every plane, then split into tetrahedra about an interior point. A plane
    /// set that encloses nothing has zero volume.
    pub fn volume(&self) -> f32 {
        polytope::clipped_box_volume(&self.planes, self.aabb_min, self.aabb_max)
    }

    /// The smallest normalized inward plane distance at `point`: positive
    /// inside, zero on the boundary, negative outside. Conservative both ways:
    /// its magnitude never exceeds the true distance to the solid's boundary.
    /// A plane with a zero or non-finite normal bounds nothing and is skipped.
    pub(crate) fn plane_depth(&self, point: Vec3) -> f32 {
        self.planes
            .iter()
            .filter_map(|plane| {
                let length = plane.normal.length();
                (length > 0.0 && length.is_finite()).then(|| -plane.side(point) / length)
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// The leaf's bounds widened by [`BOUNDS_RELATIVE_PAD`] of their largest
    /// coordinate (at least 1 m's worth): the bounds the query tree indexes.
    pub(crate) fn padded_bounds(&self) -> (Vec3, Vec3) {
        let pad = BOUNDS_RELATIVE_PAD
            * self
                .aabb_min
                .abs()
                .max(self.aabb_max.abs())
                .max_element()
                .max(1.0);
        (
            self.aabb_min - Vec3::splat(pad),
            self.aabb_max + Vec3::splat(pad),
        )
    }

    /// Whether the segment `origin + t * dir`, `t` in `[t_min, t_max]`, meets
    /// this leaf's padded bounds: the tree's own leaf test, so a scan of one
    /// volume's brushes clips exactly the brushes a tree walk would hand it.
    /// A brush failing it has no interval along the segment.
    pub(crate) fn line_may_meet(&self, origin: Vec3, dir: Vec3, t_min: f32, t_max: f32) -> bool {
        let (min, max) = self.padded_bounds();
        line_overlaps_box(origin, dir, t_min, t_max, min, max)
    }

    /// Whether `point` lies within `reach` of this leaf's bounds on every axis.
    pub(crate) fn bounds_within(&self, point: Vec3, reach: f32) -> bool {
        point.cmpge(self.aabb_min - Vec3::splat(reach)).all()
            && point.cmple(self.aabb_max + Vec3::splat(reach)).all()
    }

    /// Whether `point` is within `epsilon` of this leaf: inside its bounds
    /// widened by `epsilon`, and no plane farther than `epsilon` outside. The
    /// seam test for a gap between two intervals of one union along a line.
    pub(crate) fn is_near(&self, point: Vec3, epsilon: f32) -> bool {
        self.bounds_within(point, epsilon) && self.plane_depth(point) >= -epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::axis_box;

    fn unit_box_leaf() -> ConvexLeaf {
        axis_box(Vec3::ZERO, Vec3::ONE)
    }

    #[test]
    fn convex_leaf_reports_inside_on_face_and_outside() {
        let leaf = unit_box_leaf();
        assert!(leaf.contains_point(Vec3::splat(0.5)), "strictly inside");
        assert!(
            leaf.contains_point(Vec3::new(1.0, 0.5, 0.5)),
            "exactly on a face counts as inside"
        );
        assert!(
            !leaf.contains_point(Vec3::new(1.0 + 1.0e-4, 0.5, 0.5)),
            "just outside a face is outside"
        );
    }

    #[test]
    fn convex_leaf_volume_matches_box_and_tilted_cut() {
        const EPS: f32 = 1.0e-4;
        let leaf = axis_box(Vec3::new(-1.0, 0.0, 2.0), Vec3::new(3.0, 2.0, 5.0));
        assert!((leaf.volume() - 24.0).abs() < EPS, "4 x 2 x 3 box");

        // A diagonal plane x + y <= 1 halves the unit cube's XY cross-section.
        let mut wedge = unit_box_leaf();
        wedge.planes.push(LeafPlane {
            normal: Vec3::new(1.0, 1.0, 0.0).normalize(),
            distance: 1.0 / 2.0_f32.sqrt(),
        });
        assert!((wedge.volume() - 0.5).abs() < EPS, "half cube wedge");

        // Corner tetrahedron x + y + z <= 1 inside the unit cube: volume 1/6.
        let mut tetra = unit_box_leaf();
        tetra.planes.push(LeafPlane {
            normal: Vec3::ONE.normalize(),
            distance: 1.0 / 3.0_f32.sqrt(),
        });
        assert!(
            (tetra.volume() - 1.0 / 6.0).abs() < EPS,
            "corner tetrahedron"
        );
    }

    #[test]
    fn convex_leaf_with_disjoint_planes_has_zero_volume() {
        let mut empty = unit_box_leaf();
        empty.planes.push(LeafPlane {
            normal: Vec3::X,
            distance: -1.0,
        });
        assert!(empty.volume().abs() < 1.0e-6);
    }

    #[test]
    fn contents_mask_filters_by_shared_bits() {
        let carried = ContentsMask::FLUID | ContentsMask::GRAVITY;
        assert!(carried.contains(ContentsMask::GRAVITY));
        assert!(!carried.contains(ContentsMask::PUSH));
        assert!(carried.intersects(ContentsMask::GRAVITY | ContentsMask::PUSH));
        assert!(!carried.intersects(ContentsMask::PUSH));
        assert!(ContentsMask::EMPTY.is_empty());
    }
}
