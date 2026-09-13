use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry};
use postretro_foundation::{
    FireMode, PlayerMovementDescriptor, ResolutionMode, WeaponPlacementDescriptor,
};
use serde::{Deserialize, Serialize};

/// State retained by a session seat when its current pawn leaves a level.
///
/// A missing record deliberately seeds no defaults on a fresh seat.
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedState {
    pub health_current: Option<f32>,
    pub reserve: AmmoReserve,
    pub wieldables: [Option<String>; WIELDABLE_SLOT_CAPACITY],
    pub magazines: [Option<u32>; WIELDABLE_SLOT_CAPACITY],
    pub active_slot: usize,
}

impl Default for CarriedState {
    fn default() -> Self {
        Self {
            health_current: None,
            reserve: AmmoReserve::new(),
            wieldables: std::array::from_fn(|_| None),
            magazines: [None; WIELDABLE_SLOT_CAPACITY],
            active_slot: 0,
        }
    }
}

/// Restore carried health after descriptor materialization.
///
/// Missing and nonpositive values keep the descriptor default so a fresh or
/// dead seat never materializes a dead pawn.
pub fn restore_carried_health(
    carried: Option<&CarriedState>,
    registry: &mut EntityRegistry,
    pawn: EntityId,
) {
    let Some(health_current) = carried
        .and_then(|state| state.health_current)
        .filter(|health| *health > 0.0)
    else {
        return;
    };
    postretro_entities::components::health::set_health_absolute(registry, pawn, health_current);
}

/// Bump whenever the payload's semantic contract changes. This is independent
/// of the bitcode wire version because the payload itself is JSON.
pub const TUNING_PAYLOAD_EPOCH: u32 = 9;

/// Host-resolved values for one occupied wieldable slot.
///
/// The archetype is part of the payload because a connected client owns local
/// wieldable instances. It needs the canonical identity to materialize each
/// slot and select its presentation without consulting a host-only entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WieldableTuningPayload {
    pub canonical_name: String,
    /// Effective host placement after mod-default and per-weapon resolution.
    pub placement: WeaponPlacementDescriptor,
    /// Host-authored model-local projectile origin. Clients pair this with
    /// `placement` from this same row rather than consulting local content.
    pub muzzle_offset: Option<[f32; 3]>,
    pub range: f32,
    pub cooldown_ms: f32,
    pub pellet_count: u32,
    pub spread_degrees: f32,
    pub bloom_per_shot_degrees: f32,
    pub bloom_max_degrees: f32,
    pub bloom_decay_degrees_per_second: f32,
    pub bloom_decay_delay_ms: f32,
    pub movement_spread_degrees: f32,
    pub spread_vertical_bias: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub lower_ms: u32,
    pub raise_ms: u32,
}

/// Host-resolved tuning for one participating pawn.
///
/// Movement is optional for pawn classes without a movement descriptor. The
/// wieldable array is capacity-sized so a slot's identity survives empty
/// positions. `movement.view_feel` is always cleared because view feel is local
/// presentation rather than predicted simulation tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningPayload {
    epoch: u32,
    pub movement: Option<PlayerMovementDescriptor>,
    pub wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
}

impl TuningPayload {
    pub fn new(
        mut movement: Option<PlayerMovementDescriptor>,
        wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
    ) -> Self {
        if let Some(descriptor) = movement.as_mut() {
            descriptor.view_feel = None;
        }
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            wieldables,
        }
    }

    /// Construct the only intentionally non-canonical payload shape used by
    /// engine codec tests. The codec must clear this input's local view feel.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn new_for_test_preserving_view_feel(
        movement: Option<PlayerMovementDescriptor>,
        wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
    ) -> Self {
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            wieldables,
        }
    }

    pub fn placement_for_slot(&self, slot: usize) -> Option<&WeaponPlacementDescriptor> {
        self.wieldables
            .get(slot)?
            .as_ref()
            .map(|wieldable| &wieldable.placement)
    }

    pub fn placement_for_archetype(&self, archetype: &str) -> Option<&WeaponPlacementDescriptor> {
        self.wieldables
            .iter()
            .flatten()
            .find(|wieldable| wieldable.canonical_name == archetype)
            .map(|wieldable| &wieldable.placement)
    }

    pub fn muzzle_for_slot(&self, slot: usize) -> Option<&[f32; 3]> {
        self.wieldables.get(slot)?.as_ref()?.muzzle_offset.as_ref()
    }
}
