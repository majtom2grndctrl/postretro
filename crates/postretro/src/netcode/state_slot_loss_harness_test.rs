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

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::state_slots::RawStateSlotRecord;
use postretro_net::wire::{
    self, ClientMessage, RawSnapshotMessage, SNAPSHOT_VERSION, StateBaselineRefreshRequest,
};

use super::state_slots::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    AmmoReserve, EntityId, EntityRegistry, ReplicationScope, ScriptCtx, SlotOwnership, SlotRecord,
    SlotSchema, SlotTable, SlotType, SlotValue, Transform,
};
use postretro_foundation::{
    AmmoResource, FireMode, HealthDescriptor, IrNode, ProjectileBodyVisual, ProjectileDescriptor,
    ProjectileVisual, ReloadStyle, ResolutionMode, WeaponDescriptor, WeaponResource,
};
use postretro_scripting_core::StoreIdentityLedger;

use crate::scripting_systems::slot_accumulators::{
    SlotAccumulatorBindings, evaluate_slot_accumulators,
};

use super::command_queue::{MovementOwners, WeaponOwners};
use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::ProjectileLaunch;

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
fn replicated_number(scope: ReplicationScope, accumulate: Option<IrNode>) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(0.0)),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: scope,
        per_owner: false,
        accumulate,
    })
}

/// Both peers build this identically: one shared mod slot (`net.objective`) and the
/// engine's owner-private `player.health` / `player.maxHealth` (left at the Task 4
/// catalog-flip scope). The matching slot set is what makes the fingerprints agree.
fn harness_table(accumulate: Option<IrNode>) -> SlotTable {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "net",
            vec![(
                "objective".to_string(),
                replicated_number(ReplicationScope::SharedGlobal, accumulate),
            )],
        )
        .unwrap();
    table
}

fn harness_replication_identity() -> ReplicatedSlotIdentity<'static> {
    ReplicatedSlotIdentity::new(
        Some("test.state-slot-loss".to_string()),
        Some(StoreIdentityLedger {
            version: 1,
            slots: [("net.objective".to_string(), "k0123456789abcdef".to_string())]
                .into_iter()
                .collect(),
        }),
        ["net.objective".to_string()].into_iter().collect(),
    )
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

/// Materialize two ammunition-backed weapons on an owned pawn. The harness changes
/// their live magazine/reserve values below to model the host values seeded from a
/// carried seat; only the owner-private state-slot projection crosses the link.
fn spawn_owned_ammo_weapons(registry: &mut EntityRegistry, pawn: EntityId) -> (EntityId, EntityId) {
    let weapon = |ammo_type: &str| {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: ammo_type.to_string(),
                magazine: 30,
                cost_per_shot: 1,
                reserve: 0,
                reload_ms: 1000,
                reload_style: ReloadStyle::Magazine,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: Some(true),
        })
    };
    let shells = registry.spawn(Transform::default());
    let cells = registry.spawn(Transform::default());
    registry.set_component(shells, weapon("shells")).unwrap();
    registry.set_component(cells, weapon("cells")).unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(shells);
    inventory.wieldables[1] = Some(cells);
    registry.set_component(pawn, inventory).unwrap();
    registry.set_component(pawn, AmmoReserve::new()).unwrap();
    (shells, cells)
}

/// The conditioned in-memory state-slot harness. Holds the host glue, one client's
/// apply glue and slot table, and the two directional conditioners (snapshots to the
/// client, acks/refreshes back). The virtual clock is advanced by the caller via
/// [`Self::step`].
struct StateSlotHarness {
    host: HostStateReplication,
    replication_identity: ReplicatedSlotIdentity<'static>,
    host_ctx: ScriptCtx,
    registry: EntityRegistry,
    owners: MovementOwners,
    weapon_owners: WeaponOwners,
    owner_pawn: EntityId,
    shell_weapon: EntityId,
    cell_weapon: EntityId,

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
    /// The client-to-host path is shared with combat declarations. Keeping this
    /// explicit lets the co-op projectile test prove ordinary Health replication
    /// did not require a client-authored HIT message.
    hit_declarations_received: u32,
}

impl StateSlotHarness {
    fn new(client_id: u64, to_client: LinkConfig, to_server: LinkConfig) -> Self {
        Self::new_with_accumulator(client_id, to_client, to_server, None)
    }

