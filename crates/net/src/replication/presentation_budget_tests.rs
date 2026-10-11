//! AC13: fixed stress roster, maximum admitted curve lengths, production codec
//! and Renet fragmentation/admission. No allocation or byte-size estimates.

use super::*;
use crate::state_slots::{
    FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE, FactionSentimentPair, RawFactionSentimentRecord,
    RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE, WireSlotValue,
};
use crate::transport::{Channel, connection_config};
use crate::wire::{
    MAX_WIRE_CURVE_KEYS, ParticipationFrame, RECORD_KIND_DELTA, RECORD_KIND_DESPAWN,
    RECORD_KIND_FULL_BASELINE, WireEmitterState, WireFogAnimation, WireFogState,
    WireLightAnimation, WireLightState, WireSpinAnimation, WireTransform,
};
use std::collections::VecDeque;
use std::time::Duration;

const CLIENT: u64 = 7;
const LIGHTS: usize = 1278;
const EMITTERS: usize = 40;
const FOG: usize = 6;
const ROSTER: usize = LIGHTS + EMITTERS + FOG;
const HELD_DELTA: u32 = 2001;
const HELD_REFRESH: u32 = 2002;
const HELD_DESPAWN: u32 = 2003;
const LATE_ENEMY: u32 = 2004;
const ONE_WAY_STEPS: usize = 2;
const RTT_STEPS: usize = ONE_WAY_STEPS * 2;

fn scalar_curve(low: f32, high: f32) -> Vec<f32> {
    (0..MAX_WIRE_CURVE_KEYS)
        .map(|i| {
            let t = i as f32 / (MAX_WIRE_CURVE_KEYS - 1) as f32;
            low + (high - low) * t * t
        })
        .collect()
}

fn stress_roster() -> Vec<EntitySnapshot> {
    (0..ROSTER)
        .map(|index| {
            let (kind, ordinal, component) = if index < LIGHTS {
                let state = WireLightState {
                    intensity: 2.0 + index as f32 / LIGHTS as f32,
                    color: [0.25, 0.5, 0.75],
                    cone_direction: Some([0.0, -1.0, 0.0]),
                    animation: Some(WireLightAnimation {
                        period_ms: 5000.0,
                        phase: Some(0.25),
                        play_count: Some(2),
                        start_active: Some(true),
                        brightness: Some(scalar_curve(0.25, 1.0)),
                        color: Some(
                            scalar_curve(0.0, 1.0)
                                .into_iter()
                                .map(|t| [t, 1.0 - t, 0.5])
                                .collect(),
                        ),
                        direction: Some(
                            scalar_curve(0.0, 1.0)
                                .into_iter()
                                .map(|t| [t.sin(), -t.cos(), 0.0])
                                .collect(),
                        ),
                    }),
                    anim_start: Some(2.0),
                };
                state.validate().unwrap();
                (0, index, ComponentPayload::LightState(state))
            } else if index < LIGHTS + EMITTERS {
                let state = WireEmitterState {
                    rate: 80.0 + (index - LIGHTS) as f32,
                    spin_rate: 0.5,
                    spin_animation: Some(WireSpinAnimation {
                        duration: 5.0,
                        rate_curve: scalar_curve(0.5, 3.0),
                    }),
                    anim_start: Some(2.0),
                };
                state.validate().unwrap();
                (2, index - LIGHTS, ComponentPayload::EmitterState(state))
            } else {
                let state = WireFogState {
                    density: 0.1,
                    glow: 0.2,
                    edge_softness: 0.5,
                    falloff: 1.0,
                    tint: [0.25, 0.5, 0.75],
                    saturation: 0.8,
                    min_brightness: 0.1,
                    light_range: 8.0,
                    animation: Some(WireFogAnimation {
                        period_ms: 5000.0,
                        phase: None,
                        play_count: Some(1),
                        density: Some(scalar_curve(0.1, 0.6)),
                        saturation: Some(scalar_curve(0.8, 1.0)),
                        min_brightness: Some(scalar_curve(0.1, 0.3)),
                        light_range: Some(scalar_curve(8.0, 4.0)),
                    }),
                    anim_start: Some(2.0),
                };
                state.validate().unwrap();
                (
                    1,
                    index - LIGHTS - EMITTERS,
                    ComponentPayload::FogState(state),
                )
            };
            let mut snapshot = EntitySnapshot::unowned(index as u32 + 1, vec![component]);
            snapshot.presentation_binding = Some(WirePresentationBinding {
                kind,
                index: ordinal as u32,
                origin: [index as f32 * 0.25, 2.0, 3.0],
            });
            snapshot
        })
        .collect()
}

