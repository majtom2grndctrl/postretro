// Watertight ray–triangle hit test with an in-plane tolerance, shared by every bake tracer.
// See: context/lib/build_pipeline.md §Ray traversal

use glam::{DVec3, Vec3};

use super::BoundedRay;

/// The tracer tolerance τ, in metres: a ray that crosses a triangle's plane
/// within τ of the triangle, measured in that plane, hits it. It closes crease
/// cracks and T-junctions narrower than about 2τ, for at most τ of extra
/// shadow past a silhouette edge. Sized to the widest crease crack the
/// shared-edge census finds, and never above face extraction's split
/// tolerance (`REGION_CLASSIFICATION_EPSILON`). The census is test-only
/// (`census.rs`); re-check τ against it with `shared_edge_census_on_gate_maps`
/// after any change to face extraction or the cut. The bake trees' box pad
/// and every crack-width bound read it.
pub(crate) const TRACER_TOLERANCE_METERS: f32 = 5.0e-4;

/// A bake ray as the watertight test sees it (Woop, Benthin and Wald 2013):
/// the `f32` origin and direction widened to `f64`, the axis permutation
/// that makes the direction's largest component z, and the shear that maps
/// the ray onto that axis. Built once per ray; every triangle test reads it.
pub(crate) struct TraceRay {
    origin: Vec3,
    direction: Vec3,
    origin_f64: DVec3,
    axes: Axes,
    /// Shear: `d[kx] / d[kz]`, `d[ky] / d[kz]`, `1 / d[kz]`.
    sx: f64,
    sy: f64,
    sz: f64,
    tolerance: f64,
}

/// The permutation `(kx, ky, kz)`: `kz` the direction's largest-magnitude
/// axis, `kx` and `ky` the other two in cyclic order, swapped when that
/// component is negative so a triangle's determinant keeps its winding's
/// sign. The hit test dispatches on it once per triangle into a copy with
/// constant indices, so a projected coordinate is `v[kx] − sx·v[kz]` with no
/// runtime indexing.
#[derive(Clone, Copy)]
enum Axes {
    /// `(y, z, x)`.
    X,
    /// `(z, y, x)`.
    XNegative,
    /// `(z, x, y)`.
    Y,
    /// `(x, z, y)`.
    YNegative,
    /// `(x, y, z)`.
    Z,
    /// `(y, x, z)`.
    ZNegative,
}

/// An accepted crossing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TriangleHit {
    /// Distance along the ray in units of its direction's length.
    pub(crate) t: f64,
    /// The ray meets the triangle's front, the side its counter-clockwise
    /// winding faces: the determinant is positive.
    pub(crate) front: bool,
    /// Weights of the triangle's three corners at the plane crossing; they
    /// sum to one. A tolerance hit's crossing lies up to τ outside the
    /// triangle, so a weight is negative, and on a sliver far outside
    /// `[0, 1]`: a crossing 0.3 mm off a 20 µm-wide sliver weights a corner
    /// by about 15. A caller that interpolates attributes reads them at the
    /// nearest point on the triangle instead ([`closest_point_on_triangle`]).
    pub(crate) barycentric: [f64; 3],
}

/// The hit test's accepted values, before any divide: the edge functions,
/// their sum the determinant, and `t` scaled by it. A caller that needs only
/// the answer never pays for the quotients.
#[derive(Clone, Copy)]
struct Crossing {
    u: f64,
    v: f64,
    w: f64,
    det: f64,
    scaled_t: f64,
}

impl TraceRay {
    /// `direction` must be non-zero; callers pass a unit vector.
    #[inline]
    pub(crate) fn new(origin: Vec3, direction: Vec3) -> Self {
        let d = direction.as_dvec3();
        let abs = d.abs();
        let (axes, [kx, ky, kz]) = if abs.x >= abs.y && abs.x >= abs.z {
            if d.x < 0.0 {
                (Axes::XNegative, [2, 1, 0])
            } else {
                (Axes::X, [1, 2, 0])
            }
        } else if abs.y >= abs.z {
            if d.y < 0.0 {
                (Axes::YNegative, [0, 2, 1])
            } else {
                (Axes::Y, [2, 0, 1])
            }
        } else if d.z < 0.0 {
            (Axes::ZNegative, [1, 0, 2])
        } else {
            (Axes::Z, [0, 1, 2])
        };
        Self {
            origin,
            direction,
            origin_f64: origin.as_dvec3(),
            axes,
            sx: d[kx] / d[kz],
            sy: d[ky] / d[kz],
            sz: 1.0 / d[kz],
            tolerance: f64::from(tolerance()),
        }
    }

    pub(crate) fn origin(&self) -> Vec3 {
        self.origin
    }

    pub(crate) fn direction(&self) -> Vec3 {
        self.direction
    }

    /// The box query the traversal walks for this ray: the same origin and
    /// direction the hit test reads, bounded at `bound`.
    #[inline]
    pub(crate) fn query(&self, bound: f32) -> BoundedRay {
        BoundedRay::new(self.origin, self.direction, bound)
    }

