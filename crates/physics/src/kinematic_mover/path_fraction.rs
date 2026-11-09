//! A mover's travel fraction: how far along its waypoint path its phase is, by
//! arc length. Mover state lighting blends a mover's two baked terminus states
//! by this fraction, so every peer derives it from the replicated phase alone.
//! See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use glam::Vec3;
use postretro_entities::KinematicMoverComponent;
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

use super::segment_indices;

/// Cumulative arc length at each waypoint of one mover's path, built once per
/// path and reused every frame. A segment at or below
/// [`KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH`], or of non-finite length, adds no
/// length, matching the driver's degenerate-segment rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MoverPathArcLengths {
    /// The path these lengths describe, so a caller can tell a stale table.
    waypoints: Vec<Vec3>,
    /// Arc length from the first waypoint to waypoint `i`.
    cumulative: Vec<f32>,
}

impl MoverPathArcLengths {
    pub fn from_waypoints(waypoints: &[Vec3]) -> Self {
        let mut lengths = Self::default();
        lengths.rebuild(waypoints);
        lengths
    }

    /// Rebuild for `waypoints`, reusing this table's buffers.
    pub fn rebuild(&mut self, waypoints: &[Vec3]) {
        self.waypoints.clear();
        self.waypoints.extend_from_slice(waypoints);
        self.cumulative.clear();
        let mut total = 0.0_f32;
        let mut previous: Option<Vec3> = None;
        for &waypoint in waypoints {
            if let Some(previous) = previous {
                let length = (waypoint - previous).length();
                if length.is_finite() && length > KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
                    total += length;
                }
            }
            self.cumulative.push(total);
            previous = Some(waypoint);
        }
    }

    /// Whether this table was built from exactly `waypoints`.
    pub fn describes(&self, waypoints: &[Vec3]) -> bool {
        self.waypoints.as_slice() == waypoints
    }

    /// Rebuild only when the table does not already describe `waypoints`, so a
    /// per-frame caller allocates nothing once the path is known.
    pub fn sync_to(&mut self, waypoints: &[Vec3]) {
        if !self.describes(waypoints) {
            self.rebuild(waypoints);
        }
    }

    /// The whole path's length; zero for a path with no non-degenerate segment.
    pub fn total_length(&self) -> f32 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    pub(super) fn at_waypoint(&self, index: usize) -> f32 {
        self.cumulative.get(index).copied().unwrap_or(0.0)
    }
}

