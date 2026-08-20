// E18 Task 7 — the two-endpoint replication checks the plan requires: (1) a
// delayed `fire` step dispatching a system-targeted `setState` reaction
// reaches a connected client through state replication, with no client-side
// scheduler evaluation (O24's positive replication half — O24's own
// mechanism, "the client parks nothing", is unit-tested directly against
// `ReactionScheduler::enroll` in `reaction_scheduler.rs` and
// `reaction_scheduler_ordering_tests.rs`; this file adds the two-endpoint wire
// proof), and (2) the consumer's alarm crossing fires client-side once the
// write lands (O42).
//
// Modeled directly on `trigger_state_channel_harness_test.rs` (same
// production seams: `client_receive_and_apply`, `netcode::frame_order`,
// `dispatch_state_crossings_with_sequences`, a real conditioned
// `PacketConditioner` link) — read that file's header for what this pattern
// covers and does NOT cover (role gating, the two host-side production call
// sites). This file's addition on top of that pattern: the HOST side no
// longer writes the shared slot directly from an in-tick `BoundTriggerCommand`
// — it enrolls a `wait` at the trigger's Enter edge and only writes once the
// scheduler lands the tail's `fire` step, driven through the same
// `ReactionScheduler` production code Task 1/3/5 shipped.
//
// It is a `fire` step, not a `setState` step, and not `moverStart`: see the
// AC text this file backs (E18 index.md, "Co-op replication AC").

#![cfg(test)]

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use glam::{Vec2, Vec3};
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{self, ClientMessage, RawSnapshotMessage, SNAPSHOT_VERSION};

use super::command_queue::{MovementOwners, WeaponOwners};
use super::frame_order::{self, ReplicatedStateFrame};
use super::state_slots::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use super::{ClientPrediction, ClientReplication, client_receive_and_apply};
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::netcode::predict_reconcile_harness_test_fixtures::component as player_component;
use crate::scripting::reactions::dispatch_state_crossings_with_sequences;
use crate::scripting::reactions::registry::register_fog_reaction_primitives;
use crate::scripting::reactions::system_commands::{
    SystemReactionRegistry, register_system_reaction_primitives,
};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::reaction_scheduler::{ReactionScheduler, register_reaction_control_primitives};
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::{
    PostMovementCommand, RemotePawnCommand, SimCommand, TickEvents, TriggerTickContext,
    simulate_tick,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::TriggerSystem;
use crate::weapon::FireButtonState;
use postretro_entities::{
    EntityId, FogVolumeComponent, MoverCommand, NamedReaction, PrimitiveDescriptor,
    ReactionDescriptor, ReplicationScope, ScriptCtx, SequenceStep, SequenceTarget, SlotOwnership,
    SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_scripting_core::StoreIdentityLedger;
use postretro_scripting_core::data_descriptors::{CrossingCondition, CrossingDescriptor};
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

const CLIENT_ID: u64 = 1;
const TICK_MS: u64 = 16;
const FRAME_DT: f32 = TICK_MS as f32 / 1000.0;
const ALARM_SLOT: &str = "encounter.alarm";
const TRIGGER_EVENT: &str = "closet.timedReveal";
const RAISE_ALARM: &str = "closet.raiseAlarm";
const PRESENTATION_EVENT: &str = "closet.alarmPresentation";
const FOG_TAG: &str = "closet-alarm-fog";
const PRESENTATION_DENSITY: f32 = 0.85;
const WAIT_DURATION_MS: f64 = 34.0; // ceil(34_000 / 16_667) = 3 ticks.

fn loopback_profile() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: 0xE18_0007,
    }
}

fn alarm_slots() -> SlotTable {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "encounter",
            vec![(
                "alarm".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::SharedGlobal,
                    per_owner: false,
                    accumulate: None,
                }),
            )],
        )
        .expect("fixture namespace is unique");
    table
}

