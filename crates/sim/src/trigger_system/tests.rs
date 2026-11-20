use super::*;
use glam::Quat;
use postretro_entities::components::brain::{BrainComponent, attach_brain_graph};
use postretro_entities::{KinematicMoverComponent, KinematicMoverMode, ScriptCtx, TriggerCommand};
use postretro_foundation::{
    AirParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
    CapsuleParams, FallParams, GroundParams, MotionVerb, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams,
};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, VolumeTriggerEventDescriptor,
};
use postretro_scripting_core::data_registry::DataRegistry;

const DT: f32 = 0.05;

#[test]
fn duplicate_player_diagnostics_reset_with_trigger_system() {
    let mut system = TriggerSystem::default();
    let player = PlayerId::Remote(17);

    warn_duplicate_player_once(&mut system.warned_duplicate_players, player);
    warn_duplicate_player_once(&mut system.warned_duplicate_players, player);
    assert_eq!(system.warned_duplicate_players.len(), 1);

    system.clear();
    assert!(system.warned_duplicate_players.is_empty());

    warn_duplicate_player_once(&mut system.warned_duplicate_players, player);
    assert_eq!(system.warned_duplicate_players.len(), 1);
}

#[test]
fn canonical_player_pawns_uses_lowest_valid_pawn_per_identity() {
    let mut registry = EntityRegistry::new();
    let first = spawn_player(&mut registry, Vec3::ZERO);
    let second = spawn_player(&mut registry, Vec3::ZERO);
    let remote = PlayerId::Remote(17);

    assert_eq!(
        canonical_player_pawns(
            &registry,
            &[
                AuthoritativePlayer {
                    id: remote,
                    pawn: second,
                },
                AuthoritativePlayer {
                    id: remote,
                    pawn: first,
                },
            ],
        )
        .get(&remote),
        Some(&first),
    );
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

fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    });
    registry.set_component(id, movement()).unwrap();
    id
}

fn spawn_mover(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry.set_tags(id, vec!["lift".into()]).unwrap();
    registry
        .set_component(
            id,
            KinematicMoverComponent::new(
                1,
                postretro_entities::KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                    waypoint_names: vec!["start".into(), "end".into()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: KinematicMoverMode::Once,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            ),
        )
        .unwrap();
    id
}

fn spawn_trigger(
    registry: &mut EntityRegistry,
    bridge: &mut TriggerVolumeBridge,
    activation: TriggerActivation,
    fire_mode: TriggerFireMode,
    rearm_ms: f32,
    enabled: bool,
) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            TriggerVolumeComponent::new(
                activation,
                "lift".into(),
                String::new(),
                String::new(),
                TriggerCommand::Start,
                fire_mode,
                rearm_ms,
                enabled,
            ),
        )
        .unwrap();
    bridge.insert_for_test(id, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
    id
}

fn tick(
    system: &mut TriggerSystem,
    registry: &mut EntityRegistry,
    bridge: &TriggerVolumeBridge,
    players: &[AuthoritativePlayer],
    uses: &[(PlayerId, bool)],
) -> TriggerFireReport {
    let uses = uses.iter().copied().collect();
    system.run_authoritative_tick(registry, bridge, players, &uses, DT)
}

fn set_player_position(registry: &mut EntityRegistry, player: EntityId, position: Vec3) {
    let mut transform = *registry
        .get_component::<Transform>(player)
        .expect("player transform attached");
    transform.position = position;
    registry
        .set_component(player, transform)
        .expect("update player position");
}

fn set_event_names(registry: &mut EntityRegistry, trigger: EntityId, on_fire: &str, on_exit: &str) {
    let mut component = registry
        .get_component::<TriggerVolumeComponent>(trigger)
        .expect("trigger component attached")
        .clone();
    component.on_fire = on_fire.into();
    component.on_exit = on_exit.into();
    registry
        .set_component(trigger, component)
        .expect("update trigger event names");
}

#[test]
fn touch_tracks_each_players_rising_entry_and_once_latches() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    let first = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let second = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let players = [
        AuthoritativePlayer {
            id: PlayerId::Local(first),
            pawn: first,
        },
        AuthoritativePlayer {
            id: PlayerId::Remote(7),
            pawn: second,
        },
    ];
    let mut system = TriggerSystem::default();

    tick(&mut system, &mut registry, &bridge, &players, &[]);
    registry.get_component::<Transform>(first).unwrap();
    let mut entered = *registry.get_component::<Transform>(first).unwrap();
    entered.position = Vec3::new(0.0, 1.0, 0.0);
    registry.set_component(first, entered).unwrap();
    tick(&mut system, &mut registry, &bridge, &players, &[]);
    let mut second_entered = *registry.get_component::<Transform>(second).unwrap();
    second_entered.position = Vec3::new(0.0, 1.0, 0.0);
    registry.set_component(second, second_entered).unwrap();
    tick(&mut system, &mut registry, &bridge, &players, &[]);

    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .unwrap()
            .latched
    );
    assert_eq!(system.recorded_gate_fires(), &[PlayerId::Local(first)]);
}

