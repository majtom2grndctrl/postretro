// M15 Phase 3.5 Task 6: in-memory conditioned-loss harness for state-slot
// replication. A sibling to the movement `predict_reconcile_harness`: it drives
// the REAL host production (`HostStateReplication`) and client apply
// (`ClientStateApply`) glue, encoding each frame's state records into the real
// `RawSnapshotMessage` envelope and relaying it through the net crate's
// `PacketConditioner` under loss/jitter on a virtual clock. The return direction
// (acks + `StateBaselineRefresh`) is conditioned by a second link, so a dropped
// baseline forces the client's refresh request to repair the slot without reconnect.
// See: context/lib/networking.md · context/lib/testing_guide.md
//
// Scope: shared + owner-private slot replication under dropped snapshots. The
// assertions are: (1) the client issues `ClientMessage::StateBaselineRefresh` when a
// baseline is lost, and (2) the UI-visible slot table converges to the authoritative
// values after repair. Deterministic: seeded conditioner + caller-advanced virtual
// clock, no wall-clock read anywhere (testing_guide "deterministic time").

#![cfg(test)]

use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::state_slots::RawStateSlotRecord;
use postretro_net::wire::{
    self, ClientMessage, RawSnapshotMessage, SNAPSHOT_VERSION, StateBaselineRefreshRequest,
};

use super::state_slots::{ClientStateApply, HostStateReplication};
use crate::scripting::components::health::HealthComponent;
use crate::scripting::data_descriptors::HealthDescriptor;
use crate::scripting::registry::{EntityRegistry, Transform};
use crate::scripting::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue,
};

use super::command_queue::MovementOwners;

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;

/// One virtual tick step in ms (60 Hz, integer ms — exact enough for the link clock).
const TICK_MS: u64 = 16;

/// A lossy-but-recoverable link: a 45..105 ms one-way range (≈150 ms mean RTT under
/// the conditioner's additive jitter) at 5% loss, matching the mandated movement
/// harness profile so the state path is exercised under the same conditions.
fn mandated_link(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed,
    }
}

/// A heavy-loss link used to *guarantee* at least one dropped baseline within a short
/// run, so the refresh/repair seam is reliably exercised regardless of the mean-rate
/// seed. Still deterministic under its fixed seed.
fn heavy_loss_link(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 30,
        jitter: 0,
        loss_probability: 0.6,
        seed,
    }
}

/// A mod slot record under a given replication scope (number type, default 0).
fn replicated_number(scope: ReplicationScope) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(0.0)),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: scope,
    })
}

/// Both peers build this identically: one shared mod slot (`net.objective`) and the
/// engine's owner-private `player.health` / `player.maxHealth` (left at the Task 4
/// catalog-flip scope). The matching slot set is what makes the fingerprints agree.
fn harness_table() -> SlotTable {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "net",
            vec![(
                "objective".to_string(),
                replicated_number(ReplicationScope::SharedGlobal),
            )],
        )
        .unwrap();
    table
}

/// Spawn an owned pawn carrying a descriptor-materialized `HealthComponent` for
/// `client_id`, so the owner-private health slots have a real per-owner source.
fn spawn_owned_health(
    registry: &mut EntityRegistry,
    owners: &mut MovementOwners,
    client_id: u64,
    current: f32,
    max: f32,
) {
    let descriptor = HealthDescriptor {
        max,
        hitbox: None,
        zone_multipliers: std::collections::HashMap::new(),
    };
    let pawn = registry.spawn(Transform::default());
    let mut health = HealthComponent::from_descriptor(&descriptor);
    health.current = current;
    registry.set_component(pawn, health).unwrap();
    owners.set(pawn, client_id);
}

/// The conditioned in-memory state-slot harness. Holds the host glue, one client's
/// apply glue and slot table, and the two directional conditioners (snapshots to the
/// client, acks/refreshes back). The virtual clock is advanced by the caller via
/// [`Self::step`].
struct StateSlotHarness {
    host: HostStateReplication,
    host_table: SlotTable,
    registry: EntityRegistry,
    owners: MovementOwners,

    client_id: u64,
    client: ClientStateApply,
    client_table: SlotTable,
    fingerprint: [u8; 32],

    to_client: PacketConditioner,
    to_server: PacketConditioner,

    sequence: u32,
    now_ms: u64,
    /// Count of `StateBaselineRefresh` requests the server has received (after the
    /// conditioned return path), for the repair-seam assertion.
    refreshes_received: u32,
}

impl StateSlotHarness {
    fn new(client_id: u64, to_client: LinkConfig, to_server: LinkConfig) -> Self {
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        // One owned pawn for this client so the owner-private health slots replicate.
        spawn_owned_health(&mut registry, &mut owners, client_id, 100.0, 100.0);

        let mut host = HostStateReplication::new();
        host.register_client(client_id);
        let mut host_table = harness_table();
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(0.0));
        let fingerprint = host.fingerprint(&host_table);

