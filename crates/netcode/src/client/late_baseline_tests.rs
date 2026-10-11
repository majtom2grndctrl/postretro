use super::*;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::wire::{
    self, RawEntityRecord, RawSnapshotMessage, WireLightState, WireTransform,
};

const CLIENT: u64 = 7;

fn entity(network_id: u32, x: f32) -> EntitySnapshot {
    EntitySnapshot::unowned(
        network_id,
        vec![ComponentPayload::Transform(WireTransform {
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        })],
    )
}

fn producer(entities: Vec<EntitySnapshot>) -> ServerReplication {
    let mut server = ServerReplication::new();
    server.register_client(CLIENT);
    server.ingest_tick(entities);
    server
}

fn packet(sequence: u32, entities: Vec<EntitySnapshot>) -> RawSnapshotMessage {
    let mut raw = producer(entities)
        .encode_for_client(CLIENT, sequence * 10)
        .unwrap();
    raw.sequence = sequence;
    raw.server_script_time = f64::from(sequence);
    raw
}

fn tombstone(network_id: u32) -> RawEntityRecord {
    let mut server = producer(vec![entity(network_id, 0.0)]);
    server.ingest_tick(Vec::new());
    server
        .encode_for_client(CLIENT, 0)
        .unwrap()
        .records
        .remove(0)
}

fn receive(
    client: &mut ClientReplication,
    registry: &mut EntityRegistry,
    raw: &RawSnapshotMessage,
    resolver: &mut dyn PresentationBindingResolver,
) -> ApplyOutcome {
    receive_at_mover_target_tick(client, registry, raw, raw.server_tick, resolver)
}

fn receive_at_mover_target_tick(
    client: &mut ClientReplication,
    registry: &mut EntityRegistry,
    raw: &RawSnapshotMessage,
    mover_target_tick: u32,
    resolver: &mut dyn PresentationBindingResolver,
) -> ApplyOutcome {
    let snapshot = crate::decode_snapshot(&wire::encode(raw)).expect("valid producer snapshot");
    client.apply_snapshot_with_presentation(
        registry,
        &snapshot,
        mover_target_tick,
        1.0 / 60.0,
        resolver,
        100.0,
    )
}

// Regression: the first baseline was discarded when a newer empty flight-window
// snapshot overtook it, delaying enemy materialization beyond its spawn windup.
#[test]
fn late_first_baseline_acks_after_newer_empty_without_rewinding_cursor() {
    let mut server = producer(vec![entity(7, 4.0)]);
    let first = server.encode_for_client(CLIENT, 100).unwrap();
    let empty = server.encode_for_client(CLIENT, 110).unwrap();
    assert!(empty.records.is_empty(), "in-flight baseline is not resent");
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let newest = receive(
        &mut client,
        &mut registry,
        &empty,
        &mut NoPresentationBindings,
    );
    assert!(newest.accepted_newest_snapshot);
    let late = receive(
        &mut client,
        &mut registry,
        &first,
        &mut NoPresentationBindings,
    );
    assert!(!late.accepted_newest_snapshot);
    let id = client
        .entity_for_network_id(NetworkId(7))
        .expect("late baseline spawns");
    assert!((registry.get_component::<Transform>(id).unwrap().position.x - 4.0).abs() < 1.0e-6);
    let ack = late.ack.expect("late entity progress is acknowledged");
    assert_eq!(ack.latest_snapshot_sequence, empty.sequence);
    assert_eq!(ack.acked_server_tick, 110);
    assert_eq!(
        ack.entity_baselines,
        vec![(7, first.records[0].baseline_id_or_ref)]
    );
    assert!(ack.slot_baselines.is_empty());
    assert!(ack.faction_sentiment_baseline.is_none());
    server.apply_ack(
        CLIENT,
        ack.latest_snapshot_sequence,
        &ack.entity_baselines,
        &ack.despawn_tombstones,
    );
    assert!(
        server
            .encode_for_client(CLIENT, 120)
            .unwrap()
            .records
            .is_empty()
    );
    assert!(
        receive(
            &mut client,
            &mut registry,
            &first,
            &mut NoPresentationBindings
        )
        .ack
        .is_none()
    );
}

