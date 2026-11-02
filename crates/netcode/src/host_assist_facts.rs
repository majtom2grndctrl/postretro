// Host build of each connected client's assist facts for its snapshot.
// See: context/lib/networking.md §Version gates · context/lib/scripting.md §12

use std::collections::HashMap;

use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::wire::{WireAssistFacts, WireAssistPair};
use postretro_scripting_core::slot_table::SlotTable;
use postretro_sim::sim::assist::AssistState;

use crate::{MovementOwners, NetworkIdAllocator};

/// Each owned pawn's assist facts, keyed by its owner's client id: its pair
/// table as helper, sorted by `network_id`, and its received required ticks
/// as target. Built from `registry` as the snapshot's movement records read
/// it. A candidate pawn not yet stamped with a `NetworkId` has no entry.
pub(crate) fn host_assist_facts(
    registry: &EntityRegistry,
    slot_table: &SlotTable,
    assist: &AssistState,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
) -> HashMap<u64, WireAssistFacts> {
    let mut owned: Vec<(u64, EntityId)> = owners
        .iter()
        .filter(|(pawn, _)| registry.exists(*pawn))
        .map(|(pawn, client_id)| (client_id, pawn))
        .collect();
    if owned.is_empty() {
        return HashMap::new();
    }
    // A stable evaluation order, so any fault warning names the same pair.
    owned.sort_unstable_by_key(|(client_id, _)| *client_id);
    let helpers: Vec<EntityId> = owned.iter().map(|(_, pawn)| *pawn).collect();
    let facts = assist.snapshot_facts(registry, slot_table, &helpers);
    owned
        .into_iter()
        .zip(facts)
        .map(|((client_id, _), facts)| {
            let mut pairs: Vec<WireAssistPair> = facts
                .pairs
                .iter()
                .filter_map(|(target, values)| {
                    Some(WireAssistPair {
                        network_id: allocator.network_id_for_entity(*target)?.0,
                        radius: values.radius,
                        required_ticks: values.required_ticks,
                    })
                })
                .collect();
            pairs.sort_unstable_by_key(|pair| pair.network_id);
            (
                client_id,
                WireAssistFacts {
                    pairs,
                    received_required_ticks: facts.received_required_ticks,
                },
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "host_assist_facts_tests.rs"]
mod tests;
