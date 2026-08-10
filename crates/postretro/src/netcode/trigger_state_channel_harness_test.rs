// Headless co-op proof for trigger writes becoming client-local crossing presentation.
// See: context/lib/networking.md · context/lib/scripting.md · context/lib/testing_guide.md
//
// What this file covers — and what it does NOT — stated plainly, so the next reader
// does not take it for more than it is.
//
// COVERED (client half). The client runs the production seams: `client_receive_and_apply`
// (the same function `App::net_poll_and_apply` calls on a connected client) over a
// conditioned `PacketConditioner` link with real wire encode/decode, and the production
// stage order from `netcode::frame_order` — this harness cannot detect crossings before
// applying the frame's snapshots, because `run_crossing_stage` consumes the witness that
// `run_snapshot_apply_stage` mints. The client stage also drains reliable Control before
// Snapshot, matching `App::net_poll_and_apply`; that drain arms the transport-owned
// participation epoch without exposing a promotion message to the engine. A break in
// decode, the ack gate, fingerprint validation, baseline/refresh plumbing, or the
// apply-before-detect order fails a test here.
//
// NOT COVERED — role gating. This harness never constructs a `NetEndpoint`. The
// `NetEndpoint::Client` match arm in `App::net_poll_and_apply` and `App::is_connected_client`
// are never exercised: nothing here proves that a connected client is the role production
// routes down the client apply arm, nor that a host / single-player build stays out of it.
// A regression in that role dispatch would pass every test in this file.
//
// NOT COVERED — host half. `enqueue_host_snapshot` builds the snapshot envelope by hand
// instead of calling production `netcode::host_replicate`, and `apply_client_control_messages`
// re-implements production `netcode::host_handle_client_messages`. Both drive the real
// `HostStateReplication` tracker (production/ack/refresh bookkeeping is genuine), but the
// two host-side production call sites themselves remain uncovered.

#![cfg(test)]

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use glam::{Vec2, Vec3};
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::slots::CloseCause;
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
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::{
    PostMovementCommand, RemotePawnCommand, SimCommand, TickEvents, TriggerTickContext,
    simulate_tick,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::TriggerSystem;
use crate::weapon::FireButtonState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{
    EntityId, FogVolumeComponent, MoverCommand, ReplicationScope, ScriptCtx, SlotOwnership,
    SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::HealthDescriptor;
use postretro_scripting_core::StoreIdentityLedger;
use postretro_scripting_core::data_descriptors::{
    CrossingCondition, CrossingDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

const CLIENT_ID: u64 = 1;
const TICK_MS: u64 = 16;
/// The frame delta the harness hands the production apply stage, matching `TICK_MS`.
const FRAME_DT: f32 = TICK_MS as f32 / 1000.0;
const BLACKOUT_SLOT: &str = "atmosphere.blackout";
const TRIGGER_EVENT: &str = "triggerBlackout";
const PRESENTATION_EVENT: &str = "blackoutPresentation";
const FOG_TAG: &str = "blackout-fog";
const PRESENTATION_DENSITY: f32 = 0.85;

/// The E17-C loopback profile, reused here with fixed loss/jitter so this channel
/// stays covered by a real conditioned two-endpoint path rather than a direct call.
fn loopback_profile() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: 0xE18_0006,
    }
}

/// Minimal `sharedGlobal` declaration from the fixture's level script.
fn atmosphere_slots() -> SlotTable {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "atmosphere",
            vec![(
                "blackout".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::SharedGlobal,
                    accumulate: None,
                }),
            )],
        )
        .expect("fixture namespace is unique");
    table
}

fn atmosphere_replication_identity() -> ReplicatedSlotIdentity<'static> {
    ReplicatedSlotIdentity::new(
        Some("test.atmosphere".to_string()),
        Some(StoreIdentityLedger {
            version: 1,
            slots: [(BLACKOUT_SLOT.to_string(), "k0123456789abcdef".to_string())]
                .into_iter()
                .collect(),
        }),
        [BLACKOUT_SLOT.to_string()].into_iter().collect(),
    )
}