#[test]
fn late_first_baseline_does_not_replay_known_state_or_newer_tombstone() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let mut newest = packet(5, vec![entity(7, 5.0)]);
    newest.records.push(tombstone(8)); // The client never saw this entity alive.
    receive(
        &mut client,
        &mut registry,
        &newest,
        &mut NoPresentationBindings,
    );
    let held = client.entity_for_network_id(NetworkId(7)).unwrap();
    let mut late = packet(4, vec![entity(7, 4.0), entity(8, 4.0), entity(9, 4.0)]);
    late.records.push(tombstone(7));
    let outcome = receive(
        &mut client,
        &mut registry,
        &late,
        &mut NoPresentationBindings,
    );
    assert_eq!(outcome.ack.unwrap().entity_baselines.len(), 1);
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(held));
    assert!(
        (registry
            .get_component::<Transform>(held)
            .unwrap()
            .position
            .x
            - 5.0)
            .abs()
            < 1.0e-6
    );
    assert!(client.entity_for_network_id(NetworkId(8)).is_none());
    assert!(client.entity_for_network_id(NetworkId(9)).is_some());
    assert!(outcome.refresh_requests.is_empty());

    // A superseded delta must not request repair or mutate the newly mapped entity.
    let mut delta = packet(3, vec![entity(9, 3.0)]);
    delta.records[0].record_kind = wire::RECORD_KIND_DELTA;
    delta.records[0].new_baseline_id_or_tombstone_id = 10;
    let outcome = receive(
        &mut client,
        &mut registry,
        &delta,
        &mut NoPresentationBindings,
    );
    assert!(outcome.ack.is_none());
    assert!(outcome.refresh_requests.is_empty());

    // An empty q8 may also overtake the q7 tombstone. Even when that tombstone
    // is itself stale, the delayed first q6 baseline must never resurrect it.
    receive(
        &mut client,
        &mut registry,
        &packet(8, vec![]),
        &mut NoPresentationBindings,
    );
    let mut retired = packet(7, vec![]);
    retired.records.push(tombstone(10));
    let retired = receive(
        &mut client,
        &mut registry,
        &retired,
        &mut NoPresentationBindings,
    );
    assert!(retired.ack.is_none());
    let before_retirement = receive(
        &mut client,
        &mut registry,
        &packet(6, vec![entity(10, 6.0)]),
        &mut NoPresentationBindings,
    );
    assert!(before_retirement.ack.is_none());
    assert!(client.entity_for_network_id(NetworkId(10)).is_none());
}

#[test]
fn late_first_baseline_cannot_cross_reset_or_forgotten_history() {
    for demotion in [false, true] {
        let mut registry = EntityRegistry::new();
        let mut client = ClientReplication::new();
        receive(
            &mut client,
            &mut registry,
            &packet(5, vec![entity(7, 5.0)]),
            &mut NoPresentationBindings,
        );
        if demotion {
            client.despawn_all_mapped(&mut registry);
            client.reset_for_demotion();
        } else {
            client.reset_for_level_unload();
            registry.clear_for_level_unload();
        }
        receive(
            &mut client,
            &mut registry,
            &packet(6, vec![]),
            &mut NoPresentationBindings,
        );
        let stale = receive(
            &mut client,
            &mut registry,
            &packet(4, vec![entity(7, 4.0), entity(8, 4.0)]),
            &mut NoPresentationBindings,
        );
        assert!(stale.ack.is_none());
        assert!(client.map().is_empty());
        receive(
            &mut client,
            &mut registry,
            &packet(7, vec![entity(7, 7.0)]),
            &mut NoPresentationBindings,
        );
        assert!(client.entity_for_network_id(NetworkId(7)).is_some());
    }

    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let mut churn = packet(5, vec![]);
    churn.records = (0..=super::late_baselines::MAX_REMEMBERED_IDENTITIES as u32)
        .map(tombstone)
        .collect();
    receive(
        &mut client,
        &mut registry,
        &churn,
        &mut NoPresentationBindings,
    );
    receive(
        &mut client,
        &mut registry,
        &packet(6, vec![]),
        &mut NoPresentationBindings,
    );
    let stale = receive(
        &mut client,
        &mut registry,
        &packet(4, vec![entity(0, 4.0), entity(90_000, 4.0)]),
        &mut NoPresentationBindings,
    );
    assert!(
        stale.ack.is_none(),
        "eviction cannot revive forgotten identities"
    );
    assert!(client.map().is_empty());
    receive(
        &mut client,
        &mut registry,
        &packet(8, vec![]),
        &mut NoPresentationBindings,
    );
    let useful = receive(
        &mut client,
        &mut registry,
        &packet(7, vec![entity(90_000, 7.0)]),
        &mut NoPresentationBindings,
    );
    assert!(
        useful.ack.is_some(),
        "later flight windows remain useful after overflow"
    );
}