fn alarm_replication_identity() -> ReplicatedSlotIdentity<'static> {
    ReplicatedSlotIdentity::new(
        Some("test.e18-timed-reaction".to_string()),
        Some(StoreIdentityLedger {
            version: 1,
            slots: [(ALARM_SLOT.to_string(), "k0123456789abcdef".to_string())]
                .into_iter()
                .collect(),
        }),
        [ALARM_SLOT.to_string()].into_iter().collect(),
    )
}

fn primitive(name: &str, primitive: &str, tag: Option<&str>, args: serde_json::Value) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            tag: tag.map(str::to_string),
            on_complete: None,
            args,
        }),
    }
}

/// `TRIGGER_EVENT` = `[wait(34ms), fire(raiseAlarm)]` — the wait is the FIRST
/// step, so `partition_direct_reaction`'s amended binder routes the whole body
/// into the residual unfiltered (O46: no pre-wait consequential steps here).
/// `raiseAlarm` is a sourceless system `setState` with a LITERAL value (not an
/// IR node), so the app-drain write goes through the simple
/// `write_state_slot_json` fallback rather than needing a
/// `SystemReactionIrBindings` rebuild in this harness.
fn fixture_reactions() -> Vec<NamedReaction> {
    vec![
        NamedReaction {
            name: TRIGGER_EVENT.to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".to_string(),
                    args: serde_json::json!({ "durationMs": WAIT_DURATION_MS, "interruptible": false }),
                },
                SequenceStep {
                    id: SequenceTarget::Fire,
                    primitive: "fire".to_string(),
                    args: serde_json::json!({ "event": RAISE_ALARM }),
                },
            ]),
        },
        primitive(RAISE_ALARM, "setState", None, serde_json::json!({ "slot": ALARM_SLOT, "value": 1 })),
        primitive(
            PRESENTATION_EVENT,
            "setFogDensity",
            Some(FOG_TAG),
            serde_json::json!({ "density": PRESENTATION_DENSITY }),
        ),
    ]
}

fn fixture_crossings() -> Vec<CrossingDescriptor> {
    vec![CrossingDescriptor {
        slot: Some(ALARM_SLOT.to_string()),
        condition: CrossingCondition::Above { threshold: 0.5 },
        max: 1.0,
        edge: None,
        fire: vec![PRESENTATION_EVENT.to_string()],
    }]
}

fn install_fixture_level_script(ctx: &ScriptCtx) {
    *ctx.slot_table.borrow_mut() = alarm_slots();
    ctx.data_registry
        .borrow_mut()
        .populate_level(fixture_reactions(), fixture_crossings(), &[]);
}

fn fog_volume() -> FogVolumeComponent {
    FogVolumeComponent {
        density: 0.0,
        glow: 0.0,
        edge_softness: 0.0,
        falloff: 1.0,
        tint: [1.0, 1.0, 1.0],
        saturation: 1.0,
        min_brightness: 0.0,
        light_range: 1.0,
        animation: None,
    }
}

fn idle_command() -> SimCommand {
    SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState { pressed: false, active: false },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

fn relay_pair() -> (NetServer, NetClient) {
    let origin = Duration::from_secs(1);
    let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture server binds loopback socket");
    let server_addr: SocketAddr = server_socket
        .local_addr()
        .expect("fixture server resolves loopback address");
    let client_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture client binds loopback socket");
    let static_fingerprint = [0x5a; 32];
    let mut server = NetServer::new(server_socket, server_addr, 1, origin, Some(static_fingerprint))
        .expect("fixture server transport constructs");
    let mut client = NetClient::new(
        client_socket,
        server_addr,
        CLIENT_ID,
        origin,
        Some(static_fingerprint),
        None,
    )
    .expect("fixture client transport constructs");

    server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    server.set_mod_digest(Some(static_fingerprint));
    server.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));
    client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    client.set_mod_digest(Some(static_fingerprint));
    client.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));

    (server, client)
}

