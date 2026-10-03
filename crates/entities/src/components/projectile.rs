// Straight-line projectile flight and deferred-impact state.
// See: context/lib/entity_model.md §5, §7

use serde::{Deserialize, Serialize};

use postretro_foundation::{ProjectileImpactLight, SplashDescriptor};

use crate::registry::EntityId;

/// Engine-owned state for one projectile's resolved impact damage.
///
/// The spawn path validates the descriptor before constructing this component;
/// retaining all hit-time inputs here lets a projectile outlive its firing pawn
/// without re-resolving mutable weapon tuning or attribution data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileComponent {
    #[serde(default)]
    pub source_sounds: Option<std::sync::Arc<postretro_foundation::ActivationSounds>>,
    #[serde(default = "default_predicted_visible")]
    pub predicted_visible: bool,
    #[serde(default)]
    pub source_action: Option<std::sync::Arc<postretro_foundation::WeaponActivationDescriptor>>,
    #[serde(default)]
    pub source_shot: Option<postretro_foundation::ShotId>,
    /// Unit direction of travel, stored as an array for compact serde parity
    /// with the other gameplay components.
    pub direction: [f32; 3],
    pub speed: f32,
    pub radius: f32,
    pub remaining_range: f32,
    pub remaining_lifetime: f32,
    pub damage: f32,
    /// Fire-time resolved direct-hit velocity change, independent of damage.
    #[serde(default)]
    pub knockback_impulse: [f32; 3],
    pub credit_source: String,
    pub owner_pawn: EntityId,
    pub owner_weapon: EntityId,
    /// The spawn pass clears this without integrating, ensuring a projectile
    /// cannot impact on its fire tick.
    pub spawned: bool,
    /// Connected-client declaration authority. Zero-valued identity fields are valid:
    /// network and client tick allocation both begin at zero. Local standalone projectiles
    /// use `None`; this distinction never crosses the network wire.
    #[serde(default)]
    pub predicted_shot_id: Option<postretro_foundation::ShotId>,
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
    /// Descriptor-resolved splash tuning retained at spawn. The authoritative
    /// impact path uses this snapshot rather than consulting the live weapon.
    #[serde(default)]
    pub splash: Option<SplashDescriptor>,
    /// Canonical name of the weapon descriptor this projectile was fired from.
    /// Its contact presentation resolves from the projectile, never through
    /// the shooter, so an enemy's projectile sounds like its weapon.
    #[serde(default)]
    pub source_weapon: Option<String>,
    /// The first projectile of this one's activation. Contacts that share it on
    /// one tick are one impact; `None` makes this projectile its own
    /// activation's key, so its impacts group by its own id.
    #[serde(default)]
    pub activation: Option<EntityId>,
}

fn default_predicted_visible() -> bool {
    true
}

/// Presentation-only timing for a projectile replicated as a visual entity.
///
/// This deliberately lives in [`EntityRegistry`](crate::registry::EntityRegistry)'s
/// non-replicated side data instead of `ComponentKind`: the shared descriptor
/// determines cadence, while the replicated fixed-tick epoch determines elapsed age.
/// Adding it to the replicated component vocabulary would change the wire format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectilePresentationAge {
    /// Host fixed tick at which this presentation first became authoritative.
    /// Observer materialization keeps this stamp rather than substituting local
    /// snapshot-arrival or render-frame time.
    pub spawn_tick: u32,
    pub flipbook_active: bool,
}
