//! Life-state carry through App suspension, parity demotion and staged reload.
//! See: context/lib/networking.md §Slot lifecycle.

use super::tests::{script_ctx, test_app};
use crate::App;
use crate::netcode::{NetEndpoint, NetRole, SeatTable, WorldLessPoll};
use glam::Vec3;
use postretro_entities::components::health::{HealthComponent, set_health_absolute};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, ScriptCtx, Transform};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_net::slots::{CloseCause, SlotEvent};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{ConnectClaim, HoldingCause, PlayerClaimId, encode_connect_claim};
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_test_log_capture::LogCapture;
use serde_json::json;
use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

const CLIENT: u64 = 41;
const STEP: Duration = Duration::from_millis(34);
const LEVEL: &str = "life-state-lifecycle";
const LEVEL_DIGEST: [u8; 32] = [0x41; 32];

fn write_manifest(root: &std::path::Path, remove_down: bool, change_digest: bool) {
    let mut states = json!({"alive": {}, "dead": {}, "spectating": {"fire": false}});
    if !remove_down {
        states["down"] = json!({});
    }
    let rejoin = if remove_down {
        json!({"alive": "spectating", "dead": "spectating"})
    } else {
        json!({"dead": "spectating", "down": "spectating"})
    };
    let manifest = json!({
        "name": "Life-state lifecycle", "id": "life-state-lifecycle", "version": "1",
        "triggerPools": if change_digest {
            json!([{"tag": "lifecycle_digest_change", "arm": 1}])
        } else { json!([]) },
        "entities": [{"canonicalName": "player", "components": {
            "movement": {
                "capsule": {"radius": 0.4, "halfHeight": 0.8, "eyeHeight": 0.5},
                "ground": {"speed": {"walk": 7, "run": 11, "crouch": 3}, "accel": 10,
                    "stepHeight": 0.3, "maxSlope": 45},
                "air": {"forwardSteer": 0, "accel": 0.7, "maxControlSpeed": 0.5,
                    "bunnyHop": false, "jumps": 0, "jumpVelocity": 5.5, "jumpCeiling": 0},
                "fall": {"terminalVelocity": 40}
            },
            "health": {"max": 100},
            "lifeStates": {"initial": "alive", "states": states, "rejoin": rejoin}
        }}]
    });
    std::fs::write(
        root.join("start-script.js"),
        format!("globalThis.__postretroModManifest = {manifest};"),
    )
    .unwrap();
}

fn commit_manifest(app: &mut App, root: &std::path::Path, capture: &LogCapture) {
    let generation = app
        .session
        .as_mut()
        .unwrap()
        .scripting
        .script_runtime
        .enqueue_staged_manifest_build(root)
        .expect("staged build queued")
        .expect("debug builds stage manifest reloads");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let diagnostic_prefix = format!("[Scripting] staged mod-init generation {generation} ");
    loop {
        app.poll_staged_manifest_results();
        let errors: Vec<_> = capture
            .records()
            .into_iter()
            .filter(|record| {
                record.level == log::Level::Error && record.message.contains(&diagnostic_prefix)
            })
            .collect();
        assert!(
            errors.is_empty(),
            "staged fixture did not commit: {errors:#?}"
        );
        if app
            .session
            .as_ref()
            .unwrap()
            .scripting
            .script_runtime
            .committed_staged_manifest_generation()
            == Some(generation)
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "staged worker did not complete: {:#?}",
            capture.records()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_points() -> Vec<MapEntity> {
    [Vec3::ZERO, Vec3::new(8.0, 0.0, 3.0)]
        .into_iter()
        .map(|origin| MapEntity {
            map_ordinal: None,
            classname: "player_spawn".into(),
            origin,
            angles: Vec3::ZERO,
            key_values: [("entity_class".into(), "player".into())]
                .into_iter()
                .collect(),
            tags: Vec::new(),
        })
        .collect()
}

fn claim() -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([0x41; 16]),
        display_name: "Life-state runner".into(),
    }
}