struct TimedAlarmHarness {
    host_ctx: ScriptCtx,
    host_trigger_system: TriggerSystem,
    host_trigger_bridge: TriggerVolumeBridge,
    host_bindings: TriggerBindingTable,
    host_state: HostStateReplication,
    replication_identity: ReplicatedSlotIdentity<'static>,
    host_remote_pawn: EntityId,
    host_local_pawn: EntityId,
    host_owners: MovementOwners,
    host_scheduler: ReactionScheduler,
    host_sequence_registry: SequencedPrimitiveRegistry,
    host_reaction_registry: ReactionPrimitiveRegistry,
    host_system_registry: SystemReactionRegistry,

    server: NetServer,
    client: NetClient,
    client_ctx: ScriptCtx,
    client_fog: Option<EntityId>,
    client_replication: ClientReplication,
    client_prediction: ClientPrediction,
    client_state: ClientStateApply,
    client_crossing_detector: CrossingDetector,
    client_sequence_registry: SequencedPrimitiveRegistry,
    client_reaction_registry: ReactionPrimitiveRegistry,
    client_system_registry: SystemReactionRegistry,
    /// A connected client's own scheduler. Never enabled — a connected client
    /// keeps no active scheduler even though the control handler is
    /// registered on its long-lived sequence registry (O24's host-only guard,
    /// driven the same way `auto_close.rs`'s own tests drive `set_enabled`).
    client_scheduler: ReactionScheduler,
    client_applied_alarm: bool,
    client_level_installed: bool,
    accepted_snapshot: bool,

    to_client: PacketConditioner,
    to_server: PacketConditioner,
    sequence: u32,
    engine_frame: u64,
    connected: bool,
}

#[derive(Default)]
struct ClientNetworkStep {
    crossing_events: Vec<String>,
    accepted_snapshot: bool,
}