    /// The crossing of triangle `[a, b, c]` strictly between `t_min` and
    /// `t_max`, either side counting, or `None`.
    ///
    /// The strict test is watertight: a shared edge's edge function is the
    /// same two products in both triangles, negated, and a zero counts as
    /// inside, so a ray through a shared edge or vertex hits one of its
    /// triangles. Only `det == 0`, a ray parallel to the plane, rejects on
    /// the determinant. A crossing outside the triangle but within τ of it in
    /// its plane is accepted too; see [`TRACER_TOLERANCE_METERS`].
    ///
    /// Always inlined: a caller that reads only whether it hit, or only `t`,
    /// drops the divides for the fields it never reads.
    #[inline(always)]
    pub(crate) fn hit(&self, [a, b, c]: [Vec3; 3], t_min: f32, t_max: f32) -> Option<TriangleHit> {
        #[cfg(test)]
        crate::bake_counters::bump_trace(|trace| trace.triangle_tests += 1);
        #[cfg(test)]
        if let Some(t) = planted_t([a, b, c]) {
            return (t > f64::from(t_min) && t < f64::from(t_max)).then_some(TriangleHit {
                t,
                front: true,
                barycentric: [1.0 / 3.0; 3],
            });
        }
        let corners = [a, b, c];
        let crossing = match self.axes {
            Axes::X => self.crossing::<1, 2, 0>(corners, t_min, t_max),
            Axes::XNegative => self.crossing::<2, 1, 0>(corners, t_min, t_max),
            Axes::Y => self.crossing::<2, 0, 1>(corners, t_min, t_max),
            Axes::YNegative => self.crossing::<0, 2, 1>(corners, t_min, t_max),
            Axes::Z => self.crossing::<0, 1, 2>(corners, t_min, t_max),
            Axes::ZNegative => self.crossing::<1, 0, 2>(corners, t_min, t_max),
        }?;
        let Crossing {
            u,
            v,
            w,
            det,
            scaled_t,
        } = crossing;
        Some(TriangleHit {
            t: scaled_t / det,
            front: det > 0.0,
            barycentric: [u / det, v / det, w / det],
        })
    }

    /// [`Self::hit`] for one permutation `(KX, KY, KZ)`, the ray's own.
    #[inline(always)]
    fn crossing<const KX: usize, const KY: usize, const KZ: usize>(
        &self,
        corners: [Vec3; 3],
        t_min: f32,
        t_max: f32,
    ) -> Option<Crossing> {
        let [a, b, c] = corners.map(|p| p.as_dvec3() - self.origin_f64);
        let ax = a[KX] - self.sx * a[KZ];
        let ay = a[KY] - self.sy * a[KZ];
        let bx = b[KX] - self.sx * b[KZ];
        let by = b[KY] - self.sy * b[KZ];
        let cx = c[KX] - self.sx * c[KZ];
        let cy = c[KY] - self.sy * c[KZ];
        // Edge functions, each twice the signed area the ray's projection
        // spans with one edge: u for b→c, v for c→a, w for a→b.
        let u = cx * by - cy * bx;
        let v = ax * cy - ay * cx;
        let w = bx * ay - by * ax;
        let inside = !((u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0));
        if !inside && !self.may_reach_within_tolerance([ax, ay, bx, by, cx, cy], [u, v, w]) {
            return None;
        }
        let det = u + v + w;
        if det == 0.0 {
            #[cfg(test)]
            crate::bake_counters::bump_trace(|trace| trace.det_rejections += 1);
            return None;
        }
        let (az, bz, cz) = (self.sz * a[KZ], self.sz * b[KZ], self.sz * c[KZ]);
        let scaled_t = u * az + v * bz + w * cz;
        let (signed_t, abs_det) = if det > 0.0 {
            (scaled_t, det)
        } else {
            (-scaled_t, -det)
        };
        if !(signed_t > f64::from(t_min) * abs_det && signed_t < f64::from(t_max) * abs_det) {
            return None;
        }
        if !inside && !self.crossing_within_tolerance(corners, scaled_t / det) {
            return None;
        }
        Some(Crossing {
            u,
            v,
            w,
            det,
            scaled_t,
        })
    }

