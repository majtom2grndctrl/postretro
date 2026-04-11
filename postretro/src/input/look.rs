// Evanescent look-input values drained once per render frame.
// See: context/lib/input.md §3

/// Gamepad look sensitivity: radians per second at full stick deflection.
/// Consumed by `LookInputs::yaw_delta` / `pitch_delta` when integrating
/// gamepad velocity over a render frame's elapsed time.
pub const GAMEPAD_LOOK_SENSITIVITY: f32 = 2.5;

/// Snapshot of the look-axis contributions accumulated since the last drain.
///
/// `*_displacement` fields hold already-scaled mouse deltas in radians
/// (evanescent — lost if not consumed this frame). `*_velocity` fields hold
/// gamepad stick deflections in `[-1, 1]`, resolved through the binding
/// table. Combine them with `yaw_delta` / `pitch_delta` to produce a
/// frame-rate-correct rotation step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LookInputs {
    pub yaw_displacement: f32,
    pub pitch_displacement: f32,
    pub yaw_velocity: f32,
    pub pitch_velocity: f32,
}

impl LookInputs {
    /// Combined yaw rotation for a render frame of length `frame_dt` seconds.
    /// Mouse displacement is applied as-is; gamepad velocity integrates over
    /// the frame's elapsed time at `GAMEPAD_LOOK_SENSITIVITY`.
    pub fn yaw_delta(&self, frame_dt: f32) -> f32 {
        self.yaw_displacement + self.yaw_velocity * GAMEPAD_LOOK_SENSITIVITY * frame_dt
    }

    /// Combined pitch rotation for a render frame of length `frame_dt` seconds.
    pub fn pitch_delta(&self, frame_dt: f32) -> f32 {
        self.pitch_displacement + self.pitch_velocity * GAMEPAD_LOOK_SENSITIVITY * frame_dt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_inputs_yaw_delta_combines_displacement_and_velocity() {
        let look = LookInputs {
            yaw_displacement: 0.02,
            pitch_displacement: 0.0,
            yaw_velocity: 0.5,
            pitch_velocity: 0.0,
        };
        // 0.02 + 0.5 * 2.5 * 0.016 = 0.02 + 0.02 = 0.04
        let delta = look.yaw_delta(0.016);
        let expected = 0.02 + 0.5 * GAMEPAD_LOOK_SENSITIVITY * 0.016;
        assert!(
            (delta - expected).abs() < 1e-6,
            "expected {}, got {}",
            expected,
            delta
        );
    }

    #[test]
    fn look_inputs_pitch_delta_combines_displacement_and_velocity() {
        let look = LookInputs {
            yaw_displacement: 0.0,
            pitch_displacement: -0.01,
            yaw_velocity: 0.0,
            pitch_velocity: -0.25,
        };
        let delta = look.pitch_delta(0.032);
        let expected = -0.01 + -0.25 * GAMEPAD_LOOK_SENSITIVITY * 0.032;
        assert!(
            (delta - expected).abs() < 1e-6,
            "expected {}, got {}",
            expected,
            delta
        );
    }

    #[test]
    fn look_inputs_default_produces_zero_deltas() {
        let look = LookInputs::default();
        assert!(look.yaw_delta(0.016).abs() < f32::EPSILON);
        assert!(look.pitch_delta(0.016).abs() < f32::EPSILON);
    }
}
