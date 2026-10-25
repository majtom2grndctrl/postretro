// Shaft-cull margins: the hull's bounds, each candidate triangle's gap and
// margin along an axis, and the candidate query's growth.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::f64::consts::SQRT_2;

use glam::{DVec3, Vec3};

use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;

/// `f32` unit roundoff, 2⁻²⁴.
pub(super) const UNIT_ROUNDOFF: f64 = 1.0 / 16_777_216.0;

/// Safety factor on every margin. It covers second-order rounding terms, a
/// ray direction a few ulps off unit length, and the `f64` evaluation of
/// gaps and margins, with room for an operation miscount.
const KAPPA: f64 = 2.0;

/// Ulps per Möller–Trumbore numerator: γ₅ for its cross-then-dot, and √2
/// from bounding that product's terms by Cauchy–Schwarz.
const HIT_TEST_ULPS: f64 = 5.0 * SQRT_2;

/// Sine of the ray-angle floor, 2°. Off a triangle's own normal, a hit's
/// in-plane reach grows as 1/sin θ, so margins on other axes hold only for
/// rays at least this steep against the triangle's plane; shallower rays are
/// the cull's accepted gap.
pub(super) const SIN_ANGLE_FLOOR: f64 = 0.034_899_496_702_500_97;

/// Largest shape term ρ = ‖e1‖‖e2‖ / ‖e1 × e2‖ (one over the sine of the
/// triangle's angle at its first corner, 0.57° here) a candidate may have. A
/// candidate past it blocks the tile; the cap also bounds the candidate
/// query's growth ([`ShaftScene`]).
pub(super) const SHAPE_CAP: f64 = 100.0;

/// Ulps, per world axis, that the sampler's `f32` endpoint arithmetic can
/// move a segment end from its exact place: ten of the largest coordinate
/// term (texel centre, offset, lift, target), six of the longest segment.
const POSITION_ULPS: f64 = 10.0;
const LENGTH_ULPS: f64 = 6.0;

/// Map-wide shape maxima the candidate query reads: ρ, ρ·e_T and e_T (e_T
/// the longer of a triangle's first two edges) over every BVH triangle with
/// ρ ≤ [`SHAPE_CAP`]. Built once per bake and shared read-only; no cache
/// key holds it, since the cull changes no byte.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ShaftScene {
    pub(super) shape: f64,
    pub(super) shape_edge: f64,
    pub(super) edge: f64,
}

impl ShaftScene {
    pub(crate) fn new(primitives: &[BvhPrimitive], geometry: &GeometryResult) -> Self {
        let mut scene = Self::default();
        let geom = &geometry.geometry;
        for prim in primitives {
            let start = prim.index_offset as usize;
            let end = start + prim.index_count as usize;
            for tri in (start..end).step_by(3).take_while(|&tri| tri + 3 <= end) {
                let points = [0, 1, 2]
                    .map(|k| Vec3::from(geom.vertices[geom.indices[tri + k] as usize].position));
                if let Some(candidate) = Candidate::of(points)
                    && candidate.shape <= SHAPE_CAP
                {
                    scene.shape = scene.shape.max(candidate.shape);
                    scene.shape_edge = scene
                        .shape_edge
                        .max(candidate.shape * candidate.longest_edge);
                    scene.edge = scene.edge.max(candidate.longest_edge);
                }
            }
        }
        scene
    }

    /// G: how far past the hull's box a triangle can lie and still lack the
    /// separation its axis margin asks. A triangle at box distance D has
    /// S_T ≤ D + 2e_T + diag, so D ≥ m_A once D clears this bound with the
    /// map's largest shape terms. A triangle past the cap is the accepted
    /// sliver gap outside the query.
    pub(super) fn query_growth(&self, bounds: &HullBounds) -> Option<f64> {
        let per_shape = HIT_TEST_ULPS * UNIT_ROUNDOFF / SIN_ANGLE_FLOOR;
        let shrink = 1.0 - KAPPA * (3.0 * per_shape * self.shape + UNIT_ROUNDOFF);
        let diag = bounds.diag;
        let growth = KAPPA
            * (per_shape * (4.0 * self.shape * diag + 8.0 * self.shape_edge)
                + UNIT_ROUNDOFF * (diag + 7.0 * self.edge)
                + bounds.sampler_error.length())
            / shrink;
        (shrink > 0.0 && growth.is_finite()).then_some(growth)
    }
}