/// Minimal level-script manifest. `triggerBlackout` has the requested direct
/// `setState` plus a presentation step; the client obtains presentation through the
/// separate crossing reaction after replicated-state convergence.
fn atmosphere_reactions() -> Vec<NamedReaction> {
    vec![
        NamedReaction {
            name: TRIGGER_EVENT.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "applyDamage".to_string(),
                target: Some("@activators".to_string()),
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "amount": 25 }),
            }),
        },
        primitive(
            TRIGGER_EVENT,
            "setState",
            None,
            serde_json::json!({ "slot": BLACKOUT_SLOT, "value": 1 }),
        ),
        primitive(
            TRIGGER_EVENT,
            "setFogDensity",
            Some(FOG_TAG),
            serde_json::json!({ "density": PRESENTATION_DENSITY }),
        ),
        primitive(
            PRESENTATION_EVENT,
            "setFogDensity",
            Some(FOG_TAG),
            serde_json::json!({ "density": PRESENTATION_DENSITY }),
        ),
    ]
}

fn atmosphere_crossings() -> Vec<CrossingDescriptor> {
    vec![CrossingDescriptor {
        slot: Some(BLACKOUT_SLOT.to_string()),
        condition: CrossingCondition::Above { threshold: 0.5 },
        max: 1.0,
        edge: None,
        fire: vec![PRESENTATION_EVENT.to_string()],
    }]
}

fn primitive(
    name: &str,
    primitive: &str,
    tag: Option<&str>,
    args: serde_json::Value,
) -> NamedReaction {
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

fn install_fixture_level_script(ctx: &ScriptCtx) {
    *ctx.slot_table.borrow_mut() = atmosphere_slots();
    ctx.data_registry.borrow_mut().populate_level(
        atmosphere_reactions(),
        atmosphere_crossings(),
        &[],
    );
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
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

/// Test-only two-endpoint fixture following the E17-C loopback pattern. The client half
/// drives the production transport, `client_receive_and_apply`, and crossing-dispatch
/// seams in the production `netcode::frame_order` stage order. The host half hand-rolls
/// its snapshot send and control-message drain (see the file header for the gaps).
struct PersistentAtmosphereHarness {
    host_ctx: ScriptCtx,
    host_trigger_system: TriggerSystem,
    host_trigger_bridge: TriggerVolumeBridge,
    host_bindings: TriggerBindingTable,
    host_state: HostStateReplication,
    replication_identity: ReplicatedSlotIdentity<'static>,
    host_remote_pawn: EntityId,
    host_local_pawn: EntityId,
    host_owners: MovementOwners,

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
    client_applied_blackout: bool,
    client_level_installed: bool,
    /// Set by the production apply stage each step; read back by `step_network`.
    accepted_snapshot: bool,

    to_client: PacketConditioner,
    to_server: PacketConditioner,
    sequence: u32,
    /// The harness's stand-in for `ScriptCtx::frame` — the per-frame stamp the
    /// `frame_order` witness carries, so a witness cannot be reused across steps.
    engine_frame: u64,
    connected: bool,
}

#[derive(Default)]
struct ClientNetworkStep {
    crossing_events: Vec<String>,
    accepted_snapshot: bool,
}

fn relay_pair() -> (NetServer, NetClient) {
    let origin = Duration::from_secs(1);
    let server_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture server binds loopback socket");
    let server_addr: SocketAddr = server_socket
        .local_addr()
        .expect("fixture server resolves loopback address");
    let client_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture client binds loopback socket");
    let static_fingerprint = [0x5a; 32];
    let mut server = NetServer::new(
        server_socket,
        server_addr,
        1,
        origin,
        Some(static_fingerprint),
    )
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

    // E15 requires matching admission and parity declarations before participation.
    server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    server.set_mod_digest(Some(static_fingerprint));
    server.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));
    client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    client.set_mod_digest(Some(static_fingerprint));
    client.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));

    (server, client)
}

