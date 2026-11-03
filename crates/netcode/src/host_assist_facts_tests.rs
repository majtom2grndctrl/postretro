// Host assist facts: keyed by owner, sorted by `network_id`, unstamped pawns
// left out, and the wire bounds pinned to the engine constants.
use std::collections::HashMap;

use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    ASSIST_MAX_HOLD_MS, ASSIST_MAX_RADIUS_M, PlayerMovementComponent, Seat,
};
use postretro_net::wire::{
    WIRE_ASSIST_MAX_RADIUS, WIRE_ASSIST_MAX_REQUIRED_TICKS, WireAssistFacts, WireAssistPair,
};
use postretro_scripting_core::slot_table::SlotTable;
use postretro_sim::sim::assist::{AssistState, required_count};

use super::host_assist_facts;
use crate::predict_reconcile_harness_test_fixtures::player_descriptor;
use crate::{MovementOwners, NetworkIdAllocator};

/// The wire crate declares its own bounds; they must stay the engine's.
#[test]
fn assist_wire_bounds_equal_the_engine_constants() {
    assert_eq!(WIRE_ASSIST_MAX_RADIUS, ASSIST_MAX_RADIUS_M);
    assert_eq!(
        WIRE_ASSIST_MAX_REQUIRED_TICKS,
        required_count(ASSIST_MAX_HOLD_MS)
    );
}

fn spawn_pawn(registry: &mut EntityRegistry, seat: u16, assistable: bool) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    let mut movement = PlayerMovementComponent::from_descriptor(&player_descriptor());
    movement.assistable = assistable;
    registry.set_component(pawn, movement).unwrap();
    registry.bind_pawn_seat(pawn, Seat(seat));
    pawn
}

fn unassistable(network_id: u32) -> WireAssistPair {
    WireAssistPair {
        network_id,
        radius: 0.0,
        required_ticks: 0,
    }
}

/// Each owner gets its own pawn's table: every other assistable pawn, sorted
/// by `network_id` whatever the `players()` order, a pawn with no `NetworkId`
/// yet left out. The default assist state binds nothing, so each pair is
/// unassistable and nothing is received.
#[test]
fn assist_facts_are_keyed_by_owner_sorted_and_skip_unstamped_pawns() {
    let mut registry = EntityRegistry::new();
    let a = spawn_pawn(&mut registry, 0, true);
    let b = spawn_pawn(&mut registry, 1, true);
    let c = spawn_pawn(&mut registry, 2, true);
    let unstamped = spawn_pawn(&mut registry, 3, true);
    let host = spawn_pawn(&mut registry, 4, false);
    registry.mark_local_player_pawn(host).unwrap();

    // Stamp out of `players()` order so the sort is visible.
    let mut allocator = NetworkIdAllocator::new();
    for pawn in [host, c, b, a] {
        allocator.stamp(pawn);
    }
    let id = |pawn| allocator.network_id_for_entity(pawn).unwrap().0;
    let mut owners = MovementOwners::new();
    owners.set(a, 10);
    owners.set(b, 11);
    let _ = unstamped;

    let facts = host_assist_facts(
        &registry,
        &SlotTable::default(),
        &AssistState::default(),
        &owners,
        &allocator,
        &[10, 11],
    );
    let expected = |pawns: &[EntityId]| {
        let mut pairs: Vec<WireAssistPair> = pawns.iter().map(|p| unassistable(id(*p))).collect();
        pairs.sort_unstable_by_key(|pair| pair.network_id);
        WireAssistFacts {
            pairs,
            received_required_ticks: 0,
        }
    };
    assert_eq!(
        facts,
        HashMap::from([(10, expected(&[b, c])), (11, expected(&[a, c]))])
    );
    assert!(
        facts[&10]
            .pairs
            .windows(2)
            .all(|w| w[0].network_id < w[1].network_id)
    );
}

/// O59 (host half): with no assistable pawn the owner still gets facts, an
/// empty table, so the flag carries the emptied list.
#[test]
fn assist_facts_o59_no_candidate_is_an_empty_table() {
    let mut registry = EntityRegistry::new();
    let a = spawn_pawn(&mut registry, 0, false);
    let b = spawn_pawn(&mut registry, 1, false);
    let mut allocator = NetworkIdAllocator::new();
    allocator.stamp(a);
    allocator.stamp(b);
    let mut owners = MovementOwners::new();
    owners.set(a, 7);
    let facts = host_assist_facts(
        &registry,
        &SlotTable::default(),
        &AssistState::default(),
        &owners,
        &allocator,
        &[7],
    );
    assert_eq!(facts, HashMap::from([(7, WireAssistFacts::default())]));
}

/// Only a participating owner whose pawn is in the `players()` group gets
/// facts: a pawn whose seat is in a disconnect hold (binding cleared, owner
/// still recorded) and an owner not participating are skipped, so nothing is
/// evaluated for them.
#[test]
fn assist_facts_skip_held_seats_and_non_participating_owners() {
    let mut registry = EntityRegistry::new();
    let seated = spawn_pawn(&mut registry, 1, true);
    let held = spawn_pawn(&mut registry, 2, true);
    let loading = spawn_pawn(&mut registry, 3, true);
    registry.clear_pawn_seat(held);
    let mut allocator = NetworkIdAllocator::new();
    for pawn in [seated, held, loading] {
        allocator.stamp(pawn);
    }
    let mut owners = MovementOwners::new();
    owners.set(seated, 10);
    owners.set(held, 11);
    owners.set(loading, 12);
    let facts = host_assist_facts(
        &registry,
        &SlotTable::default(),
        &AssistState::default(),
        &owners,
        &allocator,
        &[10, 11],
    );
    let id = |pawn| allocator.network_id_for_entity(pawn).unwrap().0;
    assert_eq!(
        facts,
        HashMap::from([(
            10,
            WireAssistFacts {
                pairs: vec![unassistable(id(loading))],
                received_required_ticks: 0,
            }
        )])
    );
}
