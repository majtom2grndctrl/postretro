// See: context/lib/networking.md §Combat authority · context/lib/testing_guide.md
//! E16 acceptance through opaque conditioned packets and the production weapon paths.
//! The sockets only construct the transports; the relay exercises renet's actual
//! reliable ordered channels, including retransmission, in both directions.
//! It also hosts the client combat-feel parity reproductions and the cadence-bound tests.

use super::*;
use crate::client_weapon::ClientWeaponFrame;
use crate::weapon;
use glam::{Vec2, Vec3};
use postretro_ai::AiRuntime;
use postretro_combat_model::AuthorizedShot;
use postretro_entities::Transform;
use postretro_entities::components::health::Hitbox;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationRelease, ProjectileBodyVisual, ShotId,
    WeaponDescriptor, WeaponPlacementDescriptor,
};
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::replication::ServerReplication;
use postretro_net::transport::NetClient;
use postretro_scripting_core::data_descriptors::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::net::{Ipv4Addr, UdpSocket};
use std::rc::Rc;
use std::time::Duration;

const CLIENT: u64 = 7;
const DT: f32 = 1.0 / 60.0;
const TARGET_HP: f32 = 10_000.0;
const MAGAZINE: u32 = 32;

/// The E16 mandated-link seed; its losses reach ordinary held commands.
const MANDATED_SEED: u64 = 0x1502;
/// Mandated-link seeds whose losses stall the input stream into the catch-up
/// trim, so a hold restart reaches the host early against its cadence.
const TRIM_SEED_A: u64 = 0x1_3c6e_e670;
const TRIM_SEED_B: u64 = 0x9e37_6cbb;

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected}, got {actual}"
    );
}

fn mandated_link() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: MANDATED_SEED,
    }
}

fn mandated_link_seeded(seed: u64) -> LinkConfig {
    LinkConfig {
        seed,
        ..mandated_link()
    }
}

fn neutral() -> sim::SimCommand {
    sim::SimCommand {
        input_tick: 0,
        movement: crate::movement::MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: ActivationInput::default(),
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

fn token(tick: u32, lane: ActivationLane) -> ActivationToken {
    ActivationToken {
        start_tick: tick,
        lane,
    }
}

fn held(lane: ActivationLane, pressed: bool) -> sim::SimCommand {
    let mut command = neutral();
    let button = weapon::FireButtonState {
        pressed,
        active: true,
    };
    match lane {
        ActivationLane::Primary => command.fire_button = button,
        ActivationLane::Secondary => command.secondary_button = button,
    }
    command
}

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
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
    })
}

fn health(hitbox: Option<Hitbox>) -> HealthComponent {
    HealthComponent {
        max: TARGET_HP,
        current: TARGET_HP,
        hitbox,
        death_handled: false,
        pending_kill_credit: None,
        zone_multipliers: Default::default(),
        contributor_ledger: Default::default(),
    }
}

fn action(charged: bool, shots: usize, trigger: &str, full_damage: f32) -> Value {
    let mut steps = Vec::new();
    for ordinal in 0..shots {
        if ordinal != 0 {
            steps.push(json!({ "kind": "wait", "durationMs": 50 }));
        }
        let damage = if charged {
            json!({ "op": "mul", "a": { "op": "const", "value": full_damage }, "b": { "op": "input", "name": "charge" } })
        } else {
            json!(3)
        };
        steps.push(json!({ "kind": "shot", "scale": {
            "damage": damage, "resourceCost": if charged { 2 } else { 1 },
            "projectileSize": if charged { 1.5 } else { 0.75 },
            "projectileRadius": 1, "projectileSpeed": 1, "range": 1,
        }}));
    }
    let mut action = json!({ "trigger": trigger, "recoveryMs": 300, "steps": steps });
    if charged {
        action["charge"] = json!({ "minMs": 100, "fullMs": 500 });
    }
    action
}

fn descriptor(projectile: bool, charged: bool, shots: usize, trigger: &str) -> WeaponDescriptor {
    descriptor_with_damage(projectile, charged, shots, trigger, 6.0)
}

fn descriptor_with_damage(
    projectile: bool,
    charged: bool,
    shots: usize,
    trigger: &str,
    full_damage: f32,
) -> WeaponDescriptor {
    // Damage, price, flight, collision, and visual size deliberately have
    // independent authored axes. An ordinary action is 3x; a full charge is 6x.
    let mut value = json!({
        "damage": 10, "range": 100,
        "resolution": if projectile { "projectile" } else { "hitscan" },
        "primary": action(false, shots, trigger, full_damage),
        "secondary": action(charged, shots, "press", full_damage),
        "resource": { "kind": "ammo", "type": "rounds", "magazine": MAGAZINE, "reserve": 0 },
        "projectile": { "speed": 60, "radius": 0.125, "lifetimeMs": 5000,
            "visual": { "body": { "kind": "sprite", "sprite": "sprites/test.png", "size": 0.25 } } },
    });
    if !projectile {
        value.as_object_mut().unwrap().remove("projectile");
    }
    serde_json::from_value::<WeaponDescriptor>(value)
        .unwrap()
        .validate()
        .unwrap()
}

struct Actors {
    pawn: EntityId,
    weapon: EntityId,
    target: EntityId,
}

fn actors(registry: &mut EntityRegistry, descriptor: &WeaponDescriptor, host: bool) -> Actors {
    if host {
        // The listen-host local seat is intentionally unarmed. The owned remote
        // pawn must never be selected by the legacy local-movement fallback.
        let local = registry.spawn(Transform {
            position: Vec3::new(20.0, 0.0, 0.0),
            ..Default::default()
        });
        registry.set_component(local, movement()).unwrap();
        registry.set_component(local, health(None)).unwrap();
        registry.mark_local_player_pawn(local).unwrap();
    }
    let pawn = registry.spawn(Transform::default());
    registry.set_component(pawn, movement()).unwrap();
    registry.set_component(pawn, health(None)).unwrap();
    if !host {
        registry.mark_local_player_pawn(pawn).unwrap();
    }
    let weapon = registry.spawn(Transform::default());
    registry
        .set_component(
            weapon,
            WeaponComponent::from_descriptor_with_canonical(descriptor, Some("conditioned-weapon")),
        )
        .unwrap();
    registry
        .set_component(
            weapon,
            DescriptorProvenance {
                canonical_name: "conditioned-weapon".into(),
                owned_components: BTreeSet::from([DescriptorComponentKind::Weapon]),
                map_overrides: BTreeSet::new(),
                spawn_path: DescriptorSpawnPath::DefaultWeapon,
            },
        )
        .unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(weapon);
    registry.set_component(pawn, inventory).unwrap();
    let target = registry.spawn(Transform {
        position: Vec3::new(0.0, 0.5, -5.0),
        ..Default::default()
    });
    registry
        .set_component(
            target,
            health(Some(Hitbox {
                half_extents: Vec3::ONE,
                offset: Vec3::ZERO,
            })),
        )
        .unwrap();
    Actors {
        pawn,
        weapon,
        target,
    }
}

struct Fixture {
    server: NetServer,
    client: NetClient,
    up: PacketConditioner,
    down: PacketConditioner,
    cut_up: bool,
    cut_packets: usize,
    host: Rc<RefCell<EntityRegistry>>,
    local: Rc<RefCell<EntityRegistry>>,
    host_actors: Actors,
    local_actors: Actors,
    allocator: netcode::NetworkIdAllocator,
    owners: netcode::MovementOwners,
    queues: netcode::HostCommandQueues,
    open: netcode::OpenAuthorizedShots,
    pending: netcode::PendingHitDeclarations,
    replication: ServerReplication,
    slots: netcode::HostStateReplication,
    frame: ClientWeaponFrame,
    predicted: weapon::ClientPredictedShots,
    projection: weapon::ReplicatedWeaponProjection,
    world: crate::collision::CollisionWorld,
    zones: postretro_sim::scripting_systems::hit_zones::HitZoneStore,
    ai: AiRuntime,
    progress: ProgressTracker,
    movers: crate::kinematic_mover::MoverTickStateTable,
    weaponless: HashSet<EntityId>,
    tick: u32,
    sent: Vec<wire::InputCommand>,
    /// Host tick on which each `sent` command left the client.
    sent_at: Vec<u32>,
    snapshots: Vec<weapon::ResolvedWeaponShot>,
    snapshot_ticks: Vec<u32>,
    hidden_launches: usize,
    authorized: Vec<AuthorizedShot>,
    presentations: Vec<sim::RemoteProjectilePresentationLaunch>,
    outcomes: Vec<wire::ActivationOutcome>,
    verdicts: Vec<wire::ShotVerdict>,
    hit_refusals: Vec<ShotId>,
    retired_hit_feedback: Vec<ShotId>,
    reconciled: Vec<(ActivationToken, bool, bool)>,
    sources: Vec<netcode::ResolutionSource>,
    resolved_ticks: Vec<u32>,
    /// Retract a rejected activation's predictions through the same
    /// `retract_rejected_activation` the binary's outcome handling calls.
    mirror_rejection_despawn: bool,
    /// Predicted shots whose flight ended naturally (range/lifetime expiry).
    expired: Vec<ShotId>,
    /// `(host tick, resolved client tick, source)` per host resolution.
    playout: Vec<(u32, u32, netcode::ResolutionSource)>,
    /// `(host tick, start)` per retained start the lane refused.
    lane_refusals: Vec<(u32, ActivationToken)>,
    /// `(host tick, command)` per host resolution, as playout delivered it.
    resolved_commands: Vec<(u32, netcode::ResolvedPawnCommand)>,
    /// Camera pitch the next `send_input` declares.
    aim_pitch: f32,
    /// `(host tick, owned pawn facing yaw)` after each host simulation tick.
    host_facing: Vec<(u32, f32)>,
    weapon_owners: netcode::WeaponOwners,
    /// Switch declarations the client has sent, numbered as production does.
    next_switch_id: u32,
    /// `(host tick, declaration id, accepted)` per switch outcome the client received.
    switch_outcomes: Vec<(u32, u32, bool)>,
    /// `(host tick, active slot)` on the host after each tick's switches applied.
    host_active: Vec<(u32, usize)>,
    /// `(host tick, weapon, outcome)` per host reload delivery.
    reloads: Vec<(u32, EntityId, sim::ReloadOutcome)>,
    /// Host touch stage state and the descriptors it resolves drops against.
    touch: crate::sim::touch::TouchSystem,
    descriptors: Vec<postretro_entities::EntityTypeDescriptor>,
}

impl Fixture {
    fn new(link: LinkConfig, descriptor: WeaponDescriptor) -> Self {
        let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_address = server_socket.local_addr().unwrap();
        let client_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let fingerprint = [0x5a; 32];
        let origin = Duration::from_secs(1);
        let mut server =
            NetServer::new(server_socket, server_address, 8, origin, Some(fingerprint)).unwrap();
        let mut client = NetClient::new(
            client_socket,
            server_address,
            CLIENT,
            origin,
            Some(fingerprint),
            None,
        )
        .unwrap();
        server.set_mod_identity("test.mod".into(), "1.0.0".into());
        client.set_mod_identity("test.mod".into(), "1.0.0".into());
        server.set_mod_digest(Some(fingerprint));
        client.set_mod_digest(Some(fingerprint));
        server.set_level_parity(Some(("test-level".into(), fingerprint)));
        client.set_level_parity(Some(("test-level".into(), fingerprint)));
        server.add_relay_connection(CLIENT, None);
        client.set_connected();
        let mut host = EntityRegistry::new();
        let mut local = EntityRegistry::new();
        let host_actors = actors(&mut host, &descriptor, true);
        let local_actors = actors(&mut local, &descriptor, false);
        let mut allocator = netcode::NetworkIdAllocator::for_test();
        allocator.stamp(host_actors.pawn);
        allocator.stamp(host_actors.target);
        let mut owners = netcode::MovementOwners::default();
        owners.set_for_test(host_actors.pawn, CLIENT);
        let mut down_link = link;
        down_link.seed ^= 0x5a5a;
        let mut fixture = Self {
            server,
            client,
            up: PacketConditioner::new(link),
            down: PacketConditioner::new(down_link),
            cut_up: false,
            cut_packets: 0,
            host: Rc::new(RefCell::new(host)),
            local: Rc::new(RefCell::new(local)),
            host_actors,
            local_actors,
            allocator,
            owners,
            queues: Default::default(),
            open: Default::default(),
            pending: Default::default(),
            replication: Default::default(),
            slots: Default::default(),
            frame: Default::default(),
            predicted: weapon::ClientPredictedShots::new(),
            projection: Default::default(),
            world: crate::collision::CollisionWorld::new(),
            zones: postretro_sim::scripting_systems::hit_zones::HitZoneStore::new(),
            ai: AiRuntime::new(),
            progress: ProgressTracker::new(),
            movers: Default::default(),
            weaponless: HashSet::new(),
            tick: 0,
            sent: Vec::new(),
            sent_at: Vec::new(),
            snapshots: Vec::new(),
            snapshot_ticks: Vec::new(),
            hidden_launches: 0,
            authorized: Vec::new(),
            presentations: Vec::new(),
            outcomes: Vec::new(),
            verdicts: Vec::new(),
            hit_refusals: Vec::new(),
            retired_hit_feedback: Vec::new(),
            reconciled: Vec::new(),
            sources: Vec::new(),
            resolved_ticks: Vec::new(),
            mirror_rejection_despawn: false,
            expired: Vec::new(),
            playout: Vec::new(),
            lane_refusals: Vec::new(),
            resolved_commands: Vec::new(),
            aim_pitch: 0.0,
            host_facing: Vec::new(),
            weapon_owners: Default::default(),
            next_switch_id: 0,
            switch_outcomes: Vec::new(),
            host_active: Vec::new(),
            reloads: Vec::new(),
            touch: Default::default(),
            descriptors: Vec::new(),
        };
        for _ in 0..256 {
            fixture.transport();
            if fixture.server.is_participating(CLIENT) && fixture.client.is_participating() {
                break;
            }
        }
        assert!(
            fixture.server.is_participating(CLIENT) && fixture.client.is_participating(),
            "conditioned parity handshake must finish"
        );
        fixture
    }

