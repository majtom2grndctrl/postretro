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

use crate::ghost_links::bus::GhostBus;
use crate::ghost_links::clocks::{ClockSource, MemberLane, PoseStamps, PresentationClocks};
use crate::ghost_links::eligibility::CandidateView;
use crate::ghost_links::opt_ins_test_fixtures::{data_registry, descriptor};
use crate::ghost_links::{GhostFrame, GhostLinks, GhostLook, GhostMember};
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

/// The barrier's level-stable mover id, when a scenario places it.
pub(super) const BARRIER_MOVER_ID: u32 = 1;
/// Where the barrier rests in copy A, and where it opens to: both inside link
/// 0's volume, so its ghost stays a member while it moves.
pub(super) const BARRIER_CLOSED: Vec3 = Vec3::new(4.0, -2.0, 2.0);
pub(super) const BARRIER_OPEN: Vec3 = Vec3::new(4.0, 2.0, 2.0);

/// The barrier: a tagged mover in copy A, closed until started.
fn barrier() -> (
    postretro_level_loader::LoadedKinematicMover,
    Vec<postretro_level_loader::LoadedKinematicWaypoint>,
) {
    let mut mover = crate::runtime_movers::tests::mover(1);
    mover.mover_id = BARRIER_MOVER_ID;
    mover.name = "barrier".into();
    mover.tags = vec!["barrier".into()];
    mover.origin = BARRIER_CLOSED;
    mover.path = "barrier_closed".into();
    mover.speed_mps = 4.0;
    mover.wait_ms = 0.0;
    mover.start_on_spawn = false;
    let waypoints = vec![
        postretro_level_loader::LoadedKinematicWaypoint {
            name: "barrier_closed".into(),
            next: "barrier_open".into(),
            origin: BARRIER_CLOSED,
        },
        postretro_level_loader::LoadedKinematicWaypoint {
            name: "barrier_open".into(),
            next: String::new(),
            origin: BARRIER_OPEN,
        },
    ];
    (mover, waypoints)
}

