use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_entities::{EntityRegistry, Transform};
use postretro_foundation::{PlayerMovementComponent, TeleportState};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_net::wire::{
    InputCommand, NetworkId, WireActivationInput, WireFireButtonState, WireMovementInput,
    WireTeleportState,
};

use crate::collision::CollisionWorld;
use crate::command_queue::{
    HostCommandQueues, MovementOwners, ResolutionSource, host_resolve_remote_commands_in_registry,
};
use crate::movement_state::movement_state_to_wire;
use crate::predict_reconcile_harness_test_fixtures::component;
use crate::prediction::{ClientPrediction, CorrectionClass};
use crate::reconcile::reconcile_local_pawn;

fn command(tick: u32, yaw: f32, stamp: f32) -> InputCommand {
    InputCommand {
        client_tick: tick,
        teleport_yaw_stamp: stamp,
        movement: WireMovementInput {
            wish_dir: [0.0, 0.0],
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: yaw,
            use_pressed: false,
            drop_pressed: false,
            aim_pitch: 0.0,
            firing_slot: 0,
        },
        fire_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        secondary_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        activation: WireActivationInput::default(),
    }
}

fn world() -> CollisionWorld {
    let record = |source_entity| TeleporterRecord {
        source_entity,
        mode: TeleportMode::Seamless,
        enabled_on_spawn: true,
        // Empty regions are sufficient for adoption tests: they must not fire
        // again during replay, and their installed indices are still valid.
        brushes: Vec::new(),
        destination_origin: [100.0, 0.0, 0.0],
        destination_yaw: 0.0,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    };
    let mut world = CollisionWorld::new();
    world.teleporters =
        postretro_physics::teleporters::Teleporters::from_section(&TeleportersSection {
            teleporters: vec![record(10), record(11)],
        });
    world
}

#[test]
fn teleport_input_stamp_survives_real_held_neutral_and_late_commands() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport.accumulated_yaw = 1.0;
    registry.set_component(pawn, movement).unwrap();
    let mut owners = MovementOwners::default();
    owners.set(pawn, 7);
    let mut queues = HostCommandQueues::default();
    queues.ingest(7, &command(0, 0.2, 0.4));
    queues.ingest(7, &command(1, 0.2, 0.4));
    for _ in 0..2 {
        let resolved = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
        assert_eq!(resolved[0].source, ResolutionSource::Real);
        assert!((resolved[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    }
    let held = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    assert_eq!(held[0].source, ResolutionSource::Held);
    assert!((held[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    let mut neutral = held;
    for _ in 0..4 {
        neutral = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    }
    assert_eq!(neutral[0].source, ResolutionSource::Neutral);
    assert!((neutral[0].command.movement.facing_yaw - 0.8).abs() < 1.0e-6);
    assert!((neutral[0].teleport_yaw_stamp - 0.4).abs() < 1.0e-6);
    let next_tick = queues.resolved_cursor(7).unwrap().wrapping_add(1);
    queues.ingest(7, &command(next_tick, 1.2, 1.0));
    queues.ingest(7, &command(next_tick.wrapping_add(1), 1.2, 1.0));
    let late = host_resolve_remote_commands_in_registry(&owners, &mut queues, &registry);
    assert_eq!(late[0].source, ResolutionSource::Real);
    assert!((late[0].command.movement.facing_yaw - 1.2).abs() < 1.0e-6);
}

#[test]
fn teleport_stamp_sanitization_rejects_invalid_frame_without_queueing() {
    let mut queues = HostCommandQueues::default();
    for stamp in [
        f32::NAN,
        f32::INFINITY,
        -std::f32::consts::PI,
        std::f32::consts::TAU,
    ] {
        assert!(!queues.ingest(7, &command(0, 0.0, stamp)));
    }
    assert!(queues.resolve_tick(7).is_none());
}

#[test]
fn teleport_same_generation_replacement_cuts_short_distance_and_keeps_ring_without_cue() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport = TeleportState {
        generation: 1,
        last_teleporter: 0,
        ..Default::default()
    };
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    assert!(prediction.first_record_pending());
    prediction.mark_first_record_adopted();
    let (transform, movement, _) = prediction
        .predict_tick_with_events(
            command(1, 0.0, 0.0),
            (Transform::default(), movement),
            &world,
            environment,
            1.0 / 60.0,
        )
        .unwrap();
    registry.set_component(pawn, transform).unwrap();
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport.last_teleporter = 1;
    let class = reconcile_local_pawn(
        &mut registry,
        &mut prediction,
        pawn,
        Transform {
            position: Vec3::X,
            ..transform
        },
        Some(&wire),
        Some(0),
        &world,
        environment,
        1.0 / 60.0,
    );
    assert_eq!(class, Some(CorrectionClass::Teleport));
    assert_eq!(
        prediction.history().len(),
        1,
        "generation cut retains unacked command"
    );
    let adoption = prediction.teleport_adoptions[0];
    assert!(adoption.camera_cut);
    assert!(adoption.yaw_delta.abs() < 1.0e-6);
    assert_eq!(adoption.cue_teleporter, None);
}

#[test]
fn teleport_first_record_is_silent_but_same_pawn_rebaseline_adopts_with_yaw_and_last_cue() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let movement = component();
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport = WireTeleportState::new(1, 0.5, 0, &[0, 1]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X * 100.0,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Ordinary)
    );
    assert!(prediction.teleport_adoptions.is_empty());
    prediction.arm(NetworkId(7), pawn);
    wire.teleport = WireTeleportState::new(3, 1.0, 1, &[1]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X * 200.0,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Teleport)
    );
    assert_eq!(prediction.teleport_adoptions.len(), 1);
    let adoption = prediction.teleport_adoptions[0];
    assert!((adoption.yaw_delta - 0.5).abs() < 1.0e-6);
    assert_eq!(
        adoption.cue_teleporter,
        Some(1),
        "unseen arrivals coalesce to last cue"
    );
    let stored = &registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .teleport;
    assert_eq!(stored.arrival_latches, vec![1]);
}

#[test]
fn teleport_reconcile_refusal_restores_yaw_silently_and_unknown_indices_are_rejected() {
    let world = world();
    let volumes = EnvironmentVolumes::empty();
    let environment = EnvironmentView::new(0.0, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport = TeleportState {
        generation: 1,
        accumulated_yaw: 0.5,
        last_teleporter: 0,
        ..Default::default()
    };
    registry.set_component(pawn, movement.clone()).unwrap();
    let mut prediction = ClientPrediction::new();
    prediction.arm(NetworkId(7), pawn);
    prediction.mark_first_record_adopted();
    let mut wire = movement_state_to_wire(&movement, 0.0);
    wire.teleport = Default::default();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform {
                position: Vec3::X,
                ..Default::default()
            },
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        Some(CorrectionClass::Teleport)
    );
    assert!((prediction.teleport_adoptions[0].yaw_delta + 0.5).abs() < 1.0e-6);
    assert_eq!(prediction.teleport_adoptions[0].cue_teleporter, None);
    wire.teleport = WireTeleportState::new(2, 0.5, 2, &[]).unwrap();
    assert_eq!(
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            pawn,
            Transform::default(),
            Some(&wire),
            None,
            &world,
            environment,
            1.0 / 60.0
        ),
        None
    );
}

