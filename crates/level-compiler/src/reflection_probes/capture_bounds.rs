// Reflection-probe capture bounds: which neighbour holds an excluded hull's exit, and the projection box the leak guard tests.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};

use super::capture_exclusion::CONTACT_TOLERANCE_M;
use super::placement::{PLANE_TOLERANCE_M, PlacedProbe};
use crate::map_data::BrushVolume;
use crate::partition::Aabb;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

/// Where a neighbour the ray spans over `(enters, leaves)` holds what lies
/// just past `exit` from the excluded `hull`: the first point at or past the
/// exit where the ray is inside it, when the ray leaves it after the exit
/// and that point lies within [`CONTACT_TOLERANCE_M`] of `hull`. Measured
/// off the hull's planes, as CSG measures contact, so a gap it cut holds at
/// any ray angle. `None` when it does not hold.
pub(super) fn held_at(
    hull: &BrushVolume,
    (enters, leaves): (f64, f64),
    exit: f64,
    (origin, direction): (DVec3, DVec3),
) -> Option<f64> {
    let at = enters.max(exit);
    (leaves > exit
        && enters < leaves
        && outside_by(hull, origin + direction * at) <= CONTACT_TOLERANCE_M)
        .then_some(at)
}

/// The neighbour that holds an exit and that the ray is inside first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Held {
    /// Kept solid: the leak guard.
    Kept,
    /// The excluded hull in this slot of [`ProbeExclusion::excluded`], which
    /// the ray goes on through and leaves at this distance.
    Excluded { slot: usize, leaves: f64 },
}

/// Of the neighbours that hold an exit, the one the ray is inside first, by
/// where [`held_at`] measures each: `kept` gives each kept one's depth,
/// `excluded` each excluded one's depth, slot and leaving distance. Kept
/// solid wins an exact tie, as where a kept and an excluded brush overlap at
/// the exit; excluded ones tie in the order given.
///
/// Depth decides, not slot order, which is brush order and says nothing of
/// the ray: an excluded brush taken out of depth order steps over kept solid
/// entered first through a face CSG removed, and the re-trace from its exit
/// reads through that solid. Checking kept solid first, at any depth, is no
/// fix: where kept solid lies behind the excluded brush the ray enters
/// first, the ray may read past that brush's far side, and the kept face it
/// meets there (`resource_management.md` §5.2), so taking the guard instead
/// shows the excluded brush where the capture need not.
///
/// Known limit: kept solid that holds the exit behind the excluded brush
/// taken is not carried on. Should the ray leave that brush into open
/// space, beyond its contact, then enter the kept solid through a face CSG
/// removed against the hull whose exit it held, the re-trace reads through
/// the solid.
pub(super) fn nearest_held(
    kept: impl IntoIterator<Item = f64>,
    excluded: impl IntoIterator<Item = (f64, usize, f64)>,
) -> Option<Held> {
    let kept = kept.into_iter().min_by(f64::total_cmp);
    // `min_by` keeps the first of equal depths.
    let excluded = excluded
        .into_iter()
        .min_by(|(a, ..), (b, ..)| a.total_cmp(b));
    match (kept, excluded) {
        (Some(kept), Some((at, ..))) if kept <= at => Some(Held::Kept),
        (_, Some((_, slot, leaves))) => Some(Held::Excluded { slot, leaves }),
        (Some(_), None) => Some(Held::Kept),
        (None, None) => None,
    }
}

/// How far `point` lies outside `hull`: its greatest distance past any of
/// the hull's planes, negative inside.
pub(super) fn outside_by(hull: &BrushVolume, point: DVec3) -> f64 {
    hull.planes
        .iter()
        .map(|plane| plane.normal.dot(point) - plane.distance)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// A probe's projection box as the leak guard tests it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectionBox {
    min: DVec3,
    max: DVec3,
}

impl ProjectionBox {
    pub(crate) fn of(probe: &PlacedProbe) -> Self {
        Self {
            min: Vec3::from(probe.box_min).as_dvec3(),
            max: Vec3::from(probe.box_max).as_dvec3(),
        }
    }

    /// Whether `point` lies in the box within [`PLANE_TOLERANCE_M`], so a
    /// wall or ceiling the box was fitted to holds on rounding. The floor
    /// along the reflector normal (up, for water) is ignored: a derived box
    /// sets it at the capture point's height, so a ray below the horizon
    /// takes the guard only on a miss.
    pub(super) fn holds(&self, point: DVec3) -> bool {
        let tolerance = PLANE_TOLERANCE_M;
        point.x >= self.min.x - tolerance
            && point.x <= self.max.x + tolerance
            && point.z >= self.min.z - tolerance
            && point.z <= self.max.z + tolerance
            && point.y <= self.max.y + tolerance
    }

    /// Whether `aabb` touches the box under [`Self::holds`]'s rule, its floor
    /// ignored: within [`PLANE_TOLERANCE_M`] and 2τ more
    /// ([`TRACER_TOLERANCE_METERS`]). A tolerance hit lies up to τ off its
    /// triangle; the second τ is margin for where `holds` reads the hit
    /// against the hull's `f64` bounds: the mesh corners' `f32` rounding
    /// (half an ulp, 15 µm at ±500 m) and the `f32` hit distance's (2⁻²⁴ of
    /// it, 60 µm at 1 km). A hull that does not touch the box holds no point
    /// the box holds, and no tolerance hit on its faces lands in it.
    pub(crate) fn touches(&self, aabb: &Aabb) -> bool {
        let tolerance = PLANE_TOLERANCE_M + 2.0 * f64::from(TRACER_TOLERANCE_METERS);
        aabb.min.x <= self.max.x + tolerance
            && aabb.max.x >= self.min.x - tolerance
            && aabb.min.z <= self.max.z + tolerance
            && aabb.max.z >= self.min.z - tolerance
            && aabb.min.y <= self.max.y + tolerance
    }
}