    /// Whether a crossing outside the triangle could still lie within τ of
    /// it. The shear maps an in-plane displacement of length r to one of at
    /// most 2r (each sheared axis adds at most the z run, |s| ≤ 1), so a ray
    /// more than 2τ from the projected triangle, past its box or past one
    /// edge's line, is more than τ from the triangle in its plane.
    #[inline(always)]
    fn may_reach_within_tolerance(
        &self,
        [ax, ay, bx, by, cx, cy]: [f64; 6],
        [u, v, w]: [f64; 3],
    ) -> bool {
        if self.tolerance <= 0.0 {
            return false;
        }
        // Margin over 2τ for the edge functions' own rounding.
        let reach = 2.0 * self.tolerance * (1.0 + 1e-6);
        // The projected box test, as plain comparisons: the coordinates are
        // finite, so no min or max needs NaN handling.
        if (ax > reach) & (bx > reach) & (cx > reach)
            || (ax < -reach) & (bx < -reach) & (cx < -reach)
            || (ay > reach) & (by > reach) & (cy > reach)
            || (ay < -reach) & (by < -reach) & (cy < -reach)
        {
            return false;
        }
        // Orient by the determinant's sign, u + v + w: an edge function of
        // the opposite sign is a side the ray lies outside of, by its value
        // over that edge's projected length.
        let positive = u + v + w > 0.0;
        let within = |e: f64, px: f64, py: f64, qx: f64, qy: f64| {
            let outside = if positive { e < 0.0 } else { e > 0.0 };
            !outside || e * e <= reach * reach * ((qx - px).powi(2) + (qy - py).powi(2))
        };
        within(u, bx, by, cx, cy) && within(v, cx, cy, ax, ay) && within(w, ax, ay, bx, by)
    }

    /// Whether the crossing at `t`, outside triangle `corners`, lies within
    /// τ of it in its plane. Rare, so kept out of the inlined test.
    #[cold]
    #[inline(never)]
    fn crossing_within_tolerance(&self, corners: [Vec3; 3], t: f64) -> bool {
        let crossing = self.origin_f64 + self.direction.as_dvec3() * t;
        let distance_squared = distance_squared_to_triangle(crossing, corners.map(Vec3::as_dvec3));
        // A NaN distance comes only from a degenerate triangle, which has no
        // area to hit, and its crossing can lie anywhere: rejecting it keeps
        // every accepted point inside its padded box (`ray_traversal::pad`).
        distance_squared <= self.tolerance * self.tolerance
    }
}

/// Squared distance from `point` to the closed triangle `[a, b, c]`: to its
/// [`closest_point_on_triangle`]. For a point in the triangle's plane it is
/// the in-plane distance, rounded at the corners. Not finite for a
/// degenerate triangle whose region test lands in its interior.
pub(super) fn distance_squared_to_triangle(point: DVec3, corners: [DVec3; 3]) -> f64 {
    (point - closest_point_on_triangle(point, corners).0).length_squared()
}

/// The point of the closed triangle `[a, b, c]` nearest `point`, and its
/// corner weights, each in `[0, 1]` up to rounding (Ericson, *Real-Time
/// Collision Detection* §5.1.5): a corner, the foot on an edge, or the
/// projection into the interior, by the Voronoi region `point` lies in.
#[inline]
pub(crate) fn closest_point_on_triangle(point: DVec3, [a, b, c]: [DVec3; 3]) -> (DVec3, [f64; 3]) {
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = point - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let along = d1 / (d1 - d3);
        return (a + ab * along, [1.0 - along, along, 0.0]);
    }
    let cp = point - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let along = d2 / (d2 - d6);
        return (a + ac * along, [1.0 - along, 0.0, along]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let along = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * along, [0.0, 1.0 - along, along]);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// The tolerance a ray built on this thread uses: [`TRACER_TOLERANCE_METERS`],
/// or a test's override.
#[cfg(not(test))]
#[inline(always)]
fn tolerance() -> f32 {
    TRACER_TOLERANCE_METERS
}

#[cfg(test)]
thread_local! {
    static TOLERANCE_OVERRIDE: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn tolerance() -> f32 {
    TOLERANCE_OVERRIDE
        .with(std::cell::Cell::get)
        .unwrap_or(TRACER_TOLERANCE_METERS)
}

/// Run `work` with rays built on this thread using tolerance `tau`. The bake
/// trees stay padded for the shipped τ, so `tau` may not exceed it.
#[cfg(test)]
pub(crate) fn with_tolerance<R>(tau: f32, work: impl FnOnce() -> R) -> R {
    assert!(
        (0.0..=TRACER_TOLERANCE_METERS).contains(&tau),
        "a test tolerance above the shipped τ outgrows the tree's pad"
    );
    let previous = TOLERANCE_OVERRIDE.with(|cell| cell.replace(Some(tau)));
    let result = work();
    TOLERANCE_OVERRIDE.with(|cell| cell.set(previous));
    result
}

#[cfg(test)]
thread_local! {
    static PLANTED_T: std::cell::Cell<Option<([Vec3; 3], f64)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn planted_t(corners: [Vec3; 3]) -> Option<f64> {
    PLANTED_T
        .with(std::cell::Cell::get)
        .and_then(|(planted, t)| (planted == corners).then_some(t))
}

/// Run `work` with the hit test on this thread reporting distance `t` for
/// the triangle `corners`, whatever the ray: a seam for hits whose computed
/// distance falls where no exact crossing lies.
#[cfg(test)]
pub(crate) fn with_planted_t<R>(corners: [Vec3; 3], t: f64, work: impl FnOnce() -> R) -> R {
    let previous = PLANTED_T.with(|cell| cell.replace(Some((corners, t))));
    let result = work();
    PLANTED_T.with(|cell| cell.set(previous));
    result
}
