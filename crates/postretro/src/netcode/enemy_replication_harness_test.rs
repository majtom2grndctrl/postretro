// E10 (Networked Enemy Authority Baseline) Task 7: the integration harness proving
// the WHOLE host→client enemy path end to end. A host registers a map-placed AI enemy
// (Brain + Agent + MapPlacement) for replication; its authoritative Transform plus
// optional mesh-animation state rides the wire — through the dev in-memory
// `PacketConditioner` under a conditioned link — to a connected client that
// materializes a remote PRESENTATION entity (descriptor mesh only) and samples its pose
// through the Phase 2 `RemoteInterpolationBuffer`. The host owns
// AI/steering/damage/death; the client carries NO local
// `Brain`/`Agent`/`Health`/`Weapon`/`PlayerMovement` for the enemy.
//
// This drives the genuine production seams (Task 1 materialization, Task 3 wire
// `entity_class`, Task 4 host registration, Task 5 client spawn suppression, Task 6
// remote-enemy apply) — it never re-implements them. It complements the focused unit
// tests already co-located with each seam: this is the cross-seam, conditioned-link,
// end-to-end gate.
// See: context/lib/networking.md · context/lib/testing_guide.md §3 (seam-crossing)
//
// ---------------------------------------------------------------------------
// Manual loopback recipe — E10 enemy authority (host + client over `lo`)
// ---------------------------------------------------------------------------
//
// The deterministic in-memory harness below is the automated E10 gate; this is its
// manual real-socket complement, for eyeballing the host→client enemy path over the
// real encrypted UDP loopback that automated tests cannot judge. Use a map with a
// descriptor-backed AI enemy placement — `content/dev/maps/combat-demo.prl`.
//
// 0. Compile the map if `content/dev/maps/combat-demo.prl` is absent (the `.prl` is
//    NOT checked in; this compile triggers an SH/lightmap bake, so run it once by hand):
//
//        cargo run -p postretro-level-compiler -- \
//            content/dev/maps/combat-demo.map -o content/dev/maps/combat-demo.prl
//
// 1. Host the combat-demo map (listen server, default port):
//
//        RUST_LOG=info cargo run -p xtask -- run content/dev/maps/combat-demo.prl --host
//
// 2. Connect a client back to the host's port over loopback, same map:
//
//        RUST_LOG=info cargo run -p xtask -- run \
//            content/dev/maps/combat-demo.prl --connect 127.0.0.1:<port>
//
// 3. Shape the loopback link to the Phase 2/3 profile (≈150 ms RTT, ~5% loss, jitter)
//    BEFORE driving the client, matching the automated harness's
//    `LinkConfig { delay: 45, jitter: 60, loss_probability: 0.05 }`:
//
//        sudo tc qdisc add dev lo root netem delay 75ms 30ms loss 5%
//        # ... observe, then ALWAYS tear down:
//        sudo tc qdisc del dev lo root netem
//
// Expected, on the CLIENT:
//   - Exactly ONE moving enemy per map-placed enemy — the host-authoritative remote
//     one, interpolated. There is NO second static, locally-spawned enemy (Task 5
//     suppression + Task 6 materialization together prove no duplicate).
//   - The remote enemy moves smoothly via the interpolation buffer (it lags by the
//     interpolation delay), never predicted.
//   - Killing the enemy on the HOST removes it from the client (the absent-from-tick
//     despawn flows through and the client drops the remote entity).
//
// As with the Phase 1/2/3 soak, the shaped `tc netem` qdisc affects all loopback
// traffic for its duration — apply it only for the soak session and always tear down.

#![cfg(test)]

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};

use postretro_net::harness::{LinkConfig, PacketConditioner, VirtualMillis};
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::wire::{
    self, ComponentPayload, EntityRecord, NetworkId, RawSnapshotMessage, SnapshotMessage,
    WireMeshAnimationState, WireTransform,
};

use super::client::{ClientReplication, RemoteEntityMaterialize};
use super::interpolation::{MAX_DELAY_MICROS, PoseSource};
use super::remote_materialize::materialize_armed_remote_enemy;
use super::replication::{
    ReplicableSet, host_register_map_enemies, host_register_world_items, produce_owned_snapshots,
};
use crate::netcode::{HostCommandQueues, MovementOwners, NetworkIdAllocator};
use crate::scripting::builtins::data_archetype::{
    descriptor_materializes_ai_enemy, descriptor_materializes_world_item,
    filter_out_client_host_replicated_placements,
};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::mesh::{AnimationState, InterruptPolicy, MeshComponent};
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::components::touchable::TouchableComponent;
use postretro_entities::data_descriptors::{
    ActionVerb, AttackParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
    BehaviorGraphEnvelope, GuardedRow, MotionVerb, TouchMode, TouchableDescriptor,
};
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, EntityTypeDescriptor, MeshDescriptor,
    MoverCommand, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ScriptCtx, Transform,
    TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{
    AirParams, BRAIN_TARGET_DISTANCE_INPUT, CapsuleParams, FallParams, GroundParams, IrNode,
    IrValue, PlayerMovementComponent, PlayerMovementDescriptor, SpeedParams,
};

use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::spawner::SpawnContext;
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::{
    AuthoritativePlayer, PlayerId, TriggerDispatchInputs, TriggerSystem, TriggerTickInputs,
};

// --- Fixture constants ------------------------------------------------------

const CLIENT_ID: u64 = 1;
/// One sim tick in ms (60 Hz); integer ms keeps the virtual clock exact-ish, matching
/// the sibling movement harness and the net-crate timesync harness.
const TICK_MS: VirtualMillis = 16;
/// The descriptor class the map-placed enemy is registered under (and the host stamps
/// on its Transform-only snapshot).
const ENEMY_CLASS: &str = "grunt";
const WORLD_ITEM_CLASS: &str = "reference_pistol";

/// The mandated automated conditioned-link profile (≈150 ms mean RTT, 5% loss, heavy
/// jitter), applied per direction. Identical to the Phase 2 timesync harness and the
/// Phase 3 predict/reconcile harness — the one profile the whole epic is gated on.
fn mandated_link() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: 0x1502,
    }
}

/// A perfect link for the deterministic structural assertions (no-duplicate, despawn,
/// late-join) whose contract is independent of loss/jitter. The conditioned-link
/// interpolation test below uses [`mandated_link`].
fn perfect_link() -> LinkConfig {
    LinkConfig::perfect()
}

// --- Descriptor + entity fixtures -------------------------------------------

