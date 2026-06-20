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
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::components::weapon::WeaponComponent;
use crate::scripting::data_descriptors::{
    AirParams, CapsuleParams, FallParams, FireMode, ForgivenessParams, GroundParams,
    PlayerMovementDescriptor, ResolutionMode, SpeedParams, WeaponDescriptor,
};
use crate::scripting::reaction_dispatch::ProgressTracker;
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{FireButtonState, WeaponFireCommand};

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
            weapon_fire: WeaponFireCommand {
                button: self.to_sim_command().fire_button,
                aim_origin: Vec3::new(0.0, 2.0, -20.0),
                aim_direction: Vec3::new(self.facing_yaw.sin(), 0.0, -self.facing_yaw.cos())
                    .normalize(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StageEvents {
    movement: Vec<&'static str>,
    ai: Vec<&'static str>,
    weapon: Vec<&'static str>,
    death: Vec<String>,
}

impl From<TickEvents> for StageEvents {
    fn from(events: TickEvents) -> Self {
        Self {
            movement: events.movement,
            ai: events.ai,
            weapon: events.weapon,
            death: events.death,
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
    events: Vec<StageEvents>,
}

struct SimHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    active_wieldable: EntityId,
    progress: ProgressTracker,
    ai_warned: HashSet<String>,
    role_ids: Vec<(Role, EntityId)>,
}

impl SimHarness {
    fn new(spawn_order: SpawnOrder) -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let mut role_ids = Vec::new();
        let active_wieldable = {
            let mut registry = registry.borrow_mut();
            for role in spawn_order.roles() {
                let id = spawn_player(&mut registry, role.start_position());
                role_ids.push((role, id));
            }
            spawn_weapon(&mut registry)
        };

        Self {
            registry,
            world: floor_world(),
            hit_zones: HitZoneStore::new(),
            active_wieldable,
            progress: ProgressTracker::new(),
            ai_warned: HashSet::new(),
            role_ids,
        }
    }

    fn tick(&mut self, command: RecordedCommand) -> StageEvents {
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
        .into()
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
        events,
    }
}

fn assert_runs_match(actual: &SimRun, expected: &SimRun) {
    assert_eq!(
        actual.events, expected.events,
        "stage-grouped event names must match exactly"
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
        yaw,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                right,
                forward,
                dash_pressed,
                running,
                crouch_intent,
                facing_yaw,
                fire_pressed,
                fire_held,
            )| RecordedCommand {
                wish_dir: Vec2::new(right, forward),
                jump_pressed: false,
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 8,
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
