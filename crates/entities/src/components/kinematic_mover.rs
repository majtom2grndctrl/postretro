// Deterministic kinematic mover state.
//
// Scripts address movers as `getMapEntities("mover")` member handles. Raw
// phase remains engine-owned and cannot be attached or mutated directly.

use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KinematicMoverMode {
    Once,
    PingPong,
}

/// Host-authoritative response when a kinematic mover contacts an entity.
///
/// The policy is static map authoring data. Clients reconcile only the resulting
/// phase (for example, a `blocked` stop hold) and never evaluate this choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockPolicy {
    #[default]
    Displace,
    Reverse,
    Stop,
    Crush,
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
    /// Set the host-authoritative response to entity contact.
    SetBlockPolicy(BlockPolicy),
}

/// Live deterministic phase for one moving-world payload.
///
/// Waypoints, speed, wait, spin axis, acceleration, and carry policy are static
/// data seeded when the mover is constructed. The remaining fields are phase
/// mirrored by the wire payload without replicating the path itself.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
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
    /// Static host-authoritative collision response, seeded from map authoring.
    pub block_policy: BlockPolicy,
    /// Static host-only damage amount dealt on each crusher hit.
    pub crush_damage: f32,
    /// Static host-only cadence between crusher hits; zero hits every tick.
    pub crush_interval_ms: f32,
    /// Static host-only automatic-close delay.
    pub auto_close_ms: f32,
    /// Optional host-local named-event address for reaching the open terminus.
    pub open_event: Option<String>,
    /// Optional host-local named-event address for reaching the closed terminus.
    pub close_event: Option<String>,
    /// Optional host-local named-event address for reactive block contact.
    pub blocked_event: Option<String>,
    /// Optional host-local named-event address dispatched when a crusher deals damage.
    pub crush_event: Option<String>,
    /// Optional presentation-only sound key played at the open terminus. Sound
    /// keys never replicate; see context/lib/audio.md §4.
    pub open_sound: Option<String>,
    /// Optional sound key played at the closed terminus.
    pub close_sound: Option<String>,
    /// Optional sound key played on reactive block contact.
    pub blocked_sound: Option<String>,
    /// Optional sound key played each time a crusher deals damage.
    pub crush_sound: Option<String>,
    /// Portal ids this mover fully seals at its closed dock. The renderer derives
    /// the live blocked set from this static association and current phase.
    pub sealed_portal_ids: Vec<u32>,
    pub segment_index: u16,
    pub direction_sign: i8,
    pub segment_elapsed_ms: f32,
    pub wait_remaining_ms: f32,
    pub current_linear_velocity: Vec3,
    pub started: bool,
    pub completed: bool,
    /// Replicated host-derived stop hold. No block policy or timer crosses the wire.
    pub blocked: bool,
    /// Runtime target waypoint index for `go_to_path_node`; replicated as phase.
    pub target_segment: Option<u16>,
    /// Replicated accumulated spin phase, wrapped by the deterministic driver.
    pub spin_angle_rad: f32,
    /// Replicated spin phase at the start of the most recently simulated tick.
    /// Together with `spin_angle_rad`, this reconstructs the exact rotation that
    /// tick applied even when argument reduction or phase wrapping occurred.
    pub spin_angle_before_tick_rad: f32,
    /// Whether the mover was active at the start of the most recently simulated
    /// tick. Commands may change `started`/`completed` after that tick; replay
    /// uses this provenance instead of the post-command gate.
    pub was_active_this_tick: bool,
    /// Replicated current spin rate after the driver's acceleration ramp.
    pub spin_rate_rad_s: f32,
    /// Replicated target spin rate set by mover commands.
    pub spin_target_rate_rad_s: f32,
}

