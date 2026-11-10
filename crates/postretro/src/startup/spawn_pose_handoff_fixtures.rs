//! In-memory listen host and relay clients for the spawn-pose hint tests: the
//! host's real net polls, a stand-in for install's segment B, and each
//! client's Control stream in arrival order, participation markers included.
//! See: context/lib/testing_guide.md §4

use super::lifecycle_test_fixtures::{level_world, script_ctx, test_app};
use super::lifecycle_world_handles::bind_installed_first_player;
use super::*;
use crate::netcode::{NetEndpoint, NetRole, SeatTable};
use glam::{EulerRot, Quat};
use postretro_entities::{EntityId, EntityTypeDescriptor, Transform};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_net::transport::{ControlArrival, NetClient, NetServer};
use postretro_net::wire::{
    ConnectClaim, PlayerClaimId, RevealedLevel, ServerControlMessage, SpawnPoseHint,
    encode_connect_claim,
};
use postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts;
use postretro_sim::scripting::map_entity::MapEntity;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

pub(super) use crate::spawn_pose::tests::{OfferOne, movement_descriptor, player_spawn};

const STEP: Duration = Duration::from_millis(34);
const MOD_ID: &str = "spawn-pose-handoff";
pub(super) const LEVEL: &str = "spawn-arena";
const PLAYER_CAPSULE_HALF_EXTENT: f32 = 0.4 + 0.8;

pub(super) fn descriptors() -> Vec<EntityTypeDescriptor> {
    vec![
        movement_descriptor("player", 0.4, 0.8, 0.5),
        movement_descriptor("scout", 0.3, 0.6, 0.9),
    ]
}

/// `count` starts; every joiner start faces a nonzero yaw and pitch.
pub(super) fn spawn_points(count: usize) -> Vec<MapEntity> {
    (0..count)
        .map(|index| {
            if index == 0 {
                return player_spawn(Vec3::ZERO, 0.0, 0.0);
            }
            let index = index as f32;
            player_spawn(
                Vec3::new(6.0 * index, 0.5 * index, -2.0 * index),
                -0.1 * index,
                0.7 * index,
            )
        })
        .collect()
}

/// Where a `player` pawn on start `index` of `points` stands.
pub(super) fn standing_on(points: &[MapEntity], index: usize) -> Vec3 {
    points[index].origin + Vec3::Y * PLAYER_CAPSULE_HALF_EXTENT
}

/// The hint a `player` joiner on start `index` of `points` receives.
pub(super) fn hint_pose_of(points: &[MapEntity], index: usize) -> ([f32; 3], f32, f32) {
    let eye = standing_on(points, index) + Vec3::Y * 0.5;
    (
        eye.to_array(),
        points[index].angles.y,
        points[index].angles.x,
    )
}

pub(super) fn pose(hint: &SpawnPoseHint) -> ([f32; 3], f32, f32) {
    (hint.eye, hint.yaw, hint.pitch)
}

/// One Control arrival a test reads.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Naming {
    Relevel(u64),
    Hint(SpawnPoseHint),
    Marker,
}

pub(super) struct RelayClient {
    id: u64,
    client: NetClient,
    log: Vec<ControlArrival>,
}

pub(super) struct Relay {
    pub(super) app: App,
    clients: Vec<RelayClient>,
    server_address: SocketAddr,
    mod_digest: [u8; 32],
    level_digest: [u8; 32],
    /// The host poll the frame runs: the Running net frame, or the world-less
    /// poll a Loading, Settling or Frontend frame runs.
    pub(super) world_less: bool,
}

fn claim(client_id: u64) -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([client_id as u8; 16]),
        display_name: format!("Player {client_id}"),
    }
}