    /// Give both peers' pawns a second weapon in slot 1. Returns the host instance.
    fn equip_second(&mut self, descriptor: &WeaponDescriptor) -> EntityId {
        let mut host_weapon = None;
        for (registry, pawn) in [
            (&self.host, self.host_actors.pawn),
            (&self.local, self.local_actors.pawn),
        ] {
            let mut registry = registry.borrow_mut();
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(
                    weapon,
                    WeaponComponent::from_descriptor_with_canonical(
                        descriptor,
                        Some("conditioned-second"),
                    ),
                )
                .expect("second weapon component attaches");
            registry
                .set_component(
                    weapon,
                    DescriptorProvenance {
                        canonical_name: "conditioned-second".into(),
                        owned_components: BTreeSet::from([DescriptorComponentKind::Weapon]),
                        map_overrides: BTreeSet::new(),
                        spawn_path: DescriptorSpawnPath::DefaultWeapon,
                    },
                )
                .expect("second weapon provenance attaches");
            let mut inventory = registry
                .get_component::<Inventory>(pawn)
                .expect("pawn holds an inventory")
                .clone();
            inventory.wieldables[1] = Some(weapon);
            registry
                .set_component(pawn, inventory)
                .expect("pawn inventory updates");
            host_weapon.get_or_insert(weapon);
        }
        let host_weapon = host_weapon.expect("the host registry is equipped first");
        self.allocator.stamp(host_weapon);
        host_weapon
    }

    fn pawn_network(&self) -> u32 {
        self.allocator
            .network_id_for_entity(self.host_actors.pawn)
            .unwrap()
            .0
    }
    fn shot_id(&self, token: ActivationToken, ordinal: u8) -> ShotId {
        ShotId::from_parts(self.pawn_network(), token.start_tick, token.lane, ordinal)
    }

    fn transport(&mut self) {
        let dt = Duration::from_millis(17);
        self.client.update_connections(dt);
        self.server.update_connections(dt);
        for packet in self.client.packets_to_send() {
            if self.cut_up {
                self.cut_packets += 1;
            } else {
                self.up.enqueue(packet);
            }
        }
        self.down.enqueue_all(self.server.packets_to_send(CLIENT));
        self.up.advance(17);
        self.down.advance(17);
        for packet in self.up.take_ready() {
            self.server.process_packet_from(&packet, CLIENT);
        }
        for packet in self.down.take_ready() {
            self.client.process_packet(&packet);
        }
        let poll = self.server.poll_handshakes();
        for (client_id, declaration) in poll.switch_declarations {
            netcode::host_retain_switch_declaration(
                &mut self.queues,
                &self.owners,
                client_id,
                declaration,
            );
        }
        for message in self.client.drain_control() {
            match message {
                wire::ServerControlMessage::SwitchAccepted(accepted) => self
                    .switch_outcomes
                    .push((self.tick, accepted.declaration_id, true)),
                wire::ServerControlMessage::SwitchRefused(refused) => {
                    self.switch_outcomes
                        .push((self.tick, refused.declaration_id, false))
                }
                _ => {}
            }
        }
        for bytes in self.client.drain_input() {
            match wire::decode::<wire::ServerMessage>(&bytes).unwrap() {
                wire::ServerMessage::ActivationOutcomes(outcomes) => {
                    for outcome in outcomes {
                        let effect = {
                            let mut registry = self.local.borrow_mut();
                            self.frame.records.outcome(&mut registry, outcome)
                        };
                        if let Some(effect) = effect {
                            if let Some(recovery) = effect.recovery_ms
                                && let Ok(ComponentValue::Weapon(component)) = self
                                    .local
                                    .borrow_mut()
                                    .get_component_value_mut(effect.weapon, ComponentKind::Weapon)
                            {
                                self.predicted.reconcile_cooldown(
                                    effect.weapon,
                                    component,
                                    recovery,
                                );
                            }
                            if self.mirror_rejection_despawn && effect.rejected {
                                self.frame.retract_rejected_activation(
                                    &mut self.local.borrow_mut(),
                                    &mut self.predicted,
                                    effect.token,
                                    effect.weapon,
                                );
                            }
                            self.reconciled
                                .push((effect.token, effect.terminal, effect.rejected));
                        }
                        self.outcomes.push(outcome);
                    }
                }
                wire::ServerMessage::ShotVerdicts(message) => {
                    for verdict in message.verdicts {
                        self.predicted.apply_verdict(
                            &mut self.local.borrow_mut(),
                            netcode::wire_convert::shot_id_from_wire(verdict.shot_id),
                            verdict.accept,
                            verdict.hit_accepted,
                        );
                        self.verdicts.push(verdict);
                    }
                }
                wire::ServerMessage::HitRefused(shot_id) => {
                    let shot_id = netcode::wire_convert::shot_id_from_wire(shot_id);
                    // Same HIT-only handler as the production App receive path:
                    // settle feedback without claiming FIRE acceptance/rejection.
                    if self.predicted.refuse_hit(shot_id).is_some() {
                        self.retired_hit_feedback.push(shot_id);
                    }
                    self.hit_refusals.push(shot_id);
                }
                _ => {}
            }
        }
    }

    fn send_input(&mut self, tick: u32, command: &sim::SimCommand) {
        // Fixed fixture data, not an alternative serializer or queue: actual
        // bitcode and the production incoming converter handle these messages.
        assert_eq!(command.movement.wish_dir, Vec2::ZERO);
        let input = wire::InputCommand {
            client_tick: tick,
            movement: wire::WireMovementInput {
                wish_dir: [0.0; 2],
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                facing_yaw: command.movement.facing_yaw,
                use_pressed: false,
                drop_pressed: command.movement.drop_pressed,
                aim_pitch: self.aim_pitch,
                firing_slot: command.firing_slot,
            },
            fire_button: wire::WireFireButtonState {
                pressed: command.fire_button.pressed,
                active: command.fire_button.active,
            },
            secondary_button: wire::WireFireButtonState {
                pressed: command.secondary_button.pressed,
                active: command.secondary_button.active,
            },
            reload: command.reload,
            activation: wire::WireActivationInput {
                initiation: command.activation.initiation.map(wire_token),
                release: command
                    .activation
                    .release
                    .map(|release| wire::WireActivationRelease {
                        token: wire_token(release.token),
                        release_tick: release.release_tick,
                    }),
                cancel: command.activation.cancel.map(wire_token),
            },
        };
        self.client
            .send_input(wire::encode(&wire::ClientMessage::Input(input)));
        self.sent.push(input);
        self.sent_at.push(self.tick);
    }

    fn declare(&mut self, shot: ShotId, records: Vec<wire::HitRecord>) {
        self.client
            .send_input(wire::encode(&wire::ClientMessage::HitDeclaration(
                wire::HitDeclaration {
                    shot_id: netcode::wire_convert::shot_id_to_wire(shot),
                    records,
                },
            )));
    }

    fn target_record(&self) -> wire::HitRecord {
        wire::HitRecord {
            target: self
                .allocator
                .network_id_for_entity(self.host_actors.target)
                .unwrap()
                .0,
            point: [0.0, 0.5, -4.0],
            normal: [0.0, 0.0, 1.0],
            zone: None,
        }
    }

    fn predict(&mut self, tick: u32, command: &mut sim::SimCommand) {
        command.input_tick = tick;
        // Match the real main loop: equip/switch first, fixed activation second,
        // spatial resolution last. This catches accidental double advancement.
        let (switch_accepted, _) = sim::simulate_client_wieldable_tick(
            self.local.clone(),
            &self.world,
            &self.zones,
            Some(self.local_actors.pawn),
            false,
            command.select_slot,
            command.fire_button,
            command.reload,
            0.0,
            DT,
        );
        // Production declares an accepted switch on Control, stamped with the
        // tick of the command it was made on.
        if switch_accepted && let Some(slot) = command.select_slot {
            self.client
                .send_switch_declaration(wire::ClientSwitchDeclaration {
                    declaration_id: self.next_switch_id,
                    slot: u8::try_from(slot).expect("inventory slots fit a u8"),
                    client_tick: tick,
                });
            self.next_switch_id += 1;
        }
        let pawn_network = self.pawn_network();
        self.frame.predict(
            &mut self.local.borrow_mut(),
            command,
            tick,
            pawn_network,
            DT * 1000.0,
            WeaponPlacementDescriptor::default(),
            &self.projection,
        );
        for queued in std::mem::take(&mut self.frame.due) {
            let id = queued.shot.activation.shot_id;
            let resolution = weapon::resolve_client_shot(
                Some(queued.pawn),
                queued.component,
                &queued.pellet_salt,
                queued.slot,
                Vec3::new(0.0, 0.5, 0.0),
                Vec3::NEG_Z,
                &queued.placement,
                &self.world,
                &self.local.borrow(),
                &self.zones,
                0.0,
                queued.shot.clone(),
            );
            self.predicted.predict(
                id,
                queued.weapon,
                &resolution,
                queued.cooldown_before,
                queued.cooldown_after,
                queued.presentation,
            );
            self.snapshots.push(queued.shot);
            self.snapshot_ticks.push(tick);
            if let Some(launch) = resolution.projectile_launch {
                let entity = sim::spawn_projectile(
                    &mut self.local.borrow_mut(),
                    queued.pawn,
                    queued.weapon,
                    launch,
                    Some(id),
                    sim::ProjectileSource {
                        weapon: Some("conditioned-weapon".into()),
                        activation: None,
                    },
                )
                .expect("semantic predicted projectile");
                sim::set_predicted_projectile_visible(
                    &mut self.local.borrow_mut(),
                    entity,
                    queued.presentation == weapon::ClientPullPresentation::Fire,
                );
                if queued.presentation != weapon::ClientPullPresentation::Fire {
                    assert!(!self.local.borrow().get_component::<postretro_entities::components::projectile::ProjectileComponent>(entity).unwrap().predicted_visible);
                    self.hidden_launches += 1;
                }
            } else {
                let target = self.local_actors.target;
                let target_network = self
                    .allocator
                    .network_id_for_entity(self.host_actors.target)
                    .unwrap()
                    .0;
                let records = resolution
                    .hits
                    .into_iter()
                    .map(|hit| {
                        assert_eq!(
                            hit.target, target,
                            "real client ray must hit the seeded target"
                        );
                        wire::HitRecord {
                            target: target_network,
                            point: hit.point.to_array(),
                            normal: hit.normal.to_array(),
                            zone: hit.zone,
                        }
                    })
                    .collect();
                self.declare(id, records);
            }
        }
    }

    fn advance_projectiles(&mut self) {
        let mut results = Vec::new();
        sim::advance_predicted(
            &self.local,
            &self.world,
            &self.zones,
            0.0,
            DT,
            &mut |result| results.push(result),
        );
        for result in results {
            match result {
                sim::PredictedProjectileResolution::Impact {
                    shot_id, impact, ..
                } => {
                    assert_eq!(impact.target, Some(self.local_actors.target));
                    let target = self
                        .allocator
                        .network_id_for_entity(self.host_actors.target)
                        .unwrap()
                        .0;
                    self.declare(
                        shot_id,
                        vec![wire::HitRecord {
                            target,
                            point: impact.point.to_array(),
                            normal: impact.normal.to_array(),
                            zone: impact.zone,
                        }],
                    );
                }
                sim::PredictedProjectileResolution::Expired { shot_id } => {
                    self.expired.push(shot_id);
                    self.declare(shot_id, Vec::new())
                }
            }
        }
    }

    fn host_tick(&mut self) {
        self.transport();
        netcode::host_handle_client_messages(
            &mut self.server,
            &mut self.replication,
            &mut self.slots,
            &mut self.queues,
            &mut self.pending,
            &mut self.open,
            CLIENT,
            self.tick,
            u64::from(self.tick) * 16_667,
        );
        let resolved = netcode::host_resolve_remote_commands(&self.owners, &mut self.queues);
        netcode::host_apply_switch_deliveries(
            &mut self.host.borrow_mut(),
            &mut self.server,
            &self.owners,
            &mut self.queues,
            &mut self.weapon_owners,
            false,
        );
        let active = self
            .host
            .borrow()
            .get_component::<Inventory>(self.host_actors.pawn)
            .map_or(usize::MAX, |inventory| inventory.active_slot);
        self.host_active.push((self.tick, active));
        let mut commands = Vec::new();
        for resolved in resolved {
            self.sources.push(resolved.source);
            self.resolved_ticks.push(resolved.client_tick);
            self.playout
                .push((self.tick, resolved.client_tick, resolved.source));
            if let Some(start) = resolved.rejected_activation {
                self.lane_refusals.push((self.tick, start));
            }
            self.resolved_commands.push((self.tick, resolved.clone()));
            let mut registry = self.host.borrow_mut();
            observe_lifecycle(
                &mut registry,
                &mut self.allocator,
                &mut self.queues,
                &mut self.server,
                CLIENT,
                resolved.pawn,
                self.tick,
            );
            let mut command = App::prepare_remote_pawn_command(
                &self.allocator,
                &registry,
                &mut self.weaponless,
                self.tick,
                &resolved,
            );
            guard_initiation(
                &mut registry,
                &mut self.allocator,
                &mut self.queues,
                &mut self.server,
                &mut command,
            );
            commands.push(command);
        }
        // The production drop route: each remote drop press becomes a touch-stage edge.
        let drops: std::collections::HashMap<_, _> = commands
            .iter()
            .filter(|remote| remote.command.drop_pressed)
            .map(|remote| {
                (
                    crate::trigger_system::PlayerId::Remote(remote.owner_client_id),
                    true,
                )
            })
            .collect();
        let factions = postretro_entities::FactionRegistry::default();
        let sentiment = RefCell::new(postretro_entities::FactionSentimentState::default());
        let events = sim::simulate_tick_with_presentation_aim(
            self.host.clone(),
            &self.world,
            &self.zones,
            None,
            0.0,
            false,
            0.0,
            (0.0, 0.0),
            &mut self.progress,
            postretro_ai::tick_runner!(&mut self.ai),
            &[],
            &mut self.movers,
            &commands,
            &neutral(),
            |_| sim::PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            &mut self.touch,
            &self.descriptors,
            0,
            &factions,
            &sentiment,
            None,
            &std::collections::HashMap::new(),
            &drops,
            None,
            |_, _| {},
            |_| {},
            sim::TimingGate::OFF,
        );
        let facing = self
            .host
            .borrow()
            .get_component::<Transform>(self.host_actors.pawn)
            .map(|transform| transform.rotation.to_euler(glam::EulerRot::YXZ).0)
            .unwrap_or(f32::NAN);
        self.host_facing.push((self.tick, facing));
        self.reloads.extend(
            events
                .reload_deliveries
                .iter()
                .map(|delivery| (self.tick, delivery.weapon(), delivery.outcome)),
        );
        for progress in events.remote_activation_progress {
            record_activation_progress(&mut self.allocator, &mut self.queues, &progress, |fact| {
                match fact {
                    HostActivationFact::Outcome(outcome) => {
                        netcode::send_activation_outcome(&mut self.server, CLIENT, outcome)
                    }
                    HostActivationFact::ShotVerdict {
                        shot_id,
                        accept,
                        hit_accepted,
                    } => netcode::send_shot_verdict(
                        &mut self.server,
                        CLIENT,
                        shot_id,
                        accept,
                        hit_accepted,
                    ),
                }
            });
        }
        release_departed_weapons(&mut self.host.borrow_mut(), &mut self.queues, &self.owners);
        for open in events.authorized_shots {
            self.authorized.push(open.shot.clone());
            self.open.record(open.shot, open.owner_client_id);
        }
        self.presentations
            .extend(events.remote_projectile_presentation_launches);
        netcode::host_flush_pending_hit_declarations(
            &mut self.server,
            &mut self.host.borrow_mut(),
            &self.world,
            &self.zones,
            &self.allocator,
            &self.owners,
            &self.queues,
            &mut self.open,
            &mut self.pending,
            self.tick,
            0.0,
            |_| {},
            |_, _| {},
            |_| {},
        );
        self.tick = self.tick.wrapping_add(1);
    }