/// A valid graph brain — the predicate only needs `Brain` present, but a real
/// `BrainComponent` keeps the fixture honest.
fn brain() -> BrainComponent {
    BrainComponent::from_graph(&BehaviorGraphDescriptor {
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                BehaviorActivityDescriptor {
                    animation: Some("idle".to_string()),
                    motion: Some(MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 3.5,
    })
}

fn agent() -> AgentComponent {
    AgentComponent::new(0.4, 1.6, 0.3, 3.5)
}

fn agent_params() -> postretro_foundation::NavAgentParams {
    postretro_foundation::NavAgentParams {
        radius: 0.4,
        height: 1.6,
        step_height: 0.3,
        max_slope_deg: 45.0,
    }
}

fn materialize_remote_enemy_presentation(
    remote: &RemoteEntityMaterialize,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    let materialized =
        materialize_armed_remote_enemy(remote, descriptors, registry, Some(agent_params()));
    if materialized {
        if let Some(state) = remote.initial_animation_state.as_deref() {
            super::client::apply_mesh_animation_state(registry, remote.entity_id, state, true);
        }
    }
}

fn ai_enemy_provenance(class: &str, spawn_path: DescriptorSpawnPath) -> DescriptorProvenance {
    DescriptorProvenance {
        canonical_name: class.to_string(),
        owned_components: std::iter::once(DescriptorComponentKind::Health).collect(),
        map_overrides: Default::default(),
        spawn_path,
    }
}

/// Spawn an AI enemy in one of the two eligible descriptor paths: map placement or a
/// fixed-tick runtime spawner. Both carry the same live `Brain` + `Agent` pair and are
/// therefore registered by the production networked-AI sweep.
fn spawn_host_ai_enemy(
    registry: &mut EntityRegistry,
    class: &str,
    position: Vec3,
    spawn_path: DescriptorSpawnPath,
) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let _ = registry.set_component_value(id, ComponentValue::Brain(brain()));
    let _ = registry.set_component_value(id, ComponentValue::Agent(agent()));
    let _ = registry.set_component(id, ai_enemy_provenance(class, spawn_path));
    id
}

fn spawn_host_map_ai_enemy(registry: &mut EntityRegistry, class: &str, position: Vec3) -> EntityId {
    spawn_host_ai_enemy(registry, class, position, DescriptorSpawnPath::MapPlacement)
}

fn spawn_host_runtime_ai_enemy(
    registry: &mut EntityRegistry,
    class: &str,
    position: Vec3,
) -> EntityId {
    spawn_host_ai_enemy(registry, class, position, DescriptorSpawnPath::RuntimeSpawn)
}

fn spawn_host_map_world_item(
    registry: &mut EntityRegistry,
    class: &str,
    position: Vec3,
) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let _ = registry.set_component(
        id,
        TouchableComponent {
            mode: TouchMode::Auto,
            radius: 32.0,
        },
    );
    let _ = registry.set_component(
        id,
        DescriptorProvenance {
            canonical_name: class.to_string(),
            owned_components: std::iter::once(DescriptorComponentKind::Touchable).collect(),
            map_overrides: Default::default(),
            spawn_path: DescriptorSpawnPath::MapPlacement,
        },
    );
    id
}

/// The shared descriptor table both peers load (same content on both ends). The enemy
/// descriptor carries an `ai` block (so the Task 5 client filter classifies it as an
/// AI enemy) and a two-state animated mesh (so the Task 1/6 client materialization
/// attaches a presentation mesh). A non-AI prop descriptor is included so the
/// suppression assertions can prove props are NOT dropped.
fn entity_descriptors() -> Vec<EntityTypeDescriptor> {
    vec![
        enemy_descriptor(ENEMY_CLASS),
        world_item_descriptor(WORLD_ITEM_CLASS),
        prop_descriptor("crate"),
    ]
}

fn enemy_descriptor(class: &str) -> EntityTypeDescriptor {
    let mut states = std::collections::HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle_clip".to_string(),
            looping: true,
            crossfade_ms: 150.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: None,
        },
    );
    states.insert(
        "attack".to_string(),
        AnimationState {
            clip: "attack_clip".to_string(),
            looping: false,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Snap,
            travel_speed: None,
            clip_index: None,
        },
    );
    EntityTypeDescriptor {
        canonical_name: Some(class.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: Some(MeshDescriptor {
            model: "decraniated".to_string(),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: states,
            default_state: Some("idle".to_string()),
            locomotion: None,
        }),
        health: None,
        behavior: Some(enemy_behavior_graph()),
    }
}

fn enemy_behavior_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([
                (
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: Default::default(),
                    },
                ),
                (
                    "attack".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("attack".to_string()),
                        motion: Some(MotionVerb::ChaseTarget),
                        action: Some(ActionVerb::Attack("attack".to_string())),
                        on_enter: None,
                        layers: Default::default(),
                    },
                ),
            ]),
            transitions: std::collections::BTreeMap::from([(
                "idle".to_string(),
                vec![GuardedRow {
                    to: "attack".to_string(),
                    when: IrNode::Le {
                        a: Box::new(IrNode::Input {
                            name: BRAIN_TARGET_DISTANCE_INPUT.to_string(),
                            owner: None,
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(16.0),
                        }),
                    },
                }],
            )]),
        },
        candidate_filter: None,
        patrol: None,
        attacks: std::collections::BTreeMap::from([(
            "attack".to_string(),
            AttackParams {
                weapon: None,
                damage: Some(8.0),
                max_range: Some(2.0),
                cooldown_ms: Some(1000.0),
                engagement_radius: None,
                standoff_distance: None,
            },
        )]),
        engagement_radius: None,
        move_speed: 3.5,
    }
}

/// Compatibility fixture name for coverage that compares the shared direct
/// graph through host and client descriptor paths.
fn behavior_enemy_descriptor(class: &str) -> EntityTypeDescriptor {
    enemy_descriptor(class)
}

/// A `MapEntity` placement of `classname` (the connected-client install filter input).
fn placement(classname: &str) -> crate::scripting::map_entity::MapEntity {
    crate::scripting::map_entity::MapEntity {
        classname: classname.to_string(),
        origin: Vec3::ZERO,
        angles: Vec3::ZERO,
        key_values: std::collections::HashMap::new(),
        tags: Vec::new(),
    }
}

fn prop_descriptor(class: &str) -> EntityTypeDescriptor {
    let mut states = std::collections::HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle_clip".to_string(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Snap,
            travel_speed: None,
            clip_index: None,
        },
    );
    EntityTypeDescriptor {
        canonical_name: Some(class.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: Some(MeshDescriptor {
            model: "crate_mesh".to_string(),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: states,
            default_state: Some("idle".to_string()),
            locomotion: None,
        }),
        health: None,
        behavior: None,
    }
}

fn world_item_descriptor(class: &str) -> EntityTypeDescriptor {
    let mut descriptor = prop_descriptor(class);
    descriptor.touchable = Some(TouchableDescriptor {
        mode: TouchMode::Auto,
        radius: 32.0,
    });
    descriptor.mesh.as_mut().expect("fixture mesh").model = "world_item_mesh".to_string();
    descriptor
}

// --- The two-sided enemy-replication harness --------------------------------

/// Drives the genuine host-produce → wire → conditioner → client-apply path for
/// host-registered AI enemies. One conditioner per direction on a caller-advanced
/// virtual clock (no wall-clock read). The client side runs the REAL
/// `ClientReplication::apply_snapshot` + Task 6 remote-enemy materialization seam.
struct EnemyReplicationHarness {
    // Host (authoritative).
    host_registry: EntityRegistry,
    allocator: NetworkIdAllocator,
    replicable: ReplicableSet,
    /// The host endpoint's `map_enemies` tracking set (the field on `NetEndpoint::Host`).
    /// Drives the reload-cleanup / no-leak assertions.
    map_enemies: HashSet<EntityId>,
    /// Host endpoint world-item tracking. Touchability is the sole membership key.
    world_items: HashSet<EntityId>,
    server_replication: ServerReplication,
    server_tick: u32,

    // Client (viewer).
    client_registry: EntityRegistry,
    client_replication: ClientReplication,
    descriptors: Vec<EntityTypeDescriptor>,

    // Conditioned link, one per direction, on a shared virtual clock.
    to_client: PacketConditioner,
    virtual_ms: VirtualMillis,
}

impl EnemyReplicationHarness {
    fn new(link: LinkConfig) -> Self {
        let mut server_replication = ServerReplication::new();
        server_replication.register_client(CLIENT_ID);

        Self {
            host_registry: EntityRegistry::new(),
            allocator: NetworkIdAllocator::new(),
            replicable: ReplicableSet::new(),
            map_enemies: HashSet::new(),
            world_items: HashSet::new(),
            server_replication,
            server_tick: 0,
            client_registry: EntityRegistry::new(),
            client_replication: ClientReplication::new(),
            descriptors: entity_descriptors(),
            to_client: PacketConditioner::new(link),
            virtual_ms: 0,
        }
    }

    /// Register the host's eligible AI enemies for replication (the production
    /// sweep) into the `ReplicableSet`, stamping NetworkIds and tracking ids in
    /// `map_enemies`.
    fn host_register_enemies(&mut self) {
        host_register_map_enemies(
            &self.host_registry,
            &mut self.allocator,
            &mut self.replicable,
            &mut self.map_enemies,
        );
    }

