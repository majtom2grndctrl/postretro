// Forced-rounding spike for measuring seam divergence.
// See: context/plans/in-progress/M15--p0-headless-sim-seam/index.md

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glam::{Vec2, Vec3};
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;

use super::{SimCommand, simulate_tick};
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
use crate::weapon::FireButtonState;

const TICK_COUNT: usize = 600;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const ROUNDING_STEP: f32 = 1.0 / 1_048_576.0;

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
struct PawnSample {
    role: Role,
    position: Vec3,
}

#[derive(Debug, Default, Clone, Copy)]
struct DivergenceSample {
    per_axis: Vec3,
    total: f32,
}

impl DivergenceSample {
    fn from_positions(a: Vec3, b: Vec3) -> Self {
        let per_axis = (a - b).abs();
        Self {
            per_axis,
            total: per_axis.length(),
        }
    }

    fn max(self, other: Self) -> Self {
        Self {
            per_axis: self.per_axis.max(other.per_axis),
            total: self.total.max(other.total),
        }
    }
}

#[derive(Debug, Default)]
struct DivergenceMeasurement {
    max: DivergenceSample,
    final_sample: DivergenceSample,
    first_over_1mm_tick: Option<usize>,
    first_over_1cm_tick: Option<usize>,
    first_over_5cm_tick: Option<usize>,
}

struct SimHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    active_wieldable: EntityId,
    progress: ProgressTracker,
    ai_warned: HashSet<String>,
    role_ids: Vec<(Role, EntityId)>,
    force_post_tick_rounding: bool,
}

impl SimHarness {
    fn new(force_post_tick_rounding: bool) -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (role_ids, active_wieldable) = {
            let mut registry = registry.borrow_mut();
            let role_ids = [Role::Alpha, Role::Beta]
                .into_iter()
                .map(|role| (role, spawn_player(&mut registry, role.start_position())))
                .collect::<Vec<_>>();
            let active_wieldable = spawn_weapon(&mut registry);
            (role_ids, active_wieldable)
        };

        Self {
            registry,
            world: floor_world(),
            hit_zones: HitZoneStore::new(),
            active_wieldable,
            progress: ProgressTracker::new(),
            ai_warned: HashSet::new(),
            role_ids,
            force_post_tick_rounding,
        }
    }

    fn tick(&mut self, command: RecordedCommand) {
        let sim_command = command.to_sim_command();
        let _ = simulate_tick(
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
        );
        if self.force_post_tick_rounding {
            self.force_round_post_tick_state();
        }
    }

    fn force_round_post_tick_state(&mut self) {
        let mut registry = self.registry.borrow_mut();
        for (_, id) in &self.role_ids {
            if let Ok(transform) = registry.get_component::<Transform>(*id) {
                let mut rounded = *transform;
                rounded.position = round_vec3(rounded.position, ROUNDING_STEP);
                let _ = registry.set_component(*id, rounded);
            }
            if let Ok(component) = registry.get_component::<PlayerMovementComponent>(*id) {
                let mut rounded = component.clone();
                rounded.velocity = round_vec3(rounded.velocity, ROUNDING_STEP);
                let _ = registry.set_component(*id, rounded);
            }
        }
    }

    fn samples(&self) -> Vec<PawnSample> {
        let registry = self.registry.borrow();
        let mut samples = self
            .role_ids
            .iter()
            .map(|(role, id)| {
                let transform = *registry
                    .get_component::<Transform>(*id)
                    .expect("role entity must keep transform");
                PawnSample {
                    role: *role,
                    position: transform.position,
                }
            })
            .collect::<Vec<_>>();
        samples.sort_by_key(|sample| sample.role);
        samples
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

fn recorded_command_stream() -> Vec<RecordedCommand> {
    (0..TICK_COUNT)
        .map(|tick| {
            let phase = tick % 150;
            let fire_pressed = matches!(tick, 12 | 170 | 335 | 501);
            RecordedCommand {
                wish_dir: if phase < 55 {
                    Vec2::new(0.3, 1.0)
                } else if phase < 95 {
                    Vec2::new(-0.65, 0.25)
                } else {
                    Vec2::new(0.0, -0.2)
                },
                jump_pressed: matches!(tick, 36 | 224 | 411),
                dash_pressed: matches!(tick, 88 | 260 | 430),
                running: phase < 90,
                crouch_intent: (320..370).contains(&tick),
                facing_yaw: if tick < 200 {
                    0.0
                } else if tick < 420 {
                    0.25
                } else {
                    -0.35
                },
                fire_pressed,
                fire_active: fire_pressed || matches!(tick, 13 | 171 | 336 | 502),
            }
        })
        .collect()
}

fn measure_forced_rounding_divergence(commands: &[RecordedCommand]) -> DivergenceMeasurement {
    let mut baseline = SimHarness::new(false);
    let mut rounded = SimHarness::new(true);
    let mut measurement = DivergenceMeasurement::default();

    for (tick, command) in commands.iter().copied().enumerate() {
        baseline.tick(command);
        rounded.tick(command);
        let current = compare_samples(&baseline.samples(), &rounded.samples());
        measurement.max = measurement.max.max(current);
        measurement.final_sample = current;
        if current.total > 0.001 && measurement.first_over_1mm_tick.is_none() {
            measurement.first_over_1mm_tick = Some(tick + 1);
        }
        if current.total > 0.01 && measurement.first_over_1cm_tick.is_none() {
            measurement.first_over_1cm_tick = Some(tick + 1);
        }
        if current.total > 0.05 && measurement.first_over_5cm_tick.is_none() {
            measurement.first_over_5cm_tick = Some(tick + 1);
        }
    }

    measurement
}

fn compare_samples(baseline: &[PawnSample], rounded: &[PawnSample]) -> DivergenceSample {
    assert_eq!(baseline.len(), rounded.len());
    baseline
        .iter()
        .zip(rounded)
        .map(|(a, b)| {
            assert_eq!(a.role, b.role, "samples must be compared by stable role");
            DivergenceSample::from_positions(a.position, b.position)
        })
        .fold(DivergenceSample::default(), DivergenceSample::max)
}

fn round_vec3(value: Vec3, step: f32) -> Vec3 {
    Vec3::new(
        round_to_step(value.x, step),
        round_to_step(value.y, step),
        round_to_step(value.z, step),
    )
}

fn round_to_step(value: f32, step: f32) -> f32 {
    (value / step).round() * step
}

#[test]
fn forced_rounding_spike_measures_position_divergence() {
    let commands = recorded_command_stream();
    assert_eq!(commands.len(), TICK_COUNT);

    let measurement = measure_forced_rounding_divergence(&commands);
    println!(
        "[Task4 divergence] ticks={} rounding_step_m={:.9} max_axis_m=({:.9}, {:.9}, {:.9}) max_total_m={:.9} final_axis_m=({:.9}, {:.9}, {:.9}) final_total_m={:.9} first_over_1mm_tick={:?} first_over_1cm_tick={:?} first_over_5cm_tick={:?}",
        TICK_COUNT,
        ROUNDING_STEP,
        measurement.max.per_axis.x,
        measurement.max.per_axis.y,
        measurement.max.per_axis.z,
        measurement.max.total,
        measurement.final_sample.per_axis.x,
        measurement.final_sample.per_axis.y,
        measurement.final_sample.per_axis.z,
        measurement.final_sample.total,
        measurement.first_over_1mm_tick,
        measurement.first_over_1cm_tick,
        measurement.first_over_5cm_tick,
    );
    assert!(
        measurement.max.total > 0.0,
        "forced post-tick state rounding should produce measurable divergence"
    );
}