    fn step(&mut self, tick: u32, mut command: sim::SimCommand) {
        self.predict(tick, &mut command);
        self.send_input(tick, &command);
        self.advance_projectiles();
        self.host_tick();
    }

    fn idle(&mut self, next: u32, count: u32) {
        for offset in 0..count {
            self.step(next.wrapping_add(offset), neutral());
        }
    }

    fn ammo(&self) -> u32 {
        self.host
            .borrow()
            .get_component::<WeaponComponent>(self.host_actors.weapon)
            .unwrap()
            .magazine
    }
    fn damage(&self) -> f32 {
        TARGET_HP
            - self
                .host
                .borrow()
                .get_component::<HealthComponent>(self.host_actors.target)
                .unwrap()
                .current
    }
    fn host_charging(&self) -> bool {
        self.host
            .borrow()
            .get_component::<WeaponComponent>(self.host_actors.weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_some_and(|cursor| cursor.phase == postretro_foundation::ActivationPhase::Charging)
    }
    fn assert_no_host_shot(&self) {
        assert!(self.authorized.is_empty());
        assert_eq!(self.ammo(), MAGAZINE);
        near(self.damage(), 0.0);
    }
    fn accepted(&self, activation: ActivationToken) -> usize {
        self.outcomes.iter().filter(|o| matches!(o, wire::ActivationOutcome::InitiationAccepted { token, .. } if *token == wire_token(activation))).count()
    }
    fn cancelled(&self, activation: ActivationToken) -> bool {
        self.outcomes.iter().any(|o| matches!(o, wire::ActivationOutcome::Cancelled { token, .. } if *token == wire_token(activation)))
    }
}

#[test]
fn conditioned_activation_clean_and_mandated_link_preserve_full_charge_sequence_and_scaled_parity()
{
    for link in [LinkConfig::perfect(), mandated_link()] {
        for projectile in [false, true] {
            for scale in [3.0, 6.0] {
                // The only descriptor difference between paired runs is the
                // damage expression constant. Inputs, cost, size, and flight match.
                let mut fixture = Fixture::new(
                    link,
                    descriptor_with_damage(projectile, true, 3, "press", scale),
                );
                let start = token(u32::MAX - 20, ActivationLane::Secondary);
                fixture.step(start.start_tick, held(start.lane, true));
                fixture.step(start.start_tick.wrapping_add(1), held(start.lane, false));
                for _ in 0..64 {
                    if fixture.accepted(start) == 1 {
                        break;
                    }
                    fixture.host_tick();
                }
                assert_eq!(fixture.accepted(start), 1);
                let held_losses_before = fixture.up.dropped();
                for offset in 2..96 {
                    fixture.step(
                        start.start_tick.wrapping_add(offset),
                        held(start.lane, false),
                    );
                    if offset == 24 {
                        assert!(fixture.host_charging());
                        fixture.assert_no_host_shot();
                    }
                }
                fixture.assert_no_host_shot();
                assert!(
                    fixture.host_charging(),
                    "reaching full charge never invents a release"
                );
                if link.loss_probability > 0.0 {
                    assert!(
                        fixture.up.dropped() > held_losses_before,
                        "ordinary held-command packets must actually be lost under the mandated seed"
                    );
                    assert!(fixture.sources.contains(&netcode::ResolutionSource::Held));
                    assert!(
                        fixture
                            .sources
                            .contains(&netcode::ResolutionSource::Neutral)
                    );
                }
                let release_tick = start.start_tick.wrapping_add(96);
                let mut release = neutral();
                release.activation.release = Some(ActivationRelease {
                    token: start,
                    release_tick,
                });
                fixture.step(release_tick, release);
                fixture.idle(release_tick.wrapping_add(1), 120);
                assert_eq!(fixture.accepted(start), 1);
                assert_eq!(fixture.authorized.len(), 3);
                assert_eq!(
                    fixture.snapshots.len(),
                    3,
                    "equip and prediction must advance exactly once"
                );
                assert_eq!(
                    fixture.snapshot_ticks,
                    [
                        release_tick,
                        release_tick.wrapping_add(3),
                        release_tick.wrapping_add(6)
                    ],
                    "client issuance preserves each authored 50 ms wait across tick wrap"
                );
                for pair in fixture.authorized.windows(2) {
                    assert_eq!(
                        pair[1].fire_tick.wrapping_sub(pair[0].fire_tick),
                        3,
                        "host issuance preserves each authored 50 ms wait independently of link delay"
                    );
                }
                assert_eq!(fixture.ammo(), MAGAZINE - 6);
                near(fixture.damage(), 30.0 * scale);
                for ordinal in 0..3 {
                    let id = fixture.shot_id(start, ordinal);
                    assert_eq!(fixture.authorized[usize::from(ordinal)].shot_id, id);
                    let shot = &fixture.snapshots[usize::from(ordinal)];
                    assert_eq!(shot.activation.shot_id, id);
                    near(shot.activation.values.damage, 10.0 * scale);
                    near(
                        fixture.authorized[usize::from(ordinal)].damage,
                        10.0 * scale,
                    );
                    near(shot.activation.values.range, 100.0);
                    assert_eq!(
                        shot.activation.values.resource_cost,
                        postretro_foundation::ShotResourceCost::Ammo(2)
                    );
                    if projectile {
                        let descriptor = shot.projectile.as_ref().unwrap();
                        near(descriptor.speed, 60.0);
                        near(descriptor.radius, 0.125);
                        let ProjectileBodyVisual::Sprite { size, .. } = descriptor.visual.body
                        else {
                            panic!("sprite fixture");
                        };
                        near(size, 0.375);
                        assert_eq!(fixture.presentations.len(), 3);
                        let host = &fixture.presentations[usize::from(ordinal)];
                        assert_eq!(host.shot_id, id);
                        near(host.projectile.speed, descriptor.speed);
                        near(host.projectile.radius, descriptor.radius);
                        near(host.range, shot.activation.values.range);
                        let ProjectileBodyVisual::Sprite {
                            size: host_size, ..
                        } = host.projectile.visual.body
                        else {
                            panic!("host sprite fixture");
                        };
                        near(host_size, size);
                    }
                    assert!(
                        fixture
                            .verdicts
                            .iter()
                            .any(|v| v.shot_id == netcode::wire_convert::shot_id_to_wire(id)
                                && v.accept
                                && v.hit_accepted)
                    );
                }
                let accepted_index = fixture.outcomes.iter().position(|o| matches!(o, wire::ActivationOutcome::InitiationAccepted { token, .. } if *token == wire_token(start))).unwrap();
                let execution_index = fixture.outcomes.iter().position(|o| matches!(o, wire::ActivationOutcome::ExecutionAccepted { token, charge_millionths: 1_000_000, .. } if *token == wire_token(start))).unwrap();
                assert!(accepted_index < execution_index);
                assert!(fixture.reconciled.contains(&(start, true, false)));
                near(
                    fixture
                        .local
                        .borrow()
                        .get_component::<HealthComponent>(fixture.local_actors.target)
                        .unwrap()
                        .current,
                    TARGET_HP,
                );
                if link.loss_probability != 0.0 {
                    assert!(fixture.up.dropped() > 0 && fixture.down.dropped() > 0);
                }
            }
        }
    }
}

// Regression: accepted charge must clamp after queue compression and correct the live owner flight.
#[test]
fn conditioned_activation_backlog_clamps_accepted_charge_and_corrects_live_owner_projectile() {
    use postretro_entities::components::projectile::ProjectileComponent;
    use postretro_entities::components::sprite_visual::SpriteVisual;

    let mut authored = serde_json::to_value(descriptor(true, true, 1, "press")).unwrap();
    let charge_axis = json!({
        "op": "add", "a": { "op": "const", "value": 1 },
        "b": { "op": "input", "name": "charge" },
    });
    for axis in [
        "range",
        "projectileSize",
        "projectileRadius",
        "projectileSpeed",
    ] {
        authored["secondary"]["steps"][0]["scale"][axis] = charge_axis.clone();
    }
    let descriptor = serde_json::from_value::<WeaponDescriptor>(authored)
        .unwrap()
        .validate()
        .unwrap();
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor);
    let start = token(100, ActivationLane::Secondary);
    fixture.step(100, held(start.lane, true));
    fixture.step(101, held(start.lane, false));
    assert_eq!(fixture.tick, 2);
    assert_eq!(fixture.resolved_ticks.last(), Some(&100));
    assert!(fixture.host_charging());
    // Deliver the real initiation outcome before producing the owner's full-charge shot.
    fixture.transport();
    assert_eq!(fixture.accepted(start), 1);
    for tick in 102..130 {
        let mut command = held(start.lane, false);
        fixture.predict(tick, &mut command);
        fixture.send_input(tick, &command);
    }
    let mut release = neutral();
    release.activation.release = Some(ActivationRelease {
        token: start,
        release_tick: 130,
    });
    fixture.predict(130, &mut release);
    fixture.send_input(130, &release);
    assert_eq!(fixture.snapshots.len(), 1);
    assert_eq!(fixture.snapshot_ticks, [130]);
    near(fixture.snapshots[0].activation.values.damage, 60.0);
    assert_eq!(
        fixture.tick, 2,
        "transport backlog advances no host simulation"
    );