#[derive(Default)]
struct LightResolver {
    entity: Option<EntityId>,
    starts: Vec<Option<f64>>,
}

impl PresentationBindingResolver for LightResolver {
    fn resolve(&self, _: &EntityRegistry, key: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        (key.kind == PresentationBindingKind::MapLight && key.index == 0)
            .then_some((self.entity?, Vec3::ZERO))
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        entity: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        (self.entity == Some(entity)).then_some((
            PresentationBindingKey {
                kind: PresentationBindingKind::MapLight,
                index: 0,
            },
            Vec3::ZERO,
        ))
    }
    fn seed_light(&mut self, _: &EntityRegistry, _: EntityId, start: Option<f64>) -> bool {
        self.starts.push(start);
        true
    }
}

fn light(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            LightComponent {
                origin: [0.0; 3],
                light_type: LightKind::Point,
                intensity: 1.0,
                color: [1.0; 3],
                falloff_model: FalloffKind::Linear,
                falloff_range: 5.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: true,
                animated_slot: None,
                follow_transform: false,
                carrier: None,
                animation: None,
            },
        )
        .unwrap();
    id
}

fn bound(network_id: u32) -> EntitySnapshot {
    let mut entity = EntitySnapshot::unowned(
        network_id,
        vec![ComponentPayload::LightState(WireLightState {
            intensity: 2.0,
            color: [1.0; 3],
            cone_direction: None,
            animation: Some(wire::WireLightAnimation {
                period_ms: 10_000.0,
                phase: None,
                play_count: Some(1),
                start_active: Some(true),
                brightness: Some(vec![0.0, 1.0]),
                color: None,
                direction: None,
            }),
            anim_start: Some(1.0),
        })],
    );
    entity.presentation_binding = Some(wire::WirePresentationBinding {
        kind: 0,
        index: 0,
        origin: [0.0; 3],
    });
    entity
}

#[test]
fn late_bound_baseline_keeps_newest_clock_and_cannot_displace_or_unreject_identity() {
    let mut registry = EntityRegistry::new();
    let id = light(&mut registry);
    let mut resolver = LightResolver {
        entity: Some(id),
        ..LightResolver::default()
    };
    let mut client = ClientReplication::new();
    receive(
        &mut client,
        &mut registry,
        &packet(5, vec![]),
        &mut resolver,
    );
    let late = receive(
        &mut client,
        &mut registry,
        &packet(4, vec![bound(7)]),
        &mut resolver,
    );
    assert!(late.ack.is_some());
    client.apply_presentation_batch(
        &mut registry,
        late.presentation_payloads,
        100.0,
        Some(0.0),
        &mut resolver,
    );
    assert!(
        (resolver.starts[0].unwrap() - 96.0).abs() < 1.0e-9,
        "age uses newest header 5, not late header 4"
    );
    let alias = receive(
        &mut client,
        &mut registry,
        &packet(3, vec![bound(8)]),
        &mut resolver,
    );
    assert!(alias.ack.is_none());
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(id));
    assert!(client.entity_for_network_id(NetworkId(8)).is_none());
    assert!(
        !client.bindings.is_presentation_bound(NetworkId(8)),
        "alias check is side-effect free"
    );

    resolver.entity = None;
    let rejected = receive(
        &mut client,
        &mut registry,
        &packet(6, vec![bound(9)]),
        &mut resolver,
    );
    assert!(rejected.ack.is_some());
    resolver.entity = Some(id);
    let late = receive(
        &mut client,
        &mut registry,
        &packet(4, vec![bound(9)]),
        &mut resolver,
    );
    assert!(late.ack.is_none());
    assert!(client.entity_for_network_id(NetworkId(9)).is_none());
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(id));

    // Identity displacement also retires the old id for late-admission purposes.
    receive(
        &mut client,
        &mut registry,
        &packet(7, vec![bound(10)]),
        &mut resolver,
    );
    let retired = receive(
        &mut client,
        &mut registry,
        &packet(5, vec![bound(7)]),
        &mut resolver,
    );
    assert!(retired.ack.is_none());
    assert_eq!(client.entity_for_network_id(NetworkId(10)), Some(id));
}