    fn host_register_world_items(&mut self) {
        host_register_world_items(
            &self.host_registry,
            &mut self.allocator,
            &mut self.replicable,
            &mut self.world_items,
        );
    }

    /// The stable `NetworkId` the host stamped for an enemy `EntityId`.
    fn enemy_network_id(&mut self, enemy: EntityId) -> NetworkId {
        self.allocator.stamp(enemy)
    }

    fn world_item_network_id(&mut self, item: EntityId) -> NetworkId {
        self.allocator.stamp(item)
    }

    /// HOST TICK: produce owned snapshots for the registered set, ingest into the net
    /// tracker (the absent-from-tick despawn path runs here when an enemy vanished),
    /// and enqueue the encoded snapshot for the client through the to-client
    /// conditioner. Advances the virtual clock by one tick first.
    fn host_tick(&mut self) {
        self.virtual_ms += TICK_MS;
        self.to_client.advance(TICK_MS);
        self.server_tick = self.server_tick.wrapping_add(1);

        let owned: Vec<EntitySnapshot> = produce_owned_snapshots(
            &self.host_registry,
            &self.replicable,
            &mut self.allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        self.server_replication.ingest_tick(owned);
        let sequence = self.server_replication.begin_batch();
        if let Some(raw) =
            self.server_replication
                .encode_in_batch(CLIENT_ID, self.server_tick, sequence)
        {
            self.to_client.enqueue(wire::encode(&raw));
        }
    }

    /// CLIENT RECEIVE: deliver any due snapshot packets and apply each through the real
    /// `ClientReplication::apply_snapshot` + Task 6 remote-enemy materialization seam
    /// (`materialize_armed_remote_enemy`), then feed the produced acks back to the
    /// server tracker so subsequent encodes emit deltas (the genuine steady state).
    fn client_receive(&mut self) {
        let mut acks = Vec::new();
        for packet in self.to_client.take_ready() {
            let Ok(raw) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
            let outcome = self
                .client_replication
                .apply_snapshot(&mut self.client_registry, &snapshot);
            // Task 6 seam: materialize the descriptor presentation for each remote
            // enemy this snapshot first spawned (mesh only — never AI state).
            for remote in &outcome.remote_entities {
                materialize_remote_enemy_presentation(
                    remote,
                    &self.descriptors,
                    &mut self.client_registry,
                );
            }
            if let Some(ack) = outcome.ack {
                acks.push(ack);
            }
        }
        for ack in &acks {
            self.server_replication.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
    }

    /// One full step: host ticks + snapshots, client receives + materializes + acks.
    fn step(&mut self) {
        self.host_tick();
        self.client_receive();
    }

    /// Step until the client has mapped `network_id` (received its first baseline and
    /// materialized the remote enemy), or a bounded number of steps elapse. Returns
    /// the number of steps taken. Robust to conditioned-link delay/loss — a dropped
    /// baseline is superseded by the next tick's snapshot.
    fn step_until_client_maps(&mut self, network_id: NetworkId, max_steps: u32) -> u32 {
        let mut steps = 0;
        while self.client_mapped_entity(network_id).is_none() && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }

    /// Step until the client has REMOVED its mapping for `network_id` (a despawn
    /// flowed through), or a bound elapses. Returns steps taken.
    fn step_until_client_removes(&mut self, network_id: NetworkId, max_steps: u32) -> u32 {
        let mut steps = 0;
        while self.client_mapped_entity(network_id).is_some() && steps < max_steps {
            self.step();
            steps += 1;
        }
        steps
    }

    /// The client `EntityId` mapped to `network_id`, if the client has spawned it.
    fn client_mapped_entity(&self, network_id: NetworkId) -> Option<EntityId> {
        self.client_replication.map().get(&network_id).copied()
    }

    /// Count of client entities carrying `kind` (used for the no-duplicate assertion:
    /// exactly ONE enemy presentation exists).
    fn client_entities_with_kind(&self, kind: ComponentKind) -> usize {
        self.client_registry.iter_with_kind(kind).count()
    }

    /// TEST-ONLY helper: deliver a `SnapshotMessage` directly through the REAL
    /// `ClientReplication::apply_snapshot` + Task 6 remote-enemy materialization seam,
    /// bypassing the conditioner/wire layer. The sequence must be greater than any
    /// previously applied sequence for the snapshot to be accepted. Feeds produced acks
    /// back to the server tracker (identical to `client_receive`). Returns the
    /// `ApplyOutcome` for the caller to inspect.
    ///
    /// Use this for structural edge-case tests (re-baseline, despawn of unknown id) that
    /// need surgical control over the snapshot content — the conditioned-link path cannot
    /// target a specific record shape deterministically.
    #[cfg(test)]
    fn inject_snapshot_to_client(
        &mut self,
        snapshot: &postretro_net::wire::SnapshotMessage,
    ) -> super::client::ApplyOutcome {
        let outcome = self
            .client_replication
            .apply_snapshot(&mut self.client_registry, snapshot);
        for remote in &outcome.remote_entities {
            materialize_remote_enemy_presentation(
                remote,
                &self.descriptors,
                &mut self.client_registry,
            );
        }
        if let Some(ack) = &outcome.ack {
            self.server_replication.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
        outcome
    }
}

// ---------------------------------------------------------------------------
// 1. No-duplicate enemy state on the connected client (headline AC).
// ---------------------------------------------------------------------------

// Task 5 suppression + Task 6 materialization together: a connected client ends with
// exactly ONE enemy entity — the moving replicated remote one — and NO static
// map-spawned local authoritative enemy. We prove BOTH halves at their real seams:
//   - the client install filter (`filter_out_client_host_replicated_placements`) drops the AI-enemy
//     placement before dispatch, so no local authoritative enemy is ever spawned;
//   - the host snapshot then materializes exactly one remote presentation entity.
#[test]
fn connected_client_has_exactly_one_remote_enemy_and_no_local_authoritative_copy() {
    let mut h = EnemyReplicationHarness::new(perfect_link());

    // Host: one map-placed AI enemy, registered for replication.
    let enemy =
        spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::new(3.0, 0.0, 0.0));
    h.host_register_enemies();
    let net_id = h.enemy_network_id(enemy);

    // Task 5: on the CLIENT install, the AI-enemy placement is filtered out before
    // dispatch — so the client never spawns a local authoritative enemy. We assert the
    // filter on the same descriptor table both peers share (the real suppression seam).
    let placements = vec![placement(ENEMY_CLASS), placement("crate")];
    let kept = filter_out_client_host_replicated_placements(&placements, &h.descriptors);
    assert_eq!(
        kept.len(),
        1,
        "the AI-enemy placement is suppressed on the client; the prop is kept"
    );
    assert_eq!(kept[0].classname, "crate", "only the non-AI prop survives");

    // Drive the host→client path: the client maps the enemy and materializes its
    // remote presentation (mesh only) via the Task 6 seam.
    let steps = h.step_until_client_maps(net_id, 16);
    assert!(steps > 0, "the client mapped the remote enemy");
    let remote = h
        .client_mapped_entity(net_id)
        .expect("the client mapped the host enemy's NetworkId");

    // No duplicate: exactly ONE Brain-less, Agent-less enemy presentation exists.
    // (The client never locally spawned an authoritative enemy, and the host produced
    // exactly one remote one.)
    assert_eq!(
        h.client_replication.map().len(),
        1,
        "the client maps exactly one networked entity (no duplicate)"
    );
    assert_eq!(
        h.client_entities_with_kind(ComponentKind::Mesh),
        1,
        "exactly one enemy PRESENTATION mesh exists on the client (no duplicate)"
    );
    // The remote enemy carries NO local authoritative sim components.
    for kind in [
        ComponentKind::Brain,
        ComponentKind::Agent,
        ComponentKind::Health,
        ComponentKind::Weapon,
        ComponentKind::PlayerMovement,
    ] {
        assert_eq!(
            h.client_registry.has_component_kind(remote, kind),
            Ok(false),
            "the remote enemy must carry no local {kind:?} (host stays authoritative)"
        );
    }
    // And there is NO authoritative enemy anywhere on the client: no Brain/Agent at all.
    assert_eq!(
        h.client_entities_with_kind(ComponentKind::Brain),
        0,
        "the connected client runs no local enemy Brain (host-authoritative only)"
    );
    assert_eq!(
        h.client_entities_with_kind(ComponentKind::Agent),
        0,
        "the connected client runs no local enemy Agent (host-authoritative only)"
    );

    // Cross-check the descriptor classification the suppression keys on, at the seam.
    assert!(
        descriptor_materializes_ai_enemy(&enemy_descriptor(ENEMY_CLASS)),
        "the enemy descriptor classifies as an AI enemy (the suppression key)"
    );
    assert!(
        !descriptor_materializes_ai_enemy(&prop_descriptor("crate")),
        "the prop descriptor is NOT an AI enemy (kept on the client)"
    );
}

// World items follow the same host-baseline path as AI enemies, but their lifecycle is
// driven solely by `TouchableComponent`: acquisition removes it, drop restores it. The
// client never runs touch, so every visible item must be exactly one descriptor-mesh
// presentation materialized from a Transform + entity_class baseline.
#[test]
fn world_item_baseline_acquire_despawn_and_drop_rebaseline_materialize_one_mesh() {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let item = spawn_host_map_world_item(
        &mut h.host_registry,
        WORLD_ITEM_CLASS,
        Vec3::new(3.0, 0.0, 0.0),
    );
    h.host_register_world_items();
    let initial_network_id = h.world_item_network_id(item);

    let kept = filter_out_client_host_replicated_placements(
        &[placement(WORLD_ITEM_CLASS), placement("crate")],
        &h.descriptors,
    );
    assert_eq!(
        kept.len(),
        1,
        "the client suppresses its map-world-item copy"
    );
    assert_eq!(kept[0].classname, "crate");
    assert!(descriptor_materializes_world_item(
        h.descriptors
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(WORLD_ITEM_CLASS))
            .expect("world item descriptor")
    ));

    assert!(
        h.step_until_client_maps(initial_network_id, 16) > 0,
        "the map item reaches the client as a full baseline"
    );
    let remote_item = h
        .client_mapped_entity(initial_network_id)
        .expect("the initial item baseline materializes one remote entity");
    assert_eq!(
        h.client_replication.map().len(),
        1,
        "no local duplicate item exists"
    );
    assert_eq!(h.client_entities_with_kind(ComponentKind::Mesh), 1);
    assert_eq!(
        h.client_registry
            .get_component::<MeshComponent>(remote_item)
            .expect("baseline materializes descriptor mesh")
            .model,
        "world_item_mesh"
    );
    assert_eq!(
        h.client_registry
            .has_component_kind(remote_item, ComponentKind::Touchable),
        Ok(false),
        "touchability stays host-local"
    );

    h.host_registry
        .remove_component::<TouchableComponent>(item)
        .expect("acquisition removes the world-item membership component");
    h.host_register_world_items();
    assert!(
        h.step_until_client_removes(initial_network_id, 16) > 0,
        "the stale registration emits a despawn tombstone"
    );
    assert!(
        h.client_replication.map().is_empty(),
        "the acquired item has no client replica"
    );

    h.host_registry
        .set_component(
            item,
            TouchableComponent {
                mode: TouchMode::Auto,
                radius: 32.0,
            },
        )
        .expect("drop restores touchability");
    h.host_register_world_items();
    let dropped_network_id = h.world_item_network_id(item);
    assert!(
        dropped_network_id.0 > initial_network_id.0,
        "drop registers a fresh NetworkId after the acquired item was forgotten"
    );
    assert!(
        h.step_until_client_maps(dropped_network_id, 16) > 0,
        "the dropped item reaches the client as a new full baseline"
    );
    let dropped_remote = h
        .client_mapped_entity(dropped_network_id)
        .expect("the dropped item materializes");
    assert_eq!(
        h.client_replication.map().len(),
        1,
        "one dropped item remains"
    );
    assert_eq!(h.client_entities_with_kind(ComponentKind::Mesh), 1);
    assert_eq!(
        h.client_registry
            .get_component::<MeshComponent>(dropped_remote)
            .expect("dropped baseline materializes descriptor mesh")
            .model,
        "world_item_mesh"
    );
}

// The suppression seam keys on "carries a brain", and the shipped reference
// enemy authors that brain as a `components.behavior` graph. A connected client
// must never spawn a local authoritative duplicate brain for that class.
#[test]
fn connected_client_suppresses_the_behavior_spelled_enemy_placement_too() {
    let descriptors = vec![
        behavior_enemy_descriptor(ENEMY_CLASS),
        prop_descriptor("crate"),
    ];
    assert!(
        descriptor_materializes_ai_enemy(&descriptors[0]),
        "an authored `behavior` graph classifies as an AI enemy"
    );

    let kept = filter_out_client_host_replicated_placements(
        &[placement(ENEMY_CLASS), placement("crate")],
        &descriptors,
    );

    assert_eq!(kept.len(), 1, "the behavior-spelled enemy is suppressed");
    assert_eq!(kept[0].classname, "crate", "only the non-AI prop survives");
}

// A runtime-spawned enemy uses the same host snapshot → client materialization
// seam once the host registration sweep has assigned it a NetworkId: the
// RuntimeSpawn classifier (descriptor_class::is_networked_ai_enemy) treats
// MapPlacement and RuntimeSpawn provenance identically, so this test pins the
// shared transport and presentation budget against the E15 mandated
// conditioned-link envelope. The spawner behavior test proves the host cannot
// attack before this timer expires.
#[test]
fn conditioned_link_delivers_and_materializes_enemy_before_spawn_windup_expires() {
    let windup_ms = (MAX_DELAY_MICROS / 1000) as VirtualMillis;

    // Vary the conditioner seed to cover the configured jitter/loss envelope,
    // rather than accepting a single fortunate packet schedule.
    for seed_offset in 0..64 {
        let mut link = mandated_link();
        link.seed = 0xE18C_0000 + seed_offset;
        let mut h = EnemyReplicationHarness::new(link);
        let enemy = spawn_host_ai_enemy(
            &mut h.host_registry,
            ENEMY_CLASS,
            Vec3::ZERO,
            DescriptorSpawnPath::RuntimeSpawn,
        );
        h.host_register_enemies();
        let net_id = h.enemy_network_id(enemy);

        while h.virtual_ms < windup_ms && h.client_mapped_entity(net_id).is_none() {
            h.step();
        }

        let remote = h.client_mapped_entity(net_id).unwrap_or_else(|| {
            panic!(
                "conditioned-link seed {seed_offset:#x} did not deliver the enemy before the \
                 {windup_ms} ms interpolation windup expired"
            )
        });
        assert!(
            h.client_registry
                .has_component_kind(remote, ComponentKind::Mesh)
                == Ok(true),
            "conditioned-link seed {seed_offset:#x} delivered a baseline but not a drawable mesh"
        );
    }
}

// A fixed-tick spawner creates this entity after level installation, so the host's
// post-tick re-sweep is the only production path that can register it. Once registered,
// its Transform-only baseline must carry the descriptor class and materialize the same
// remote presentation as a map placement.
#[test]
fn runtime_spawned_enemy_registers_and_materializes_on_connected_client() {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let enemy =
        spawn_host_runtime_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::new(7.0, 0.0, -2.0));

