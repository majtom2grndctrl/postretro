// Pawn-owned ordered wieldable instances and their committed switch state.
// See: context/lib/entity_model.md §2

use serde::{Deserialize, Serialize};

use crate::registry::EntityId;

/// Number of inventory positions addressable by the number-row bindings.
pub const WIELDABLE_SLOT_CAPACITY: usize = 10;

/// Ordered pawn-owned wieldable instances.
///
/// The active slot remains the outgoing instance until lowering completes. The
/// optional target records an accepted in-flight switch; input cursor and dwell
/// state intentionally do not belong in simulation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub wieldables: [Option<EntityId>; WIELDABLE_SLOT_CAPACITY],
    pub active_slot: usize,
    pub switch_target: Option<usize>,
    /// Slot active when the current local switch began. It lets a delayed host
    /// refusal restore the prior holder even after the local lower has repointed.
    /// The cursor and dwell remain input-only; this is switch lifecycle state.
    pub switch_origin: Option<usize>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            wieldables: [None; WIELDABLE_SLOT_CAPACITY],
            active_slot: 0,
            switch_target: None,
            switch_origin: None,
        }
    }
}

impl Inventory {
    pub const fn active_wieldable(&self) -> Option<EntityId> {
        if self.active_slot < WIELDABLE_SLOT_CAPACITY {
            self.wieldables[self.active_slot]
        } else {
            None
        }
    }

    pub const fn target_wieldable(&self) -> Option<EntityId> {
        match self.switch_target {
            Some(slot) if slot < WIELDABLE_SLOT_CAPACITY => self.wieldables[slot],
            None => None,
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_inventory_is_empty_and_has_no_switch_target() {
        let inventory = Inventory::default();
        assert_eq!(inventory.wieldables, [None; WIELDABLE_SLOT_CAPACITY]);
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(inventory.switch_origin, None);
    }

    #[test]
    fn malformed_slot_indices_degrade_to_no_wieldable() {
        let inventory = Inventory {
            active_slot: WIELDABLE_SLOT_CAPACITY,
            switch_target: Some(WIELDABLE_SLOT_CAPACITY),
            ..Inventory::default()
        };

        assert_eq!(inventory.active_wieldable(), None);
        assert_eq!(inventory.target_wieldable(), None);
    }
}
