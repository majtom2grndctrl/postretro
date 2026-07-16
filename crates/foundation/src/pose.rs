//! Foundation owns transient, VM-free inputs for CPU-side skeletal pose modifiers.
//! See: context/lib/rendering_pipeline.md §9.

use glam::Vec3;

/// Maximum number of foot probes carried per pose evaluation.
///
/// Sized to cover multi-legged monsters, not just bipeds. The IK loader caps
/// its authored leg sets at this value, so it doubles as the leg-set capacity.
pub const MAX_FEET: usize = 6;

/// A single foot's ground-probe result, authored by the game-logic ground step.
///
/// All quantities are **model-space** — the probe is resolved against the same
/// space the pose is sampled in, so the IK solver consumes it without a further
/// transform. Like the rest of [`PoseInputs`], this is transient per-frame data
/// with no serde representation; it rides the per-instance render payload.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FootProbe {
    /// Model-space height of the ground contact under this foot.
    pub contact_height: f32,
    /// Model-space ground normal at the contact point.
    pub normal: Vec3,
    /// Whether the probe found ground. When `false` the other fields are unused.
    pub hit: bool,
}

/// Game-logic-authored inputs consumed while sampling a model pose.
///
/// All angles are radians. This is transient per-frame data rather than authored
/// or persistent state, so it intentionally has no serde representation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PoseInputs {
    /// Vertical aim angle relative to the entity's forward plane.
    pub aim_pitch: f32,
    /// World-space yaw toward the entity's aim target.
    pub aim_yaw: f32,
    /// World-space yaw of the entity's body heading.
    pub heading_yaw: f32,
    /// Per-foot ground probes in model space. Only the first `foot_count` are live.
    pub feet: [FootProbe; MAX_FEET],
    /// Number of populated entries in `feet`.
    pub foot_count: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_copy<T: Copy>(_: &T) {}

    #[test]
    fn pose_inputs_stays_copy_with_feet_populated() {
        let mut inputs = PoseInputs::default();
        inputs.feet[0] = FootProbe {
            contact_height: 1.5,
            normal: Vec3::Y,
            hit: true,
        };
        inputs.foot_count = 1;

        // Copy is what lets PoseInputs ride the per-instance render payload as POD.
        assert_copy(&inputs);
        let copied = inputs;
        // Mutating the original after the copy proves it was a value copy, not a move.
        inputs.foot_count = 3;
        assert_eq!(copied.foot_count, 1);
        assert_eq!(inputs.foot_count, 3);
        assert!(copied.feet[0].hit);
    }

    #[test]
    fn foot_probe_round_trips_fields() {
        let probe = FootProbe {
            contact_height: -2.25,
            normal: Vec3::new(0.0, 1.0, 0.0),
            hit: true,
        };

        assert_eq!(probe.contact_height, -2.25);
        assert_eq!(probe.normal, Vec3::new(0.0, 1.0, 0.0));
        assert!(probe.hit);
    }

    #[test]
    fn foot_probe_default_is_a_no_hit_zeroed_probe() {
        let probe = FootProbe::default();

        assert_eq!(probe.contact_height, 0.0);
        assert_eq!(probe.normal, Vec3::ZERO);
        assert!(!probe.hit);
    }
}