// Hand-written so `clone_from` reuses the path, name and event buffers: client
// replay re-seeds a working phase from history every snapshot. Both methods
// destructure exhaustively, so a new field fails to compile until listed here.
impl Clone for KinematicMoverComponent {
    fn clone(&self) -> Self {
        let Self {
            mover_id,
            waypoints,
            waypoint_names,
            speed_mps,
            wait_ms,
            mode,
            spin_axis,
            spin_accel_rad_s2,
            carry_yaw,
            block_policy,
            crush_damage,
            crush_interval_ms,
            auto_close_ms,
            open_event,
            close_event,
            blocked_event,
            crush_event,
            open_sound,
            close_sound,
            blocked_sound,
            crush_sound,
            sealed_portal_ids,
            segment_index,
            direction_sign,
            segment_elapsed_ms,
            wait_remaining_ms,
            current_linear_velocity,
            started,
            completed,
            blocked,
            target_segment,
            spin_angle_rad,
            spin_angle_before_tick_rad,
            was_active_this_tick,
            spin_rate_rad_s,
            spin_target_rate_rad_s,
        } = self;
        Self {
            mover_id: *mover_id,
            waypoints: waypoints.clone(),
            waypoint_names: waypoint_names.clone(),
            speed_mps: *speed_mps,
            wait_ms: *wait_ms,
            mode: *mode,
            spin_axis: *spin_axis,
            spin_accel_rad_s2: *spin_accel_rad_s2,
            carry_yaw: *carry_yaw,
            block_policy: *block_policy,
            crush_damage: *crush_damage,
            crush_interval_ms: *crush_interval_ms,
            auto_close_ms: *auto_close_ms,
            open_event: open_event.clone(),
            close_event: close_event.clone(),
            blocked_event: blocked_event.clone(),
            crush_event: crush_event.clone(),
            open_sound: open_sound.clone(),
            close_sound: close_sound.clone(),
            blocked_sound: blocked_sound.clone(),
            crush_sound: crush_sound.clone(),
            sealed_portal_ids: sealed_portal_ids.clone(),
            segment_index: *segment_index,
            direction_sign: *direction_sign,
            segment_elapsed_ms: *segment_elapsed_ms,
            wait_remaining_ms: *wait_remaining_ms,
            current_linear_velocity: *current_linear_velocity,
            started: *started,
            completed: *completed,
            blocked: *blocked,
            target_segment: *target_segment,
            spin_angle_rad: *spin_angle_rad,
            spin_angle_before_tick_rad: *spin_angle_before_tick_rad,
            was_active_this_tick: *was_active_this_tick,
            spin_rate_rad_s: *spin_rate_rad_s,
            spin_target_rate_rad_s: *spin_target_rate_rad_s,
        }
    }

    fn clone_from(&mut self, source: &Self) {
        let Self {
            mover_id,
            waypoints,
            waypoint_names,
            speed_mps,
            wait_ms,
            mode,
            spin_axis,
            spin_accel_rad_s2,
            carry_yaw,
            block_policy,
            crush_damage,
            crush_interval_ms,
            auto_close_ms,
            open_event,
            close_event,
            blocked_event,
            crush_event,
            open_sound,
            close_sound,
            blocked_sound,
            crush_sound,
            sealed_portal_ids,
            segment_index,
            direction_sign,
            segment_elapsed_ms,
            wait_remaining_ms,
            current_linear_velocity,
            started,
            completed,
            blocked,
            target_segment,
            spin_angle_rad,
            spin_angle_before_tick_rad,
            was_active_this_tick,
            spin_rate_rad_s,
            spin_target_rate_rad_s,
        } = self;
        *mover_id = source.mover_id;
        waypoints.clone_from(&source.waypoints);
        waypoint_names.clone_from(&source.waypoint_names);
        *speed_mps = source.speed_mps;
        *wait_ms = source.wait_ms;
        *mode = source.mode;
        *spin_axis = source.spin_axis;
        *spin_accel_rad_s2 = source.spin_accel_rad_s2;
        *carry_yaw = source.carry_yaw;
        *block_policy = source.block_policy;
        *crush_damage = source.crush_damage;
        *crush_interval_ms = source.crush_interval_ms;
        *auto_close_ms = source.auto_close_ms;
        open_event.clone_from(&source.open_event);
        close_event.clone_from(&source.close_event);
        blocked_event.clone_from(&source.blocked_event);
        crush_event.clone_from(&source.crush_event);
        open_sound.clone_from(&source.open_sound);
        close_sound.clone_from(&source.close_sound);
        blocked_sound.clone_from(&source.blocked_sound);
        crush_sound.clone_from(&source.crush_sound);
        sealed_portal_ids.clone_from(&source.sealed_portal_ids);
        *segment_index = source.segment_index;
        *direction_sign = source.direction_sign;
        *segment_elapsed_ms = source.segment_elapsed_ms;
        *wait_remaining_ms = source.wait_remaining_ms;
        *current_linear_velocity = source.current_linear_velocity;
        *started = source.started;
        *completed = source.completed;
        *blocked = source.blocked;
        *target_segment = source.target_segment;
        *spin_angle_rad = source.spin_angle_rad;
        *spin_angle_before_tick_rad = source.spin_angle_before_tick_rad;
        *was_active_this_tick = source.was_active_this_tick;
        *spin_rate_rad_s = source.spin_rate_rad_s;
        *spin_target_rate_rad_s = source.spin_target_rate_rad_s;
    }
}