/// The convex hull that holds every segment piece the walk tests from one
/// tile: the tile's texel-centre rectangle padded in-plane and lifted to the
/// slab (eight corners), joined with the emitter sphere (radius zero for a
/// hard light). Exact arithmetic over the sampler's `f32` inputs.
#[derive(Debug, Clone, Copy)]
pub(super) struct Hull {
    pub(super) corners: [DVec3; 8],
    pub(super) centre: DVec3,
    pub(super) radius: f64,
    /// Per world axis, the largest magnitude among the terms the `f32`
    /// texel-centre expression adds; the hull's own box adds its extent.
    pub(super) terms: DVec3,
}

impl Hull {
    /// `(min, max)` of `axis · x` over the hull.
    pub(super) fn range(&self, axis: DVec3) -> (f64, f64) {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for corner in self.corners {
            let along = axis.dot(corner);
            lo = lo.min(along);
            hi = hi.max(along);
        }
        let centre = axis.dot(self.centre);
        let reach = self.radius * axis.length();
        (lo.min(centre - reach), hi.max(centre + reach))
    }
}

/// A hull and what every candidate's margins read from it.
pub(super) struct HullBounds {
    hull: Hull,
    pub(super) lo: DVec3,
    pub(super) hi: DVec3,
    /// L: the box diagonal, at least every tested segment's length.
    diag: f64,
    /// ε_i: how far, per world axis, an `f32` segment end the sampler forms
    /// can stray from its exact place in the hull.
    sampler_error: DVec3,
}

impl HullBounds {
    pub(super) fn new(hull: Hull) -> Self {
        let axis_range = |axis: DVec3| hull.range(axis);
        let (x, y, z) = (
            axis_range(DVec3::X),
            axis_range(DVec3::Y),
            axis_range(DVec3::Z),
        );
        let lo = DVec3::new(x.0, y.0, z.0);
        let hi = DVec3::new(x.1, y.1, z.1);
        let diag = (hi - lo).length();
        let magnitude = hull.terms.max(lo.abs().max(hi.abs()));
        let sampler_error = magnitude * (POSITION_ULPS * UNIT_ROUNDOFF)
            + DVec3::splat(LENGTH_ULPS * UNIT_ROUNDOFF * diag);
        Self {
            hull,
            lo,
            hi,
            diag,
            sampler_error,
        }
    }

    /// e_H(a) for a unit axis.
    fn spread(&self, axis: DVec3) -> f64 {
        axis.abs().dot(self.sampler_error)
    }

    /// S_T: a bound on the distance from `corner` to every hull point.
    fn reach_from(&self, corner: DVec3) -> f64 {
        corner.distance((self.lo + self.hi) * 0.5) + 0.5 * self.diag
    }

    /// (g_N, m_N): the gap between the hull and the plane of the triangle's
    /// `f32` edges along its own normal, and the margin that gap must reach.
    /// The hit test's height error along N′ carries no ray-angle term: the
    /// 1/sin θ in its distance error cancels the 1/sin θ along the ray.
    pub(super) fn normal_certificate(&self, tri: &Candidate) -> (f64, f64) {
        let axis = tri.normal / tri.normal.length();
        let offset = axis.dot(tri.corners[0]);
        let (lo, hi) = self.hull.range(axis);
        let gap = (offset - hi).max(lo - offset);
        let reach = self.reach_from(tri.corners[0]);
        let margin = KAPPA
            * (HIT_TEST_ULPS * UNIT_ROUNDOFF * tri.shape * (reach + self.diag)
                + UNIT_ROUNDOFF * reach
                + self.spread(axis));
        (gap, margin)
    }