#[test]
fn late_first_mover_baseline_binds_and_advances_after_newer_empty() {
    let mut registry = EntityRegistry::new();
    let authored = super::tests::spawn_loaded_mover(&mut registry, 3);
    let mut phase = super::tests::mover_payload(3);
    let ComponentPayload::KinematicMoverState(wire) = &mut phase else {
        unreachable!("mover_payload returns a mover phase");
    };
    wire.segment_index = 0;
    wire.direction = 1;
    wire.segment_elapsed_ms = 0.0;
    wire.wait_remaining_ms = 0.0;
    wire.blocked = false;
    wire.velocity = [1.0, 0.0, 0.0];
    let mut mover = entity(7, 0.0);
    mover.components.push(phase);
    let mut server = producer(vec![mover]);
    let first = server.encode_for_client(CLIENT, 100).unwrap();
    let empty = server.encode_for_client(CLIENT, 110).unwrap();
    assert!(empty.records.is_empty(), "in-flight baseline is not resent");
    let mut client = ClientReplication::new();
    receive(
        &mut client,
        &mut registry,
        &empty,
        &mut NoPresentationBindings,
    );
    assert!(client.map().is_empty(), "authored mover is still unmapped");

    // Regression: an overtaken first mover baseline must still seed the authored
    // mover and replay its phase to the caller's current target tick.
    let late = receive_at_mover_target_tick(
        &mut client,
        &mut registry,
        &first,
        112,
        &mut NoPresentationBindings,
    );
    assert!(!late.accepted_newest_snapshot);
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(authored));
    assert_eq!(client.map().len(), 1);
    assert_eq!(
        registry
            .iter_with_kind(ComponentKind::KinematicMover)
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        vec![authored],
        "late baseline does not spawn a duplicate mover"
    );
    let transform = registry.get_component::<Transform>(authored).unwrap();
    assert!((transform.position.x - 0.2).abs() < 1.0e-6);
    let phase = registry
        .get_component::<KinematicMoverComponent>(authored)
        .unwrap();
    assert_eq!(phase.segment_index, 0);
    assert_eq!(phase.direction_sign, 1);
    assert!((phase.segment_elapsed_ms - 200.0).abs() < 1.0e-3);
    assert_eq!(client.sample_count(NetworkId(7)), 0);
    assert_eq!(client.mover_history_sample_count(3), 2);
    let history = client.mover_history();
    let anchor = history.pose_at_tick(3, 100, 1.0 / 60.0).unwrap();
    assert!(anchor.transform.position.length() < 1.0e-6);
    let advanced = history.pose_at_tick(3, 112, 1.0 / 60.0).unwrap();
    assert!((advanced.transform.position.x - 0.2).abs() < 1.0e-6);
    assert!((advanced.tick_delta - Vec3::X / 60.0).length() < 1.0e-6);
    assert_eq!(late.mover_corrections.len(), 1);
    let correction = &late.mover_corrections[0];
    assert_eq!(correction.network_id, NetworkId(7));
    assert_eq!(correction.mover_id, 3);
    assert_eq!(correction.authoritative_state.entity, authored);
    assert!((correction.magnitude - 0.2).abs() < 1.0e-6);
    assert!(
        (correction.authoritative_state.transform.position - advanced.transform.position).length()
            < 1.0e-6
    );
    let ack = late.ack.unwrap();
    assert_eq!(ack.latest_snapshot_sequence, empty.sequence);
    assert_eq!(ack.acked_server_tick, empty.server_tick);
    assert_eq!(
        ack.entity_baselines,
        vec![(7, first.records[0].baseline_id_or_ref)]
    );
    assert_eq!(client.latest_sequence(), Some(empty.sequence));
    let repeated = receive_at_mover_target_tick(
        &mut client,
        &mut registry,
        &first,
        120,
        &mut NoPresentationBindings,
    );
    assert!(repeated.ack.is_none());
    assert!(repeated.mover_corrections.is_empty());
    assert_eq!(client.mover_history_sample_count(3), 2);
    assert!(
        (registry
            .get_component::<Transform>(authored)
            .unwrap()
            .position
            .x
            - 0.2)
            .abs()
            < 1.0e-6
    );
}

