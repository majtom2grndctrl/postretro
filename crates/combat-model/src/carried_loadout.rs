// Carried-seat and tuning-payload types shared by gameplay and netcode.
// See: context/lib/networking.md

use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry};
use postretro_foundation::{
    KnockbackDescriptor, PlayerMovementDescriptor, ProjectileDescriptor, ResolutionMode,
    SplashDescriptor, WeaponActivationDescriptor, WeaponDescriptor, WeaponPlacementDescriptor,
    WeaponResource,
};
use serde::{Deserialize, Serialize};

use crate::FluidMovementTable;

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
/// of the bitcode wire version because the payload itself is JSON. 12: the
/// movement descriptor gains `vault` tuning, and `swim.fluid_jump_velocity`
/// becomes nullable (an epoch-11 reader rejects the null). 13: a weapon
/// action's `charge` gains `interruptedBy`, which the client's vault fire gate
/// reads (an epoch-12 reader rejects the unknown field). 14: the payload
/// gains `spawn_active_slot`, the slot the host composed the pawn active with,
/// which the client's install composition selects (an epoch-13 reader rejects
/// the unknown field).
pub const TUNING_PAYLOAD_EPOCH: u32 = 14;

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
    pub primary: WeaponActivationDescriptor,
    pub secondary: Option<WeaponActivationDescriptor>,
    pub damage: f32,
    pub knockback: Option<KnockbackDescriptor>,
    pub projectile: Option<ProjectileDescriptor>,
    pub splash: Option<SplashDescriptor>,
    pub resource: Option<WeaponResource>,
    pub pellet_count: u32,
    pub spread_degrees: f32,
    pub bloom_per_shot_degrees: f32,
    pub bloom_max_degrees: f32,
    pub bloom_decay_degrees_per_second: f32,
    pub bloom_decay_delay_ms: f32,
    pub movement_spread_degrees: f32,
    pub spread_vertical_bias: f32,
    pub resolution: ResolutionMode,
    pub lower_ms: u32,
    pub raise_ms: u32,
    pub block_during_reload: Option<bool>,
}

impl WieldableTuningPayload {
    pub fn resource_from_weapon(
        weapon: &postretro_entities::components::weapon::WeaponComponent,
    ) -> Option<WeaponResource> {
        if let Some(ammo) = &weapon.ammo {
            Some(WeaponResource::Ammo(postretro_foundation::AmmoResource {
                ammo_type: ammo.ammo_type.clone(),
                magazine: ammo.capacity,
                cost_per_shot: ammo.cost_per_shot,
                reserve: 0,
                reload_ms: ammo.reload_ms,
                reload_style: ammo.reload_style,
            }))
        } else if let Some(heat) = &weapon.heat {
            Some(WeaponResource::Heat(heat.tuning))
        } else {
            weapon.cell.map(|cell| WeaponResource::Cell(cell.tuning))
        }
    }
    /// Project host-owned gameplay bases and expressions into a validated descriptor.
    pub fn weapon_descriptor(&self) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: self.damage,
            knockback: self.knockback,
            range: self.range,
            primary: self.primary.clone(),
            secondary: self.secondary.clone(),
            pellet_count: self.pellet_count,
            spread_degrees: self.spread_degrees,
            bloom_per_shot_degrees: self.bloom_per_shot_degrees,
            bloom_max_degrees: self.bloom_max_degrees,
            bloom_decay_degrees_per_second: self.bloom_decay_degrees_per_second,
            bloom_decay_delay_ms: self.bloom_decay_delay_ms,
            movement_spread_degrees: self.movement_spread_degrees,
            spread_vertical_bias: self.spread_vertical_bias,
            resolution: self.resolution,
            projectile: self.projectile.clone(),
            splash: self.splash.clone(),
            resource: self.resource.clone(),
            lower_ms: self.lower_ms,
            raise_ms: self.raise_ms,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            sounds: None,
            placement: None,
            muzzle_offset: self.muzzle_offset,
            block_during_reload: self.block_during_reload,
        }
    }

    pub fn validate(&self) -> Result<(), postretro_foundation::DescriptorError> {
        self.placement.validate()?;
        self.weapon_descriptor().validate().map(|_| ())
    }
}

