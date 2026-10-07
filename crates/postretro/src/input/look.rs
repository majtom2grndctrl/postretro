// Evanescent look-input values drained once per render frame.
// See: context/lib/input.md §3

/// Gamepad look sensitivity: radians per second at full stick deflection.
/// Default for `LookInputs::gamepad_sensitivity`.
pub const DEFAULT_GAMEPAD_LOOK_SENSITIVITY: f32 = 2.5;

/// Snapshot of the look-axis contributions accumulated since the last drain.
///
/// `*_displacement` fields hold already-scaled mouse deltas in radians
/// (evanescent — lost if not consumed this frame). `*_velocity` fields hold
/// gamepad stick deflections in `[-1, 1]`, resolved through the binding
/// table. Combine them with `yaw_delta` / `pitch_delta` to produce a
/// frame-rate-correct rotation step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookInputs {
    pub yaw_displacement: f32,
    pub pitch_displacement: f32,
    pub yaw_velocity: f32,
    pub pitch_velocity: f32,
    /// Radians per second at full stick deflection: the player's gamepad look
    /// sensitivity, separate from mouse sensitivity.
    pub gamepad_sensitivity: f32,
}

impl Default for LookInputs {
    fn default() -> Self {
        Self {
            yaw_displacement: 0.0,
            pitch_displacement: 0.0,
            yaw_velocity: 0.0,
            pitch_velocity: 0.0,
            gamepad_sensitivity: DEFAULT_GAMEPAD_LOOK_SENSITIVITY,
        }
    }
}

impl LookInputs {
    /// Combined yaw rotation for a render frame of length `frame_dt` seconds.
    /// Mouse displacement is applied as-is; gamepad velocity integrates over
    /// the frame's elapsed time at `DEFAULT_GAMEPAD_LOOK_SENSITIVITY`.
    pub fn yaw_delta(&self, frame_dt: f32) -> f32 {
        self.yaw_displacement + self.yaw_velocity * self.gamepad_sensitivity * frame_dt
    }

    /// Combined pitch rotation for a render frame of length `frame_dt` seconds.
    pub fn pitch_delta(&self, frame_dt: f32) -> f32 {
        self.pitch_displacement + self.pitch_velocity * self.gamepad_sensitivity * frame_dt
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
            ..LookInputs::default()
        };
        // 0.02 + 0.5 * 2.5 * 0.016 = 0.02 + 0.02 = 0.04
        let delta = look.yaw_delta(0.016);
        let expected = 0.02 + 0.5 * DEFAULT_GAMEPAD_LOOK_SENSITIVITY * 0.016;
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
            ..LookInputs::default()
        };
        let delta = look.pitch_delta(0.032);
        let expected = -0.01 + -0.25 * DEFAULT_GAMEPAD_LOOK_SENSITIVITY * 0.032;
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
