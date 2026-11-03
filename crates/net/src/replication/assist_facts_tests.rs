//! Assist-fact delivery: owner-only stamping, the held-baseline resend on a
//! change, and confirmation only by snapshot ack.

use std::collections::HashMap;

use crate::replication::{EntitySnapshot, ServerReplication, typed_records};
use crate::wire::{
    ComponentPayload, EntityRecord, RawSnapshotMessage, WireAssistFacts, WireAssistPair,
    WireGroundRef, WireMovementState, WirePlayerMovementState, WireTransform,
};

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;

fn pawn(network_id: u32, owner: Option<u64>) -> EntitySnapshot {
    EntitySnapshot {
        network_id,
        components: vec![
            ComponentPayload::Transform(WireTransform {
                position: [network_id as f32, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            }),
            ComponentPayload::PlayerMovementState(WirePlayerMovementState {
                life_state: 255,
                life_state_elapsed_ticks: 0,
                life_state_table_revision: 0,
                assistable: false,
                teleport: Default::default(),
                knockback_velocity: [0.0; 3],
                push_velocity: [0.0; 3],
                velocity: [0.0; 3],
                ground: WireGroundRef::World,
                air_jumps_remaining: 1,
                air_dashes_remaining: 1,
                dash_cooldown_ms: 0.0,
                air_ticks: 0,
                movement_state: WireMovementState::Normal,
                coyote_timer_ms: 0.0,
                jump_buffer_timer_ms: 0.0,
                jump_spent: false,
                swim_reentry_latched: false,
                capsule_half_height: 0.8,
                capsule_eye_height: 1.5,
                aim_pitch: 0.0,
            }),
        ],
        owner_client_id: owner,
        // No resolved cursor, so only the assist facts can force a resend.
        last_processed_client_tick: None,
        entity_class: Some("player".into()),
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
    }
}

fn facts(required_ticks: u32) -> WireAssistFacts {
    WireAssistFacts {
        pairs: vec![WireAssistPair {
            network_id: 2,
            radius: 1.5,
            required_ticks,
        }],
        received_required_ticks: 0,
    }
}

fn record_for(snapshot: &RawSnapshotMessage, network_id: u32) -> Option<EntityRecord> {
    typed_records(snapshot)
        .into_iter()
        .find(|record| match record {
            EntityRecord::FullBaseline { network_id: n, .. }
            | EntityRecord::Delta { network_id: n, .. }
            | EntityRecord::Despawn { network_id: n, .. } => *n == network_id,
        })
}

fn assist_facts(record: &EntityRecord) -> Option<WireAssistFacts> {
    match record {
        EntityRecord::FullBaseline { assist_facts, .. }
        | EntityRecord::Delta { assist_facts, .. } => assist_facts.clone(),
        EntityRecord::Despawn { .. } => None,
    }
}

fn baseline_of(record: &EntityRecord) -> u32 {
    match record {
        EntityRecord::FullBaseline { baseline_id, .. } => *baseline_id,
        EntityRecord::Delta {
            new_baseline_id, ..
        } => *new_baseline_id,
        EntityRecord::Despawn { .. } => panic!("live record"),
    }
}

/// A server with A owning pawn 1 and B owning pawn 2, both baselines acked
/// by both clients with A's facts `facts(6)` confirmed.
fn settled() -> (ServerReplication, u32) {
    let mut server = ServerReplication::new();
    server.register_client(CLIENT_A);
    server.register_client(CLIENT_B);
    server.ingest_tick(vec![pawn(1, Some(CLIENT_A)), pawn(2, Some(CLIENT_B))]);
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(6))]));
    let sequence = server.begin_batch();
    for client in [CLIENT_A, CLIENT_B] {
        let snapshot = server.encode_in_batch(client, 1, sequence).unwrap();
        let acks: Vec<(u32, u32)> = typed_records(&snapshot)
            .iter()
            .map(|record| match record {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    ..
                } => (*network_id, *baseline_id),
                _ => panic!("first records are full baselines"),
            })
            .collect();
        server.apply_ack(client, sequence, &acks, &[]);
    }
    let baseline = server.current_baseline_id(1).unwrap();
    (server, baseline)
}

#[test]
fn assist_facts_ride_only_the_owners_local_pawn_record() {
    let mut server = ServerReplication::new();
    server.register_client(CLIENT_A);
    server.register_client(CLIENT_B);
    server.ingest_tick(vec![
        pawn(1, Some(CLIENT_A)),
        pawn(2, Some(CLIENT_B)),
        pawn(3, None),
    ]);
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(6))]));
    let to_a = server.encode_for_client(CLIENT_A, 1).unwrap();
    assert_eq!(assist_facts(&record_for(&to_a, 1).unwrap()), Some(facts(6)));
    for other in [2, 3] {
        assert_eq!(assist_facts(&record_for(&to_a, other).unwrap()), None);
    }
    // B has no facts handed in, so even its own pawn carries none.
    let to_b = server.encode_for_client(CLIENT_B, 1).unwrap();
    for id in [1, 2, 3] {
        assert_eq!(assist_facts(&record_for(&to_b, id).unwrap()), None);
    }
}