    let id = fixture.shot_id(start, 0);
    let launched = fixture
        .local
        .borrow()
        .iter_with_kind(ComponentKind::Projectile)
        .filter_map(|(entity, value)| match value {
            ComponentValue::Projectile(component) if component.predicted_shot_id == Some(id) => {
                Some(entity)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [entity] = launched.as_slice() else {
        panic!("exactly one real predicted launch must exist");
    };
    let entity = *entity;
    let launch_position = fixture
        .local
        .borrow()
        .get_component::<Transform>(entity)
        .unwrap()
        .position;
    fixture.advance_projectiles();
    fixture.advance_projectiles();
    let before = fixture
        .local
        .borrow()
        .get_component::<ProjectileComponent>(entity)
        .unwrap()
        .clone();
    let before_position = fixture
        .local
        .borrow()
        .get_component::<Transform>(entity)
        .unwrap()
        .position;
    assert!(
        before_position.distance(launch_position) > 0.5,
        "flight must have advanced before correction"
    );
    near(before.damage, 60.0);
    near(before.speed, 120.0);
    near(before.radius, 0.25);
    near(
        fixture
            .local
            .borrow()
            .get_component::<SpriteVisual>(entity)
            .unwrap()
            .size,
        0.5,
    );
    let traveled = 200.0 - before.remaining_range;
    assert!(traveled > 0.5);

    fixture.host_tick();
    assert_eq!(fixture.tick, 3);
    assert_eq!(fixture.resolved_ticks.last(), Some(&129));
    assert_eq!(
        fixture.sources.last(),
        Some(&netcode::ResolutionSource::Real)
    );
    assert_eq!(fixture.authorized.len(), 1);
    assert_eq!(fixture.authorized[0].shot_id, id);
    assert_eq!(fixture.authorized[0].fire_tick, 2);
    near(fixture.authorized[0].damage, 20.0);
    near(fixture.authorized[0].range, 100.0 * (4.0 / 3.0));
    near(fixture.authorized[0].projectile_speed.unwrap(), 80.0);
    near(fixture.authorized[0].projectile_radius.unwrap(), 1.0 / 6.0);
    assert_eq!(fixture.ammo(), MAGAZINE - 2);
    near(fixture.damage(), 0.0);
    assert!(
        !fixture
            .outcomes
            .iter()
            .any(|outcome| matches!(outcome, wire::ActivationOutcome::ExecutionAccepted { .. }))
    );

    // Receive the outcome emitted by the production host path; do not inject a correction.
    fixture.transport();
    let execution = fixture
        .outcomes
        .iter()
        .filter(|outcome| {
            matches!(outcome,
        wire::ActivationOutcome::ExecutionAccepted { token, charge_millionths: 333_333, .. }
        if *token == wire_token(start))
        })
        .count();
    assert_eq!(
        execution, 1,
        "30 client ticks clamp to one host tick plus nine tolerance ticks"
    );
    let after = fixture
        .local
        .borrow()
        .get_component::<ProjectileComponent>(entity)
        .unwrap()
        .clone();
    let after_position = fixture
        .local
        .borrow()
        .get_component::<Transform>(entity)
        .unwrap()
        .position;
    near(after_position.distance(before_position), 0.0);
    near(after.damage, 20.0);
    near(after.speed, 80.0);
    near(after.radius, 1.0 / 6.0);
    near(after.remaining_range, 100.0 * (4.0 / 3.0) - traveled);
    near(after.remaining_lifetime, before.remaining_lifetime);
    assert!(!after.spawned, "correction must not replay the spawn pass");
    near(
        fixture
            .local
            .borrow()
            .get_component::<SpriteVisual>(entity)
            .unwrap()
            .size,
        1.0 / 3.0,
    );
    let live = fixture.local.borrow().iter_with_kind(ComponentKind::Projectile)
        .filter(|(_, value)| matches!(value, ComponentValue::Projectile(component) if component.predicted_shot_id == Some(id)))
        .count();
    assert_eq!(live, 1);
    assert_eq!(fixture.snapshots.len(), 1);
    assert_eq!(fixture.presentations.len(), 1);
    fixture.idle(131, 80);
    assert_eq!(fixture.authorized.len(), 1);
    assert_eq!(fixture.snapshots.len(), 1);
    assert_eq!(fixture.ammo(), MAGAZINE - 2);
    near(fixture.damage(), 20.0);
    assert!(fixture.verdicts.iter().any(|verdict| verdict.shot_id
        == netcode::wire_convert::shot_id_to_wire(id)
        && verdict.accept
        && verdict.hit_accepted));
}

#[test]
fn conditioned_activation_early_future_declaration_and_duplicate_stale_start_debit_each_ordinal_once()
 {
    let mut fixture = Fixture::new(mandated_link(), descriptor(false, false, 3, "press"));
    let start = token(100, ActivationLane::Primary);
    fixture.declare(fixture.shot_id(start, 2), vec![fixture.target_record()]);
    fixture.step(100, held(start.lane, true));
    fixture.step(101, neutral());
    for _ in 0..64 {
        if !fixture.authorized.is_empty() {
            break;
        }
        fixture.host_tick();
    }
    assert!(matches!(
        fixture
            .queues
            .activations
            .status(CLIENT, fixture.shot_id(start, 2), fixture.tick),
        netcode::activation_ledger::OrdinalStatus::Pending { .. }
    ));
    fixture.idle(102, 149);
    assert_eq!(fixture.accepted(start), 1);
    assert_eq!(fixture.authorized.len(), 3);
    assert_eq!(fixture.ammo(), MAGAZINE - 3);
    near(fixture.damage(), 90.0);
    for tick in [100, 100, 100] {
        let mut replay = held(start.lane, true);
        replay.activation.initiation = Some(start);
        fixture.send_input(tick, &replay);
    }
    fixture.declare(fixture.shot_id(start, 2), vec![fixture.target_record()]);
    fixture.declare(fixture.shot_id(start, 15), vec![fixture.target_record()]);
    fixture.idle(252, 150);
    assert_eq!(fixture.accepted(start), 1);
    assert_eq!(fixture.authorized.len(), 3);
    assert_eq!(fixture.ammo(), MAGAZINE - 3);
    near(fixture.damage(), 90.0);
    assert!(fixture.verdicts.iter().any(|v| v.shot_id
        == netcode::wire_convert::shot_id_to_wire(fixture.shot_id(start, 15))
        && !v.accept
        && !v.hit_accepted));
}

#[test]
fn conditioned_activation_release_packet_outage_expires_without_host_damage_or_spend() {
    let mut fixture = Fixture::new(mandated_link(), descriptor(true, true, 1, "press"));
    let start = token(500, ActivationLane::Secondary);
    fixture.step(500, held(start.lane, true));
    fixture.step(501, held(start.lane, false));
    for _ in 0..64 {
        if fixture.accepted(start) == 1 {
            break;
        }
        fixture.host_tick();
    }
    for offset in 2..24 {
        fixture.step(500 + offset, held(start.lane, false));
    }
    assert_eq!(fixture.accepted(start), 1);
    assert!(fixture.host_charging());
    fixture.cut_up = true;
    for offset in 24..36 {
        fixture.step(500 + offset, held(start.lane, false));
    }
    let mut release = neutral();
    release.activation.release = Some(ActivationRelease {
        token: start,
        release_tick: 536,
    });
    fixture.step(536, release);
    fixture.idle(537, 160);
    assert!(
        fixture.cut_packets > 0,
        "outage drops actual connection packets, including retransmissions"
    );
    fixture.assert_no_host_shot();
    assert!(fixture.cancelled(start));
    assert!(fixture.reconciled.contains(&(start, true, false)));
    fixture.cut_up = false;
    fixture.idle(697, 160);
    fixture.assert_no_host_shot();
    assert!(!fixture.host_charging());
}

#[test]
fn conditioned_activation_release_before_admission_and_same_tick_early_release_are_correlated_at_wrap()
 {
    for elapsed in [0, 8] {
        let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 1, "press"));
        let start = token(u32::MAX - 3, ActivationLane::Secondary);
        // Establish the real movement frontier before submitting an older start
        // behind the first-arriving release. Bootstrap intentionally rejects
        // a command preceding the very first observed command tick.
        fixture.send_input(start.start_tick.wrapping_sub(2), &neutral());
        fixture.send_input(start.start_tick.wrapping_sub(1), &neutral());
        fixture.host_tick();
        fixture.host_tick();
        let release = ActivationRelease {
            token: start,
            release_tick: start.start_tick.wrapping_add(elapsed),
        };
        let mut initiating = held(start.lane, true);
        initiating.activation = ActivationInput {
            initiation: Some(start),
            release: Some(release),
            cancel: None,
        };
        fixture.predict(start.start_tick, &mut initiating);
        // The release arrives first on the reliable stream, while command
        // sorting still resolves the older initiation. The retained edge joins it.
        let mut release_first = neutral();
        release_first.activation.release = Some(release);
        fixture.send_input(start.start_tick.wrapping_add(1), &release_first);
        initiating.activation.release = None;
        fixture.send_input(start.start_tick, &initiating);
        fixture.host_tick();
        fixture.idle(start.start_tick.wrapping_add(2), 80);
        assert_eq!(fixture.accepted(start), 1);
        if elapsed == 0 {
            fixture.assert_no_host_shot();
            assert!(fixture.cancelled(start));
        } else {
            assert_eq!(fixture.authorized.len(), 1);
            assert_eq!(fixture.ammo(), MAGAZINE - 2);
            assert!((fixture.damage() - 16.0).abs() < 0.001);
            assert!(fixture.outcomes.iter().any(|o| matches!(o, wire::ActivationOutcome::ExecutionAccepted { token, charge_millionths: 266_667, .. } if *token == wire_token(start))));
        }
    }
}

#[test]
fn conditioned_activation_trimmed_start_stale_projection_and_named_hold_restart() {
    let mut trimmed = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 1, "hold"));
    let retained = token(1, ActivationLane::Secondary);
    trimmed
        .frame
        .records
        .request(retained, trimmed.local_actors.weapon);
    for tick in 1..=12 {
        let mut command = neutral();
        if tick == 1 {
            command.activation.initiation = Some(retained);
        }
        if tick == 3 {
            command.activation.release = Some(ActivationRelease {
                token: retained,
                release_tick: 3,
            });
        }
        trimmed.send_input(tick, &command);
    }
    trimmed.host_tick();
    trimmed.idle(13, 16);
    trimmed.assert_no_host_shot();
    assert_eq!(
        trimmed.accepted(retained),
        1,
        "the trimmed start survives in its retained lane"
    );
    assert!(
        trimmed.cancelled(retained),
        "its retained early release cancels without debit"
    );
    let mut retry = neutral();
    retry.activation.initiation = Some(retained);
    trimmed.send_input(retained.start_tick, &retry);
    trimmed.idle(30, 32);
    trimmed.assert_no_host_shot();
    assert_eq!(
        trimmed.accepted(retained),
        1,
        "a stale retransmit cannot admit the settled start again"
    );

    let mut fixture = Fixture::new(mandated_link(), descriptor(true, false, 1, "hold"));
    // A stale empty projection suppresses cosmetics, while host ammo permits a
    // real semantic projectile. Initial host recovery refuses the first token.
    fixture.projection.magazine = Some(weapon::SlotSample {
        slot: 0,
        value: Some(0.0),
    });
    fixture.projection.reload_active = Some(weapon::SlotSample {
        slot: 0,
        value: false,
    });
    let mut host_weapon = fixture
        .host
        .borrow()
        .get_component::<WeaponComponent>(fixture.host_actors.weapon)
        .unwrap()
        .clone();
    host_weapon.cooldown_remaining_ms = 500.0;
    fixture
        .host
        .borrow_mut()
        .set_component(fixture.host_actors.weapon, host_weapon)
        .unwrap();
    fixture.step(1000, held(ActivationLane::Primary, true));
    fixture.step(1001, held(ActivationLane::Primary, false));
    for _ in 0..64 {
        if fixture
            .outcomes
            .iter()
            .any(|o| matches!(o, wire::ActivationOutcome::InitiationRejected { .. }))
        {
            break;
        }
        fixture.host_tick();
    }
    for offset in 2..100 {
        fixture.step(1000 + offset, held(ActivationLane::Primary, false));
    }
    fixture.idle(1100, 120);
    let names = fixture
        .sent
        .iter()
        .filter_map(|input| input.activation.initiation)
        .collect::<Vec<_>>();
    assert!(names.len() >= 2);
    assert!(names.windows(2).all(|pair| pair[0] != pair[1]));
    assert!(
        fixture
            .outcomes
            .iter()
            .any(|o| matches!(o, wire::ActivationOutcome::InitiationRejected { .. }))
    );
    assert!(
        !fixture.authorized.is_empty(),
        "a later client-named held restart must recover"
    );
    assert_eq!(fixture.ammo(), MAGAZINE - fixture.authorized.len() as u32);
    near(fixture.damage(), 30.0 * fixture.authorized.len() as f32);
    assert!(
        fixture.hidden_launches > 0,
        "stale empty projection must hide cosmetics while retaining semantic launches"
    );
    assert!(fixture.snapshots.len() >= fixture.authorized.len());
    for shot in &fixture.authorized {
        assert!(
            names.contains(&wire_token(shot.shot_id.activation().token)),
            "host never invents a hold restart"
        );
        near(shot.damage, 30.0);
        near(shot.projectile_radius.unwrap(), 0.125);
    }
}

#[test]
fn conditioned_activation_edge_overflow_and_unknown_declaration_expiry_link_to_terminal_outcomes() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, true, 1, "press"));
    let start = token(100, ActivationLane::Secondary);
    for offset in 0..12 {
        fixture.step(100 + offset, held(start.lane, offset == 0));
    }
    assert!(fixture.host_charging());
    let future = fixture.shot_id(start, 0);
    fixture.declare(future, vec![fixture.target_record()]);
    // Stale movement samples still carry retained release edges. Fill the
    // bounded edge store with unknown tokens, then overflow the live token.
    for offset in 0..64 {
        let unknown = token(1000 + offset, ActivationLane::Primary);
        // The matching future declaration remains first in the pending queue.
        // The final unknown HIT exceeds the same bounded production intake;
        // its refusal cannot alter FIRE or the live activation's resource.
        fixture.declare(fixture.shot_id(unknown, 0), vec![fixture.target_record()]);
        let mut input = neutral();
        input.activation.release = Some(ActivationRelease {
            token: unknown,
            release_tick: 1000 + offset,
        });
        fixture.send_input(111, &input);
    }
    let mut release = neutral();
    release.activation.release = Some(ActivationRelease {
        token: start,
        release_tick: 136,
    });
    fixture.send_input(112, &release);
    fixture.idle(113, 80);
    fixture.assert_no_host_shot();
    assert!(fixture.cancelled(start));
    assert!(fixture.verdicts.iter().any(|v| v.shot_id
        == netcode::wire_convert::shot_id_to_wire(future)
        && !v.accept
        && !v.hit_accepted));
    let overflowed = fixture.shot_id(token(1063, ActivationLane::Primary), 0);
    assert_eq!(
        fixture
            .hit_refusals
            .iter()
            .filter(|id| **id == overflowed)
            .count(),
        1
    );
    assert!(
        !fixture
            .verdicts
            .iter()
            .any(|v| v.shot_id == netcode::wire_convert::shot_id_to_wire(overflowed)),
        "overflow refuses unknown HIT without inventing a FIRE verdict"
    );
    let unknown = fixture.shot_id(token(3000, ActivationLane::Secondary), 0);
    // A local feedback record can exist before this host sees its initiation.
    // Retain it through the actual client ledger so receive must settle it.
    fixture.predicted.predict(
        unknown,
        fixture.local_actors.weapon,
        &weapon::ClientFireResolution {
            client_tick: unknown.start_tick,
            hits: vec![weapon::LocalHitRecord {
                target: fixture.local_actors.target,
                point: Vec3::new(0.0, 0.5, -4.0),
                normal: Vec3::Z,
                zone: None,
            }],
            world_contacts: Vec::new(),
            projectile_launch: None,
        },
        0.0,
        300.0,
        weapon::ClientPullPresentation::Fire,
    );
    fixture.declare(unknown, vec![fixture.target_record()]);
    fixture.idle(193, 100);
    assert!(
        !fixture
            .verdicts
            .iter()
            .any(|v| v.shot_id == netcode::wire_convert::shot_id_to_wire(unknown))
    );
    assert!(
        !fixture.hit_refusals.contains(&unknown),
        "HIT waits until its deadline"
    );
    assert!(fixture.predicted.clone().refuse_hit(unknown).is_some());
    fixture.assert_no_host_shot();
    // A duplicate before expiry must not restart the pending declaration's age.
    fixture.declare(unknown, vec![fixture.target_record()]);
    fixture.idle(293, 80);
    assert_eq!(
        fixture
            .hit_refusals
            .iter()
            .filter(|id| **id == unknown)
            .count(),
        1
    );
    assert_eq!(
        fixture
            .retired_hit_feedback
            .iter()
            .filter(|id| **id == unknown)
            .count(),
        1
    );
    assert!(fixture.predicted.clone().refuse_hit(unknown).is_none());
    assert!(
        !fixture
            .verdicts
            .iter()
            .any(|v| v.shot_id == netcode::wire_convert::shot_id_to_wire(unknown)),
        "expiry settles exact unknown HIT without fabricating FIRE denial"
    );
    fixture.assert_no_host_shot();
}