    h.host_register_enemies();
    assert!(
        h.replicable.contains(enemy),
        "runtime spawn enters ReplicableSet"
    );
    assert!(
        h.map_enemies.contains(&enemy),
        "runtime spawn is reload-tracked"
    );
    let net_id = h.enemy_network_id(enemy);

    assert!(
        h.step_until_client_maps(net_id, 16) > 0,
        "connected client receives the runtime-spawn baseline"
    );
    let remote = h
        .client_mapped_entity(net_id)
        .expect("connected client materializes the runtime spawn");
    assert!(
        h.client_registry
            .get_component::<MeshComponent>(remote)
            .is_ok(),
        "Transform-only runtime-spawn snapshot carries its canonical class for presentation"
    );
}

// E18 Task 5: the host alone performs the pinned pool install. Its selected
// trigger fires through the normal authoritative trigger evaluator and bound
// reaction executor before the spawned enemy follows the established
// host→client consequence path. No pool state or new wire field is introduced.
#[test]
fn host_armed_trap_pool_spawn_reaches_client_while_client_keeps_authored_trigger_state() {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let trap = h.host_registry.spawn(Transform::default());
    h.host_registry
        .set_tags(trap, vec!["trap-pool".to_string()])
        .expect("host trap accepts its pool tag");
    h.host_registry
        .set_component(
            trap,
            TriggerVolumeComponent::new(
                TriggerActivation::Touch,
                String::new(),
                "trapPools.spawn".to_string(),
                String::new(),
                MoverCommand::Start,
                TriggerFireMode::Once,
                0.0,
                false,
            ),
        )
        .expect("host trap receives trigger state");
    let spawner = h.host_registry.spawn(Transform {
        position: Vec3::new(3.0, 0.0, 0.0),
        ..Transform::default()
    });
    h.host_registry
        .set_tags(spawner, vec!["trap-spawner".to_string()])
        .expect("host spawner accepts its reaction tag");
    h.host_registry
        .set_component(
            spawner,
            SpawnerComponent {
                archetype_name: ENEMY_CLASS.to_string(),
                count: 1,
                resolved: true,
            },
        )
        .expect("host spawner receives resolved state");
    let spawn_context = SpawnContext::default();
    spawn_context.replace_level_data(
        [(ENEMY_CLASS.to_string(), enemy_descriptor(ENEMY_CLASS))]
            .into_iter()
            .collect(),
        Some(agent_params()),
    );
    let script_ctx = ScriptCtx::new();
    script_ctx
        .data_registry
        .borrow_mut()
        .populate_level_with_trigger_events(
            vec![NamedReaction {
                name: "trapPools.spawn".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "spawnFromSpawner".to_string(),
                    target: None,
                    tag: Some("trap-spawner".to_string()),
                    args: serde_json::json!({}),
                    on_complete: None,
                }),
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &[],
        );
    let bindings = {
        let data_registry = script_ctx.data_registry.borrow();
        TriggerBindingTable::build_with_script_ctx_and_diagnostics(
            &h.host_registry,
            &data_registry,
            &script_ctx,
            Default::default(),
            spawn_context,
        )
    };

    let report = crate::trigger_pools::install_trigger_pools(
        &mut h.host_registry,
        &[postretro_entities::TriggerPoolDescriptor {
            tag: "trap-pool".to_string(),
            arm: postretro_entities::TriggerPoolArm::Count(1),
            levels: Vec::new(),
        }],
        crate::trigger_pools::TriggerPoolSeedPolicy::Seeded(17),
        &Default::default(),
        &Default::default(),
    );
    assert_eq!(report.seed, Some(17));
    assert_eq!(report.pools[0].selected, [trap]);

    // Install the client map through the real lifecycle seam. The seeded policy
    // would arm this sole member if the connected-client suppression gate failed.
    let client_install =
        crate::startup::lifecycle::install_connected_client_trigger_pool_fixture_for_test();
    assert_eq!(client_install.report, Default::default());
    let client_trap = client_install.trap;
    h.client_registry = client_install.registry;
    assert!(
        !h.client_registry
            .get_component::<TriggerVolumeComponent>(client_trap)
            .expect("client trap remains live")
            .armed,
        "the client does not rerun the host pool roll",
    );

    let player = h.host_registry.spawn(Transform {
        position: Vec3::new(0.0, 1.0, 0.0),
        ..Transform::default()
    });
    h.host_registry
        .set_component(
            player,
            PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
                capsule: CapsuleParams {
                    radius: 0.4,
                    half_height: 0.8,
                    eye_height: 0.5,
                },
                ground: GroundParams {
                    speed: SpeedParams {
                        walk: 7.0,
                        run: 11.0,
                        crouch: 3.0,
                    },
                    accel: 10.0,
                    step_height: 0.3,
                    max_slope: 45.0,
                },
                air: AirParams {
                    forward_steer: 0.0,
                    accel: 0.7,
                    max_control_speed: 0.5,
                    bunny_hop: false,
                    jumps: 0,
                    jump_velocity: 5.5,
                    jump_ceiling: 0.0,
                },
                fall: FallParams {
                    terminal_velocity: 40.0,
                },
                stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
                stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
                dash: None,
                forgiveness: None,
                crouch: None,
                slide: None,
                view_feel: None,
            }),
        )
        .expect("host player receives movement state");
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let alive_players = HashSet::from([player_id]);
    let bound_edges = bindings.bound_edges().clone();
    let mut bridge = TriggerVolumeBridge::new();
    bridge.insert_for_test(trap, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
    let mut trigger_system = TriggerSystem::default();
    let fires = trigger_system.run_authoritative_tick_with_dispatch(
        &mut h.host_registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: 1.0 / 60.0,
        },
        TriggerDispatchInputs {
            alive_players: &alive_players,
            bound_edges: &bound_edges,
        },
        |event, occupancy, registry| {
            bindings.execute_with_script_ctx(
                event.fire.trigger,
                event.edge,
                registry,
                &script_ctx,
                &crate::trigger_commands::TriggerFireContext {
                    fired_trigger: Some(event.fire.trigger),
                    activator: Some(player),
                    occupancy,
                },
            );
        },
    );
    assert!(
        fires
            .fires
            .iter()
            .any(|fire| fire.fire.trigger == trap && fire.fire.event_name == "trapPools.spawn"),
        "the host-selected trap must enter the ordinary trigger fire stream",
    );

    let spawned = h
        .host_registry
        .iter_with_kind(ComponentKind::Brain)
        .map(|(id, _)| id)
        .next()
        .expect("the fired spawner materializes one authoritative enemy");
    h.host_register_enemies();
    let network_id = h.enemy_network_id(spawned);
    assert!(
        h.step_until_client_maps(network_id, 16) > 0,
        "the trap consequence reaches the connected client through existing replication",
    );
    let remote = h
        .client_mapped_entity(network_id)
        .expect("the connected client materializes the spawned enemy");
    assert!(
        h.client_registry
            .get_component::<MeshComponent>(remote)
            .is_ok(),
        "the replicated consequence is a drawable remote presentation",
    );
}

