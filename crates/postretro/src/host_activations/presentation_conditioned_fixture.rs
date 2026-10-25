// Shared CPU fixture for the conditioned presentation scenarios.
use crate::startup::lifecycle::tests::{level_world, script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};
use crate::{App, netcode, scripting_systems};
use glam::Vec3;
use postretro_entities::components::{
    billboard_emitter::BillboardEmitterComponent, light::LightComponent, mesh::MeshComponent,
};
use postretro_entities::{
    ComponentKind, EntityId, EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor,
    ReactionDescriptor, ScriptCtx, SequenceStep, SequenceTarget, SystemReactionCommand, Transform,
};
use postretro_foundation::Seat;
use postretro_level_format::{map_entity::MapEntityRecord, trigger_volumes::TriggerVolumeRecord};
use postretro_net::{
    harness::{LinkConfig, PacketConditioner},
    replication::ServerReplication,
    transport::{NetClient, NetServer},
    wire,
};
use postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences;
use postretro_sim::{
    player_events::PlayerEventTable,
    residual_drain::{ResidualRegistries, drain_frame_residuals},
    trigger_bindings::TriggerBindingTable,
    trigger_commands::TriggerFireContext,
    trigger_system::{
        AuthoritativePlayer, PlayerId, TriggerDispatchInputs, TriggerSystem, TriggerTickInputs,
    },
};
use scripting_systems::presentation_bindings::{PresentationBindings, PresentationBindingsMut};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, UdpSocket},
    time::Duration,
};

const DT: f64 = 1.0 / 60.0;
const RTT: f64 = 160_000.0;
const CLIENT_A: u64 = 7;
const CLIENT_B: u64 = 8;
const MODEL: &str = "models/decraniated_low_poly_retro_pixel/scene.gltf";
const ANIMATED_MODEL: &str = "models/reference_enemy_kaykit_knight/scene.gltf";

