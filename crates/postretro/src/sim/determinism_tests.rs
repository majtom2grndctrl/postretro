// Determinism coverage for the headless fixed-tick seam.
// See: context/plans/in-progress/M15--p0-headless-sim-seam/index.md

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glam::{Vec2, Vec3};
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;
use proptest::prelude::*;

use super::{SimCommand, TickEvents, simulate_tick};
use crate::collision::CollisionWorld;
use crate::movement::MovementInput;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::FireButtonState;
use postretro_entities::components::brain::{AiStateMap, AiTuning, BrainComponent, LogicalState};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, FireMode, ForgivenessParams, GroundParams,
    PlayerMovementComponent, PlayerMovementDescriptor, ResolutionMode, SpeedParams,
    WeaponDescriptor,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

const TICK_COUNT: usize = 600;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const POSITION_EPSILON: f32 = 0.001;
const VELOCITY_EPSILON: f32 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Alpha,
    Beta,
}

impl Role {
    fn start_position(self) -> Vec3 {
        match self {
            Role::Alpha => Vec3::new(-2.0, 1.21, 0.0),
            Role::Beta => Vec3::new(2.0, 1.21, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SpawnOrder {
    AlphaThenBeta,
    BetaThenAlpha,
}

impl SpawnOrder {
    fn roles(self) -> [Role; 2] {
        match self {
            SpawnOrder::AlphaThenBeta => [Role::Alpha, Role::Beta],
            SpawnOrder::BetaThenAlpha => [Role::Beta, Role::Alpha],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RecordedCommand {
    wish_dir: Vec2,
    jump_pressed: bool,
    dash_pressed: bool,
    running: bool,
    crouch_intent: bool,
    facing_yaw: f32,
    fire_pressed: bool,
    fire_active: bool,
}

impl RecordedCommand {
    fn to_sim_command(self) -> SimCommand {
        SimCommand {
            movement: MovementInput {
                wish_dir: self.wish_dir,
                jump_pressed: self.jump_pressed,
                dash_pressed: self.dash_pressed,
                running: self.running,
                crouch_intent: self.crouch_intent,
                facing_yaw: self.facing_yaw,
            },
            fire_button: FireButtonState {
                pressed: self.fire_pressed,
                active: self.fire_active,
            },
        }
    }

    fn to_post_movement_command(self) -> super::PostMovementCommand {
        super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, -20.0),
            aim_direction: Vec3::new(self.facing_yaw.sin(), 0.0, -self.facing_yaw.cos())
                .normalize(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PawnOutcome {
    position: Vec3,
    velocity: Vec3,
}

#[derive(Debug)]
struct SimRun {
    pawns: Vec<(Role, PawnOutcome)>,
    selected_player_health: f32,
    enemy_state: LogicalState,
    events: Vec<TickEvents>,
}

struct SimHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    active_wieldable: EntityId,
    progress: ProgressTracker,
    ai_warned: HashSet<String>,
    role_ids: Vec<(Role, EntityId)>,
    selected_player: EntityId,
    enemy: EntityId,
}

impl SimHarness {
    fn new(spawn_order: SpawnOrder) -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let mut role_ids = Vec::new();
        let (active_wieldable, enemy) = {
            let mut registry = registry.borrow_mut();
            for role in spawn_order.roles() {
                let id = spawn_player(&mut registry, role.start_position());
                if role == Role::Alpha {
                    // Phase 0 still has one local sim command, not keyed
                    // per-pawn commands. Mark Alpha as the local pawn so
                    // full-seam AI/health paths remain order-observable while
                    // comparing outcomes by stable test role.
                    registry
                        .mark_local_player_pawn(id)
                        .expect("alpha role can be marked as local player");
                }
                role_ids.push((role, id));
            }
            let enemy = spawn_enemy(&mut registry, Vec3::new(-1.0, 1.0, 0.0));
            (spawn_weapon(&mut registry), enemy)
        };
        let selected_player = role_ids
            .iter()
            .find_map(|(role, id)| (*role == Role::Alpha).then_some(*id))
            .expect("alpha role is always spawned");

        Self {
            registry,
            world: floor_world(),
            hit_zones: HitZoneStore::new(),
            active_wieldable,
            progress: ProgressTracker::new(),
            ai_warned: HashSet::new(),
            role_ids,
            selected_player,
            enemy,
        }
    }

    fn tick(&mut self, command: RecordedCommand) -> TickEvents {
        let sim_command = command.to_sim_command();
        simulate_tick(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            GRAVITY,
            Some(self.active_wieldable),
            0.0,
            &mut self.progress,
            &mut self.ai_warned,
            &sim_command,
            |_| command.to_post_movement_command(),
            DT,
        )
    }

    fn role_outcomes(&self) -> Vec<(Role, PawnOutcome)> {
        let registry = self.registry.borrow();
        let mut outcomes = self
            .role_ids
            .iter()
            .map(|(role, id)| {
                let transform = *registry
                    .get_component::<Transform>(*id)
                    .expect("role entity must keep its transform");
                let movement = registry
                    .get_component::<PlayerMovementComponent>(*id)
                    .expect("role entity must keep its movement component");
                (
                    *role,
                    PawnOutcome {
                        position: transform.position,
                        velocity: movement.velocity,
                    },
                )
            })
            .collect::<Vec<_>>();
        outcomes.sort_by_key(|(role, _)| *role);
        outcomes
    }

    fn selected_player_health(&self) -> f32 {
        self.registry
            .borrow()
            .get_component::<HealthComponent>(self.selected_player)
            .expect("selected player keeps health")
            .current
    }

    fn enemy_state(&self) -> LogicalState {
        self.registry
            .borrow()
            .get_component::<BrainComponent>(self.enemy)
            .expect("enemy keeps brain")
            .state
    }
}

fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            PlayerMovementComponent::from_descriptor(&player_descriptor()),
        )
        .expect("player movement component should attach");
    registry
        .set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: Default::default(),
            },
        )
        .expect("player health component should attach");
    id
}

fn spawn_enemy(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            BrainComponent {
                state: LogicalState::Idle,
                attack_cooldown_remaining_ms: 0.0,
                think_stride_counter: 0,
                death_despawn_remaining_ms: None,
                tuning: AiTuning {
                    detection_range: 8.0,
                    attack_range: 2.0,
                    leash_range: 12.0,
                    attack_damage: 7.0,
                    attack_cooldown_ms: 1000.0,
                    move_speed: 0.0,
                    death_despawn_ms: 1000.0,
                    states: AiStateMap {
                        idle: "idle".to_string(),
                        alert: "alert".to_string(),
                        attack: "attack".to_string(),
                        death: "death".to_string(),
                    },
                },
            },
        )
        .expect("enemy brain component should attach");
    registry
        .set_component(
            id,
            HealthComponent {
                max: 20.0,
                current: 20.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: Default::default(),
            },
        )
        .expect("enemy health component should attach");
    id
}

fn spawn_target(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            HealthComponent {
                max: 20.0,
                current: 20.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                zone_multipliers: Default::default(),
            },
        )
        .expect("target health component should attach");
    id
}

fn spawn_weapon(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            WeaponComponent::from_descriptor(&WeaponDescriptor {
                damage: 10.0,
                range: 30.0,
                cooldown_ms: 80.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
            }),
        )
        .expect("weapon component should attach");
    id
}

fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
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
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        view_feel: None,
    }
}

fn floor_world() -> CollisionWorld {
    let points = vec![
        Point::new(-500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, 500.0),
        Point::new(-500.0, 0.0, 500.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2]];
    CollisionWorld {
        mesh: TriMesh::new(points, triangles),
        isometry: Isometry::identity(),
    }
}

fn fixed_command_stream() -> Vec<RecordedCommand> {
    (0..TICK_COUNT)
        .map(|tick| {
            let phase = tick % 120;
            let fire_pressed = matches!(tick, 5 | 180 | 360 | 540);
            RecordedCommand {
                wish_dir: if phase < 45 {
                    Vec2::new(0.25, 1.0)
                } else if phase < 80 {
                    Vec2::new(-0.5, 0.2)
                } else {
                    Vec2::ZERO
                },
                jump_pressed: matches!(tick, 30 | 210 | 390),
                dash_pressed: matches!(tick, 90 | 270 | 450),
                running: phase < 70,
                crouch_intent: (300..360).contains(&tick),
                facing_yaw: if tick < 300 { 0.0 } else { 0.35 },
                fire_pressed,
                fire_active: fire_pressed || matches!(tick, 6 | 181 | 361 | 541),
            }
        })
        .collect()
}

fn run_stream(commands: &[RecordedCommand], spawn_order: SpawnOrder) -> SimRun {
    let mut harness = SimHarness::new(spawn_order);
    let events = commands
        .iter()
        .copied()
        .map(|command| harness.tick(command))
        .collect();
    SimRun {
        pawns: harness.role_outcomes(),
        selected_player_health: harness.selected_player_health(),
        enemy_state: harness.enemy_state(),
        events,
    }
}

