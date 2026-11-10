//! Host install resolves each client-bound seat's spawn pose, sends its hint
//! after Relevel, and promotion spawns the pawn at that pose.
//! See: context/lib/networking.md §Slot lifecycle

use super::lifecycle_test_fixtures::{
    commit_level_bundle_source, level_world, script_ctx, test_app,
};
use super::lifecycle_world_handles::bind_installed_first_player;
use super::*;
use crate::netcode::{NetEndpoint, NetRole, SeatTable};
use crate::spawn_pose::{SpawnPose, UnbackedSpawn};
use glam::{EulerRot, Quat};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{RevealedLevel, ServerControlMessage, SpawnPoseHint};
use postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts;
use postretro_sim::scripting::map_entity::MapEntity;
use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

const STEP: Duration = Duration::from_millis(34);
const MOD_ID: &str = "spawn-pose-handoff";
const LEVEL: &str = "spawn-arena";

/// Three starts; the second, the first joiner's, faces a nonzero yaw and pitch.
fn spawn_points() -> Vec<MapEntity> {
    vec![
        crate::spawn_pose::tests::player_spawn(Vec3::ZERO, 0.0, 0.0),
        crate::spawn_pose::tests::player_spawn(Vec3::new(6.0, 1.0, -2.0), -0.3, 1.2),
        crate::spawn_pose::tests::player_spawn(Vec3::new(-6.0, 0.5, 4.0), 0.2, -2.0),
    ]
}

struct Relay {
    app: App,
    clients: Vec<(u64, NetClient)>,
    level_digest: [u8; 32],
}

impl Relay {
    /// A listen host with a host seat table and `client_ids` admitted on its
    /// relay, before any level installs.
    fn admitted(client_ids: &[u64]) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        ctx.data_registry.borrow_mut().entities = vec![
            crate::spawn_pose::tests::movement_descriptor("player", 0.4, 0.8, 0.5),
            crate::spawn_pose::tests::movement_descriptor("scout", 0.3, 0.6, 0.9),
        ];
        let mod_digest =
            crate::mod_digest::mod_compatibility_digest_from_registry(&ctx.data_registry.borrow());
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_address = socket.local_addr().unwrap();
        let mut server =
            NetServer::new(socket, server_address, 4, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity(MOD_ID.into(), "1".into());
        server.set_mod_digest(Some(mod_digest));
        let mut endpoint = NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .unwrap()
            .unwrap();
        let NetEndpoint::Host {
            server: endpoint_server,
            ..
        } = &mut endpoint
        else {
            unreachable!()
        };
        **endpoint_server = server;
        let session = app.session.as_mut().unwrap();
        session.net_endpoint = Some(endpoint);
        session.seat_table = Some(SeatTable::new().unwrap());

        let clients = client_ids
            .iter()
            .map(|&client_id| {
                let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
                let mut client = NetClient::new(
                    socket,
                    server_address,
                    client_id,
                    Duration::from_secs(1),
                    None,
                    None,
                )
                .unwrap();
                client.set_mod_identity(MOD_ID.into(), "1".into());
                client.set_mod_digest(Some(mod_digest));
                client.set_connected();
                (client_id, client)
            })
            .collect();
        let mut session = Self {
            app,
            clients,
            level_digest: [0; 32],
        };
        for &client_id in client_ids {
            session.server().add_relay_connection(client_id, None);
        }
        session.pump_until(|session| {
            client_ids
                .iter()
                .all(|&client_id| session.seat(client_id).is_some())
        });
        session
    }

    fn server(&mut self) -> &mut NetServer {
        let Some(NetEndpoint::Host { server, .. }) =
            self.app.session.as_mut().unwrap().net_endpoint.as_mut()
        else {
            unreachable!()
        };
        server
    }

    fn seats(&self) -> &SeatTable {
        self.app
            .session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
    }

    fn seat(&self, client_id: u64) -> Option<Seat> {
        self.seats().seat_for_client(client_id)
    }

    fn client(&mut self, client_id: u64) -> &mut NetClient {
        &mut self
            .clients
            .iter_mut()
            .find(|(id, _)| *id == client_id)
            .unwrap()
            .1
    }

    /// One relay exchange, the host side driven by its Running net frame.
    /// The host's socket transport drains every client's queued packets in
    /// its update, relay clients included, so what the engine queued between
    /// frames is delivered before the frame's poll runs.
    fn step(&mut self) {
        for index in 0..self.clients.len() {
            let (client_id, client) = &mut self.clients[index];
            let client_id = *client_id;
            client.update_connections(STEP);
            let packets = client.packets_to_send();
            for packet in packets {
                self.server().process_packet_from(&packet, client_id);
            }
        }
        self.server().update_connections(STEP);
        self.deliver_to_clients();
        self.app.net_poll_and_apply(0.0);
        self.deliver_to_clients();
    }

    fn deliver_to_clients(&mut self) {
        for index in 0..self.clients.len() {
            let client_id = self.clients[index].0;
            let packets = self.server().packets_to_send(client_id);
            for packet in packets {
                self.clients[index].1.process_packet(&packet);
            }
        }
    }

    fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..128 {
            self.step();
            if done(self) {
                return;
            }
        }
        panic!("relay session did not reach the expected state");
    }

    /// The host's install as production orders it: the identity stage
    /// (parity and Relevel), segment B's first-player bind and respawn
    /// publish, the post-load passes' retained spawn points, then the hints.
    fn install(&mut self) -> (EntityId, usize) {
        let world = level_world(LEVEL, 1);
        self.level_digest =
            crate::runtime_movers::level_content_digest(&world.kinematic_geometry, &world);
        // The catalog load in flight is the source install retains.
        self.app.level_load = Some(crate::startup::InFlightLevelLoad {
            map_path: PathBuf::from("content/dev/maps/spawn-arena.prl"),
            content_root: PathBuf::from("content/dev"),
            entry: crate::startup::LevelLoadEntry {
                catalog_id: Some(LEVEL.to_string()),
                path: "maps/spawn-arena.prl".to_string(),
                name: "Spawn arena".to_string(),
                tags: Vec::new(),
                loading_tree: Vec::new(),
            },
        });
        self.app.install_level_identity(&world);
        let ctx = script_ctx(&self.app);
        let points = spawn_points();
        let first = spawn_from_player_starts(
            &points,
            &ctx.data_registry.borrow().entities,
            &mut ctx.registry.borrow_mut(),
            None,
        )
        .expect("the host takes its first-player start");
        let session = self.app.session.as_mut().unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        bind_installed_first_player(seats, &mut ctx.registry.borrow_mut(), Some(first));
        crate::respawn_placements::publish(
            &ctx,
            &points,
            &ctx.data_registry.borrow().entities,
            Some(seats),
            Some(first.placement),
        );
        self.app.host_spawn_points = points;
        self.app.send_install_spawn_pose_hints();
        self.step();
        (first.pawn, first.placement)
    }

    /// The client's Relevel and spawn-pose hint, in arrival order.
    /// Pumps until the hint lands; the client has not revealed, so nothing
    /// promotes meanwhile.
    fn drain_level_naming(&mut self, client_id: u64) -> Vec<ServerControlMessage> {
        let mut received = Vec::new();
        for _ in 0..16 {
            received.extend(
                self.client(client_id)
                    .drain_control()
                    .into_iter()
                    .filter(|message| {
                        matches!(
                            message,
                            ServerControlMessage::Relevel { .. }
                                | ServerControlMessage::SpawnPoseHint(_)
                        )
                    }),
            );
            if received
                .iter()
                .any(|message| matches!(message, ServerControlMessage::SpawnPoseHint(_)))
            {
                break;
            }
            self.step();
        }
        received
    }

    /// The client installs and reveals the host's level; the host reveals too.
    fn reveal(&mut self, client_id: u64, level_life: u64) {
        let digest = self.level_digest;
        let client = self.client(client_id);
        client.set_level_parity(Some((LEVEL.into(), digest)));
        client.set_revealed_level(Some(RevealedLevel::new(LEVEL, Some(level_life))));
        self.server().set_revealed_level(Some(LEVEL.into()));
    }

    fn pawn(&self, client_id: u64) -> Option<EntityId> {
        self.seats().pawn_for_client(client_id)
    }

    fn eye(&self, pawn: EntityId) -> Vec3 {
        let ctx = script_ctx(&self.app);
        let registry = ctx.registry.borrow();
        registry.get_component::<Transform>(pawn).unwrap().position
            + Vec3::Y
                * registry
                    .get_component::<PlayerMovementComponent>(pawn)
                    .unwrap()
                    .capsule
                    .eye_height
    }

    fn rotation(&self, pawn: EntityId) -> Quat {
        script_ctx(&self.app)
            .registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap()
            .rotation
    }
}

fn hint_of(messages: &[ServerControlMessage]) -> SpawnPoseHint {
    match messages {
        [
            ServerControlMessage::Relevel { .. },
            ServerControlMessage::SpawnPoseHint(hint),
        ] => hint.clone(),
        other => panic!("expected Relevel then its hint, got {other:?}"),
    }
}

fn assert_pawn_at_hint(session: &Relay, pawn: EntityId, hint: &SpawnPoseHint) {
    let eye = session.eye(pawn);
    assert!(
        eye.distance(Vec3::from_array(hint.eye)) < 0.01,
        "pawn eye {eye:?} is within 1 cm of the hinted eye {:?}",
        hint.eye
    );
    let expected = Quat::from_euler(EulerRot::YXZ, hint.yaw, hint.pitch, 0.0);
    assert!(
        session.rotation(pawn).angle_between(expected) < 1e-4,
        "spawn rotation carries the hinted yaw {} and pitch {}",
        hint.yaw,
        hint.pitch
    );
}