impl TimedAlarmHarness {
    fn new() -> Self {
        let host_ctx = ScriptCtx::new();
        install_fixture_level_script(&host_ctx);

        let (_host_trigger, host_bindings, host_trigger_bridge, host_remote_pawn, host_local_pawn) = {
            let mut registry = host_ctx.registry.borrow_mut();
            let remote_pawn = registry.spawn(Transform { position: Vec3::new(0.0, 1.0, 0.0), ..Transform::default() });
            registry
                .set_component(remote_pawn, player_component())
                .expect("fixture pawn accepts movement");
            let local_pawn = registry.spawn(Transform { position: Vec3::new(20.0, 1.0, 0.0), ..Transform::default() });
            registry
                .set_component(local_pawn, player_component())
                .expect("fixture local pawn accepts movement");
            registry
                .mark_local_player_pawn(local_pawn)
                .expect("fixture pawn becomes the host-local player");

            let trigger = registry.spawn(Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        TRIGGER_EVENT.to_string(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .expect("fixture trigger accepts its component");

            let bindings =
                TriggerBindingTable::build_with_script_ctx(&registry, &host_ctx.data_registry.borrow(), &host_ctx);
            let mut bridge = TriggerVolumeBridge::new();
            bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            (trigger, bindings, bridge, remote_pawn, local_pawn)
        };
        let mut host_owners = MovementOwners::new();
        host_owners.set(host_remote_pawn, CLIENT_ID);

        let host_scheduler = ReactionScheduler::default();
        host_scheduler.set_enabled(true);
        let mut host_sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut host_sequence_registry, host_scheduler.clone());
        let host_reaction_registry = ReactionPrimitiveRegistry::new();
        let mut host_system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut host_system_registry);

        // A connected client's scheduler is constructed but never enabled —
        // the O24 mechanism.
        let client_scheduler = ReactionScheduler::default();
        let mut client_sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut client_sequence_registry, client_scheduler.clone());
        let mut client_reaction_registry = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut client_reaction_registry);
        let mut client_system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut client_system_registry);
        let (server, client) = relay_pair();

        Self {
            host_ctx,
            host_trigger_system: TriggerSystem::default(),
            host_trigger_bridge,
            host_bindings,
            host_state: HostStateReplication::new(),
            replication_identity: alarm_replication_identity(),
            host_remote_pawn,
            host_local_pawn,
            host_owners,
            host_scheduler,
            host_sequence_registry,
            host_reaction_registry,
            host_system_registry,
            server,
            client,
            client_ctx: ScriptCtx::new(),
            client_fog: None,
            client_replication: ClientReplication::new(),
            client_prediction: ClientPrediction::new(),
            client_state: ClientStateApply::new(),
            client_crossing_detector: CrossingDetector::new(),
            client_sequence_registry,
            client_reaction_registry,
            client_system_registry,
            client_scheduler,
            client_applied_alarm: false,
            client_level_installed: false,
            accepted_snapshot: false,
            to_client: PacketConditioner::new(loopback_profile()),
            to_server: PacketConditioner::new(loopback_profile()),
            sequence: 0,
            engine_frame: 0,
            connected: false,
        }
    }

    fn connect_client(&mut self) {
        assert!(!self.connected, "fixture client connects once");
        self.install_client_level_before_network_baseline();
        self.server.add_relay_connection(CLIENT_ID, None);
        self.client.set_connected();
        for _ in 0..128 {
            self.relay_client_to_server();
            if self.server.is_participating(CLIENT_ID) {
                self.host_state.register_client(CLIENT_ID);
                self.connected = true;
                return;
            }
        }
        panic!("conditioned relay did not complete the fixture handshake");
    }

    fn install_client_level_before_network_baseline(&mut self) {
        assert!(!self.client_level_installed, "fixture client installs one level before connecting");
        install_fixture_level_script(&self.client_ctx);
        let fog = {
            let mut registry = self.client_ctx.registry.borrow_mut();
            let fog = registry.spawn(Transform::default());
            registry
                .set_tags(fog, vec![FOG_TAG.to_string()])
                .expect("fixture fog accepts its presentation tag");
            registry.set_component(fog, fog_volume()).expect("fixture fog accepts its component");
            fog
        };
        self.client_crossing_detector.initialize(
            &self.client_ctx.data_registry.borrow(),
            &self.client_ctx.slot_table.borrow(),
            &self.client_ctx,
        );
        self.client_fog = Some(fog);
        self.client_level_installed = true;
    }

    /// Fire the host trigger's Enter edge, then drive the resumed tail through
    /// the SAME sequence `main.rs`'s frame-end drain uses: resolve the
    /// residual under a scoped origin guard, dispatch it (the wait enrolls
    /// and stops), then advance frames until the scheduler lands the tail's
    /// `fire(raiseAlarm)` and the app-drain queue write applies. Returns once
    /// the alarm slot is written host-side — no client machinery touched.
    fn fire_trigger_and_wait_for_alarm(&mut self) {
        let host_events = self.simulate_host_tick();
        assert_eq!(host_events.trigger_residuals.len(), 1, "one Enter-bound residual fires");
        let (handle, trigger, player) = host_events.trigger_residuals[0];

        {
            // Scoped exactly like `main.rs`'s residual loop: the origin guard
            // covers only the residual dispatch, not the later deferred hop —
            // there is none here, since the wait's `fire` step does not
            // collect into `chained` (it's past the wait, in the tail).
            let _origin = self.host_scheduler.begin_origin(trigger, player, true);
            let steps = self
                .host_bindings
                .residual(handle)
                .expect("fixture residual stays bound")
                .steps()
                .to_vec();
            let follow_ups = fire_prepartitioned_reactions_with_sequences(
                &steps,
                &self.host_sequence_registry,
                &self.host_reaction_registry,
                &self.host_system_registry,
                &self.host_ctx,
                ResidualOrigin::TriggerBinding,
            );
            assert!(follow_ups.is_empty(), "the wait stops the drain before any fire collects");
        }
        assert_eq!(self.host_scheduler.pending_len(), 1, "the tail parked host-side");
        assert!(
            self.host_ctx.slot_table.borrow().get(ALARM_SLOT).unwrap().value
                == Some(SlotValue::Number(0.0)),
            "no write yet — the tail has not landed"
        );

        // Advance frames until the scheduler lands the tail. Mirrors
        // `SimHarness::frame`: several idle ticks, then the frame-end drain.
        for _ in 0..8 {
            self.host_scheduler.begin_frame();
            self.host_scheduler.evaluate(&[]);
        }
        {
            let data = self.host_ctx.data_registry.borrow();
            self.host_scheduler.drain_landings(
                &data,
                &self.host_sequence_registry,
                &self.host_reaction_registry,
                &self.host_system_registry,
                &self.host_ctx,
            );
        }
        assert_eq!(self.host_scheduler.pending_len(), 0, "the wait landed");

        // The app-drain write: `raiseAlarm`'s literal `setState` value queues
        // a `SystemReactionCommand::SetState`, applied through the same
        // literal-fallback path `dispatch_system_commands` uses in `main.rs`.
        for command in self.host_ctx.system_commands.take() {
            if let SystemReactionCommand::SetState { slot, value, .. } = command {
                crate::scripting::primitives::store::write_state_slot_json(&self.host_ctx, &slot, &value)
                    .expect("literal alarm write applies");
            }
        }
    }

    fn simulate_host_tick(&mut self) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = postretro_scripting_core::reaction_dispatch::ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let use_edges = HashMap::new();
        simulate_tick(
            self.host_ctx.registry.clone(),
            &world,
            &hit_zones,
            None,
            -20.0,
            None,
            0.0,
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[RemotePawnCommand {
                pawn: self.host_remote_pawn,
                owner_client_id: CLIENT_ID,
                weapon: None,
                shot_id: None,
                fire_tick: 0,
                client_tick: 0,
                aim_pitch: 0.0,
                command: idle_command(),
            }],
            &idle_command(),
            |_| PostMovementCommand { aim_origin: Vec3::ZERO, aim_direction: Vec3::NEG_Z },
            1.0 / 60.0,
            Some(TriggerTickContext {
                system: &mut self.host_trigger_system,
                bridge: &self.host_trigger_bridge,
                bindings: &self.host_bindings,
                slot_table: self.host_ctx.slot_table.clone(),
                script_ctx: Some(self.host_ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        )
    }

    fn enqueue_host_snapshot(&mut self) {
        assert!(self.connected && self.server.is_participating(CLIENT_ID), "only an accepted client receives state");
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let weapon_owners = WeaponOwners::new();
        let slots = self.host_ctx.slot_table.borrow();
        let registry = self.host_ctx.registry.borrow();
        self.host_state.ingest_frame(&slots, &self.replication_identity, &registry, &self.host_owners, &weapon_owners);
        let fingerprint = self.host_state.fingerprint(&slots, &self.replication_identity);
        let records = self
            .host_state
            .produce_for_client(CLIENT_ID, sequence)
            .expect("accepted fixture client receives state records");
        let snapshot = RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence,
            server_tick: sequence,
            records: Vec::new(),
            state_schema_fingerprint: fingerprint,
            state_records: records,
        };
        assert!(
            self.server.send_snapshot(CLIENT_ID, wire::encode(&snapshot)),
            "accepted fixture client receives the host snapshot through NetServer"
        );
    }

    fn step_network(&mut self) -> ClientNetworkStep {
        let engine_frame = self.engine_frame;
        self.engine_frame = self.engine_frame.wrapping_add(1);
        self.accepted_snapshot = false;
        let applied = frame_order::run_snapshot_apply_stage(self, engine_frame, FRAME_DT);
        let crossing_events = frame_order::run_crossing_stage(self, engine_frame, applied);
        self.relay_client_to_server();
        self.apply_client_control_messages();
        ClientNetworkStep { crossing_events, accepted_snapshot: self.accepted_snapshot }
    }

    fn replicate_until_client_applies_alarm(&mut self) -> Vec<String> {
        let mut crossing_events = Vec::new();
        for _ in 0..128 {
            self.enqueue_host_snapshot();
            let step = self.step_network();
            let applied_this_step = self.client_applied_alarm;
            if applied_this_step {
                assert_eq!(
                    step.crossing_events,
                    vec![PRESENTATION_EVENT.to_string()],
                    "the frame that applies the alarm must also cross to presentation"
                );
            }
            crossing_events.extend(step.crossing_events);
            if applied_this_step {
                return crossing_events;
            }
        }
        panic!("conditioned loopback did not deliver the alarm baseline");
    }

    fn host_alarm(&self) -> Option<SlotValue> {
        self.host_ctx.slot_table.borrow().get(ALARM_SLOT).and_then(|r| r.value.clone())
    }

    fn client_alarm(&self) -> Option<SlotValue> {
        self.client_ctx.slot_table.borrow().get(ALARM_SLOT).and_then(|r| r.value.clone())
    }

    fn client_fog_density(&self) -> f32 {
        self.client_ctx
            .registry
            .borrow()
            .get_component::<FogVolumeComponent>(self.client_fog.expect("client fog installed"))
            .expect("fixture client fog remains present")
            .density
    }

    fn relay_client_to_server(&mut self) {
        self.client.update_connections(Duration::from_millis(TICK_MS));
        self.to_server.enqueue_all(self.client.packets_to_send());
        self.to_server.advance(TICK_MS);
        for packet in self.to_server.take_ready() {
            self.server.process_packet_from(&packet, CLIENT_ID);
        }
        self.server.update_connections(Duration::from_millis(TICK_MS));
        let _ = self.server.poll_handshakes();
    }

    fn relay_server_to_client(&mut self) {
        self.server.update_connections(Duration::from_millis(TICK_MS));
        self.to_client.enqueue_all(self.server.packets_to_send(CLIENT_ID));
        self.to_client.advance(TICK_MS);
        for packet in self.to_client.take_ready() {
            self.client.process_packet(&packet);
        }
        self.client.update_connections(Duration::from_millis(TICK_MS));
    }

    fn apply_client_control_messages(&mut self) {
        for bytes in self.server.drain_input(CLIENT_ID) {
            let message = wire::decode::<ClientMessage>(&bytes).expect("fixture sends only real client control messages");
            match message {
                ClientMessage::Ack(ack) => {
                    self.host_state.apply_ack(CLIENT_ID, ack.latest_snapshot_sequence, &ack.slot_baselines)
                }
                ClientMessage::StateBaselineRefresh(refresh) => {
                    self.host_state.request_refresh(CLIENT_ID, refresh.slot_id, refresh.missing_baseline_ref)
                }
                _ => {}
            }
        }
    }
}

