// Shaft-cull margins: the hull's bounds, each candidate triangle's gap and
// margin along an axis, and the candidate query's growth.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

//! Margin derivation, compact enough to re-check each coefficient. Notation:
//! u = 2⁻²⁴; a, e1 = fl₃₂(b − a), e2 = fl₃₂(c − a) in index order, as
//! Möller–Trumbore forms them; N′ = e1 × e2, exact; ρ = ‖e1‖‖e2‖/‖N′‖;
//! e_T = max(‖e1‖, ‖e2‖); B the hull's box, L = diag(B) ≥ every t_max;
//! S_T = ‖a − centre(B)‖ + ½L ≥ ‖x − a‖ for every hull point x; θ the ray's
//! angle to plane(T′), T′ = (a, a + e1, a + e2).
//!
//! - **Accepted-hit residual.** The hit test's det and u, v, t numerators are
//!   Cramer's rule for o + t·d = a + u·e1 + v·e2, each an `f32` cross then
//!   dot, off by at most γ₅·Σ|xᵢ|(|y_j z_k| + |y_k z_j|) ≤ γ₅√2‖x‖‖y‖‖z‖
//!   (Cauchy–Schwarz; any summation order, FMA or not): [`HIT_TEST_ULPS`].
//!   On an accepted hit (u, v ≥ 0, u + v ≤ 1, 0 < t < t_max), the residual
//!   r from the triangle point to the ray point has |N′·r| ≤
//!   5√2u‖e1‖‖e2‖(S_T + L), and |(d × e2)·r|, |(d × e1)·r| bounded alike with
//!   ‖s‖ ≤ S_T. Forming s = o − a moves o by ≤ u·S_T. Off the normal, the
//!   triangle point also strays ≤ 4u·e_T past T′ (u + v ≤ 1 + 4u), and T′
//!   lies within u·e_T of the stored corners.
//! - **m_N: no ray-angle term, but ρ.** |N′·r|/‖N′‖ bounds the hit point's
//!   height off plane(T′) by 5√2uρ(S_T + L) + u·S_T. It bounds height, not t,
//!   so t's 1/sin θ error cancels the 1/sin θ run along the ray. ρ stays: the
//!   products scale as ‖e1‖‖e2‖, not as ‖N′‖.
//! - **m_A: the 2° floor.** On any other axis, write r = α·d + β·e1 + γ·e2;
//!   each coefficient is a residual over |det| = ‖N′‖ sin θ, so the hit point
//!   lies within 5√2uρ(3S_T + L + ‖e1‖ + ‖e2‖)/sin θ + u(S_T + 5e_T) of the
//!   stored triangle. That grows without bound as θ → 0 (all three residual
//!   normals turn perpendicular to d, pinning the line, not the point), so
//!   it is taken at θ0 ([`SIN_ANGLE_FLOOR`]).
//! - **Sampler endpoints.** The hull is exact over the sampler's `f32`
//!   inputs. The `f32` segment ends stray per world axis by ε_i = 10u·R_i +
//!   6u·L ([`POSITION_ULPS`], [`LENGTH_ULPS`]): 7u·R_i for the texel centre,
//!   2u·R_i for offset and lift, u·R_i for the target, u·R_i + 6u·L for the
//!   ray's delta, length, direction, origin, `max_distance` and t. R_i is the
//!   larger of the chart frame's terms ([`Hull::terms`]) and B's coordinates.
//!   Along a unit axis this adds e_H(a) = Σ|aᵢ|ε_i ≤ ‖ε‖. A sampler change
//!   that adds an `f32` operation to a segment end re-counts ε_i.
//! - **κ.** [`KAPPA`] multiplies each sum, for second-order terms (relative
//!   < 1e-6), ‖d‖ ≠ 1 and the `f64` evaluation of gaps and margins. The
//!   proven factor is 1 + 1e-5; the rest guards an operation miscount.
//!
//! So m_N = κ[5√2uρ(S_T + L) + u·S_T + e_H(n̂′)] and m_A(a) = κ[5√2uρ(3S_T +
//! L + ‖e1‖ + ‖e2‖)/sin θ0 + u(S_T + 5e_T) + e_H(a)], and a gap at or past
//! its margin admits no accepted hit.
//!
//! **Query growth.** A triangle at box distance D has S_T ≤ D + 2e_T + diag.
//! With L = diag, 3S_T + L + ‖e1‖ + ‖e2‖ ≤ 3D + 4·diag + 8e_T and S_T + 5e_T
//! ≤ D + diag + 7e_T. With P = 5√2u/sin θ0, m_A ≤ D once D ≥ κ[P(4ρ·diag +
//! 8ρe_T) + u(diag + 7e_T) + ‖ε‖] / (1 − κ(3Pρ + u)), the `shrink`
//! denominator. That is monotone in ρ, ρe_T and e_T, so the map's maxima
//! bound every triangle under the cap.
//!
//! **Accepted gaps.** Rays under θ0 fall outside m_A, and a sliver past
//! [`SHAPE_CAP`] (or with N′ = 0) outside the query is never tested; inside
//! it, such a sliver blocks. Verify mode in tests, which re-traces every
//! clear tile, is the oracle.

