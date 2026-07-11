// E18 Task 6: two-endpoint proof for the persistent-atmosphere trigger channel.
//
// The fixture models one level script on both endpoints: a touch trigger's
// `on_fire` event writes a shared-global blackout slot and carries a fog-density
// presentation residual. The host runs the consequential write in its fixed tick;
// the client receives only replicated slot state, detects the crossing locally, and
// dispatches its own fog presentation reaction. No transient trigger event crosses
// the network.
// See: context/lib/networking.md · context/lib/scripting.md · context/lib/testing_guide.md

#![cfg(test)]

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::wire::{
    self, ClientMessage, RawSnapshotMessage, SNAPSHOT_VERSION, StateBaselineRefreshRequest,
};

use super::command_queue::{MovementOwners, WeaponOwners};
use super::state_slots::{ClientStateApply, HostStateReplication};
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::netcode::predict_reconcile_harness_test_fixtures::component as player_component;
use crate::scripting::reactions::registry::register_fog_reaction_primitives;
use crate::scripting::reactions::system_commands::{
    SystemReactionRegistry, register_system_reaction_primitives,
};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::{PostMovementCommand, SimCommand, TickEvents, TriggerTickContext, simulate_tick};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::TriggerSystem;
use crate::weapon::FireButtonState;
use postretro_entities::{
    EntityId, FogVolumeComponent, MoverCommand, ReplicationScope, ScriptCtx, SlotOwnership,
    SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::{
    CrossingCondition, CrossingDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

const CLIENT_ID: u64 = 1;
const TICK_MS: u64 = 16;
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
                }),
            )],
        )
        .expect("fixture namespace is unique");
    table
}

/// Minimal level-script manifest. `triggerBlackout` has the requested direct
/// `setState` plus a presentation step; the client obtains presentation through the
/// separate crossing reaction after replicated-state convergence.
fn atmosphere_reactions() -> Vec<NamedReaction> {
    vec![
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
        slot: BLACKOUT_SLOT.to_string(),
        condition: CrossingCondition::Above { threshold: 0.5 },
        max: 1.0,
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
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        use_pressed: false,
    }
}

/// Test-only two-endpoint fixture following the E17-C loopback pattern: each
/// endpoint owns its own registry/slot table and packets traverse conditioned links.
/// It deliberately uses the production state-slot producer and
/// `ClientStateApply::apply_snapshot_state` rather than copying the slot value.
struct PersistentAtmosphereHarness {
    host_ctx: ScriptCtx,
    host_trigger_system: TriggerSystem,
    host_trigger_bridge: TriggerVolumeBridge,
    host_bindings: TriggerBindingTable,
    host_state: HostStateReplication,

    client_ctx: ScriptCtx,
    client_fog: EntityId,
    client_state: ClientStateApply,
    client_crossing_detector: CrossingDetector,
    client_sequence_registry: SequencedPrimitiveRegistry,
    client_reaction_registry: ReactionPrimitiveRegistry,
    client_system_registry: SystemReactionRegistry,
    client_applied_blackout: bool,

    to_client: PacketConditioner,
    to_server: PacketConditioner,
    sequence: u32,
    connected: bool,
}

