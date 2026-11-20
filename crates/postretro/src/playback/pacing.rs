// Wall-clock pacing: where on the route a frame at elapsed time t sits, and
// the pose interpolated there. Pure: no clock, App or GPU.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;
use postretro_camera_route::RoutePose;

/// Where a wall-clock route frame sits: pose `pose`, `blend` of the way to
/// the next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClockPick {
    pub(crate) pose: usize,
    /// In `[0, 1)`; zero draws `pose` exactly.
    pub(crate) blend: f32,
    /// At or past the last pose's time: this frame draws the last pose and
    /// is the route's last frame.
    pub(crate) last: bool,
}

/// The route position at `elapsed_seconds` since route frame 0, at
/// `cadence_hz` poses a second (pin P16). `cut[i]` marks pose `i` reached by
/// a teleport: interpolation never crosses it, so the earlier pose draws
/// until the clock reaches the cut pose.
pub(crate) fn pick(elapsed_seconds: f64, cadence_hz: f64, cut: &[bool]) -> ClockPick {
    let last_index = cut.len().saturating_sub(1);
    let position = (elapsed_seconds * cadence_hz).max(0.0);
    if position >= last_index as f64 {
        return ClockPick {
            pose: last_index,
            blend: 0.0,
            last: true,
        };
    }
    // Below `last_index`, so the floor fits and a next pose exists.
    let pose = position.floor() as usize;
    let blend = if cut[pose + 1] {
        0.0
    } else {
        (position - pose as f64) as f32
    };
    ClockPick {
        pose,
        blend,
        last: false,
    }
}

/// The eye, yaw and pitch `blend` of the way from `from` to `to`: eye and
/// pitch linear, yaw the short way round. A zero blend is `from`, bit-equal.
pub(crate) fn interpolate(from: &RoutePose, to: &RoutePose, blend: f32) -> (Vec3, f32, f32) {
    if blend <= 0.0 {
        return (from.eye, from.yaw, from.pitch);
    }
    let turn = (to.yaw - from.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    (
        from.eye.lerp(to.eye, blend),
        from.yaw + turn * blend,
        from.pitch + (to.pitch - from.pitch) * blend,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, yaw: f32, cut: bool) -> RoutePose {
        RoutePose {
            eye: Vec3::new(x, 1.0, 0.0),
            yaw,
            pitch: x * 0.01,
            walk: None,
            cut,
        }
    }

    #[test]
    fn wall_clock_pick_reaches_pose_k_at_k_over_cadence() {
        let cut = [false; 5];
        for k in 0..4 {
            let at = pick(k as f64 / 60.0, 60.0, &cut);
            let drawn = at.pose as f32 + at.blend;
            assert!(
                (drawn - k as f32).abs() < 1.0e-4,
                "t = {k}/60 s draws pose {k}: {at:?}"
            );
            assert!(!at.last);
        }
        let between = pick(2.5 / 60.0, 60.0, &cut);
        assert_eq!(between.pose, 2);
        assert!((between.blend - 0.5).abs() < 1.0e-4);
    }

    #[test]
    fn wall_clock_pick_at_or_past_the_last_pose_draws_it_and_ends() {
        let cut = [false; 4];
        for elapsed in [3.0 / 60.0, 3.2 / 60.0, 10.0] {
            assert_eq!(
                pick(elapsed, 60.0, &cut),
                ClockPick {
                    pose: 3,
                    blend: 0.0,
                    last: true
                },
                "{elapsed}"
            );
        }
        // A one-pose file draws its pose once, at elapsed zero.
        assert_eq!(
            pick(0.0, 60.0, &[false]),
            ClockPick {
                pose: 0,
                blend: 0.0,
                last: true
            }
        );
    }

    #[test]
    fn wall_clock_pick_never_interpolates_across_a_cut() {
        // Pose 2 is reached by a teleport: between poses 1 and 2 the earlier
        // pose draws, then the cut pose once the clock reaches it.
        let cut = [false, false, true, false];
        let before = pick(1.7 / 60.0, 60.0, &cut);
        assert_eq!((before.pose, before.blend), (1, 0.0));
        let at = pick(2.0 / 60.0 + 1.0e-9, 60.0, &cut);
        assert_eq!(at.pose, 2);
        let after = pick(2.5 / 60.0, 60.0, &cut);
        assert_eq!(after.pose, 2);
        assert!(after.blend > 0.4, "past the cut it interpolates again");
    }

    #[test]
    fn interpolate_turns_the_short_way_round_and_zero_blend_is_bit_equal() {
        let from = pose(0.0, 3.0, false);
        let to = pose(2.0, -3.0, false);
        let (eye, yaw, pitch) = interpolate(&from, &to, 0.5);
        assert_eq!(eye, Vec3::new(1.0, 1.0, 0.0));
        assert!((pitch - 0.01).abs() < 1.0e-6);
        // 3.0 to -3.0 is 0.28 rad the short way, through ±π, not 6 rad back.
        let short = std::f32::consts::TAU - 6.0;
        assert!((yaw - (3.0 + short * 0.5)).abs() < 1.0e-5, "{yaw}");

        let exact = interpolate(&from, &to, 0.0);
        assert_eq!(
            (exact.0.to_array(), exact.1.to_bits(), exact.2.to_bits()),
            (
                from.eye.to_array(),
                from.yaw.to_bits(),
                from.pitch.to_bits()
            )
        );
    }
}
