//! Carried names through both descriptor-backed spawn routes and the first host tick.
use super::*;
use crate::player_events::tests::{input, number, on_player, player_event};
use crate::scripting::builtins::{
    data_archetype::spawn_from_player_starts_with_carried_loadout,
    net_descriptor::spawn_net_slot_pawn_with_carried_loadout,
};
use crate::scripting::map_entity::MapEntity;
use crate::sim::tests::{run_remote_only_tick, trigger_movement};
use glam::Vec3;
use postretro_combat_model::CarriedState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityTypeDescriptor, LifeStatesDescriptor, ScriptCtx, Transform};
use postretro_foundation::{
    Capability, CrouchParams, HealthDescriptor, IrNode, LifeStatePosture, LifeStateProfile,
    MovementState, PlayerMovementDescriptor, Seat, UNDECLARED_LIFE_STATE, player_capability,
};
use postretro_scripting_core::data_descriptors::PlayerEventEdge;
use serde_json::json;
use std::collections::HashMap;

const EPS: f32 = 1.0e-5;

#[derive(Clone, Copy, Debug)]
enum Route {
    FirstPlayer,
    NetSlot,
}
const ROUTES: [Route; 2] = [Route::FirstPlayer, Route::NetSlot];

fn descriptor(initial: &str, rejoin_target: Option<&str>) -> EntityTypeDescriptor {
    let m = trigger_movement();
    let mut movement = PlayerMovementDescriptor {
        capsule: m.capsule,
        ground: m.ground_params,
        air: m.air,
        fall: m.fall,
        dash: m.dash,
        crouch: m.crouch,
        slide: m.slide,
        swim: m.swim,
        vault: m.vault,
        view_feel: m.view_feel,
        sounds: m.sounds,
        knockback: m.knockback,
        forgiveness: None,
        stuck_stop_enabled: m.stuck_stop_enabled,
        stuck_stop_threshold: m.stuck_stop_threshold,
    };
    movement.crouch = Some(CrouchParams {
        half_height: 0.4,
        eye_height: 0.2,
        transition_rate: 15.0,
    });
    let mut down = LifeStateProfile::default();
    down.movement.posture = LifeStatePosture::Crouch;
    down.movement.abilities = false;
    down.movement.speed_scale = 0.3;
    let mut spectating = down.clone();
    spectating.look = false;
    spectating.fire = false;
    spectating.movement.speed_scale = 0.0;
    EntityTypeDescriptor {
        canonical_name: Some("player".into()),
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: Some(HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        }),
        behavior: None,
        movement: Some(movement),
        life_states: Some(LifeStatesDescriptor {
            initial: initial.into(),
            states: [
                ("alive".into(), LifeStateProfile::default()),
                ("down".into(), down),
                ("spectating".into(), spectating),
            ]
            .into(),
            rejoin: rejoin_target
                .map(|target| [("down".into(), target.into())].into())
                .unwrap_or_default(),
            state_ids: Default::default(),
            table_revision: 0,
        }),
    }
}

fn context(descriptor: EntityTypeDescriptor) -> ScriptCtx {
    let ctx = ScriptCtx::new();
    ctx.gravity.set(-9.81);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor]);
    ctx
}

fn placement(foot: Vec3) -> MapEntity {
    MapEntity {
        map_ordinal: Some(3),
        classname: "player_spawn".into(),
        origin: foot,
        angles: Vec3::new(0.0, 0.6, 0.0),
        key_values: HashMap::new(),
        tags: Vec::new(),
    }
}

fn spawn(ctx: &ScriptCtx, route: Route, foot: Vec3, carried: Option<&CarriedState>) -> EntityId {
    let data = ctx.data_registry.borrow();
    let mut registry = ctx.registry.borrow_mut();
    let placement = placement(foot);
    let pawn = match route {
        Route::FirstPlayer => {
            spawn_from_player_starts_with_carried_loadout(
                &[placement],
                &data.entities,
                &mut registry,
                None,
                carried,
            )
            .expect("first-player spawn")
            .pawn
        }
        Route::NetSlot => spawn_net_slot_pawn_with_carried_loadout(
            &placement,
            &data.entities,
            &mut registry,
            None,
            carried,
        )
        .expect("net-slot spawn"),
    };
    if matches!(route, Route::NetSlot) {
        assert!(
            registry.seat_for_pawn(pawn).is_none(),
            "restore precedes durable seat binding"
        );
        assert_ne!(registry.local_player_pawn(), Some(pawn));
    }
    pawn
}