#[test]
fn multiple_rearms_disabled_is_inert_and_use_needs_same_tick_edge() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let multiple = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        100.0,
        true,
    );
    let disabled = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        false,
    );
    let use_trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Use,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let id = PlayerId::Local(player);
    let players = [AuthoritativePlayer { id, pawn: player }];
    let mut system = TriggerSystem::default();

    tick(&mut system, &mut registry, &bridge, &players, &[]);
    tick(&mut system, &mut registry, &bridge, &players, &[(id, true)]);
    tick(&mut system, &mut registry, &bridge, &players, &[]);
    let mut out = *registry.get_component::<Transform>(player).unwrap();
    out.position.x = 4.0;
    registry.set_component(player, out).unwrap();
    tick(&mut system, &mut registry, &bridge, &players, &[]);
    let mut back = *registry.get_component::<Transform>(player).unwrap();
    back.position.x = 0.0;
    registry.set_component(player, back).unwrap();
    tick(&mut system, &mut registry, &bridge, &players, &[]);

    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(multiple)
            .unwrap()
            .rearm_remaining_ms
            > 0.0
    );
    assert!(
        !registry
            .get_component::<TriggerVolumeComponent>(disabled)
            .unwrap()
            .latched
    );
    // Touch fires on entry, use fires only on its explicit edge, then touch
    // fires again only after the 100 ms rearm interval has elapsed.
    assert_eq!(system.recorded_gate_fires(), &[id, id, id]);
    assert!(
        !registry
            .get_component::<TriggerVolumeComponent>(use_trigger)
            .unwrap()
            .latched
    );
}

// Regression: a level replacement used to leave the continuing host session
// without the per-player trigger paths that distinguish local input from a
// remote `use_pressed` command.
#[test]
fn level_change_rebuilds_local_and_remote_touch_and_use_paths() {
    let mut system = TriggerSystem::default();

    for level in 0..2 {
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let touch = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        let use_trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Use,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        set_event_names(&mut registry, touch, "touch", "");
        set_event_names(&mut registry, use_trigger, "use", "");
        let mover = spawn_mover(&mut registry);
        let local = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
        let remote = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
        let local_id = PlayerId::Local(local);
        let remote_id = PlayerId::Remote(7);
        let players = [
            AuthoritativePlayer {
                id: local_id,
                pawn: local,
            },
            AuthoritativePlayer {
                id: remote_id,
                pawn: remote,
            },
        ];

        let report = tick(
            &mut system,
            &mut registry,
            &bridge,
            &players,
            &[(local_id, true), (remote_id, true)],
        );
        assert_eq!(
            report.enters(),
            vec![
                TriggerEventFire {
                    trigger: touch,
                    player: local_id,
                    event_name: "touch".into(),
                },
                TriggerEventFire {
                    trigger: touch,
                    player: remote_id,
                    event_name: "touch".into(),
                },
                TriggerEventFire {
                    trigger: use_trigger,
                    player: local_id,
                    event_name: "use".into(),
                },
                TriggerEventFire {
                    trigger: use_trigger,
                    player: remote_id,
                    event_name: "use".into(),
                },
            ],
            "level {level} preserves both local Use and remote use_pressed activation"
        );
        assert_eq!(system.occupancy(touch), 2);
        assert!(
            registry
                .get_component::<KinematicMoverComponent>(mover)
                .expect("trigger target mover remains live")
                .started,
            "touch and Use paths still reach the host-only mover command after level {level}"
        );

        // The session persists while its level-scoped registry and bridge are
        // replaced. The fresh level must start without old occupancy, then admit
        // both local and remote activators again.
        system.clear();
    }

    assert_eq!(
        system.recorded_gate_fires().len(),
        8,
        "two levels each run touch and Use through local and remote player identities"
    );
}

#[test]
fn trigger_command_starts_targeted_mover_and_gate_is_sole_fire_path() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Use,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    let mover = spawn_mover(&mut registry);
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let id = PlayerId::Local(player);
    let players = [AuthoritativePlayer { id, pawn: player }];
    let mut system = TriggerSystem::default();

    tick(&mut system, &mut registry, &bridge, &players, &[(id, true)]);
    let mover_state = registry
        .get_component::<KinematicMoverComponent>(mover)
        .unwrap();
    assert!(
        mover_state.started,
        "trigger command must mutate the mover phase"
    );
    assert_eq!(
        system.recorded_gate_fires(),
        &[id],
        "only the gate records fires and receives its activator"
    );
    let mut mover_ticks = crate::kinematic_mover::MoverTickStateTable::default();
    crate::kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut mover_ticks, DT);
    assert!(
        registry
            .get_component::<Transform>(mover)
            .unwrap()
            .position
            .x
            > 0.0,
        "the trigger command must produce observed mover motion on the next fixed tick"
    );
}

