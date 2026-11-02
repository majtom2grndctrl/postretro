// The local pawn's assist facts: read from every accepted local-pawn record,
// a held-baseline resend included, kept while omitted, resolved to entities,
// and cleared at the retention map's sites.
use std::collections::HashMap;

use super::*;
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::wire::{
    WireAssistFacts, WireAssistPair, WireMovementState, WireTeleportState, WireTransform,
};

const LOCAL: u32 = 3;
const REMOTE: u32 = 7;
const UNMAPPED: u32 = 9;

fn components() -> Vec<ComponentPayload> {
    vec![
        ComponentPayload::Transform(WireTransform {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }),
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            life_state: 0,
            life_state_elapsed_ticks: 0,
            life_state_table_revision: 0,
            assistable: false,
            teleport: WireTeleportState::new(0, 0.0, postretro_net::wire::NO_WIRE_TELEPORTER, &[])
                .unwrap(),
            knockback_velocity: [0.0; 3],
            push_velocity: [0.0; 3],
            velocity: [0.0; 3],
            ground: WireGroundRef::Airborne,
            air_jumps_remaining: 0,
            air_dashes_remaining: 0,
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
    ]
}

fn facts(required_ticks: u32, received: u32) -> WireAssistFacts {
    WireAssistFacts {
        pairs: vec![
            WireAssistPair {
                network_id: REMOTE,
                radius: 1.5,
                required_ticks,
            },
            WireAssistPair {
                network_id: UNMAPPED,
                radius: 2.0,
                required_ticks,
            },
        ],
        received_required_ticks: received,
    }
}

fn baseline(
    network_id: u32,
    baseline_id: u32,
    assist_facts: Option<WireAssistFacts>,
) -> EntityRecord {
    EntityRecord::FullBaseline {
        network_id,
        baseline_id,
        last_processed_client_tick: None,
        local_player: network_id == LOCAL,
        assist_facts,
        entity_class: None,
        active_weapon_archetype: None,
        components: components(),
        projectile_presentation: None,
        presentation_binding: None,
    }
}

fn resend(baseline_id: u32, assist_facts: Option<WireAssistFacts>) -> EntityRecord {
    EntityRecord::Delta {
        network_id: LOCAL,
        baseline_ref: baseline_id,
        new_baseline_id: baseline_id,
        last_processed_client_tick: None,
        local_player: true,
        assist_facts,
        entity_class: None,
        active_weapon_archetype: None,
        components: components(),
        projectile_presentation: None,
        presentation_binding: None,
    }
}

fn snapshot(sequence: u32, records: Vec<EntityRecord>) -> SnapshotMessage {
    SnapshotMessage {
        sequence,
        server_tick: sequence + 1,
        records,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        faction_sentiment_record: None,
        ghost_audio: None,
        server_script_time: 0.0,
        level_tick: 0,
    }
}

fn entry(target: EntityId, required_ticks: u32) -> AssistPairEntry {
    AssistPairEntry {
        target,
        radius: 1.5,
        required_ticks,
    }
}

/// A client holding its local pawn (with `facts(6, 4)`) and remote pawn 7.
fn armed() -> (EntityRegistry, ClientReplication, EntityId) {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            0,
            vec![
                baseline(LOCAL, 1, Some(facts(6, 4))),
                baseline(REMOTE, 1, None),
            ],
        ),
    );
    assert_eq!(client.local_pawn_network_id(), Some(NetworkId(LOCAL)));
    let remote = client.entity_for_network_id(NetworkId(REMOTE)).unwrap();
    (registry, client, remote)
}

#[test]
fn assist_pair_table_resolves_entries_and_leaves_unmapped_ones_out() {
    let client = ClientReplication::new();
    assert!(client.assist_pair_table().is_empty());
    assert_eq!(client.received_required_ticks(), 0);

    let (_registry, client, remote) = armed();
    assert_eq!(client.assist_pair_table(), vec![entry(remote, 6)]);
    assert_eq!(client.received_required_ticks(), 4);
}

#[test]
fn assist_facts_read_from_a_held_baseline_resend_and_kept_while_omitted() {
    let (mut registry, mut client, remote) = armed();
    // A metadata-only resend at the held baseline carries new values.
    client.apply_snapshot(
        &mut registry,
        &snapshot(1, vec![resend(1, Some(facts(9, 0)))]),
    );
    assert_eq!(client.assist_pair_table(), vec![entry(remote, 9)]);
    assert_eq!(client.received_required_ticks(), 0);
    // A record without them, or a snapshot without the pawn, keeps them.
    client.apply_snapshot(&mut registry, &snapshot(2, vec![resend(1, None)]));
    client.apply_snapshot(&mut registry, &snapshot(3, Vec::new()));
    assert_eq!(client.assist_pair_table(), vec![entry(remote, 9)]);

    // O59: an empty list behind the flag empties the table.
    client.apply_snapshot(
        &mut registry,
        &snapshot(4, vec![resend(1, Some(WireAssistFacts::default()))]),
    );
    assert!(client.assist_pair_table().is_empty());
}