// Regression: the harness materializes remote enemies directly, so it must replay the
// spawn baseline's mesh-animation state just like `client_receive_and_apply`.
#[test]
fn remote_enemy_spawn_baseline_applies_initial_mesh_animation_state() {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let net_id = NetworkId(777);
    let snapshot = SnapshotMessage {
        sequence: 1,
        server_tick: 1,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        records: vec![EntityRecord::FullBaseline {
            network_id: net_id.0,
            baseline_id: 1,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some(ENEMY_CLASS.to_string()),
            active_weapon_archetype: None,
            components: vec![
                ComponentPayload::Transform(WireTransform {
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                }),
                ComponentPayload::MeshAnimationState(WireMeshAnimationState {
                    current_state: "attack".to_string(),
                }),
            ],
        }],
    };

    let outcome = h.inject_snapshot_to_client(&snapshot);

    assert_eq!(
        outcome.remote_entities.len(),
        1,
        "the spawn baseline surfaces one remote enemy materialization request"
    );
    let remote = outcome.remote_entities[0].entity_id;
    let mesh = h
        .client_registry
        .get_component::<MeshComponent>(remote)
        .expect("remote entity carries a MeshComponent after materialization");
    assert_eq!(
        mesh.animation.as_ref().unwrap().current_state,
        "attack",
        "spawn baseline MeshAnimationState is applied after descriptor materialization"
    );
}