#[test]
fn trigger_kvp_start_retriggers_an_opened_auto_close_hold() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    let mover = spawn_mover(&mut registry);
    let mut mover_phase = registry
        .get_component::<KinematicMoverComponent>(mover)
        .expect("mover component attached")
        .clone();
    mover_phase.auto_close_ms = 200.0;
    mover_phase.segment_index = 1;
    mover_phase.completed = true;
    registry.set_component(mover, mover_phase).unwrap();

    let timers = MoverAutoCloseTimers::default();
    timers.set_enabled(true);
    timers.arm_opened_termini(
        &mut registry,
        &[(crate::kinematic_mover::MoverEventKind::Opened, 1)],
    );
    timers.tick(&mut registry, DT);
    assert_eq!(timers.remaining_ms(mover), Some(200.0));
    timers.tick(&mut registry, DT);
    assert!(timers.remaining_ms(mover).unwrap() < 200.0);

    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let id = PlayerId::Local(player);
    let players = [AuthoritativePlayer { id, pawn: player }];
    let mut system = TriggerSystem::with_mover_auto_close_timers(timers.clone());
    tick(&mut system, &mut registry, &bridge, &players, &[]);
    timers.tick(&mut registry, DT);

    assert_eq!(
        timers.remaining_ms(mover),
        Some(200.0),
        "the direct trigger-volume KVP command must reset the open hold without consuming its same fixed tick, even when Start is phase-idempotent"
    );
}

#[test]
fn occupancy_tracks_each_overlapping_player_independently_of_activation_gate() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        false,
    );
    let first = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let second = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let players = [
        AuthoritativePlayer {
            id: PlayerId::Local(first),
            pawn: first,
        },
        AuthoritativePlayer {
            id: PlayerId::Remote(7),
            pawn: second,
        },
    ];
    let mut system = TriggerSystem::default();

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(system.occupancy(trigger), 0);
    set_player_position(&mut registry, first, Vec3::new(0.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(system.occupancy(trigger), 1);
    set_player_position(&mut registry, second, Vec3::new(0.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(system.occupancy(trigger), 2);
    set_player_position(&mut registry, first, Vec3::new(4.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(system.occupancy(trigger), 1);
    set_player_position(&mut registry, second, Vec3::new(4.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(system.occupancy(trigger), 0);
}

#[test]
fn fire_report_orders_enter_and_paired_exit_edges_by_trigger_then_player() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let first_trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    let second_trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, first_trigger, "first_enter", "first_exit");
    set_event_names(&mut registry, second_trigger, "second_enter", "second_exit");
    let local = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let remote = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let local_id = PlayerId::Local(local);
    let remote_id = PlayerId::Remote(7);
    let players = [
        AuthoritativePlayer {
            id: remote_id,
            pawn: remote,
        },
        AuthoritativePlayer {
            id: local_id,
            pawn: local,
        },
    ];
    let mut system = TriggerSystem::default();

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, local, Vec3::new(0.0, 1.0, 0.0));
    set_player_position(&mut registry, remote, Vec3::new(0.0, 1.0, 0.0));
    let enters = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(
        enters.enters(),
        vec![
            TriggerEventFire {
                trigger: first_trigger,
                player: local_id,
                event_name: "first_enter".into(),
            },
            TriggerEventFire {
                trigger: first_trigger,
                player: remote_id,
                event_name: "first_enter".into(),
            },
            TriggerEventFire {
                trigger: second_trigger,
                player: local_id,
                event_name: "second_enter".into(),
            },
            TriggerEventFire {
                trigger: second_trigger,
                player: remote_id,
                event_name: "second_enter".into(),
            },
        ]
    );

    set_player_position(&mut registry, local, Vec3::new(4.0, 1.0, 0.0));
    set_player_position(&mut registry, remote, Vec3::new(4.0, 1.0, 0.0));
    let exits = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(
        exits.exits(),
        vec![
            TriggerEventFire {
                trigger: first_trigger,
                player: local_id,
                event_name: "first_exit".into(),
            },
            TriggerEventFire {
                trigger: first_trigger,
                player: remote_id,
                event_name: "first_exit".into(),
            },
            TriggerEventFire {
                trigger: second_trigger,
                player: local_id,
                event_name: "second_exit".into(),
            },
            TriggerEventFire {
                trigger: second_trigger,
                player: remote_id,
                event_name: "second_exit".into(),
            },
        ]
    );
}

#[test]
fn same_tick_enter_and_exit_share_one_trigger_player_order() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let local = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let remote = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let local_id = PlayerId::Local(local);
    let remote_id = PlayerId::Remote(7);
    let players = [
        AuthoritativePlayer {
            id: remote_id,
            pawn: remote,
        },
        AuthoritativePlayer {
            id: local_id,
            pawn: local,
        },
    ];
    let mut system = TriggerSystem::default();

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, local, Vec3::new(4.0, 1.0, 0.0));
    set_player_position(&mut registry, remote, Vec3::new(0.0, 1.0, 0.0));

    let report = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(
        report.fires,
        vec![
            TriggerEvent {
                fire: TriggerEventFire {
                    trigger,
                    player: local_id,
                    event_name: "left".into(),
                },
                edge: TriggerEventEdge::Exit,
            },
            TriggerEvent {
                fire: TriggerEventFire {
                    trigger,
                    player: remote_id,
                    event_name: "entered".into(),
                },
                edge: TriggerEventEdge::Enter,
            },
        ],
        "enter and paired-exit callbacks share the canonical (trigger, player) stream"
    );
}

#[test]
fn missing_player_snapshot_removes_occupancy_and_fires_paired_exit() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let mut system = TriggerSystem::default();

    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).enters(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "entered".into(),
        }]
    );
    assert_eq!(system.occupancy(trigger), 1);

    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &[], &[]).exits(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "left".into(),
        }]
    );
    assert_eq!(system.occupancy(trigger), 0);
    assert_eq!(system.recorded_paired_exits(), &[player_id]);
}