fn enemy(network_id: u32, position: f32) -> EntitySnapshot {
    let mut snapshot = EntitySnapshot::unowned(
        network_id,
        vec![ComponentPayload::Transform(WireTransform {
            position: [position, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        })],
    );
    snapshot.entity_class = Some("dev.reference_enemy".into());
    snapshot
}

fn ack(server: &mut ServerReplication, raw: &RawSnapshotMessage) {
    let baselines: Vec<_> = raw
        .records
        .iter()
        .filter_map(|r| match r.record_kind {
            RECORD_KIND_FULL_BASELINE => Some((r.network_id, r.baseline_id_or_ref)),
            RECORD_KIND_DELTA => Some((r.network_id, r.new_baseline_id_or_tombstone_id)),
            _ => None,
        })
        .collect();
    let tombstones: Vec<_> = raw
        .records
        .iter()
        .filter(|r| r.record_kind == RECORD_KIND_DESPAWN)
        .map(|r| (r.network_id, r.new_baseline_id_or_tombstone_id))
        .collect();
    server.apply_ack(CLIENT, raw.sequence, &baselines, &tombstones);
}

/// Match the host's final envelope splice. This declared workload carries eight
/// shared scalar slots and four live faction pairs alongside presentation state.
fn complete_envelope(raw: &mut RawSnapshotMessage) {
    raw.server_script_time = f64::from(raw.server_tick) / 60.0;
    raw.state_schema_fingerprint = [0x5a; 32];
    raw.state_records = (0..8)
        .map(|slot_id| RawStateSlotRecord {
            slot_id,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: 1,
            value: WireSlotValue::Number(f32::from(slot_id) + 0.5),
        })
        .collect();
    raw.faction_sentiment_record = Some(RawFactionSentimentRecord {
        kind: FACTION_SENTIMENT_RECORD_KIND_FULL_BASELINE,
        has_baseline_ref: false,
        baseline_ref: 0,
        baseline_id: 1,
        pairs: (0..4)
            .map(|index| FactionSentimentPair {
                from_idx: index,
                to_idx: index + 1,
                value: -0.75,
            })
            .collect(),
    });
    raw.validate().unwrap();
}

#[test]
fn presentation_budget_1324_max_curves_converge_and_transport_admits_full_envelopes() {
    assert_eq!(ROSTER, 1324);
    let roster = stress_roster();
    assert_eq!(
        roster
            .iter()
            .filter(|r| r.presentation_binding.unwrap().kind == 0)
            .count(),
        LIGHTS
    );
    assert_eq!(
        roster
            .iter()
            .filter(|r| r.presentation_binding.unwrap().kind == 2)
            .count(),
        EMITTERS
    );
    assert_eq!(
        roster
            .iter()
            .filter(|r| r.presentation_binding.unwrap().kind == 1)
            .count(),
        FOG
    );
    let mut tracker = ServerReplication::new();
    tracker.register_client(CLIENT);
    let mut held = vec![
        enemy(HELD_DELTA, 0.0),
        enemy(HELD_REFRESH, 1.0),
        enemy(HELD_DESPAWN, 2.0),
    ];
    tracker.ingest_tick(held.clone());
    let initial = tracker.encode_for_client(CLIENT, 0).unwrap();
    ack(&mut tracker, &initial);
    let mut all = roster.clone();
    all.extend(held.clone());
    tracker.ingest_tick(all);

    let config = connection_config();
    let allowance = config.available_bytes_per_tick as usize;
    let mut transport = renet::RenetServer::new(config);
    let mut peer = transport.new_local_client(CLIENT);
    let mut arrivals = VecDeque::new();
    let mut acknowledgements = VecDeque::new();
    let mut sent = HashSet::new();
    let mut received = HashSet::new();
    let mut total_baseline_bytes = 0;
    let mut maximum_subtotal = 0;
    let mut maximum_snapshot = 0;
    let mut maximum_carrier = 0;
    let mut maximum_packets = 0;
    let mut converged = None;
    for step in 0..1024 {
        while acknowledgements
            .front()
            .is_some_and(|(due, _)| *due <= step)
        {
            let (_, raw) = acknowledgements.pop_front().unwrap();
            ack(&mut tracker, &raw);
        }
        while arrivals.front().is_some_and(|(due, _)| *due <= step) {
            let (_, raw): (_, RawSnapshotMessage) = arrivals.pop_front().unwrap();
            received.extend(
                raw.records
                    .iter()
                    .filter(|r| (1..=ROSTER as u32).contains(&r.network_id))
                    .map(|r| r.network_id),
            );
            acknowledgements.push_back((step + ONE_WAY_STEPS, raw));
        }
        if received.len() == ROSTER {
            converged = Some(step);
            break;
        }
        if step == 1 {
            held[0] = enemy(HELD_DELTA, 9.0);
            held.pop();
            tracker.request_refresh(CLIENT, HELD_REFRESH, 1);
            let mut all = roster.clone();
            all.extend(held.clone());
            all.push(enemy(LATE_ENEMY, 3.0));
            tracker.ingest_tick(all);
        }
        let mut raw = tracker.encode_for_client(CLIENT, step as u32 * 2).unwrap();
        if step == 1 {
            assert_eq!(raw.records[0].network_id, HELD_DESPAWN);
            assert_eq!(raw.records[0].record_kind, RECORD_KIND_DESPAWN);
            assert!(
                raw.records
                    .iter()
                    .any(|r| r.network_id == HELD_DELTA && r.record_kind == RECORD_KIND_DELTA)
            );
            assert!(raw.records.iter().any(
                |r| r.network_id == HELD_REFRESH && r.record_kind == RECORD_KIND_FULL_BASELINE
            ));
            let late = raw
                .records
                .iter()
                .position(|r| r.network_id == LATE_ENEMY)
                .unwrap();
            let bound = raw
                .records
                .iter()
                .position(|r| r.presentation_binding.is_some())
                .unwrap();
            assert!(late < bound, "late gameplay entity must pass bound backlog");
        }
        let pending: Vec<_> = raw
            .records
            .iter()
            .filter(|r| r.record_kind == RECORD_KIND_FULL_BASELINE && r.network_id != HELD_REFRESH)
            .collect();
        let subtotal: usize = pending
            .iter()
            .skip(1)
            .map(|r| crate::wire::encode(*r).len())
            .sum();
        assert!(subtotal <= JOIN_BASELINE_BUDGET_BYTES);
        maximum_subtotal = maximum_subtotal.max(subtotal);
        for record in raw
            .records
            .iter()
            .filter(|r| r.presentation_binding.is_some())
        {
            assert_eq!(record.record_kind, RECORD_KIND_FULL_BASELINE);
            assert!(
                sent.insert(record.network_id),
                "in-flight baseline repeated before acknowledgement"
            );
            total_baseline_bytes += crate::wire::encode(record).len();
        }
        complete_envelope(&mut raw);
        let bytes = crate::wire::encode(&raw);
        maximum_snapshot = maximum_snapshot.max(bytes.len());
        let carrier = crate::wire::encode(&ParticipationFrame {
            participation_epoch: 1,
            payload: bytes,
        });
        maximum_carrier = maximum_carrier.max(carrier.len());
        assert!(
            carrier.len() <= allowance,
            "full carrier exceeds Renet's per-tick admission allowance"
        );
        transport.update(Duration::from_millis(34));
        peer.update(Duration::from_millis(34));
        transport.send_message(CLIENT, Channel::Snapshot as u8, carrier.clone());
        let packets = transport.get_packets_to_send(CLIENT).unwrap();
        maximum_packets = maximum_packets.max(packets.len());
        for packet in packets {
            peer.process_packet(&packet);
        }
        assert_eq!(
            peer.receive_message(Channel::Snapshot as u8).as_deref(),
            Some(carrier.as_slice()),
            "Renet must deliver the whole fragmented carrier"
        );
        for packet in peer.get_packets_to_send() {
            transport.process_packet_from(&packet, CLIENT).unwrap();
        }
        arrivals.push_back((step + ONE_WAY_STEPS, raw));
    }
    let converged = converged.expect("stress roster failed to converge");
    let limit = total_baseline_bytes.div_ceil(JOIN_BASELINE_BUDGET_BYTES) + 2 + RTT_STEPS;
    assert!(
        converged <= limit,
        "converged at {converged}, limit {limit}"
    );
    assert_eq!(sent.len(), ROSTER);
    assert_eq!(received.len(), ROSTER);
    // Drain final acknowledgements, then retire registrations and flights.
    for (_, raw) in acknowledgements.into_iter().chain(arrivals) {
        ack(&mut tracker, &raw);
    }
    assert!(
        tracker
            .encode_for_client(CLIENT, 4096)
            .unwrap()
            .records
            .is_empty()
    );
    for network_id in sent
        .iter()
        .copied()
        .chain([HELD_DELTA, HELD_REFRESH, LATE_ENEMY])
    {
        tracker.forget_entity_without_tombstone(network_id);
    }
    tracker.remove_client(CLIENT);
    assert!(tracker.entities.is_empty());
    assert!(tracker.tombstones.is_empty());
    assert!(!tracker.has_registered_clients());
    transport.remove_connection(CLIENT);
    println!(
        "AC13 roster={LIGHTS}+{EMITTERS}+{FOG}={ROSTER} keys={MAX_WIRE_CURVE_KEYS} baseline_bytes={total_baseline_bytes} max_budget_subtotal={maximum_subtotal} max_snapshot_bytes={maximum_snapshot} max_participation_carrier_bytes={maximum_carrier} max_renet_packets={maximum_packets} admission_bytes_per_tick={allowance} convergence_snapshots={converged} bound_snapshots={limit} one_way_snapshots={ONE_WAY_STEPS}"
    );
}
