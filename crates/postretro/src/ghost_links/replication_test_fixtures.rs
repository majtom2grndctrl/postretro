// Two-peer ghost-link loopback: a listen host and a connected client over the relay transport.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/entity_model.md §7d
//
// Both peers install the same level (two room copies, a link over each mapping
// it onto the other, a seamless teleporter pair joining them) and run the
// production seams in frame-loop order: host fixed tick (movement, host-seat pose
// record, link history), snapshot replication, a conditioned relay, client
// apply and the link-state drain, then several presented frames per tick. The
// host seat and the client each evaluate ghost membership every presented frame.

use std::collections::HashSet;
use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

use glam::Vec3;
use postretro_entities::presentation_binding::{
    PresentationBindingKey, PresentationBindingResolver,
};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    EntityId, EntityRegistry, GhostLinkComponent, ScriptCtx, TeleporterComponent, Transform,
};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord};
use postretro_level_loader::LevelWorld;
use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::replication::ServerReplication;
use postretro_net::transport::{NetClient, NetServer};
use postretro_physics::movement::MovementInput;

use crate::ghost_links::clocks::{ClockSource, MemberLane, PoseStamps, PresentationClocks};
use crate::ghost_links::eligibility::CandidateView;
use crate::ghost_links::opt_ins_test_fixtures::{data_registry, descriptor};
use crate::ghost_links::{GhostFrame, GhostLinks, GhostMember};
use crate::netcode;

/// The connected client whose view is measured.
pub(super) const CLIENT_A: u64 = 7;
/// Owner of the slot pawn the host seat presents through its delay buffer; it
/// never connects, so the host simulates its pawn on neutral input.
const CLIENT_B: u64 = 8;
pub(super) const FRAMES_PER_TICK: usize = 4;
const TICK_SECONDS: f32 = 1.0 / 60.0;
pub(super) const FRAME_SECONDS: f32 = TICK_SECONDS / FRAMES_PER_TICK as f32;
/// The client presents remotes this far behind the host's tick: the conditioned
/// link's one-way delay and jitter plus snapshot spacing, with margin, the role
/// the adaptive interpolation delay plays in production.
const CLIENT_RENDER_DELAY_TICKS: f64 = 9.0;
const FINGERPRINT: [u8; 32] = [0x6b; 32];

/// Copy A spans x in [-10, 10]; copy B is the same room at x + 100.
pub(super) const COPY_OFFSET: f32 = 100.0;
const COPY_HALF_EXTENT: f32 = 10.0;

fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            let mut negative = [0.0; 3];
            negative[axis] = -1.0;
            [
                EnvironmentPlane {
                    normal,
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: negative,
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// The room copy centered at `x`.
pub(super) fn copy_volume(x: f32) -> (Vec3, Vec3) {
    (
        Vec3::new(x - COPY_HALF_EXTENT, -5.0, -5.0),
        Vec3::new(x + COPY_HALF_EXTENT, 5.0, 5.0),
    )
}

pub(super) fn inside(volume: (Vec3, Vec3), point: Vec3) -> bool {
    point.cmpge(volume.0).all() && point.cmple(volume.1).all()
}

/// Link 0 covers copy A and maps it onto copy B; link 1 the reverse. Teleporter
/// 0 is a seamless pad in copy A arriving in copy B; teleporter 1 the reverse.
pub(super) fn level(links_armed: bool) -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![postretro_level_loader::CellData {
            bounds_min: Vec3::splat(-500.0),
            bounds_max: Vec3::splat(500.0),
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }],
        Vec::new(),
        postretro_level_loader::CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    let copy = |index: u32| index as f32 * COPY_OFFSET;
    let other = |index: u32| (1 - index) as f32 * COPY_OFFSET;
    world.ghost_links.links = (0..2)
        .map(|index| {
            let (min, max) = copy_volume(copy(index));
            GhostLinkRecord {
                source_entity: 20 + index,
                enabled_on_spawn: links_armed,
                members: GhostMembers::all(),
                style: String::new(),
                brushes: vec![box_brush(min, max)],
                landmark_origin: [copy(index), 0.0, 0.0],
                landmark_yaw: 0.0,
                target_origin: [other(index), 0.0, 0.0],
                target_yaw: 0.0,
                ghost_tags: Vec::new(),
            }
        })
        .collect();
    world.teleporters.teleporters = (0..2)
        .map(|index| TeleporterRecord {
            source_entity: 30 + index,
            mode: TeleportMode::Seamless,
            enabled_on_spawn: false,
            brushes: vec![box_brush(
                Vec3::new(copy(index) - 3.0, -5.0, -3.0),
                Vec3::new(copy(index) + 3.0, 5.0, 3.0),
            )],
            destination_origin: [other(index), 0.0, 0.0],
            destination_yaw: 0.0,
            landmark_origin: [copy(index), 0.0, 0.0],
            landmark_yaw: 0.0,
            arrival_sound: String::new(),
            on_teleport: String::new(),
        })
        .collect();
    world
}

/// Every role reads `ghost` from its own descriptors: the player opts in.
pub(super) fn install_ghosts(world: &LevelWorld, registry: &EntityRegistry) -> GhostLinks {
    let mut ghosts = GhostLinks::install(world, registry);
    ghosts.refresh_opt_ins(&data_registry(vec![descriptor("player", true)]));
    ghosts
}

fn neutral_input() -> MovementInput {
    crate::build_sim_command(
        &crate::input::ActionSnapshot::neutral(),
        &crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0),
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .movement
}

pub(super) fn spawn_pawn(
    registry: &mut EntityRegistry,
    at: Vec3,
    spawn_path: DescriptorSpawnPath,
) -> EntityId {
    let pawn = registry.spawn(Transform {
        position: at,
        ..Default::default()
    });
    registry
        .set_component(
            pawn,
            postretro_foundation::PlayerMovementComponent::from_descriptor(
                &crate::mod_digest::tests::movement_descriptor(),
            ),
        )
        .unwrap();
    registry
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: "player".into(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path,
            },
        )
        .unwrap();
    pawn
}

/// One presented frame on one peer: the ghosts, and where each pawn of interest
/// was drawn.
#[derive(Debug, Clone)]
pub(super) struct PresentedFrame {
    pub members: Vec<GhostMember>,
    /// Presented positions of the host's pawns as this peer draws them, in the
    /// order [`Loopback::pawns`] lists them.
    pub pawns: [Option<Vec3>; 2],
    /// This peer's entity for each of [`Loopback::pawns`].
    pub entities: [Option<EntityId>; 2],
    /// The delay-buffered lane's clock this frame: what a remote member would
    /// read without its own pose stamp.
    pub remote_clock: f64,
}

impl PresentedFrame {
    pub(super) fn member(&self, link: u16, pawn: usize) -> Option<&GhostMember> {
        let entity = self.entities[pawn]?;
        self.members
            .iter()
            .find(|member| member.link == link && member.entity == entity)
    }
}

struct NoBindings;

impl PresentationBindingResolver for NoBindings {
    fn resolve(&self, _: &EntityRegistry, _: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        None
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        _: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        None
    }
}

/// Where the scenario's two host-simulated pawns start, and whether the links
/// start armed. Pawn 0 is the host seat's own pawn (never a ghost on the host
/// seat); pawn 1 is a connected slot's pawn the host seat presents delayed. The
/// measured client has no pawn of its own, so both are remotes it may ghost.
pub(super) struct Scenario {
    pub links_armed: bool,
    pub pawns: [Vec3; 2],
}