#[test]
fn conditioned_activation_stale_full_projection_refuses_damage_and_old_verdict_preserves_new_recovery()
 {
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(false, false, 1, "press"));
    fixture.projection.magazine = Some(weapon::SlotSample {
        slot: 0,
        value: Some(MAGAZINE as f32),
    });
    let mut host_weapon = fixture
        .host
        .borrow()
        .get_component::<WeaponComponent>(fixture.host_actors.weapon)
        .unwrap()
        .clone();
    host_weapon.magazine = 0;
    fixture
        .host
        .borrow_mut()
        .set_component(fixture.host_actors.weapon, host_weapon)
        .unwrap();
    let old = token(2000, ActivationLane::Primary);
    fixture.step(2000, held(old.lane, true));
    fixture.idle(2001, 32);
    assert_eq!(
        fixture.snapshots.len(),
        1,
        "stale full projection still permits a semantic attempt"
    );
    assert!(fixture.authorized.is_empty());
    assert_eq!(fixture.ammo(), 0);
    near(fixture.damage(), 0.0);
    let denied = fixture
        .verdicts
        .iter()
        .copied()
        .find(|v| {
            v.shot_id == netcode::wire_convert::shot_id_to_wire(fixture.shot_id(old, 0))
                && !v.accept
        })
        .expect("correlated resource refusal");
    let terminal = fixture.outcomes.iter().copied().find(|o| matches!(o, wire::ActivationOutcome::Cancelled { token, .. } if *token == wire_token(old))).expect("resource refusal terminates its activation");
    // Retain a pending old FIRE record to model its later flight declaration.
    // Resolution uses the original frozen shot; it mutates no weapon/resources.
    let old_shot = fixture.snapshots[0].clone();
    let old_resolution = weapon::resolve_client_shot(
        Some(fixture.local_actors.pawn),
        fixture
            .local
            .borrow()
            .get_component::<WeaponComponent>(fixture.local_actors.weapon)
            .unwrap()
            .clone(),
        "conditioned-weapon",
        0,
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::NEG_Z,
        &WeaponPlacementDescriptor::default(),
        &fixture.world,
        &fixture.local.borrow(),
        &fixture.zones,
        0.0,
        old_shot,
    );
    fixture.predicted.predict(
        fixture.shot_id(old, 0),
        fixture.local_actors.weapon,
        &old_resolution,
        0.0,
        300.0,
        weapon::ClientPullPresentation::Fire,
    );
    let mut host_weapon = fixture
        .host
        .borrow()
        .get_component::<WeaponComponent>(fixture.host_actors.weapon)
        .unwrap()
        .clone();
    host_weapon.magazine = MAGAZINE;
    fixture
        .host
        .borrow_mut()
        .set_component(fixture.host_actors.weapon, host_weapon)
        .unwrap();
    let newer = token(2033, ActivationLane::Primary);
    fixture.step(newer.start_tick, held(newer.lane, true));
    fixture.step(2034, neutral());
    for _ in 0..8 {
        fixture.transport();
    }
    assert_eq!(fixture.accepted(newer), 1);
    assert_eq!(fixture.authorized.len(), 1);
    assert_eq!(fixture.ammo(), MAGAZINE - 1);
    near(fixture.damage(), 30.0);
    let recovery = fixture
        .local
        .borrow()
        .get_component::<WeaponComponent>(fixture.local_actors.weapon)
        .unwrap()
        .cooldown_remaining_ms;
    assert!(recovery > 0.0);
    // Replay already-produced old facts through the real ordered server channel.
    // Neither an old terminal fact nor its pending per-shot refusal owns B's clock.
    netcode::send_activation_outcome(&mut fixture.server, CLIENT, terminal);
    netcode::send_shot_verdict(
        &mut fixture.server,
        CLIENT,
        netcode::wire_convert::shot_id_from_wire(denied.shot_id),
        denied.accept,
        denied.hit_accepted,
    );
    for _ in 0..8 {
        fixture.transport();
    }
    near(
        fixture
            .local
            .borrow()
            .get_component::<WeaponComponent>(fixture.local_actors.weapon)
            .unwrap()
            .cooldown_remaining_ms,
        recovery,
    );
    near(fixture.damage(), 30.0);
    assert_eq!(fixture.ammo(), MAGAZINE - 1);
}

/// Reference plasma rifle shape (`content/dev/scripts/reference-projectiles.ts`,
/// `reference_plasma_bolt` primary): hold, 130 ms recovery, one shot per step,
/// cell resource, 40 m/s bolt, 96 m range, 5 s lifetime.
fn plasma_rifle(cell_capacity: f32) -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(json!({
        "damage": 10, "knockback": { "speed": 3.0, "upwardBias": 0.1 }, "range": 96,
        "resolution": "projectile",
        "primary": { "trigger": "hold", "recoveryMs": 130, "steps": [{ "kind": "shot" }] },
        "resource": { "kind": "cell", "capacity": cell_capacity, "costPerShot": 5,
            "regenPerSecond": 25, "regenDelayMs": 600 },
        "projectile": { "speed": 40, "radius": 0.5, "lifetimeMs": 5000,
            "visual": { "body": { "kind": "sprite", "sprite": "sprites/test.png", "size": 1.5 } } },
    }))
    .expect("plasma rifle fixture parses")
    .validate()
    .expect("plasma rifle fixture validates")
}

/// A one-shot hitscan rifle with an ample magazine.
fn hitscan_rifle(trigger: &str, recovery_ms: f32) -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(json!({
        "damage": 10, "range": 96, "resolution": "hitscan",
        "primary": { "trigger": trigger, "recoveryMs": recovery_ms, "steps": [{ "kind": "shot" }] },
        "resource": { "kind": "ammo", "type": "rounds", "magazine": 100_000, "reserve": 0 },
    }))
    .expect("hitscan rifle fixture parses")
    .validate()
    .expect("hitscan rifle fixture validates")
}

/// Client ticks one stream cell runs: 6 s at 60 Hz.
const STREAM_TICKS: u32 = 360;

/// 130 ms of recovery is 8 ticks at 60 Hz: the plasma and hitscan rifles' cadence.
const RIFLE_RECOVERY_TICKS: u32 = 8;

/// A floor proving a stream cell fired: half the shots its authored period
/// allows over the stream. It asserts that the weapon was exercised, not its cadence.
fn fired_floor(period_ticks: u32, shots_per_execution: usize) -> usize {
    STREAM_TICKS.div_ceil(period_ticks) as usize * shots_per_execution / 2
}

/// The cell fired at least `floor` shots and the host authorized each one, in order.
fn fired_in_order(report: &StreamReport, floor: usize) -> bool {
    report.predicted.len() >= floor && report.authorized == report.predicted
}

/// Move both peers' targets far off the line of fire so every bolt can reach
/// its full range.
fn clear_line_of_fire(fixture: &Fixture) {
    for (registry, target) in [
        (&fixture.host, fixture.host_actors.target),
        (&fixture.local, fixture.local_actors.target),
    ] {
        registry
            .borrow_mut()
            .set_component(
                target,
                Transform {
                    position: Vec3::new(500.0, 0.5, 500.0),
                    ..Default::default()
                },
            )
            .expect("target transform updates");
    }
}

/// A primary press naming its own start at `tick`, as a client stamps it.
fn stamped_start(tick: u32) -> sim::SimCommand {
    let mut command = held(ActivationLane::Primary, true);
    command.activation.initiation = Some(token(tick, ActivationLane::Primary));
    command
}

/// Drives a stream's client ticks against a host that may hitch.
/// `hitch = Some((period, length))`: every `period` client ticks the host
/// simulation stalls for `length` ticks (a long host frame) while the client
/// keeps predicting and sending, then runs the missed fixed ticks back to back,
/// as the frame loop's accumulator does.
struct HitchDriver {
    hitch: Option<(u32, u32)>,
    /// Client tick of the stream's first command.
    start: u32,
    stalled: u32,
}

impl HitchDriver {
    fn new(hitch: Option<(u32, u32)>, start: u32) -> Self {
        Self {
            hitch,
            start,
            stalled: 0,
        }
    }

    /// The stream's client tick `offset` ticks in.
    fn step(&mut self, fixture: &mut Fixture, offset: u32, mut command: sim::SimCommand) {
        let tick = self.start + offset;
        let in_hitch = self
            .hitch
            .is_some_and(|(period, length)| offset >= period && offset % period < length);
        if in_hitch {
            fixture.predict(tick, &mut command);
            fixture.send_input(tick, &command);
            fixture.advance_projectiles();
            self.stalled += 1;
            return;
        }
        for _ in 0..std::mem::take(&mut self.stalled) {
            fixture.host_tick();
        }
        fixture.step(tick, command);
    }
}

/// Authorized shots over every window between two host fires stay within the
/// cadence window bound for `recovery_ticks`.
fn assert_within_cadence_window_bound(fixture: &Fixture, recovery_ticks: u32, label: &str) {
    let fire_ticks: Vec<u32> = fixture
        .authorized
        .iter()
        .map(|shot| shot.fire_tick)
        .collect();
    for (first, &opened) in fire_ticks.iter().enumerate() {
        for (last, &closed) in fire_ticks.iter().enumerate().skip(first) {
            let shots = (last - first + 1) as u32;
            let window = closed - opened;
            assert!(
                shots <= netcode::HostCommandQueues::cadence_window_bound(window, recovery_ticks),
                "{label}: {shots} shots in {window} host ticks"
            );
        }
    }
}

#[derive(Debug, Default)]
struct StreamReport {
    predicted: Vec<ShotId>,
    authorized: Vec<ShotId>,
    /// Activation starts the host refused (never a resource decision).
    initiation_rejected: Vec<ShotId>,
    /// Per-shot FIRE denials (resource or other).
    fire_denied: Vec<ShotId>,
    /// Predicted bolts removed before natural expiry.
    bolts_removed_early: Vec<ShotId>,
    /// Predicted shots the host neither authorized nor refused.
    ghosts: Vec<ShotId>,
    /// For each refused start: its start tick, the client gap since the previous
    /// authorized start, and the host ticks between their resolutions.
    refusal_spacing: Vec<(u32, u32, u32)>,
    /// Playout steps whose resolved client tick skipped ahead: the catch-up trim.
    catch_up_jumps: usize,
}

fn run_steady_hold(
    link: LinkConfig,
    descriptor: WeaponDescriptor,
    projectile: bool,
) -> (Fixture, StreamReport) {
    run_stream(link, descriptor, projectile, None, None)
}

/// A `STREAM_TICKS` primary stream: a steady hold, or with `tap_period` a tap
/// every that many ticks (pressed one tick, held two). `hitch` is a
/// [`HitchDriver`] schedule.
fn run_stream(
    link: LinkConfig,
    descriptor: WeaponDescriptor,
    projectile: bool,
    hitch: Option<(u32, u32)>,
    tap_period: Option<u32>,
) -> (Fixture, StreamReport) {
    let mut fixture = Fixture::new(link, descriptor);
    fixture.mirror_rejection_despawn = true;
    if projectile {
        clear_line_of_fire(&fixture);
    }
    let start = 1000u32;
    let mut driver = HitchDriver::new(hitch, start);
    for offset in 0..STREAM_TICKS {
        let command = match tap_period {
            None => held(ActivationLane::Primary, offset == 0),
            Some(period) if offset % period < 2 => {
                held(ActivationLane::Primary, offset % period == 0)
            }
            Some(_) => neutral(),
        };
        driver.step(&mut fixture, offset, command);
    }
    // Long enough for 96 m at 40 m/s plus the declaration round trip.
    fixture.idle(start + STREAM_TICKS, 360);
    let report = stream_report(&fixture, projectile);
    (fixture, report)
}

/// Classify every predicted shot of a finished stream by its host outcome.
fn stream_report(fixture: &Fixture, projectile: bool) -> StreamReport {
    let mut report = StreamReport {
        predicted: fixture
            .snapshots
            .iter()
            .map(|shot| shot.activation.shot_id)
            .collect(),
        authorized: fixture.authorized.iter().map(|shot| shot.shot_id).collect(),
        ..Default::default()
    };
    let wire_of = |id: ShotId| netcode::wire_convert::shot_id_to_wire(id);
    for &id in &report.predicted {
        let token = wire_token(id.activation().token);
        if fixture.outcomes.iter().any(|outcome| {
            matches!(outcome, wire::ActivationOutcome::InitiationRejected { token: t, .. } if *t == token)
        }) {
            report.initiation_rejected.push(id);
        }
        if fixture
            .verdicts
            .iter()
            .any(|verdict| verdict.shot_id == wire_of(id) && !verdict.accept)
        {
            report.fire_denied.push(id);
        }
        let authorized = report.authorized.contains(&id);
        if !authorized
            && !report.initiation_rejected.contains(&id)
            && !report.fire_denied.contains(&id)
        {
            report.ghosts.push(id);
        }
    }
    if projectile {
        let live: HashSet<ShotId> = fixture
            .local
            .borrow()
            .iter_with_kind(ComponentKind::Projectile)
            .filter_map(|(_, value)| match value {
                ComponentValue::Projectile(component) => component.predicted_shot_id,
                _ => None,
            })
            .collect();
        assert!(live.is_empty(), "every flight must have ended: {live:?}");
        report.bolts_removed_early = report
            .predicted
            .iter()
            .copied()
            .filter(|id| !fixture.expired.contains(id))
            .collect();
    }
    let host_tick_of = |client_tick: u32| {
        fixture
            .playout
            .iter()
            .find(|(_, tick, source)| {
                *tick == client_tick && *source == netcode::ResolutionSource::Real
            })
            .map(|(host, _, _)| *host)
    };
    for &id in &report.initiation_rejected {
        let previous = report
            .authorized
            .iter()
            .filter(|shot| shot.start_tick < id.start_tick)
            .map(|shot| shot.start_tick)
            .max();
        if let (Some(previous), Some(now), Some(then)) = (
            previous,
            host_tick_of(id.start_tick),
            previous.and_then(host_tick_of),
        ) {
            report
                .refusal_spacing
                .push((id.start_tick, id.start_tick - previous, now - then));
        }
    }
    report.catch_up_jumps = fixture
        .playout
        .windows(2)
        .filter(|pair| pair[1].1.wrapping_sub(pair[0].1) > 1)
        .count();
    report
}

fn summarize(label: &str, report: &StreamReport) -> String {
    format!(
        "{label}: predicted {}, host authorized {}, initiation-rejected {} {:?}, \
         fire-denied {}, bolts removed early {}, ghosts {}, playout jumps {}, \
         refused (start, client gap, host gap) {:?}",
        report.predicted.len(),
        report.authorized.len(),
        report.initiation_rejected.len(),
        report
            .initiation_rejected
            .iter()
            .map(|id| id.start_tick)
            .collect::<Vec<_>>(),
        report.fire_denied.len(),
        report.bolts_removed_early.len(),
        report.ghosts.len(),
        report.catch_up_jumps,
        report.refusal_spacing,
    )
}

/// A cell that hitches the host or runs a trim seed claims the catch-up trim,
/// so it must show one.
fn missed_its_trim(link: LinkConfig, hitch: Option<(u32, u32)>, report: &StreamReport) -> bool {
    let claims_trim = hitch.is_some() || [TRIM_SEED_A, TRIM_SEED_B].contains(&link.seed);
    claims_trim && report.catch_up_jumps == 0
}