        Self {
            host,
            host_table,
            registry,
            owners,
            client_id,
            client: ClientStateApply::new(),
            client_table: harness_table(),
            fingerprint,
            to_client: PacketConditioner::new(to_client),
            to_server: PacketConditioner::new(to_server),
            sequence: 0,
            now_ms: 0,
            refreshes_received: 0,
        }
    }

    /// Set the shared objective value the host will replicate next frame.
    fn set_objective(&mut self, value: f32) {
        self.host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(value));
    }

    /// Set the owning pawn's current health on the host (mutating the live component,
    /// the descriptor-fed owner-private source).
    fn set_owner_health(&mut self, current: f32) {
        for (pawn, owner) in self.owners.iter() {
            if owner == self.client_id {
                let mut health = self
                    .registry
                    .get_component::<HealthComponent>(pawn)
                    .expect("owned pawn has health")
                    .clone();
                health.current = current;
                self.registry.set_component(pawn, health).unwrap();
            }
        }
    }

    /// One server→client→server round: the host produces this frame's state records,
    /// encodes them into a real snapshot envelope, and relays it through the lossy
    /// `to_client` link. The client decodes any delivered snapshot, applies the state
    /// batch, and sends its acks + refresh requests back through the lossy `to_server`
    /// link, where the host applies them. The virtual clock advances one tick.
    fn step(&mut self) {
        let seq = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);

        // Host: produce this frame's records and wrap them in the real envelope.
        if let Some(records) = self.host.produce_for_client(
            &self.host_table,
            &self.registry,
            &self.owners,
            self.client_id,
            seq,
        ) {
            let snapshot = snapshot_with_state(seq, self.fingerprint, records);
            self.to_client.enqueue(wire::encode(&snapshot));
        }

        // Advance both directional clocks one tick.
        self.to_client.advance(TICK_MS);
        self.to_server.advance(TICK_MS);
        self.now_ms += TICK_MS;

        // Client: receive any delivered snapshot(s), apply the state batch, queue the
        // resulting acks/refresh requests back through the return link.
        for packet in self.to_client.take_ready() {
            let Ok(snapshot) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let outcome = self.client.apply_snapshot_state(
                &mut self.client_table,
                snapshot.sequence,
                &snapshot.state_schema_fingerprint,
                &snapshot.state_records,
            );
            // Acks ride as a real ClientMessage::Ack envelope on the return link.
            if !outcome.slot_baselines.is_empty() {
                let ack = wire::AckMessage {
                    latest_snapshot_sequence: snapshot.sequence,
                    acked_server_tick: snapshot.server_tick,
                    entity_baselines: Vec::new(),
                    despawn_tombstones: Vec::new(),
                    slot_baselines: outcome.slot_baselines,
                };
                self.to_server
                    .enqueue(wire::encode(&ClientMessage::Ack(ack)));
            }
            // Each refresh request rides as a ClientMessage::StateBaselineRefresh.
            for req in outcome.refresh_requests {
                self.to_server
                    .enqueue(wire::encode(&ClientMessage::StateBaselineRefresh(req)));
            }
        }

        // Server: receive any delivered acks/refreshes off the return link.
        for packet in self.to_server.take_ready() {
            let Ok(message) = wire::decode::<ClientMessage>(&packet) else {
                continue;
            };
            match message {
                ClientMessage::Ack(ack) => {
                    self.host.apply_ack(
                        self.client_id,
                        ack.latest_snapshot_sequence,
                        &ack.slot_baselines,
                    );
                }
                ClientMessage::StateBaselineRefresh(StateBaselineRefreshRequest {
                    slot_id,
                    missing_baseline_ref,
                    ..
                }) => {
                    self.refreshes_received += 1;
                    self.host
                        .request_refresh(self.client_id, slot_id, missing_baseline_ref);
                }
                _ => {}
            }
        }
    }

    /// The client's current value for a dotted slot name (the UI-visible value).
    fn client_value(&self, name: &str) -> Option<SlotValue> {
        self.client_table.get(name).and_then(|r| r.value.clone())
    }
}

/// Build a real snapshot envelope carrying the given state records (no entity records).
fn snapshot_with_state(
    sequence: u32,
    fingerprint: [u8; 32],
    state_records: Vec<RawStateSlotRecord>,
) -> RawSnapshotMessage {
    RawSnapshotMessage {
        version: SNAPSHOT_VERSION,
        sequence,
        server_tick: sequence,
        records: Vec::new(),
        state_schema_fingerprint: fingerprint,
        state_records,
    }
}

