// Unshadowed polygon term: Lambert's clipped irradiance, irradiance vector, solid angle, distance,
// and the direction a probe takes toward the light.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::{DMat3, DVec3};

use super::{AreaEmitter, EmitterPolygon};

/// A receiver whose signed distance from an emitter's plane is at or below
/// this is on or behind it and takes nothing from that polygon: the emitter's
/// own faces and coplanar receivers read 0. Far below any lift a consumer
/// applies (the tracer tolerance is 0.5 mm), so it only absorbs rounding.
pub(crate) const PLANE_EPSILON_METERS: f64 = 1.0e-9;

/// Whether `p` lies strictly in front of `poly`'s plane.
pub(super) fn faces_receiver(poly: &EmitterPolygon, p: DVec3) -> bool {
    poly.vertices.len() >= 3 && p.is_finite() && poly.signed_distance(p) > PLANE_EPSILON_METERS
}

/// Lambert's polygon irradiance per unit radiance at receiver `(p, n)`, clipped
/// to the receiver's horizon, and the clipped polygon's irradiance vector Φ
/// (pointing from `p` toward the light, so the returned E is Φ·n). Receivers on
/// or behind the polygon's plane read `(0, 0)`. Never NaN or infinite.
pub(crate) fn polygon_irradiance(poly: &EmitterPolygon, p: DVec3, n: DVec3) -> (f64, DVec3) {
    let Some(n) = n.try_normalize() else {
        return (0.0, DVec3::ZERO);
    };
    if !faces_receiver(poly, p) {
        return (0.0, DVec3::ZERO);
    }
    let mut edges = EdgeSum::default();
    // Sutherland–Hodgman against n·(x − p) ≥ 0, streamed into the edge sum so
    // no clipped polygon is allocated. A quad can gain a fifth corner.
    let count = poly.vertices.len();
    let mut prev = poly.vertices[count - 1] - p;
    let mut prev_side = n.dot(prev);
    for &vertex in &poly.vertices {
        let cur = vertex - p;
        let cur_side = n.dot(cur);
        if (prev_side >= 0.0) != (cur_side >= 0.0) {
            let t = prev_side / (prev_side - cur_side);
            edges.push(prev + (cur - prev) * t);
        }
        if cur_side >= 0.0 {
            edges.push(cur);
        }
        prev = cur;
        prev_side = cur_side;
    }
    let phi = edges.irradiance_vector();
    let e = phi.dot(n);
    if !(e.is_finite() && phi.is_finite()) {
        return (0.0, DVec3::ZERO);
    }
    (e.max(0.0), phi)
}

/// Unclipped irradiance vector Φ = ½ Σ Θᵢ Γᵢ per unit radiance (oriented toward
/// the light); zero when `p` is on or behind the plane. For a receiver whose
/// horizon holds the whole polygon, Φ·n is its irradiance.
pub(crate) fn polygon_irradiance_vector(poly: &EmitterPolygon, p: DVec3) -> DVec3 {
    if !faces_receiver(poly, p) {
        return DVec3::ZERO;
    }
    let mut edges = EdgeSum::default();
    for &vertex in &poly.vertices {
        edges.push(vertex - p);
    }
    let phi = edges.irradiance_vector();
    if phi.is_finite() { phi } else { DVec3::ZERO }
}

/// Second moment ∫ ω ωᵀ dω of the directions the polygon subtends at `p`, per
/// unit radiance; zero when `p` is on or behind the plane. Arvo's axial-moment
/// recurrence (Arvo 1995) for n = 2: wᵀMw = ⅓[Ω − Σᵢ (n̂ᵢ·w) ∫ᵢ (ω·w) dθ], with
/// n̂ᵢ the unit normal of edge i's plane in the winding [`polygon_irradiance_vector`]
/// uses and the arc integral (a·w) sin Θᵢ + (t·w)(1 − cos Θᵢ), where t is the
/// arc's unit tangent at its start a. Its trace is Ω. Probe bakes read it for
/// the L2 band.
pub(crate) fn polygon_second_moment(poly: &EmitterPolygon, p: DVec3) -> DMat3 {
    if !faces_receiver(poly, p) {
        return DMat3::ZERO;
    }
    let count = poly.vertices.len();
    let mut edges = DMat3::ZERO;
    for i in 0..count {
        let (Some(a), Some(b)) = (
            (poly.vertices[i] - p).try_normalize(),
            (poly.vertices[(i + 1) % count] - p).try_normalize(),
        ) else {
            continue;
        };
        let c = a.cross(b);
        let length = c.length();
        if length < f64::MIN_POSITIVE {
            continue;
        }
        let Some(tangent) = (b - a.dot(b) * a).try_normalize() else {
            continue;
        };
        let normal = c / length;
        let (sin, cos) = length.atan2(a.dot(b)).sin_cos();
        let arc = a * sin + tangent * (1.0 - cos);
        let outer = outer_product(normal, arc);
        edges += (outer + outer.transpose()) * 0.5;
    }
    let omega = polygon_solid_angle(poly, p);
    let moment = (DMat3::from_diagonal(DVec3::splat(omega)) - edges) * (1.0 / 3.0);
    if moment.is_finite() {
        moment
    } else {
        DMat3::ZERO
    }
}