/// Decision 14: a start whose input stalled past the catch-up allowance is
/// refused. Checks that every refused start of `report` is such a refusal, and
/// measures its stall: the host ticks from the earliest it could have played
/// (its send plus the cell's fastest transit) to its refusal on which the host
/// could not deliver it, because no real command resolved, the real command
/// resolved was stamped before it, or an older start of the backlog was
/// executing. The stall must exceed the allowance; the lane, not the client
/// half or the machine, refused it; a newer start the client had already sent
/// still fired, so only the backlog's oldest starts were refused; and the
/// refusal came before the lane's retention could have expired it. Returns
/// `(start tick, stall in host ticks)` per refused start.
fn stall_refusals(fixture: &Fixture, report: &StreamReport) -> Result<Vec<(u32, u32)>, String> {
    let allowance = netcode::HostCommandQueues::CATCH_UP_ALLOWANCE_TICKS;
    let mut sent_at = std::collections::HashMap::new();
    for (command, &tick) in fixture.sent.iter().zip(&fixture.sent_at) {
        sent_at.entry(command.client_tick).or_insert(tick);
    }
    let transit = fixture
        .playout
        .iter()
        .filter(|(_, _, source)| *source == netcode::ResolutionSource::Real)
        .filter_map(|(tick, client_tick, _)| Some(tick - sent_at.get(client_tick)?))
        .min()
        .unwrap_or(0);
    // Host ticks from each authorized start's first shot to its last.
    let mut executions = std::collections::HashMap::new();
    for shot in &fixture.authorized {
        let span = executions
            .entry(shot.shot_id.start_tick)
            .or_insert((shot.fire_tick, shot.fire_tick));
        span.0 = span.0.min(shot.fire_tick);
        span.1 = span.1.max(shot.fire_tick);
    }
    let mut starts: Vec<u32> = report
        .initiation_rejected
        .iter()
        .map(|id| id.start_tick)
        .collect();
    starts.dedup();
    starts
        .into_iter()
        .map(|start| {
            let &(refused_at, _) = fixture
                .lane_refusals
                .iter()
                .find(|(_, token)| token.start_tick == start)
                .ok_or_else(|| format!("{start}: refused outside the lane"))?;
            let sent = *sent_at
                .get(&start)
                .ok_or_else(|| format!("{start}: never sent"))?;
            let older_executing = |tick: u32| {
                executions
                    .iter()
                    .any(|(&older, &(first, last))| older < start && (first..=last).contains(&tick))
                    || fixture
                        .lane_refusals
                        .iter()
                        .any(|&(at, token)| at == tick && token.start_tick < start)
            };
            let stall = fixture
                .playout
                .iter()
                .filter(|&&(tick, client_tick, source)| {
                    (sent + transit..refused_at).contains(&tick)
                        && (source != netcode::ResolutionSource::Real
                            || client_tick < start
                            || older_executing(tick))
                })
                .count() as u32;
            let newer_fired = report.authorized.iter().any(|shot| {
                shot.start_tick > start
                    && sent_at
                        .get(&shot.start_tick)
                        .is_some_and(|&at| at < refused_at)
            });
            if stall <= allowance {
                Err(format!("{start}: input stalled only {stall} host ticks"))
            } else if !newer_fired {
                Err(format!("{start}: no newer start of its backlog fired"))
            } else if refused_at - sent >= netcode::HostCommandQueues::START_RETENTION_TICKS {
                Err(format!(
                    "{start}: refused {} host ticks after its send, not promptly",
                    refused_at - sent
                ))
            } else {
                Ok((start, stall))
            }
        })
        .collect()
}

/// A stream cell's parity: at least `floor` predicted shots, each authorized
/// in order, none denied, none cut short. On a lossy link the shots of starts
/// `stall_refusals` accepts are exempt: their input stalled past the catch-up
/// allowance, as when a lost packet's resend is lost too, and decision 14
/// refuses them. The mandated seed does that once near every stream's start.
fn stream_parity(
    link: LinkConfig,
    fixture: &Fixture,
    report: &StreamReport,
    floor: usize,
) -> Result<(), String> {
    let refusals = if link.loss_probability > 0.0 {
        stall_refusals(fixture, report)?
    } else if report.initiation_rejected.is_empty() {
        Vec::new()
    } else {
        return Err("refused on a loss-free link".into());
    };
    let refused = |id: &ShotId| refusals.iter().any(|&(start, _)| start == id.start_tick);
    let kept: Vec<ShotId> = report
        .predicted
        .iter()
        .copied()
        .filter(|id| !refused(id))
        .collect();
    if report.predicted.len() < floor {
        Err(format!("predicted only {}", report.predicted.len()))
    } else if report.authorized != kept {
        Err("authorized differs from the predicted shots of unrefused starts".into())
    } else if !report.fire_denied.iter().all(refused) {
        Err("a FIRE denial outside the stall refusals".into())
    } else if !report.bolts_removed_early.iter().all(refused) {
        Err("a bolt cut short outside the stall refusals".into())
    } else {
        Ok(())
    }
}

/// Why a stream cell fails `stream_parity` or its trim, if it does.
fn stream_cell_failure(
    label: &str,
    link: LinkConfig,
    hitch: Option<(u32, u32)>,
    fixture: &Fixture,
    report: &StreamReport,
    floor: usize,
) -> Option<String> {
    let verdict = stream_parity(link, fixture, report, floor).and_then(|()| {
        if missed_its_trim(link, hitch, report) {
            Err("no catch-up trim".into())
        } else {
            Ok(())
        }
    });
    verdict
        .err()
        .map(|why| format!("{}: {why}", summarize(label, report)))
}

