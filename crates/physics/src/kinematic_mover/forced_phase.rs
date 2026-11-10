//! A mover's phase at a chosen travel fraction: the inverse of
//! [`mover_path_fraction`]. A tool that must show a mover at a known point of
//! its travel, such as a capture scene, sets the phase here. Everything a
//! frame derives from phase then agrees: the pose, the docked-closed portal
//! seal and the mover's light state.
//! See: context/lib/rendering_pipeline.md §7.8 · §4 (Mover state lighting)

use std::fmt;

use glam::Vec3;
use postretro_entities::KinematicMoverComponent;
use postretro_level_format::kinematic_geometry::KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH;

use super::MoverPathArcLengths;
#[cfg(doc)]
use super::mover_path_fraction;

/// Why a mover's phase cannot express a travel fraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForcedPhaseError {
    /// The fraction is not finite or lies outside `[0, 1]`.
    FractionOutOfRange(f32),
    /// A fraction strictly between the termini needs a mover that travels:
    /// a positive, finite speed on a path with length.
    NoTravel,
}

impl fmt::Display for ForcedPhaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FractionOutOfRange(t) => {
                write!(f, "travel fraction must be finite and in [0, 1], got {t}")
            }
            Self::NoTravel => f.write_str(
                "only a mover with a positive speed on a path with length can rest mid-travel",
            ),
        }
    }
}

impl std::error::Error for ForcedPhaseError {}

/// Set `mover`'s phase to rest at travel fraction `t`, heading toward its
/// last waypoint: 0 docks it at its first waypoint, 1 at its last, and a
/// fraction between places it that far along the path by arc length.
/// `arc_lengths` must describe `mover.waypoints`.
///
/// The phase is at rest: no wait, no stop hold, no target, not completed, no
/// velocity, and not active this tick. So t = 0 reads as docked closed and
/// seals its portals, as a mover settled at its first waypoint does.
/// [`mover_path_fraction`] then reads `t` back, within float rounding, and the
/// driver's pose for the phase is the point at `t`. Returns the position, for
/// the caller's transform.
pub fn set_mover_phase_at_path_fraction(
    mover: &mut KinematicMoverComponent,
    t: f32,
    arc_lengths: &MoverPathArcLengths,
) -> Result<Vec3, ForcedPhaseError> {
    debug_assert!(
        arc_lengths.describes(&mover.waypoints),
        "arc lengths must be built from this mover's path"
    );
    if !t.is_finite() || !(0.0..=1.0).contains(&t) {
        return Err(ForcedPhaseError::FractionOutOfRange(t));
    }
    let last = mover.waypoints.len().saturating_sub(1);
    let total = arc_lengths.total_length();
    let (segment, elapsed_ms, position) = if t == 0.0 || mover.waypoints.len() < 2 {
        (
            0,
            0.0,
            mover.waypoints.first().copied().unwrap_or(Vec3::ZERO),
        )
    } else if t == 1.0 {
        (last, 0.0, mover.waypoints[last])
    } else {
        if !total.is_finite()
            || total <= 0.0
            || !mover.speed_mps.is_finite()
            || mover.speed_mps <= 0.0
        {
            return Err(ForcedPhaseError::NoTravel);
        }
        let target = t * total;
        // The last non-degenerate segment that starts at or before the
        // target: a degenerate segment adds no length, so it is never chosen.
        let segment = (0..last)
            .rev()
            .find(|&index| {
                let length = (mover.waypoints[index + 1] - mover.waypoints[index]).length();
                length.is_finite()
                    && length > KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH
                    && arc_lengths.at_waypoint(index) <= target
            })
            .ok_or(ForcedPhaseError::NoTravel)?;
        let (from, to) = (mover.waypoints[segment], mover.waypoints[segment + 1]);
        let length = (to - from).length();
        let fraction = ((target - arc_lengths.at_waypoint(segment)) / length).clamp(0.0, 1.0);
        let duration_ms = (length / mover.speed_mps) * 1000.0;
        (segment, fraction * duration_ms, from.lerp(to, fraction))
    };
    mover.segment_index = u16::try_from(segment).unwrap_or(u16::MAX);
    mover.direction_sign = 1;
    mover.segment_elapsed_ms = elapsed_ms;
    mover.wait_remaining_ms = 0.0;
    mover.current_linear_velocity = Vec3::ZERO;
    mover.completed = false;
    mover.blocked = false;
    mover.target_segment = None;
    mover.was_active_this_tick = false;
    Ok(position)
}

