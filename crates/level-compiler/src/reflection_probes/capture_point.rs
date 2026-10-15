// Reflection-probe capture points: where over the water a probe captures, and the tests that drop it.
// See: context/lib/resource_management.md §5.1

use std::collections::HashSet;
use std::fmt;

use glam::{DVec2, DVec3};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentVolumesSection};

use crate::partition::{BspTree, find_leaf_for_point};

use super::placement::{CAPTURE_LIFT_M, UpFacingFace};

/// A centroid moved onto the water lands this far inside the face it lands
/// on, toward that face's own centroid (at most halfway): a point exactly on
/// the edge would sit on the wall plane of the solid that cut the water
/// there, where the BSP's side is a tie and capture rays start in the wall.
pub(crate) const NEAREST_POINT_INSET_M: f64 = 0.01;
/// A centroid within this horizontal distance of a face counts as over it,
/// so one lying on the shared edge of two pieces stays put.
const OVER_WATER_EPSILON_M: f64 = 1.0e-6;

/// Horizontal area centroid of `faces`, [`CAPTURE_LIFT_M`] above `plane`.
/// A centroid not over any of their water moves to the nearest point on
/// them, inset by [`NEAREST_POINT_INSET_M`]. `faces` is non-empty.
pub(crate) fn capture_point_over(faces: &[&UpFacingFace], plane: f64) -> DVec3 {
    let mut area = 0.0;
    let mut weighted = DVec2::ZERO;
    for face in faces {
        let g = &face.geometry;
        area += g.area;
        weighted += DVec2::new(g.centroid.x, g.centroid.z) * g.area;
    }
    let centroid = weighted / area;
    let mut nearest: Option<(f64, DVec2, &UpFacingFace)> = None;
    for face in faces {
        let (point, distance) = nearest_point_on_polygon(centroid, &face.outline);
        // Strict: ties go to the earlier face, so the point is order-stable
        // for faces given in id-52 order.
        if nearest.is_none_or(|(best, ..)| distance < best) {
            nearest = Some((distance, point, face));
        }
    }
    let (distance, on_water, face) = nearest.expect("a capture point needs at least one face");
    let horizontal = if distance <= OVER_WATER_EPSILON_M {
        centroid
    } else {
        let toward = DVec2::new(face.geometry.centroid.x, face.geometry.centroid.z) - on_water;
        let length = toward.length();
        if length > 0.0 {
            on_water + toward * (NEAREST_POINT_INSET_M / length).min(0.5)
        } else {
            on_water
        }
    };
    DVec3::new(horizontal.x, plane + CAPTURE_LIFT_M, horizontal.y)
}

/// The point of polygon `outline` (horizontal `(x, z)`) nearest `p`, and the
/// distance to it; 0 inside.
pub(crate) fn nearest_point_on_polygon(p: DVec2, outline: &[DVec2]) -> (DVec2, f64) {
    if contains(outline, p) {
        return (p, 0.0);
    }
    let mut best = (p, f64::INFINITY);
    for (i, &a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        let edge = b - a;
        let t = if edge.length_squared() > 0.0 {
            ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = a + edge * t;
        let distance = q.distance(p);
        if distance < best.1 {
            best = (q, distance);
        }
    }
    best
}

/// Even-odd containment; boundary points are left to the distance test.
fn contains(outline: &[DVec2], p: DVec2) -> bool {
    let mut inside = false;
    for (i, &a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Why a capture point cannot hold a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointFault {
    InSolid,
    /// Inside a brush of this fluid entity, the probe's own included.
    InFluid(u32),
    OutsideHull,
}

impl fmt::Display for PointFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InSolid => write!(f, "in solid"),
            Self::InFluid(entity) => write!(f, "inside fluid entity {entity}"),
            Self::OutsideHull => write!(f, "outside the hull"),
        }
    }
}

/// The drop tests a capture point must pass, against the compiler BSP the
/// SH bake validates its probes with, and the fluid brushes of id 52.
pub(crate) struct PointClassifier<'a> {
    tree: &'a BspTree,
    /// Empty leaves the exterior flood fill reached: outside the hull.
    exterior: &'a HashSet<usize>,
    fluids: Vec<(u32, &'a [EnvironmentBrush])>,
}

impl<'a> PointClassifier<'a> {
    pub(crate) fn new(
        tree: &'a BspTree,
        exterior: &'a HashSet<usize>,
        volumes: &'a EnvironmentVolumesSection,
    ) -> Self {
        Self {
            tree,
            exterior,
            fluids: volumes
                .volumes
                .iter()
                .filter(|volume| !volume.fluid.is_empty())
                .map(|volume| (volume.source_entity, volume.brushes.as_slice()))
                .collect(),
        }
    }

    /// The first fault of `point`, solid before hull before fluid; `None`
    /// when it can hold a probe.
    pub(crate) fn fault(&self, point: DVec3) -> Option<PointFault> {
        if !self.tree.leaves.is_empty() {
            let leaf = find_leaf_for_point(self.tree, point);
            if self.tree.leaves[leaf].is_solid {
                return Some(PointFault::InSolid);
            }
            if self.exterior.contains(&leaf) {
                return Some(PointFault::OutsideHull);
            }
        }
        self.fluids
            .iter()
            .find(|(_, brushes)| brushes.iter().any(|brush| brush_contains(brush, point)))
            .map(|&(entity, _)| PointFault::InFluid(entity))
    }
}

/// Inside when `dot(p, n) - d <= 0` for every plane, as id 52 defines it.
fn brush_contains(brush: &EnvironmentBrush, p: DVec3) -> bool {
    let lo = DVec3::from(brush.aabb_min.map(f64::from));
    let hi = DVec3::from(brush.aabb_max.map(f64::from));
    if p.cmplt(lo).any() || p.cmpgt(hi).any() {
        return false;
    }
    brush.planes.iter().all(|plane| {
        DVec3::from(plane.normal.map(f64::from)).dot(p) - f64::from(plane.distance) <= 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<DVec2> {
        vec![
            DVec2::new(x0, z0),
            DVec2::new(x0, z1),
            DVec2::new(x1, z1),
            DVec2::new(x1, z0),
        ]
    }

    #[test]
    fn nearest_point_is_the_point_itself_inside_and_the_closest_edge_point_outside() {
        let outline = square(0.0, 0.0, 2.0, 1.0);
        assert_eq!(
            nearest_point_on_polygon(DVec2::new(1.0, 0.5), &outline),
            (DVec2::new(1.0, 0.5), 0.0)
        );
        let (q, d) = nearest_point_on_polygon(DVec2::new(3.0, 0.25), &outline);
        assert!((q - DVec2::new(2.0, 0.25)).length() < 1e-12 && (d - 1.0).abs() < 1e-12);
        let (q, _) = nearest_point_on_polygon(DVec2::new(-1.0, -1.0), &outline);
        assert!((q - DVec2::ZERO).length() < 1e-12, "a corner");
        // On an edge reads as over it.
        let (_, d) = nearest_point_on_polygon(DVec2::new(2.0, 0.5), &outline);
        assert!(d <= OVER_WATER_EPSILON_M);
    }
}