#[test]
fn effective_occupancy_excludes_corpses_and_exit_excludes_the_leaver() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let live = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let corpse = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let live_id = PlayerId::Local(live);
    let corpse_id = PlayerId::Remote(9);
    let players = [
        AuthoritativePlayer {
            id: live_id,
            pawn: live,
        },
        AuthoritativePlayer {
            id: corpse_id,
            pawn: corpse,
        },
    ];
    let alive = HashSet::from([live_id]);
    let mut system = TriggerSystem::default();
    let mut observed = Vec::new();

    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &HashSet::new(),
        },
        |event, occupancy, occupants, _| {
            observed.push((event.clone(), occupancy, occupants.to_vec()))
        },
    );
    assert_eq!(observed.len(), 2, "both physical entries still emit edges");
    assert!(
        observed
            .iter()
            .all(|(_, occupancy, occupants)| *occupancy == 1 && occupants == &[live_id])
    );

    set_player_position(&mut registry, live, Vec3::new(4.0, 1.0, 0.0));
    observed.clear();
    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &HashSet::new(),
        },
        |event, occupancy, occupants, _| {
            observed.push((event.clone(), occupancy, occupants.to_vec()))
        },
    );
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].0.edge, TriggerEventEdge::Exit);
    assert_eq!(observed[0].0.fire.player, live_id);
    assert_eq!(
        observed[0].1, 0,
        "the exiting live pawn is removed before dispatch"
    );
    assert!(observed[0].2.is_empty());
}

#[test]
fn edge_occupants_include_simultaneous_entries_and_remain_frozen_across_ticks() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let first = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let second = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let third = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let ids = [
        PlayerId::Local(first),
        PlayerId::Remote(2),
        PlayerId::Remote(3),
    ];
    let players = [
        AuthoritativePlayer {
            id: ids[0],
            pawn: first,
        },
        AuthoritativePlayer {
            id: ids[1],
            pawn: second,
        },
        AuthoritativePlayer {
            id: ids[2],
            pawn: third,
        },
    ];
    let alive = HashSet::from(ids);
    let mut system = TriggerSystem::default();
    let mut captured = Vec::new();
    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &HashSet::new(),
        },
        |event, count, occupants, _| {
            assert_eq!(count, occupants.len());
            captured.push((event.edge, occupants.to_vec()));
        },
    );
    assert_eq!(
        captured,
        vec![
            (TriggerEventEdge::Enter, vec![ids[0], ids[1]]),
            (TriggerEventEdge::Enter, vec![ids[0], ids[1]]),
        ]
    );
    // A later catch-up tick removes the first player and adds the third.
    // Both edges see the completed tick occupancy, while queued snapshots
    // still retain the first tick's recipients for the frame-end drain.
    set_player_position(&mut registry, first, Vec3::new(4.0, 1.0, 0.0));
    set_player_position(&mut registry, third, Vec3::new(0.0, 1.0, 0.0));
    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &HashSet::new(),
        },
        |event, count, occupants, _| {
            assert_eq!(count, occupants.len());
            captured.push((event.edge, occupants.to_vec()));
        },
    );
    assert_eq!(
        captured,
        vec![
            (TriggerEventEdge::Enter, vec![ids[0], ids[1]]),
            (TriggerEventEdge::Enter, vec![ids[0], ids[1]]),
            (TriggerEventEdge::Exit, vec![ids[1], ids[2]]),
            (TriggerEventEdge::Enter, vec![ids[1], ids[2]]),
        ]
    );
}

