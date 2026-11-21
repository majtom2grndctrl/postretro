// Polygon area-light maths: emitter shape, unshadowed term, reach, solid-angle sampling.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

// Consumers arrive in later area-lights tasks.
#![allow(dead_code, unused_imports)]

mod polygon;
mod reach;
mod sampler;
mod visibility;

#[cfg(test)]
mod tests;

use glam::DVec3;
use serde::{Deserialize, Serialize};

pub(crate) use polygon::{
    PLANE_EPSILON_METERS, polygon_distance, polygon_irradiance, polygon_irradiance_vector,
    polygon_solid_angle,
};
pub(crate) use reach::{REACH_CUTOFF_FRACTION, derived_reach, reach_window};
pub(crate) use sampler::{AreaSample, AreaSampler, target_lift_meters};
pub(crate) use visibility::{one_sample_visibility, weighted_visibility};

/// One planar convex emitting polygon. Vertices in world metres (engine space,
/// Y-up), counter-clockwise seen from the emitting (front) side; `normal` is the
/// unit front normal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EmitterPolygon {
    pub vertices: Vec<DVec3>,
    pub normal: DVec3,
    pub area: f64,
}

/// The emitter of one area light: its polygons plus derived bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AreaEmitter {
    pub polygons: Vec<EmitterPolygon>,
    pub centroid: DVec3,
    pub circumradius: f64,
    pub total_area: f64,
}

impl EmitterPolygon {
    /// Builds a polygon and computes its area. `normal` names the front side; a
    /// winding that disagrees with it is reversed, so the counter-clockwise
    /// contract holds whatever order the caller passed. A zero `normal` takes
    /// the winding's own normal.
    pub(crate) fn new(mut vertices: Vec<DVec3>, normal: DVec3) -> Self {
        let winding = newell_normal(&vertices);
        let normal = normal
            .try_normalize()
            .unwrap_or_else(|| winding.normalize_or_zero());
        if winding.dot(normal) < 0.0 {
            vertices.reverse();
        }
        let area = 0.5 * winding.length();
        Self {
            vertices,
            normal,
            area,
        }
    }

    /// Area-weighted centroid (fan triangles about vertex 0).
    pub(crate) fn centroid(&self) -> DVec3 {
        let Some(&v0) = self.vertices.first() else {
            return DVec3::ZERO;
        };
        let mut weighted = DVec3::ZERO;
        let mut total = 0.0;
        for pair in self.vertices[1..].windows(2) {
            let (a, b) = (pair[0] - v0, pair[1] - v0);
            let area = 0.5 * a.cross(b).dot(self.normal);
            weighted += (a + b) / 3.0 * area;
            total += area;
        }
        if total > 0.0 {
            v0 + weighted / total
        } else {
            v0
        }
    }

    /// Signed distance of `p` from the polygon's plane, positive on the front.
    pub(crate) fn signed_distance(&self, p: DVec3) -> f64 {
        self.vertices
            .first()
            .map_or(0.0, |&v0| (p - v0).dot(self.normal))
    }
}

/// Twice the vector area of a planar polygon, taken relative to its first
/// vertex so far-from-origin corners keep their precision.
fn newell_normal(vertices: &[DVec3]) -> DVec3 {
    let Some(&v0) = vertices.first() else {
        return DVec3::ZERO;
    };
    vertices[1..]
        .windows(2)
        .map(|pair| (pair[0] - v0).cross(pair[1] - v0))
        .sum()
}

impl AreaEmitter {
    /// Derives the centroid, circumradius and total area. `None` for no
    /// polygons or a total area that is not positive and finite.
    pub(crate) fn new(polygons: Vec<EmitterPolygon>) -> Option<Self> {
        let total_area: f64 = polygons.iter().map(|poly| poly.area).sum();
        let positive = total_area > 0.0 && total_area.is_finite();
        if polygons.is_empty() || !positive {
            return None;
        }
        let centroid = polygons
            .iter()
            .map(|poly| poly.centroid() * poly.area)
            .sum::<DVec3>()
            / total_area;
        let circumradius = polygons
            .iter()
            .flat_map(|poly| poly.vertices.iter())
            .map(|v| v.distance(centroid))
            .fold(0.0, f64::max);
        Some(Self {
            polygons,
            centroid,
            circumradius,
            total_area,
        })
    }

    /// Horizon-clipped irradiance per unit radiance at a receiver `(p, n)`,
    /// summed over the polygons, with the summed clipped irradiance vector.
    pub(crate) fn irradiance(&self, p: DVec3, n: DVec3) -> (f64, DVec3) {
        self.polygons.iter().fold((0.0, DVec3::ZERO), |acc, poly| {
            let (e, phi) = polygon_irradiance(poly, p, n);
            (acc.0 + e, acc.1 + phi)
        })
    }

    /// [`Self::irradiance`] scaled by the reach window at the distance to the
    /// nearest polygon: the term every area-light consumer evaluates.
    pub(crate) fn windowed_irradiance(&self, p: DVec3, n: DVec3, reach: f64) -> (f64, DVec3) {
        let window = reach_window(self.nearest_distance(p), reach);
        if window == 0.0 {
            return (0.0, DVec3::ZERO);
        }
        let (e, phi) = self.irradiance(p, n);
        (e * window, phi * window)
    }

    /// Unclipped irradiance vector per unit radiance (probes: no receiver normal).
    pub(crate) fn irradiance_vector(&self, p: DVec3) -> DVec3 {
        self.polygons
            .iter()
            .map(|poly| polygon_irradiance_vector(poly, p))
            .sum()
    }

    /// Solid angle subtended by the polygons whose front half-space holds `p`.
    pub(crate) fn solid_angle(&self, p: DVec3) -> f64 {
        self.polygons
            .iter()
            .map(|poly| polygon_solid_angle(poly, p))
            .sum()
    }

    /// Distance from `p` to the nearest point of any polygon, either side.
    pub(crate) fn nearest_distance(&self, p: DVec3) -> f64 {
        self.polygons
            .iter()
            .map(|poly| polygon_distance(poly, p))
            .fold(f64::INFINITY, f64::min)
    }
}