fn emitter() -> BillboardEmitterComponent {
    BillboardEmitterComponent {
        rate: 0.0,
        burst: None,
        spread: 0.0,
        lifetime: 1.0,
        velocity: [0.0; 3],
        buoyancy: 0.0,
        drag: 0.0,
        size_over_lifetime: [1.0, 1.0].into(),
        opacity_over_lifetime: [1.0, 0.0].into(),
        color: [1.0; 3],
        sprite: "fixture.png".into(),
        spin_rate: 0.0,
        spin_animation: None,
    }
}
fn descriptors() -> Vec<EntityTypeDescriptor> {
    use crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor;
    let mut mesh = mesh_descriptor("mesh_prop", true);
    let animated_mesh = mesh.mesh.as_mut().unwrap();
    animated_mesh.model = ANIMATED_MODEL.into();
    animated_mesh.animations.get_mut("idle").unwrap().clip = "Idle".into();
    animated_mesh.animations.get_mut("attack").unwrap().clip = "Walking_A".into();
    let mut lit = mesh.clone();
    lit.canonical_name = Some("lit_prop".into());
    lit.light = Some(postretro_entities::LightDescriptor {
        color: [1.0; 3],
        intensity: 1.0,
        range: 8.0,
        is_dynamic: true,
    });
    lit.emitter = Some(emitter());
    let mut light = lit.clone();
    light.canonical_name = Some("light_only".into());
    light.mesh = None;
    light.emitter = None;
    let mut particles = light.clone();
    particles.canonical_name = Some("emitter_only".into());
    particles.light = None;
    particles.emitter = Some(emitter());
    let mut dummy = mesh.clone();
    dummy.canonical_name = Some("target_dummy".into());
    dummy.mesh.as_mut().unwrap().model = MODEL.into();
    dummy.mesh.as_mut().unwrap().animations.clear();
    dummy.mesh.as_mut().unwrap().default_state = None;
    dummy.health = Some(postretro_entities::HealthDescriptor {
        max: 48.0,
        hitbox: Some(postretro_entities::HitboxDescriptor {
            half_extents: [0.5, 1.0, 0.5],
            offset: None,
        }),
        zone_multipliers: Default::default(),
    });
    vec![mesh, lit, light, particles, dummy]
}
fn fixture_world() -> postretro_level_loader::LevelWorld {
    let mut world = level_world("conditioned_presentation", 1);
    world.lights = vec![map_light(), map_light()];
    world.lights[0].tags = vec!["prl".into()];
    world.lights[1].tags = vec!["loop".into()];
    world.fog_volumes = vec![fog_record()];
    world.fog_volumes[0].tags = vec!["fog".into()];
    world.map_entities = [
        "billboard_emitter",
        "light_only",
        "emitter_only",
        "lit_prop",
        "mesh_prop",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, classname)| MapEntityRecord {
        classname: classname.into(),
        origin: [index as f32 * 4.0, 0.0, 0.0],
        tags: vec![classname.into()],
        ..Default::default()
    })
    .collect();
    world.trigger_volumes = vec![
        trigger("plate", 100.0, "plate"),
        trigger("changes", 200.0, "changes"),
    ];
    world
}
fn trigger(name: &str, x: f32, fire: &str) -> TriggerVolumeRecord {
    TriggerVolumeRecord {
        name: name.into(),
        tags: vec![name.into()],
        aabb_min: [x - 1.0, -1.0, -1.0],
        aabb_max: [x + 1.0, 3.0, 1.0],
        activation: 0,
        target_tag: String::new(),
        command: 0,
        command_arg: String::new(),
        fire_mode: 1,
        rearm_ms: 0.0,
        enabled_on_spawn: true,
        on_fire: fire.into(),
        on_exit: String::new(),
    }
}
fn primitive(name: &str, verb: &str, tag: Option<&str>, args: Value) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: verb.into(),
            tag: tag.map(str::to_owned),
            target: None,
            kind: None,
            on_complete: None,
            args,
        }),
    }
}
fn sequence(name: &str, id: EntityId, verb: &str, args: Value) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::Entity(id),
            primitive: verb.into(),
            args,
        }]),
    }
}
fn tagged(ctx: &ScriptCtx, tag: &str) -> EntityId {
    ctx.registry
        .borrow()
        .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
        .map(|(id, _)| id)
        .next()
        .expect(tag)
}
struct Peer {
    app: App,
    client: NetClient,
    replication: netcode::ClientReplication,
    state: netcode::ClientStateApply,
    prediction: netcode::ClientPrediction,
    up: PacketConditioner,
    down: PacketConditioner,
    id: u64,
    active: bool,
    commands: Vec<SystemReactionCommand>,
    rtt: Option<f64>,
    clock: f64,
    clock_origin: f64,
}
impl Peer {
    fn new(
        address: std::net::SocketAddr,
        id: u64,
        world: &postretro_level_loader::LevelWorld,
        active: bool,
    ) -> Self {
        let app = installed(world, true);
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = NetClient::new(
            socket,
            address,
            id,
            Duration::from_secs(1),
            Some([0x6a; 32]),
            None,
        )
        .unwrap();
        client.set_mod_identity("presentation.fixture".into(), "1".into());
        client.set_mod_digest(Some([0x6a; 32]));
        client.set_level_parity(Some(("presentation".into(), [0x6a; 32])));
        client.set_connected();
        let link = LinkConfig {
            delay: 80,
            jitter: 0,
            loss_probability: 0.0,
            seed: id,
        };
        Self {
            app,
            client,
            replication: Default::default(),
            state: Default::default(),
            prediction: Default::default(),
            up: PacketConditioner::new(link),
            down: PacketConditioner::new(link),
            id,
            active,
            commands: Vec::new(),
            rtt: Some(RTT),
            clock: 0.0,
            clock_origin: 0.0,
        }
    }
    fn receive(&mut self, world: &postretro_level_loader::LevelWorld) {
        // Participation markers share the reliable control lane. Process them
        // before snapshot/presentation drains, as the production frame loop does.
        let _ = self.client.drain_control();
        let ctx = script_ctx(&self.app);
        ctx.presentation_script_time.set(self.clock);
        let session = self.app.session.as_mut().unwrap();
        let mut resolver = PresentationBindingsMut {
            lights: &mut session.light_bridge,
            fog: &mut session.fog_volume_bridge,
            emitters: &mut session.emitter_bridge,
            map_entities: &world.map_entities,
        };
        let factions = postretro_entities::FactionRegistry::default();
        let materialized = netcode::client_receive_and_apply(
            &mut ctx.registry.borrow_mut(),
            &mut ctx.slot_table.borrow_mut(),
            &factions,
            ctx.faction_sentiment.as_ref(),
            &Default::default(),
            &mut self.client,
            &mut self.replication,
            &mut self.state,
            &mut self.prediction,
            &ctx.data_registry.borrow().entities,
            &session.hit_zone_store,
            None,
            &self.app.collision_world,
            &self.app.environment_volumes,
            DT as f32,
            Duration::from_secs_f64(DT),
            None,
            None,
            None,
            false,
            &mut resolver,
            self.clock,
            self.rtt,
        );
        if materialized.materialized_remote_entity_presentation {
            crate::resolve_mesh_entity_bindings(
                &mut ctx.registry.borrow_mut(),
                &session.mesh_clip_tables,
                &session.hit_zone_store,
            );
        }
        let (commands, passive) = netcode::presentation_commands::split_presentation_commands(
            self.client.drain_presentation(),
        );
        assert!(passive.is_empty());
        netcode::presentation_commands::ingest_presentation_commands(
            commands,
            &ctx.system_commands,
        );
        self.commands.extend(ctx.system_commands.take());
    }
    fn sample(&mut self) -> Vec<f32> {
        sample(&mut self.app, self.clock)
    }
}
fn installed(world: &postretro_level_loader::LevelWorld, client: bool) -> App {
    installed_with_reactions(world, client, Vec::new())
}
fn installed_with_reactions(
    world: &postretro_level_loader::LevelWorld,
    client: bool,
    reactions: Vec<NamedReaction>,
) -> App {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.data_registry.borrow_mut().entities = descriptors();
    ctx.data_registry.borrow_mut().replace_global_reactions(
        reactions
            .into_iter()
            .map(|reaction| postretro_entities::ScopedReaction {
                reaction,
                levels: Vec::new(),
            })
            .collect(),
    );
    let session = app.session.as_mut().unwrap();
    crate::scripting::builtins::register_builtins(&mut session.classname_dispatch);
    crate::scripting::primitives::light::register_light_reaction_primitives(
        &mut session.scripting.reaction_registry,
    );
    postretro_sim::scripting::reactions::registry::register_emitter_reaction_primitives(
        &mut session.scripting.reaction_registry,
    );
    postretro_sim::scripting::reactions::registry::register_fog_reaction_primitives(
        &mut session.scripting.reaction_registry,
    );
    postretro_sim::scripting::reactions::registry::register_sequenced_fog_primitives(
        &mut session.scripting.sequence_registry,
        ctx.clone(),
    );
    crate::scripting::primitives::light::register_sequenced_light_primitives(
        &mut session.scripting.sequence_registry,
        ctx.clone(),
    );
    postretro_sim::scripting::reactions::system_commands::register_system_reaction_primitives(
        &mut session.scripting.system_registry,
    );
    postretro_sim::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
        &mut session.scripting.sequence_registry,
        session.scripting.scheduler.clone(),
    );
    session.scripting.scheduler.set_enabled(!client);
    session
        .light_bridge
        .populate_from_level(&world.lights, &mut ctx.registry.borrow_mut(), 0);
    let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    install_world_cpu(
        WorldInstallHandles {
            world,
            script_ctx: &ctx,
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: Default::default(),
            content_root: &content,
            active_level_tags: &[],
            nav_graph: None,
            fog_volume_bridge: &mut session.fog_volume_bridge,
            trigger_volume_bridge: &mut session.trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut session.modal_stack,
            progress_tracker: &mut session.progress_tracker,
            crossing_detector: &mut session.crossing_detector,
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
            suppress_host_replicated: client,
            suppress_boot_pawn: true,
            local_carried_loadout: None,
        },
        &mut crate::startup::StartupTimings::new(),
        |models, tables| {
            for model in models {
                let loaded = model.result.as_ref().expect("fixture shipped model parses");
                let metadata: Vec<_> = loaded
                    .clips
                    .iter()
                    .map(|clip| postretro_model::ClipMetadata {
                        name: clip.name.clone(),
                        duration: clip.duration,
                    })
                    .collect();
                tables.insert_with_bounds(
                    postretro_model::ModelHandle::from(model.handle.clone()),
                    &metadata,
                    loaded.mesh.bounds(),
                );
            }
            scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
        },
        |_| {},
    );
    crate::startup::lifecycle::start_installed_presentation_animations(
        &ctx.registry.borrow(),
        &mut session.light_bridge,
        &mut session.fog_volume_bridge,
        &mut session.emitter_bridge,
    );
    app.script_time = 0.0;
    app
}
fn sample(app: &mut App, clock: f64) -> Vec<f32> {
    sample_delta(app, clock, DT as f32)
}
fn sample_delta(app: &mut App, clock: f64, delta: f32) -> Vec<f32> {
    sample_deltas(app, clock, delta, delta)
}
fn sample_deltas(app: &mut App, clock: f64, delta: f32, spin_delta: f32) -> Vec<f32> {
    let ctx = script_ctx(app);
    let session = app.session.as_mut().unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let brightness = session
        .light_bridge
        .update(&mut registry, clock as f32, 0.0)
        .map(|u| u.effective_brightness.to_vec())
        .unwrap_or_default();
    session.fog_volume_bridge.tick(&mut registry, clock);
    session.emitter_bridge.update(
        &mut registry,
        delta,
        spin_delta,
        clock as f32,
        &mut session.particle_pool,
        &mut session.particle_render,
    );
    brightness
}
struct Harness {
    server: NetServer,
    host: App,
    peers: Vec<Peer>,
    world: postretro_level_loader::LevelWorld,
    allocator: netcode::NetworkIdAllocator,
    set: netcode::ReplicableSet,
    tracked: HashSet<EntityId>,
    replication: ServerReplication,
    state: netcode::HostStateReplication,
    owners: netcode::MovementOwners,
    weapons: netcode::WeaponOwners,
    queues: netcode::HostCommandQueues,
    pending_hits: netcode::PendingHitDeclarations,
    open_shots: netcode::OpenAuthorizedShots,
    seats: netcode::SeatTable,
    last: Option<u32>,
    tick: u32,
    clock: f64,
    system: TriggerSystem,
    bindings: TriggerBindingTable,
    players: Vec<AuthoritativePlayer>,
    host_commands: Vec<SystemReactionCommand>,
}
impl Harness {
    fn new() -> Self {
        Self::with_world(fixture_world())
    }
    fn with_world(world: postretro_level_loader::LevelWorld) -> Self {
        Self::with_install_reactions(world, Vec::new())
    }
    fn with_install_reactions(
        world: postretro_level_loader::LevelWorld,
        reactions: Vec<NamedReaction>,
    ) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server =
            NetServer::new(socket, address, 8, Duration::from_secs(1), Some([0x6a; 32])).unwrap();
        server.set_mod_identity("presentation.fixture".into(), "1".into());
        server.set_mod_digest(Some([0x6a; 32]));
        server.set_level_parity(Some(("presentation".into(), [0x6a; 32])));
        let host = installed_with_reactions(&world, false, reactions);
        let mut peers = vec![
            Peer::new(address, CLIENT_A, &world, true),
            Peer::new(address, CLIENT_B, &world, false),
        ];
        postretro_net::harness::reveal_both(&mut server, &mut peers[0].client, "presentation");
        server.add_relay_connection(CLIENT_A, None);
        let ctx = script_ctx(&host);
        let mut seats = netcode::SeatTable::local_only();
        seats.admit_or_reclaim(CLIENT_A, None, false).unwrap();
        seats.admit_or_reclaim(CLIENT_B, None, false).unwrap();
        let mut players = Vec::new();
        for (index, id) in [None, Some(CLIENT_A), Some(CLIENT_B)]
            .into_iter()
            .enumerate()
        {
            let mut registry = ctx.registry.borrow_mut();
            let pawn = registry.spawn(Transform {
                position: Vec3::new(-100.0, 0.0, 0.0),
                ..Default::default()
            });
            registry.set_component(pawn, movement()).unwrap();
            seats.bind_pawn(&mut registry, Seat(index as u16), pawn);
            if index == 0 {
                registry.mark_local_player_pawn(pawn).unwrap();
            }
            players.push(AuthoritativePlayer {
                id: id.map_or(PlayerId::Local(pawn), PlayerId::Remote),
                pawn,
            });
        }
        Self {
            server,
            host,
            peers,
            world,
            allocator: netcode::NetworkIdAllocator::for_test(),
            set: Default::default(),
            tracked: Default::default(),
            replication: Default::default(),
            state: Default::default(),
            owners: Default::default(),
            weapons: Default::default(),
            queues: Default::default(),
            pending_hits: Default::default(),
            open_shots: Default::default(),
            seats,
            last: None,
            tick: 0,
            clock: 0.0,
            system: Default::default(),
            bindings: Default::default(),
            players,
            host_commands: Vec::new(),
        }
    }
    fn connect_late(&mut self) {
        // The late peer starts advancing its level clock from zero. The harness
        // clock remains the shared instant used to compare completion times.
        self.peers[1].clock_origin = self.clock;
        self.peers[1].clock = 0.0;
        self.peers[1].active = true;
        postretro_net::harness::reveal_both(
            &mut self.server,
            &mut self.peers[1].client,
            "presentation",
        );
        self.server.add_relay_connection(CLIENT_B, None);
    }
    fn transport(&mut self) {
        let duration = Duration::from_secs_f64(DT);
        self.server.update_connections(duration);
        for peer in &mut self.peers {
            if !peer.active {
                continue;
            }
            peer.client.update_connections(duration);
            peer.up.enqueue_all(peer.client.packets_to_send());
            peer.down.enqueue_all(self.server.packets_to_send(peer.id));
            peer.up.advance(17);
            peer.down.advance(17);
            for packet in peer.up.take_ready() {
                self.server.process_packet_from(&packet, peer.id);
            }
            for packet in peer.down.take_ready() {
                peer.client.process_packet(&packet);
            }
        }
        let _ = self.server.poll_handshakes();
        for peer in &self.peers {
            if !peer.active {
                continue;
            }
            netcode::host_handle_client_messages(
                &mut self.server,
                &mut self.replication,
                &mut self.state,
                &mut self.queues,
                &self.owners,
                &mut self.pending_hits,
                &mut self.open_shots,
                peer.id,
                self.tick,
                (self.clock * 1_000_000.0) as u64,
            );
        }
    }
    fn publish(&mut self) {
        let ctx = script_ctx(&self.host);
        let registry = ctx.registry.borrow();
        let session = self.host.session.as_ref().unwrap();
        netcode::host_register_descriptor_entities(
            &registry,
            &mut self.allocator,
            &mut self.set,
            &mut self.tracked,
            &mut ctx.presentation_overrides.borrow_mut(),
        );
        let resolver = PresentationBindings {
            lights: &session.light_bridge,
            fog: &session.fog_volume_bridge,
            map_entities: &self.world.map_entities,
        };
        netcode::host_register_presentation_overrides(
            &registry,
            &mut ctx.presentation_overrides.borrow_mut(),
            &resolver,
            &mut self.allocator,
            &mut self.set,
            &mut self.replication,
        );
        netcode::host_replicate(
            &registry,
            &ctx.slot_table.borrow(),
            &ctx.faction_sentiment.borrow(),
            &Default::default(),
            &Default::default(),
            &mut self.server,
            &mut self.allocator,
            &mut self.replication,
            &mut self.state,
            &self.set,
            &self.owners,
            &self.weapons,
            &self.queues,
            None,
            self.tick,
            self.clock,
            true,
            &mut self.last,
            &ctx.presentation_overrides.borrow(),
        );
    }
    fn step(&mut self) {
        self.tick += 1;
        // Script time keeps elapsed frame time even when a stall costs ticks.
        self.clock += DT;
        self.publish();
        self.transport();
        for peer in &mut self.peers {
            if peer.active {
                peer.clock = self.clock - peer.clock_origin;
                peer.receive(&self.world);
                peer.sample();
            }
        }
        sample(&mut self.host, self.clock);
    }
    fn advance(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }
    fn install_reactions(&mut self, reactions: Vec<NamedReaction>) {
        let ctx = script_ctx(&self.host);
        ctx.data_registry
            .borrow_mut()
            .populate_level(reactions, vec![], &[]);
        self.bindings = TriggerBindingTable::build_with_script_ctx(
            &ctx.registry.borrow(),
            &ctx.data_registry.borrow(),
            &ctx,
        );
    }
    fn fire_plate(&mut self, x: f32, participants: &[usize]) {
        let ctx = script_ctx(&self.host);
        ctx.presentation_script_time.set(self.clock);
        ctx.presentation_overrides.borrow_mut().begin_frame();
        for (index, player) in self.players.iter().enumerate() {
            let mut t = *ctx
                .registry
                .borrow()
                .get_component::<Transform>(player.pawn)
                .unwrap();
            t.position = Vec3::new(
                if participants.contains(&index) {
                    x
                } else {
                    -100.0
                },
                0.0,
                0.0,
            );
            ctx.registry
                .borrow_mut()
                .set_component(player.pawn, t)
                .unwrap();
        }
        let mut residuals = Vec::new();
        let alive = self.players.iter().map(|p| p.id).collect();
        let session = self.host.session.as_ref().unwrap();
        self.system.run_authoritative_tick_with_dispatch(
            &mut ctx.registry.borrow_mut(),
            &session.trigger_volume_bridge,
            TriggerTickInputs {
                players: &self.players,
                use_pressed: &HashMap::new(),
                tick_dt: DT as f32,
            },
            TriggerDispatchInputs {
                alive_players: &alive,
                bound_edges: self.bindings.bound_edges(),
            },
            |event, count, occupants, registry| {
                let execution = self.bindings.execute_with_script_ctx(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    &ctx,
                    &TriggerFireContext {
                        fired_teleporter: None,
                        fired_trigger: Some(event.fire.trigger),
                        activator: self
                            .players
                            .iter()
                            .find(|p| p.id == event.fire.player)
                            .map(|p| p.pawn),
                        occupancy: count,
                        event_player: None,
                    },
                );
                if let Some(handle) = execution.residual() {
                    residuals.push((
                        handle,
                        event.fire.trigger,
                        event.fire.player,
                        occupants.to_vec(),
                    ));
                }
            },
        );
        let mut follow = Vec::new();
        drain_frame_residuals(
            &residuals,
            &self.bindings,
            &self.system,
            &[],
            &PlayerEventTable::default(),
            &session.scripting.scheduler,
            ResidualRegistries {
                sequence: &session.scripting.sequence_registry,
                reaction: &session.scripting.reaction_registry,
                system: &session.scripting.system_registry,
            },
            &ctx,
            &mut follow,
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(id) => self.seats.seat_for_client(id),
            },
        );
        dispatch_deferred_named_events_with_sequences(
            follow,
            &ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &ctx,
        );
        self.route();
    }
    fn route(&mut self) {
        let ctx = script_ctx(&self.host);
        netcode::presentation_commands::route_player_presentation(
            &ctx.system_commands,
            Some(&mut self.server),
            Some(&self.seats),
        );
        self.host_commands.extend(ctx.system_commands.take());
    }
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
            friction: GroundParams::DEFAULT_FRICTION,
            stop_speed: GroundParams::DEFAULT_STOP_SPEED,
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
        swim: None,
        vault: None,
        view_feel: None,
    })
}

use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::data_descriptors::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
};

fn map_light() -> MapLight {
    MapLight {
        origin: [1.0, 2.0, 3.0],
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::InverseSquared,
        falloff_range: 10.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0; 3],
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn fog_record() -> FogVolumeRecord {
    FogVolumeRecord {
        min: [-1.0; 3],
        max: [1.0; 3],
        center: [0.0; 3],
        inv_half_ext: [1.0; 3],
        half_diag: 3.0_f32.sqrt(),
        shape_mode: 0.0,
        plane_count: 0,
        planes: vec![],
        density: 0.5,
        edge_softness: 1.0,
        glow: 0.0,
        radial_falloff: 2.0,
        tint: [1.0; 3],
        saturation: 1.0,
        min_brightness: 0.0,
        light_range: 1.0,
        anisotropy: 0.0,
        ambient_scatter: 1.0,
        tags: vec![],
    }
}

use postretro_level_format::fog_volumes::FogVolumeRecord;
use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
