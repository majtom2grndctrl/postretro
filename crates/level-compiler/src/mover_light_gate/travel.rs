// Where a mover stands at travel fraction t: its path as the runtime holds
// it, read by pose placement and by geometry knots' enclosure intervals.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel, Poses)

use glam::{DVec3, Vec3};
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;
use postretro_physics::kinematic_mover::MoverPathArcLengths;

use super::geometry_knots::KNOT_MERGE_TOLERANCE_M;

/// A mover's path as the runtime holds it (`f32` waypoints, as id 43 packs
/// them) and its arc lengths.
pub(super) struct PathTravel {
    path: Vec<Vec3>,
    arc_lengths: MoverPathArcLengths,
}

/// One path segment that adds length, as the mover's offset from its first
/// waypoint, linear in travel fraction t over `[t0, t1]`:
/// `offset0 + (t − t0) · per_t`.
#[derive(Debug, Clone, Copy)]
pub(super) struct TravelLeg {
    pub(super) t0: f64,
    pub(super) t1: f64,
    pub(super) offset0: DVec3,
    pub(super) per_t: DVec3,
}

impl PathTravel {
    pub(super) fn new(origins: &[DVec3]) -> Self {
        let path: Vec<Vec3> = origins.iter().map(|o| o.as_vec3()).collect();
        let arc_lengths = MoverPathArcLengths::from_waypoints(&path);
        Self { path, arc_lengths }
    }

    /// The path's length in metres; zero with no segment that adds length.
    pub(super) fn length(&self) -> f32 {
        self.arc_lengths.total_length()
    }

    /// The knot merge tolerance as a travel fraction; zero on a path with
    /// no length, which places no geometry knot.
    pub(super) fn merge_tolerance(&self) -> f64 {
        let length = f64::from(self.length());
        if length > 0.0 {
            KNOT_MERGE_TOLERANCE_M / length
        } else {
            0.0
        }
    }

    /// The mid pose at `t`'s offset from the first waypoint, by the
    /// runtime's own rule ([`MoverPathArcLengths::point_at`]). A path with
    /// no length has nothing between its termini, so a mid pose there
    /// stacks on the first.
    pub(super) fn mid_offset(&self, t: f32) -> DVec3 {
        let first = self.path[0];
        let position = self
            .arc_lengths
            .point_at(t)
            .map_or(first, |point| point.position);
        (position - first).as_dvec3()
    }

    /// Each segment that adds length, in path order, by the rule
    /// [`MoverPathArcLengths`] measures with: a segment at or below
    /// [`KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH`] or of non-finite length adds
    /// none, so no t lands on it. Lengths and their running sum are the
    /// runtime's `f32` values; the offsets along each leg are exact in `f64`,
    /// so a pose placed by [`Self::mid_offset`] differs from them only by
    /// that `f32` rounding.
    pub(super) fn legs(&self) -> Vec<TravelLeg> {
        let total = self.length();
        if !(total.is_finite() && total > 0.0) {
            return Vec::new();
        }
        let first = self.path[0].as_dvec3();
        let mut legs = Vec::new();
        let mut walked = 0.0_f32;
        for ends in self.path.windows(2) {
            let length = (ends[1] - ends[0]).length();
            if !(length.is_finite() && length > KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH) {
                continue;
            }
            let (from, to) = (ends[0].as_dvec3(), ends[1].as_dvec3());
            let start = walked;
            walked += length;
            legs.push(TravelLeg {
                t0: f64::from(start) / f64::from(total),
                t1: f64::from(walked) / f64::from(total),
                offset0: from - first,
                per_t: (to - from) * (f64::from(total) / f64::from(length)),
            });
        }
        legs
    }
}