fn movement(ctx: &ScriptCtx, pawn: EntityId) -> PlayerMovementComponent {
    ctx.registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone()
}
fn health(ctx: &ScriptCtx, pawn: EntityId) -> f32 {
    ctx.registry
        .borrow()
        .get_component::<HealthComponent>(pawn)
        .unwrap()
        .current
}
fn name(ctx: &ScriptCtx, pawn: EntityId) -> Option<String> {
    life_state_name(
        &ctx.registry.borrow(),
        pawn,
        &ctx.data_registry.borrow().life_state_names,
    )
}
fn record(state: Option<&str>, health: Option<f32>) -> CarriedState {
    CarriedState {
        life_state: state.map(str::to_owned),
        health_current: health,
        ..Default::default()
    }
}

fn assert_spawn(ctx: &ScriptCtx, pawn: EntityId, foot: Vec3, state: &str, hp: f32) {
    assert_eq!(name(ctx, pawn).as_deref(), Some(state));
    assert_eq!(health(ctx, pawn), hp);
    let movement = movement(ctx, pawn);
    let data = ctx.data_registry.borrow();
    let states = data.entities[0].life_states.as_ref().unwrap();
    let (id, profile) = states.state(state).unwrap();
    assert_eq!(
        (
            movement.life_state,
            movement.life_state_table_revision,
            movement.life_state_elapsed_ticks
        ),
        (id, states.table_revision, 0)
    );
    assert_eq!(
        player_capability(&movement, Capability::Look),
        Some(profile.look)
    );
    assert_eq!(
        player_capability(&movement, Capability::Fire),
        Some(profile.fire)
    );
    assert_eq!(movement.life_speed_scale, profile.movement.speed_scale);
    let crouch = profile.movement.posture == LifeStatePosture::Crouch;
    assert_eq!(movement.capsule.half_height, if crouch { 0.4 } else { 0.8 });
    assert_eq!(movement.capsule.eye_height, if crouch { 0.2 } else { 0.5 });
    assert_eq!(
        matches!(movement.movement_state, MovementState::Crouching { .. }),
        crouch
    );
    let expected = foot + Vec3::Y * (movement.capsule.radius + movement.capsule.half_height);
    let registry = ctx.registry.borrow();
    for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
        assert!(
            (registry
                .interpolated_transform(pawn, alpha)
                .unwrap()
                .position
                - expected)
                .length()
                < EPS,
            "spawn posture starts at its target with the authored foot anchor"
        );
    }
}

#[test]
fn rejoin_spawn_routes_map_down_to_alive_or_spectating_at_elapsed_zero() {
    for route in ROUTES {
        for target in ["alive", "spectating"] {
            let ctx = context(descriptor("alive", Some(target)));
            let foot = Vec3::new(7.0, 2.0, -3.0);
            let pawn = spawn(&ctx, route, foot, Some(&record(Some("down"), Some(29.0))));
            assert_spawn(&ctx, pawn, foot, target, 29.0);
        }
    }
}

#[test]
fn rejoin_spawn_routes_fall_back_for_missing_map_removed_name_or_expired_hold() {
    for route in ROUTES {
        for case in 0..4 {
            let mut desc = descriptor("alive", Some("spectating"));
            let carried = match case {
                0 => {
                    desc.life_states.as_mut().unwrap().rejoin.clear();
                    Some(record(Some("down"), Some(37.0)))
                }
                1 => {
                    let block = desc.life_states.as_mut().unwrap();
                    block.states.remove("down");
                    block.rejoin.clear();
                    Some(record(Some("down"), Some(37.0)))
                }
                2 => None, // hold expired: a fresh seat supplies no record
                _ => Some(record(None, Some(37.0))),
            };
            let ctx = context(desc);
            let foot = Vec3::new(1.0, 0.0, 2.0);
            let pawn = spawn(&ctx, route, foot, carried.as_ref());
            assert_spawn(
                &ctx,
                pawn,
                foot,
                "alive",
                if case == 2 { 100.0 } else { 37.0 },
            );
        }
    }
}

#[test]
fn rejoin_spawn_routes_keep_nonpositive_health_restore_at_descriptor_max() {
    for route in ROUTES {
        for hp in [0.0, -4.0] {
            let ctx = context(descriptor("alive", Some("spectating")));
            let foot = Vec3::ZERO;
            let pawn = spawn(&ctx, route, foot, Some(&record(Some("down"), Some(hp))));
            assert_spawn(&ctx, pawn, foot, "spectating", 100.0);
        }
    }
}

