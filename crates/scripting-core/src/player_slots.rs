// Per-pawn values of the owner-private engine player slots.
// See: context/lib/scripting.md §5 (Per-player engine slots)

// One lookup from pawn to value, read straight from the pawn's components.
// Owner-private replication, player-event condition reads and `byPlayer`
// reads all call it, so no stored per-player copy can drift from the
// components it describes.

use crate::components::health::HealthComponent;
use crate::ir::IrType;
use crate::registry::{EntityId, EntityRegistry};
use crate::slot_table::SlotValue;

/// An owner-private engine player slot with a per-pawn source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerSlot {
    Health,
    MaxHealth,
}

impl PlayerSlot {
    pub const ALL: &'static [PlayerSlot] = &[PlayerSlot::Health, PlayerSlot::MaxHealth];

    /// The stable dotted wire name.
    pub const fn name(self) -> &'static str {
        match self {
            PlayerSlot::Health => "player.health",
            PlayerSlot::MaxHealth => "player.maxHealth",
        }
    }

    /// The IR type the slot projects to.
    pub const fn ir_type(self) -> IrType {
        match self {
            PlayerSlot::Health | PlayerSlot::MaxHealth => IrType::Number,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|slot| slot.name() == name)
    }
}

/// The value `slot` holds for `pawn`, or `None` when the pawn has no source
/// for it. Absence is never filled with a default: a missing value must not
/// read as another player's, or as zero.
pub fn player_slot_value(
    registry: &EntityRegistry,
    slot: PlayerSlot,
    pawn: EntityId,
) -> Option<SlotValue> {
    match slot {
        PlayerSlot::Health | PlayerSlot::MaxHealth => {
            let health = registry.get_component::<HealthComponent>(pawn).ok()?;
            let value = if slot == PlayerSlot::Health {
                health.current
            } else {
                health.max
            };
            Some(SlotValue::Number(value))
        }
    }
}
