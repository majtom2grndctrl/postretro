// Descriptor-load deferral and malformed wire bytes must stop before timer
// sample intake. This drives the production transport drain and receive seam.
// See: context/lib/networking.md §Snapshot apply ordering

use std::cell::RefCell;
use std::time::Duration;

use postretro_collision::EnvironmentVolumes;
use postretro_entities::{
    EntityRegistry, EntityTypeDescriptor, FactionRegistry, FactionSentimentState, SlotTable,
};
use postretro_net::state_slots::{
    RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE, WireSlotValue,
};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{
    self, RawComponentPayload, RawEntityRecord, RawSnapshotMessage, WireTransform,
};

use crate::client::{ClientReplication, NoPresentationBindings};
use crate::collision::CollisionWorld;
use crate::predict_reconcile_harness_test_fixtures::{CLIENT_ID, entity_descriptors};
use crate::predict_reconcile_relay_link_test_fixtures::relay_pair;
use crate::prediction::ClientPrediction;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::state_slots::{ClientStateApply, ReplicatedSlotIdentity, ReplicatedSlotSchema};
use crate::{ClientApplyFrameOutcome, client_receive_and_apply};

const FRAME: Duration = Duration::from_micros(16_667);

struct Receiver {
    server: NetServer,
    client: NetClient,
    registry: EntityRegistry,
    table: SlotTable,
    replication: ClientReplication,
    slots: ClientStateApply,
    prediction: ClientPrediction,
}

impl Receiver {
    fn new() -> Self {
        let (mut server, mut client) = relay_pair();
        server.add_relay_connection(CLIENT_ID, None);
        client.set_connected();
        for _ in 0..16 {
            client.update_connections(FRAME);
            for packet in client.packets_to_send() {
                server.process_packet_from(&packet, CLIENT_ID);
            }
            server.update_connections(FRAME);
            let _ = server.poll_handshakes();
            for packet in server.packets_to_send(CLIENT_ID) {
                client.process_packet(&packet);
            }
            // The production Control drain accepts the participation marker
            // before Snapshot's epoch guard can deliver a payload.
            client.drain_control();
            if server.is_participating(CLIENT_ID) && client.is_participating() {
                break;
            }
        }
        assert!(server.is_participating(CLIENT_ID) && client.is_participating());
        Self {
            server,
            client,
            registry: EntityRegistry::new(),
            table: SlotTable::new(),
            replication: ClientReplication::new(),
            slots: ClientStateApply::new(),
            prediction: ClientPrediction::new(),
        }
    }

    fn deliver(
        &mut self,
        bytes: Vec<u8>,
        descriptors: &[EntityTypeDescriptor],
    ) -> ClientApplyFrameOutcome {
        assert!(self.server.send_snapshot(CLIENT_ID, bytes));
        self.server.update_connections(FRAME);
        for packet in self.server.packets_to_send(CLIENT_ID) {
            self.client.process_packet(&packet);
        }
        self.client.update_connections(FRAME);
        self.client.drain_control();
        client_receive_and_apply(
            &mut self.registry,
            &mut self.table,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            &ReplicatedSlotIdentity::default(),
            &mut self.client,
            &mut self.replication,
            &mut self.slots,
            &mut self.prediction,
            descriptors,
            &HitZoneStore::default(),
            None,
            &CollisionWorld::new(),
            &EnvironmentVolumes::default(),
            1.0 / 60.0,
            Duration::ZERO,
            None,
            None,
            None,
            false,
            &mut NoPresentationBindings,
            0.0,
            None,
        )
    }

    fn timer_snapshot(&self, sequence: u32, tick: u32, progress: f32) -> RawSnapshotMessage {
        let schema = ReplicatedSlotSchema::build(&self.table, &ReplicatedSlotIdentity::default());
        let records = [
            ("player.reloadProgress", progress),
            ("player.reloadActive", 1.0),
        ]
        .into_iter()
        .map(|(name, value)| RawStateSlotRecord {
            slot_id: schema.id_for(name).unwrap().0,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: sequence + 1,
            value: WireSlotValue::Array(vec![0.0, value]),
        })
        .collect();
        RawSnapshotMessage {
            version: wire::SNAPSHOT_VERSION,
            sequence,
            server_tick: tick,
            records: Vec::new(),
            state_schema_fingerprint: *schema.fingerprint(),
            state_records: records,
            faction_sentiment_record: None,
            server_script_time: f64::from(tick) / 60.0,
        }
    }
}