// ---------------------------------------------------------------------------
// 2. Remote pose via interpolation under a conditioned link.
// ---------------------------------------------------------------------------

// Under the mandated conditioned link (≈150 ms RTT, 5% loss, jitter, seeded virtual
// clock), the host moves the enemy each tick; its finite Transform payloads flow into
// the client's `RemoteInterpolationBuffer`, and sampled remote poses come from the
// interpolation / hold / bounded-extrapolation state machine — NEVER from prediction.
// The remote enemy carries no local Brain/Agent. Fully deterministic (seed 0x1502 +
// caller-advanced virtual clock; no wall-clock read).
#[test]
fn remote_enemy_pose_comes_from_interpolation_under_conditioned_link() {
    let mut h = EnemyReplicationHarness::new(mandated_link());

    let enemy = spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::ZERO);
    h.host_register_enemies();
    let net_id = h.enemy_network_id(enemy);

    // Move the host enemy along +X by a fixed step each tick (the host owns its motion;
    // here we substitute a deterministic mover for the AI tick, which is not exercised
    // client-side). Run well past the link's buffering horizon so the interpolation
    // buffer has multiple bracketing samples under loss/jitter.
    let mut host_x = 0.0_f32;
    const STEP: f32 = 0.25;
    const TICKS: u32 = 120;
    let mut max_seen_x = f32::MIN;
    for _ in 0..TICKS {
        host_x += STEP;
        let t = Transform {
            position: Vec3::new(host_x, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let _ = h
            .host_registry
            .set_component_value(enemy, ComponentValue::Transform(t));
        h.step();
        max_seen_x = max_seen_x.max(host_x);
    }

    // The client mapped the enemy and buffered multiple samples for it (loss did not
    // starve the buffer to a single sample over 120 ticks).
    let remote = h
        .client_mapped_entity(net_id)
        .expect("client mapped the remote enemy under the conditioned link");
    assert!(
        h.client_replication.sample_count(net_id) >= 2,
        "the interpolation buffer holds multiple bracketing samples (loss-tolerant)"
    );

    // Sample the buffer at a render target IN THE PAST (server_tick - delay), where two
    // samples bracket it: the pose must come from the INTERPOLATED branch, and its X
    // must be strictly between the start and the newest host X (a genuine lerp, not a
    // teleport to the latest packet). The render target the production path uses is
    // `estimated_server_tick - interpolation_delay`; here we sample a tick a few back
    // from the host's current tick to land inside the buffered window.
    let render_target = f64::from(h.server_tick.saturating_sub(8));
    let source = h
        .client_replication
        .presented_source(net_id, render_target)
        .expect("the remote enemy has a presented pose at a past render target");
    assert_eq!(
        source,
        PoseSource::Interpolated,
        "a past render target inside the buffered window samples from interpolation"
    );

    // A render target OLDER than the oldest sample HOLDS the oldest pose, and one NEWER
    // than the newest (starvation) HOLDS the newest (the enemy is Transform-only, so no
    // velocity to extrapolate with) — the full interpolate / hold state machine drives
    // the remote pose, all without any client-side AI.
    let very_old = h
        .client_replication
        .presented_source(net_id, 0.0)
        .expect("an ancient render target still resolves a pose");
    assert_eq!(
        very_old,
        PoseSource::HeldOldest,
        "a render target before the buffer holds the oldest pose"
    );
    let future = h
        .client_replication
        .presented_source(net_id, f64::from(h.server_tick) + 1000.0)
        .expect("a far-future render target still resolves a pose");
    assert_eq!(
        future,
        PoseSource::HeldNewest,
        "a Transform-only remote holds its newest pose on starvation (no extrapolation)"
    );

    // The remote enemy carries NO local Brain/Agent — the host stays authoritative; the
    // client never ran an AI tick / steering / death sweep for it.
    assert_eq!(
        h.client_registry
            .has_component_kind(remote, ComponentKind::Brain),
        Ok(false),
        "remote enemy has no local Brain under the conditioned link"
    );
    assert_eq!(
        h.client_registry
            .has_component_kind(remote, ComponentKind::Agent),
        Ok(false),
        "remote enemy has no local Agent under the conditioned link"
    );

    // The link actually dropped packets (the 5% loss model was exercised) — the
    // interpolation still converged through it.
    assert!(
        h.to_client.dropped() > 0,
        "the 5% loss model dropped at least one snapshot over 120 ticks"
    );
}

// The conditioned-link run is bit-for-bit reproducible under the fixed seed: the drop
// count and the buffered sample count are identical across two independent runs.
#[test]
fn remote_enemy_interpolation_is_deterministic_under_seed_0x1502() {
    fn run() -> (u64, usize) {
        let mut h = EnemyReplicationHarness::new(mandated_link());
        let enemy = spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::ZERO);
        h.host_register_enemies();
        let net_id = h.enemy_network_id(enemy);
        let mut x = 0.0_f32;
        for _ in 0..120 {
            x += 0.25;
            let _ = h.host_registry.set_component_value(
                enemy,
                ComponentValue::Transform(Transform {
                    position: Vec3::new(x, 0.0, 0.0),
                    ..Transform::default()
                }),
            );
            h.step();
        }
        (
            h.to_client.dropped(),
            h.client_replication.sample_count(net_id),
        )
    }
    assert_eq!(run(), run(), "the conditioned enemy run is reproducible");
}

// ---------------------------------------------------------------------------
// 3. Server-authoritative despawn → client cleanup (entity + interp buffer).
// ---------------------------------------------------------------------------

// A registered enemy removed on the host (it vanishes from the host registry — death /
// despawn) flows through the existing absent-from-tick tracker path to emit a despawn;
// the client despawn apply removes the remote entity AND forgets its
// `RemoteInterpolationBuffer` state. Host-side: the `map_enemies` set does not leak the
// dead id in a way that re-registers it (a stale registration is harmless — the
// producer skips a vanished id and a reused slot is a distinct generation-stamped
// EntityId).
#[test]
fn host_despawned_enemy_is_removed_and_interp_forgotten_on_client() {
    let mut h = EnemyReplicationHarness::new(perfect_link());

    let enemy =
        spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::new(2.0, 0.0, 0.0));
    h.host_register_enemies();
    let net_id = h.enemy_network_id(enemy);

    // Establish the remote presentation and buffer a sample for it.
    h.step_until_client_maps(net_id, 16);
    let remote = h
        .client_mapped_entity(net_id)
        .expect("client mapped the remote enemy before despawn");
    // Drive a couple more ticks so the interp buffer holds samples to forget.
    for _ in 0..4 {
        h.step();
    }
    assert!(
        h.client_replication.sample_count(net_id) >= 1,
        "the client buffered remote-enemy interpolation samples before despawn"
    );

    // The enemy DIES on the host: it vanishes from the host registry (as the death
    // sweep would do). The producer skips the vanished id; the net tracker sees it
    // absent from the tick and emits a despawn record.
    h.host_registry
        .despawn(enemy)
        .expect("the live enemy despawns");

    // Drive the path: the despawn record reaches the client, which removes the remote
    // entity and forgets its interpolation buffer.
    let steps = h.step_until_client_removes(net_id, 16);
    assert!(steps > 0, "a despawn flowed through to the client");
    assert!(
        h.client_mapped_entity(net_id).is_none(),
        "the client dropped its NetworkId mapping for the despawned enemy"
    );
    assert!(
        !h.client_registry.exists(remote),
        "the remote presentation entity is removed on the client"
    );
    // The interpolation buffer is forgotten: no samples remain and no pose resolves.
    assert_eq!(
        h.client_replication.sample_count(net_id),
        0,
        "the despawn forgot the remote enemy's interpolation buffer"
    );
    assert!(
        h.client_replication
            .presented_source(net_id, f64::from(h.server_tick))
            .is_none(),
        "no pose resolves for a forgotten remote enemy buffer"
    );
}

