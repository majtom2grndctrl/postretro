//! Physical player occupancy per trigger volume: the set the trigger stage
//! keeps across ticks, unfiltered by arm, latch or life state.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §5 (Occupancy)

use std::collections::{BTreeMap, BTreeSet, HashSet};

use postretro_entities::EntityId;

use crate::trigger_system::PlayerId;

/// Players overlapping each trigger. Sorted keys keep edge emission stable.
#[derive(Debug, Default)]
pub(crate) struct TriggerOccupants {
    by_trigger: BTreeMap<EntityId, BTreeSet<PlayerId>>,
}

impl TriggerOccupants {
    pub(crate) fn clear(&mut self) {
        self.by_trigger.clear();
    }

    /// Number of players overlapping `trigger`.
    pub(crate) fn count(&self, trigger: EntityId) -> usize {
        self.by_trigger.get(&trigger).map_or(0, BTreeSet::len)
    }

    /// Drop every trigger outside `active`. A removed trigger keeps no members.
    pub(crate) fn retain_triggers(&mut self, active: &BTreeSet<EntityId>) {
        self.by_trigger
            .retain(|trigger, _| active.contains(trigger));
    }

    /// Remove and return, in player order, the members of `trigger` that
    /// `still_inside` rejects. The set is retained in place: only leaving
    /// players need temporary storage.
    pub(crate) fn take_leaving(
        &mut self,
        trigger: EntityId,
        mut still_inside: impl FnMut(PlayerId) -> bool,
    ) -> Vec<PlayerId> {
        let members = self.by_trigger.entry(trigger).or_default();
        let leaving: Vec<PlayerId> = members
            .iter()
            .copied()
            .filter(|&player| !still_inside(player))
            .collect();
        for player in &leaving {
            members.remove(player);
        }
        leaving
    }

    /// Record `player` inside `trigger`. True when it was not already a member:
    /// the physical Enter.
    pub(crate) fn enter(&mut self, trigger: EntityId, player: PlayerId) -> bool {
        self.by_trigger.entry(trigger).or_default().insert(player)
    }

    /// Each volume with at least one member the effective occupancy rule
    /// counts, in volume order, with those members' pawns. Armed and latched
    /// state play no part: membership is physical.
    pub(crate) fn counted_volumes(
        &self,
        counted: &HashSet<PlayerId>,
        pawns: &std::collections::BTreeMap<PlayerId, EntityId>,
    ) -> Vec<(EntityId, Vec<EntityId>)> {
        self.by_trigger
            .iter()
            .filter_map(|(&trigger, members)| {
                let inside: Vec<EntityId> = members
                    .iter()
                    .filter(|player| counted.contains(player))
                    .filter_map(|player| pawns.get(player).copied())
                    .collect();
                (!inside.is_empty()).then_some((trigger, inside))
            })
            .collect()
    }

    /// Members of `trigger` the effective occupancy rule counts, in player order.
    pub(crate) fn effective(
        &self,
        trigger: EntityId,
        counted: &HashSet<PlayerId>,
    ) -> Vec<PlayerId> {
        self.by_trigger
            .get(&trigger)
            .map_or_else(Vec::new, |members| {
                members
                    .iter()
                    .filter(|player| counted.contains(player))
                    .copied()
                    .collect()
            })
    }
}
