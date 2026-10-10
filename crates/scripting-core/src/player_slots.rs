// Per-pawn values of the owner-private engine player slots.
// See: context/lib/scripting.md §5 (Per-player engine slots)

// One lookup from pawn to value, read straight from the pawn's components.
// Owner-private replication, the host HUD, player-event condition reads and
// `byPlayer` reads all call it, so no per-player copy can drift from the
// components it describes. Wire shaping stays with replication.

use crate::components::ammo_reserve::AmmoReserve;
use crate::components::health::HealthComponent;
use crate::components::inventory::Inventory;
use crate::components::weapon::{ReloadFeedbackConsumer, WeaponComponent};
use crate::ir::IrType;
use crate::registry::{EntityId, EntityRegistry};
use crate::slot_table::SlotValue;

/// An owner-private engine player slot. The engine-state catalog marks
/// exactly these (`ReplicationScope::OwnerPrivatePlayer`); a catalog test
/// keeps the two in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerSlot {
    Health,
    MaxHealth,
    Ammo,
    AmmoReserve,
    Heat,
    OverheatAt,
    Overheated,
    Cell,
    CellCapacity,
    ReloadActive,
    ReloadProgress,
    WeaponCooldownMs,
}

impl PlayerSlot {
    pub const ALL: &'static [PlayerSlot] = &[
        PlayerSlot::Health,
        PlayerSlot::MaxHealth,
        PlayerSlot::Ammo,
        PlayerSlot::AmmoReserve,
        PlayerSlot::Heat,
        PlayerSlot::OverheatAt,
        PlayerSlot::Overheated,
        PlayerSlot::Cell,
        PlayerSlot::CellCapacity,
        PlayerSlot::ReloadActive,
        PlayerSlot::ReloadProgress,
        PlayerSlot::WeaponCooldownMs,
    ];

    /// The stable dotted wire name.
    pub const fn name(self) -> &'static str {
        match self {
            PlayerSlot::Health => "player.health",
            PlayerSlot::MaxHealth => "player.maxHealth",
            PlayerSlot::Ammo => "player.ammo",
            PlayerSlot::AmmoReserve => "player.ammoReserve",
            PlayerSlot::Heat => "player.heat",
            PlayerSlot::OverheatAt => "player.overheatAt",
            PlayerSlot::Overheated => "player.overheated",
            PlayerSlot::Cell => "player.cell",
            PlayerSlot::CellCapacity => "player.cellCapacity",
            PlayerSlot::ReloadActive => "player.reloadActive",
            PlayerSlot::ReloadProgress => "player.reloadProgress",
            PlayerSlot::WeaponCooldownMs => "player.weaponCooldownMs",
        }
    }

    /// The IR type the slot projects to.
    pub const fn ir_type(self) -> IrType {
        match self {
            PlayerSlot::Overheated | PlayerSlot::ReloadActive => IrType::Bool,
            PlayerSlot::Health
            | PlayerSlot::MaxHealth
            | PlayerSlot::Ammo
            | PlayerSlot::AmmoReserve
            | PlayerSlot::Heat
            | PlayerSlot::OverheatAt
            | PlayerSlot::Cell
            | PlayerSlot::CellCapacity
            | PlayerSlot::ReloadProgress
            | PlayerSlot::WeaponCooldownMs => IrType::Number,
        }
    }

    /// This slot's position in [`Self::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|slot| slot.name() == name)
    }
}

/// How a reload slot is sampled. The other slots read the same either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadRead {
    /// The weapon's state right now: per-tick readers.
    Current,
    /// One consumer's endpoint stream, so a reload shorter than that
    /// consumer's cadence still shows. The HUD and owner-private projection
    /// each keep their own cursor.
    Feedback(ReloadFeedbackConsumer),
}

/// The pawn's live active weapon: the wieldable slot it sits in and its
/// component.
#[derive(Debug, Clone, Copy)]
pub struct ActiveWeapon<'a> {
    pub slot: usize,
    pub entity: EntityId,
    pub weapon: &'a WeaponComponent,
}