fn assert_runs_match(actual: &SimRun, expected: &SimRun) {
    assert_eq!(
        actual.events, expected.events,
        "stage-grouped event names must match exactly"
    );
    assert_eq!(
        actual.enemy_state, expected.enemy_state,
        "AI state must resolve from the same selected local pawn label"
    );
    // Exact equality is safe here: health deltas are integer damage values (10.0)
    // applied via integer-path arithmetic with no per-frame interpolation, so
    // deterministic runs must produce bit-identical results.
    assert_eq!(
        actual.selected_player_health, expected.selected_player_health,
        "selected player health must match exactly"
    );
    assert_eq!(
        actual.pawns.len(),
        expected.pawns.len(),
        "same role count expected"
    );
    for ((actual_role, actual), (expected_role, expected)) in
        actual.pawns.iter().zip(expected.pawns.iter())
    {
        assert_eq!(actual_role, expected_role, "roles must compare by label");
        assert_vec3_within(
            actual.position,
            expected.position,
            POSITION_EPSILON,
            "position",
        );
        assert_vec3_within(
            actual.velocity,
            expected.velocity,
            VELOCITY_EPSILON,
            "velocity",
        );
    }
}

fn assert_vec3_within(actual: Vec3, expected: Vec3, epsilon: f32, label: &str) {
    let delta = (actual - expected).abs();
    assert!(
        delta.x <= epsilon && delta.y <= epsilon && delta.z <= epsilon,
        "{label} differed by ({:.6}, {:.6}, {:.6}); actual=({:.6}, {:.6}, {:.6}) expected=({:.6}, {:.6}, {:.6})",
        delta.x,
        delta.y,
        delta.z,
        actual.x,
        actual.y,
        actual.z,
        expected.x,
        expected.y,
        expected.z,
    );
}

fn command_strategy() -> impl Strategy<Value = RecordedCommand> {
    let axis = prop_oneof![
        Just(-1.0_f32),
        Just(-0.35),
        Just(0.0),
        Just(0.35),
        Just(1.0)
    ];
    let yaw = prop_oneof![
        Just(-0.7_f32),
        Just(-0.25),
        Just(0.0),
        Just(0.25),
        Just(0.7)
    ];
    (
        axis.clone(),
        axis,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        yaw,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                right,
                forward,
                jump_pressed,
                dash_pressed,
                running,
                crouch_intent,
                facing_yaw,
                fire_pressed,
                fire_held,
            )| RecordedCommand {
                wish_dir: Vec2::new(right, forward),
                jump_pressed,
                dash_pressed,
                running,
                crouch_intent,
                facing_yaw,
                fire_pressed,
                fire_active: fire_pressed || fire_held,
            },
        )
}

#[test]
fn simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order() {
    let commands = fixed_command_stream();
    assert_eq!(commands.len(), TICK_COUNT);

    let baseline = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let rerun = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let reversed_spawn = run_stream(&commands, SpawnOrder::BetaThenAlpha);

    assert_runs_match(&rerun, &baseline);
    assert_runs_match(&reversed_spawn, &baseline);
}

#[test]
fn run_movement_tick_applies_local_command_only_to_marked_pawn() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (beta, alpha) = {
        let mut registry = registry.borrow_mut();
        let beta = spawn_player(&mut registry, Role::Beta.start_position());
        let alpha = spawn_player(&mut registry, Role::Alpha.start_position());
        registry.mark_local_player_pawn(alpha).unwrap();
        for id in [alpha, beta] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.is_grounded = true;
            registry.set_component(id, movement).unwrap();
        }
        (beta, alpha)
    };
    let beta_start = registry
        .borrow()
        .get_component::<Transform>(beta)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "only the marked local pawn may emit movement outcomes"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(alpha)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "marked local pawn should consume the jump command"
    );
    assert_eq!(
        registry.get_component::<Transform>(beta).unwrap().position,
        beta_start,
        "unmarked additional pawn must not move from local input"
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(beta)
            .unwrap()
            .velocity,
        Vec3::ZERO,
        "unmarked additional pawn velocity must remain untouched"
    );
}