// R1, O8.
#[test]
fn host_install_sends_relevel_then_hint_and_promotion_spawns_at_the_hinted_pose() {
    let mut session = Relay::admitted(&[41]);
    let (host_pawn, host_placement) = session.install();

    let messages = session.drain_level_naming(41);
    let life = match messages.first() {
        Some(ServerControlMessage::Relevel {
            catalog_id,
            level_life,
        }) => {
            assert_eq!(catalog_id, LEVEL);
            *level_life
        }
        other => panic!("Relevel comes first, got {other:?}"),
    };
    let hint = hint_of(&messages);
    assert!(
        !session.server().is_participating(41) && !session.client(41).is_participating(),
        "the hint precedes promotion"
    );
    assert_eq!(hint.level_identity, LEVEL);
    assert_eq!(hint.level_life, Some(life), "the life Revealed declares");
    let start = &spawn_points()[1];
    assert_eq!((hint.yaw, hint.pitch), (start.angles.y, start.angles.x));
    assert!(
        hint.yaw != 0.0 && hint.pitch != 0.0,
        "fixture faces off-axis"
    );

    session.reveal(41, life);
    session.pump_until(|session| session.pawn(41).is_some());

    let pawn = session.pawn(41).unwrap();
    assert_pawn_at_hint(&session, pawn, &hint);
    // R14: the host keeps its install spawn and no joiner is given it.
    let seat = session.seat(41).unwrap();
    assert_ne!(
        session.seats().placement_for_seat(seat),
        Some(host_placement)
    );
    assert_eq!(
        session.seats().placement_for_seat(Seat(0)),
        Some(host_placement)
    );
    assert_eq!(session.seats().pawn_for_client(41), Some(pawn));
    let ctx = script_ctx(&session.app);
    assert_eq!(
        ctx.registry.borrow().local_player_pawn(),
        Some(host_pawn),
        "the host's pawn is still its own"
    );
    assert!(
        ctx.registry
            .borrow()
            .get_component::<Transform>(host_pawn)
            .unwrap()
            .position
            .distance(spawn_points()[host_placement].origin + Vec3::Y * 1.2)
            < 1e-5
    );
}

// R2.
#[test]
fn unbacked_test_pose_reserves_nothing_and_its_pawn_spawns_there() {
    let mut session = Relay::admitted(&[41, 42]);
    // Install resolves in seat order; offer the lower seat, so the other
    // seat resolves after it.
    let (offered_client, other_client) = if session.seat(41) < session.seat(42) {
        (41, 42)
    } else {
        (42, 41)
    };
    let offered = session.seat(offered_client).unwrap();
    let other = session.seat(other_client).unwrap();
    let scout = UnbackedSpawn {
        pose: SpawnPose {
            eye: Vec3::new(20.0, 3.0, -9.0),
            yaw: 0.8,
            pitch: 0.25,
        },
        entity_class: "scout".into(),
        single_use: false,
    };
    session.app.spawn_poses.set_unbacked_source(Some(Box::new(
        crate::spawn_pose::tests::OfferOne {
            seat: offered,
            spawn: scout.clone(),
            consumed: Default::default(),
        },
    )));
    let (_, host_placement) = session.install();

    let offered_hint = hint_of(&session.drain_level_naming(offered_client));
    let other_hint = hint_of(&session.drain_level_naming(other_client));
    assert_eq!(offered_hint.eye, scout.pose.eye.to_array());
    assert_eq!((offered_hint.yaw, offered_hint.pitch), (0.8, 0.25));
    assert_eq!(session.seats().placement_for_seat(offered), None);
    assert_eq!(host_placement, 0);
    assert_eq!(
        session.seats().placement_for_seat(other),
        Some(1),
        "the next seat takes the lowest free start, as if the first were absent"
    );
    let start = &spawn_points()[1];
    assert_eq!(
        (other_hint.yaw, other_hint.pitch),
        (start.angles.y, start.angles.x)
    );

    let life = offered_hint.level_life.unwrap();
    session.reveal(offered_client, life);
    session.reveal(other_client, life);
    session.pump_until(|session| session.pawn(41).is_some() && session.pawn(42).is_some());

    let offered_pawn = session.pawn(offered_client).unwrap();
    assert_pawn_at_hint(&session, offered_pawn, &offered_hint);
    assert_eq!(
        script_ctx(&session.app)
            .registry
            .borrow()
            .get_component::<DescriptorProvenance>(offered_pawn)
            .unwrap()
            .canonical_name,
        "scout",
        "the pawn spawns as the class the source named"
    );
    assert_pawn_at_hint(&session, session.pawn(other_client).unwrap(), &other_hint);
    assert_eq!(session.seats().placement_for_seat(offered), None);
}

// O8: production install sends the hints after Relevel, the host pawn's bind
// and the retained spawn points, as the harness above orders them.
#[test]
fn commit_sends_spawn_pose_hints_after_relevel_host_bind_and_spawn_points() {
    let commit = commit_level_bundle_source();
    let at = |needle: &str| {
        commit
            .find(needle)
            .unwrap_or_else(|| panic!("commit reaches `{needle}`"))
    };
    let hints = at("self.send_install_spawn_pose_hints()");
    assert!(at("endpoint.set_relevel_catalog_id(") < hints);
    assert!(at("bind_installed_first_player(") < hints);
    assert!(at("self.host_spawn_points = products.spawn_points") < hints);
    assert_eq!(
        commit
            .matches("self.send_install_spawn_pose_hints()")
            .count(),
        1
    );
}