impl Relay {
    /// A listen host with a host seat table, no level and no clients.
    pub(super) fn host() -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        ctx.data_registry.borrow_mut().entities = descriptors();
        let mod_digest =
            crate::mod_digest::mod_compatibility_digest_from_registry(&ctx.data_registry.borrow());
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_address = socket.local_addr().unwrap();
        let mut server =
            NetServer::new(socket, server_address, 6, Duration::from_secs(1), None).unwrap();
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
        Self {
            app,
            clients: Vec::new(),
            server_address,
            mod_digest,
            level_digest: [0; 32],
            world_less: false,
        }
    }

    /// [`Self::host`] with `client_ids` connected and seated.
    pub(super) fn admitted(client_ids: &[u64]) -> Self {
        let mut relay = Self::host();
        relay.admit(client_ids);
        relay
    }

    /// Connect a client under `claim_of`'s player identity, without pumping.
    pub(super) fn connect_as(&mut self, client_id: u64, claim_of: u64) -> &mut NetClient {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = NetClient::new(
            socket,
            self.server_address,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap();
        client.set_mod_identity(MOD_ID.into(), "1".into());
        client.set_mod_digest(Some(self.mod_digest));
        client.set_connected();
        self.server()
            .add_relay_connection(client_id, Some(encode_connect_claim(&claim(claim_of))));
        self.clients.push(RelayClient {
            id: client_id,
            client,
            log: Vec::new(),
        });
        self.client(client_id)
    }

    /// Connect `client_ids` and pump until each holds a seat.
    pub(super) fn admit(&mut self, client_ids: &[u64]) {
        for &client_id in client_ids {
            self.connect_as(client_id, client_id);
        }
        self.pump_until(|relay| {
            client_ids
                .iter()
                .all(|&client_id| relay.seat(client_id).is_some())
        });
    }

    /// A transport disconnect, as a closed socket reports it.
    pub(super) fn disconnect(&mut self, client_id: u64) {
        let _ = self
            .server()
            .close_relay_connection(client_id, postretro_net::slots::CloseCause::Disconnect);
        self.clients.retain(|client| client.id != client_id);
        self.step();
    }

    pub(super) fn server(&mut self) -> &mut NetServer {
        let Some(NetEndpoint::Host { server, .. }) =
            self.app.session.as_mut().unwrap().net_endpoint.as_mut()
        else {
            unreachable!()
        };
        server
    }

    fn server_ref(&self) -> &NetServer {
        let Some(NetEndpoint::Host { server, .. }) =
            self.app.session.as_ref().unwrap().net_endpoint.as_ref()
        else {
            unreachable!()
        };
        server
    }

    pub(super) fn participating(&self, client_id: u64) -> bool {
        self.server_ref().is_participating(client_id)
    }

    pub(super) fn seats(&self) -> &SeatTable {
        self.app
            .session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
    }

    pub(super) fn seats_mut(&mut self) -> &mut SeatTable {
        self.app
            .session
            .as_mut()
            .unwrap()
            .seat_table
            .as_mut()
            .unwrap()
    }

    pub(super) fn seat(&self, client_id: u64) -> Option<Seat> {
        self.seats().seat_for_client(client_id)
    }

    pub(super) fn placement(&self, client_id: u64) -> Option<usize> {
        self.seats().placement_for_seat(self.seat(client_id)?)
    }

    pub(super) fn client(&mut self, client_id: u64) -> &mut NetClient {
        &mut self
            .clients
            .iter_mut()
            .find(|client| client.id == client_id)
            .unwrap()
            .client
    }

    /// One relay exchange around one host poll. The host's socket transport
    /// drains every client's queued packets inside its update, relay clients
    /// included, so packets are collected before the poll as well as after.
    /// Renet resends what that drain lost, in order.
    pub(super) fn step(&mut self) {
        for index in 0..self.clients.len() {
            let client_id = self.clients[index].id;
            let client = &mut self.clients[index].client;
            client.update_connections(STEP);
            let packets = client.packets_to_send();
            for packet in packets {
                self.server().process_packet_from(&packet, client_id);
            }
        }
        self.server().update_connections(STEP);
        self.deliver_to_clients();
        if self.world_less {
            let _ = self.app.poll_world_less_transport(0.0);
        } else {
            self.app.net_poll_and_apply(0.0);
        }
        self.deliver_to_clients();
    }

    fn deliver_to_clients(&mut self) {
        for index in 0..self.clients.len() {
            let client_id = self.clients[index].id;
            let packets = self.server().packets_to_send(client_id);
            let client = &mut self.clients[index];
            for packet in packets {
                client.client.process_packet(&packet);
            }
            let arrivals = client.client.drain_control_with_markers();
            client.log.extend(arrivals);
        }
    }

    pub(super) fn pump_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..160 {
            self.step();
            if done(self) {
                return;
            }
        }
        panic!("relay session did not reach the expected state");
    }

    /// Pump a few frames with nothing expected, so a late send would land.
    pub(super) fn settle(&mut self) {
        for _ in 0..24 {
            self.step();
        }
    }

    /// The host's catalog install of `points`.
    pub(super) fn install(&mut self, points: Vec<MapEntity>) -> Option<(EntityId, usize)> {
        self.install_from(Some(LEVEL), points)
    }

    /// The host's install as production orders it: the identity stage
    /// (parity and Relevel), segment B's first-player bind and respawn
    /// publish, the post-load passes' retained spawn points, then the hints.
    /// `catalog_id` `None` installs an uncatalogued level.
    pub(super) fn install_from(
        &mut self,
        catalog_id: Option<&str>,
        points: Vec<MapEntity>,
    ) -> Option<(EntityId, usize)> {
        let world = level_world(LEVEL, 1);
        self.level_digest =
            crate::runtime_movers::level_content_digest(&world.kinematic_geometry, &world);
        // The load in flight is the source install retains.
        self.app.level_load = Some(crate::startup::InFlightLevelLoad {
            map_path: PathBuf::from("content/dev/maps/spawn-arena.prl"),
            content_root: PathBuf::from("content/dev"),
            entry: crate::startup::LevelLoadEntry {
                catalog_id: catalog_id.map(str::to_string),
                path: "maps/spawn-arena.prl".to_string(),
                name: "Spawn arena".to_string(),
                tags: Vec::new(),
                loading_tree: Vec::new(),
            },
        });
        self.app.install_level_identity(&world);
        self.app.level_load = None;
        let ctx = script_ctx(&self.app);
        let first = spawn_from_player_starts(
            &points,
            &ctx.data_registry.borrow().entities,
            &mut ctx.registry.borrow_mut(),
            None,
        );
        let session = self.app.session.as_mut().unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        bind_installed_first_player(seats, &mut ctx.registry.borrow_mut(), first);
        crate::respawn_placements::publish(
            &ctx,
            &points,
            &ctx.data_registry.borrow().entities,
            Some(seats),
            first.map(|first| first.placement),
        );
        self.app.host_spawn_points = points;
        self.app.send_install_spawn_pose_hints();
        self.step();
        first.map(|first| (first.pawn, first.placement))
    }

    /// Production unload; it clears the data registry, which a mod reload
    /// would refill, so the fixture refills it.
    pub(super) fn unload(&mut self) {
        self.app.unload_level();
        script_ctx(&self.app).data_registry.borrow_mut().entities = descriptors();
    }

    /// The host's reveal edge.
    pub(super) fn host_reveal(&mut self) {
        self.app.publish_net_reveal();
    }

    /// The client installs and reveals the host's installed level, naming the
    /// host level life as its Relevel did.
    pub(super) fn reveal(&mut self, client_id: u64) {
        let identity = self
            .app
            .published_level_identity
            .clone()
            .expect("the host names an installed level");
        let life = self.server().installed_level_life();
        self.reveal_as(client_id, &identity, life);
    }

    pub(super) fn reveal_as(&mut self, client_id: u64, identity: &str, life: Option<u64>) {
        let digest = self.level_digest;
        let client = self.client(client_id);
        client.set_level_parity(Some((identity.into(), digest)));
        client.set_revealed_level(Some(RevealedLevel::new(identity, life)));
    }

    /// `client_id`'s Relevels, hints and participation markers, in arrival order.
    pub(super) fn naming(&self, client_id: u64) -> Vec<Naming> {
        self.clients
            .iter()
            .find(|client| client.id == client_id)
            .unwrap()
            .log
            .iter()
            .filter_map(|arrival| match arrival {
                ControlArrival::ParticipationMarker { .. } => Some(Naming::Marker),
                ControlArrival::Message(ServerControlMessage::Relevel { level_life, .. }) => {
                    Some(Naming::Relevel(*level_life))
                }
                ControlArrival::Message(ServerControlMessage::SpawnPoseHint(hint)) => {
                    Some(Naming::Hint(hint.clone()))
                }
                ControlArrival::Message(_) => None,
            })
            .collect()
    }

    pub(super) fn hints(&self, client_id: u64) -> Vec<SpawnPoseHint> {
        self.naming(client_id)
            .into_iter()
            .filter_map(|naming| match naming {
                Naming::Hint(hint) => Some(hint),
                _ => None,
            })
            .collect()
    }

    /// Pump until `client_id` has received a hint, and return the latest.
    pub(super) fn await_hint(&mut self, client_id: u64) -> SpawnPoseHint {
        self.pump_until(|relay| !relay.hints(client_id).is_empty());
        self.hints(client_id).pop().unwrap()
    }

    /// `client_id`'s live pawn while it participates.
    pub(super) fn pawn(&self, client_id: u64) -> Option<EntityId> {
        let pawn = self.seats().pawn_for_client(client_id)?;
        (self.participating(client_id) && script_ctx(&self.app).registry.borrow().exists(pawn))
            .then_some(pawn)
    }

    /// Pump until `client_id` participates with a live pawn.
    pub(super) fn await_pawn(&mut self, client_id: u64) -> EntityId {
        self.pump_until(|relay| relay.pawn(client_id).is_some());
        self.pawn(client_id).unwrap()
    }

    pub(super) fn transform(&self, pawn: EntityId) -> Transform {
        *script_ctx(&self.app)
            .registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap()
    }

    pub(super) fn move_pawn(&self, pawn: EntityId, position: Vec3) {
        let ctx = script_ctx(&self.app);
        let mut registry = ctx.registry.borrow_mut();
        let transform = Transform {
            position,
            ..*registry.get_component::<Transform>(pawn).unwrap()
        };
        registry.set_component(pawn, transform).unwrap();
    }

    pub(super) fn eye(&self, pawn: EntityId) -> Vec3 {
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

    /// The pawn's eye is within 1 cm of the hinted eye and its spawn rotation
    /// carries the hinted yaw and pitch.
    pub(super) fn assert_pawn_at_hint(&self, pawn: EntityId, hint: &SpawnPoseHint) {
        let eye = self.eye(pawn);
        assert!(
            eye.distance(Vec3::from_array(hint.eye)) < 0.01,
            "pawn eye {eye:?} is within 1 cm of the hinted eye {:?}",
            hint.eye
        );
        let expected = Quat::from_euler(EulerRot::YXZ, hint.yaw, hint.pitch, 0.0);
        assert!(
            self.transform(pawn).rotation.angle_between(expected) < 1e-4,
            "spawn rotation carries the hinted yaw {} and pitch {}",
            hint.yaw,
            hint.pitch
        );
    }

    /// Flip the host's mod digest away and back: every participating slot is
    /// demoted, then re-promoted with no reload and no fresh reveal.
    pub(super) fn demote_by_mod_digest(&mut self, participants: &[u64]) {
        self.server().set_mod_digest(Some([0xEE; 32]));
        self.pump_until(|relay| participants.iter().all(|&id| !relay.participating(id)));
    }

    pub(super) fn restore_mod_digest(&mut self) {
        let digest = self.mod_digest;
        self.server().set_mod_digest(Some(digest));
    }
}