#[test]
fn script_bound_edge_dispatches_enter_and_exit_with_no_kvp_event_name() {
    // AC 6: a volume bound only through the script path (a trigger member's `on`)
    // carries no on_fire/on_exit KVP, yet the widened enter/exit dispatch
    // gates must still fire because `bound_edges` holds its (volume, edge).
    // The existing tests either name their events or pass an empty
    // `bound_edges`, so this widened branch is otherwise unproven.
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    // spawn_trigger leaves on_fire/on_exit empty — the KVP-less binding case.
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let alive = HashSet::from([player_id]);
    let bound_edges = HashSet::from([
        (trigger, TriggerEventEdge::Enter),
        (trigger, TriggerEventEdge::Exit),
    ]);
    let mut system = TriggerSystem::default();

    // Enter: the player starts inside, so tick one produces the rising edge.
    let mut observed = Vec::new();
    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &bound_edges,
        },
        |event, _, _, _| observed.push(event.clone()),
    );
    assert_eq!(
        observed.len(),
        1,
        "a script-bound edge dispatches even with no KVP event name"
    );
    assert_eq!(observed[0].edge, TriggerEventEdge::Enter);
    assert_eq!(observed[0].fire.player, player_id);
    assert!(
        observed[0].fire.event_name.is_empty(),
        "a script-only binding carries an empty event name"
    );

    // Exit: leaving fires the paired exit through the same widened gate.
    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    observed.clear();
    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &bound_edges,
        },
        |event, _, _, _| observed.push(event.clone()),
    );
    assert_eq!(observed.len(), 1, "the paired exit dispatches too");
    assert_eq!(observed[0].edge, TriggerEventEdge::Exit);
    assert!(
        observed[0].fire.event_name.is_empty(),
        "the script-bound exit also carries an empty event name"
    );
}

#[test]
fn closet_reveal_enter_edge_dispatches_door_and_enemy_release_reactions() {
    // E18-C containment is authored as a trigger-event fan-out. One
    // script-bound enter edge must dispatch both named reaction bodies;
    // neither is nested in the other as a sequence step.
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    registry
        .set_tags(trigger, vec!["closet_reveal_plate".into()])
        .expect("tag reveal plate");
    let mover = spawn_mover(&mut registry);
    registry
        .set_tags(mover, vec!["closet_door".into()])
        .expect("tag closet door");
    let enemy = registry.spawn(Transform::default());
    registry
        .set_tags(enemy, vec!["closet_enemies".into()])
        .expect("tag closet enemy");
    let graph = BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                BehaviorActivityDescriptor {
                    pose: None,
                    stagger_to: None,
                    passes: None,
                    sound: None,
                    animation: Some("idle".to_string()),
                    motion: Some(MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    facing: None,
                    reposition: None,
                    attack_turn: None,
                    speed: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 3.0,
    };
    attach_brain_graph(&mut registry, enemy, &graph).expect("attach closed closet brain");
    let mut brain = registry
        .get_component::<BrainComponent>(enemy)
        .expect("closet brain attached")
        .clone();
    brain.aggro_armed = false;
    registry
        .set_component(enemy, brain)
        .expect("close closet aggro gate");

    let mut data = DataRegistry::new();
    data.populate_level_with_trigger_events(
        vec![
            NamedReaction {
                name: "closet.openDoor".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "moverStart".into(),
                    target: None,
                    kind: None,
                    tag: Some("closet_door".into()),
                    args: serde_json::json!({}),
                    on_complete: None,
                }),
            },
            NamedReaction {
                name: "closet.releaseCloset".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "updateNpcState".into(),
                    target: None,
                    kind: None,
                    tag: Some("closet_enemies".into()),
                    args: serde_json::json!({ "aggro": true }),
                    on_complete: None,
                }),
            },
        ],
        Vec::new(),
        vec![VolumeTriggerEventDescriptor {
            trigger,
            event: "enter".into(),
            fire: vec!["closet.openDoor".into(), "closet.releaseCloset".into()],
        }],
        Vec::new(),
        &[],
    );
    let script_ctx = ScriptCtx::new();
    let mut bindings = crate::trigger_bindings::TriggerBindingTable::build_with_script_ctx(
        &registry,
        &data,
        &script_ctx,
    );
    bindings.install_manifest_events(&registry, &data, &script_ctx);
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let alive = HashSet::from([player_id]);
    let bound_edges = bindings.bound_edges().clone();
    let mut system = TriggerSystem::default();
    let mut dispatched = Vec::new();

    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &bound_edges,
        },
        |event, _, _, registry| {
            let execution = bindings.execute_with_script_ctx(
                event.fire.trigger,
                event.edge,
                registry,
                &script_ctx,
                &crate::trigger_commands::TriggerFireContext::default(),
            );
            dispatched.extend(execution.commands);
        },
    );

    assert_eq!(
        dispatched,
        vec![
            crate::trigger_bindings::BoundTriggerCommandKind::Mover,
            crate::trigger_bindings::BoundTriggerCommandKind::UpdateNpcState,
        ],
        "one reveal enter edge fans out to the door and aggro-release reactions"
    );
    assert!(
        registry
            .get_component::<KinematicMoverComponent>(mover)
            .expect("closet door mover attached")
            .started,
        "openDoor dispatch starts the closet door"
    );
    assert!(
        registry
            .get_component::<BrainComponent>(enemy)
            .expect("closet brain attached")
            .aggro_armed,
        "releaseCloset dispatch opens the foundation-owned aggro gate"
    );
}

