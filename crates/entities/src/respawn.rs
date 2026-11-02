//! Host-local respawn requests and installed spawn foot poses.
//! See: context/lib/scripting.md §12 and context/lib/networking.md (seat placement).
use crate::EntityId;
use glam::Vec3;
use postretro_foundation::Seat;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct RespawnRequest {
    pub pawn: EntityId,
    pub health: Option<f32>,
    /// Resolve at drain so a reload between enqueue and drain cannot shift ids.
    pub state: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct RespawnPlacement {
    pub map_index: usize,
    /// Map origin: derive the retained pawn's centre using its current capsule.
    pub foot_anchor: Vec3,
    pub yaw: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RespawnPlacements {
    /// Seat assignment stores the original map index, not the filtered Vec index.
    pub seats: BTreeMap<Seat, usize>,
    pub map_order: Vec<RespawnPlacement>,
    pub first_player: Option<usize>,
}