/// Link 0 covers copy A and maps it onto copy B; link 1 the reverse. Teleporter
/// 0 is a seamless pad in copy A arriving in copy B; teleporter 1 the reverse.
/// With `barrier`, copy A holds a tagged mover both links list.
pub(super) fn level(links_armed: bool, barrier: bool) -> LevelWorld {
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
                ghost_tags: if barrier {
                    vec!["barrier".into()]
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    if barrier {
        let (mover, waypoints) = self::barrier();
        world.kinematic_geometry.movers = vec![mover];
        world.kinematic_geometry.waypoints = waypoints;
    }
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
    /// This peer's barrier mover and where it presents it, when placed.
    pub barrier: Option<(EntityId, Vec3)>,
    /// Each link's look at host-now on this peer, before membership and the
    /// flash clamp.
    pub link_looks: [GhostLook; 2],
    /// The Ghost bus profile this peer resolves this frame, at its own script
    /// time: what it hands its audio bus.
    pub ghost_audio: postretro_foundation::GhostAudioValues,
}

impl PresentedFrame {
    /// The barrier's ghost through link `link` this frame.
    pub(super) fn barrier_member(&self, link: u16) -> Option<&GhostMember> {
        let (entity, _) = self.barrier?;
        self.members
            .iter()
            .find(|member| member.link == link && member.entity == entity)
    }

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
    /// Place the tagged barrier mover in copy A, loaded on both peers and
    /// replicated as PRL movers are.
    pub barrier: bool,
}

pub(super) struct Loopback {
    world: LevelWorld,
    server: NetServer,
    client: NetClient,
    up: PacketConditioner,
    down: PacketConditioner,
    // Host.
    host: ScriptCtx,
    /// The host's registered ghost-link member steps, as level scripts call them.
    host_steps: postretro_scripting_core::sequence::SequencedPrimitiveRegistry,
    host_ghosts: GhostLinks,
    /// The host's Ghost bus: `setGhostAudio` applies here at each tick's start.
    pub host_bus: GhostBus,
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
    /// Motion each host pawn makes during the next tick ([`Self::displace_pawn`]).
    pending_displacement: [Vec3; 2],
    links: Vec<EntityId>,
    teleporters: Vec<EntityId>,
    /// Each peer's barrier entity, when placed, and its mover tick state.
    host_barrier: Option<EntityId>,
    host_mover_ticks: postretro_physics::kinematic_mover::MoverTickStateTable,
    client_barrier: Option<EntityId>,
    client_mover_ticks: postretro_physics::kinematic_mover::MoverTickStateTable,
    // Client.
    client_ctx: ScriptCtx,
    client_replication: netcode::ClientReplication,
    client_state: netcode::ClientStateApply,
    prediction: netcode::ClientPrediction,
    client_collision: crate::collision::CollisionWorld,
    volumes: postretro_collision::EnvironmentVolumes,
    hit_zones: crate::scripting_systems::hit_zones::HitZoneStore,
    pub client_ghosts: GhostLinks,
    /// The client's Ghost bus: its own manifest under the host's received fields.
    pub client_bus: GhostBus,
    /// Every ghost-audio record the client received, in apply order.
    pub client_audio_received: Vec<postretro_net::wire::WireGhostAudioState>,
    /// The smoothed RTT the client's time sync reports; `None` before its
    /// first echo, when the presentation clock cannot anchor.
    pub client_rtt_micros: Option<f64>,
    /// Every link record the client received, in apply order.
    pub client_received: Vec<netcode::GhostLinkUpdate>,
    /// Every presented frame, in order.
    pub host_frames: Vec<PresentedFrame>,
    pub client_frames: Vec<PresentedFrame>,
}

impl Loopback {
    pub(super) fn new(scenario: Scenario) -> Self {
        let world = level(scenario.links_armed, scenario.barrier);
        let client_collision = crate::collision::CollisionWorld::from_level(&world).unwrap();

        let host = ScriptCtx::new();
        let (pawns, links, teleporters, host_ghosts, collision, host_barrier) = {
            let mut registry = host.registry.borrow_mut();
            let host_barrier = spawn_barrier(&mut registry, &world);
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
                host_barrier,
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
            netcode::host_register_loaded_movers(
                &registry,
                &mut allocator,
                &mut replicable,
                &mut HashSet::new(),
            );
        }

        let client_ctx = ScriptCtx::new();
        client_ctx.data_registry.borrow_mut().entities = vec![descriptor("player", true)];
        let (client_ghosts, client_barrier) = {
            let mut registry = client_ctx.registry.borrow_mut();
            let client_barrier = spawn_barrier(&mut registry, &world);
            crate::ghost_links::spawn_loaded_ghost_links(&mut registry, &world);
            crate::runtime_teleporters::spawn_loaded_teleporters(&mut registry, &world);
            (install_ghosts(&world, &registry), client_barrier)
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

        let mut host_steps = postretro_scripting_core::sequence::SequencedPrimitiveRegistry::new();
        postretro_sim::ghost_link_commands::register_sequenced_ghost_link_primitives(
            &mut host_steps,
            host.clone(),
        );

        Self {
            world,
            server,
            client,
            host_steps,
            up: PacketConditioner::new(link(0x6057)),
            down: PacketConditioner::new(link(0x6058)),
            client_collision,
            host,
            host_ghosts,
            host_bus: GhostBus::default(),
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
            pending_displacement: [Vec3::ZERO; 2],
            links,
            teleporters,
            host_barrier,
            host_mover_ticks: Default::default(),
            client_barrier,
            client_mover_ticks: Default::default(),
            client_ctx,
            client_replication: Default::default(),
            client_state: Default::default(),
            prediction: Default::default(),
            volumes: Default::default(),
            hit_zones: Default::default(),
            client_ghosts,
            client_bus: GhostBus::default(),
            client_audio_received: Vec::new(),
            client_rtt_micros: Some(120_000.0),
            client_received: Vec::new(),
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

    /// Name `style` on every link and bind each peer to its own style table,
    /// as each resolves its own manifest at install.
    pub(super) fn bind_styles(
        &mut self,
        style: &str,
        host: &postretro_foundation::GhostStyleRegistry,
        client: &postretro_foundation::GhostStyleRegistry,
    ) {
        for link in &mut self.world.ghost_links.links {
            link.style = style.to_string();
        }
        self.host_ghosts.bind_styles(&self.world, host);
        self.client_ghosts.bind_styles(&self.world, client);
    }

    /// A committed staged reload on one peer replaced its style table.
    pub(super) fn reload_styles(
        &mut self,
        host: Option<&postretro_foundation::GhostStyleRegistry>,
        client: Option<&postretro_foundation::GhostStyleRegistry>,
    ) {
        if let Some(styles) = host {
            self.host_ghosts.rebind_styles(styles);
        }
        if let Some(styles) = client {
            self.client_ghosts.rebind_styles(styles);
        }
    }

    /// Run one ghost-link member step on link `link` through the host's
    /// registered primitive, as a level script's reaction does. It queues and
    /// applies at the start of the next host tick.
    pub(super) fn host_step(&self, link: usize, primitive: &str, args: serde_json::Value) {
        self.host_steps.get(primitive).expect("registered")(self.links[link], &args)
            .expect("a valid step");
    }

    /// Run one `setGhostAudio` on the host, as its reaction drain queues it:
    /// parsed as the primitive parses it, applied at the next tick's start.
    pub(super) fn host_set_ghost_audio(&mut self, args: serde_json::Value) {
        let targets = postretro_sim::ghost_audio_command::parse_set_ghost_audio(&args)
            .expect("a valid setGhostAudio");
        self.host_bus.queue(targets);
    }

    /// Move host pawn `index` by `delta` during the next tick, as its
    /// movement would: every peer presents the motion toward that tick's stamp.
    pub(super) fn displace_pawn(&mut self, index: usize, delta: Vec3) {
        self.pending_displacement[index] += delta;
    }

    /// Run `ticks` host ticks with nothing reaching the client: a peer that
    /// has not joined yet. Its snapshots are dropped unsent.
    pub(super) fn run_host_only(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.host_tick();
            let _ = self.server.packets_to_send(CLIENT_A);
            self.present_host();
        }
    }

    /// Host script time at fractional tick `alpha` past the latest completed
    /// tick: a frame's script time, which also stamps every command the tick
    /// at its start applied.
    pub(super) fn host_script_time(&self, alpha: f32) -> f64 {
        (f64::from(self.tick) + f64::from(alpha)) * f64::from(TICK_SECONDS)
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

    /// Start the barrier on the host, as a mover command applies at the start
    /// of the next tick: it travels from closed to open from that tick on.
    pub(super) fn start_barrier(&mut self) {
        let barrier = self.host_barrier.expect("scenario places the barrier");
        let mut registry = self.host.registry.borrow_mut();
        let mut mover = registry
            .get_component::<postretro_entities::KinematicMoverComponent>(barrier)
            .unwrap()
            .clone();
        mover.started = true;
        registry.set_component(barrier, mover).unwrap();
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
        // The frame this tick runs in: its script time, then the queued member
        // steps at the tick's start, before anything in the tick runs.
        self.host
            .presentation_script_time
            .set(f64::from(self.tick + 1) * f64::from(TICK_SECONDS));
        self.host_ghosts.apply_pending_commands(&self.host);
        self.host_bus
            .apply_pending(self.host.presentation_script_time.get());
        let mut registry = self.host.registry.borrow_mut();
        netcode::host_restore_client_pawn_authoritative_poses(
            &self.seat_buffer,
            &self.owners,
            &self.allocator,
            &mut registry,
        );
        registry.snapshot_transforms();
        for (pawn, delta) in self
            .pawns
            .iter()
            .zip(std::mem::take(&mut self.pending_displacement))
        {
            if delta != Vec3::ZERO {
                let mut transform = *registry.get_component::<Transform>(*pawn).unwrap();
                transform.position += delta;
                registry.set_component(*pawn, transform).unwrap();
            }
        }
        // The host's fixed tick steps its movers after the transform snapshot.
        postretro_physics::kinematic_mover::run_kinematic_mover_tick(
            &mut registry,
            &mut self.host_mover_ticks,
            TICK_SECONDS,
        );
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
            self.host_bus.script_fields(),
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
        // The client's fixed tick predicts its loaded movers, as production's
        // `client_predict_loaded_movers_tick` does, before applying snapshots.
        {
            let mut registry = self.client_ctx.registry.borrow_mut();
            if let Some(barrier) = self.client_barrier {
                registry.snapshot_transform(barrier);
            }
            postretro_physics::kinematic_mover::run_kinematic_mover_tick(
                &mut registry,
                &mut self.client_mover_ticks,
                TICK_SECONDS,
            );
        }
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
            // Movers re-simulate to the host tick the harness clock estimates.
            Some(self.tick),
            None,
            None,
            false,
            &mut NoBindings,
            f64::from(self.tick) * f64::from(TICK_SECONDS),
            self.client_rtt_micros,
        );
        // The binary's per-frame drain: every received state into link history,
        // look ramps rebased by the client's host-clock offset.
        let offset = super::host_clock_offset(
            &self.client_replication,
            f64::from(self.tick) * f64::from(TICK_SECONDS),
            self.client_rtt_micros,
        );
        let updates: Vec<_> = self.client_replication.drain_ghost_link_updates().collect();
        self.client_received.extend_from_slice(&updates);
        self.client_ghosts
            .intake_received(updates.into_iter(), offset);
        // The audio stage's drain: the newest ghost-audio record, rebased.
        let record = self.client_replication.take_ghost_audio();
        self.client_audio_received.extend(record);
        self.client_bus.intake_received(
            record.map(|state| netcode::ghost_audio_from_wire(&state)),
            offset,
        );
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
            self.present_host_frame(alpha);
            self.present_client_frame(alpha, client_entities);
        }
    }

    /// The host seat's frames for one tick, with no client frames.
    fn present_host(&mut self) {
        for frame in 0..FRAMES_PER_TICK {
            self.present_host_frame(frame as f32 / FRAMES_PER_TICK as f32);
        }
    }

    /// Host seat: the delayed client pawn, then membership.
    fn present_host_frame(&mut self, alpha: f32) {
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
        let script_time = self.host_script_time(alpha);
        self.host.presentation_script_time.set(script_time);
        self.host_ghosts.set_look_time(script_time);
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
        let barrier = presented_barrier(&registry, self.host_barrier, alpha);
        self.host_frames.push(PresentedFrame {
            members: self.host_ghosts.members().to_vec(),
            pawns: host_pawns,
            entities: self.pawns.map(Some),
            remote_clock: clocks.lane(MemberLane::Remote),
            barrier,
            link_looks: [0, 1].map(|link| self.host_ghosts.link_look(link)),
            ghost_audio: self.host_bus.resolve(script_time),
        });
    }

    /// Client: remotes sampled at the delayed render tick, then membership.
    /// Its script clock runs in step with the host's, as the harness clock does.
    fn present_client_frame(&mut self, alpha: f32, client_entities: [Option<EntityId>; 2]) {
        let render_tick = f64::from(self.tick) - CLIENT_RENDER_DELAY_TICKS + f64::from(alpha);
        let script_time = self.host_script_time(alpha);
        self.client_ctx.presentation_script_time.set(script_time);
        self.client_ghosts.set_look_time(script_time);
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
        let barrier = presented_barrier(&registry, self.client_barrier, alpha);
        self.client_frames.push(PresentedFrame {
            members: self.client_ghosts.members().to_vec(),
            pawns: client_pawns,
            entities: client_entities,
            remote_clock: clocks.lane(MemberLane::Remote),
            barrier,
            link_looks: [0, 1].map(|link| self.client_ghosts.link_look(link)),
            ghost_audio: self.client_bus.resolve(script_time),
        });
    }
}

/// Spawn the level's movers (the barrier, when placed) as every peer does at
/// install, from the level file.
fn spawn_barrier(registry: &mut EntityRegistry, world: &LevelWorld) -> Option<EntityId> {
    crate::runtime_movers::spawn_loaded_kinematic_movers(registry, world, 0.0)
        .unwrap()
        .first()
        .copied()
}

/// The barrier as this peer presents it this frame.
fn presented_barrier(
    registry: &EntityRegistry,
    barrier: Option<EntityId>,
    alpha: f32,
) -> Option<(EntityId, Vec3)> {
    let barrier = barrier?;
    let presented = registry.interpolated_transform(barrier, alpha).ok()?;
    Some((barrier, presented.position))
}
