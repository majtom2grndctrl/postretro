// Owner-private heat and cell slots: each owner's own active weapon lowered to
// slot-correlated samples on the host, and recorded beside that slot on the client.
// See: context/lib/networking.md §Combat authority: FIRE vs HIT

use postretro_entities::SlotValue;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_net::state_slots::WireSlotValue;
use postretro_scripting_core::player_slots::{PlayerSlot, ReloadRead, weapon_slot_value};

use super::{ReplicatedWireShape, absent_wieldable_slot_sample, wieldable_slot_sample};
use crate::weapon::{ReplicatedWeaponProjection, SlotSample};

pub(super) const HEAT_SLOT: &str = "player.heat";
pub(super) const OVERHEAT_AT_SLOT: &str = "player.overheatAt";
pub(super) const OVERHEATED_SLOT: &str = "player.overheated";
pub(super) const CELL_SLOT: &str = "player.cell";
pub(super) const CELL_CAPACITY_SLOT: &str = "player.cellCapacity";

/// The wire shape of a heat or cell slot; `None` for any other name. The
/// numbers are optional so a weapon of another kind travels as `[slot]`; the
/// latch is a plain flag, false for any weapon without heat.
pub(super) fn wire_shape(name: &str) -> Option<ReplicatedWireShape> {
    match name {
        HEAT_SLOT | OVERHEAT_AT_SLOT | CELL_SLOT | CELL_CAPACITY_SLOT => {
            Some(ReplicatedWireShape::WieldableSlotOptionalNumber)
        }
        OVERHEATED_SLOT => Some(ReplicatedWireShape::WieldableSlotBoolean),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HeatValues {
    heat: f32,
    overheat_at: f32,
    overheated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CellValues {
    charge: f32,
    capacity: f32,
}

/// One owner's heat and cell values, read from that pawn's own active weapon.
/// It mirrors what the host HUD publishes for its own pawn, and never reads
/// the slot table, whose values are the host player's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct ResourceSlotProjection {
    /// The host wieldable slot the values describe. `None` with no live
    /// active weapon: nothing is sent, and the client keeps its values.
    wieldable_slot: Option<usize>,
    heat: Option<HeatValues>,
    cell: Option<CellValues>,
}

impl ResourceSlotProjection {
    /// `weapon` is the live weapon in `wieldable_slot`, the pawn's active slot.
    pub(super) fn of(wieldable_slot: Option<usize>, weapon: Option<&WeaponComponent>) -> Self {
        let (Some(slot), Some(weapon)) = (wieldable_slot, weapon) else {
            return Self::default();
        };
        let number = |slot| match weapon_slot_value(weapon, None, slot, ReloadRead::Current) {
            Some(SlotValue::Number(value)) => Some(value),
            _ => None,
        };
        let overheated = matches!(
            weapon_slot_value(weapon, None, PlayerSlot::Overheated, ReloadRead::Current),
            Some(SlotValue::Boolean(true))
        );
        Self {
            wieldable_slot: Some(slot),
            heat: number(PlayerSlot::Heat)
                .zip(number(PlayerSlot::OverheatAt))
                .map(|(heat, overheat_at)| HeatValues {
                    heat,
                    overheat_at,
                    overheated,
                }),
            cell: number(PlayerSlot::Cell)
                .zip(number(PlayerSlot::CellCapacity))
                .map(|(charge, capacity)| CellValues { charge, capacity }),
        }
    }

    /// The slot-correlated sample for `name`. The outer option names the slots
    /// this projection owns; the inner one is `None` when nothing is sent. A
    /// live weapon of another kind is an authoritative absence: `[slot]` for
    /// that kind's numbers and `[slot, 0]` for the latch.
    pub(super) fn wire_sample(&self, name: &str) -> Option<Option<WireSlotValue>> {
        wire_shape(name)?;
        let Some(slot) = self.wieldable_slot else {
            return Some(None);
        };
        let number = |value: Option<f32>| match value {
            Some(value) => wieldable_slot_sample(slot, &SlotValue::Number(value)),
            None => Some(absent_wieldable_slot_sample(slot)),
        };
        Some(match name {
            HEAT_SLOT => number(self.heat.map(|heat| heat.heat)),
            OVERHEAT_AT_SLOT => number(self.heat.map(|heat| heat.overheat_at)),
            OVERHEATED_SLOT => wieldable_slot_sample(
                slot,
                &SlotValue::Boolean(self.heat.is_some_and(|heat| heat.overheated)),
            ),
            CELL_SLOT => number(self.cell.map(|cell| cell.charge)),
            CELL_CAPACITY_SLOT => number(self.cell.map(|cell| cell.capacity)),
            _ => None,
        })
    }
}

/// Record a committed heat or cell value in the client's weapon projection.
/// `value` is `None` for a committed absence. Returns whether `name` is a
/// heat or cell slot.
pub(super) fn record_resource_sample(
    projection: &mut ReplicatedWeaponProjection,
    name: &str,
    slot: usize,
    value: Option<&SlotValue>,
) -> bool {
    let number = match value {
        Some(SlotValue::Number(value)) => Some(Some(*value)),
        None => Some(None),
        Some(_) => None,
    };
    let number_sample = number.map(|value| SlotSample { slot, value });
    match name {
        HEAT_SLOT => projection.heat = number_sample.or(projection.heat),
        OVERHEAT_AT_SLOT => projection.overheat_at = number_sample.or(projection.overheat_at),
        CELL_SLOT => projection.cell = number_sample.or(projection.cell),
        CELL_CAPACITY_SLOT => {
            projection.cell_capacity = number_sample.or(projection.cell_capacity);
        }
        OVERHEATED_SLOT => {
            if let Some(SlotValue::Boolean(value)) = value {
                projection.overheated = Some(SlotSample {
                    slot,
                    value: *value,
                });
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests;
