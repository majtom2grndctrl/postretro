// Shaft-cull margins: the hull's bounds, each candidate triangle's gap and
// margin along an axis, and the candidate query's growth.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

//! Margin derivation, compact enough to re-check each coefficient. Notation:
//! u = 2⁻²⁴ and w = 2⁻⁵³ (`f32` and `f64` unit roundoff); a, b, c the stored
//! `f32` corners in index order, e1 = b − a, e2 = c − a in `f64`, exact;
//! N = e1 × e2; ρ = ‖e1‖‖e2‖/‖N‖; e_T = max(‖e1‖, ‖e2‖); q = e_T/‖N‖;
//! B the hull's box, L = diag(B) ≥ every t_max; S_T = ‖a − centre(B)‖ + ½L ≥
//! ‖x − a‖ for every hull point x, and R_T = S_T + e_T ≥ ‖v − x‖ for every
//! corner v and hull point x; M the largest coordinate magnitude of the
//! hull's terms and box; θ the ray's angle to plane(T); τ the tracer
//! tolerance ([`TRACER_TOLERANCE_METERS`]). A star marks an exact value; d
//! is the ray's direction, k_z its largest axis. A ray origin lies up to
//! ‖ε‖ outside the hull (**Sampler endpoints**); the w·‖ε‖ that adds to
//! each R_T or M term is second order, inside κ.
//!
//! The hit test (`ray_traversal::TraceRay`, Woop et al. 2013 in `f64`)
//! projects each corner along the ray, a shear onto the plane across k_z. An
//! accepted hit has the ray within 2τ of the projected triangle, whose
//! diameter the shear stretches at most twice, so every projected coordinate
//! is at most P = 4e_T + 2τ (P_s = 4e_T for a strict hit, the ray inside).
//!
//! - **Rounding.** Each projected coordinate is off by at most δp = 6w·R_T
//!   (v − o, the shear ratio, its product, the difference). An edge function
//!   E, a 2×2 cross of them with |E| ≤ 2P², is off by δE ≤ 4P·δp + 4wP² =
//!   w·P·A, A = 24R_T + 4P; det = ΣE, two sums, by δdet ≤ 3δE + 10wP² =
//!   w·P(3A + 10P). Each depth z = s_z·(v − o)_kz rounds three times, with
//!   |z| ≤ R_T/|d_kz|, so the scaled distance T = Σ E·z is off by δT ≤
//!   (R_T/|d_kz|)·w·P(3A + 34P): 3δE from the edge functions, 18wP² from the
//!   depths, 16wP² from three products and two sums. |det*| =
//!   ‖N‖ sin θ/|d_kz| ≥ ‖N‖ sin θ, the projected area.
//! - **The accepted point.** Y = o + t·d at t = T/det, which acceptance puts
//!   below t_max ≤ L ≤ 2R_T. Its height off plane(T) is H = |t − t*| sin θ =
//!   |(t − t*)·det*|·|d_kz|/‖N‖, with no ray-angle term, and
//!   (t − t*)·det* = δT − t·δdet exactly. So H ≤ w·R_T·P(9A + 54P)/‖N‖:
//!   3A + 34P from δT, whose 1/|d_kz| cancels, and 2(3A + 10P) from t·δdet.
//!   No τ enters: a tolerance hit still crosses the plane at t*. The two
//!   parts are bounded apart; their partial cancellation (the δE parts sum
//!   to Σ δE·(z − t)) is left unclaimed, as the cull only gains speed from
//!   it. A strict hit's crossing lies within 6e_T·δE/|det*| of T in its
//!   plane (negative barycentrics sum at most 3δE/|det*|, each corner within
//!   e_T of a), and Y within that plus H/sin θ. A tolerance hit is accepted
//!   only when the computed Y lies within τ of T's computed nearest point
//!   (`closest_point_on_triangle`), so Y lies within τ + δ_r of T, δ_r =
//!   6w(M + R_T) + 20w(e_T + τ)²/τ.
//! - **δ_r.** Its first part rounds coordinates: Y's by wL + √3wM (d·t,
//!   then o + d·t), and the nearest point's. The interior arm is the worst:
//!   two sums at coordinates up to M + R_T and products with edges summing
//!   to e_T. With L + e_T ≤ 2R_T that is under 3√3wM + (2 + 2√3)wR_T; an
//!   edge arm, one sum and an edge up to 2e_T, stays under 2√3wM + 4wR_T.
//!   The second part holds the squared distance's rounding, under 3wτ, and
//!   the interior arm's weights, which rounding can carry outside [0, 1]: a
//!   few w past a sum of 1, and near a corner O(wρq·τ(e_T + τ)). An edge
//!   arm's weight stays in [0, 1], so its rounding moves the nearest point
//!   along T and only lengthens the computed distance. On a small sliver
//!   the corner term outgrows the second part; κ covers it there, as m_A's
//!   strict term and H_s each exceed it.
//! - **m_N.** Height alone: m_N = κ[H_s + min(H, τ + δ_r) + e_H(n̂)], H_s at
//!   P_s for strict hits, H at P (capped by the tolerance check) for
//!   tolerance hits.
//! - **m_A: τ and the 2° floor.** On any other axis the in-plane reach adds:
//!   m_A(a) = κ[τ + δ_r + w·P_s(6e_T·A_s + R_T(9A_s + 54P_s))/(‖N‖ sin θ0) +
//!   e_H(a)], A_s = 24R_T + 4P_s, taken at θ0 ([`SIN_ANGLE_FLOOR`]): the
//!   strict term grows without bound as θ → 0. With P_s = 4e_T it is
//!   exactly (w·q/sin θ0)(864R_T² + 2016e_T·R_T + 384e_T²).
//! - **Sampler endpoints.** The hull is exact over the sampler's `f32`
//!   inputs. The `f32` segment ends stray per world axis by ε_i = 10u·R_i +
//!   6u·L ([`POSITION_ULPS`], [`LENGTH_ULPS`]): 7u·R_i for the texel centre,
//!   2u·R_i for offset and lift, u·R_i for the target, u·R_i + 6u·L for the
//!   ray's delta, length, direction, origin, `max_distance` and t. R_i is the
//!   larger of the chart frame's terms ([`Hull::terms`]) and B's coordinates.
//!   Along a unit axis this adds e_H(a) = Σ|aᵢ|ε_i ≤ ‖ε‖. A sampler change
//!   that adds an `f32` operation to a segment end re-counts ε_i.
//! - **κ.** [`KAPPA`] multiplies each sum, for second-order terms, ‖d‖ ≠ 1,
//!   Y past t_max by the rounding of t_max·|det| (w·L), and the `f64`
//!   evaluation of gaps and margins. Together they need a negligible share
//!   of it, so nearly a factor of 2 is left for an operation miscount.
//!
//! A gap at or past its margin admits no accepted hit.
//!
//! **Query growth.** A triangle at box distance D has R_T ≤ D + K, K =
//! 3e_T + diag. So m_A ≤ κ[c₀ + c₁R + c₂R²], R = D + K, with the map's
//! maxima of q, q·e_T, q·e_T² and e_T: c₂ = 864w·q/sin θ0, c₁ = 6w +
//! 2016w·(q·e_T)/sin θ0, c₀ = τ + 6wM + 20w(e_T + τ)²/τ + 384w·(q·e_T²)/sin θ0 +
//! ‖ε‖. The code reads each coefficient off the helper m_A itself calls
//! ([`ShaftScene::axis_margin_bound`]), so the two cannot drift apart.
//! m_A ≤ D between the roots of κc₂R² + (κc₁ − 1)R + κc₀ + K; the
//! lower is the growth, and the upper must reach past every triangle the
//! map holds, or the query cannot bound the candidates and the cull stays
//! off for the light.
//!
//! **Accepted gaps.** Rays under θ0 fall outside m_A, and a sliver past
//! [`SHAPE_CAP`] (or with N = 0) outside the query is never tested; inside
//! it, such a sliver blocks. Verify mode in tests, which re-traces every
//! clear tile, is the oracle.