impl ReplicatedStateFrame for TimedAlarmHarness {
    fn apply_received_snapshots(&mut self, frame_dt: f32) {
        self.relay_server_to_client();
        assert!(
            self.client.drain_control().is_empty(),
            "the fixture expects only the transport-owned participation marker"
        );
        let previous_sequence = self.client_replication.latest_sequence();
        let collision = CollisionWorld::new();
        {
            let mut registry = self.client_ctx.registry.borrow_mut();
            let mut slots = self.client_ctx.slot_table.borrow_mut();
            let _ = client_receive_and_apply(
                &mut registry,
                &mut slots,
                &self.replication_identity,
                &mut self.client,
                &mut self.client_replication,
                &mut self.client_state,
                &mut self.client_prediction,
                &[],
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                None,
                &collision,
                -20.0,
                1.0 / 60.0,
                Duration::from_secs_f32(frame_dt),
                None,
                None,
                None,
                false,
            );
        }
        let accepted = self.client_replication.latest_sequence() != previous_sequence;
        self.accepted_snapshot = accepted;
        if accepted && matches!(self.client_alarm(), Some(SlotValue::Number(value)) if value == 1.0) {
            self.client_applied_alarm = true;
        }
    }

    fn dispatch_state_crossings(&mut self) -> Vec<String> {
        dispatch_state_crossings_with_sequences(
            &mut self.client_crossing_detector,
            &self.client_ctx.slot_table.borrow(),
            &self.client_ctx.data_registry.borrow(),
            &self.client_sequence_registry,
            &self.client_reaction_registry,
            &self.client_system_registry,
            &self.client_ctx,
        )
    }
}