struct World {
    app: App,
    root: tempfile::TempDir,
    client: NetClient,
    client_id: u64,
    host: EntityId,
    remote: EntityId,
    seat: Seat,
    server_address: std::net::SocketAddr,
}

impl World {
    fn new() -> Self {
        let mut app = test_app();
        let root = tempfile::tempdir().unwrap();
        write_manifest(root.path(), false, false);
        let capture = LogCapture::start();
        commit_manifest(&mut app, root.path(), &capture);
        drop(capture);
        let ctx = script_ctx(&app);
        let descriptors = ctx.data_registry.borrow().entities.clone();
        let first = postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts(
            &spawn_points(),
            &descriptors,
            &mut ctx.registry.borrow_mut(),
            None,
        )
        .expect("production first-player materialization");
        let mut seats = SeatTable::new().unwrap();
        seats.bind_level_spawn_placement(first.pawn, first.placement);
        seats.bind_pawn(&mut ctx.registry.borrow_mut(), Seat(0), first.pawn);
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_address = socket.local_addr().unwrap();
        let mut server =
            NetServer::new(socket, server_address, 2, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity("life-state-lifecycle".into(), "1".into());
        server.set_mod_digest(Some(
            crate::mod_digest::mod_compatibility_digest_from_registry(&ctx.data_registry.borrow()),
        ));
        server.set_level_parity(Some((LEVEL.into(), LEVEL_DIGEST)));
        server.set_revealed_level(Some(LEVEL.into()));
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
        session.seat_table = Some(seats);
        let client = Self::new_client(&ctx, server_address, CLIENT);
        let mut world = Self {
            app,
            root,
            client,
            client_id: CLIENT,
            host: first.pawn,
            remote: first.pawn,
            seat: Seat(0),
            server_address,
        };
        world
            .server()
            .add_relay_connection(CLIENT, Some(encode_connect_claim(&claim())));
        world.pump_until_participating();
        world.seat = world
            .app
            .session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
            .seat_for_client(CLIENT)
            .expect("App admission assigned durable seat");
        world.remote = world.materialize();
        world
    }

    fn new_client(ctx: &ScriptCtx, address: std::net::SocketAddr, client_id: u64) -> NetClient {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = NetClient::new(
            socket,
            address,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap();
        client.set_mod_identity("life-state-lifecycle".into(), "1".into());
        client.set_mod_digest(Some(
            crate::mod_digest::mod_compatibility_digest_from_registry(&ctx.data_registry.borrow()),
        ));
        client.set_level_parity(Some((LEVEL.into(), LEVEL_DIGEST)));
        client.set_revealed_level(Some(postretro_net::wire::RevealedLevel::new(LEVEL, None)));
        client.set_connected();
        client
    }

    fn server(&mut self) -> &mut NetServer {
        let NetEndpoint::Host { server, .. } = self
            .app
            .session
            .as_mut()
            .unwrap()
            .net_endpoint
            .as_mut()
            .unwrap()
        else {
            unreachable!()
        };
        server
    }

    fn poll(&mut self) -> postretro_net::transport::ServerPoll {
        let Some(WorldLessPoll::Host(poll)) = self.app.poll_world_less_transport(0.0) else {
            panic!("host CPU lifecycle poll")
        };
        poll
    }

    fn pump_until_participating(&mut self) {
        let id = self.client_id;
        for _ in 0..128 {
            self.client.update_connections(STEP);
            for packet in self.client.packets_to_send() {
                self.server().process_packet_from(&packet, id);
            }
            self.server().update_connections(STEP);
            let _ = self.poll();
            for packet in self.server().packets_to_send(id) {
                self.client.process_packet(&packet);
            }
            if self.server().is_participating(id) {
                return;
            }
        }
        panic!("relay client did not enter participation");
    }

    fn materialize(&mut self) -> EntityId {
        let ctx = script_ctx(&self.app);
        let descriptors = ctx.data_registry.borrow().entities.clone();
        let points = spawn_points();
        let session = self.app.session.as_mut().unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        let seat = seats.seat_for_client(self.client_id).unwrap();
        let placement = seats
            .assign_joiner_placement(&ctx.registry.borrow(), seat, &[0, 1])
            .unwrap();
        let carried = seats.carried_state(seat).cloned();
        let NetEndpoint::Host {
            allocator,
            replicable,
            slot_pawns,
            command_queues,
            owners,
            weapon_owners,
            open_shots,
            pending_hit_declarations,
            weaponless_fire_logged,
            ..
        } = session.net_endpoint.as_mut().unwrap()
        else {
            unreachable!()
        };
        let pawn = crate::netcode::host_handle_accept_descriptor_at_placement(
            &mut ctx.registry.borrow_mut(),
            allocator,
            replicable,
            slot_pawns,
            command_queues,
            owners,
            weapon_owners,
            open_shots,
            pending_hit_declarations,
            weaponless_fire_logged,
            self.client_id,
            &points,
            placement,
            &descriptors,
            None,
            carried.as_ref(),
        )
        .expect("production net-slot materialization");
        seats.bind_pawn(&mut ctx.registry.borrow_mut(), seat, pawn);
        crate::respawn_placements::publish(&ctx, &points, &descriptors, Some(seats), Some(0));
        pawn
    }

    fn enter(&self, pawn: EntityId, state: &str, ticks: u32, health: f32) {
        let ctx = script_ctx(&self.app);
        let mut registry = ctx.registry.borrow_mut();
        postretro_sim::life_state_commands::set_life_state(&ctx, &mut registry, &[pawn], state);
        let Ok(ComponentValue::PlayerMovement(movement)) =
            registry.get_component_value_mut(pawn, ComponentKind::PlayerMovement)
        else {
            panic!("spawned player has movement")
        };
        movement.life_state_elapsed_ticks = ticks;
        set_health_absolute(&mut registry, pawn, health);
    }

    fn assert_state(&self, pawn: EntityId, state: &str, ticks: u32) {
        let ctx = script_ctx(&self.app);
        let registry = ctx.registry.borrow();
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        let data = ctx.data_registry.borrow();
        assert_eq!(
            data.life_state_names[usize::from(movement.life_state)],
            state
        );
        assert_eq!(movement.life_state_elapsed_ticks, ticks);
    }

    fn carried_name(&self) -> Option<String> {
        self.app
            .session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
            .carried_state(self.seat)
            .unwrap()
            .life_state
            .clone()
    }

    fn assert_restored(&self, pawn: EntityId, state: &str) {
        self.assert_state(pawn, state, 0);
        let ctx = script_ctx(&self.app);
        let registry = ctx.registry.borrow();
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        let transform = registry.get_component::<Transform>(pawn).unwrap();
        let expected = spawn_points()[1].origin
            + Vec3::Y * (movement.capsule.half_height + movement.capsule.radius);
        assert!(transform.position.distance(expected) < 1.0e-5);
        let health = registry.get_component::<HealthComponent>(pawn).unwrap();
        assert!((health.current - health.max).abs() < 1.0e-5);
    }

    fn restore_host_parity(&mut self) {
        self.server()
            .set_level_parity(Some((LEVEL.into(), LEVEL_DIGEST)));
        self.server().set_revealed_level(Some(LEVEL.into()));
    }
}

#[test]
fn life_state_o6_down_disconnect_reclaims_rejoin_state_until_hold_expiry_then_starts_initial() {
    let mut world = World::new();
    world.enter(world.remote, "down", 600, 0.0);
    let original_seat = world.seat;
    let old = world.remote;
    let _ = world
        .server()
        .close_relay_connection(CLIENT, CloseCause::Disconnect);
    let _ = world.poll();
    assert!(!script_ctx(&world.app).registry.borrow().exists(old));
    assert_eq!(world.carried_name().as_deref(), Some("down"));

    world.client_id = CLIENT + 1;
    world.client = World::new_client(&script_ctx(&world.app), world.server_address, CLIENT + 1);
    world
        .server()
        .add_relay_connection(CLIENT + 1, Some(encode_connect_claim(&claim())));
    world.pump_until_participating();
    let reclaimed = world
        .app
        .session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .seat_for_client(CLIENT + 1)
        .unwrap();
    assert_eq!(reclaimed, original_seat);
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");

    world.enter(restored, "down", 600, 0.0);
    let _ = world
        .server()
        .close_relay_connection(CLIENT + 1, CloseCause::Disconnect);
    let _ = world.poll();
    assert!(!script_ctx(&world.app).registry.borrow().exists(restored));
    assert_eq!(world.carried_name().as_deref(), Some("down"));
    world
        .app
        .session
        .as_mut()
        .unwrap()
        .seat_table
        .as_mut()
        .unwrap()
        .advance_hold_clock(Duration::from_secs(31));
    let _ = world.poll();
    assert!(
        world
            .app
            .session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
            .carried_state(original_seat)
            .is_none(),
        "the production poll discards the expired seat's carried record"
    );

    world.client_id = CLIENT + 2;
    world.client = World::new_client(&script_ctx(&world.app), world.server_address, CLIENT + 2);
    world
        .server()
        .add_relay_connection(CLIENT + 2, Some(encode_connect_claim(&claim())));
    world.pump_until_participating();
    let seats = world
        .app
        .session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap();
    world.seat = seats.seat_for_client(CLIENT + 2).unwrap();
    assert_ne!(
        world.seat, original_seat,
        "the expired claim receives a fresh seat"
    );
    assert!(seats.carried_state(world.seat).is_none());
    let fresh = world.materialize();
    world.assert_restored(fresh, "alive");
}

#[test]
fn life_state_o22_held_dead_survives_app_suspend_reclaims_as_spectating_and_unload_clears_name() {
    let mut world = World::new();
    world.enter(world.remote, "dead", 731, 0.0);
    let old = world.remote;
    let _ = world
        .server()
        .close_relay_connection(CLIENT, CloseCause::Disconnect);
    let _ = world.poll();
    assert!(!script_ctx(&world.app).registry.borrow().exists(old));
    assert_eq!(world.carried_name().as_deref(), Some("dead"));
    world.app.suspend_platform();
    assert_eq!(world.carried_name().as_deref(), Some("dead"));
    world.restore_host_parity();
    world.client_id = CLIENT + 1;
    world.client = World::new_client(&script_ctx(&world.app), world.server_address, CLIENT + 1);
    world
        .server()
        .add_relay_connection(CLIENT + 1, Some(encode_connect_claim(&claim())));
    world.pump_until_participating();
    let reclaimed = world
        .app
        .session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .seat_for_client(CLIENT + 1)
        .unwrap();
    assert_eq!(reclaimed, world.seat);
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");
    let _ = world
        .server()
        .close_relay_connection(CLIENT + 1, CloseCause::Disconnect);
    let _ = world.poll();
    assert_eq!(world.carried_name().as_deref(), Some("spectating"));
    world.app.unload_level();
    assert_eq!(
        world.carried_name(),
        None,
        "actual level retirement clears held names"
    );
}

#[test]
fn life_state_o30_app_suspend_keeps_host_clock_and_demotes_remote_before_rejoin() {
    let mut world = World::new();
    world.enter(world.host, "down", 731, 0.0);
    world.enter(world.remote, "down", 600, 0.0);
    let old = world.remote;
    world.app.suspend_platform();
    world.assert_state(world.host, "down", 731);
    assert!(
        script_ctx(&world.app).registry.borrow().exists(old),
        "demotion drains at resumed poll"
    );
    let poll = world.poll();
    assert!(poll.lifecycle.iter().any(|event| matches!(
        event,
        SlotEvent::Demoted {
            client_id: CLIENT,
            cause: HoldingCause::HostLevelAbsent
        }
    )));
    assert!(!script_ctx(&world.app).registry.borrow().exists(old));
    world.assert_state(world.host, "down", 731);
    assert_eq!(world.carried_name().as_deref(), Some("down"));
    world.restore_host_parity();
    world.pump_until_participating();
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");
    world.assert_state(world.host, "down", 731);
}

#[test]
fn life_state_o23_actual_reload_digest_demotion_harvests_down_at_ten_seconds_then_rejoins() {
    let mut world = World::new();
    world.enter(world.remote, "down", 600, 0.0);
    let old = world.remote;
    write_manifest(world.root.path(), false, true);
    let capture = LogCapture::start();
    commit_manifest(&mut world.app, world.root.path(), &capture);
    world.assert_state(old, "down", 600);
    let poll = world.poll();
    assert!(poll.lifecycle.iter().any(|event| matches!(
        event,
        SlotEvent::Demoted {
            client_id: CLIENT,
            cause: HoldingCause::ModDigest { .. }
        }
    )));
    assert!(!script_ctx(&world.app).registry.borrow().exists(old));
    assert_eq!(world.carried_name().as_deref(), Some("down"));
    let digest = crate::mod_digest::mod_compatibility_digest_from_registry(
        &script_ctx(&world.app).data_registry.borrow(),
    );
    world.client.set_mod_digest(Some(digest));
    world.pump_until_participating();
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");
}

#[test]
fn life_state_o41_actual_reload_remaps_removed_down_before_digest_harvest_and_initial_rejoin() {
    let mut world = World::new();
    world.enter(world.remote, "down", 600, 0.0);
    let old = world.remote;
    write_manifest(world.root.path(), true, true);
    let capture = LogCapture::start();
    commit_manifest(&mut world.app, world.root.path(), &capture);
    capture.assert_logged_once(
        log::Level::Warn,
        "state `down` removed by reload; entering initial",
    );
    world.assert_state(old, "alive", 0);
    let poll = world.poll();
    assert!(poll.lifecycle.iter().any(|event| matches!(
        event,
        SlotEvent::Demoted {
            client_id: CLIENT,
            cause: HoldingCause::ModDigest { .. }
        }
    )));
    assert!(!script_ctx(&world.app).registry.borrow().exists(old));
    assert_eq!(world.carried_name().as_deref(), Some("alive"));
    let digest = crate::mod_digest::mod_compatibility_digest_from_registry(
        &script_ctx(&world.app).data_registry.borrow(),
    );
    world.client.set_mod_digest(Some(digest));
    world.pump_until_participating();
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");
    capture.assert_logged_once(
        log::Level::Warn,
        "state `down` removed by reload; entering initial",
    );
}

#[test]
fn life_state_o21_app_demotion_harvests_pre_respawn_state_and_missing_pawn_drain_warns_once() {
    let mut world = World::new();
    world.enter(world.remote, "down", 600, 0.0);
    let old = world.remote;
    let ctx = script_ctx(&world.app);
    let mut reactions = ReactionPrimitiveRegistry::new();
    postretro_sim::respawn_commands::register_respawn_reaction_primitives(
        &mut reactions,
        ctx.clone(),
    );
    assert!(
        reactions
            .dispatch(
                "respawn",
                &mut ctx.registry.borrow_mut(),
                &[old],
                &json!({})
            )
            .unwrap()
    );
    assert_eq!(ctx.pending_respawns.borrow().len(), 1);
    // The parity poll precedes tick-start relocation, as in the real frame.
    world.server().set_level_parity(None);
    let _ = world.poll();
    assert_eq!(world.carried_name().as_deref(), Some("down"));
    assert!(!ctx.registry.borrow().exists(old));
    let capture = LogCapture::start();
    let _ = postretro_sim::sim::begin_tick(&ctx.registry, Some(&ctx));
    capture.assert_logged_once(log::Level::Warn, "no longer participates; skipping");
    assert!(ctx.pending_respawns.borrow().is_empty());
    let _ = postretro_sim::sim::begin_tick(&ctx.registry, Some(&ctx));
    capture.assert_logged_once(log::Level::Warn, "no longer participates; skipping");
    world.restore_host_parity();
    world.pump_until_participating();
    let restored = world.materialize();
    world.assert_restored(restored, "spectating");
}

#[test]
fn life_state_o39_actual_staged_app_reload_discards_a_parked_life_state_wait_tail() {
    use postretro_entities::{
        GroupKind, GroupTarget, NamedReaction, ReactionDescriptor, SequenceStep, SequenceTarget,
    };
    use postretro_scripting_core::reaction_dispatch::fire_named_event_with_sequences;
    use postretro_sim::scripting_systems::reaction_scheduler::register_reaction_control_primitives;

    let mut world = World::new();
    world.app.level = Some(super::tests::level_world("parked-life-state", 0));
    assert!(world.app.has_installed_level());
    let ctx = script_ctx(&world.app);
    let scheduler = world
        .app
        .session
        .as_ref()
        .unwrap()
        .scripting
        .scheduler
        .clone();
    scheduler.set_role(postretro_sim::scripting_systems::reaction_scheduler::SchedulerRole::Host);
    {
        let scripting = &mut world.app.session.as_mut().unwrap().scripting;
        register_reaction_control_primitives(&mut scripting.sequence_registry, scheduler.clone());
        postretro_sim::life_state_commands::register_sequenced_life_state_primitives(
            &mut scripting.sequence_registry,
            ctx.clone(),
        );
        postretro_sim::life_state_commands::register_life_state_reaction_primitives(
            &mut scripting.reaction_registry,
            ctx.clone(),
        );
    }
    ctx.data_registry
        .borrow_mut()
        .set_level_reactions(vec![NamedReaction {
            name: "park-life-state".into(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".into(),
                    args: json!({"durationMs": 1}),
                },
                SequenceStep {
                    id: SequenceTarget::Group(GroupTarget {
                        kind: GroupKind::Player,
                        tag: None,
                    }),
                    primitive: "setLifeState".into(),
                    args: json!({"state": "down"}),
                },
            ]),
        }]);
    ctx.data_registry.borrow_mut().recompose(&[]);
    let fire = |app: &App| {
        let scripting = &app.session.as_ref().unwrap().scripting;
        let _ = fire_named_event_with_sequences(
            "park-life-state",
            &ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &ctx,
            None,
        );
    };
    let drain = |app: &App| {
        let scripting = &app.session.as_ref().unwrap().scripting;
        scheduler.begin_frame();
        scheduler.evaluate(&[]);
        scheduler.drain_landings(
            &ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            &ctx,
        );
    };
    fire(&world.app);
    assert_eq!(scheduler.pending_len(), 1);
    drain(&world.app);
    world.assert_state(world.remote, "down", 0);
    world.assert_state(world.host, "down", 0);
    world.enter(world.remote, "alive", 0, 100.0);
    world.enter(world.host, "alive", 0, 100.0);
    fire(&world.app);
    assert_eq!(
        scheduler.pending_len(),
        1,
        "a real wait is parked at the commit"
    );

    let capture = LogCapture::start();
    commit_manifest(&mut world.app, world.root.path(), &capture);
    capture.assert_logged_once(
        log::Level::Warn,
        "dropping 1 pending timed-reaction instance(s)",
    );
    assert_eq!(scheduler.pending_len(), 0);
    for _ in 0..3 {
        drain(&world.app);
    }
    world.assert_state(world.remote, "alive", 0);
    world.assert_state(world.host, "alive", 0);
}