#[test]
fn spawn_from_spawner_trigger_event_materializes_runtime_enemies() {
    // AC 2 fixed-tick coverage: a `spawnFromSpawner` reaction bound to a
    // trigger's enter edge must materialize enemies through the real
    // `TriggerBindingTable` execute path — verifying the `spawn_context`
    // threading, not just a direct `spawn_from_spawner_*` call. The table is
    // built with a non-Default spawn context (via
    // `build_with_script_ctx_and_diagnostics`) that both holds runtime-spawn
    // authority and has the archetype resolved in its descriptor cache; a
    // `Default` spawn context would silently no-op the materialization.
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    registry
        .set_tags(trigger, vec!["ambush_plate".into()])
        .expect("tag ambush plate");

    // A resolved spawner tagged so the fire-time `spawnFromSpawner` tag target
    // resolves against the Spawner column.
    const SPAWN_COUNT: u32 = 3;
    let spawner = registry.spawn(Transform::default());
    registry
        .set_tags(spawner, vec!["ambush_spawner".into()])
        .expect("tag ambush spawner");
    registry
        .set_component(
            spawner,
            postretro_entities::components::spawner::SpawnerComponent {
                archetype_name: "cultist".into(),
                count: SPAWN_COUNT,
                spawned_tags: Vec::new(),
                resolved: true,
            },
        )
        .expect("attach resolved spawner");

    let mut data = DataRegistry::new();
    data.populate_level_with_trigger_events(
        vec![NamedReaction {
            name: "ambush.spawn".into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "spawnFromSpawner".into(),
                target: None,
                kind: None,
                tag: Some("ambush_spawner".into()),
                args: serde_json::json!({}),
                on_complete: None,
            }),
        }],
        Vec::new(),
        vec![VolumeTriggerEventDescriptor {
            trigger,
            event: "enter".into(),
            fire: vec!["ambush.spawn".into()],
        }],
        Vec::new(),
        &[],
    );

    // Resolved archetype + runtime-spawn authority (default). A Default
    // context has neither the descriptor cache nor level data, so the bound
    // command would drop the spawn even though it dispatches.
    let spawn_context = crate::spawner::SpawnContext::default();
    spawn_context.replace_level_data(
        [(
            "cultist".to_string(),
            crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
                "cultist",
            ),
        )]
        .into_iter()
        .collect(),
        Some(postretro_foundation::NavAgentParams {
            radius: 0.4,
            height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
        }),
    );

    let script_ctx = ScriptCtx::new();
    let mut bindings =
        crate::trigger_bindings::TriggerBindingTable::build_with_script_ctx_and_diagnostics(
            &registry,
            &data,
            &script_ctx,
            MoverCommandDiagnostics::default(),
            spawn_context,
        );
    bindings.install_manifest_events(&registry, &data, &script_ctx);
    assert_eq!(
        registry.iter_with_kind(ComponentKind::Brain).count(),
        0,
        "no AI entities exist before the trigger fires"
    );

    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let alive = HashSet::from([player_id]);
    let bound_edges = bindings.bound_edges().clone();
    let mut system = TriggerSystem::default();

    system.run_authoritative_tick_with_dispatch(
        &mut registry,
        &bridge,
        TriggerTickInputs {
            players: &players,
            use_pressed: &HashMap::new(),
            tick_dt: DT,
        },
        TriggerDispatchInputs {
            alive_players: &alive,
            bound_edges: &bound_edges,
        },
        |event, _, _, registry| {
            bindings.execute_with_script_ctx(
                event.fire.trigger,
                event.edge,
                registry,
                &script_ctx,
                &crate::trigger_commands::TriggerFireContext::default(),
            );
        },
    );

    assert_eq!(
        registry.iter_with_kind(ComponentKind::Brain).count(),
        SPAWN_COUNT as usize,
        "the enter-edge spawnFromSpawner reaction materialized the spawner's enemies through the real execute path"
    );
}