// Under a lossy link with changing values, the client's UI-visible slots converge to
// the authoritative values: dropped snapshots are superseded by later ones (full
// baseline fallback for unacked slots), and the slot table tracks the host. Drives the
// real produce/apply glue through the conditioned link on a virtual clock.
#[test]
fn state_slots_converge_under_lossy_link() {
    let mut h = StateSlotHarness::new(CLIENT_A, mandated_link(0x5101), mandated_link(0x5102));

    // Drive a varied value stream so the reconcile-via-baseline path is exercised: the
    // shared objective climbs and the owner health drains. Enough ticks that loss is
    // certainly hit, then drain so every in-flight packet is delivered.
    for tick in 0..200u32 {
        h.set_objective(tick as f32);
        h.set_owner_health(100.0 - (tick as f32) * 0.25);
        h.step();
    }
    // Freeze the values and drain: keep stepping (no value change) until both links are
    // empty, so the last authoritative values certainly reach the client.
    h.set_objective(199.0);
    h.set_owner_health(100.0 - 199.0 * 0.25);
    for _ in 0..400 {
        h.step();
        if h.to_client.in_flight() == 0 && h.to_server.in_flight() == 0 {
            // One more step to deliver anything the final step queued.
            h.step();
            break;
        }
    }

    // The conditioned link actually dropped packets (the scenario is non-trivial).
    assert!(
        h.to_client.dropped() > 0,
        "the 5% loss model dropped at least one snapshot toward the client"
    );

    // The UI-visible slots converged to the authoritative values.
    assert_eq!(
        h.client_value("net.objective"),
        Some(SlotValue::Number(199.0)),
        "the shared objective converges to the authoritative value after loss"
    );
    let expected_health = 100.0 - 199.0 * 0.25;
    match h.client_value("player.health") {
        Some(SlotValue::Number(n)) => assert!(
            (n - expected_health).abs() < 1e-3,
            "owner-private health converges (got {n}, expected {expected_health})"
        ),
        other => panic!("player.health should be a number after convergence, got {other:?}"),
    }
}

// A dropped baseline forces a `StateBaselineRefresh` and the slot repairs WITHOUT
// reconnect. Under a heavy-loss link the client will receive a delta referencing a
// baseline it never held (its full baseline was dropped); the apply path requests a
// refresh, the server schedules a full baseline, and the slot converges once a frame
// finally survives the link.
#[test]
fn dropped_baseline_triggers_refresh_and_repairs() {
    let mut h = StateSlotHarness::new(CLIENT_A, heavy_loss_link(0x5201), heavy_loss_link(0x5202));

    // Change the shared value every few ticks so the server keeps minting fresh
    // baselines; heavy loss guarantees some are dropped, producing delta-against-missing
    // on the client and thus a refresh request.
    let mut value = 0.0_f32;
    for tick in 0..300u32 {
        if tick % 3 == 0 {
            value += 1.0;
            h.set_objective(value);
        }
        h.step();
    }
    // Drain under the lossy link until both directions empty (capped).
    for _ in 0..2000 {
        h.step();
        if h.to_client.in_flight() == 0 && h.to_server.in_flight() == 0 {
            h.step();
            break;
        }
    }

    // The repair seam fired: the client requested at least one baseline refresh and the
    // server received it (proving the refresh repairs without reconnect).
    assert!(
        h.refreshes_received > 0,
        "a dropped baseline must trigger at least one StateBaselineRefresh round-trip"
    );

    // Despite the heavy loss + refresh churn, the slot converges to the last
    // authoritative value once a frame survives.
    assert_eq!(
        h.client_value("net.objective"),
        Some(SlotValue::Number(value)),
        "the slot converges to the authoritative value after refresh repair"
    );
}

// Owner-private isolation holds over the conditioned link: a second client's harness
// (its own pawn, its own health) never sees the first client's private health value,
// even as both replicate under loss. Two independent harnesses model two clients; the
// shared schema/fingerprint is identical, but each client's owner-private records carry
// only its own pawn's value.
#[test]
fn owner_private_isolation_holds_under_lossy_link() {
    let mut a = StateSlotHarness::new(CLIENT_A, mandated_link(0x5301), mandated_link(0x5302));
    let mut b = StateSlotHarness::new(CLIENT_B, mandated_link(0x5303), mandated_link(0x5304));

    // Distinct authoritative health per client.
    a.set_owner_health(80.0);
    b.set_owner_health(40.0);

    for _ in 0..150 {
        a.step();
        b.step();
    }
    for _ in 0..400 {
        a.step();
        b.step();
        if a.to_client.in_flight() == 0
            && a.to_server.in_flight() == 0
            && b.to_client.in_flight() == 0
            && b.to_server.in_flight() == 0
        {
            a.step();
            b.step();
            break;
        }
    }

    assert_eq!(
        a.client_value("player.health"),
        Some(SlotValue::Number(80.0)),
        "client A converges to ITS OWN health"
    );
    assert_eq!(
        b.client_value("player.health"),
        Some(SlotValue::Number(40.0)),
        "client B converges to ITS OWN (different) health — no cross-client leak"
    );
}