    /// (g_a, m_A) along `axis`, normalized here; `None` for a zero or
    /// non-finite axis, which certifies nothing. The margin holds for rays at
    /// or above the angle floor against the triangle's plane.
    pub(super) fn axis_certificate(&self, tri: &Candidate, axis: DVec3) -> Option<(f64, f64)> {
        let length = axis.length();
        if !(length > 0.0 && length.is_finite()) {
            return None;
        }
        let axis = axis / length;
        let along = tri.corners.map(|corner| axis.dot(corner));
        let (t_lo, t_hi) = (
            along[0].min(along[1]).min(along[2]),
            along[0].max(along[1]).max(along[2]),
        );
        let (lo, hi) = self.hull.range(axis);
        let gap = (t_lo - hi).max(lo - t_hi);
        let reach = self.reach_from(tri.corners[0]);
        let margin = KAPPA
            * (HIT_TEST_ULPS
                * UNIT_ROUNDOFF
                * tri.shape
                * (3.0 * reach + self.diag + tri.edge_sum)
                / SIN_ANGLE_FLOOR
                + UNIT_ROUNDOFF * (reach + 5.0 * tri.longest_edge)
                + self.spread(axis));
        (gap.is_finite() && margin.is_finite()).then_some((gap, margin))
    }

    /// Whether some tested axis separates `tri` from the hull by at least
    /// its margin: the triangle's own normal, then the receiver normal and
    /// each triangle edge crossed with the chart axes, the normal and the
    /// shaft direction (`frame_axes`).
    pub(super) fn separates(
        &self,
        tri: &Candidate,
        receiver_normal: DVec3,
        frame_axes: &[DVec3; 4],
    ) -> bool {
        if tri.shape > SHAPE_CAP {
            return false;
        }
        let (gap, margin) = self.normal_certificate(tri);
        if gap >= margin {
            return true;
        }
        let certified = |axis: DVec3| {
            self.axis_certificate(tri, axis)
                .is_some_and(|(gap, margin)| gap >= margin)
        };
        if certified(receiver_normal) {
            return true;
        }
        let [a, b, c] = tri.corners;
        [b - a, c - a, c - b]
            .into_iter()
            .any(|edge| frame_axes.iter().any(|&axis| certified(edge.cross(axis))))
    }
}

/// One candidate triangle as the hit test sees it: its stored `f32`
/// corners in index order, and the edges Möller–Trumbore forms from them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) corners: [DVec3; 3],
    /// N′ = e1 × e2, exact over the `f32`-rounded edges.
    pub(super) normal: DVec3,
    /// ‖e1‖ + ‖e2‖.
    pub(super) edge_sum: f64,
    /// e_T = max(‖e1‖, ‖e2‖).
    pub(super) longest_edge: f64,
    /// ρ = ‖e1‖‖e2‖ / ‖N′‖.
    pub(super) shape: f64,
}

impl Candidate {
    /// `None` when the triangle has no certificate: a zero or non-finite N′.
    pub(super) fn of([a, b, c]: [Vec3; 3]) -> Option<Self> {
        let e1 = (b - a).as_dvec3();
        let e2 = (c - a).as_dvec3();
        let normal = e1.cross(e2);
        let area = normal.length();
        let (l1, l2) = (e1.length(), e2.length());
        let shape = l1 * l2 / area;
        (area > 0.0 && area.is_finite() && shape.is_finite()).then_some(Self {
            corners: [a.as_dvec3(), b.as_dvec3(), c.as_dvec3()],
            normal,
            edge_sum: l1 + l2,
            longest_edge: l1.max(l2),
            shape,
        })
    }
}