use glam::{DVec3, Vec3};

use super::{primitive_triangles, triangle_points};
use crate::bvh_build::BvhPrimitive;
use crate::geometry::GeometryResult;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

/// `f32` unit roundoff, 2⁻²⁴.
pub(super) const UNIT_ROUNDOFF: f64 = 1.0 / 16_777_216.0;

/// `f64` unit roundoff, 2⁻⁵³.
pub(super) const F64_ROUNDOFF: f64 = f64::EPSILON / 2.0;

/// Safety factor on every margin. It covers second-order rounding terms, a
/// ray direction a few ulps off unit length, an accepted point up to w·L
/// past the segment end, and the `f64` evaluation of gaps and margins, with
/// room for an operation miscount.
pub(super) const KAPPA: f64 = 2.0;

/// Sine of the ray-angle floor, 2°. Off a triangle's own normal, a strict
/// hit's in-plane reach grows as 1/sin θ, so margins on other axes hold only
/// for rays at least this steep against the triangle's plane; shallower rays
/// are the cull's accepted gap.
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

/// The tolerance τ, as the margins read it.
fn tolerance() -> f64 {
    f64::from(TRACER_TOLERANCE_METERS)
}

/// Map-wide maxima the candidate query reads, over every BVH triangle with
/// ρ ≤ [`SHAPE_CAP`]: q = e_T/‖N‖, q·e_T, q·e_T² and e_T, and the largest
/// coordinate magnitude of any corner. Built once per bake and shared
/// read-only; no cache key holds it, since the cull changes no byte.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ShaftScene {
    pub(super) reach: f64,
    pub(super) reach_edge: f64,
    pub(super) reach_edge2: f64,
    pub(super) edge: f64,
    pub(super) extent: f64,
}

