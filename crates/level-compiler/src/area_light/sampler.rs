// Solid-angle sampling over an area light's faced polygons: shadow-ray targets and weights.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use std::f64::consts::TAU;

use glam::{DVec2, DVec3};

use super::AreaEmitter;
use super::polygon::{faces_receiver, polygon_solid_angle, triangle_solid_angle};
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

/// Largest f64 below 1: the top of every remapped stratum.
const BELOW_ONE: f64 = 1.0 - f64::EPSILON / 2.0;

/// A total solid angle at or below this (steradians) has nothing to sample.
const SOLID_ANGLE_FLOOR: f64 = 1.0e-14;

/// A four-corner polygon is a rectangle, sampled exactly by Ureña et al., when
/// the cosine at its first corner is within this of 0 and its fourth corner
/// closes the parallelogram within this times its edge length.
const RECTANGLE_TOLERANCE: f64 = 1.0e-9;

/// How far each sample target is lifted off its polygon along the front
/// normal: the tracer tolerance τ, so a shadow segment never crosses the
/// emitter's plane and never ends on the face it aims at. Well inside the
/// emitter sphere's 1 mm slack.
pub(crate) fn target_lift_meters() -> f64 {
    f64::from(TRACER_TOLERANCE_METERS)
}

/// One shadow-ray sample on an area light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AreaSample {
    /// The sampled point on the polygon, lifted along its front normal by τ.
    pub target: DVec3,
    /// Unit direction from the receiver to the sampled (unlifted) point.
    pub direction: DVec3,
    /// max(0, cosθ_r) for a receiver with a normal; 1 for a probe.
    pub weight: f64,
    /// Index of the sampled polygon in the emitter's list.
    pub polygon: usize,
}

/// Uniform-in-solid-angle sampling over the polygons whose front half-space
/// holds one receiver. Polygons the receiver is on or behind are never sampled.
pub(crate) struct AreaSampler<'a> {
    emitter: &'a AreaEmitter,
    receiver: DVec3,
    normal: Option<DVec3>,
    faced: Vec<FacedPolygon>,
    total_solid_angle: f64,
}

struct FacedPolygon {
    index: usize,
    solid_angle: f64,
    shape: Shape,
}

enum Shape {
    Rectangle(SphericalRectangle),
    /// Fan triangles about vertex 0: receiver-relative unit corners and solid angle.
    Fan(Vec<([DVec3; 3], f64)>),
}

impl<'a> AreaSampler<'a> {
    /// `None` when no polygon faces `receiver` or their total solid angle is ~0.
    pub(crate) fn new(
        emitter: &'a AreaEmitter,
        receiver: DVec3,
        normal: Option<DVec3>,
    ) -> Option<Self> {
        let normal = normal.map(|n| n.normalize_or_zero());
        let mut faced = Vec::new();
        let mut total_solid_angle = 0.0;
        for (index, poly) in emitter.polygons.iter().enumerate() {
            if !faces_receiver(poly, receiver) {
                continue;
            }
            let solid_angle = polygon_solid_angle(poly, receiver);
            if solid_angle <= 0.0 {
                continue;
            }
            let rel: Vec<DVec3> = poly.vertices.iter().map(|&v| v - receiver).collect();
            let shape = match rectangle_frame(&rel) {
                Some(rect) => Shape::Rectangle(rect),
                None => Shape::Fan(fan_triangles(&rel)),
            };
            total_solid_angle += solid_angle;
            faced.push(FacedPolygon {
                index,
                solid_angle,
                shape,
            });
        }
        if total_solid_angle <= SOLID_ANGLE_FLOOR {
            return None;
        }
        Some(Self {
            emitter,
            receiver,
            normal,
            faced,
            total_solid_angle,
        })
    }

    /// Ω: the total solid angle of the faced polygons. A sample's one-sample
    /// estimate divides w·Ω by the clipped projected solid angle.
    pub(crate) fn total_solid_angle(&self) -> f64 {
        self.total_solid_angle
    }

