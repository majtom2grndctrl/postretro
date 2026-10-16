// Wire-mirrored pawn relocation identity and shared teleport yaw representation.
// See: context/lib/movement.md §6 (Teleport)

use serde::{Deserialize, Serialize};

pub const NO_TELEPORTER: u16 = u16::MAX;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeleportState {
    pub generation: u8,
    pub accumulated_yaw: f32,
    pub last_teleporter: u16,
    /// Sorted map-order indices; arrival can latch several overlapping regions.
    pub arrival_latches: Vec<u16>,
}

impl Default for TeleportState {
    fn default() -> Self {
        Self {
            generation: 0,
            accumulated_yaw: 0.0,
            last_teleporter: NO_TELEPORTER,
            arrival_latches: Vec::new(),
        }
    }
}

/// The stored representation is (-pi, pi], on every machine, before encoding.
pub fn wrap_teleport_yaw(yaw: f32) -> f32 {
    let yaw = yaw.rem_euclid(std::f32::consts::TAU);
    if yaw > std::f32::consts::PI {
        yaw - std::f32::consts::TAU
    } else {
        yaw
    }
}