pub(super) struct Loopback {
    world: LevelWorld,
    server: NetServer,
    client: NetClient,
    up: PacketConditioner,
    down: PacketConditioner,
    // Host.
    host: ScriptCtx,
    host_ghosts: GhostLinks,
    collision: crate::collision::CollisionWorld,
    allocator: netcode::NetworkIdAllocator,
    replicable: netcode::ReplicableSet,
    replication: ServerReplication,
    host_state: netcode::HostStateReplication,
    owners: netcode::MovementOwners,
    weapons: netcode::WeaponOwners,
    queues: netcode::HostCommandQueues,
    pending_hits: netcode::PendingHitDeclarations,
    open_shots: netcode::OpenAuthorizedShots,
    seat_buffer: netcode::RemoteInterpolationBuffer,
    last_snapshot: Option<u32>,
    snapshot_due: bool,
    pub tick: u32,
    pub pawns: [EntityId; 2],
    links: Vec<EntityId>,
    teleporters: Vec<EntityId>,
    // Client.
    client_ctx: ScriptCtx,
    client_replication: netcode::ClientReplication,
    client_state: netcode::ClientStateApply,
    prediction: netcode::ClientPrediction,
    client_collision: crate::collision::CollisionWorld,
    volumes: postretro_collision::EnvironmentVolumes,
    hit_zones: crate::scripting_systems::hit_zones::HitZoneStore,
    pub client_ghosts: GhostLinks,
    /// Every presented frame, in order.
    pub host_frames: Vec<PresentedFrame>,
    pub client_frames: Vec<PresentedFrame>,
}

impl Loopback {
    pub(super) fn new(scenario: Scenario) -> Self {
        let world = level(scenario.links_armed);
        let client_collision = crate::collision::CollisionWorld::from_level(&world).unwrap();

        let host = ScriptCtx::new();
        let (pawns, links, teleporters, host_ghosts, collision) = {
            let mut registry = host.registry.borrow_mut();
            crate::ghost_links::spawn_loaded_ghost_links(&mut registry, &world);
            crate::runtime_teleporters::spawn_loaded_teleporters(&mut registry, &world);
            let mut collision = crate::collision::CollisionWorld::from_level(&world).unwrap();
            crate::runtime_teleporters::bind_loaded_teleporters(
                &mut collision.teleporters,
                &registry,
            );
            let host_pawn = spawn_pawn(
                &mut registry,
                scenario.pawns[0],
                DescriptorSpawnPath::PlayerSpawn,
            );
            registry.mark_local_player_pawn(host_pawn).unwrap();
            let slot_pawn = spawn_pawn(
                &mut registry,
                scenario.pawns[1],
                DescriptorSpawnPath::NetworkSlot,
            );
            let mut links: Vec<_> = registry
                .iter_with_kind(postretro_entities::ComponentKind::GhostLink)
                .map(|(id, _)| {
                    let index = registry
                        .get_component::<GhostLinkComponent>(id)
                        .unwrap()
                        .index;
                    (index, id)
                })
                .collect();
            links.sort();
            let mut teleporters: Vec<_> = registry
                .iter_with_kind(postretro_entities::ComponentKind::Teleporter)
                .map(|(id, _)| {
                    let index = registry
                        .get_component::<TeleporterComponent>(id)
                        .unwrap()
                        .index;
                    (index, id)
                })
                .collect();
            teleporters.sort();
            let ghosts = install_ghosts(&world, &registry);
            (
                [host_pawn, slot_pawn],
                links.into_iter().map(|(_, id)| id).collect(),
                teleporters.into_iter().map(|(_, id)| id).collect(),
                ghosts,
                collision,
            )
        };

        let mut allocator = netcode::NetworkIdAllocator::for_test();
        let mut replicable = netcode::ReplicableSet::default();
        let mut weapons = netcode::WeaponOwners::default();
        let mut owners = netcode::MovementOwners::default();
        owners.set_for_test(pawns[1], CLIENT_B);
        for pawn in pawns {
            netcode::host_register_own_pawn(
                &mut allocator,
                &mut replicable,
                &mut None,
                &mut weapons,
                pawn,
            );
        }
        {
            let registry = host.registry.borrow();
            netcode::host_register_loaded_teleporters(
                &registry,
                &mut allocator,
                &mut replicable,
                &mut HashSet::new(),
            );
            netcode::host_register_loaded_ghost_links(
                &registry,
                &mut allocator,
                &mut replicable,
                &mut HashSet::new(),
            );
        }

        let client_ctx = ScriptCtx::new();
        client_ctx.data_registry.borrow_mut().entities = vec![descriptor("player", true)];
        let client_ghosts = {
            let mut registry = client_ctx.registry.borrow_mut();
            crate::ghost_links::spawn_loaded_ghost_links(&mut registry, &world);
            crate::runtime_teleporters::spawn_loaded_teleporters(&mut registry, &world);
            install_ghosts(&world, &registry)
        };

        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server = NetServer::new(
            socket,
            address,
            8,
            Duration::from_secs(1),
            Some(FINGERPRINT),
        )
        .unwrap();
        server.set_mod_identity("ghost.fixture".into(), "1".into());
        server.set_mod_digest(Some(FINGERPRINT));
        server.set_level_parity(Some(("ghosts".into(), FINGERPRINT)));
        let mut client = NetClient::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            address,
            CLIENT_A,
            Duration::from_secs(1),
            Some(FINGERPRINT),
            None,
        )
        .unwrap();
        client.set_mod_identity("ghost.fixture".into(), "1".into());
        client.set_mod_digest(Some(FINGERPRINT));
        client.set_level_parity(Some(("ghosts".into(), FINGERPRINT)));
        postretro_net::harness::reveal_both(&mut server, &mut client, "ghosts");
        server.add_relay_connection(CLIENT_A, None);
        client.set_connected();
        // The harness profile's clean co-op link: 60 ms one way with up to 10 ms
        // of jitter, seeded so every run sees the same arrivals.
        let link = |seed| LinkConfig {
            delay: 60,
            jitter: 10,
            loss_probability: 0.0,
            seed,
        };

