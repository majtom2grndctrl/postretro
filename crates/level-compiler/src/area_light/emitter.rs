// Area-light emitter shape: convex polygons, centroid, bounds. Compiled by the
// library too, beside `map_data`, so it holds no ray-traversal dependency.
// See: context/lib/build_pipeline.md §Compiler pipeline (Area lights)

use glam::DVec3;
use serde::{Deserialize, Serialize};

/// One planar convex emitting polygon. Vertices in world metres (engine space,
/// Y-up), counter-clockwise seen from the emitting (front) side; `normal` is the
/// unit front normal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitterPolygon {
    pub vertices: Vec<DVec3>,
    pub normal: DVec3,
    pub area: f64,
}

/// The emitter of one area light: its polygons plus derived bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreaEmitter {
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
        let winding = fan_vector_area(&vertices);
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

/// Twice the vector area of a planar polygon (a fan of cross products), taken
/// relative to its first vertex so far-from-origin corners keep their precision.
fn fan_vector_area(vertices: &[DVec3]) -> DVec3 {
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