#[test]
fn late_baseline_cannot_claim_newer_mover_binding_or_second_local_pawn() {
    let mut registry = EntityRegistry::new();
    let mover = super::tests::spawn_loaded_mover(&mut registry, 3);
    let mover_entity = |nid| {
        let mut mover = entity(nid, 0.0);
        mover.components.push(super::tests::mover_payload(3));
        mover
    };
    let pawn = |nid| {
        let mut pawn = entity(nid, 5.0);
        pawn.components.push(super::tests::movement_payload());
        pawn.owner_client_id = Some(CLIENT);
        pawn.entity_class = Some("pawn:player".into());
        pawn
    };
    let mut client = ClientReplication::new();
    receive(
        &mut client,
        &mut registry,
        &packet(5, vec![mover_entity(7), pawn(9)]),
        &mut NoPresentationBindings,
    );
    let original_pawn = client.entity_for_network_id(NetworkId(9));
    assert!(original_pawn.is_some());
    let late = receive(
        &mut client,
        &mut registry,
        &packet(4, vec![mover_entity(8), pawn(10)]),
        &mut NoPresentationBindings,
    );
    assert!(late.ack.is_none());
    assert!(late.armed_local_pawn.is_none());
    assert_eq!(client.entity_for_network_id(NetworkId(7)), Some(mover));
    assert_eq!(client.entity_for_network_id(NetworkId(9)), original_pawn);
    assert!(client.entity_for_network_id(NetworkId(8)).is_none());
    assert!(client.entity_for_network_id(NetworkId(10)).is_none());
}