#[test]
fn duplicate_player_ids_and_despawned_triggers_leave_no_stale_occupancy() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let duplicate_players = [
        AuthoritativePlayer {
            id: player_id,
            pawn: player,
        },
        AuthoritativePlayer {
            id: player_id,
            pawn: player,
        },
    ];
    let mut system = TriggerSystem::default();

    let report = tick(&mut system, &mut registry, &bridge, &duplicate_players, &[]);
    assert_eq!(report.enters().len(), 1, "one PlayerId has one edge");
    assert_eq!(system.occupancy(trigger), 1);

    registry.despawn(trigger).expect("trigger can despawn");
    let report = tick(&mut system, &mut registry, &bridge, &duplicate_players, &[]);
    assert!(report.fires.is_empty());
    assert_eq!(
        system.occupancy(trigger),
        0,
        "despawn drops occupancy and paired-exit bookkeeping instead of retaining a stale trigger ID"
    );
}

#[test]
fn suppressed_enter_does_not_produce_a_paired_exit() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let first = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let second = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let players = [
        AuthoritativePlayer {
            id: PlayerId::Local(first),
            pawn: first,
        },
        AuthoritativePlayer {
            id: PlayerId::Remote(7),
            pawn: second,
        },
    ];
    let mut system = TriggerSystem::default();

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, first, Vec3::new(0.0, 1.0, 0.0));
    let fired = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(fired.enters().len(), 1);
    set_player_position(&mut registry, second, Vec3::new(0.0, 1.0, 0.0));
    let suppressed = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert!(suppressed.enters().is_empty());
    set_player_position(&mut registry, second, Vec3::new(4.0, 1.0, 0.0));
    let exited = tick(&mut system, &mut registry, &bridge, &players, &[]);

    assert!(exited.exits().is_empty());
    assert!(system.recorded_paired_exits().is_empty());
}

#[test]
fn paired_exit_survives_once_rearm_and_mid_stand_disarm() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let once = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    let rearming = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        1_000.0,
        true,
    );
    let disarmed = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    set_event_names(&mut registry, once, "once_enter", "once_exit");
    set_event_names(&mut registry, rearming, "rearm_enter", "rearm_exit");
    set_event_names(&mut registry, disarmed, "disarm_enter", "disarm_exit");
    let player = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let mut system = TriggerSystem::default();

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
    let entered = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(entered.enters().len(), 3);
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(once)
            .expect("once trigger exists")
            .latched
    );
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(rearming)
            .expect("rearming trigger exists")
            .rearm_remaining_ms
            > 0.0
    );
    disarm_trigger_targets(&mut registry, &[disarmed], &Default::default());

    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    let exited = tick(&mut system, &mut registry, &bridge, &players, &[]);
    assert_eq!(
        exited.exits(),
        vec![
            TriggerEventFire {
                trigger: once,
                player: player_id,
                event_name: "once_exit".into(),
            },
            TriggerEventFire {
                trigger: rearming,
                player: player_id,
                event_name: "rearm_exit".into(),
            },
            TriggerEventFire {
                trigger: disarmed,
                player: player_id,
                event_name: "disarm_exit".into(),
            },
        ]
    );
    assert_eq!(system.recorded_paired_exits(), &[player_id; 3]);
}

#[test]
fn arm_and_disarm_primitives_control_enter_firing_and_reset_gate_state() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        false,
    );
    set_event_names(&mut registry, trigger, "armed_enter", "armed_exit");
    let player = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let mut system = TriggerSystem::default();
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_trigger_reaction_primitives(&mut reactions, Default::default());

    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .enters()
            .is_empty(),
        "a disabled-on-spawn trigger must not fire"
    );
    assert!(
        reactions
            .dispatch(
                "armTrigger",
                &mut registry,
                &[trigger],
                &serde_json::Value::Null
            )
            .expect("arm dispatch succeeds")
    );
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("trigger exists")
            .armed
    );
    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).enters(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "armed_enter".into(),
        }],
        "arming reopens a Touch trigger for a player already standing in it"
    );
    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).exits(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "armed_exit".into(),
        }]
    );
    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .exits()
            .is_empty(),
        "the reactivated entry retains one paired exit"
    );

    let mut component = registry
        .get_component::<TriggerVolumeComponent>(trigger)
        .expect("trigger exists")
        .clone();
    component.armed = false;
    component.rearm_remaining_ms = 250.0;
    registry
        .set_component(trigger, component)
        .expect("seed arm reset state");
    assert!(
        reactions
            .dispatch(
                "armTrigger",
                &mut registry,
                &[trigger],
                &serde_json::Value::Null
            )
            .expect("second arm dispatch succeeds")
    );
    let component = registry
        .get_component::<TriggerVolumeComponent>(trigger)
        .expect("trigger exists");
    assert!(component.armed);
    assert!(!component.latched);
    assert_eq!(component.rearm_remaining_ms, 0.0);

    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .enters()
            .len(),
        1,
        "arming clears a once latch and enables a new enter"
    );

    assert!(
        reactions
            .dispatch(
                "disarmTrigger",
                &mut registry,
                &[trigger],
                &serde_json::Value::Null
            )
            .expect("disarm dispatch succeeds")
    );
    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
    set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .enters()
            .is_empty(),
        "disarming prevents later enter fires"
    );
}