impl ShaftScene {
    pub(crate) fn new(primitives: &[BvhPrimitive], geometry: &GeometryResult) -> Self {
        let mut scene = Self::default();
        for prim in primitives {
            for tri in primitive_triangles(prim) {
                if let Some(candidate) = Candidate::of(triangle_points(geometry, tri))
                    && candidate.shape <= SHAPE_CAP
                {
                    scene.include(&candidate);
                }
            }
        }
        scene
    }

    pub(super) fn include(&mut self, candidate: &Candidate) {
        let q = candidate.reach;
        let e = candidate.longest_edge;
        self.reach = self.reach.max(q);
        self.reach_edge = self.reach_edge.max(q * e);
        self.reach_edge2 = self.reach_edge2.max(q * e * e);
        self.edge = self.edge.max(e);
        for corner in candidate.corners {
            self.extent = self.extent.max(corner.abs().max_element());
        }
    }

    /// G: how far past the hull's box a triangle can lie and still lack the
    /// separation its axis margin asks, or `None` when no finite query
    /// bounds the candidates. A triangle at box distance D has R_T ≤ D + K,
    /// and m_A ≤ D between the roots of the module docs' quadratic; the upper
    /// root must lie past the farthest any corner of the map can be from the
    /// box. A triangle past the cap is the accepted sliver gap outside the
    /// query.
    pub(super) fn query_growth(&self, bounds: &HullBounds) -> Option<f64> {
        let k = 3.0 * self.edge + bounds.diag;
        let [m0, m1, m2] = self.axis_margin_bound(bounds);
        let (a, b, c) = (m2, 1.0 - m1, m0 + k);
        let disc = b * b - 4.0 * a * c;
        if !(b > 0.0 && disc >= 0.0 && disc.is_finite()) {
            return None;
        }
        // The lower root, in the form that keeps precision as a → 0.
        let low = 2.0 * c / (b + disc.sqrt());
        let high = if a > 0.0 {
            (b + disc.sqrt()) / (2.0 * a)
        } else {
            f64::INFINITY
        };
        let farthest = 3.0_f64.sqrt() * (self.extent + bounds.magnitude) + k;
        let growth = low - k;
        (growth.is_finite() && high >= farthest).then_some(growth.max(0.0))
    }