/// One host and one participating client over the real relay transport.
pub(crate) struct RelaySession {
    pub(crate) server: postretro_net::transport::NetServer,
    pub(crate) client: postretro_net::transport::NetClient,
}

impl RelaySession {
    pub(crate) const CLIENT: u64 = 7;
    pub(crate) const STEP: std::time::Duration = std::time::Duration::from_millis(16);
    pub(crate) const FINGERPRINT: [u8; 32] = [0x5a; 32];

    pub(crate) fn participating() -> Self {
        use std::net::{Ipv4Addr, UdpSocket};
        let origin = std::time::Duration::from_secs(1);
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server = postretro_net::transport::NetServer::new(
            socket,
            address,
            1,
            origin,
            Some(Self::FINGERPRINT),
        )
        .unwrap();
        server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        server.set_mod_digest(Some(Self::FINGERPRINT));
        server.set_level_parity(Some(("test-level".to_string(), Self::FINGERPRINT)));
        server.set_revealed_level(Some("test-level".to_string()));
        let mut client = postretro_net::transport::NetClient::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            address,
            Self::CLIENT,
            origin,
            Some(Self::FINGERPRINT),
            None,
        )
        .unwrap();
        client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        client.set_mod_digest(Some(Self::FINGERPRINT));
        client.set_level_parity(Some(("test-level".to_string(), Self::FINGERPRINT)));
        client.set_revealed_level(Some(postretro_net::wire::RevealedLevel::new(
            "test-level",
            None,
        )));
        server.add_relay_connection(Self::CLIENT, None);
        client.set_connected();
        let mut session = Self { server, client };
        session.settle();
        assert!(session.server.is_participating(Self::CLIENT));
        session
    }

    pub(crate) fn relay_to_server(&mut self) {
        self.client.update_connections(Self::STEP);
        for packet in self.client.packets_to_send() {
            self.server.process_packet_from(&packet, Self::CLIENT);
        }
        self.server.update_connections(Self::STEP);
        let _ = self.server.poll_handshakes();
    }

    pub(crate) fn relay_to_client(&mut self) {
        self.server.update_connections(Self::STEP);
        for packet in self.server.packets_to_send(Self::CLIENT) {
            self.client.process_packet(&packet);
        }
        self.client.update_connections(Self::STEP);
        let _ = self.client.drain_control();
    }

    pub(crate) fn settle(&mut self) {
        for _ in 0..64 {
            self.relay_to_server();
            self.relay_to_client();
        }
    }

    fn inputs(&mut self) -> Vec<InputCommand> {
        self.server
            .drain_input(Self::CLIENT)
            .iter()
            .filter_map(|bytes| match postretro_net::wire::decode(bytes) {
                Ok(postretro_net::wire::ClientMessage::Input(input)) => Some(input),
                _ => None,
            })
            .collect()
    }
}