#[cfg(test)]
mod tests {
    use super::super::{mover_is_docked_closed, mover_path_fraction, mover_pose_for_current_phase};
    use super::*;
    use postretro_entities::{KinematicMoverConfig, KinematicMoverMode, Transform};

    fn mover(waypoints: Vec<Vec3>, speed_mps: f32) -> KinematicMoverComponent {
        let names = (0..waypoints.len())
            .map(|index| format!("wp{index}"))
            .collect();
        KinematicMoverComponent::new(
            5,
            KinematicMoverConfig {
                waypoints,
                waypoint_names: names,
                speed_mps,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        )
    }

    fn force(mover: &mut KinematicMoverComponent, t: f32) -> Vec3 {
        let lengths = MoverPathArcLengths::from_waypoints(&mover.waypoints);
        set_mover_phase_at_path_fraction(mover, t, &lengths).expect("a fraction the path holds")
    }

    fn read_back(mover: &KinematicMoverComponent) -> f32 {
        mover_path_fraction(
            mover,
            &MoverPathArcLengths::from_waypoints(&mover.waypoints),
        )
    }

    fn driver_position(mover: &KinematicMoverComponent) -> Vec3 {
        mover_pose_for_current_phase(Transform::default(), mover, 0.0)
            .transform
            .position
    }

    #[test]
    fn a_forced_fraction_reads_back_and_poses_the_mover_there_across_unequal_segments() {
        // Segments of 1 m and 3 m: t = 0.25 is the middle waypoint.
        let mut path = mover(vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 3.0, 0.0)], 2.0);
        for (t, expected) in [
            (0.0, Vec3::ZERO),
            (0.125, Vec3::new(0.5, 0.0, 0.0)),
            (0.25, Vec3::X),
            (0.5, Vec3::new(1.0, 1.0, 0.0)),
            (1.0, Vec3::new(1.0, 3.0, 0.0)),
        ] {
            let position = force(&mut path, t);
            assert!(
                (read_back(&path) - t).abs() <= 1.0e-5,
                "t {t} reads back {}",
                read_back(&path)
            );
            assert!(position.abs_diff_eq(expected, 1.0e-5), "t {t}: {position}");
            assert!(
                driver_position(&path).abs_diff_eq(position, 1.0e-5),
                "t {t}: the driver's pose for the phase is the returned point"
            );
        }
    }

    #[test]
    fn only_a_forced_first_waypoint_docks_closed() {
        let mut door = mover(vec![Vec3::ZERO, Vec3::Z], 1.0);
        door.current_linear_velocity = Vec3::Z;
        door.was_active_this_tick = true;
        door.blocked = true;
        force(&mut door, 0.0);
        assert!(mover_is_docked_closed(&door), "t = 0 settles and seals");
        for t in [0.5, 1.0] {
            force(&mut door, t);
            assert!(!mover_is_docked_closed(&door), "t {t} seals nothing");
        }
    }

    #[test]
    fn a_degenerate_segment_is_skipped_and_out_of_range_or_motionless_fractions_are_refused() {
        let mut path = mover(vec![Vec3::ZERO, Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)], 1.0);
        let position = force(&mut path, 0.5);
        assert!(position.abs_diff_eq(Vec3::Y, 1.0e-5), "{position}");
        assert!((read_back(&path) - 0.5).abs() <= 1.0e-5);

        let lengths = MoverPathArcLengths::from_waypoints(&path.waypoints);
        for t in [-0.1, 1.1, f32::NAN] {
            assert!(matches!(
                set_mover_phase_at_path_fraction(&mut path, t, &lengths),
                Err(ForcedPhaseError::FractionOutOfRange(_))
            ));
        }
        let mut still = mover(vec![Vec3::ZERO, Vec3::X], 0.0);
        let lengths = MoverPathArcLengths::from_waypoints(&still.waypoints);
        assert_eq!(
            set_mover_phase_at_path_fraction(&mut still, 0.5, &lengths),
            Err(ForcedPhaseError::NoTravel)
        );
        assert_eq!(
            set_mover_phase_at_path_fraction(&mut still, 1.0, &lengths),
            Ok(Vec3::X),
            "a terminus needs no travel"
        );
    }
}
