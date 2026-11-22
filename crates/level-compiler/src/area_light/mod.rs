// Polygon area-light maths: emitter shape, unshadowed term, reach, solid-angle sampling.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

// Consumers arrive in later area-lights tasks.
#![allow(dead_code, unused_imports)]

pub(crate) mod emitter;
mod exposure;
mod lattice;
mod polygon;
mod reach;
mod sampler;
mod visibility;

#[cfg(test)]
pub(crate) mod test_lights;
#[cfg(test)]
mod tests;

use glam::{DMat3, DVec3};

pub(crate) use emitter::{AreaEmitter, EmitterPolygon};
pub(crate) use exposure::{EMITTER_PIECE_AREA_FLOOR_M2, resolve_area_lights};
pub(crate) use lattice::{area_lattice_point, area_probe_indices};

pub(crate) use polygon::{
    PLANE_EPSILON_METERS, polygon_distance, polygon_irradiance, polygon_irradiance_vector,
    polygon_second_moment, polygon_solid_angle,
};
pub(crate) use reach::{REACH_CUTOFF_FRACTION, derived_reach, reach_window};
pub(crate) use sampler::{AreaSample, AreaSampler, target_lift_meters};
pub(crate) use visibility::{one_sample_visibility, weighted_visibility};

impl AreaEmitter {
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

    /// Second moment ∫ ω ωᵀ dω over the polygons whose front half-space holds
    /// `p`, per unit radiance (probes: the L2 band).
    pub(crate) fn second_moment(&self, p: DVec3) -> DMat3 {
        self.polygons
            .iter()
            .map(|poly| polygon_second_moment(poly, p))
            .fold(DMat3::ZERO, |sum, moment| sum + moment)
    }

    /// Distance from `p` to the nearest point of any polygon, either side.
    pub(crate) fn nearest_distance(&self, p: DVec3) -> f64 {
        self.polygons
            .iter()
            .map(|poly| polygon_distance(poly, p))
            .fold(f64::INFINITY, f64::min)
    }
}