// Host-side cleanup: a dead enemy's id does NOT get re-registered. `map_enemies` may
// hold a stale id until the next reload sweep (it drains there), but the re-sweep never
// re-registers the dead enemy — a despawned id is not in the registry's Brain column,
// so `host_register_map_enemies` cannot pick it up. A reused slot is a distinct
// generation-stamped EntityId. We prove the stale registration is harmless: the
// producer emits nothing for it and a re-sweep does not resurrect it.
#[test]
fn host_map_enemies_does_not_leak_or_re_register_a_dead_enemy() {
    let mut h = EnemyReplicationHarness::new(perfect_link());

    let enemy = spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::ZERO);
    h.host_register_enemies();
    assert!(
        h.map_enemies.contains(&enemy),
        "enemy tracked after register"
    );

    // The enemy dies on the host.
    h.host_registry.despawn(enemy).expect("live enemy despawns");

    // A re-sweep (as a level reload would run) drains the stale tracked id and finds no
    // live AI enemy to register — so nothing dead is re-registered.
    h.host_register_enemies();
    assert!(
        !h.map_enemies.contains(&enemy),
        "the reload sweep drained the dead enemy's stale tracked id"
    );
    assert!(
        !h.replicable.contains(enemy),
        "the dead enemy is no longer registered for replication after the sweep"
    );

    // The producer emits no snapshot for the dead enemy even before a re-sweep would
    // run: a vanished registered id is skipped. (Register it again to simulate the
    // pre-sweep stale-registration window, then prove the producer skips it.)
    h.replicable.register(enemy);
    let owned = produce_owned_snapshots(
        &h.host_registry,
        &h.replicable,
        &mut h.allocator,
        &MovementOwners::new(),
        &HostCommandQueues::new(),
    );
    assert!(
        owned.is_empty(),
        "a stale registration for a vanished enemy produces no snapshot (harmless)"
    );
}

// ---------------------------------------------------------------------------
// 4. Late join: a client that joins after enemies exist receives only the LIVE set.
// ---------------------------------------------------------------------------