#[test]
fn rejoin_spawn_routes_leave_descriptors_without_life_states_undeclared() {
    for route in ROUTES {
        let mut desc = descriptor("alive", None);
        desc.life_states = None;
        let ctx = context(desc);
        let foot = Vec3::new(3.0, 1.0, 4.0);
        let pawn = spawn(&ctx, route, foot, Some(&record(Some("down"), Some(31.0))));
        let movement = movement(&ctx, pawn);
        assert_eq!(
            (movement.life_state, movement.life_state_elapsed_ticks),
            (UNDECLARED_LIFE_STATE, 0)
        );
        assert_eq!(name(&ctx, pawn), None);
        assert_eq!(health(&ctx, pawn), 31.0);
        assert_eq!(player_capability(&movement, Capability::Fire), None);
        assert_eq!(movement.capsule.half_height, 0.8);
        assert_eq!(movement.life_speed_scale, 1.0);
        assert!(
            (ctx.registry
                .borrow()
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                - (foot + Vec3::Y * 1.2))
                .length()
                < EPS
        );
    }
}

#[test]
fn rejoin_spawn_routes_resolve_saved_name_after_reload_shifts_numeric_ids() {
    for route in ROUTES {
        let ctx = context(descriptor("down", Some("spectating")));
        let old = spawn(&ctx, route, Vec3::ZERO, None);
        let old_id = movement(&ctx, old).life_state;
        let carried = record(name(&ctx, old).as_deref(), Some(42.0));
        ctx.registry.borrow_mut().despawn(old).unwrap();
        let mut next = ctx.data_registry.borrow().entities.clone();
        next[0]
            .life_states
            .as_mut()
            .unwrap()
            .states
            .insert("aardvark".into(), LifeStateProfile::default());
        ctx.data_registry.borrow_mut().replace_entity_types(next);
        let shifted = ctx.data_registry.borrow().entities[0]
            .life_states
            .as_ref()
            .unwrap()
            .state("down")
            .unwrap()
            .0;
        assert_ne!(
            old_id, shifted,
            "fixture actually shifts the saved state's numeric id"
        );
        let foot = Vec3::new(9.0, 1.0, 2.0);
        let pawn = spawn(&ctx, route, foot, Some(&carried));
        assert_spawn(&ctx, pawn, foot, "spectating", 42.0);
    }
}

#[test]
fn rejoin_spawn_routes_apply_initial_and_rejoin_crouch_before_any_command() {
    for route in ROUTES {
        let ctx = context(descriptor("down", Some("alive")));
        let foot = Vec3::new(4.0, 2.0, 1.0);
        let pawn = spawn(&ctx, route, foot, None);
        assert_spawn(&ctx, pawn, foot, "down", 100.0);
        ctx.registry.borrow_mut().despawn(pawn).unwrap();
        let pawn = spawn(&ctx, route, foot, Some(&record(Some("down"), None)));
        assert_spawn(&ctx, pawn, foot, "alive", 100.0);
    }
}

#[test]
fn rejoin_remote_first_tick_without_command_reaches_sixteen_ms_and_keeps_other_pawn() {
    let ctx = context(descriptor("alive", Some("spectating")));
    let host = spawn(&ctx, Route::FirstPlayer, Vec3::new(100.0, 0.0, 0.0), None);
    ctx.registry.borrow_mut().bind_pawn_seat(host, Seat(0));
    let mut other = movement(&ctx, host);
    other.life_state_elapsed_ticks = 47;
    ctx.registry
        .borrow_mut()
        .set_component(host, other.clone())
        .unwrap();
    postretro_entities::components::health::set_health_absolute(
        &mut ctx.registry.borrow_mut(),
        host,
        77.0,
    );
    let other_pose = *ctx
        .registry
        .borrow()
        .get_component::<Transform>(host)
        .unwrap();
    let remote = spawn(
        &ctx,
        Route::NetSlot,
        Vec3::new(8.0, 0.0, 0.0),
        Some(&record(Some("down"), Some(50.0))),
    );
    assert_spawn(&ctx, remote, Vec3::new(8.0, 0.0, 0.0), "spectating", 50.0);
    assert_eq!(movement(&ctx, host).life_state_elapsed_ticks, 47);
    assert_eq!(health(&ctx, host), 77.0);
    assert_eq!(
        *ctx.registry
            .borrow()
            .get_component::<Transform>(host)
            .unwrap(),
        other_pose
    );
    ctx.registry.borrow_mut().bind_pawn_seat(remote, Seat(1));
    let remote_pose = *ctx
        .registry
        .borrow()
        .get_component::<Transform>(remote)
        .unwrap();
    run_remote_only_tick(ctx.registry.clone(), &[]);
    assert_eq!(movement(&ctx, remote).life_state_elapsed_ticks, 1);
    assert_eq!(name(&ctx, remote).as_deref(), Some("spectating"));
    assert_eq!(
        *ctx.registry
            .borrow()
            .get_component::<Transform>(remote)
            .unwrap(),
        remote_pose,
        "no remote command means no movement step"
    );
    assert_eq!(name(&ctx, host).as_deref(), Some("alive"));
    assert_eq!(movement(&ctx, host).life_state_elapsed_ticks, 48);
    assert_eq!(health(&ctx, host), 77.0);
    {
        let mut data = ctx.data_registry.borrow_mut();
        data.set_level_reactions(vec![on_player(
            "clock-witness",
            "grantHealth",
            json!({"amount":25}),
        )]);
        data.set_level_player_events(vec![player_event(
            PlayerEventEdge::Becomes,
            IrNode::And {
                a: Box::new(input("player.inLifeState:spectating")),
                b: Box::new(IrNode::Eq {
                    a: Box::new(input("player.lifeStateMs")),
                    b: Box::new(number(16.0)),
                }),
            },
            &["clock-witness"],
        )]);
        data.recompose(&[]);
    }
    let mut table = crate::player_events::PlayerEventTable::build(
        &ctx,
        Default::default(),
        Default::default(),
        None,
    );
    let mut residuals = Vec::new();
    table.run_tick(&ctx, &[], &mut residuals);
    assert!(
        residuals.is_empty(),
        "the condition witness executes in the player pass"
    );
    assert_eq!(
        health(&ctx, remote),
        75.0,
        "first post-restore pass reads 16ms without a client command"
    );
    assert_eq!(
        health(&ctx, host),
        77.0,
        "another player's state does not satisfy the rejoin predicate"
    );
}