/// a bᵀ.
fn outer_product(a: DVec3, b: DVec3) -> DMat3 {
    DMat3::from_cols(a * b.x, a * b.y, a * b.z)
}

/// Solid angle the polygon subtends at `p` (Van Oosterom–Strackee over a fan);
/// zero when `p` is on or behind the plane.
pub(crate) fn polygon_solid_angle(poly: &EmitterPolygon, p: DVec3) -> f64 {
    if !faces_receiver(poly, p) {
        return 0.0;
    }
    let a = poly.vertices[0] - p;
    let omega: f64 = poly.vertices[1..]
        .windows(2)
        .map(|pair| triangle_solid_angle(a, pair[0] - p, pair[1] - p))
        .sum();
    if omega.is_finite() { omega } else { 0.0 }
}

impl AreaEmitter {
    /// The unit direction a probe at `p` takes toward the light: the lift its
    /// shadow segments leave along, the facing of its scatter card and the
    /// mover channel's direction. Normally Φ̂, the normalised irradiance
    /// vector. Where Φ cancels to exactly zero though polygons face `p` (a
    /// probe at the symmetric centre of opposing faces), it is the reversed
    /// normal of the faced polygon with the largest solid angle, the first on
    /// a tie, so the light is never dropped while Ω > 0. `None` when no
    /// polygon faces `p`.
    pub(crate) fn probe_direction(&self, p: DVec3) -> Option<DVec3> {
        if let Some(direction) = self.irradiance_vector(p).try_normalize() {
            return Some(direction);
        }
        let mut largest: Option<(f64, DVec3)> = None;
        for poly in &self.polygons {
            let omega = polygon_solid_angle(poly, p);
            if omega > 0.0 && largest.is_none_or(|(most, _)| omega > most) {
                largest = Some((omega, -poly.normal));
            }
        }
        largest.map(|(_, direction)| direction)
    }
}

/// Solid angle of the triangle with receiver-relative corners `a`, `b`, `c`
/// (unnormalised): tan(Ω/2) = |a·(b×c)| / (abc + (a·b)c + (a·c)b + (b·c)a).
pub(super) fn triangle_solid_angle(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let (la, lb, lc) = (a.length(), b.length(), c.length());
    let numerator = a.dot(b.cross(c)).abs();
    let denominator = la * lb * lc + a.dot(b) * lc + a.dot(c) * lb + b.dot(c) * la;
    2.0 * numerator.atan2(denominator)
}

/// Exact distance from `p` to the nearest point of the convex polygon, from
/// either side of its plane.
pub(crate) fn polygon_distance(poly: &EmitterPolygon, p: DVec3) -> f64 {
    let count = poly.vertices.len();
    if count == 0 {
        return f64::INFINITY;
    }
    let plane_distance = poly.signed_distance(p);
    // Work relative to p: the projection of p onto the plane is -d·n.
    let foot = -plane_distance * poly.normal;
    let mut inside = count >= 3;
    let mut nearest_edge = f64::INFINITY;
    for i in 0..count {
        let a = poly.vertices[i] - p;
        let b = poly.vertices[(i + 1) % count] - p;
        let edge = b - a;
        if edge.cross(foot - a).dot(poly.normal) < 0.0 {
            inside = false;
        }
        let length_sq = edge.length_squared();
        let t = if length_sq > 0.0 {
            (-a.dot(edge) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        nearest_edge = nearest_edge.min((a + edge * t).length());
    }
    if inside {
        plane_distance.abs()
    } else {
        nearest_edge
    }
}

/// Running Σ Θᵢ Γᵢ over the edges of a closed polygon whose receiver-relative
/// vertices arrive in order.
#[derive(Default)]
struct EdgeSum {
    first: Option<DVec3>,
    last: DVec3,
    sum: DVec3,
}

impl EdgeSum {
    fn push(&mut self, vertex: DVec3) {
        if self.first.is_some() {
            self.sum += edge_term(self.last, vertex);
        } else {
            self.first = Some(vertex);
        }
        self.last = vertex;
    }

    /// Closes the polygon and returns Φ = −½ Σ Θᵢ Γᵢ. The sign turns the sum,
    /// which points along the front normal for a counter-clockwise front
    /// winding, toward the light. Fewer than three vertices cancel to zero.
    fn irradiance_vector(mut self) -> DVec3 {
        if let Some(first) = self.first {
            self.sum += edge_term(self.last, first);
        }
        -0.5 * self.sum
    }
}

/// Θ·Γ for the edge from `a` to `b` (receiver-relative, unnormalised), formed as
/// c·(Θ/|c|) with Θ = atan2(|c|, a·b). |c| = 0 means the receiver lies on the
/// edge's line; outside the segment Θ is 0, and on it the receiver is in the
/// emitter's plane, which callers reject first — either way the term is 0.
fn edge_term(a: DVec3, b: DVec3) -> DVec3 {
    let c = a.cross(b);
    let length = c.length();
    if length < f64::MIN_POSITIVE {
        return DVec3::ZERO;
    }
    c * (length.atan2(a.dot(b)) / length)
}
