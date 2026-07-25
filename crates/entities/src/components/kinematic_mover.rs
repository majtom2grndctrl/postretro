// Deterministic kinematic mover state.
//
// Scripts query movers through `world.query` handles. Raw phase remains
// engine-owned and cannot be attached or mutated directly.

use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KinematicMoverMode {
    Once,
    PingPong,
}

/// Closed, declarative commands accepted by the deterministic mover driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoverCommand {
    Start,
    Stop,
    Reverse,
    GoToPathNode(String),
    /// Set the authored target spin rate in degrees per second.
    SetSpinRate(f32),
}

/// Live deterministic phase for one moving-world payload.
///
/// The waypoint list, speed, wait, and mode are static path data seeded when the
/// mover is constructed. The remaining fields are phase mirrored by the wire
/// payload without replicating the path itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KinematicMoverComponent {
    pub mover_id: u32,
    pub waypoints: Vec<Vec3>,
    /// Static, resolved waypoint names paired index-for-index with `waypoints`.
    pub waypoint_names: Vec<String>,
    pub speed_mps: f32,
    pub wait_ms: f32,
    pub mode: KinematicMoverMode,
    /// Static local-space rotation axis, normalized at construction.
    pub spin_axis: Vec3,
    /// Static angular acceleration used to approach the target spin rate.
    pub spin_accel_rad_s2: f32,
    /// Static rider-orientation policy, held locally rather than replicated.
    pub carry_yaw: bool,
    pub segment_index: u16,
    pub direction_sign: i8,
    pub segment_elapsed_ms: f32,
    pub wait_remaining_ms: f32,
    pub current_linear_velocity: Vec3,
    pub started: bool,
    pub completed: bool,
    /// Runtime target waypoint index for `go_to_path_node`; replicated as phase.
    pub target_segment: Option<u16>,
    /// Replicated accumulated spin phase, wrapped by the deterministic driver.
    pub spin_angle_rad: f32,
    /// Replicated current spin rate after the driver's acceleration ramp.
    pub spin_rate_rad_s: f32,
    /// Replicated target spin rate set by future authored commands.
    pub spin_target_rate_rad_s: f32,
}

impl KinematicMoverComponent {
    pub fn new(
        mover_id: u32,
        waypoints: Vec<Vec3>,
        waypoint_names: Vec<String>,
        speed_mps: f32,
        wait_ms: f32,
        mode: KinematicMoverMode,
        started: bool,
        spin_axis: Vec3,
        initial_spin_rate_rad_s: f32,
        spin_accel_rad_s2: f32,
        carry_yaw: bool,
    ) -> Self {
        Self {
            mover_id,
            waypoints,
            waypoint_names,
            speed_mps,
            wait_ms,
            mode,
            spin_axis: spin_axis.normalize_or_zero(),
            spin_accel_rad_s2,
            carry_yaw,
            segment_index: 0,
            direction_sign: 1,
            segment_elapsed_ms: 0.0,
            wait_remaining_ms: 0.0,
            current_linear_velocity: Vec3::ZERO,
            started,
            completed: false,
            target_segment: None,
            spin_angle_rad: 0.0,
            spin_rate_rad_s: initial_spin_rate_rad_s,
            spin_target_rate_rad_s: initial_spin_rate_rad_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Component, ComponentKind, ComponentValue};

    #[test]
    fn kinematic_mover_component_registers_kind_and_round_trips_serde() {
        let mover = KinematicMoverComponent::new(
            9,
            vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)],
            vec!["start".to_string(), "finish".to_string()],
            2.5,
            125.0,
            KinematicMoverMode::PingPong,
            true,
            Vec3::new(0.0, 2.0, 0.0),
            1.25,
            0.75,
            true,
        );
        let value = mover.clone().into_value();

        assert_eq!(value.kind(), ComponentKind::KinematicMover);
        assert_eq!(KinematicMoverComponent::KIND, ComponentKind::KinematicMover);

        let json = serde_json::to_value(&value).unwrap();
        let back: ComponentValue = serde_json::from_value(json).unwrap();
        assert_eq!(back, value);
        assert_eq!(mover.spin_axis, Vec3::Y);
        assert_eq!(mover.spin_rate_rad_s, 1.25);
        assert_eq!(mover.spin_target_rate_rad_s, 1.25);
    }

    #[test]
    fn set_spin_rate_command_uses_snake_case_degrees_payload() {
        let command = MoverCommand::SetSpinRate(-90.0);

        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            serde_json::json!({ "set_spin_rate": -90.0 })
        );
        assert_eq!(
            serde_json::from_value::<MoverCommand>(serde_json::json!({
                "set_spin_rate": 180.0
            }))
            .unwrap(),
            MoverCommand::SetSpinRate(180.0)
        );
    }
}