#[test]
fn run_movement_tick_no_marker_fallback_drives_first_movement_pawn_only() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (first, second) = {
        let mut registry = registry.borrow_mut();
        let first = spawn_player(&mut registry, Role::Alpha.start_position());
        let second = spawn_player(&mut registry, Role::Beta.start_position());
        for id in [first, second] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.is_grounded = true;
            registry.set_component(id, movement).unwrap();
        }
        (first, second)
    };
    let second_start = registry
        .borrow()
        .get_component::<Transform>(second)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "no-marker fallback applies the local command to one deterministic pawn"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "first fallback pawn should consume the jump command"
    );
    assert_eq!(
        registry
            .get_component::<Transform>(second)
            .unwrap()
            .position,
        second_start,
        "second pawn must not move from the single local command"
    );
}

#[test]
fn run_movement_tick_invalid_marker_fallback_drives_first_movement_pawn_only() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (first, second) = {
        let mut registry = registry.borrow_mut();
        let invalid_marker = registry.spawn(Transform::default());
        registry.mark_local_player_pawn(invalid_marker).unwrap();
        let first = spawn_player(&mut registry, Role::Alpha.start_position());
        let second = spawn_player(&mut registry, Role::Beta.start_position());
        for id in [first, second] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.is_grounded = true;
            registry.set_component(id, movement).unwrap();
        }
        (first, second)
    };
    let second_start = registry
        .borrow()
        .get_component::<Transform>(second)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "invalid marker fallback applies the local command to one deterministic pawn"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "first fallback pawn should consume the jump command"
    );
    assert_eq!(
        registry
            .get_component::<Transform>(second)
            .unwrap()
            .position,
        second_start,
        "second pawn must not move from an invalid local marker fallback"
    );
}

#[test]
fn simulate_tick_uses_sim_command_fire_button_with_callback_aim() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_warned,
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, -20.0),
            aim_direction: Vec3::Z,
        },
        DT,
    );

    assert!(
        events.weapon.is_empty(),
        "valid callback aim must not fire when SimCommand.fire_button is inactive"
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "inactive fire button must leave the valid target undamaged"
    );
}

#[test]
fn simulate_tick_normalizes_callback_aim_direction_before_weapon_fire() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -45.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_warned,
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, 0.0),
            aim_direction: Vec3::new(0.0, 0.0, -2.0),
        },
        DT,
    );

    assert_eq!(
        events.weapon,
        vec!["activate"],
        "valid non-unit aim still fires, but range is measured after normalization"
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "non-unit aim must not extend hitscan range in metres"
    );
}

#[test]
fn simulate_tick_noops_weapon_fire_for_invalid_callback_aim_direction() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        let weapon = spawn_weapon(&mut registry);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon keeps component")
            .clone();
        component.cooldown_remaining_ms = 100.0;
        registry.set_component(weapon, component).unwrap();
        let target = spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0));
        (weapon, target)
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_warned,
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, 0.0),
            aim_direction: Vec3::ZERO,
        },
        DT,
    );

    assert!(
        events.weapon.is_empty(),
        "zero aim should suppress shot events"
    );
    let registry = registry.borrow();
    let weapon_component = registry
        .get_component::<WeaponComponent>(weapon)
        .expect("weapon keeps component");
    assert!(
        (weapon_component.cooldown_remaining_ms - (100.0 - DT * 1000.0)).abs() < 1.0e-4,
        "invalid aim must still advance weapon cooldown"
    );
    assert!(
        weapon_component.shoot_press_consumed,
        "invalid aim must still advance semi-auto press state"
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "invalid aim must not damage a target"
    );
}

#[test]
fn simulate_tick_noops_weapon_fire_for_non_finite_callback_aim_origin() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_warned,
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(f32::NAN, 2.0, 0.0),
            aim_direction: Vec3::NEG_Z,
        },
        DT,
    );

    assert!(
        events.weapon.is_empty(),
        "non-finite aim origin should suppress shot events"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon keeps component")
            .shoot_press_consumed,
        "non-finite origin must still advance semi-auto press state"
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "non-finite origin must not damage a target"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn simulate_tick_is_deterministic_for_random_command_stream(
        commands in prop::collection::vec(command_strategy(), TICK_COUNT)
    ) {
        let baseline = run_stream(&commands, SpawnOrder::AlphaThenBeta);
        let rerun = run_stream(&commands, SpawnOrder::AlphaThenBeta);
        let reversed_spawn = run_stream(&commands, SpawnOrder::BetaThenAlpha);

        assert_runs_match(&rerun, &baseline);
        assert_runs_match(&reversed_spawn, &baseline);
    }
}