    fn new_with_accumulator(
        client_id: u64,
        to_client: LinkConfig,
        to_server: LinkConfig,
        accumulate: Option<IrNode>,
    ) -> Self {
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        // One owned pawn for this client so the owner-private health slots replicate.
        spawn_owned_health(&mut registry, &mut owners, client_id, 100.0, 100.0);
        let owner_pawn = owners
            .iter()
            .find_map(|(pawn, owner)| (owner == client_id).then_some(pawn))
            .expect("fixture registered its owner pawn");
        let (shell_weapon, cell_weapon) = spawn_owned_ammo_weapons(&mut registry, owner_pawn);

        let mut host = HostStateReplication::new();
        host.register_client(client_id);
        let host_ctx = ScriptCtx::new();
        *host_ctx.slot_table.borrow_mut() = harness_table(accumulate.clone());
        host_ctx
            .slot_table
            .borrow_mut()
            .get_mut("net.objective")
            .unwrap()
            .value = Some(SlotValue::Number(0.0));
        let replication_identity = harness_replication_identity();
        let fingerprint = host.fingerprint(&host_ctx.slot_table.borrow(), &replication_identity);

        Self {
            host,
            replication_identity,
            host_ctx,
            registry,
            owners,
            weapon_owners: WeaponOwners::new(),
            owner_pawn,
            shell_weapon,
            cell_weapon,
            client_id,
            client: ClientStateApply::new(),
            client_table: harness_table(accumulate),
            fingerprint,
            to_client: PacketConditioner::new(to_client),
            to_server: PacketConditioner::new(to_server),
            sequence: 0,
            now_ms: 0,
            refreshes_received: 0,
            hit_declarations_received: 0,
        }
    }

