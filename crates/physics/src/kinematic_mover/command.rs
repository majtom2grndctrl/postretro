//! Deterministic application of the closed mover-command vocabulary.

use glam::Vec3;
use postretro_entities::{KinematicMoverComponent, MoverCommand};

use super::{mover_is_at_waypoint, path_coordinate, reanchor_direction};

/// Apply one command to mover phase without consulting a registry or clock.
pub fn apply_mover_command(mover: &mut KinematicMoverComponent, command: &MoverCommand) {
    match command {
        MoverCommand::Start => {
            mover.blocked = false;
            if mover.completed || (mover.started && mover.wait_remaining_ms <= 0.0) {
                return;
            }
            mover.started = true;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::Stop => {
            if !mover.started {
                return;
            }
            mover.started = false;
        }
        MoverCommand::Reverse => {
            mover.blocked = false;
            if mover.waypoints.len() < 2 {
                return;
            }
            reanchor_direction(mover, if mover.direction_sign >= 0 { -1 } else { 1 });
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::GoToPathNode(name) => {
            let mut matches = mover
                .waypoint_names
                .iter()
                .enumerate()
                .filter_map(|(index, waypoint_name)| (waypoint_name == name).then_some(index));
            let Some(target) = matches.next() else {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} references unknown waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            };
            if matches.next().is_some() || target > usize::from(u16::MAX) {
                log::warn!(
                    "[Mover] go_to_path_node for mover {} cannot uniquely resolve waypoint `{name}`; skipping",
                    mover.mover_id
                );
                return;
            }
            let target = target as u16;
            mover.blocked = false;
            if mover_is_at_waypoint(mover, target) {
                return;
            }

            let direction = if path_coordinate(mover)
                .map(|coordinate| f32::from(target) > coordinate)
                .unwrap_or(target > mover.segment_index)
            {
                1
            } else {
                -1
            };
            reanchor_direction(mover, direction);
            mover.target_segment = Some(target);
            mover.started = true;
            mover.completed = false;
            mover.wait_remaining_ms = 0.0;
        }
        MoverCommand::SetSpinRate(rate_deg_s) => {
            if !rate_deg_s.is_finite() {
                log::warn!(
                    "[Mover] set_spin_rate for mover {} has non-finite rate; skipping",
                    mover.mover_id
                );
                return;
            }
            if *rate_deg_s != 0.0
                && (!mover.spin_axis.is_finite()
                    || mover.spin_axis.normalize_or_zero() == Vec3::ZERO)
            {
                log::warn!(
                    "[Mover] set_spin_rate for mover {} requires a non-zero spin axis; skipping",
                    mover.mover_id
                );
                return;
            }
            mover.spin_target_rate_rad_s = rate_deg_s.to_radians();
        }
        MoverCommand::SetBlockPolicy(policy) => {
            mover.block_policy = *policy;
        }
    }
}