    /// Maps a stratified `u ∈ [0,1)²` to a sample. `u.x` picks a polygon (and
    /// then a fan triangle) by solid angle and is remapped inside the chosen
    /// interval, so the stratification carries through.
    pub(crate) fn sample(&self, u: DVec2) -> AreaSample {
        let (pick, ux) = pick_by_weight(
            self.faced.iter().map(|faced| faced.solid_angle),
            self.total_solid_angle,
            u.x,
        );
        let faced = &self.faced[pick];
        let poly = &self.emitter.polygons[faced.index];
        let v = u.y.clamp(0.0, BELOW_ONE);
        let rel_point = match &faced.shape {
            Shape::Rectangle(rect) => rect.sample(ux, v),
            Shape::Fan(triangles) => {
                let total: f64 = triangles.iter().map(|(_, omega)| omega).sum();
                let (tri, ux) =
                    pick_by_weight(triangles.iter().map(|(_, omega)| *omega), total, ux);
                let ([a, b, c], omega) = triangles[tri];
                let direction = sample_spherical_triangle(a, b, c, omega, ux, v);
                let rel_v0 = poly.vertices[0] - self.receiver;
                plane_hit(direction, rel_v0, poly.normal)
                    .unwrap_or_else(|| poly.centroid() - self.receiver)
            }
        };
        let rel_point = if rel_point.is_finite() {
            rel_point
        } else {
            poly.centroid() - self.receiver
        };
        let direction = rel_point.normalize_or_zero();
        let weight = match self.normal {
            Some(n) => direction.dot(n).max(0.0),
            None => 1.0,
        };
        AreaSample {
            target: self.receiver + rel_point + poly.normal * target_lift_meters(),
            direction,
            weight,
            polygon: faced.index,
        }
    }
}

/// Picks an item by `weights` (summing to `total`) with `u ∈ [0,1)` and remaps
/// `u` into [0,1) within the chosen item's interval. Zero weights are skipped.
fn pick_by_weight(weights: impl Iterator<Item = f64>, total: f64, u: f64) -> (usize, f64) {
    let target = u.clamp(0.0, BELOW_ONE) * total;
    let mut before = 0.0;
    let mut last = (0, 0.0, 0.0);
    for (index, weight) in weights.enumerate() {
        if weight <= 0.0 {
            continue;
        }
        if target < before + weight {
            return (index, ((target - before) / weight).clamp(0.0, BELOW_ONE));
        }
        last = (index, before, weight);
        before += weight;
    }
    // Rounding pushed the target past the summed weights: the last item's top.
    let (index, before, weight) = last;
    let remapped = if weight > 0.0 {
        (target - before) / weight
    } else {
        0.0
    };
    (index, remapped.clamp(0.0, BELOW_ONE))
}

/// Where the ray from the receiver along `direction` meets the plane through
/// `rel_v0` with `normal` (receiver-relative); `None` when it runs parallel or away.
fn plane_hit(direction: DVec3, rel_v0: DVec3, normal: DVec3) -> Option<DVec3> {
    let along = direction.dot(normal);
    let to_plane = rel_v0.dot(normal);
    let t = to_plane / along;
    (along < 0.0 && t > 0.0 && t.is_finite()).then_some(direction * t)
}

/// Fan triangles about vertex 0 with their solid angles; degenerate ones carry 0.
fn fan_triangles(rel: &[DVec3]) -> Vec<([DVec3; 3], f64)> {
    let a = rel[0];
    rel[1..]
        .windows(2)
        .map(|pair| {
            let omega = triangle_solid_angle(a, pair[0], pair[1]);
            let corners = [a.normalize(), pair[0].normalize(), pair[1].normalize()];
            (corners, if omega.is_finite() { omega } else { 0.0 })
        })
        .collect()
}

/// Arvo 1995: a direction uniform in solid angle over the spherical triangle
/// with unit corners `a`, `b`, `c` and solid angle `omega`.
fn sample_spherical_triangle(a: DVec3, b: DVec3, c: DVec3, omega: f64, u1: f64, u2: f64) -> DVec3 {
    let alpha = corner_angle(a, b, c);
    let (sin_alpha, cos_alpha) = alpha.sin_cos();
    let (s, t) = (u1 * omega - alpha).sin_cos();
    let u = t - cos_alpha;
    let v = s + sin_alpha * a.dot(b);
    let q = ((v * t - u * s) * cos_alpha - v) / ((v * s + u * t) * sin_alpha);
    if !q.is_finite() {
        return (a + b + c).normalize();
    }
    let q = q.clamp(-1.0, 1.0);
    let c_hat = q * a + (1.0 - q * q).sqrt() * tangent_toward(c, a);
    let z = (1.0 - u2 * (1.0 - c_hat.dot(b))).clamp(-1.0, 1.0);
    z * b + (1.0 - z * z).sqrt() * tangent_toward(c_hat, b)
}