/// Host-resolved tuning for one participating pawn.
///
/// Movement is optional for pawn classes without a movement descriptor. The
/// wieldable array is capacity-sized so a slot's identity survives empty
/// positions. `movement.view_feel` and `movement.sounds` are always cleared
/// because they are local presentation rather than predicted simulation tuning;
/// this tuning payload omits sound keys. Observer cues carry frozen effective
/// sound keys separately. `environment` is the host's level state the client
/// predicts against ([`EnvironmentTuning`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningPayload {
    epoch: u32,
    pub movement: Option<PlayerMovementDescriptor>,
    pub wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
    /// The wieldable slot the host composed this pawn's inventory active with
    /// (a carried selection after a level change or rejoin). Fixed at spawn and
    /// never tracking a later switch, so the host's whole-struct dedupe never
    /// resends tuning for one. A client composing its own inventory from this
    /// payload starts on it; one naming no populated slot falls back to the
    /// first populated slot. Not pawn tuning: [`Self::same_pawn_tuning`]
    /// ignores it.
    pub spawn_active_slot: usize,
    pub environment: EnvironmentTuning,
}

/// Host level state a connected client resolves environment volumes against.
///
/// It rides every pawn's payload because the client never reads its own copy:
/// prediction and reconcile replay build their environment view from the
/// installed value. Whole-struct `PartialEq` dedupe on the host resends the
/// payload whenever a field here changes. Level-scoped, not pawn-scoped, so a
/// change here alone never rebuilds the client's pawn tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentTuning {
    /// The level default gravity, signed vertical m/s² (worldspawn
    /// `initialGravity`, then `setGravity`). Never a volume's gravity.
    pub level_gravity: f32,
    /// The host's fluid table, movement fields only: a connected client
    /// resolves level fluid names against this, never its own registry.
    pub fluids: FluidMovementTable,
}

impl TuningPayload {
    pub fn new(
        mut movement: Option<PlayerMovementDescriptor>,
        mut wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
        environment: EnvironmentTuning,
    ) -> Self {
        if let Some(descriptor) = movement.as_mut() {
            descriptor.view_feel = None;
            descriptor.sounds = None;
        }
        for weapon in wieldables.iter_mut().flatten() {
            weapon.primary.sounds = None;
            if let Some(secondary) = &mut weapon.secondary {
                secondary.sounds = None;
            }
        }
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            wieldables,
            spawn_active_slot: 0,
            environment,
        }
    }

    /// Name the slot the host composed this pawn active with. See
    /// [`Self::spawn_active_slot`].
    pub fn with_spawn_active_slot(mut self, slot: usize) -> Self {
        self.spawn_active_slot = slot;
        self
    }

    /// Whether `other` carries the same pawn-scoped tuning (movement and
    /// wieldables), whatever its environment. A client keeps its live
    /// descriptor-derived pawn state across an environment-only change.
    pub fn same_pawn_tuning(&self, other: &Self) -> bool {
        self.movement == other.movement && self.wieldables == other.wieldables
    }

    /// Construct the only intentionally non-canonical payload shape used by
    /// engine codec tests. The codec must clear this input's local view feel.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn new_for_test_preserving_view_feel(
        movement: Option<PlayerMovementDescriptor>,
        wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY],
        environment: EnvironmentTuning,
    ) -> Self {
        Self {
            epoch: TUNING_PAYLOAD_EPOCH,
            movement,
            wieldables,
            spawn_active_slot: 0,
            environment,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::FluidMovementTuning;

    fn payload_with(level_gravity: f32, fluids: &[(&str, f32)]) -> TuningPayload {
        let fluids = FluidMovementTable::new(
            fluids
                .iter()
                .map(|&(name, viscosity)| FluidMovementTuning {
                    name: name.to_string(),
                    viscosity,
                })
                .collect(),
        )
        .expect("valid fixture table");
        TuningPayload::new(
            None,
            std::array::from_fn(|_| None),
            EnvironmentTuning {
                level_gravity,
                fluids,
            },
        )
    }

    // The host resends on whole-struct inequality, so a level-gravity or
    // fluid-table change alone must read as a new payload while leaving pawn
    // tuning untouched: the client applies it without rebuilding its pawn.
    #[test]
    fn environment_change_is_a_new_payload_with_the_same_pawn_tuning() {
        let water = [("water", 2.0)];
        let before = payload_with(-9.81, &water);
        for after in [
            payload_with(-3.0, &water),
            payload_with(-9.81, &[("water", 2.0), ("lava", 9.0)]),
            payload_with(-9.81, &[("water", 4.0)]),
            payload_with(-9.81, &[]),
        ] {
            assert_ne!(before, after, "host dedupe must resend the change");
            assert!(before.same_pawn_tuning(&after));
        }
        assert_eq!(before, payload_with(-9.81, &water));
    }
}