fn descriptor_record() -> RawEntityRecord {
    RawEntityRecord {
        record_kind: wire::RECORD_KIND_FULL_BASELINE,
        network_id: 77,
        baseline_id_or_ref: 1,
        new_baseline_id_or_tombstone_id: 0,
        reason: 0,
        has_last_processed_client_tick: false,
        last_processed_client_tick: 0,
        local_player: false,
        has_entity_class: true,
        entity_class: "player".into(),
        has_active_weapon_archetype: false,
        active_weapon_archetype: String::new(),
        projectile_presentation: None,
        presentation_binding: None,
        components: vec![RawComponentPayload {
            component_kind: wire::COMPONENT_KIND_TRANSFORM,
            transform: Some(WireTransform {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            }),
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
            teleporter: None,
        }],
    }
}

#[test]
fn timed_slot_receive_deferral_and_undecodable_snapshot_do_not_sample() {
    let mut receiver = Receiver::new();
    receiver.slots.advance_timed_slot_targets(Some(10.0), 0.0);
    for (sequence, tick, progress) in [(1, 10, 0.125), (2, 12, 0.25), (3, 14, 0.375)] {
        let snapshot = receiver.timer_snapshot(sequence, tick, progress);
        let outcome = receiver.deliver(wire::encode(&snapshot), &[]);
        assert!(outcome.replicated_state_changed);
        receiver.slots.advance_timed_slot_targets(
            Some(f64::from(tick)),
            if sequence == 1 { 0.0 } else { 2.0 / 60.0 },
        );
    }
    receiver
        .slots
        .advance_timed_slot_targets(Some(15.0), 1.0 / 60.0);
    let before = receiver.slots.timed_slot_debug();
    assert_eq!(before.slots[0].samples, 3);
    assert!(before.slots[0].target.unwrap() > before.slots[0].newest_value.unwrap());

    let signature = |slots: &ClientStateApply| {
        let debug = slots.timed_slot_debug();
        (
            debug.playback_tick,
            debug.slots.map(|slot| {
                (
                    slot.samples,
                    slot.newest_tick,
                    slot.newest_value,
                    slot.target,
                    slot.agreed_slope,
                    slot.fed_tick,
                )
            }),
        )
    };
    let held = signature(&receiver.slots);
    let undecodable = vec![0xff, 0x00, 0xff];
    assert!(crate::decode_snapshot(&undecodable).is_err());
    let corrupt = receiver.deliver(undecodable, &[]);
    assert!(!corrupt.replicated_state_changed);
    assert_eq!(corrupt.owner_private_weapon, Default::default());
    assert_eq!(
        signature(&receiver.slots),
        held,
        "undecodable bytes never become a held-value sample"
    );

    let mut deferred = receiver.timer_snapshot(4, 16, 0.5);
    deferred.records.push(descriptor_record());
    let bytes = wire::encode(&deferred);
    assert!(
        crate::decode_snapshot(&bytes).is_ok(),
        "deferral packet is a valid snapshot, not another decoder rejection"
    );
    let outcome = receiver.deliver(bytes.clone(), &[]);
    assert!(!outcome.replicated_state_changed);
    assert_eq!(outcome.owner_private_weapon, Default::default());
    assert_eq!(
        signature(&receiver.slots),
        held,
        "descriptor-load deferral leaves sample count, history, timeline and target untouched"
    );
    assert_eq!(
        receiver
            .slots
            .weapon_projection()
            .reload_progress
            .unwrap()
            .value,
        0.375
    );

    // Once descriptors exist, the same sequence can commit: deferral did not
    // consume the newest-snapshot gate or create a fictitious timer sample.
    let outcome = receiver.deliver(bytes, &entity_descriptors());
    assert!(outcome.replicated_state_changed);
    assert_eq!(
        outcome.owner_private_weapon.reload_progress.unwrap().value,
        0.5
    );
    let accepted = receiver.slots.timed_slot_debug();
    assert_eq!(accepted.slots[0].samples, 4);
    assert_eq!(accepted.slots[0].newest_tick, Some(16.0));
    assert_eq!(accepted.slots[0].newest_value, Some(0.5));
}