fn assert_number_slot_near(value: Option<SlotValue>, expected: f32, context: &str) {
    match value {
        Some(SlotValue::Number(actual)) => assert!((actual - expected).abs() <= 1e-6, "{context}: got {actual}, expected {expected}"),
        other => panic!("{context}: expected Number slot value, got {other:?}"),
    }
}

fn assert_fog_density_near(actual: f32, expected: f32, context: &str) {
    assert!((actual - expected).abs() <= 1e-6, "{context}: got {actual}, expected {expected}");
}

/// The co-op replication AC and O42: a delayed `fire` step dispatching a
/// system-targeted `setState` reaction — reached only after the scheduler
/// lands the wait, entirely host-side — writes a `network: "shared"` slot
/// that reaches the connected client through state replication, and the
/// client's crossing fires its local presentation reaction from that write.
#[test]
fn delayed_fire_of_system_setstate_replicates_and_drives_client_local_presentation() {
    let mut harness = TimedAlarmHarness::new();
    harness.connect_client();

    harness.fire_trigger_and_wait_for_alarm();
    assert_number_slot_near(
        harness.host_alarm(),
        1.0,
        "the delayed fire(raiseAlarm) writes the sharedGlobal slot host-side, after the wait lands",
    );
    assert_fog_density_near(
        harness.client_fog_density(),
        0.0,
        "the client has received no trigger event or presentation command directly",
    );

    let client_crossings = harness.replicate_until_client_applies_alarm();
    assert!(harness.client_applied_alarm, "the client slot value was written by ClientStateApply::apply_snapshot_state");
    assert_number_slot_near(harness.client_alarm(), 1.0, "the client converges to the host's persistent shared state");
    assert_eq!(
        client_crossings,
        vec![PRESENTATION_EVENT.to_string()],
        "the client crossing detector fires the presentation reaction exactly once"
    );
    assert_fog_density_near(
        harness.client_fog_density(),
        PRESENTATION_DENSITY,
        "the fog presentation primitive executes against the client-local registry",
    );
}

/// O24's negative half, made concrete in the two-endpoint context: even
/// though the client possesses a fully wired scheduler (control handlers
/// registered, `wait`/`fire` reachable), it is never enabled — attempting to
/// enroll the SAME reaction's wait client-side is refused at the host-only
/// guard, so "no client-side scheduler evaluation" is not merely "the client
/// never tried" but "the client is structurally blocked if it did".
#[test]
fn client_scheduler_never_enrolls_the_same_reaction() {
    let harness = TimedAlarmHarness::new();
    assert_eq!(
        harness.client_scheduler.pending_len(),
        0,
        "a freshly constructed, never-enabled client scheduler starts empty"
    );
    // Attempt the exact enrollment a client-side re-run of TRIGGER_EVENT's
    // wait would make.
    harness
        .client_scheduler
        .enroll(TRIGGER_EVENT, 0, None, vec![], 3, false);
    assert_eq!(
        harness.client_scheduler.pending_len(),
        0,
        "the host-only guard refuses enrollment; no tail ever parks or runs client-side"
    );
}