fn pawn_with_accumulated_yaw(
    registry: &mut EntityRegistry,
    yaw: f32,
) -> postretro_entities::EntityId {
    let pawn = registry.spawn(Transform::default());
    let mut movement = component();
    movement.teleport.generation = 1;
    movement.teleport.accumulated_yaw = yaw;
    registry.set_component(pawn, movement).unwrap();
    pawn
}

// A command stamped in one session — its pawn had turned 1 rad
// through teleports — is in flight across a level change or a restart of the
// same level. The real transport's participation epoch drops it, so the new
// session's pawn (teleported 0.7 rad through the same indices) never rotates it.
// The new session's first command, sampled before its first record, carries a
// zero stamp, never the cached one.
#[test]
fn teleport_stale_epoch_input_is_never_rotated_in_the_new_session() {
    for level_change in [true, false] {
        let mut session = RelaySession::participating();
        let mut client_registry = EntityRegistry::new();
        let old_pawn = pawn_with_accumulated_yaw(&mut client_registry, 1.0);
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(1), old_pawn);
        let mut sampled =
            crate::netcode::predict_reconcile_harness_test_fixtures::forward_command(false);
        sampled.movement.facing_yaw = 0.25;
        let stale =
            crate::netcode::stamped_client_input(&mut prediction, &sampled, &client_registry);
        assert!((stale.teleport_yaw_stamp - 1.0).abs() < 1.0e-6);
        session.client.send_input(postretro_net::wire::encode(
            &postretro_net::wire::ClientMessage::Input(stale),
        ));
        session.client.update_connections(RelaySession::STEP);
        let in_flight = session.client.packets_to_send();
        assert!(!in_flight.is_empty());

        // The host reloads the level (or restarts it) and re-promotes the client.
        // No client traffic crosses the boundary, so the old command is still in
        // flight when the new participation begins.
        if level_change {
            session.server.set_level_parity(None);
            let _ = session.server.poll_handshakes();
            session.relay_to_client();
            session
                .server
                .set_level_parity(Some(("test-level".to_string(), RelaySession::FINGERPRINT)));
        } else {
            session.server.set_revealed_level(None);
            let _ = session.server.poll_handshakes();
            session.relay_to_client();
            session
                .server
                .set_revealed_level(Some("test-level".to_string()));
        }
        let _ = session.server.poll_handshakes();
        session.relay_to_client();
        assert!(session.server.is_participating(RelaySession::CLIENT));
        prediction.reset_for_level_unload();

        // The old session's packets arrive late: the epoch drops them.
        for packet in &in_flight {
            session
                .server
                .process_packet_from(packet, RelaySession::CLIENT);
        }
        assert!(
            session.inputs().is_empty(),
            "the stale command never reaches the queue"
        );

        // New session: the host pawn has its own teleports; the client has no
        // record yet, so its first command is stamped zero.
        let mut host_registry = EntityRegistry::new();
        let new_pawn = pawn_with_accumulated_yaw(&mut host_registry, 0.7);
        let first =
            crate::netcode::stamped_client_input(&mut prediction, &sampled, &client_registry);
        assert_eq!(
            first.teleport_yaw_stamp, 0.0,
            "no cached stamp survives the unload"
        );
        session.client.send_input(postretro_net::wire::encode(
            &postretro_net::wire::ClientMessage::Input(first),
        ));
        session.relay_to_server();
        let delivered = session.inputs();
        assert_eq!(delivered.len(), 1, "only the new session's command arrives");
        let mut owners = MovementOwners::default();
        owners.set(new_pawn, RelaySession::CLIENT);
        let mut queues = HostCommandQueues::default();
        assert!(queues.ingest(RelaySession::CLIENT, &delivered[0]));
        queues.ingest(
            RelaySession::CLIENT,
            &command(delivered[0].client_tick + 1, 0.25, 0.0),
        );
        let resolved =
            host_resolve_remote_commands_in_registry(&owners, &mut queues, &host_registry);
        assert_eq!(resolved[0].client_tick, delivered[0].client_tick);
        assert!(
            (resolved[0].command.movement.facing_yaw - 0.95).abs() < 1.0e-6,
            "a zero stamp turns by the new pawn's whole yaw"
        );
    }
}