#[test]
fn rearming_touch_while_standing_refires_once_and_keeps_one_paired_exit() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    set_event_names(&mut registry, trigger, "entered", "left");
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let mut system = TriggerSystem::default();
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_trigger_reaction_primitives(&mut reactions, Default::default());

    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).enters(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "entered".into(),
        }]
    );
    reactions
        .dispatch(
            "armTrigger",
            &mut registry,
            &[trigger],
            &serde_json::Value::Null,
        )
        .expect("re-arm dispatch succeeds");
    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).enters(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "entered".into(),
        }],
        "a puzzle-reset arm re-fires a player still on the plate"
    );

    set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
    assert_eq!(
        tick(&mut system, &mut registry, &bridge, &players, &[]).exits(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "left".into(),
        }],
        "repeated entries share one paired exit while occupancy is continuous"
    );
    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .exits()
            .is_empty(),
        "leaving cannot consume the same pair twice"
    );
}

#[test]
fn arming_use_while_standing_still_requires_a_press() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Use,
        TriggerFireMode::Once,
        0.0,
        false,
    );
    set_event_names(&mut registry, trigger, "used", "released");
    let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
    let player_id = PlayerId::Local(player);
    let players = [AuthoritativePlayer {
        id: player_id,
        pawn: player,
    }];
    let mut system = TriggerSystem::default();
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_trigger_reaction_primitives(&mut reactions, Default::default());

    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .enters()
            .is_empty(),
        "a disabled Use trigger ignores an overlapping player"
    );
    reactions
        .dispatch(
            "armTrigger",
            &mut registry,
            &[trigger],
            &serde_json::Value::Null,
        )
        .expect("arm dispatch succeeds");
    assert!(
        tick(&mut system, &mut registry, &bridge, &players, &[])
            .enters()
            .is_empty(),
        "arming a Use trigger does not synthesize a touch entry"
    );
    assert_eq!(
        tick(
            &mut system,
            &mut registry,
            &bridge,
            &players,
            &[(player_id, true)],
        )
        .enters(),
        vec![TriggerEventFire {
            trigger,
            player: player_id,
            event_name: "used".into(),
        }]
    );
}

#[test]
fn trigger_primitives_skip_non_trigger_targets() {
    let mut registry = EntityRegistry::new();
    let mut bridge = TriggerVolumeBridge::new();
    let trigger = spawn_trigger(
        &mut registry,
        &mut bridge,
        TriggerActivation::Touch,
        TriggerFireMode::Once,
        0.0,
        true,
    );
    let non_trigger = registry.spawn(Transform::default());
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_trigger_reaction_primitives(&mut reactions, Default::default());

    assert!(
        reactions
            .dispatch(
                "disarmTrigger",
                &mut registry,
                &[trigger, non_trigger],
                &serde_json::Value::Null
            )
            .expect("disarm dispatch succeeds")
    );
    assert!(
        !registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("trigger retained its component")
            .armed
    );
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(non_trigger)
            .is_err(),
        "a mixed tag target must not gain trigger state"
    );
}

#[test]
fn sequenced_trigger_primitives_apply_arm_and_disarm_to_the_resolved_id() {
    let ctx = ScriptCtx::new();
    let trigger = {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    "tripwire".into(),
                    String::new(),
                    String::new(),
                    TriggerCommand::Start,
                    TriggerFireMode::Once,
                    100.0,
                    true,
                ),
            )
            .unwrap();
        id
    };
    let mut sequences = SequencedPrimitiveRegistry::new();
    register_sequenced_trigger_primitives(&mut sequences, ctx.clone(), Default::default());
    assert!(sequences.contains("armTrigger"));
    assert!(sequences.contains("disarmTrigger"));

    sequences.get("disarmTrigger").unwrap()(trigger, &serde_json::json!({}))
        .expect("disarm sequenced primitive succeeds");
    assert!(
        !ctx.registry
            .borrow()
            .get_component::<TriggerVolumeComponent>(trigger)
            .unwrap()
            .armed
    );

    {
        let mut registry = ctx.registry.borrow_mut();
        let mut state = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .unwrap()
            .clone();
        state.latched = true;
        state.rearm_remaining_ms = 50.0;
        registry.set_component(trigger, state).unwrap();
    }
    sequences.get("armTrigger").unwrap()(trigger, &serde_json::json!({}))
        .expect("arm sequenced primitive succeeds");
    let state = ctx
        .registry
        .borrow()
        .get_component::<TriggerVolumeComponent>(trigger)
        .unwrap()
        .clone();
    assert!(state.armed);
    assert!(!state.latched);
    assert_eq!(state.rearm_remaining_ms, 0.0);
}