    /// Set the shared objective value the host will replicate next frame.
    fn set_objective(&mut self, value: f32) {
        self.host_ctx
            .slot_table
            .borrow_mut()
            .get_mut("net.objective")
            .unwrap()
            .value = Some(SlotValue::Number(value));
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

    /// Set the authoritative equipped weapon's values on the host. This deliberately
    /// writes components only: the assertion below proves `AmmoSlotProjection` feeds
    /// `ClientStateApply`, rather than a HUD or a client-local weapon component.
    fn set_host_ammo(&mut self, active_slot: usize, magazine: u32, reserve: u32) {
        let weapon = match active_slot {
            0 => self.shell_weapon,
            1 => self.cell_weapon,
            other => panic!("fixture has no ammo weapon in slot {other}"),
        };
        let ammo_type = match active_slot {
            0 => "shells",
            1 => "cells",
            _ => unreachable!(),
        };
        let mut inventory = self
            .registry
            .get_component::<Inventory>(self.owner_pawn)
            .expect("fixture pawn has inventory")
            .clone();
        inventory.active_slot = active_slot;
        self.registry
            .set_component(self.owner_pawn, inventory)
            .unwrap();

        let mut component = self
            .registry
            .get_component::<WeaponComponent>(weapon)
            .expect("fixture weapon keeps its component")
            .clone();
        component.magazine = magazine;
        self.registry.set_component(weapon, component).unwrap();

        let mut balances = self
            .registry
            .get_component::<AmmoReserve>(self.owner_pawn)
            .expect("fixture pawn keeps its reserve")
            .clone();
        balances.set_exact(ammo_type, reserve);
        self.registry
            .set_component(self.owner_pawn, balances)
            .unwrap();
    }

    /// A level transition rebuilds the state-slot schema and resets the client apply
    /// state. The host pawn is already seeded with carried values by the time its next
    /// snapshot flushes, which is the boundary Task 8 verifies.
    fn level_changed(&mut self) {
        self.host.reset_schema_for_clients([self.client_id]);
        self.client.reset_schema();
        self.client_table = harness_table(None);
        self.fingerprint = self.host.fingerprint(
            &self.host_ctx.slot_table.borrow(),
            &self.replication_identity,
        );
    }

    /// A rejoin drops the old participation baseline and creates a fresh client apply
    /// state, while the host-owned pawn and its carried values remain authoritative.
    fn client_rejoined(&mut self) {
        self.host.remove_client(self.client_id);
        self.host.register_client(self.client_id);
        self.client = ClientStateApply::new();
        self.client_table = harness_table(None);
    }

    /// One server→client→server round: the host produces this frame's state records,
    /// encodes them into a real snapshot envelope, and relays it through the lossy
    /// `to_client` link. The client decodes any delivered snapshot, applies the state
    /// batch, and sends its acks + refresh requests back through the lossy `to_server`
    /// link, where the host applies them. The virtual clock advances one tick.
    fn step(&mut self) {
        let seq = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);

        // Host: ingest this frame's authoritative values once, then produce this
        // client's records and wrap them in the real envelope.
        self.host.ingest_frame(
            &self.host_ctx.slot_table.borrow(),
            &self.replication_identity,
            &self.registry,
            &self.owners,
            &self.weapon_owners,
        );
        if let Some(records) = self.host.produce_for_client(self.client_id, seq) {
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
                &self.replication_identity,
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
                ClientMessage::HitDeclaration(_) => {
                    self.hit_declarations_received += 1;
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

// E16 Task 6: the connected client's pawn is hit by a host-only enemy
// projectile. The authoritative impact changes Health first; the existing
// owner-private Health projection then carries that result to the client. No
// client prediction, AuthorizedShot, or HitDeclaration participates.
#[test]
fn enemy_projectile_damages_connected_pawn_through_host_health_replication() {
    let direct = LinkConfig {
        delay: 0,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0xe16_0601,
    };
    let mut h = StateSlotHarness::new(CLIENT_A, direct, direct);
    let mut target_transform = *h
        .registry
        .get_component::<Transform>(h.owner_pawn)
        .expect("connected pawn has a transform");
    target_transform.position = Vec3::X;
    h.registry
        .set_component(h.owner_pawn, target_transform)
        .expect("connected pawn remains live");
    let mut target_health = h
        .registry
        .get_component::<HealthComponent>(h.owner_pawn)
        .expect("connected pawn has health")
        .clone();
    target_health.hitbox = Some(Hitbox {
        half_extents: Vec3::splat(0.25),
        offset: Vec3::ZERO,
    });
    h.registry
        .set_component(h.owner_pawn, target_health)
        .expect("connected pawn accepts its host hitbox");

    let enemy = h.registry.spawn(Transform::default());
    let projectile = crate::sim::spawn_projectile(
        &mut h.registry,
        enemy,
        enemy,
        ProjectileLaunch {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 4.0,
            radius: 0.1,
            range: 8.0,
            lifetime: 2.0,
            damage: 10.0,
            credit_source: "enemy.rifle".to_string(),
            descriptor: ProjectileDescriptor {
                speed: 4.0,
                radius: 0.1,
                lifetime_ms: 2_000.0,
                visual: ProjectileVisual {
                    body: ProjectileBodyVisual::Sprite {
                        sprite: "sprites/projectiles/enemy-bolt.png".to_string(),
                        size: 0.2,
                        opacity: 1.0,
                        rotation: 0.0,
                        tint: [1.0, 0.2, 0.1],
                        emissive: 0.0,
                        frame_duration_ms: None,
                    },
                    trail: None,
                    light: None,
                    impact_light: None,
                },
            },
        },
        None,
    )
    .expect("host enemy projectile has capacity to spawn");
    assert_eq!(
        h.registry
            .get_component::<postretro_entities::components::projectile::ProjectileComponent>(
                projectile,
            )
            .expect("spawned projectile has common gameplay state")
            .predicted_shot_id,
        None,
        "an enemy flight has no client-authorized shot identity"
    );

    let registry = Rc::new(RefCell::new(std::mem::replace(
        &mut h.registry,
        EntityRegistry::new(),
    )));
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut ignored_impact_effects = |_: &mut EntityRegistry| {};
    assert!(
        crate::sim::advance(
            &registry,
            &world,
            &hit_zones,
            0.0,
            1.0 / 60.0,
            &mut ignored_impact_effects,
        )
        .is_empty(),
        "the common spawn grace holds the fire tick"
    );
    let contacts = crate::sim::advance(
        &registry,
        &world,
        &hit_zones,
        0.0,
        1.0,
        &mut ignored_impact_effects,
    );
    assert_eq!(contacts.len(), 1, "the host resolves one later impact");
    h.registry = Rc::into_inner(registry)
        .expect("the host projectile stage releases its registry")
        .into_inner();

    let health = h
        .registry
        .get_component::<HealthComponent>(h.owner_pawn)
        .expect("connected pawn remains host-owned after impact");
    assert_eq!(
        health.current, 90.0,
        "damage applied at the shared host chokepoint"
    );
    let [credit] = health.contributor_ledger.entries() else {
        panic!("enemy impact records its ordinary health credit");
    };
    assert_eq!(credit.source_id, "enemy.rifle");
    assert_eq!(credit.last_attacker, Some(enemy));
    assert_eq!(credit.last_weapon, Some(enemy));

    h.step();
    assert_eq!(
        h.client_value("player.health"),
        Some(SlotValue::Number(90.0)),
        "the connected client receives ordinary owner-private Health replication"
    );
    assert_eq!(
        h.hit_declarations_received, 0,
        "the host-side enemy impact did not require a client HitDeclaration"
    );
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

#[test]
fn accumulated_shared_global_converges_without_client_side_evaluation() {
    let direct = LinkConfig {
        delay: 0,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x5418,
    };
    let accumulator = IrNode::Input {
        name: "@dt".to_string(),
        owner: None,
    };
    let mut h = StateSlotHarness::new_with_accumulator(CLIENT_A, direct, direct, Some(accumulator));
    let mut bindings = SlotAccumulatorBindings::default();
    bindings.rebuild(&h.host_ctx);

    h.step();
    assert_eq!(
        h.client_value("net.objective"),
        Some(SlotValue::Number(0.0)),
        "declaring accumulate in the shared schema does not execute it on the client"
    );

    for _ in 0..10 {
        evaluate_slot_accumulators(&mut bindings, 0.5);
        h.step();
    }
    for _ in 0..4 {
        h.step();
    }

    assert_eq!(
        h.host_ctx
            .slot_table
            .borrow()
            .get("net.objective")
            .unwrap()
            .value,
        Some(SlotValue::Number(5.0))
    );
    assert_eq!(
        h.client_value("net.objective"),
        Some(SlotValue::Number(5.0)),
        "the accumulated SharedGlobal value reaches the client through real state-slot replication"
    );
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

fn assert_client_ammo_slots(
    harness: &StateSlotHarness,
    magazine: f32,
    reserve: f32,
    context: &str,
) {
    assert_eq!(
        harness.client_value("player.ammo"),
        Some(SlotValue::Number(magazine)),
        "{context}: ClientStateApply writes the host's equipped magazine"
    );
    assert_eq!(
        harness.client_value("player.ammoReserve"),
        Some(SlotValue::Number(reserve)),
        "{context}: ClientStateApply writes the host's equipped reserve"
    );
}

// AC-CLIENT-1: once a level transition has seeded the host pawn from its carried
// state, the first following snapshot flush owns the connected client's ammo UI
// values. The client-local weapon components are intentionally absent from this
// harness: only SlotValues written by ClientStateApply are observed.
#[test]
fn client_ammo_slots_apply_carried_values_on_next_snapshot_after_level_change() {
    let direct = LinkConfig {
        delay: 0,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0xe15_0801,
    };
    let mut harness = StateSlotHarness::new(CLIENT_A, direct, direct);

    // The host's replacement-level pawn has already received the carried shell
    // weapon values before the next frame's send_packets flush.
    harness.set_host_ammo(0, 7, 41);
    harness.level_changed();
    harness.step();

    assert_client_ammo_slots(
        &harness,
        7.0,
        41.0,
        "the next snapshot after a level change",
    );
}

// AC-CLIENT-1: a rejoin receives fresh owner-private baselines, and each snapshot
// resolves the active wieldable anew. Switching between two ammo-backed weapons
// therefore retargets both client SlotValues without sending gameplay ammo through
// the participation-time tuning payload.
#[test]
fn client_ammo_slots_apply_carried_values_after_rejoin_and_retarget_on_weapon_switch() {
    let direct = LinkConfig {
        delay: 0,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0xe15_0802,
    };
    let mut harness = StateSlotHarness::new(CLIENT_A, direct, direct);

    harness.set_host_ammo(0, 11, 53);
    harness.client_rejoined();
    harness.step();
    assert_client_ammo_slots(&harness, 11.0, 53.0, "the next snapshot after a rejoin");

    harness.set_host_ammo(1, 3, 97);
    harness.step();
    assert_client_ammo_slots(
        &harness,
        3.0,
        97.0,
        "the next snapshot after switching to the other ammo weapon",
    );
}
