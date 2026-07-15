//! Foundation owns transient, VM-free inputs for CPU-side skeletal pose modifiers.
//! See: context/lib/rendering_pipeline.md §9.

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
}