    /// [κc₀, κc₁, κc₂]: the module docs' bound κ(c₀ + c₁R + c₂R²) on m_A
    /// for every triangle the scene holds whose corners lie within R of
    /// each point of `bounds`' hull. Each term comes from a helper m_A
    /// itself calls ([`HullBounds::axis_margin_base`]).
    pub(super) fn axis_margin_bound(&self, bounds: &HullBounds) -> [f64; 3] {
        // The strict reach is homogeneous of degree three in (R_T, e_T), so
        // at e_T = 1 it is s(R) = w(864R² + 2016R + 384). Its values at R =
        // 0, 1 and 2 give the three coefficients, exactly: each value is a
        // small integer times w.
        let s = |r: f64| strict_reach_times_area(r, 1.0);
        let (s0, s1, s2) = (s(0.0), s(1.0), s(2.0));
        let square = (s2 - 2.0 * s1 + s0) / 2.0;
        let linear = s1 - s0 - square;
        let c2 = square * self.reach / SIN_ANGLE_FLOOR;
        let c1 = coordinate_rounding(1.0) + linear * self.reach_edge / SIN_ANGLE_FLOOR;
        let c0 = tolerance()
            + coordinate_rounding(bounds.magnitude)
            + distance_rounding(self.edge)
            + s0 * self.reach_edge2 / SIN_ANGLE_FLOOR
            + bounds.sampler_error.length();
        [KAPPA * c0, KAPPA * c1, KAPPA * c2]
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
    /// M: the largest coordinate magnitude of the hull's terms and box, the
    /// scale of a ray point's own `f64` rounding.
    magnitude: f64,
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
            magnitude: magnitude.max_element(),
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

    /// (g_N, m_N): the gap between the hull and the plane of the triangle
    /// along its own normal, and the margin that gap must reach, with
    /// `reach` its [`Self::reach`]. The accepted point's height off the plane
    /// comes from t's error alone, whose 1/sin θ cancels the 1/sin θ along
    /// the ray, so no ray-angle term enters, and τ only caps an
    /// ill-conditioned tolerance hit's height.
    pub(super) fn normal_certificate(&self, tri: &Candidate, reach: f64) -> (f64, f64) {
        let axis = tri.normal / tri.normal.length();
        let offset = axis.dot(tri.corners[0]);
        let (lo, hi) = self.hull.range(axis);
        let gap = (offset - hi).max(lo - offset);
        let r_t = reach + tri.longest_edge;
        let tau = tolerance();
        let height = |p: f64| height_times_area(r_t, p) / tri.area;
        let strict = height(4.0 * tri.longest_edge);
        let tolerant =
            height(4.0 * tri.longest_edge + 2.0 * tau).min(tau + self.point_rounding(tri, r_t));
        let margin = KAPPA * (strict + tolerant + self.spread(axis));
        (gap, margin)
    }

    /// δ_r: how far past τ from the triangle the hit test's accepted point
    /// can lie, for corners within `r_t` of every hull point.
    fn point_rounding(&self, tri: &Candidate, r_t: f64) -> f64 {
        coordinate_rounding(self.magnitude + r_t) + distance_rounding(tri.longest_edge)
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
    /// `reach` its [`Self::reach`]: τ, the accepted point's rounding, and a
    /// strict hit's in-plane reach at the angle floor.
    pub(super) fn axis_margin_base(&self, tri: &Candidate, reach: f64) -> f64 {
        let r_t = reach + tri.longest_edge;
        let strict = strict_reach_times_area(r_t, tri.longest_edge) / (tri.area * SIN_ANGLE_FLOOR);
        tolerance() + self.point_rounding(tri, r_t) + strict
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

/// δE = w·P·A, A = 24R_T + 4P: how far an edge function can round, for
/// corners within `r_t` of the ray's origin and projected coordinates up to
/// `p`.
fn edge_rounding(r_t: f64, p: f64) -> f64 {
    F64_ROUNDOFF * p * (24.0 * r_t + 4.0 * p)
}

/// ‖N‖·H, the accepted point's height off the plane times ‖N‖, for the
/// same `r_t` and `p`: R_T·[(3δE + 34wP²) from δT + 2(3δE + 10wP²) from
/// t·δdet], which is w·R_T·P(9A + 54P).
fn height_times_area(r_t: f64, p: f64) -> f64 {
    let delta_e = edge_rounding(r_t, p);
    let product = F64_ROUNDOFF * p * p;
    r_t * ((3.0 * delta_e + 34.0 * product) + 2.0 * (3.0 * delta_e + 10.0 * product))
}

/// ‖N‖·sin θ0 times a strict hit's in-plane reach at the angle floor, for
/// longest edge `e` and corners within `r_t` of every hull point:
/// 6e_T·δE plus ‖N‖·H at P_s = 4e_T, which is w·e_T(864R_T² + 2016e_T·R_T +
/// 384e_T²).
fn strict_reach_times_area(r_t: f64, e: f64) -> f64 {
    let p = 4.0 * e;
    6.0 * e * edge_rounding(r_t, p) + height_times_area(r_t, p)
}

/// δ_r's first part, 6w·x at `x` = M + R_T: how far the accepted point's
/// coordinates and its nearest point's can round.
fn coordinate_rounding(x: f64) -> f64 {
    6.0 * F64_ROUNDOFF * x
}

/// δ_r's second part, 20w(e_T + τ)²/τ for longest edge `e`: the squared
/// distance's rounding and the nearest point's weights.
fn distance_rounding(e: f64) -> f64 {
    let tau = tolerance();
    20.0 * F64_ROUNDOFF * (e + tau).powi(2) / tau
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
/// corners in index order, widened to `f64`, and its edges and normal,
/// exact there.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) corners: [DVec3; 3],
    /// N = e1 × e2.
    pub(super) normal: DVec3,
    /// ‖N‖.
    pub(super) area: f64,
    /// e_T = max(‖e1‖, ‖e2‖).
    pub(super) longest_edge: f64,
    /// ρ = ‖e1‖‖e2‖ / ‖N‖.
    pub(super) shape: f64,
    /// q = e_T / ‖N‖: the strict hit's in-plane reach per unit of rounding.
    pub(super) reach: f64,
}

impl Candidate {
    /// `None` when the triangle has no certificate: a zero or non-finite N.
    pub(super) fn of([a, b, c]: [Vec3; 3]) -> Option<Self> {
        let corners = [a, b, c].map(Vec3::as_dvec3);
        let e1 = corners[1] - corners[0];
        let e2 = corners[2] - corners[0];
        let normal = e1.cross(e2);
        let area = normal.length();
        let (l1, l2) = (e1.length(), e2.length());
        let shape = l1 * l2 / area;
        let longest_edge = l1.max(l2);
        (area > 0.0 && area.is_finite() && shape.is_finite()).then_some(Self {
            corners,
            normal,
            area,
            longest_edge,
            shape,
            reach: longest_edge / area,
        })
    }
}
