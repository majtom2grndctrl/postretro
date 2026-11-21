// Area-light reach, its smooth window, and the emitter and reach cull spheres.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::DVec3;

use super::AreaEmitter;

/// Reach is where the bound L·A/d² falls to this fraction of what a `light 300`
/// point light (intensity 1, inverse squared) gives at 1 m.
pub(crate) const REACH_CUTOFF_FRACTION: f64 = 1.0 / 256.0;

/// Derived reach in metres for radiance `intensity` (per m²) over `total_area`:
/// R = √(L·A / cutoff) = √(256·L·A). A 1 m², intensity-1 panel reaches 16 m; a
/// 16 m² one 64 m. Zero for a non-positive or non-finite product.
pub(crate) fn derived_reach(intensity: f64, total_area: f64) -> f64 {
    let flux_term = intensity * total_area;
    let positive = flux_term > 0.0 && flux_term.is_finite();
    if !positive {
        return 0.0;
    }
    (flux_term / REACH_CUTOFF_FRACTION).sqrt()
}

/// Karis 2013 window max(0, 1 − (d/r)⁴)², with `d` the distance to the nearest
/// polygon. 1 at the emitter, 0.879 at half reach, exactly 0 for d ≥ r.
pub(crate) fn reach_window(distance: f64, reach: f64) -> f64 {
    let in_reach = reach > 0.0 && distance < reach;
    if !in_reach {
        return 0.0;
    }
    let ratio = distance.max(0.0) / reach;
    let falloff = 1.0 - ratio * ratio * ratio * ratio;
    falloff * falloff
}

impl AreaEmitter {
    /// (centroid, circumradius): holds every polygon. Shadow-ray and occluder
    /// tests read it; each caller adds its own 1 mm slack, which also holds the
    /// tracer-tolerance lift of every sample target.
    pub(crate) fn emitter_sphere(&self) -> (DVec3, f64) {
        (self.centroid, self.circumradius)
    }

    /// (centroid, circumradius + reach): holds every point within `reach` of a
    /// polygon. Reach, influence and cache-key slice culls read it.
    pub(crate) fn reach_sphere(&self, reach: f64) -> (DVec3, f64) {
        (self.centroid, self.circumradius + reach.max(0.0))
    }
}