use std::f64::consts::SQRT_2;

use glam::{DVec3, Vec3};

use super::{primitive_triangles, triangle_points};
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

/// Ulps of the largest coordinate term R_i (texel centre, offset, lift,
/// target) that the sampler's `f32` endpoint arithmetic can move a segment
/// end, per world axis, from its exact place.
const POSITION_ULPS: f64 = 10.0;

/// Ulps of the longest segment L that the ray's own `f32` arithmetic (delta,
/// length, direction, origin, `max_distance`, t) adds to the same bound.
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
        for prim in primitives {
            for tri in primitive_triangles(prim) {
                if let Some(candidate) = Candidate::of(triangle_points(geometry, tri))
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

    /// The normal of the plane through `point` along `run` that touches the
    /// emitter sphere (passes through its centre, for a hard light) with the
    /// sphere below it, on the side `outward` points to. For a slab edge it
    /// is a face of the hull. `None` when the line meets the sphere.
    pub(super) fn tangent_plane(&self, point: DVec3, run: DVec3, outward: DVec3) -> Option<DVec3> {
        let run = run.normalize_or_zero();
        let to_centre = self.centre - point;
        let across = to_centre - run * run.dot(to_centre);
        let distance = across.length();
        if !(distance > self.radius && distance.is_finite()) {
            return None;
        }
        let across = across / distance;
        let mut side = run.cross(across);
        if side.dot(outward) < 0.0 {
            side = -side;
        }
        let ratio = self.radius / distance;
        Some(side * (1.0 - ratio * ratio).sqrt() - across * ratio)
    }

    /// A plane along `run`, tangent to the emitter on the side `outward`
    /// points to, pushed out to the slab: the tangent plane through `from`
    /// picks the slab corner farthest along its normal, and the tangent
    /// plane through that corner is returned. It approximates the hull face
    /// a candidate edge along `run` would meet; any axis certifies at its
    /// own margin, so it need not be exact.
    fn edge_plane(&self, from: DVec3, run: DVec3, outward: DVec3) -> Option<DVec3> {
        let first = self.tangent_plane(from, run, outward)?;
        let corner = self
            .corners
            .into_iter()
            .max_by(|a, b| first.dot(*a).total_cmp(&first.dot(*b)))?;
        self.tangent_plane(corner, run, outward)
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
    /// The mean of the slab's eight corners: the spine's start, the edge
    /// planes' first point and the shaft direction's base. Computed once
    /// per tile.
    pub(super) slab_centre: DVec3,
}

impl HullBounds {
    pub(super) fn new(hull: Hull) -> Self {
        let slab_centre = hull.corners.iter().copied().sum::<DVec3>() / 8.0;
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
            slab_centre,
        }
    }

    /// Whether `tri` meets the segment from the slab's centre to the
    /// emitter's centre, which lies inside the hull. A shadowing triangle
    /// usually does, and then the axes need not run. A wrong `true` costs
    /// only speed.
    fn meets_spine(&self, tri: &Candidate) -> bool {
        let from = self.slab_centre;
        let run = self.hull.centre - from;
        let [a, b, c] = tri.corners;
        let (e1, e2) = (b - a, c - a);
        let p = run.cross(e2);
        let det = e1.dot(p);
        if det == 0.0 || !det.is_finite() {
            return false;
        }
        let s = from - a;
        let u = s.dot(p) / det;
        let q = s.cross(e1);
        let v = run.dot(q) / det;
        let t = e2.dot(q) / det;
        u >= 0.0 && v >= 0.0 && u + v <= 1.0 && (0.0..=1.0).contains(&t)
    }

    /// e_H(a) for a unit axis.
    fn spread(&self, axis: DVec3) -> f64 {
        axis.abs().dot(self.sampler_error)
    }

    /// S_T: a bound on the distance from `tri`'s first corner to every hull
    /// point. Both margins read it; [`Self::separates`] computes it once per
    /// candidate and passes it as `reach`.
    pub(super) fn reach(&self, tri: &Candidate) -> f64 {
        tri.corners[0].distance((self.lo + self.hi) * 0.5) + 0.5 * self.diag
    }

    /// (g_N, m_N): the gap between the hull and the plane of the triangle's
    /// `f32` edges along its own normal, and the margin that gap must reach,
    /// with `reach` its [`Self::reach`]. The hit test's height error along
    /// N′ carries no ray-angle term: the 1/sin θ in its distance error
    /// cancels the 1/sin θ along the ray.
    pub(super) fn normal_certificate(&self, tri: &Candidate, reach: f64) -> (f64, f64) {
        let axis = tri.normal / tri.normal.length();
        let offset = axis.dot(tri.corners[0]);
        let (lo, hi) = self.hull.range(axis);
        let gap = (offset - hi).max(lo - offset);
        let margin = KAPPA
            * (HIT_TEST_ULPS * UNIT_ROUNDOFF * tri.shape * (reach + self.diag)
                + UNIT_ROUNDOFF * reach
                + self.spread(axis));
        (gap, margin)
    }

    /// (g_a, m_A) along `axis`, normalized here; `None` for a zero or
    /// non-finite axis, which certifies nothing. The margin holds for rays at
    /// or above the angle floor against the triangle's plane.
    #[cfg(test)]
    pub(super) fn axis_certificate(&self, tri: &Candidate, axis: DVec3) -> Option<(f64, f64)> {
        let axis = self.hull_axis(axis)?;
        axis.certificate(tri, self.axis_margin_base(tri, self.reach(tri)))
    }

    /// `axis`, normalized, with the hull's range and e_H along it: what every
    /// candidate's certificate on that axis reads from the hull. `None` for a
    /// zero or non-finite axis.
    pub(super) fn hull_axis(&self, axis: DVec3) -> Option<HullAxis> {
        let length = axis.length();
        if !(length > 0.0 && length.is_finite()) {
            return None;
        }
        let axis = axis / length;
        let (lo, hi) = self.hull.range(axis);
        Some(HullAxis {
            axis,
            lo,
            hi,
            spread: self.spread(axis),
        })
    }

    /// m_A(T, a) less its e_H(a) term, which alone depends on the axis, with
    /// `reach` its [`Self::reach`].
    pub(super) fn axis_margin_base(&self, tri: &Candidate, reach: f64) -> f64 {
        HIT_TEST_ULPS * UNIT_ROUNDOFF * tri.shape * (3.0 * reach + self.diag + tri.edge_sum)
            / SIN_ANGLE_FLOOR
            + UNIT_ROUNDOFF * (reach + 5.0 * tri.longest_edge)
    }

    /// Whether some tested axis separates `tri` from the hull by at least
    /// its margin: the triangle's own normal at m_N, then at m_A the tile's
    /// `hull_axes` (the receiver normal and the hull's faces, built on first
    /// use), the plane along each triangle edge tangent to the emitter
    /// ([`Hull::edge_plane`]), and each triangle edge crossed with the chart
    /// axes, the normal and the shaft direction (`frame_axes`). Cheap and
    /// likely axes go first; the order changes no verdict.
    ///
    /// Two exits come early, both `false`: ρ past [`SHAPE_CAP`], which the
    /// query growth does not cover, and, once the normal fails, a triangle
    /// that meets the spine ([`Self::meets_spine`]), which crosses the hull
    /// and so has no separating axis.
    pub(super) fn separates<'h>(
        &self,
        tri: &Candidate,
        hull_axes: impl FnOnce() -> &'h [Option<HullAxis>],
        frame_axes: &[DVec3; 4],
    ) -> bool {
        if tri.shape > SHAPE_CAP {
            return false;
        }
        let reach = self.reach(tri);
        let (gap, margin) = self.normal_certificate(tri, reach);
        if gap >= margin {
            return true;
        }
        if self.meets_spine(tri) {
            // It crosses the hull, so no axis separates it.
            return false;
        }
        let base = self.axis_margin_base(tri, reach);
        let certified = |axis: &HullAxis| {
            axis.certificate(tri, base)
                .is_some_and(|(gap, margin)| gap >= margin)
        };
        if hull_axes().iter().flatten().any(certified) {
            return true;
        }
        let [a, b, c] = tri.corners;
        let slab_centre = self.slab_centre;
        let toward = (a + b + c) / 3.0 - slab_centre;
        let edge_planed = [b - a, c - a, c - b].into_iter().any(|edge| {
            self.hull
                .edge_plane(slab_centre, edge, toward)
                .and_then(|axis| self.hull_axis(axis))
                .is_some_and(|axis| certified(&axis))
        });
        if edge_planed {
            return true;
        }
        [b - a, c - a, c - b].into_iter().any(|edge| {
            frame_axes.iter().any(|&axis| {
                self.hull_axis(edge.cross(axis))
                    .is_some_and(|axis| certified(&axis))
            })
        })
    }
}

/// One unit separating axis with the hull's range and e_H along it. The
/// hull's own faces are built once per tile and read by every candidate.
#[derive(Debug, Clone, Copy)]
pub(super) struct HullAxis {
    axis: DVec3,
    lo: f64,
    hi: f64,
    spread: f64,
}

impl HullAxis {
    /// (g_a, m_A) for `tri`, with `base` its [`HullBounds::axis_margin_base`].
    pub(super) fn certificate(&self, tri: &Candidate, base: f64) -> Option<(f64, f64)> {
        let along = tri.corners.map(|corner| self.axis.dot(corner));
        let (t_lo, t_hi) = (
            along[0].min(along[1]).min(along[2]),
            along[0].max(along[1]).max(along[2]),
        );
        let gap = (t_lo - self.hi).max(self.lo - t_hi);
        let margin = KAPPA * (base + self.spread);
        (gap.is_finite() && margin.is_finite()).then_some((gap, margin))
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
