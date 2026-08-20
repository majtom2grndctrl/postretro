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
    /// Reserved for the later connected-client declaration path.
    pub shot_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectile_component_serde_round_trip_preserves_impact_inputs() {
        let value = ProjectileComponent {
            direction: [0.0, 0.0, -1.0],
            speed: 40.0,
            radius: 0.2,
            remaining_range: 64.0,
            remaining_lifetime: 1.5,
            damage: 25.0,
            credit_source: "plasma.primary".to_string(),
            owner_pawn: EntityId::from_raw(4),
            owner_weapon: EntityId::from_raw(5),
            spawned: true,
            shot_id: 0,
        };

        let encoded = serde_json::to_string(&value).expect("component serializes");
        let decoded: ProjectileComponent =
            serde_json::from_str(&encoded).expect("component deserializes");
        assert_eq!(decoded, value);
    }
}
