// Straight-line projectile flight and deferred-impact state.
// See: context/lib/entity_model.md §5, §7

use serde::{Deserialize, Serialize};

use crate::registry::EntityId;

/// Engine-owned state for one direct-impact projectile.
///
/// The spawn path validates the descriptor before constructing this component;
/// retaining all hit-time inputs here lets a projectile outlive its firing pawn
/// without re-resolving mutable weapon tuning or attribution data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileComponent {
    /// Unit direction of travel, stored as an array for compact serde parity
    /// with the other gameplay components.
    pub direction: [f32; 3],
    pub speed: f32,
    pub radius: f32,
    pub remaining_range: f32,
    pub remaining_lifetime: f32,
    pub damage: f32,
    pub credit_source: String,
    pub owner_pawn: EntityId,
    pub owner_weapon: EntityId,
    /// The spawn pass clears this without integrating, ensuring a projectile
    /// cannot impact on its fire tick.
    pub spawned: bool,
    /// Connected-client declaration authority. `Some(0)` is valid: network and
    /// client tick allocation both begin at zero. Local standalone projectiles
    /// use `None`; this distinction never crosses the network wire.
    #[serde(default)]
    pub predicted_shot_id: Option<u64>,
}