#[test]
fn assist_facts_change_on_a_still_pawn_resends_the_owner_at_its_held_baseline() {
    let (mut server, held) = settled();
    // Unchanged facts on a still, acked pawn: omitted.
    server.ingest_tick(vec![pawn(1, Some(CLIENT_A)), pawn(2, Some(CLIENT_B))]);
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(6))]));
    let quiet = server.encode_for_client(CLIENT_A, 2).unwrap();
    assert!(quiet.records.is_empty(), "{:?}", quiet.records);

    // A change mints no baseline: the owner gets its pawn at the held
    // baseline carrying the new facts; B gets nothing.
    server.ingest_tick(vec![pawn(1, Some(CLIENT_A)), pawn(2, Some(CLIENT_B))]);
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(9))]));
    assert_eq!(server.current_baseline_id(1), Some(held));
    let sequence = server.begin_batch();
    let resend = server.encode_in_batch(CLIENT_A, 3, sequence).unwrap();
    let record = record_for(&resend, 1).expect("owner pawn resent");
    assert!(matches!(
        record,
        EntityRecord::Delta { baseline_ref, new_baseline_id, local_player: true, .. }
            if baseline_ref == held && new_baseline_id == held
    ));
    assert_eq!(assist_facts(&record), Some(facts(9)));
    let to_b = server.encode_in_batch(CLIENT_B, 3, sequence).unwrap();
    assert!(to_b.records.is_empty(), "{:?}", to_b.records);

    // Acked: the next snapshot omits the pawn again.
    server.apply_ack(CLIENT_A, sequence, &[(1, baseline_of(&record))], &[]);
    let after = server.encode_for_client(CLIENT_A, 4).unwrap();
    assert!(after.records.is_empty(), "{:?}", after.records);
}

#[test]
fn assist_facts_a_lost_snapshot_is_not_delivery() {
    let (mut server, held) = settled();
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(9))]));
    // The first resend is lost: no ack reaches it.
    let lost = server.encode_for_client(CLIENT_A, 2).unwrap();
    assert_eq!(assist_facts(&record_for(&lost, 1).unwrap()), Some(facts(9)));
    // So every later snapshot resends until one is acknowledged.
    let again = server.encode_for_client(CLIENT_A, 3).unwrap();
    let record = record_for(&again, 1).expect("resent while unconfirmed");
    assert_eq!(assist_facts(&record), Some(facts(9)));
    // An ack older than the carrying snapshot confirms nothing.
    server.apply_ack(CLIENT_A, lost.sequence.wrapping_sub(1), &[(1, held)], &[]);
    assert!(record_for(&server.encode_for_client(CLIENT_A, 4).unwrap(), 1).is_some());
    // An ack at or past a carrying snapshot confirms it.
    server.apply_ack(CLIENT_A, again.sequence, &[(1, held)], &[]);
    let quiet = server.encode_for_client(CLIENT_A, 5).unwrap();
    assert!(quiet.records.is_empty(), "{:?}", quiet.records);
}

#[test]
fn assist_facts_skip_while_the_full_baseline_is_in_flight_then_carry_current_values() {
    let mut server = ServerReplication::new();
    server.register_client(CLIENT_A);
    server.ingest_tick(vec![pawn(1, Some(CLIENT_A))]);
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(6))]));
    let first = server.encode_for_client(CLIENT_A, 1).unwrap();
    let baseline = record_for(&first, 1).unwrap();
    assert_eq!(assist_facts(&baseline), Some(facts(6)));

    // Changed while the baseline is in flight: the pawn is skipped.
    server.ingest_assist_facts(HashMap::from([(CLIENT_A, facts(9))]));
    let skipped = server.encode_for_client(CLIENT_A, 2).unwrap();
    assert!(record_for(&skipped, 1).is_none());

    // The baseline's ack confirms only its own value; the next record
    // carries the current one.
    server.apply_ack(
        CLIENT_A,
        first.sequence,
        &[(1, baseline_of(&baseline))],
        &[],
    );
    let next = server.encode_for_client(CLIENT_A, 3).unwrap();
    assert_eq!(
        assist_facts(&record_for(&next, 1).expect("resent with current facts")),
        Some(facts(9))
    );
}

/// A value re-sent unchanged queues once; that entry is confirmed by an ack
/// of any snapshot at or after its first send, and a changed value queues
/// beside it.
#[test]
fn assist_delivery_queues_a_still_value_once() {
    let mut delivery = super::AssistDelivery::default();
    for sequence in 1..=5 {
        delivery.record_sent(sequence, &facts(6));
    }
    assert_eq!(delivery.sent.len(), 1);
    delivery.record_sent(6, &facts(7));
    delivery.record_sent(7, &facts(6));
    assert_eq!(delivery.sent.len(), 3);
    delivery.confirm(4);
    assert_eq!(delivery.confirmed, Some(facts(6)));
    delivery.confirm(6);
    assert_eq!(delivery.confirmed, Some(facts(7)));
    delivery.confirm(7);
    assert_eq!(delivery.confirmed, Some(facts(6)));
    assert!(delivery.sent.is_empty());
}