/// Static mover authoring data and its initial runtime phase.
///
/// Rates are expressed in radians, and the constructor normalizes `spin_axis`.
#[derive(Debug, Clone, PartialEq)]
pub struct KinematicMoverConfig {
    pub waypoints: Vec<Vec3>,
    pub waypoint_names: Vec<String>,
    pub speed_mps: f32,
    pub wait_ms: f32,
    pub mode: KinematicMoverMode,
    pub started: bool,
    pub spin_axis: Vec3,
    pub initial_spin_rate_rad_s: f32,
    pub spin_accel_rad_s2: f32,
    pub carry_yaw: bool,
}

impl KinematicMoverComponent {
    pub fn new(mover_id: u32, config: KinematicMoverConfig) -> Self {
        Self {
            mover_id,
            waypoints: config.waypoints,
            waypoint_names: config.waypoint_names,
            speed_mps: config.speed_mps,
            wait_ms: config.wait_ms,
            mode: config.mode,
            spin_axis: config.spin_axis.normalize_or_zero(),
            spin_accel_rad_s2: config.spin_accel_rad_s2,
            carry_yaw: config.carry_yaw,
            block_policy: BlockPolicy::Displace,
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: 0.0,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            open_sound: None,
            close_sound: None,
            blocked_sound: None,
            crush_sound: None,
            sealed_portal_ids: Vec::new(),
            segment_index: 0,
            direction_sign: 1,
            segment_elapsed_ms: 0.0,
            wait_remaining_ms: 0.0,
            current_linear_velocity: Vec3::ZERO,
            started: config.started,
            completed: false,
            blocked: false,
            target_segment: None,
            spin_angle_rad: 0.0,
            spin_angle_before_tick_rad: 0.0,
            was_active_this_tick: false,
            spin_rate_rad_s: config.initial_spin_rate_rad_s,
            spin_target_rate_rad_s: config.initial_spin_rate_rad_s,
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
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)],
                waypoint_names: vec!["start".to_string(), "finish".to_string()],
                speed_mps: 2.5,
                wait_ms: 125.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::new(0.0, 2.0, 0.0),
                initial_spin_rate_rad_s: 1.25,
                spin_accel_rad_s2: 0.75,
                carry_yaw: true,
            },
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
        assert_eq!(mover.block_policy, BlockPolicy::Displace);
        assert!(!mover.blocked);
        assert_eq!(serde_json::to_value(BlockPolicy::Stop).unwrap(), "stop");
    }

    #[test]
    fn mover_commands_use_snake_case_payloads() {
        let spin_rate = MoverCommand::SetSpinRate(-90.0);
        let block_policy = MoverCommand::SetBlockPolicy(BlockPolicy::Crush);

        assert_eq!(
            serde_json::to_value(&spin_rate).unwrap(),
            serde_json::json!({ "set_spin_rate": -90.0 })
        );
        assert_eq!(
            serde_json::from_value::<MoverCommand>(serde_json::json!({
                "set_spin_rate": 180.0
            }))
            .unwrap(),
            MoverCommand::SetSpinRate(180.0)
        );
        assert_eq!(
            serde_json::to_value(&block_policy).unwrap(),
            serde_json::json!({ "set_block_policy": "crush" })
        );
        assert_eq!(
            serde_json::from_value::<MoverCommand>(serde_json::json!({
                "set_block_policy": "reverse"
            }))
            .unwrap(),
            MoverCommand::SetBlockPolicy(BlockPolicy::Reverse)
        );
    }
}