impl PersistentAtmosphereHarness {
    fn new() -> Self {
        let host_ctx = ScriptCtx::new();
        install_fixture_level_script(&host_ctx);

        let (_host_trigger, host_bindings, host_trigger_bridge, host_remote_pawn, host_local_pawn) = {
            let mut registry = host_ctx.registry.borrow_mut();
            let remote_pawn = registry.spawn(Transform {
                position: Vec3::new(0.0, 1.0, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(remote_pawn, player_component())
                .expect("fixture pawn accepts movement");
            registry
                .set_component(
                    remote_pawn,
                    HealthComponent::from_descriptor(&HealthDescriptor {
                        max: 100.0,
                        hitbox: None,
                        zone_multipliers: HashMap::new(),
                    }),
                )
                .expect("fixture remote pawn accepts health");

            let local_pawn = registry.spawn(Transform {
                position: Vec3::new(20.0, 1.0, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(local_pawn, player_component())
                .expect("fixture local pawn accepts movement");
            registry
                .set_component(
                    local_pawn,
                    HealthComponent::from_descriptor(&HealthDescriptor {
                        max: 100.0,
                        hitbox: None,
                        zone_multipliers: HashMap::new(),
                    }),
                )
                .expect("fixture local pawn accepts health");
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
                        TriggerFireMode::Once,
                        0.0,
                        true,
                    ),
                )
                .expect("fixture trigger accepts its component");

            // This harness drives the live-script simulation path below, so
            // bindings must own the reusable dispatch scope too. The literal-only
            // table builder intentionally has no scope for that execution mode.
            let bindings = TriggerBindingTable::build_with_script_ctx(
                &registry,
                &host_ctx.data_registry.borrow(),
                &host_ctx,
            );
            let mut bridge = TriggerVolumeBridge::new();
            bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            (trigger, bindings, bridge, remote_pawn, local_pawn)
        };
        let mut host_owners = MovementOwners::new();
        host_owners.set(host_remote_pawn, CLIENT_ID);

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
            replication_identity: atmosphere_replication_identity(),
            host_remote_pawn,
            host_local_pawn,
            host_owners,
            server,
            client,
            client_ctx: ScriptCtx::new(),
            client_fog: None,
            client_replication: ClientReplication::new(),
            client_prediction: ClientPrediction::new(),
            client_state: ClientStateApply::new(),
            client_crossing_detector: CrossingDetector::new(),
            client_sequence_registry: SequencedPrimitiveRegistry::new(),
            client_reaction_registry,
            client_system_registry,
            client_applied_blackout: false,
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

    /// Production installs local script defaults and subscriber state before accepted
    /// network baselines arrive. The headless fixture stops before App's window/UI loop.
    fn install_client_level_before_network_baseline(&mut self) {
        assert!(
            !self.client_level_installed,
            "fixture client installs one level before connecting"
        );
        install_fixture_level_script(&self.client_ctx);
        let fog = {
            let mut registry = self.client_ctx.registry.borrow_mut();
            let fog = registry.spawn(Transform::default());
            registry
                .set_tags(fog, vec![FOG_TAG.to_string()])
                .expect("fixture fog accepts its presentation tag");
            registry
                .set_component(fog, fog_volume())
                .expect("fixture fog accepts its component");
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

    /// Run the authoritative fixed tick. The assertion made by callers immediately
    /// afterward is intentionally before any network send, pinning the same-tick
    /// trigger-binding write contract.
    fn fire_host_trigger(&mut self) -> TickEvents {
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
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
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

    /// Produce one host snapshot through the real `HostStateReplication` tracker and send
    /// it over the real `NetServer`. Repeated sends before the ack arrives intentionally
    /// mirror the normal baseline-repair behavior under the conditioned E17-C link.
    ///
    /// GAP: the snapshot envelope is assembled here rather than by production
    /// `netcode::host_replicate`, so the host's own send call site is not covered — only
    /// the state producer it wraps.
    fn enqueue_host_snapshot(&mut self) {
        assert!(
            self.connected && self.server.is_participating(CLIENT_ID),
            "only an accepted client receives state"
        );
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);

        let weapon_owners = WeaponOwners::new();
        let slots = self.host_ctx.slot_table.borrow();
        let registry = self.host_ctx.registry.borrow();
        self.host_state.ingest_frame(
            &slots,
            &self.replication_identity,
            &registry,
            &self.host_owners,
            &weapon_owners,
        );
        let fingerprint = self
            .host_state
            .fingerprint(&slots, &self.replication_identity);
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
            self.server
                .send_snapshot(CLIENT_ID, wire::encode(&snapshot)),
            "accepted fixture client receives the host snapshot through NetServer"
        );
    }

    /// One client frame, sequenced by the production `netcode::frame_order` stages rather
    /// than by this harness. `run_snapshot_apply_stage` mints the `SnapshotsApplied`
    /// witness that `run_crossing_stage` consumes, so the two calls below cannot be
    /// written in the other order — that is a type error, in `main.rs` exactly as here.
    ///
    /// What sits BETWEEN the two stages in production — the catch-up tick loop, the HUD
    /// publisher, the system-command drains — is not modeled; this harness covers the
    /// stages and their order, not App's frame.
    ///
    /// The trailing client→server pump is the harness's stand-in for the socket, and is
    /// deliberately outside the two client stages.
    fn step_network(&mut self) -> ClientNetworkStep {
        let engine_frame = self.engine_frame;
        self.engine_frame = self.engine_frame.wrapping_add(1);
        self.accepted_snapshot = false;

        let applied = frame_order::run_snapshot_apply_stage(self, engine_frame, FRAME_DT);
        let crossing_events = frame_order::run_crossing_stage(self, engine_frame, applied);

        self.relay_client_to_server();
        self.apply_client_control_messages();
        ClientNetworkStep {
            crossing_events,
            accepted_snapshot: self.accepted_snapshot,
        }
    }

    fn replicate_until_client_applies_blackout(&mut self) -> Vec<String> {
        let mut crossing_events = Vec::new();
        for _ in 0..128 {
            self.enqueue_host_snapshot();
            let step = self.step_network();
            let applied_this_step = self.client_applied_blackout;
            if applied_this_step {
                // The apply stage runs before the crossing stage within one frame, so the
                // presentation crossing fires on the SAME frame the authoritative blackout
                // lands — never a frame late. Detect-before-apply would leave this step's
                // crossing list empty and defer the event to the next step.
                assert_eq!(
                    step.crossing_events,
                    vec![PRESENTATION_EVENT.to_string()],
                    "the frame that applies the authoritative blackout must also cross to \
                     presentation: production applies received snapshots before crossing \
                     detection reads the slot table"
                );
            }
            crossing_events.extend(step.crossing_events);
            if applied_this_step {
                return crossing_events;
            }
        }
        panic!("conditioned loopback did not deliver the shared blackout baseline");
    }

    fn host_blackout(&self) -> Option<SlotValue> {
        self.host_ctx
            .slot_table
            .borrow()
            .get(BLACKOUT_SLOT)
            .and_then(|record| record.value.clone())
    }

    fn client_blackout(&self) -> Option<SlotValue> {
        self.client_ctx
            .slot_table
            .borrow()
            .get(BLACKOUT_SLOT)
            .and_then(|record| record.value.clone())
    }

    fn host_pawn_health(&self, pawn: EntityId) -> f32 {
        self.host_ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .expect("fixture pawn keeps health")
            .current
    }

    fn client_health(&self) -> Option<SlotValue> {
        self.client_ctx
            .slot_table
            .borrow()
            .get("player.health")
            .and_then(|record| record.value.clone())
    }

    fn client_fog_density(&self) -> f32 {
        self.client_ctx
            .registry
            .borrow()
            .get_component::<FogVolumeComponent>(
                self.client_fog
                    .expect("client level installs the fixture fog before networking"),
            )
            .expect("fixture client fog remains present")
            .density
    }

    fn relay_client_to_server(&mut self) {
        self.client
            .update_connections(Duration::from_millis(TICK_MS));
        self.to_server.enqueue_all(self.client.packets_to_send());
        self.to_server.advance(TICK_MS);
        for packet in self.to_server.take_ready() {
            self.server.process_packet_from(&packet, CLIENT_ID);
        }
        self.server
            .update_connections(Duration::from_millis(TICK_MS));
        let _ = self.server.poll_handshakes();
    }

    fn relay_server_to_client(&mut self) {
        self.server
            .update_connections(Duration::from_millis(TICK_MS));
        self.to_client
            .enqueue_all(self.server.packets_to_send(CLIENT_ID));
        self.to_client.advance(TICK_MS);
        for packet in self.to_client.take_ready() {
            self.client.process_packet(&packet);
        }
        self.client
            .update_connections(Duration::from_millis(TICK_MS));
    }

    /// GAP: this re-implements production `netcode::host_handle_client_messages` over the
    /// same `HostStateReplication` tracker. The ack / refresh bookkeeping is real; the
    /// production drain call site is not covered.
    fn apply_client_control_messages(&mut self) {
        for bytes in self.server.drain_input(CLIENT_ID) {
            let message = wire::decode::<ClientMessage>(&bytes)
                .expect("fixture sends only real client control messages");
            match message {
                ClientMessage::Ack(ack) => self.host_state.apply_ack(
                    CLIENT_ID,
                    ack.latest_snapshot_sequence,
                    &ack.slot_baselines,
                ),
                ClientMessage::StateBaselineRefresh(refresh) => self.host_state.request_refresh(
                    CLIENT_ID,
                    refresh.slot_id,
                    refresh.missing_baseline_ref,
                ),
                _ => {}
            }
        }
    }

    fn state_send_is_participating(&mut self) -> bool {
        self.server.send_snapshot(CLIENT_ID, Vec::new())
    }

    fn close_client(&mut self) {
        let _ = self
            .server
            .close_relay_connection(CLIENT_ID, CloseCause::Disconnect);
        self.connected = false;
    }
}

/// The harness supplies the two stage bodies; `netcode::frame_order` owns their order.
/// `App` implements the same trait against `net_poll_and_apply` and the crossing
/// dispatcher, so the order under test here is the order production runs — it is not
/// re-declared by this file. This does NOT cover App's role dispatch: the harness is not
/// a `NetEndpoint`, so the `NetEndpoint::Client` arm that would host the apply body in
/// production is never entered (see the file header).
impl ReplicatedStateFrame for PersistentAtmosphereHarness {
    /// Deliver the conditioned link's due packets — the harness's stand-in for the socket
    /// read production does inside `NetClient::update` — then drain reliable Control
    /// before running `client_receive_and_apply`, matching `App::net_poll_and_apply`.
    fn apply_received_snapshots(&mut self, frame_dt: f32) {
        self.relay_server_to_client();
        // Regression: the conditioned harness skipped Control and therefore rejected
        // every correctly epoch-framed Snapshot as inactive participation traffic.
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
        if accepted
            && matches!(self.client_blackout(), Some(SlotValue::Number(value)) if value == 1.0)
        {
            self.client_applied_blackout = true;
        }
    }

    /// The exact dispatcher App runs later in the same frame. The fog reaction mutates the
    /// client registry directly; App's window, input, and command-drain work is outside
    /// this headless test.
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
        Some(SlotValue::Number(actual)) => assert!(
            (actual - expected).abs() <= 1e-6,
            "{context}: got {actual}, expected {expected}"
        ),
        other => panic!("{context}: expected Number slot value, got {other:?}"),
    }
}

fn assert_fog_density_near(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual - expected).abs() <= 1e-6,
        "{context}: got {actual}, expected {expected}"
    );
}

#[test]
fn persistent_atmosphere_trigger_replication_drives_client_local_presentation() {
    let mut harness = PersistentAtmosphereHarness::new();
    harness.connect_client();

    let host_events = harness.fire_host_trigger();
    assert!((harness.host_pawn_health(harness.host_remote_pawn) - 75.0).abs() <= 1e-6);
    assert!((harness.host_pawn_health(harness.host_local_pawn) - 100.0).abs() <= 1e-6);
    assert_number_slot_near(
        harness.host_blackout(),
        1.0,
        "host touch entry writes the sharedGlobal slot inside the firing fixed tick",
    );
    assert_eq!(host_events.trigger_residuals.len(), 1);
    assert!(matches!(
        harness
            .host_bindings
            .residual(host_events.trigger_residuals[0])
            .expect("fixture presentation step stays bound as a residual")
            .steps(),
        [PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Primitive(
            PrimitiveDescriptor { primitive, .. }
        ))] if primitive == "setFogDensity"
    ));
    assert_fog_density_near(
        harness.client_fog_density(),
        0.0,
        "the client has not received a trigger event or presentation command directly",
    );

    let client_crossings = harness.replicate_until_client_applies_blackout();
    assert!(
        harness.client_applied_blackout,
        "the client slot value was written by ClientStateApply::apply_snapshot_state"
    );
    assert_number_slot_near(
        harness.client_blackout(),
        1.0,
        "the client converges to the host's persistent shared state",
    );
    assert_number_slot_near(
        harness.client_health(),
        75.0,
        "owner-private health converges from the trigger-damaged remote pawn",
    );
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

#[test]
fn late_join_blackout_baseline_crosses_once_and_stays_quiet() {
    let mut harness = PersistentAtmosphereHarness::new();

    // The host fires while this client is disconnected. The later connect therefore
    // receives a full baseline rather than a transient event broadcast.
    let _ = harness.fire_host_trigger();
    assert_number_slot_near(harness.host_blackout(), 1.0, "host trigger state persists");
    harness.connect_client();

    let client_crossings = harness.replicate_until_client_applies_blackout();
    assert_eq!(
        client_crossings,
        vec![PRESENTATION_EVENT.to_string()],
        "a late join crosses once from the level-default baseline to the host state"
    );
    assert!(
        harness
            .client_crossing_detector
            .detect(&harness.client_ctx.slot_table.borrow())
            .is_empty(),
        "the already-observed late-join baseline cannot replay presentation every frame"
    );
    assert_fog_density_near(
        harness.client_fog_density(),
        PRESENTATION_DENSITY,
        "late-join presentation runs on the client",
    );
}

#[test]
fn repeated_same_value_snapshot_stays_quiet_after_crossing() {
    let mut harness = PersistentAtmosphereHarness::new();
    harness.connect_client();
    let _ = harness.fire_host_trigger();
    let initial_crossings = harness.replicate_until_client_applies_blackout();
    assert_eq!(initial_crossings, vec![PRESENTATION_EVENT.to_string()]);

    let mut accepted_repeat = false;
    for _ in 0..128 {
        harness.enqueue_host_snapshot();
        let step = harness.step_network();
        assert!(
            step.crossing_events.is_empty(),
            "a repeated authoritative blackout value must not replay presentation"
        );
        if step.accepted_snapshot {
            accepted_repeat = true;
            break;
        }
    }
    assert!(
        accepted_repeat,
        "the real client receive path must accept a later unchanged snapshot"
    );
    assert_fog_density_near(
        harness.client_fog_density(),
        PRESENTATION_DENSITY,
        "a quiet repeated snapshot leaves prior client-local presentation intact",
    );
}

#[test]
fn pending_and_disconnected_clients_receive_no_state_records() {
    let mut harness = PersistentAtmosphereHarness::new();
    assert!(
        !harness.state_send_is_participating(),
        "a disconnected client has no transport slot and receives no state"
    );

    harness.connect_client();
    harness.close_client();
    assert!(
        !harness.state_send_is_participating(),
        "a closed client slot refuses all later state records"
    );
}
