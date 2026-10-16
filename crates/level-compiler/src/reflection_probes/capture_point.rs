// Reflection-probe capture points: where over the water a probe captures, and the tests that drop it.
// See: context/lib/resource_management.md §5.1

use std::collections::HashSet;
use std::fmt;

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentVolumesSection};

use crate::partition::{BspTree, find_leaf_for_point};

use super::placement::{CAPTURE_LIFT_M, UpFacingFace};

/// A centroid moved onto the water lands this far inside the face it lands
/// on, toward that face's own centroid (at most halfway): a point exactly on
/// the edge would sit on the wall plane of the solid that cut the water
/// there, where the BSP's side is a tie and capture rays start in the wall.
/// Twice [`CAPTURE_CLEARANCE_M`], so a move straight off a wall clears it; a
/// move at a slant to the wall lands closer, and
/// [`PointClassifier::settle`] nudges it clear.
pub(crate) const NEAREST_POINT_INSET_M: f64 = 0.02;
/// A capture point needs this much horizontal room from solid. Capture rays
/// ignore hits closer than the tracer's 1 mm ray epsilon, so a point on or
/// within a millimetre of a wall would capture through it. A point is
/// crowded on a side when a step this long along ±X or ±Z lands in solid or
/// outside the hull; [`PointClassifier::settle`] moves a crowded point this
/// far off the crowding side, and [`PointClassifier::fault`] counts a point
/// still crowded as [`PointFault::Crowded`].
pub(crate) const CAPTURE_CLEARANCE_M: f64 = 0.01;
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
    /// In open space within [`CAPTURE_CLEARANCE_M`] of a wall, and no move
    /// that far off it stays in the box and over the water, or clears it.
    Crowded,
    /// In open space within [`CAPTURE_CLEARANCE_M`] of walls on both sides
    /// of an axis.
    Wedged,
    /// Inside a brush of this fluid entity, the probe's own included.
    InFluid(u32),
    OutsideHull,
}

impl fmt::Display for PointFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InSolid => write!(f, "in solid"),
            Self::Crowded => write!(
                f,
                "too close to a wall to move clear of it within its box and over its water"
            ),
            Self::Wedged => write!(f, "too close to walls on both sides to move clear of them"),
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

    /// Where a probe whose capture point comes out at `raw` captures: the
    /// point as stored, clamped to its box (`box_min..box_max`), moved clear
    /// of a wall it crowds; or the point and fault that drop it.
    ///
    /// `raw` itself in solid, outside the hull or inside a fluid drops with
    /// that fault, as does the stored point when the clamp moves it into one.
    /// A stored point crowded on one side of an axis moves
    /// [`CAPTURE_CLEARANCE_M`] toward the other, on X and Z independently;
    /// the step back toward the wall then lands on the point it moved from,
    /// which is clear, so a move leaves it at least that far off the wall.
    /// The move must stay in the box and, from a point over `water`, over
    /// `water`: when it cannot, staying over the water wins and the point
    /// drops as [`PointFault::Crowded`]; crowded on both sides of an axis, it
    /// drops as [`PointFault::Wedged`]. Either is in open space, so neither
    /// reads as in solid. The moved point must then pass [`Self::fault`] as
    /// stored.
    pub(crate) fn settle(
        &self,
        raw: DVec3,
        box_min: Vec3,
        box_max: Vec3,
        water: &[&UpFacingFace],
    ) -> Result<Vec3, (Vec3, PointFault)> {
        if let Some(fault) = self.own_fault(raw) {
            return Err((raw.as_vec3(), fault));
        }
        let stored = raw.as_vec3().clamp(box_min, box_max);
        let point = stored.as_dvec3();
        if let Some(fault) = self.own_fault(point) {
            return Err((stored, fault));
        }
        let mut nudge = DVec3::ZERO;
        for axis in [DVec3::X, DVec3::Z] {
            match (self.crowded(point, axis), self.crowded(point, -axis)) {
                (false, false) => {}
                (true, false) => nudge -= axis * CAPTURE_CLEARANCE_M,
                (false, true) => nudge += axis * CAPTURE_CLEARANCE_M,
                (true, true) => return Err((stored, PointFault::Wedged)),
            }
        }
        if nudge == DVec3::ZERO {
            return Ok(stored);
        }
        let moved = (point + nudge).as_vec3();
        let in_box = moved.cmpge(box_min).all() && moved.cmple(box_max).all();
        let keeps_water = !over_water(point, water) || over_water(moved.as_dvec3(), water);
        if !in_box || !keeps_water {
            return Err((stored, PointFault::Crowded));
        }
        match self.fault(moved.as_dvec3()) {
            Some(fault) => Err((moved, fault)),
            None => Ok(moved),
        }
    }

    /// The first fault of `point`, solid before hull before crowding before
    /// fluid; `None` when it can hold a probe. A point crowded on any side
    /// (see [`CAPTURE_CLEARANCE_M`]) is [`PointFault::Crowded`].
    pub(crate) fn fault(&self, point: DVec3) -> Option<PointFault> {
        match self.leaf_fault(point) {
            Some(fault) => Some(fault),
            None if [DVec3::X, DVec3::NEG_X, DVec3::Z, DVec3::NEG_Z]
                .into_iter()
                .any(|step| self.crowded(point, step)) =>
            {
                Some(PointFault::Crowded)
            }
            None => self.fluid_fault(point),
        }
    }

    /// The fault of `point` itself, without the clearance.
    fn own_fault(&self, point: DVec3) -> Option<PointFault> {
        self.leaf_fault(point).or_else(|| self.fluid_fault(point))
    }

    /// In solid, then outside the hull, by the leaf holding `point`.
    fn leaf_fault(&self, point: DVec3) -> Option<PointFault> {
        if self.tree.leaves.is_empty() {
            return None;
        }
        let leaf = find_leaf_for_point(self.tree, point);
        if self.tree.leaves[leaf].is_solid {
            Some(PointFault::InSolid)
        } else if self.exterior.contains(&leaf) {
            Some(PointFault::OutsideHull)
        } else {
            None
        }
    }

    /// Whether a [`CAPTURE_CLEARANCE_M`] step from `point` along unit `step`
    /// lands in solid or outside the hull.
    fn crowded(&self, point: DVec3, step: DVec3) -> bool {
        self.leaf_fault(point + step * CAPTURE_CLEARANCE_M)
            .is_some()
    }

    fn fluid_fault(&self, point: DVec3) -> Option<PointFault> {
        self.fluids
            .iter()
            .find(|(_, brushes)| brushes.iter().any(|brush| brush_contains(brush, point)))
            .map(|&(entity, _)| PointFault::InFluid(entity))
    }
}

/// Whether `point` lies over any of `faces`' water, horizontally.
fn over_water(point: DVec3, faces: &[&UpFacingFace]) -> bool {
    let p = DVec2::new(point.x, point.z);
    faces
        .iter()
        .any(|face| nearest_point_on_polygon(p, &face.outline).1 <= OVER_WATER_EPSILON_M)
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
