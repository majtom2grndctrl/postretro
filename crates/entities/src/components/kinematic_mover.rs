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
}

/// Live deterministic phase for one linear moving-world payload.
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
    pub segment_index: u16,
    pub direction_sign: i8,
    pub segment_elapsed_ms: f32,
    pub wait_remaining_ms: f32,
    pub current_linear_velocity: Vec3,
    pub started: bool,
    pub completed: bool,
    /// Runtime target waypoint index for `go_to_path_node`; replicated as phase.
    pub target_segment: Option<u16>,
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
    ) -> Self {
        Self {
            mover_id,
            waypoints,
            waypoint_names,
            speed_mps,
            wait_ms,
            mode,
            segment_index: 0,
            direction_sign: 1,
            segment_elapsed_ms: 0.0,
            wait_remaining_ms: 0.0,
            current_linear_velocity: Vec3::ZERO,
            started,
            completed: false,
            target_segment: None,
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
        );
        let value = mover.clone().into_value();

        assert_eq!(value.kind(), ComponentKind::KinematicMover);
        assert_eq!(KinematicMoverComponent::KIND, ComponentKind::KinematicMover);

        let json = serde_json::to_value(&value).unwrap();
        let back: ComponentValue = serde_json::from_value(json).unwrap();
        assert_eq!(back, value);
    }
}