/// The spherical triangle's interior angle at corner `a`.
fn corner_angle(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let (tb, tc) = (tangent_toward(b, a), tangent_toward(c, a));
    tb.cross(tc).length().atan2(tb.dot(tc))
}

/// Unit tangent at `from` on the great circle toward `to`.
fn tangent_toward(to: DVec3, from: DVec3) -> DVec3 {
    (to - to.dot(from) * from).normalize_or_zero()
}

/// Ureña, Fajardo & King 2013 spherical rectangle, in receiver-relative space.
struct SphericalRectangle {
    x: DVec3,
    y: DVec3,
    z: DVec3,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    z0: f64,
    b0: f64,
    b1: f64,
    k: f64,
    solid_angle: f64,
}

/// The Ureña frame for a four-corner polygon that is a rectangle, else `None`.
fn rectangle_frame(rel: &[DVec3]) -> Option<SphericalRectangle> {
    let [v0, v1, v2, v3] = rel else {
        return None;
    };
    let (ex, ey) = (*v1 - *v0, *v3 - *v0);
    let (ex_len, ey_len) = (ex.length(), ey.length());
    if ex_len <= 0.0 || ey_len <= 0.0 {
        return None;
    }
    let (x, y) = (ex / ex_len, ey / ey_len);
    let closes = (*v2 - (*v0 + ex + ey)).length() <= RECTANGLE_TOLERANCE * ex_len.max(ey_len);
    if x.dot(y).abs() > RECTANGLE_TOLERANCE || !closes {
        return None;
    }
    SphericalRectangle::new(*v0, x, y, ex_len, ey_len)
}

impl SphericalRectangle {
    fn new(corner: DVec3, x: DVec3, y: DVec3, ex_len: f64, ey_len: f64) -> Option<Self> {
        let mut z = x.cross(y);
        let mut z0 = corner.dot(z);
        if z0 > 0.0 {
            z = -z;
            z0 = -z0;
        }
        let (x0, y0) = (corner.dot(x), corner.dot(y));
        let (x1, y1) = (x0 + ex_len, y0 + ey_len);
        let v00 = DVec3::new(x0, y0, z0);
        let v01 = DVec3::new(x0, y1, z0);
        let v10 = DVec3::new(x1, y0, z0);
        let v11 = DVec3::new(x1, y1, z0);
        let n0 = v00.cross(v10).normalize_or_zero();
        let n1 = v10.cross(v11).normalize_or_zero();
        let n2 = v11.cross(v01).normalize_or_zero();
        let n3 = v01.cross(v00).normalize_or_zero();
        let angle = |a: DVec3, b: DVec3| (-a.dot(b)).clamp(-1.0, 1.0).acos();
        let (g0, g1, g2, g3) = (angle(n0, n1), angle(n1, n2), angle(n2, n3), angle(n3, n0));
        let k = TAU - g2 - g3;
        let solid_angle = g0 + g1 - k;
        let usable = solid_angle > 0.0 && solid_angle.is_finite();
        usable.then_some(Self {
            x,
            y,
            z,
            x0,
            x1,
            y0,
            y1,
            z0,
            b0: n0.z,
            b1: n2.z,
            k,
            solid_angle,
        })
    }

    /// The receiver-relative point on the rectangle for `(u, v) ∈ [0,1)²`.
    fn sample(&self, u: f64, v: f64) -> DVec3 {
        let au = u * self.solid_angle + self.k;
        let fu = (au.cos() * self.b0 - self.b1) / au.sin();
        let cu = ((fu * fu + self.b0 * self.b0).sqrt().recip())
            .copysign(fu)
            .clamp(-1.0, 1.0);
        let xu = (-(cu * self.z0) / (1.0 - cu * cu).sqrt().max(1.0e-12)).clamp(self.x0, self.x1);
        let d = (xu * xu + self.z0 * self.z0).sqrt();
        let h0 = self.y0 / (d * d + self.y0 * self.y0).sqrt();
        let h1 = self.y1 / (d * d + self.y1 * self.y1).sqrt();
        let hv = h0 + v * (h1 - h0);
        let hv2 = hv * hv;
        let yv = if hv2 < 1.0 - 1.0e-12 {
            (hv * d / (1.0 - hv2).sqrt()).clamp(self.y0, self.y1)
        } else {
            self.y1
        };
        xu * self.x + yv * self.y + self.z0 * self.z
    }
}