// Regression: during a steady hold, a connected client's predicted plasma bolts
// vanished mid-flight although the host had resource to fire every one of them.
// In this and the stream cells below, "never refused" means short of a stall
// past the catch-up allowance, which decision 14 refuses; `stream_parity` holds
// those refusals to its rules.
#[test]
fn conditioned_steady_hold_keeps_every_resourced_predicted_bolt_to_its_natural_end() {
    let mut failures = Vec::new();
    for (label, link) in [
        ("clean", LinkConfig::perfect()),
        ("mandated", mandated_link()),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A)),
        ("mandated-seed-b", mandated_link_seeded(TRIM_SEED_B)),
    ] {
        // Cell large enough that drain cannot explain any refusal.
        let (fixture, report) = run_steady_hold(link, plasma_rifle(1.0e6), true);
        failures.extend(stream_cell_failure(
            label,
            link,
            None,
            &fixture,
            &report,
            fired_floor(RIFLE_RECOVERY_TICKS, 1),
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Regression: on a loss-free link, a host frame hitch longer than the playout
// backlog bound deletes the client's in-flight predicted bolt the same way.
#[test]
fn conditioned_steady_hold_host_hitch_on_clean_link_keeps_resourced_bolts() {
    let (_, report) = run_stream(
        LinkConfig::perfect(),
        plasma_rifle(1.0e6),
        true,
        Some((60, 12)),
        None,
    );
    let summary = summarize("clean+host-hitch-200ms-every-1s", &report);
    assert!(
        report.catch_up_jumps > 0,
        "the hitch must take the trim: {summary}"
    );
    assert!(
        report.bolts_removed_early.is_empty() && report.initiation_rejected.is_empty(),
        "{summary}"
    );
    assert!(
        fired_in_order(&report, fired_floor(RIFLE_RECOVERY_TICKS, 1)),
        "{summary}"
    );
}

// A refused hold restart is not projectile-specific: it retracts a hitscan
// shot's muzzle flash and hitmarker the same way.
#[test]
fn conditioned_steady_hitscan_hold_is_never_refused_with_ample_ammo() {
    let mut failures = Vec::new();
    for (label, link) in [
        ("clean", LinkConfig::perfect()),
        ("mandated", mandated_link()),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A)),
    ] {
        let (fixture, report) = run_steady_hold(link, hitscan_rifle("hold", 130.0), false);
        failures.extend(stream_cell_failure(
            label,
            link,
            None,
            &fixture,
            &report,
            fired_floor(RIFLE_RECOVERY_TICKS, 1),
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Same mechanism for a press trigger tapped at its recovery rate.
#[test]
fn conditioned_rapid_press_taps_are_never_refused_with_ample_ammo() {
    // 9 ticks covers the 8-tick (130 ms) recovery.
    const TAP_PERIOD: u32 = 9;
    let mut failures = Vec::new();
    let tap = hitscan_rifle("press", 130.0);
    for (label, link) in [
        ("clean", LinkConfig::perfect()),
        ("mandated", mandated_link()),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A)),
    ] {
        let (fixture, report) = run_stream(link, tap.clone(), false, None, Some(TAP_PERIOD));
        failures.extend(stream_cell_failure(
            &format!("press {label}"),
            link,
            None,
            &fixture,
            &report,
            fired_floor(TAP_PERIOD, 1),
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The reference cell (100 / 5 / 25 s⁻¹ after 600 ms) drains after twenty bolts.
/// Those later refusals are resource decisions: per-shot FIRE denials that remove
/// the stale-projection bolt, never a refused activation start.
#[test]
fn conditioned_steady_hold_reference_cell_drain_refuses_only_for_resource() {
    let (_, report) = run_steady_hold(LinkConfig::perfect(), plasma_rifle(100.0), true);
    let summary = summarize("cell-100 clean", &report);
    assert!(
        report.predicted.len() >= fired_floor(RIFLE_RECOVERY_TICKS, 1),
        "{summary}"
    );
    assert!(report.initiation_rejected.is_empty(), "{summary}");
    assert!(report.authorized.len() >= 20, "{summary}");
    assert!(!report.fire_denied.is_empty(), "{summary}");
    let first_twenty: Vec<ShotId> = report.predicted.iter().take(20).copied().collect();
    assert!(
        report
            .fire_denied
            .iter()
            .all(|id| !first_twenty.contains(id)),
        "a full cell pays for the first twenty bolts"
    );
    assert_eq!(report.bolts_removed_early, report.fire_denied);
}

/// Shots in one burst execution.
const BURST_SHOTS: usize = 3;

/// A burst's two 50 ms (3-tick) waits, then the 8-tick (130 ms) recovery from
/// its last shot.
const BURST_PERIOD_TICKS: u32 = 14;

/// Three-round hitscan burst: shots 50 ms apart, then the authored recovery.
fn burst_rifle(trigger: &str, recovery_ms: f32) -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(json!({
        "damage": 10, "range": 96, "resolution": "hitscan",
        "primary": { "trigger": trigger, "recoveryMs": recovery_ms, "steps": [
            { "kind": "shot" }, { "kind": "wait", "durationMs": 50 },
            { "kind": "shot" }, { "kind": "wait", "durationMs": 50 },
            { "kind": "shot" },
        ] },
        "resource": { "kind": "ammo", "type": "rounds", "magazine": 100_000, "reserve": 0 },
    }))
    .expect("burst rifle fixture parses")
    .validate()
    .expect("burst rifle fixture validates")
}

/// Reference rocket cadence (`reference_rocket` primary: press, 750 ms recovery)
/// without splash, with an ample magazine so no reload interrupts the stream.
fn rocket_launcher() -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(json!({
        "damage": 36, "range": 128, "resolution": "projectile",
        "primary": { "trigger": "press", "recoveryMs": 750, "steps": [{ "kind": "shot" }] },
        "resource": { "kind": "ammo", "type": "rockets", "magazine": 100_000, "reserve": 0 },
        "projectile": { "speed": 30, "radius": 0.25, "lifetimeMs": 4000,
            "visual": { "body": { "kind": "sprite", "sprite": "sprites/test.png", "size": 1.0 } } },
    }))
    .expect("rocket launcher fixture parses")
    .validate()
    .expect("rocket launcher fixture validates")
}

// Regression: a burst restart reaching the host while its previous burst still
// executed there was refused as a concurrent activation. Also: a host hitch ran
// a whole burst before the client's last shot, and the client, cancelling its
// burst on that completion, restarted inside the host's recovery.
#[test]
fn conditioned_burst_hold_restarts_are_never_refused_under_loss_and_hitch() {
    let mut failures = Vec::new();
    for (label, link, hitch) in [
        ("clean", LinkConfig::perfect(), None),
        ("mandated", mandated_link(), None),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A), None),
        ("clean+host-hitch", LinkConfig::perfect(), Some((60, 12))),
    ] {
        let (fixture, report) = run_stream(link, burst_rifle("hold", 130.0), false, hitch, None);
        failures.extend(stream_cell_failure(
            &format!("burst {label}"),
            link,
            hitch,
            &fixture,
            &report,
            fired_floor(BURST_PERIOD_TICKS, BURST_SHOTS),
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Regression: a release delivered mid-burst moved the burst's cadence clock to
// the release, so the next tap at the authored rate was refused as cooling.
#[test]
fn conditioned_burst_taps_released_mid_burst_are_never_refused() {
    let mut failures = Vec::new();
    for (label, link, hitch) in [
        ("clean", LinkConfig::perfect(), None),
        ("mandated", mandated_link(), None),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A), None),
        (
            "clean+host-hitch",
            LinkConfig::perfect(),
            Some((56u32, 12u32)),
        ),
    ] {
        let mut fixture = Fixture::new(link, burst_rifle("press", 130.0));
        let start = 1000u32;
        let mut driver = HitchDriver::new(hitch, start);
        for offset in 0..STREAM_TICKS {
            let phase = offset % BURST_PERIOD_TICKS;
            let mut command = if phase < 2 {
                held(ActivationLane::Primary, phase == 0)
            } else {
                neutral()
            };
            if phase == 2 {
                command.activation.release = Some(ActivationRelease {
                    token: token(start + offset - 2, ActivationLane::Primary),
                    release_tick: start + offset,
                });
            }
            driver.step(&mut fixture, offset, command);
        }
        fixture.idle(start + STREAM_TICKS, 120);
        let taps = STREAM_TICKS.div_ceil(BURST_PERIOD_TICKS) as usize;
        let report = stream_report(&fixture, false);
        if report.predicted.len() != BURST_SHOTS * taps {
            failures.push(summarize(label, &report));
        }
        failures.extend(stream_cell_failure(
            label,
            link,
            hitch,
            &fixture,
            &report,
            BURST_SHOTS * taps,
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Regression: rocket presses at the authored rate lost their in-flight rocket
// to a refused start after a stalled input stream.
#[test]
fn conditioned_rocket_presses_at_recovery_rate_keep_every_rocket_under_loss_and_hitch() {
    // 45 ticks covers the 750 ms recovery.
    const PRESS_PERIOD: u32 = 45;
    let mut failures = Vec::new();
    for (label, link, hitch) in [
        ("clean", LinkConfig::perfect(), None),
        ("mandated", mandated_link(), None),
        ("mandated-seed-a", mandated_link_seeded(TRIM_SEED_A), None),
        ("clean+host-hitch", LinkConfig::perfect(), Some((45, 12))),
    ] {
        let (_, report) = run_stream(link, rocket_launcher(), true, hitch, Some(PRESS_PERIOD));
        // Every press lands at the authored rate, so every press fires.
        if report.predicted.len() != STREAM_TICKS.div_ceil(PRESS_PERIOD) as usize
            || report.authorized != report.predicted
            || !report.initiation_rejected.is_empty()
            || !report.bolts_removed_early.is_empty()
            || missed_its_trim(link, hitch, &report)
        {
            failures.push(format!("rocket {}", summarize(label, &report)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A plasma hold on a clean link whose input stalls: every command from
/// `STALL_AT` for `stall` ticks is withheld, then the backlog arrives at once,
/// while the host keeps ticking, as after a lost packet's resend.
struct StalledHold {
    fixture: Fixture,
    /// Host tick on which the client's offset-0 command was sent.
    base: u32,
    /// Host tick on which the backlog arrived.
    stall_end: u32,
    /// `(start tick, host tick the client saw its refusal)` per refused start.
    refusals: Vec<(u32, u32)>,
}

const STALL_START: u32 = 1000;
const STALL_AT: u32 = 120;

impl StalledHold {
    fn run(stall: u32) -> Self {
        let mut fixture = Fixture::new(LinkConfig::perfect(), plasma_rifle(1.0e6));
        fixture.mirror_rejection_despawn = true;
        clear_line_of_fire(&fixture);
        let base = fixture.tick;
        let mut withheld = Vec::new();
        let mut refusals = Vec::new();
        for offset in 0..STREAM_TICKS {
            let tick = STALL_START + offset;
            let mut command = held(ActivationLane::Primary, offset == 0);
            fixture.predict(tick, &mut command);
            if (STALL_AT..STALL_AT + stall).contains(&offset) {
                withheld.push((tick, command));
            } else {
                for (tick, command) in withheld.drain(..) {
                    fixture.send_input(tick, &command);
                }
                fixture.send_input(tick, &command);
            }
            fixture.advance_projectiles();
            let seen = fixture.outcomes.len();
            fixture.host_tick();
            for outcome in &fixture.outcomes[seen..] {
                if let wire::ActivationOutcome::InitiationRejected { token, .. } = outcome {
                    refusals.push((token.start_tick, fixture.tick));
                }
            }
        }
        fixture.idle(STALL_START + STREAM_TICKS, 360);
        Self {
            fixture,
            base,
            stall_end: base + STALL_AT + stall,
            refusals,
        }
    }

    /// Host ticks between a shot's client prediction and its host fire.
    fn fire_lag(&self, shot: &AuthorizedShot) -> u32 {
        shot.fire_tick - (self.base + shot.shot_id.start_tick - STALL_START)
    }

    /// After two recoveries past the stall, every shot fires within the
    /// tolerance of its prediction: the backlog drained instead of delaying
    /// the rest of the hold by the stall.
    fn assert_rest_of_hold_on_time(&self, label: &str) {
        let late: Vec<(u32, u32)> = self
            .fixture
            .authorized
            .iter()
            .filter(|shot| shot.fire_tick >= self.stall_end + 2 * RIFLE_RECOVERY_TICKS)
            .map(|shot| (shot.shot_id.start_tick, self.fire_lag(shot)))
            .filter(|&(_, lag)| lag > postretro_combat_model::activation::CHARGE_TOLERANCE_TICKS)
            .collect();
        assert!(late.is_empty(), "{label}: (start, fire lag) {late:?}");
    }
}

// Decision 14: a stall up to the 500 ms catch-up allowance drains at once and
// refuses nothing. Credited no earlier than host now, the backlog drained one
// shot per recovery and the rest of the hold lagged by the stall.
#[test]
fn conditioned_input_stall_within_the_allowance_drains_at_once_and_refuses_nothing() {
    let run = StalledHold::run(24);
    let report = stream_report(&run.fixture, true);
    let summary = summarize("400 ms stall", &report);
    assert!(
        report.catch_up_jumps > 0,
        "the backlog must take the trim: {summary}"
    );
    assert!(run.refusals.is_empty(), "{summary}");
    assert!(
        report.initiation_rejected.is_empty()
            && report.fire_denied.is_empty()
            && report.ghosts.is_empty()
            && report.bolts_removed_early.is_empty(),
        "{summary}"
    );
    assert!(
        report.predicted.len() >= 40 && report.authorized == report.predicted,
        "{summary}"
    );
    run.assert_rest_of_hold_on_time("400 ms stall");
}

// Decision 14: past the allowance, only the oldest stalled starts are refused,
// within a few ticks of the backlog arriving, and the rest of the hold stays on
// time.
#[test]
fn conditioned_input_stall_past_the_allowance_refuses_only_its_oldest_starts_promptly() {
    const STALL: u32 = 60;
    let run = StalledHold::run(STALL);
    let report = stream_report(&run.fixture, true);
    let summary = summarize("1 s stall", &report);
    assert!(
        report.catch_up_jumps > 0,
        "the backlog must take the trim: {summary}"
    );
    let stalled: Vec<u32> = report
        .predicted
        .iter()
        .map(|shot| shot.start_tick)
        .filter(|tick| (STALL_START + STALL_AT..STALL_START + STALL_AT + STALL).contains(tick))
        .collect();
    let refused: Vec<u32> = run.refusals.iter().map(|&(start, _)| start).collect();
    assert!(!refused.is_empty(), "{summary}");
    assert_eq!(
        refused,
        stalled[..refused.len()],
        "only the oldest stalled starts, once each: {summary}"
    );
    let stall_end_tick = STALL_START + STALL_AT + STALL;
    for &start in &stalled {
        let lag = stall_end_tick - start;
        if lag > 40 {
            assert!(refused.contains(&start), "{start} lagged {lag}: {summary}");
        } else if lag < 15 {
            assert!(!refused.contains(&start), "{start} lagged {lag}: {summary}");
        }
    }
    for &(start, seen) in &run.refusals {
        assert!(
            seen <= run.stall_end + 20,
            "{start} refused on host tick {seen}, stall ended {}",
            run.stall_end
        );
    }
    assert!(
        report.fire_denied.is_empty() && report.ghosts.is_empty(),
        "refused starts mint nothing; every other start fires: {summary}"
    );
    assert_eq!(
        report.authorized.len() + refused.len(),
        report.predicted.len(),
        "{summary}"
    );
    run.assert_rest_of_hold_on_time("1 s stall");
}

/// Shots the listen host's own player gets from the same weapon over `ticks`
/// fixed ticks of held primary: the shared machine with controller starts.
fn host_player_shots(descriptor: &WeaponDescriptor, ticks: u32) -> usize {
    let mut component =
        WeaponComponent::from_descriptor_with_canonical(descriptor, Some("host-player"));
    (0..ticks)
        .filter(|&tick| {
            weapon::execution::advance_weapon_activation(
                &mut component,
                weapon::execution::ActivationCommand {
                    tick,
                    pawn: 1,
                    real_command: true,
                    input: ActivationInput::default(),
                    controller_starts: true,
                    primary: weapon::FireButtonState {
                        pressed: tick == 0,
                        active: true,
                    },
                    secondary: weapon::FireButtonState {
                        pressed: false,
                        active: false,
                    },
                },
                DT * 1000.0,
                false,
                true,
            )
            .shot
            .is_some()
        })
        .count()
}

// Regression: an outcome reset the client's recovery to the host's remaining
// value without transit, so a clean-link client fired every ~10 ticks, not 8.
#[test]
fn conditioned_clean_link_hold_at_authored_rate_matches_the_host_player_shot_count() {
    let rifle = plasma_rifle(1.0e6);
    let host_player = host_player_shots(&rifle, STREAM_TICKS);
    assert_eq!(
        host_player, 45,
        "130 ms recovery is 8 ticks: 45 shots in 6 s"
    );
    let (_, report) = run_steady_hold(LinkConfig::perfect(), rifle, true);
    let summary = summarize("clean", &report);
    assert_eq!(report.predicted.len(), host_player, "{summary}");
    assert_eq!(report.authorized, report.predicted, "{summary}");
    assert!(
        report.initiation_rejected.is_empty() && report.fire_denied.is_empty(),
        "{summary}"
    );
}

// A client that stamps its starts one recovery apart in client ticks, while its
// clock runs many times faster than the host's, gains no fire rate: authorized
// shots over any host window stay within the cadence window bound.
#[test]
fn conditioned_tick_stamping_client_cannot_exceed_the_host_cadence_bound() {
    const HOST_TICKS: u32 = 360;
    for commands_per_host_tick in [8u32, 24] {
        let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
        let mut client_tick = 1000u32;
        for _ in 0..HOST_TICKS {
            for _ in 0..commands_per_host_tick {
                let command = if client_tick.is_multiple_of(RIFLE_RECOVERY_TICKS) {
                    stamped_start(client_tick)
                } else {
                    neutral()
                };
                fixture.send_input(client_tick, &command);
                client_tick += 1;
            }
            fixture.host_tick();
        }
        let label = format!("{commands_per_host_tick} commands per host tick");
        let authorized = fixture.authorized.len() as u32;
        // The first and last host ticks of the run are `HOST_TICKS - 1` apart.
        let run_window = HOST_TICKS - 1;
        // The cadence, not a clogged lane, is what limits the stamping client.
        assert!(
            authorized >= run_window / RIFLE_RECOVERY_TICKS,
            "{label}: {authorized} authorized"
        );
        assert!(
            authorized
                <= netcode::HostCommandQueues::cadence_window_bound(
                    run_window,
                    RIFLE_RECOVERY_TICKS
                ),
            "{label}: {authorized} authorized"
        );
        assert_within_cadence_window_bound(&fixture, RIFLE_RECOVERY_TICKS, &label);
    }
}

// Regression: after an A→B→A switch, A's start read no recorded recovery, fell
// back to the host's own cooldown, and was refused after compressed delivery.
// Also: a backlog's commands after the switch froze B's recovery under B's own
// retained start, which then held A's start until its claim lagged past the
// allowance.
#[test]
fn conditioned_alternating_two_weapons_at_authored_rates_are_never_refused() {
    // A floor proving both weapons were exercised: each fires on most visits.
    const MIN_SHOTS_PER_WEAPON: usize = 6;
    let plasma = plasma_rifle(1.0e6);
    let rifle = hitscan_rifle("hold", 200.0);
    let mut failures = Vec::new();
    for (label, link, hitch) in [
        (
            "clean+host-hitch",
            LinkConfig::perfect(),
            Some((60u32, 12u32)),
        ),
        ("mandated", mandated_link(), None),
        ("mandated+host-hitch", mandated_link(), Some((60, 12))),
        (
            "mandated-seed-a+host-hitch",
            mandated_link_seeded(TRIM_SEED_A),
            Some((45, 12)),
        ),
    ] {
        // A phase of 9 ticks fires each weapon about once per visit; 24 several times.
        for phase in [9u32, 24] {
            let mut fixture = Fixture::new(link, plasma.clone());
            fixture.mirror_rejection_despawn = true;
            let second = fixture.equip_second(&rifle);
            clear_line_of_fire(&fixture);
            let start = 1000u32;
            let mut driver = HitchDriver::new(hitch, start);
            for offset in 0..STREAM_TICKS {
                let visit = offset % phase == 0;
                let mut command = held(ActivationLane::Primary, visit);
                if visit {
                    command.select_slot = Some(((offset / phase) % 2) as usize);
                }
                driver.step(&mut fixture, offset, command);
            }
            fixture.idle(start + STREAM_TICKS, 360);
            let mut report = stream_report(&fixture, true);
            // Only the plasma's shots are bolts; the rifle's hitscan has no flight.
            report.bolts_removed_early.retain(|id| {
                fixture
                    .authorized
                    .iter()
                    .any(|shot| shot.shot_id == *id && shot.is_projectile)
            });
            let per_weapon = [fixture.host_actors.weapon, second].map(|weapon| {
                fixture
                    .authorized
                    .iter()
                    .filter(|shot| shot.weapon == weapon)
                    .count()
            });
            let label = format!("alternating {label} phase {phase}");
            if per_weapon.iter().any(|&count| count < MIN_SHOTS_PER_WEAPON) {
                failures.push(format!(
                    "{} per weapon {per_weapon:?}",
                    summarize(&label, &report)
                ));
            }
            failures.extend(stream_cell_failure(
                &label, link, hitch, &fixture, &report, 0,
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// Regression: a start the host admitted late fired along the aim of the later
// command that delivered it, so the host's bolt left on a different line.
#[test]
fn conditioned_late_admitted_start_fires_along_its_own_aim_while_the_pawn_faces_the_delivering_command()
 {
    let mut fixture = Fixture::new(LinkConfig::perfect(), descriptor(true, false, 1, "press"));
    fixture.idle(1, 30);
    let start = token(40, ActivationLane::Primary);
    let (start_yaw, start_pitch) = (0.6_f32, 0.2_f32);
    let (later_yaw, later_pitch) = (-0.9_f32, -0.3_f32);
    // A stalled stream arrives at once: the catch-up trim keeps only the newest
    // two commands, so the retained start is delivered by command 50.
    for tick in 31..=51 {
        let mut command = neutral();
        if tick == start.start_tick {
            command = held(ActivationLane::Primary, true);
            command.activation.initiation = Some(start);
            command.movement.facing_yaw = start_yaw;
            fixture.aim_pitch = start_pitch;
        } else {
            command.movement.facing_yaw = later_yaw;
            fixture.aim_pitch = later_pitch;
        }
        fixture.send_input(tick, &command);
    }
    fixture.host_tick();
    let fire_tick = fixture.tick - 1;
    assert_eq!(
        fixture.playout.last().map(|(_, tick, _)| *tick),
        Some(50),
        "the start's own command was trimmed"
    );
    let shot = fixture
        .authorized
        .iter()
        .find(|shot| shot.shot_id == fixture.shot_id(start, 0))
        .expect("the retained start is authorized");
    assert_eq!(shot.fire_tick, fire_tick);
    let aim = |yaw: f32, pitch: f32| {
        Vec3::new(
            -yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        )
    };
    let direction = shot.projectile_direction.expect("projectile launch fact");
    assert!(
        direction.dot(aim(start_yaw, start_pitch)) > 0.99,
        "the shot flies along the start's own aim: {direction:?}"
    );
    assert!(direction.dot(aim(later_yaw, later_pitch)) < 0.5);
    let presentation = fixture
        .presentations
        .iter()
        .find(|launch| launch.shot_id == shot.shot_id)
        .expect("observer launch");
    assert!(presentation.direction.dot(direction) > 0.999);
    assert!((presentation.origin - shot.fire_origin).length() < 1.0e-4);
    let (_, facing) = *fixture
        .host_facing
        .iter()
        .find(|(tick, _)| *tick == fire_tick)
        .expect("the host recorded its facing on the fire tick");
    near(facing, later_yaw);
}

/// Where [`hand_weapon`] puts the client's slot-0 host weapon.
#[derive(Clone, Copy)]
enum WeaponLocation {
    /// Back in the client pawn's slot 0. A holder's inventory is left as it was.
    Pawn,
    /// Out of the client pawn's inventory, as when dropped into the world: its
    /// cooldown freezes.
    World,
    /// Out of the client pawn's inventory and into `holder`'s slot 0, as when
    /// handed over.
    Holder(EntityId),
}

fn hand_weapon(fixture: &Fixture, to: WeaponLocation) {
    let mut registry = fixture.host.borrow_mut();
    let weapon = fixture.host_actors.weapon;
    let mut inventory = registry
        .get_component::<Inventory>(fixture.host_actors.pawn)
        .expect("client pawn holds an inventory")
        .clone();
    inventory.wieldables[0] = matches!(to, WeaponLocation::Pawn).then_some(weapon);
    registry
        .set_component(fixture.host_actors.pawn, inventory)
        .expect("client pawn inventory updates");
    if let WeaponLocation::Holder(holder) = to {
        let mut holder_inventory = Inventory::default();
        holder_inventory.wieldables[0] = Some(weapon);
        registry
            .set_component(holder, holder_inventory)
            .expect("holder inventory attaches");
    }
}

// A stamping client that lets its weapon go between starts, dropped to the
// world (its cooldown frozen) or handed to a holder who wields it (its cooldown
// running), cannot restart its credited recovery when it takes the weapon
// back: on that weapon, authorized shots over any host window stay within the
// cadence window bound.
#[test]
fn conditioned_drop_or_hand_over_under_stamping_cannot_exceed_the_weapon_bound() {
    use postretro_combat_model::activation::CHARGE_TOLERANCE_TICKS;
    const HOST_TICKS: u32 = 600;
    // The weapon is away for one host tick on even cycles and two on odd ones.
    const MAX_AWAY_TICKS: u32 = 2;
    for (recovery_ms, recovery_ticks) in [(16.0, 1u32), (66.0, 4), (130.0, 8), (500.0, 30)] {
        for (drop_period, holder_wields) in [(5u32, false), (23, false), (5, true), (23, true)] {
            let mut fixture =
                Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", recovery_ms));
            let holder = fixture.host.borrow_mut().spawn(Transform::default());
            let away_to = if holder_wields {
                WeaponLocation::Holder(holder)
            } else {
                WeaponLocation::World
            };
            // Leave late enough in each cycle that even the longer absence
            // returns before the next one begins.
            let leaves_at = drop_period - 1 - MAX_AWAY_TICKS;
            let mut client_tick = 1000u32;
            let mut away = false;
            for host_tick in 0..HOST_TICKS {
                let phase = host_tick % drop_period;
                let away_ticks = 1 + (host_tick / drop_period) % MAX_AWAY_TICKS;
                if phase == leaves_at {
                    hand_weapon(&fixture, away_to);
                    away = true;
                } else if away && phase == leaves_at + away_ticks {
                    hand_weapon(&fixture, WeaponLocation::Pawn);
                    away = false;
                }
                if away && holder_wields {
                    // The other holder's machine counts the cooldown down.
                    let mut registry = fixture.host.borrow_mut();
                    let Ok(ComponentValue::Weapon(component)) = registry
                        .get_component_value_mut(fixture.host_actors.weapon, ComponentKind::Weapon)
                    else {
                        panic!("the handed-over weapon stays live");
                    };
                    component.cooldown_remaining_ms =
                        (component.cooldown_remaining_ms - DT * 1000.0).max(0.0);
                }
                for _ in 0..8 {
                    let command = if client_tick.is_multiple_of(recovery_ticks) {
                        stamped_start(client_tick)
                    } else {
                        neutral()
                    };
                    fixture.send_input(client_tick, &command);
                    client_tick += 1;
                }
                fixture.host_tick();
            }
            let label = format!(
                "R {recovery_ticks}, away every {drop_period}, holder wields {holder_wields}"
            );
            let authorized = fixture.authorized.len() as u32;
            assert!(
                authorized >= HOST_TICKS / (recovery_ticks + CHARGE_TOLERANCE_TICKS) / 2,
                "{label}: the weapon still fires, {authorized} authorized"
            );
            assert_within_cadence_window_bound(&fixture, recovery_ticks, &label);
        }
    }
}

// Regression: a weapon handed back to a client carried that client's stale
// cadence record, which zeroed the host cooldown another holder had just begun.
#[test]
fn conditioned_handed_back_weapon_cannot_fire_on_its_old_holders_stale_record() {
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 130.0));
    let mut client_tick = 1000u32;
    let send = |fixture: &mut Fixture, client_tick: u32, start: bool| {
        let command = if start {
            stamped_start(client_tick)
        } else {
            neutral()
        };
        fixture.send_input(client_tick, &command);
    };
    for _ in 0..40 {
        send(
            &mut fixture,
            client_tick,
            client_tick.is_multiple_of(RIFLE_RECOVERY_TICKS),
        );
        client_tick += 1;
        fixture.host_tick();
    }
    let earlier = fixture.authorized.len();
    assert!(earlier >= 3, "the client's own cadence is recorded");
    let other = fixture.host.borrow_mut().spawn(Transform::default());
    hand_weapon(&fixture, WeaponLocation::Holder(other));
    send(&mut fixture, client_tick, false);
    client_tick += 1;
    fixture.host_tick();
    // The other holder fires it on this host tick, so its recovery runs anew.
    let handed_tick = fixture.tick;
    {
        let mut registry = fixture.host.borrow_mut();
        let Ok(ComponentValue::Weapon(component)) =
            registry.get_component_value_mut(fixture.host_actors.weapon, ComponentKind::Weapon)
        else {
            panic!("host weapon");
        };
        component.cooldown_remaining_ms = 130.0;
    }
    hand_weapon(&fixture, WeaponLocation::Pawn);
    // Back with the client, whose stamps claim a whole recovery has passed.
    let stale = client_tick + 64;
    for tick in client_tick..stale {
        send(&mut fixture, tick, false);
    }
    send(&mut fixture, stale, true);
    for tick in stale + 1..stale + 30 {
        send(&mut fixture, tick, false);
        fixture.host_tick();
    }
    let early: Vec<u32> = fixture.authorized[earlier..]
        .iter()
        .map(|shot| shot.fire_tick)
        .filter(|&tick| tick < handed_tick + RIFLE_RECOVERY_TICKS)
        .collect();
    assert!(
        early.is_empty(),
        "no shot inside the other holder's recovery: {early:?}"
    );
    let stale_token = wire_token(token(stale, ActivationLane::Primary));
    assert!(
        fixture.outcomes.iter().any(|outcome| matches!(
            outcome,
            wire::ActivationOutcome::InitiationRejected { token, .. }
                if *token == stale_token
        )),
        "the stale-stamped start is refused, not admitted on the old record"
    );
}

/// Host ticks from when the client sent its command at `client_tick` to `host_tick`.
fn delay_since_sent(fixture: &Fixture, client_tick: u32, host_tick: u32) -> u32 {
    let sent = fixture
        .sent
        .iter()
        .position(|input| input.client_tick == client_tick)
        .expect("the command was sent");
    host_tick - fixture.sent_at[sent]
}

/// A clean link carries no stall, so playout delivers each command within a
/// few ticks of sending it.
const CLEAN_LINK_DELIVERY_TICKS: u32 = 4;

/// A 600 ms press rifle in slot 0, tapped twice 3 ticks apart: the second tap
/// lands inside its recovery, so the client's prediction refuses it, but the
/// client names a start on every rising edge as production does. `then` runs
/// the client ticks after it.
fn double_tap_then(then: impl Fn(u32) -> sim::SimCommand) -> Fixture {
    let mut fixture = Fixture::new(LinkConfig::perfect(), hitscan_rifle("press", 600.0));
    fixture.mirror_rejection_despawn = true;
    fixture.equip_second(&hitscan_rifle("press", 130.0));
    let start = 1000u32;
    for offset in 0..60 {
        let tick = start + offset;
        let command = match offset {
            0 | 3 => stamped_start(tick),
            _ => then(offset),
        };
        fixture.step(tick, command);
    }
    fixture.idle(start + 60, 60);
    let refused_tap = token(start + 3, ActivationLane::Primary);
    assert!(
        fixture
            .snapshots
            .iter()
            .all(|shot| shot.activation.shot_id.start_tick != refused_tap.start_tick),
        "the client's prediction refuses the second tap"
    );
    assert!(
        fixture
            .sent
            .iter()
            .any(|input| input.activation.initiation == Some(wire_token(refused_tap))),
        "but still names its start"
    );
    assert!(
        fixture.outcomes.iter().any(|outcome| matches!(outcome,
            wire::ActivationOutcome::InitiationRejected { token, .. } if *token == wire_token(refused_tap))),
        "the host refuses it"
    );
    assert!(
        fixture
            .authorized
            .iter()
            .all(|shot| shot.shot_id.start_tick != refused_tap.start_tick)
    );
    fixture
}

// Regression: a tap the client's prediction refused still named a start, which
// waited at the host's lane front on host time for most of the weapon's
// recovery. A switch-and-fire or a reload right after the double tap waited
// behind it, about half a second on a clean link.
#[test]
fn conditioned_locally_refused_tap_never_delays_a_following_switch_and_fire() {
    let fired = 1006u32;
    let fixture = double_tap_then(|offset| {
        let mut command = if offset == 6 {
            stamped_start(1000 + offset)
        } else {
            neutral()
        };
        if offset == 5 {
            command.select_slot = Some(1);
        }
        command
    });
    let shot = fixture
        .authorized
        .iter()
        .find(|shot| shot.shot_id.start_tick == fired)
        .expect("the switched-to weapon fires");
    assert_ne!(shot.weapon, fixture.host_actors.weapon, "fired from slot 1");
    let delay = delay_since_sent(&fixture, fired, shot.fire_tick);
    assert!(
        delay <= CLEAN_LINK_DELIVERY_TICKS,
        "fired {delay} host ticks after sending"
    );
}

#[test]
fn conditioned_locally_refused_tap_never_delays_a_following_reload() {
    let pressed = 1005u32;
    let fixture = double_tap_then(|offset| {
        let mut command = neutral();
        command.reload = offset == 5;
        command
    });
    let (host_tick, _) = fixture
        .resolved_commands
        .iter()
        .find(|(_, resolved)| resolved.command.reload)
        .expect("the reload press reaches the host");
    let delay = delay_since_sent(&fixture, pressed, *host_tick);
    assert!(
        delay <= CLEAN_LINK_DELIVERY_TICKS,
        "reloaded {delay} host ticks after sending"
    );
}

/// A three-round burst tapped at `TAP`, whose input then stalls into the
/// catch-up trim, so the host admits the start late. Four ticks into the burst,
/// after its second shot and before its third, `edge` adds a reload press or
/// the burst's cancel.
fn late_burst_with_edge(edge: impl Fn(&mut sim::SimCommand, ActivationToken)) -> Fixture {
    const START: u32 = 1000;
    const TAP: u32 = 10;
    const STALL: std::ops::Range<u32> = 8..30;
    let mut fixture = Fixture::new(LinkConfig::perfect(), burst_rifle("press", 130.0));
    fixture.mirror_rejection_despawn = true;
    let tap = token(START + TAP, ActivationLane::Primary);
    let mut withheld = Vec::new();
    for offset in 0..60 {
        let tick = START + offset;
        let mut command = if offset == TAP {
            stamped_start(tick)
        } else {
            neutral()
        };
        if offset == TAP + 4 {
            edge(&mut command, tap);
        }
        fixture.predict(tick, &mut command);
        if STALL.contains(&offset) {
            withheld.push((tick, command));
        } else {
            for (tick, command) in withheld.drain(..) {
                fixture.send_input(tick, &command);
            }
            fixture.send_input(tick, &command);
        }
        fixture.advance_projectiles();
        fixture.host_tick();
    }
    fixture.idle(START + 60, 120);
    let delivered = fixture
        .resolved_commands
        .iter()
        .find(|(_, resolved)| resolved.command.activation.initiation == Some(tap))
        .map(|(_, resolved)| resolved.client_tick)
        .expect("the stalled start is delivered");
    assert!(
        delivered > tap.start_tick + 4,
        "the start is admitted late, behind its edge"
    );
    let predicted: Vec<u8> = fixture
        .snapshots
        .iter()
        .map(|shot| shot.activation.shot_id.ordinal)
        .collect();
    assert_eq!(
        predicted,
        vec![0, 1],
        "the client fires two rounds before its edge"
    );
    fixture
}

fn assert_host_fires_what_the_client_fired(fixture: &Fixture) {
    let predicted: Vec<ShotId> = fixture
        .snapshots
        .iter()
        .map(|shot| shot.activation.shot_id)
        .collect();
    let authorized: Vec<ShotId> = fixture.authorized.iter().map(|shot| shot.shot_id).collect();
    assert_eq!(
        authorized, predicted,
        "the host mints every round the client fired, no more"
    );
    assert!(
        fixture.hit_refusals.is_empty(),
        "{:?}",
        fixture.hit_refusals
    );
}

// Regression: after a late start's delivery, a reload the client pressed
// mid-burst reached the host on the next tick and cut the burst short of a
// round the client had already fired, so that round's HIT was denied.
#[test]
fn conditioned_reload_pressed_mid_burst_keeps_every_round_fired_before_it_after_a_stall() {
    let fixture = late_burst_with_edge(|command, _| command.reload = true);
    assert_host_fires_what_the_client_fired(&fixture);
}

// Regression: the same for a cancel, which reached the host on the start's own
// delivery tick.
#[test]
fn conditioned_cancel_mid_burst_keeps_every_round_fired_before_it_after_a_stall() {
    let fixture = late_burst_with_edge(|command, tap| command.activation.cancel = Some(tap));
    assert_host_fires_what_the_client_fired(&fixture);
}

// Weapon-switch declarations ordered by client tick with starts and presses.
mod switch_order;