#[test]
fn rejoin_level_unload_clears_bound_and_held_names_before_next_level_materialization() {
    use postretro_netcode::SeatTable;
    const CLIENT: u64 = 41;
    let ctx = context(descriptor("alive", Some("spectating")));
    let mut seats = SeatTable::local_only();
    let remote_seat = seats.admit_or_reclaim(CLIENT, None, false).unwrap().seat;
    let host = spawn(&ctx, Route::FirstPlayer, Vec3::ZERO, None);
    let remote = spawn(&ctx, Route::NetSlot, Vec3::X * 8.0, None);
    {
        let mut registry = ctx.registry.borrow_mut();
        seats.bind_pawn(&mut registry, Seat(0), host);
        seats.bind_pawn(&mut registry, remote_seat, remote);
        crate::life_state_commands::set_life_state(&ctx, &mut registry, &[host, remote], "down");
        for pawn in [host, remote] {
            postretro_entities::components::health::set_health_absolute(&mut registry, pawn, 0.0);
        }
        // A remote disconnect harvests before despawn and holds the durable seat.
        seats.harvest_pawn(
            &registry,
            remote,
            &ctx.data_registry.borrow().life_state_names,
        );
        registry.despawn(remote).unwrap();
        assert_eq!(
            seats.hold_disconnected_client(&mut registry, CLIENT),
            Some(remote_seat)
        );
    }
    assert!(seats.contains_seat(remote_seat));
    assert!(
        seats.pawn_for_client(CLIENT).is_none(),
        "held remote has no live pawn binding"
    );
    assert_eq!(
        seats
            .carried_state(remote_seat)
            .unwrap()
            .life_state
            .as_deref(),
        Some("down")
    );
    // Match actual unload ordering: definitions clear, bound harvest, entity
    // retirement, then the seat-level clear (which must also visit held records).
    ctx.data_registry.borrow_mut().clear();
    {
        let mut registry = ctx.registry.borrow_mut();
        seats.harvest_bound_pawns(&registry, &ctx.data_registry.borrow().life_state_names);
        assert_eq!(
            seats.carried_state(Seat(0)).unwrap().life_state.as_deref(),
            Some("down")
        );
        assert_eq!(
            seats
                .carried_state(remote_seat)
                .unwrap()
                .life_state
                .as_deref(),
            Some("down")
        );
        registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut registry);
    }
    let host_record = seats.carried_state(Seat(0)).unwrap().clone();
    let held_record = seats.carried_state(remote_seat).unwrap().clone();
    for carried in [&host_record, &held_record] {
        assert_eq!(carried.life_state, None);
        assert_eq!(
            carried.health_current,
            Some(0.0),
            "unload clears only the life-state carry"
        );
    }
    assert_eq!(
        ctx.data_registry.borrow().entities[0]
            .life_states
            .as_ref()
            .unwrap()
            .rejoin
            .get("down")
            .map(String::as_str),
        Some("spectating")
    );
    let next_host = spawn(&ctx, Route::FirstPlayer, Vec3::X * 20.0, Some(&host_record));
    let next_remote = spawn(&ctx, Route::NetSlot, Vec3::X * 28.0, Some(&held_record));
    assert_spawn(&ctx, next_host, Vec3::X * 20.0, "alive", 100.0);
    assert_spawn(&ctx, next_remote, Vec3::X * 28.0, "alive", 100.0);
}