        Self {
            world,
            server,
            client,
            up: PacketConditioner::new(link(0x6057)),
            down: PacketConditioner::new(link(0x6058)),
            client_collision,
            host,
            host_ghosts,
            collision,
            allocator,
            replicable,
            replication: ServerReplication::new(),
            host_state: Default::default(),
            owners,
            weapons,
            queues: Default::default(),
            pending_hits: Default::default(),
            open_shots: Default::default(),
            seat_buffer: Default::default(),
            last_snapshot: None,
            snapshot_due: false,
            tick: 0,
            pawns,
            links,
            teleporters,
            client_ctx,
            client_replication: Default::default(),
            client_state: Default::default(),
            prediction: Default::default(),
            volumes: Default::default(),
            hit_zones: Default::default(),
            client_ghosts,
            host_frames: Vec::new(),
            client_frames: Vec::new(),
        }
    }

    /// Link `link`'s volume, for asserting where its ghosts' source poses lie.
    pub(super) fn link_volume(&self, link: u16) -> (Vec3, Vec3) {
        let record = &self.world.ghost_links.links[usize::from(link)];
        let brush = &record.brushes[0];
        (Vec3::from(brush.aabb_min), Vec3::from(brush.aabb_max))
    }

    /// Set link `index`'s armed state on the host, as a host-tick primitive
    /// applies it, before the next tick's movement.
    pub(super) fn set_link_armed(&mut self, index: usize, armed: bool) {
        let mut registry = self.host.registry.borrow_mut();
        let mut link = *registry
            .get_component::<GhostLinkComponent>(self.links[index])
            .unwrap();
        link.armed = armed;
        registry.set_component(self.links[index], link).unwrap();
    }

    /// Arm teleporter `index` on the host; the next movement tick fires every
    /// occupant, as a script `arm` does at the start of the next tick.
    pub(super) fn arm_teleporter(&mut self, index: usize) {
        let mut registry = self.host.registry.borrow_mut();
        let mut teleporter = registry
            .get_component::<TeleporterComponent>(self.teleporters[index])
            .unwrap()
            .clone();
        teleporter.armed = true;
        registry
            .set_component(self.teleporters[index], teleporter)
            .unwrap();
    }

    /// Run `ticks` host ticks, each followed by replication, the relay, client
    /// apply, and [`FRAMES_PER_TICK`] presented frames on both peers.
    pub(super) fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.host_tick();
            self.relay();
            self.client_receive();
            self.present();
        }
    }

    fn host_tick(&mut self) {
        let mut registry = self.host.registry.borrow_mut();
        netcode::host_restore_client_pawn_authoritative_poses(
            &self.seat_buffer,
            &self.owners,
            &self.allocator,
            &mut registry,
        );
        registry.snapshot_transforms();
        let input = neutral_input();
        let _ = postretro_sim::sim::run_host_movement_tick(
            &mut registry,
            &self.collision,
            postretro_collision::EnvironmentView::level_only(0.0),
            &[(self.pawns[0], input.clone()), (self.pawns[1], input)],
            TICK_SECONDS,
        );
        netcode::host_record_client_pawn_poses(
            &mut self.seat_buffer,
            &self.owners,
            &self.allocator,
            &registry,
            self.tick,
        );
        let _ = netcode::host_take_client_pawn_teleports(
            &mut self.seat_buffer,
            &self.owners,
            &self.allocator,
        );
        netcode::complete_host_fixed_tick(&mut self.tick, &mut self.snapshot_due);
        self.host_ghosts
            .record_after_fixed_tick(&registry, Some(self.tick));
        netcode::host_replicate(
            &registry,
            &self.host.slot_table.borrow(),
            &self.host.faction_sentiment.borrow(),
            &Default::default(),
            &mut self.server,
            &mut self.allocator,
            &mut self.replication,
            &mut self.host_state,
            &self.replicable,
            &self.owners,
            &self.weapons,
            &self.queues,
            None,
            self.tick,
            f64::from(self.tick) * f64::from(TICK_SECONDS),
            self.snapshot_due,
            &mut self.last_snapshot,
            &self.host.presentation_overrides.borrow(),
        );
        self.snapshot_due = false;
    }

    fn relay(&mut self) {
        let step = Duration::from_secs_f32(TICK_SECONDS);
        self.server.update_connections(step);
        self.client.update_connections(step);
        self.up.enqueue_all(self.client.packets_to_send());
        self.down.enqueue_all(self.server.packets_to_send(CLIENT_A));
        self.up.advance(17);
        self.down.advance(17);
        for packet in self.up.take_ready() {
            self.server.process_packet_from(&packet, CLIENT_A);
        }
        for packet in self.down.take_ready() {
            self.client.process_packet(&packet);
        }
        let _ = self.server.poll_handshakes();
        netcode::host_handle_client_messages(
            &mut self.server,
            &mut self.replication,
            &mut self.host_state,
            &mut self.queues,
            &self.owners,
            &mut self.pending_hits,
            &mut self.open_shots,
            CLIENT_A,
            self.tick,
            u64::from(self.tick) * 16_667,
        );
    }

    fn client_receive(&mut self) {
        let _ = self.client.drain_control();
        let ctx = &self.client_ctx;
        let _ = netcode::client_receive_and_apply(
            &mut ctx.registry.borrow_mut(),
            &mut ctx.slot_table.borrow_mut(),
            &postretro_entities::FactionRegistry::default(),
            ctx.faction_sentiment.as_ref(),
            &Default::default(),
            &mut self.client,
            &mut self.client_replication,
            &mut self.client_state,
            &mut self.prediction,
            &ctx.data_registry.borrow().entities,
            &self.hit_zones,
            None,
            &self.client_collision,
            &self.volumes,
            TICK_SECONDS,
            Duration::from_secs_f32(TICK_SECONDS),
            None,
            None,
            None,
            false,
            &mut NoBindings,
            f64::from(self.tick) * f64::from(TICK_SECONDS),
            Some(120_000.0),
        );
        // The binary's per-frame drain: every received state into link history.
        for update in self.client_replication.drain_ghost_link_updates() {
            self.client_ghosts.apply_received(update);
        }
    }

    /// The client's entity for host pawn `index`, once replicated.
    fn client_entity(&mut self, index: usize) -> Option<EntityId> {
        let network_id = self.allocator.stamp(self.pawns[index]);
        self.client_replication
            .entity_network_ids()
            .into_iter()
            .find_map(|(entity, id)| (id == network_id).then_some(entity))
    }

    fn present(&mut self) {
        let client_entities = [self.client_entity(0), self.client_entity(1)];
        for frame in 0..FRAMES_PER_TICK {
            let alpha = frame as f32 / FRAMES_PER_TICK as f32;
            // Host seat: the delayed client pawn, then membership.
            {
                let mut registry = self.host.registry.borrow_mut();
                netcode::host_present_client_pawns(
                    &self.seat_buffer,
                    &self.owners,
                    &self.allocator,
                    &mut registry,
                    self.tick,
                    alpha,
                );
            }
            let registry = self.host.registry.borrow();
            let clocks = PresentationClocks::compute(ClockSource::Host { tick: self.tick }, alpha);
            self.host_ghosts.update(
                &registry,
                GhostFrame {
                    clocks,
                    alpha,
                    elapsed_seconds: FRAME_SECONDS,
                    view: CandidateView::Authoritative,
                    stamps: PoseStamps::HostSeat {
                        buffer: &self.seat_buffer,
                        allocator: &self.allocator,
                        tick: self.tick,
                        alpha,
                    },
                },
            );
            let host_pawns = self.pawns.map(|pawn| {
                registry
                    .interpolated_transform(pawn, alpha)
                    .ok()
                    .map(|t| t.position)
            });
            self.host_frames.push(PresentedFrame {
                members: self.host_ghosts.members().to_vec(),
                pawns: host_pawns,
                entities: self.pawns.map(Some),
                remote_clock: clocks.lane(MemberLane::Remote),
            });
            drop(registry);

            // Client: remotes sampled at the delayed render tick, then membership.
            let render_tick = f64::from(self.tick) - CLIENT_RENDER_DELAY_TICKS + f64::from(alpha);
            let mut registry = self.client_ctx.registry.borrow_mut();
            let _ = netcode::client_sample_interpolation_at(
                &mut registry,
                &mut self.client_replication,
                render_tick,
                0.0,
            );
            let clocks = PresentationClocks::compute(
                ClockSource::Client {
                    render_server_tick: Some(render_tick),
                    estimated_server_tick: Some(f64::from(self.tick) + f64::from(alpha)),
                    latest_snapshot_tick: self.client_replication.latest_server_tick(),
                },
                alpha,
            );
            self.client_ghosts.update(
                &registry,
                GhostFrame {
                    clocks,
                    alpha,
                    elapsed_seconds: FRAME_SECONDS,
                    view: CandidateView::Client(&self.client_replication),
                    stamps: PoseStamps::Client(&self.client_replication),
                },
            );
            let client_pawns = client_entities.map(|entity| {
                entity.and_then(|entity| {
                    registry
                        .interpolated_transform(entity, alpha)
                        .ok()
                        .map(|t| t.position)
                })
            });
            self.client_frames.push(PresentedFrame {
                members: self.client_ghosts.members().to_vec(),
                pawns: client_pawns,
                entities: client_entities,
                remote_clock: clocks.lane(MemberLane::Remote),
            });
        }
    }
}