#[test]
fn assist_facts_entry_drops_out_when_its_pawn_unmaps() {
    let (mut registry, mut client, _remote) = armed();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            1,
            vec![EntityRecord::Despawn {
                network_id: REMOTE,
                tombstone_id: 1,
                reason: 0,
            }],
        ),
    );
    assert!(client.assist_pair_table().is_empty());
    assert_eq!(client.received_required_ticks(), 4, "own facts kept");
}

#[test]
fn assist_facts_clear_at_the_retention_maps_sites() {
    let (_registry, mut client, _remote) = armed();
    let _ = client.reset_for_level_unload();
    assert_eq!(client.received_required_ticks(), 0);

    let (_registry, mut client, _remote) = armed();
    client.reset_for_demotion();
    assert_eq!(client.received_required_ticks(), 0);

    let (mut registry, mut client, _remote) = armed();
    client.apply_snapshot(
        &mut registry,
        &snapshot(
            1,
            vec![EntityRecord::Despawn {
                network_id: LOCAL,
                tombstone_id: 1,
                reason: 0,
            }],
        ),
    );
    assert_eq!(client.received_required_ticks(), 0);
    assert!(client.assist_pair_table().is_empty());
}

/// End to end: a change on a still pawn resends it to its owner at the held
/// baseline, which the client reads back through `assist_pair_table`; a lost
/// snapshot is resent until one is acknowledged.
#[test]
fn assist_facts_change_on_a_still_pawn_reaches_the_owner_through_a_held_baseline_resend() {
    const OWNER: u64 = 5;
    const OTHER: u64 = 6;
    let pawn = |network_id: u32, owner: u64| EntitySnapshot {
        network_id,
        components: components(),
        owner_client_id: Some(owner),
        last_processed_client_tick: None,
        entity_class: None,
        active_weapon_archetype: None,
        projectile_presentation: None,
        presentation_binding: None,
    };
    let pair = |required_ticks: u32| WireAssistFacts {
        pairs: vec![WireAssistPair {
            network_id: REMOTE,
            radius: 1.5,
            required_ticks,
        }],
        received_required_ticks: 0,
    };
    let mut server = ServerReplication::new();
    server.register_client(OWNER);
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let mut deliver = |server: &mut ServerReplication, client: &mut ClientReplication| {
        let raw = server.encode_for_client(OWNER, 1).unwrap();
        let typed = raw.validate().expect("valid snapshot");
        client.apply_snapshot(&mut registry, &typed);
        let acks: Vec<(u32, u32)> = typed
            .records
            .iter()
            .filter_map(|record| match record {
                EntityRecord::FullBaseline {
                    network_id,
                    baseline_id,
                    ..
                } => Some((*network_id, *baseline_id)),
                EntityRecord::Delta {
                    network_id,
                    new_baseline_id,
                    ..
                } => Some((*network_id, *new_baseline_id)),
                EntityRecord::Despawn { .. } => None,
            })
            .collect();
        server.apply_ack(OWNER, typed.sequence, &acks, &[]);
        typed.records.len()
    };

    server.ingest_tick(vec![pawn(LOCAL, OWNER), pawn(REMOTE, OTHER)]);
    server.ingest_assist_facts(HashMap::from([(OWNER, pair(6))]));
    assert_eq!(deliver(&mut server, &mut client), 2);
    let remote = client.entity_for_network_id(NetworkId(REMOTE)).unwrap();
    assert_eq!(client.assist_pair_table(), vec![entry(remote, 6)]);
    assert_eq!(deliver(&mut server, &mut client), 0, "still and confirmed");

    // The value changes on the still pawn; the first resend is lost.
    server.ingest_assist_facts(HashMap::from([(OWNER, pair(9))]));
    let lost = server.encode_for_client(OWNER, 1).unwrap();
    assert_eq!(lost.records.len(), 1);
    let held = server.current_baseline_id(LOCAL).unwrap();
    assert_eq!(deliver(&mut server, &mut client), 1, "resent until acked");
    assert_eq!(server.current_baseline_id(LOCAL), Some(held));
    assert_eq!(client.assist_pair_table(), vec![entry(remote, 9)]);
    assert_eq!(deliver(&mut server, &mut client), 0);
}