// A client that joins after a runtime-spawned enemy exists and a map enemy has despawned receives, via
// full baselines, only the currently-LIVE registered enemy set — never a despawned one.
// The net tracker recycles no ids and a late joiner's first snapshot is the live set
// only (a despawn it never knew about is not replayed to it).
#[test]
fn late_joining_client_receives_only_live_enemies_including_runtime_spawn() {
    // The host runs alone first: a runtime spawn and a map enemy exist, then the map
    // enemy dies BEFORE any client connects. Build the host with NO registered client so
    // its first batch is unsent.
    let mut server_replication = ServerReplication::new();
    let mut host_registry = EntityRegistry::new();
    let mut allocator = NetworkIdAllocator::new();
    let mut replicable = ReplicableSet::new();
    let mut map_enemies: HashSet<EntityId> = HashSet::new();

    let enemy_a =
        spawn_host_runtime_ai_enemy(&mut host_registry, ENEMY_CLASS, Vec3::new(1.0, 0.0, 0.0));
    let enemy_b =
        spawn_host_map_ai_enemy(&mut host_registry, ENEMY_CLASS, Vec3::new(9.0, 0.0, 0.0));
    host_register_map_enemies(
        &host_registry,
        &mut allocator,
        &mut replicable,
        &mut map_enemies,
    );
    let net_a = allocator.stamp(enemy_a);
    let net_b = allocator.stamp(enemy_b);

    // Tick a few times with NO clients (the tracker ingests state but sends nothing).
    let mut server_tick = 0u32;
    for _ in 0..4 {
        server_tick += 1;
        let owned = produce_owned_snapshots(
            &host_registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        server_replication.ingest_tick(owned);
        let _ = server_replication.begin_batch();
    }

    // Enemy B dies before the client connects.
    host_registry.despawn(enemy_b).expect("enemy B despawns");
    // Tick so the tracker registers B as absent (its despawn is now part of host state).
    for _ in 0..2 {
        server_tick += 1;
        let owned = produce_owned_snapshots(
            &host_registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        server_replication.ingest_tick(owned);
        let _ = server_replication.begin_batch();
    }

    // NOW a client joins: register it (its first snapshot is an all-FullBaseline of the
    // LIVE set) and apply the client side through a perfect link.
    server_replication.register_client(CLIENT_ID);
    let mut client_registry = EntityRegistry::new();
    let mut client_replication = ClientReplication::new();
    let descriptors = entity_descriptors();
    let mut to_client = PacketConditioner::new(perfect_link());

    // Drive a few late-join snapshots through.
    for _ in 0..6 {
        server_tick += 1;
        let owned = produce_owned_snapshots(
            &host_registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        server_replication.ingest_tick(owned);
        let sequence = server_replication.begin_batch();
        if let Some(raw) = server_replication.encode_in_batch(CLIENT_ID, server_tick, sequence) {
            to_client.enqueue(wire::encode(&raw));
        }
        to_client.advance(TICK_MS);
        let mut acks = Vec::new();
        for packet in to_client.take_ready() {
            let Ok(raw) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
            let outcome = client_replication.apply_snapshot(&mut client_registry, &snapshot);
            for remote in &outcome.remote_entities {
                materialize_remote_enemy_presentation(remote, &descriptors, &mut client_registry);
            }
            if let Some(ack) = outcome.ack {
                acks.push(ack);
            }
        }
        for ack in &acks {
            server_replication.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
    }

    // The late joiner mapped ONLY the live runtime-spawned enemy A — never the despawned
    // map enemy B.
    assert!(
        client_replication.map().contains_key(&net_a),
        "the late joiner received the live runtime-spawned enemy A"
    );
    assert!(
        !client_replication.map().contains_key(&net_b),
        "the late joiner never received the despawned enemy B (live set only)"
    );
    assert_eq!(
        client_replication.map().len(),
        1,
        "the late joiner's baseline is exactly the live registered enemy set"
    );
    // And exactly one presentation mesh exists — no ghost of the dead enemy.
    assert_eq!(
        client_registry.iter_with_kind(ComponentKind::Mesh).count(),
        1,
        "the late joiner renders exactly the one live enemy (no dead ghost)"
    );
}

// ---------------------------------------------------------------------------
// 5. Re-baseline of an already-materialized remote enemy is idempotent.
// ---------------------------------------------------------------------------

// A full-baseline re-delivery for a NetworkId the client has already mapped+materialized
// must NOT surface a second `RemoteEntityMaterialize` request and must NOT reset the
// enemy's live mesh-animation state. The entity stays exactly as it was: one mesh,
// unchanged animation current_state, no Brain/Agent. This exercises the
// `apply_full_baseline` branch for an already-mapped, already-live entity, which applies
// components in place (no respawn) and deliberately omits the `remote_entities` push.
#[test]
fn remote_enemy_rebaseline_does_not_resurface_materialize_or_reset_animation() {
    let mut h = EnemyReplicationHarness::new(perfect_link());

    // Host: one map-placed AI enemy registered for replication.
    let enemy =
        spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::new(5.0, 0.0, 0.0));
    h.host_register_enemies();
    let net_id = h.enemy_network_id(enemy);

    // Drive the host→client path until the enemy materializes (first baseline).
    h.step_until_client_maps(net_id, 16);
    let remote = h
        .client_mapped_entity(net_id)
        .expect("client mapped the remote enemy after first baseline");

    // Confirm the enemy arrived with its presentation mesh and the default animation
    // state ("idle" from the descriptor).
    assert_eq!(
        h.client_entities_with_kind(ComponentKind::Mesh),
        1,
        "exactly one mesh after first materialization"
    );
    let mesh_before = h
        .client_registry
        .get_component::<MeshComponent>(remote)
        .expect("remote entity carries a MeshComponent after materialization")
        .clone();
    assert_eq!(
        mesh_before.animation.as_ref().unwrap().current_state,
        "idle",
        "animation starts in the descriptor default state"
    );

    // Advance the live animation state to something non-default so a re-reset would be
    // observable.
    {
        let mut mesh = mesh_before.clone();
        mesh.animation.as_mut().unwrap().current_state = "locomotion".to_string();
        h.client_registry
            .set_component(remote, mesh)
            .expect("set animation state");
    }

    // Deliver a second FULL BASELINE for the SAME NetworkId through the real apply path.
    // We use a sequence strictly greater than the latest accepted one, and the same
    // NetworkId, to exercise the `FullBaseline, mapped + live` branch of apply_snapshot.
    let latest_seq = h.client_replication.latest_sequence().unwrap_or(0);
    let rebaseline_snapshot = SnapshotMessage {
        sequence: latest_seq + 1,
        server_tick: h.server_tick + 1,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        records: vec![EntityRecord::FullBaseline {
            network_id: net_id.0,
            // A fresh baseline_id so the server can track the re-baseline round-trip.
            baseline_id: 999,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some(ENEMY_CLASS.to_string()),
            active_weapon_archetype: None,
            components: vec![ComponentPayload::Transform(WireTransform {
                position: [5.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            })],
        }],
    };
    let outcome = h.inject_snapshot_to_client(&rebaseline_snapshot);

    // CORE ASSERTION (a1): the apply does NOT surface a second RemoteEntityMaterialize
    // for this NetworkId. The `apply_full_baseline` already-mapped-and-live branch
    // never pushes to `remote_entities`.
    assert!(
        outcome
            .remote_entities
            .iter()
            .all(|r| r.entity_id != remote),
        "re-baseline must not surface a second RemoteEntityMaterialize for the already-materialized enemy"
    );
    assert!(
        outcome.remote_entities.is_empty(),
        "the re-baseline snapshot carries only one record and it must not trigger materialization"
    );

    // CORE ASSERTION (a2): the live mesh-animation state is NOT reset. The enemy's
    // current_state must still be "locomotion" (what we set) — materialization did not
    // re-run / re-attach the mesh component.
    let mesh_after = h
        .client_registry
        .get_component::<MeshComponent>(remote)
        .expect("remote entity still carries a MeshComponent after re-baseline");
    assert_eq!(
        mesh_after.animation.as_ref().unwrap().current_state,
        "locomotion",
        "re-baseline must not reset live animation state (materialization did not re-run)"
    );

    // Structural sanity: still exactly one mesh, still no Brain/Agent.
    assert_eq!(
        h.client_entities_with_kind(ComponentKind::Mesh),
        1,
        "re-baseline must not spawn a second mesh entity (no duplicate)"
    );
    assert_eq!(
        h.client_replication.map().len(),
        1,
        "re-baseline must not change the number of mapped networked entities"
    );
    for kind in [ComponentKind::Brain, ComponentKind::Agent] {
        assert_eq!(
            h.client_registry.has_component_kind(remote, kind),
            Ok(false),
            "re-baseline must not attach {kind:?} (host stays authoritative)"
        );
    }

    // The ack IS produced (the re-baseline was accepted).
    assert!(
        outcome.ack.is_some(),
        "a re-baseline snapshot must be acked (it was accepted)"
    );
    let ack = outcome.ack.unwrap();
    assert_eq!(
        ack.latest_snapshot_sequence,
        latest_seq + 1,
        "ack carries the new sequence"
    );
    assert_eq!(
        ack.entity_baselines,
        vec![(net_id.0, 999)],
        "the re-baseline is acked with its baseline_id"
    );
}

// ---------------------------------------------------------------------------
// 6. Despawn for a never-mapped NetworkId is handled cleanly (no panic, clean ack).
// ---------------------------------------------------------------------------

// Without ever mapping a given NetworkId on the client, delivering a despawn record for
// it must be a clean no-op: no panic, the snapshot is accepted and acked (the tombstone
// is acked so the server stops resending it), no entity is removed that shouldn't be,
// and client state (map, interp buffer) stays consistent. This exercises the
// `apply_despawn` idempotency contract for a completely unknown NetworkId — a scenario
// that arises when a client joins after an enemy already despawned (the late-join path
// sends only live entities, but a duplicate or reordered despawn packet could arrive
// for an entity the client never saw).
#[test]
fn despawn_for_unmapped_network_id_is_a_clean_noop() {
    let mut h = EnemyReplicationHarness::new(perfect_link());

    // Optionally establish one live enemy so we can prove the despawn leaves it intact.
    let live_enemy =
        spawn_host_map_ai_enemy(&mut h.host_registry, ENEMY_CLASS, Vec3::new(1.0, 0.0, 0.0));
    h.host_register_enemies();
    let live_net_id = h.enemy_network_id(live_enemy);
    h.step_until_client_maps(live_net_id, 16);
    assert!(
        h.client_mapped_entity(live_net_id).is_some(),
        "live enemy is mapped before the test"
    );
    let live_remote = h.client_mapped_entity(live_net_id).unwrap();

    // Choose a NetworkId that was NEVER seen by this client.
    let phantom_net_id: u32 = 9999;
    assert!(
        !h.client_replication
            .map()
            .contains_key(&NetworkId(phantom_net_id)),
        "sanity: phantom NetworkId must not be mapped before the test"
    );

    let map_len_before = h.client_replication.map().len();
    let registry_len_before = h
        .client_registry
        .iter_with_kind(ComponentKind::Mesh)
        .count();

    // Deliver a despawn for the never-mapped id through the real apply path.
    let latest_seq = h.client_replication.latest_sequence().unwrap_or(0);
    let despawn_snapshot = SnapshotMessage {
        sequence: latest_seq + 1,
        server_tick: h.server_tick + 1,
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
        records: vec![EntityRecord::Despawn {
            network_id: phantom_net_id,
            tombstone_id: 42,
            reason: 0,
        }],
    };
    // Must not panic.
    let outcome = h.inject_snapshot_to_client(&despawn_snapshot);

    // CORE ASSERTION (b1): the snapshot is accepted — an ack is produced. The despawn
    // apply is idempotent; unknown ids are no-ops, and the snapshot advances the
    // sequence, so an ack is always emitted for an accepted snapshot.
    assert!(
        outcome.ack.is_some(),
        "despawn for unknown NetworkId must still produce an ack (snapshot was accepted)"
    );
    let ack = outcome.ack.unwrap();
    assert_eq!(
        ack.latest_snapshot_sequence,
        latest_seq + 1,
        "ack carries the despawn snapshot's sequence"
    );

    // CORE ASSERTION (b2): the tombstone IS acked. The server needs this to stop
    // resending the despawn record — the client acknowledges it has reached the
    // despawned state for this id (trivially true since it never had it).
    assert_eq!(
        ack.despawn_tombstones,
        vec![(phantom_net_id, 42)],
        "despawn for unknown id must ack its tombstone so the server stops resending"
    );

    // CORE ASSERTION (b3): client state is consistent — the map did not grow or
    // shrink (the phantom id was not spuriously inserted or the live enemy removed),
    // no mesh entity was removed, and the interp buffer for the phantom id has no
    // samples.
    assert_eq!(
        h.client_replication.map().len(),
        map_len_before,
        "map length must not change after despawn of unmapped id"
    );
    assert_eq!(
        h.client_registry
            .iter_with_kind(ComponentKind::Mesh)
            .count(),
        registry_len_before,
        "no mesh entity was removed when despawning an unmapped id"
    );
    assert!(
        !h.client_replication
            .map()
            .contains_key(&NetworkId(phantom_net_id)),
        "phantom NetworkId must not appear in the map after its despawn"
    );
    assert_eq!(
        h.client_replication.sample_count(NetworkId(phantom_net_id)),
        0,
        "interp buffer for the phantom id must be empty (no spurious samples)"
    );

    // The live enemy is untouched.
    assert!(
        h.client_registry.exists(live_remote),
        "the live enemy entity was not removed by the phantom despawn"
    );
    assert!(
        h.client_replication.map().contains_key(&live_net_id),
        "the live enemy's mapping is intact after the phantom despawn"
    );
}