// Regression: entity progress in a late snapshot must not reopen the receive
// path's state-slot/faction gate, even though that progress produces an ack.
#[test]
fn late_baseline_receive_keeps_newer_slots_factions_and_presentation_clock() {
    use crate::state_slots::{ClientStateApply, ReplicatedSlotIdentity, ReplicatedSlotSchema};
    use postretro_entities::{
        FactionRegistry, FactionSentimentState, ReplicationScope, SlotTable, SlotValue,
    };
    use postretro_net::state_slots::{
        FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE, FactionSentimentPair,
        RawFactionSentimentRecord, RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE,
        WireSlotValue,
    };
    use postretro_net::transport::{NetClient, NetServer};
    use std::cell::RefCell;
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::Duration;

    let dt = Duration::from_millis(16);
    let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = server_socket.local_addr().unwrap();
    let client_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let fingerprint = [0x5a; 32];
    let mut server = NetServer::new(server_socket, address, 1, dt, Some(fingerprint)).unwrap();
    let mut client =
        NetClient::new(client_socket, address, CLIENT, dt, Some(fingerprint), None).unwrap();
    server.set_mod_identity("test.mod".into(), "1.0.0".into());
    server.set_mod_digest(Some(fingerprint));
    server.set_level_parity(Some(("test-level".into(), fingerprint)));
    client.set_mod_identity("test.mod".into(), "1.0.0".into());
    client.set_mod_digest(Some(fingerprint));
    client.set_level_parity(Some(("test-level".into(), fingerprint)));
    postretro_net::harness::reveal_both(&mut server, &mut client, "test-level");
    server.add_relay_connection(CLIENT, None);
    client.set_connected();
    for _ in 0..8 {
        client.update_connections(dt);
        for bytes in client.packets_to_send() {
            server.process_packet_from(&bytes, CLIENT);
        }
        server.update_connections(dt);
        let _ = server.poll_handshakes();
        for bytes in server.packets_to_send(CLIENT) {
            client.process_packet(&bytes);
        }
        client.update_connections(dt);
        let _ = client.drain_control();
    }
    assert!(server.is_participating(CLIENT));
    assert!(client.is_participating());

    let identity = ReplicatedSlotIdentity::default();
    let mut slots = SlotTable::new();
    slots.get_mut("player.health").unwrap().schema.network = ReplicationScope::SharedGlobal;
    let schema = ReplicatedSlotSchema::build(&slots, &identity);
    let health_slot = schema.id_for("player.health").unwrap().0;
    let envelope = |sequence, health, sentiment, entities| {
        let mut snapshot = packet(sequence, entities);
        snapshot.state_schema_fingerprint = *schema.fingerprint();
        snapshot.state_records.push(RawStateSlotRecord {
            slot_id: health_slot,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: sequence,
            value: WireSlotValue::Number(health),
        });
        snapshot.faction_sentiment_record = Some(RawFactionSentimentRecord {
            kind: FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: sequence,
            pairs: vec![FactionSentimentPair {
                from_idx: 0,
                to_idx: 1,
                value: sentiment,
            }],
        });
        snapshot
    };
    let newest = envelope(5, 75.0, -0.5, vec![]);
    let late = envelope(4, 25.0, -0.75, vec![bound(7)]);
    let mut registry = EntityRegistry::new();
    let authored = light(&mut registry);
    let mut resolver = LightResolver {
        entity: Some(authored),
        ..LightResolver::default()
    };
    let mut replication = ClientReplication::new();
    let mut state = ClientStateApply::new();
    let mut prediction = crate::ClientPrediction::new();
    let factions = FactionRegistry::default();
    let sentiment = RefCell::new(FactionSentimentState::default());
    let collision = crate::collision::CollisionWorld::new();
    let hit_zones = crate::scripting_systems::hit_zones::HitZoneStore::new();
    for (index, snapshot) in [newest, late].into_iter().enumerate() {
        assert!(server.send_snapshot(CLIENT, wire::encode(&snapshot)));
        server.update_connections(dt);
        for bytes in server.packets_to_send(CLIENT) {
            client.process_packet(&bytes);
        }
        client.update_connections(dt);
        assert!(client.drain_control().is_empty());
        crate::client_receive_and_apply(
            &mut registry,
            &mut slots,
            &factions,
            &sentiment,
            &identity,
            &mut client,
            &mut replication,
            &mut state,
            &mut prediction,
            &[],
            &hit_zones,
            None,
            &collision,
            -20.0,
            1.0 / 60.0,
            dt,
            None,
            None,
            None,
            false,
            &mut resolver,
            100.0,
            Some(0.0),
        );
        assert!(
            matches!(slots.get("player.health").unwrap().value, Some(SlotValue::Number(value)) if (value - 75.0).abs() < 1.0e-6)
        );
        assert!((sentiment.borrow().get(0.0, 1.0).unwrap() + 0.5).abs() < 1.0e-6);
        assert_eq!(replication.latest_sequence(), Some(5));

        client.update_connections(dt);
        for bytes in client.packets_to_send() {
            server.process_packet_from(&bytes, CLIENT);
        }
        server.update_connections(dt);
        let acks: Vec<AckMessage> = server
            .drain_input(CLIENT)
            .into_iter()
            .filter_map(
                |bytes| match wire::decode::<ClientMessage>(&bytes).unwrap() {
                    ClientMessage::Ack(ack) => Some(ack),
                    _ => None,
                },
            )
            .collect();
        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].latest_snapshot_sequence, 5);
        assert_eq!(acks[0].acked_server_tick, 50);
        if index == 0 {
            assert_eq!(acks[0].slot_baselines, vec![(health_slot, 5)]);
            assert_eq!(acks[0].faction_sentiment_baseline, Some(5));
        } else {
            assert_eq!(acks[0].entity_baselines, vec![(7, 1)]);
            assert!(acks[0].slot_baselines.is_empty());
            assert!(acks[0].faction_sentiment_baseline.is_none());
        }
    }
    assert_eq!(
        replication.entity_for_network_id(NetworkId(7)),
        Some(authored)
    );
    assert_eq!(resolver.starts.len(), 1);
    assert!((resolver.starts[0].unwrap() - 96.0).abs() < 1.0e-9);
}
