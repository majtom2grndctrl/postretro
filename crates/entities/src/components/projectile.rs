// Straight-line projectile flight and deferred-impact state.
// See: context/lib/entity_model.md §5, §7

use serde::{Deserialize, Serialize};

use postretro_foundation::ProjectileImpactLight;

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
    /// Fixed-tick flight time used by a cadence-enabled sprite body. Bodies
    /// without cadence leave this at exactly zero so their packed instance stays
    /// byte-identical to the static billboard path.
    #[serde(default)]
    pub elapsed_flight_age: f32,
    /// Resolved once from the descriptor at spawn. The render collector must not
    /// infer animation from collection frame count: a multi-frame directory is
    /// still static until its descriptor authors a cadence.
    #[serde(default)]
    pub flipbook_active: bool,
    /// Descriptor-resolved impact presentation retained for the flight's
    /// contact branch. It is gameplay-local state and never materialized from
    /// replication, so a projectile can flash after its owner weapon despawns.
    #[serde(default)]
    pub impact_light: Option<ProjectileImpactLight>,
}

/// Presentation-only timing for a projectile replicated as a visual entity.
///
/// This deliberately lives in [`EntityRegistry`](crate::registry::EntityRegistry)'s
/// non-replicated side data instead of `ComponentKind`: the shared descriptor
/// determines cadence, while the local presentation clock determines elapsed age.
/// Adding it to the replicated component vocabulary would change the wire format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectilePresentationAge {
    pub spawn_time: f32,
    pub flipbook_active: bool,
}