/// The weapon in `pawn`'s active wieldable slot, when it is live.
pub fn active_weapon(registry: &EntityRegistry, pawn: EntityId) -> Option<ActiveWeapon<'_>> {
    if !registry.exists(pawn) {
        return None;
    }
    let inventory = registry.get_component::<Inventory>(pawn).ok()?;
    let entity = inventory.active_wieldable()?;
    let weapon = registry.get_component::<WeaponComponent>(entity).ok()?;
    Some(ActiveWeapon {
        slot: inventory.active_slot,
        entity,
        weapon,
    })
}

/// The value `slot` holds for `pawn` right now, or `None` when the pawn has no
/// source for it. See [`player_slot_value_with`].
pub fn player_slot_value(
    registry: &EntityRegistry,
    slot: PlayerSlot,
    pawn: EntityId,
) -> Option<SlotValue> {
    player_slot_value_with(registry, slot, pawn, ReloadRead::Current)
}

/// The value `slot` holds for `pawn`, or `None` when the pawn has no source
/// for it. Absence is never filled with a default — a missing value must not
/// read as another player's, or as zero:
///
/// - health slots need a `HealthComponent`;
/// - every weapon slot needs a live weapon in the active wieldable slot;
/// - ammo and reserve need an ammo weapon, heat numbers a heat weapon, cell
///   numbers a cell weapon. `overheated` is false for any other weapon;
/// - a non-finite cooldown is absent.
pub fn player_slot_value_with(
    registry: &EntityRegistry,
    slot: PlayerSlot,
    pawn: EntityId,
    reload: ReloadRead,
) -> Option<SlotValue> {
    if let PlayerSlot::Health | PlayerSlot::MaxHealth = slot {
        let health = registry.get_component::<HealthComponent>(pawn).ok()?;
        let value = if slot == PlayerSlot::Health {
            health.current
        } else {
            health.max
        };
        return Some(SlotValue::Number(value));
    }

    let ActiveWeapon { weapon, .. } = active_weapon(registry, pawn)?;
    let reserve = registry.get_component::<AmmoReserve>(pawn).ok();
    weapon_slot_value(weapon, reserve, slot, reload)
}

/// The value a weapon slot holds for the pawn wielding `weapon` as its active
/// weapon, with that pawn's `reserve`. `None` for the health slots and for a
/// weapon slot this weapon has no source for (see [`player_slot_value_with`]).
pub fn weapon_slot_value(
    weapon: &WeaponComponent,
    reserve: Option<&AmmoReserve>,
    slot: PlayerSlot,
    reload: ReloadRead,
) -> Option<SlotValue> {
    let number = |value: f32| Some(SlotValue::Number(value));
    match slot {
        PlayerSlot::Health | PlayerSlot::MaxHealth => None,
        PlayerSlot::Ammo => weapon
            .effective()
            .ammo
            .map(|_| SlotValue::Number(weapon.magazine as f32)),
        PlayerSlot::AmmoReserve => {
            let ammo = weapon.effective().ammo?;
            number(reserve.map_or(0, |reserve| reserve.available(ammo.ammo_type)) as f32)
        }
        PlayerSlot::Heat => number(weapon.heat.as_ref()?.heat),
        PlayerSlot::OverheatAt => number(weapon.heat.as_ref()?.effective().overheat_at),
        PlayerSlot::Overheated => Some(SlotValue::Boolean(
            weapon.heat.as_ref().is_some_and(|heat| heat.overheated),
        )),
        PlayerSlot::Cell => number(weapon.cell.as_ref()?.charge),
        PlayerSlot::CellCapacity => number(weapon.cell.as_ref()?.effective().capacity),
        PlayerSlot::ReloadActive | PlayerSlot::ReloadProgress => {
            let (progress, active) = match reload {
                ReloadRead::Current => weapon.reload_state(),
                ReloadRead::Feedback(consumer) => {
                    let sample = weapon.reload_feedback_sample(consumer);
                    (sample.progress, sample.active)
                }
            };
            Some(if slot == PlayerSlot::ReloadActive {
                SlotValue::Boolean(active)
            } else {
                SlotValue::Number(progress)
            })
        }
        PlayerSlot::WeaponCooldownMs => weapon
            .cooldown_remaining_ms
            .is_finite()
            .then_some(SlotValue::Number(weapon.cooldown_remaining_ms)),
    }
}