/// The mover's travel fraction t from its current phase: 0 at the first
/// waypoint, 1 at the last, the arc-length fraction between. `arc_lengths`
/// must describe `mover.waypoints`.
///
/// Reads phase exactly as the driver's pose does: a wait, a completed run, or a
/// terminus with no segment left (an auto-close driven home, a finished
/// `once` run) reads its waypoint's fraction from the waypoint index; a stop
/// hold changes no phase, so it holds t; a reversal re-expresses the same
/// pose, so t continues with no jump. The result is always finite and in
/// `[0, 1]`: a path with no length reads 0.
pub fn mover_path_fraction(
    mover: &KinematicMoverComponent,
    arc_lengths: &MoverPathArcLengths,
) -> f32 {
    debug_assert!(
        arc_lengths.cumulative.len() == mover.waypoints.len(),
        "arc lengths must be built from this mover's path"
    );
    let total = arc_lengths.total_length();
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let fraction = arc_length_for_phase(mover, arc_lengths) / total;
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Arc length from the first waypoint to the phase's pose, mirroring
/// `position_for_phase` branch for branch.
fn arc_length_for_phase(mover: &KinematicMoverComponent, arc_lengths: &MoverPathArcLengths) -> f32 {
    if mover.waypoints.is_empty() {
        return 0.0;
    }
    let endpoint = usize::from(mover.segment_index).min(mover.waypoints.len() - 1);
    if mover.completed || mover.wait_remaining_ms > 0.0 {
        return arc_lengths.at_waypoint(endpoint);
    }
    let Some((from_index, to_index)) = segment_indices(mover) else {
        return arc_lengths.at_waypoint(endpoint);
    };
    let from = arc_lengths.at_waypoint(from_index);
    let length = (mover.waypoints[to_index] - mover.waypoints[from_index]).length();
    if !length.is_finite()
        || length <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH
        || mover.speed_mps <= 0.0
    {
        return from;
    }
    let duration_ms = (length / mover.speed_mps) * 1000.0;
    let traversed = (mover.segment_elapsed_ms / duration_ms).clamp(0.0, 1.0);
    let to = arc_lengths.at_waypoint(to_index);
    from + (to - from) * traversed
}

#[cfg(test)]
mod tests {
    use super::super::{
        advance_mover_phase_one_tick, apply_mover_command, mover_is_docked_closed,
        travel_toward_closed_terminus,
    };
    use super::*;
    use postretro_entities::{KinematicMoverConfig, KinematicMoverMode, MoverCommand, Transform};

    const EPSILON: f32 = 1.0e-5;
    const TICK_DT: f32 = 0.25;

    fn mover(
        waypoints: Vec<Vec3>,
        mode: KinematicMoverMode,
        wait_ms: f32,
    ) -> KinematicMoverComponent {
        let names = (0..waypoints.len())
            .map(|index| format!("wp{index}"))
            .collect();
        KinematicMoverComponent::new(
            3,
            KinematicMoverConfig {
                waypoints,
                waypoint_names: names,
                speed_mps: 1.0,
                wait_ms,
                mode,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        )
    }

    /// A one-metre, two-waypoint door: closed at waypoint 0, open at 1.
    fn one_metre_door(mode: KinematicMoverMode) -> KinematicMoverComponent {
        mover(vec![Vec3::ZERO, Vec3::X], mode, 0.0)
    }

    fn tick(mover: &mut KinematicMoverComponent) {
        let mut transform = Transform::default();
        advance_mover_phase_one_tick(mover, &mut transform, TICK_DT);
    }

    fn fraction(mover: &KinematicMoverComponent) -> f32 {
        mover_path_fraction(
            mover,
            &MoverPathArcLengths::from_waypoints(&mover.waypoints),
        )
    }

    fn assert_fraction(mover: &KinematicMoverComponent, expected: f32, context: &str) {
        let t = fraction(mover);
        assert!(
            (t - expected).abs() <= EPSILON,
            "{context}: expected t = {expected}, got {t}"
        );
    }

    #[test]
    fn path_fraction_reads_zero_docked_at_the_first_waypoint_and_one_on_the_arrival_tick() {
        let mut door = one_metre_door(KinematicMoverMode::Once);
        assert_eq!(fraction(&door), 0.0, "seeded at waypoint 0 before any tick");
        for _ in 0..3 {
            tick(&mut door);
        }
        assert_fraction(&door, 0.75, "three quarter-second ticks at 1 m/s");
        // P1: the tick that reaches the last waypoint reads exactly 1.
        tick(&mut door);
        assert!(door.completed, "the arrival tick completes a `once` run");
        assert_eq!(
            fraction(&door),
            1.0,
            "arrival at the last waypoint reads t = 1"
        );
    }

    #[test]
    fn path_fraction_is_the_arc_length_fraction_across_unequal_segments() {
        // Segments of 1 m and 3 m: the middle waypoint sits at a quarter of the arc.
        let mut lift = mover(
            vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 3.0, 0.0)],
            KinematicMoverMode::Once,
            0.0,
        );
        let lengths = MoverPathArcLengths::from_waypoints(&lift.waypoints);
        assert!((lengths.total_length() - 4.0).abs() <= EPSILON);

        tick(&mut lift);
        tick(&mut lift);
        assert_fraction(&lift, 0.125, "halfway along the first segment");
        tick(&mut lift);
        tick(&mut lift);
        assert_eq!(
            lift.segment_index, 1,
            "four ticks reach the middle waypoint"
        );
        assert_fraction(&lift, 0.25, "the middle waypoint");
        for _ in 0..6 {
            tick(&mut lift);
        }
        assert_fraction(&lift, 0.625, "halfway along the second segment");
        for _ in 0..6 {
            tick(&mut lift);
        }
        assert_eq!(fraction(&lift), 1.0, "the last waypoint");
    }

    #[test]
    fn path_fraction_holds_through_an_endpoint_wait_and_a_stop_hold() {
        let mut door = mover(
            vec![Vec3::ZERO, Vec3::X],
            KinematicMoverMode::PingPong,
            600.0,
        );
        for _ in 0..4 {
            tick(&mut door);
        }
        assert!(
            door.wait_remaining_ms > 0.0,
            "arrival starts the endpoint wait"
        );
        assert_eq!(fraction(&door), 1.0);
        tick(&mut door);
        tick(&mut door);
        assert!(door.wait_remaining_ms > 0.0, "still waiting");
        assert_eq!(fraction(&door), 1.0, "a wait holds t");

        // Leave the wait, travel back a quarter, then hold.
        tick(&mut door);
        tick(&mut door);
        let travelling = fraction(&door);
        assert!(
            travelling < 1.0,
            "the return trip has begun, t = {travelling}"
        );
        door.blocked = true;
        for _ in 0..3 {
            tick(&mut door);
            assert_eq!(fraction(&door), travelling, "a stop hold holds t");
        }
    }

    #[test]
    fn path_fraction_continues_without_a_jump_through_reversals_in_either_direction() {
        let mut lift = mover(
            vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 3.0, 0.0)],
            KinematicMoverMode::Once,
            0.0,
        );
        for _ in 0..10 {
            tick(&mut lift);
        }
        let opening = fraction(&lift);
        assert!((opening - 0.625).abs() <= EPSILON);

        apply_mover_command(&mut lift, &MoverCommand::Reverse);
        assert_eq!(lift.direction_sign, -1, "the reversal heads back");
        assert_fraction(&lift, opening, "reversing while opening");
        tick(&mut lift);
        assert_fraction(&lift, opening - 0.0625, "closing from the reversal point");

        let closing = fraction(&lift);
        apply_mover_command(&mut lift, &MoverCommand::Reverse);
        assert_eq!(lift.direction_sign, 1, "the second reversal opens again");
        assert_fraction(&lift, closing, "reversing while closing");
        tick(&mut lift);
        assert_fraction(
            &lift,
            closing + 0.0625,
            "opening from the second reversal point",
        );
    }

    #[test]
    fn path_fraction_reads_terminus_waypoints_with_no_segment_left() {
        // P9: auto-close drives an open door home; it ends with no segment
        // ahead and still reads its waypoint's t.
        let mut door = one_metre_door(KinematicMoverMode::Once);
        for _ in 0..4 {
            tick(&mut door);
        }
        assert!(door.completed);
        assert_eq!(
            fraction(&door),
            1.0,
            "a completed `once` run reads its last waypoint"
        );
        travel_toward_closed_terminus(&mut door);
        for _ in 0..4 {
            tick(&mut door);
        }
        assert_eq!(door.segment_index, 0);
        assert!(
            door.completed,
            "the auto-close target completes at waypoint 0"
        );
        assert_eq!(
            fraction(&door),
            0.0,
            "an auto-close driven home reads t = 0"
        );

        // A ping-pong mover parked at its first waypoint heading off the path.
        let mut parked = one_metre_door(KinematicMoverMode::PingPong);
        parked.direction_sign = -1;
        parked.started = false;
        assert!(segment_indices(&parked).is_none(), "no segment ahead");
        assert_eq!(fraction(&parked), 0.0);
        parked.segment_index = 1;
        parked.direction_sign = 1;
        assert!(segment_indices(&parked).is_none(), "no segment ahead");
        assert_eq!(fraction(&parked), 1.0);
    }

    #[test]
    fn path_fraction_reads_zero_on_the_closing_arrival_tick_before_portals_seal() {
        // P11: the tick that reaches waypoint 0 reads t = 0; the docked
        // predicate seals portals only from the next, settle tick. No tick has
        // the door docked while t is above 0.
        let mut door = one_metre_door(KinematicMoverMode::Once);
        tick(&mut door);
        tick(&mut door);
        travel_toward_closed_terminus(&mut door);
        let mut arrival_tick = None;
        for tick_index in 0..6 {
            tick(&mut door);
            let t = fraction(&door);
            if mover_is_docked_closed(&door) {
                assert_eq!(t, 0.0, "tick {tick_index}: docked only at t = 0");
            }
            if t == 0.0 && arrival_tick.is_none() {
                arrival_tick = Some(tick_index);
                assert!(
                    !mover_is_docked_closed(&door),
                    "the arrival tick reads t = 0 before its portals seal"
                );
            }
        }
        let arrival_tick = arrival_tick.expect("the door closes within six ticks");
        assert_eq!(
            arrival_tick, 1,
            "two quarter-second ticks close half a metre"
        );
        assert!(
            mover_is_docked_closed(&door),
            "the settle tick after arrival seals"
        );
    }

    #[test]
    fn path_fraction_stays_finite_on_degenerate_paths() {
        // P23: a zero-length path reads 0, never NaN.
        let mut still = mover(vec![Vec3::ONE, Vec3::ONE], KinematicMoverMode::Once, 0.0);
        assert_eq!(
            MoverPathArcLengths::from_waypoints(&still.waypoints).total_length(),
            0.0
        );
        assert_eq!(fraction(&still), 0.0);
        for _ in 0..3 {
            tick(&mut still);
            assert_eq!(fraction(&still), 0.0, "a zero-length path stays at 0");
        }
        let single = mover(vec![Vec3::ONE], KinematicMoverMode::Once, 0.0);
        assert_eq!(fraction(&single), 0.0, "a one-waypoint rotator reads 0");

        // One degenerate interior segment adds no length: waypoints 1 and 2
        // share the arc's midpoint.
        let degenerate = vec![Vec3::ZERO, Vec3::X, Vec3::X, Vec3::new(1.0, 1.0, 0.0)];
        let lengths = MoverPathArcLengths::from_waypoints(&degenerate);
        assert!((lengths.total_length() - 2.0).abs() <= EPSILON);
        let mut lift = mover(degenerate, KinematicMoverMode::Once, 0.0);
        lift.segment_index = 1;
        lift.segment_elapsed_ms = 100.0;
        assert_fraction(&lift, 0.5, "inside the degenerate segment");
        lift.segment_index = 0;
        lift.segment_elapsed_ms = 0.0;
        let mut previous = 0.0;
        for _ in 0..12 {
            tick(&mut lift);
            let t = fraction(&lift);
            assert!(t.is_finite() && (0.0..=1.0).contains(&t), "t = {t}");
            assert!(t + EPSILON >= previous, "a `once` run never moves back");
            previous = t;
        }

        // A non-finite speed leaves a finite t.
        let mut runaway = one_metre_door(KinematicMoverMode::Once);
        runaway.speed_mps = f32::NAN;
        runaway.segment_elapsed_ms = 10.0;
        assert!(fraction(&runaway).is_finite());
    }

    #[test]
    fn arc_lengths_sync_rebuilds_only_for_a_different_path() {
        let mut lengths = MoverPathArcLengths::from_waypoints(&[Vec3::ZERO, Vec3::X]);
        assert!(lengths.describes(&[Vec3::ZERO, Vec3::X]));
        lengths.sync_to(&[Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)]);
        assert!((lengths.total_length() - 3.0).abs() <= EPSILON);
        assert!(!lengths.describes(&[Vec3::ZERO, Vec3::X]));
    }
}