impl PersistentAtmosphereHarness {
    fn new() -> Self {
        let host_ctx = ScriptCtx::new();
        install_fixture_level_script(&host_ctx);

        let (_host_trigger, host_bindings, host_trigger_bridge) = {
            let mut registry = host_ctx.registry.borrow_mut();
            let pawn = registry.spawn(Transform {
                position: Vec3::new(0.0, 1.0, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(pawn, player_component())
                .expect("fixture pawn accepts movement");
            registry
                .mark_local_player_pawn(pawn)
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

            let bindings = TriggerBindingTable::build(
                &registry,
                &host_ctx.data_registry.borrow(),
                &host_ctx.slot_table.borrow(),
            );
            let mut bridge = TriggerVolumeBridge::new();
            bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            (trigger, bindings, bridge)
        };

        let client_ctx = ScriptCtx::new();
        install_fixture_level_script(&client_ctx);
        let client_fog = {
            let mut registry = client_ctx.registry.borrow_mut();
            let fog = registry.spawn(Transform::default());
            registry
                .set_tags(fog, vec![FOG_TAG.to_string()])
                .expect("fixture fog accepts its presentation tag");
            registry
                .set_component(fog, fog_volume())
                .expect("fixture fog accepts its component");
            fog
        };

        // This is the client-side half of CROSSING-CHANNEL INSTALL ORDER: initialize
        // from the local level defaults before any state snapshot can be applied.
        let mut client_crossing_detector = CrossingDetector::new();
        client_crossing_detector.initialize(
            &client_ctx.data_registry.borrow(),
            &client_ctx.slot_table.borrow(),
        );

        let mut client_reaction_registry = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut client_reaction_registry);
        let mut client_system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut client_system_registry);

        Self {
            host_ctx,
            host_trigger_system: TriggerSystem::default(),
            host_trigger_bridge,
            host_bindings,
            host_state: HostStateReplication::new(),
            client_ctx,
            client_fog,
            client_state: ClientStateApply::new(),
            client_crossing_detector,
            client_sequence_registry: SequencedPrimitiveRegistry::new(),
            client_reaction_registry,
            client_system_registry,
            client_applied_blackout: false,
            to_client: PacketConditioner::new(loopback_profile()),
            to_server: PacketConditioner::new(loopback_profile()),
            sequence: 0,
            connected: false,
        }
    }

    fn connect_client(&mut self) {
        assert!(!self.connected, "fixture client connects once");
        self.host_state.register_client(CLIENT_ID);
        self.connected = true;
    }

    /// Run the authoritative fixed tick. The assertion made by callers immediately
    /// afterward is intentionally before any network send, pinning the same-tick
    /// trigger-binding write contract.
    fn fire_host_trigger(&mut self) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = postretro_scripting_core::reaction_dispatch::ProgressTracker::new();
        let mut ai_warned = HashSet::new();
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
            &mut ai_warned,
            &[],
            &mut mover_states,
            &[],
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
                use_edges: &use_edges,
            }),
        )
    }

    /// Send one host snapshot through the actual state producer. Repeated sends before
    /// the ack arrives intentionally mirror the normal baseline-repair behavior under
    /// the conditioned E17-C link.
    fn enqueue_host_snapshot(&mut self) {
        assert!(self.connected, "only an accepted client receives state");
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);

        let owners = MovementOwners::new();
        let weapon_owners = WeaponOwners::new();
        let slots = self.host_ctx.slot_table.borrow();
        let registry = self.host_ctx.registry.borrow();
        self.host_state
            .ingest_frame(&slots, &registry, &owners, &weapon_owners);
        let fingerprint = self.host_state.fingerprint(&slots);
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
        self.to_client.enqueue(wire::encode(&snapshot));
    }

    /// Advance the two conditioned links once, route snapshot data through the real
    /// `ClientStateApply` seam, then run the same local crossing→reaction dispatch the
    /// app uses after state writes. Returns all client-local crossing event names.
    fn step_network(&mut self) -> Vec<String> {
        self.to_client.advance(TICK_MS);
        self.to_server.advance(TICK_MS);

        let mut crossing_events = Vec::new();
        for packet in self.to_client.take_ready() {
            let snapshot = wire::decode::<RawSnapshotMessage>(&packet)
                .expect("fixture sends only real state snapshot envelopes");
            let outcome = self.client_state.apply_snapshot_state(
                &mut self.client_ctx.slot_table.borrow_mut(),
                snapshot.sequence,
                &snapshot.state_schema_fingerprint,
                &snapshot.state_records,
            );
            self.client_applied_blackout |=
                outcome.fresh_slots.iter().any(|slot| slot == BLACKOUT_SLOT);

            if !outcome.slot_baselines.is_empty() {
                self.to_server
                    .enqueue(wire::encode(&ClientMessage::Ack(wire::AckMessage {
                        latest_snapshot_sequence: snapshot.sequence,
                        acked_server_tick: snapshot.server_tick,
                        entity_baselines: Vec::new(),
                        despawn_tombstones: Vec::new(),
                        slot_baselines: outcome.slot_baselines,
                    })));
            }
            for refresh in outcome.refresh_requests {
                self.to_server
                    .enqueue(wire::encode(&ClientMessage::StateBaselineRefresh(refresh)));
            }

            let detected = self
                .client_crossing_detector
                .detect(&self.client_ctx.slot_table.borrow());
            for event_name in &detected {
                let _ = fire_named_event_with_sequences(
                    event_name,
                    &self.client_ctx.data_registry.borrow(),
                    &self.client_sequence_registry,
                    &self.client_reaction_registry,
                    &self.client_system_registry,
                    &self.client_ctx,
                );
            }
            crossing_events.extend(detected);
        }

        for packet in self.to_server.take_ready() {
            let message = wire::decode::<ClientMessage>(&packet)
                .expect("fixture sends only real client control messages");
            match message {
                ClientMessage::Ack(ack) => self.host_state.apply_ack(
                    CLIENT_ID,
                    ack.latest_snapshot_sequence,
                    &ack.slot_baselines,
                ),
                ClientMessage::StateBaselineRefresh(StateBaselineRefreshRequest {
                    slot_id,
                    missing_baseline_ref,
                    ..
                }) => self
                    .host_state
                    .request_refresh(CLIENT_ID, slot_id, missing_baseline_ref),
                _ => {}
            }
        }

        crossing_events
    }

    fn replicate_until_client_applies_blackout(&mut self) -> Vec<String> {
        let mut crossing_events = Vec::new();
        for _ in 0..128 {
            self.enqueue_host_snapshot();
            crossing_events.extend(self.step_network());
            if self.client_applied_blackout {
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

    fn client_fog_density(&self) -> f32 {
        self.client_ctx
            .registry
            .borrow()
            .get_component::<FogVolumeComponent>(self.client_fog)
            .expect("fixture client fog remains present")
            .density
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
            .descriptors(),
        [ReactionDescriptor::Primitive(PrimitiveDescriptor { primitive, .. })]
            if primitive == "setFogDensity"
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
