// Host build of each connected client's assist facts for its snapshot.
// See: context/lib/networking.md §Version gates · context/lib/scripting.md §12

use std::collections::HashMap;

use postretro_entities::{EntityId, EntityRegistry};
use postretro_net::wire::{WireAssistFacts, WireAssistPair};
use postretro_scripting_core::group_resolution::is_player_pawn;
use postretro_scripting_core::slot_table::SlotTable;
use postretro_sim::sim::assist::AssistState;

use crate::{MovementOwners, NetworkIdAllocator};

/// Each participating owner's assist facts, keyed by its client id: its
/// pawn's pair table as helper, sorted by `network_id`, and its received
/// required ticks as target. Built from `registry` as the snapshot's movement
/// records read it. A candidate pawn not yet stamped with a `NetworkId` has no
/// entry. An owner not in `participating`, or whose pawn is outside the
/// `players()` group (its seat in a disconnect hold), receives no snapshot and
/// gets no facts, so nothing is evaluated for it.
pub(crate) fn host_assist_facts(
    registry: &EntityRegistry,
    slot_table: &SlotTable,
    assist: &AssistState,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
    participating: &[u64],
) -> HashMap<u64, WireAssistFacts> {
    let mut owned: Vec<(u64, EntityId)> = owners
        .iter()
        .filter(|(pawn, client_id)| {
            participating.contains(client_id)
                && registry.exists(*pawn)
                && is_player_pawn(registry, *pawn)
        })
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
        .filter_map(|((client_id, _), facts)| {
            let facts = facts?;
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
            // Validation refuses a table not strictly ascending by id; each
            // target is a distinct pawn with its own id, so sorting suffices.
            pairs.sort_unstable_by_key(|pair| pair.network_id);
            Some((
                client_id,
                WireAssistFacts {
                    pairs,
                    received_required_ticks: facts.received_required_ticks,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "host_assist_facts_tests.rs"]
mod tests;
