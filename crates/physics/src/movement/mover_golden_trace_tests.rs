// Recorded player run on a mover: carry, displacement, release velocity and
// ground-stick stay bit-identical through the shared mover-contact primitives.
// See: context/lib/movement.md §6

use std::collections::HashMap;

use glam::{Quat, Vec2, Vec3};
use postretro_collision::EnvironmentView;
use postretro_entities::Transform;
use postretro_foundation::{GroundRef, PlayerMovementComponent};

use super::environment_tests::{descriptor, idle_input};
use super::overlap_tests::Geometry;
use super::{MovementInput, tick};
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
const LIFT: u32 = 7;
const PUSHER: u32 = 8;
const TICKS: u32 = 600;

/// When the pusher starts: the pawn has walked off the lift and landed.
const PUSH_START: u32 = 470;

/// The golden trace. Rewrite it only with `write_player_mover_golden_trace`,
/// and only for a change meant to alter player movement.
const GOLDEN: &str = include_str!("mover_golden_trace.txt");

#[derive(Default)]
struct Poses(HashMap<u32, MoverPose>);

impl MoverPoseSource for Poses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.0.get(&mover_id).copied()
    }
}

fn lift() -> MoverCollider {
    let half = 3.0;
    MoverCollider::from_local_triangles(
        LIFT,
        &[
            Vec3::new(-half, 0.0, -half),
            Vec3::new(half, 0.0, -half),
            Vec3::new(half, 0.0, half),
            Vec3::new(-half, 0.0, half),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}

/// A wall facing -X at local x = 0, wide enough that the pawn stays in front.
fn pusher() -> MoverCollider {
    let (y, z) = (4.0, 6.0);
    MoverCollider::from_local_triangles(
        PUSHER,
        &[
            Vec3::new(0.0, 0.0, -z),
            Vec3::new(0.0, y, -z),
            Vec3::new(0.0, y, z),
            Vec3::new(0.0, 0.0, z),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}

/// The lift's motion on `tick`: linear velocity and yaw rate.
fn lift_motion(tick: u32) -> (Vec3, f32) {
    match tick {
        40..100 => (Vec3::Y * 2.0, 0.0),
        100..160 => (Vec3::X * 3.0, 0.0),
        160..220 => (Vec3::NEG_Y * 1.5, 0.0),
        220..280 => (Vec3::ZERO, 1.5),
        300..340 => (Vec3::Y, 0.0),
        340..460 => (Vec3::X * 2.0, 0.0),
        _ => (Vec3::ZERO, 0.0),
    }
}

/// The pawn's input on `tick`: jump off the rising lift, then walk off the
/// moving one.
fn input(tick: u32) -> MovementInput {
    let mut input = idle_input();
    match tick {
        310 => input.jump_pressed = true,
        360..460 => input.wish_dir = Vec2::new(1.0, 0.0),
        _ => {}
    }
    input
}

/// One traced tick: pose and velocity as raw bits, then the ground reference.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TraceRow {
    tick: u32,
    position: Vec3,
    velocity: Vec3,
    ground: GroundRef,
}

impl TraceRow {
    fn format(&self) -> String {
        let [px, py, pz] = self.position.to_array().map(f32::to_bits);
        let [vx, vy, vz] = self.velocity.to_array().map(f32::to_bits);
        format!(
            "{} {px:08x} {py:08x} {pz:08x} {vx:08x} {vy:08x} {vz:08x} {}",
            self.tick,
            ground_name(self.ground)
        )
    }

    fn parse(line: &str) -> Self {
        let mut fields = line.split_whitespace();
        let tick = fields.next().unwrap().parse().unwrap();
        let mut float = || f32::from_bits(u32::from_str_radix(fields.next().unwrap(), 16).unwrap());
        let position = Vec3::new(float(), float(), float());
        let velocity = Vec3::new(float(), float(), float());
        Self {
            tick,
            position,
            velocity,
            ground: parse_ground(fields.next().unwrap()),
        }
    }
}

fn ground_name(ground: GroundRef) -> String {
    match ground {
        GroundRef::Airborne => "airborne".to_string(),
        GroundRef::World => "world".to_string(),
        GroundRef::Mover(id) => format!("mover:{id}"),
    }
}

fn parse_ground(name: &str) -> GroundRef {
    match name {
        "airborne" => GroundRef::Airborne,
        "world" => GroundRef::World,
        other => GroundRef::Mover(other.strip_prefix("mover:").unwrap().parse().unwrap()),
    }
}

fn world() -> CollisionWorld {
    Geometry::default().floor((-50.0, 50.0), 0.0).build()
}

fn record() -> Vec<TraceRow> {
    let world = world();
    let movers = [lift(), pusher()];
    let mut component = PlayerMovementComponent::from_descriptor(&descriptor());
    let mut position = Vec3::new(1.0, 2.3, 0.5);
    let mut lift_position = Vec3::Y;
    let mut lift_rotation = Quat::IDENTITY;
    let mut pusher_x = None::<f32>;
    let mut poses = Poses::default();
    let mut trace = Vec::with_capacity(TICKS as usize);

    for tick_index in 0..TICKS {
        let (velocity, yaw_rate) = lift_motion(tick_index);
        let tick_delta = velocity * DT;
        let tick_rotation_delta = Quat::from_rotation_y(yaw_rate * DT);
        lift_position += tick_delta;
        lift_rotation = (tick_rotation_delta * lift_rotation).normalize();
        poses.0.insert(
            LIFT,
            MoverPose {
                transform: Transform {
                    position: lift_position,
                    rotation: lift_rotation,
                    scale: Vec3::ONE,
                },
                linear_velocity: velocity,
                tick_delta,
                angular_velocity: Vec3::Y * yaw_rate,
                tick_rotation_delta,
                carry_yaw: false,
                tick_dt: DT,
            },
        );

        if tick_index >= PUSH_START {
            let speed = 2.0;
            let x = pusher_x.get_or_insert(position.x - 1.0);
            *x += speed * DT;
            poses.0.insert(
                PUSHER,
                MoverPose {
                    transform: Transform {
                        position: Vec3::new(*x, 0.0, position.z),
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                    linear_velocity: Vec3::X * speed,
                    tick_delta: Vec3::X * speed * DT,
                    angular_velocity: Vec3::ZERO,
                    tick_rotation_delta: Quat::IDENTITY,
                    carry_yaw: false,
                    tick_dt: DT,
                },
            );
        }

        let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
        let (next, _) = tick(
            &mut component,
            &input(tick_index),
            &collision,
            ENV,
            DT,
            position,
        );
        position = next;
        trace.push(TraceRow {
            tick: tick_index,
            position,
            velocity: component.velocity,
            ground: component.ground,
        });
    }
    trace
}

fn golden() -> Vec<TraceRow> {
    GOLDEN.lines().map(TraceRow::parse).collect()
}

#[test]
fn a_recorded_player_mover_run_keeps_its_positions_and_ground_refs() {
    let live = record();
    let golden = golden();
    assert_eq!(live.len(), golden.len(), "the trace length changed");
    for (live, golden) in live.iter().zip(&golden) {
        assert_eq!(live.ground, golden.ground, "ground at tick {}", live.tick);
        // Bit-identical where the trace was recorded. Another platform's libm
        // may round friction's `exp` differently, so it gets a tolerance that
        // still catches a behaviour change.
        if cfg!(target_os = "windows") {
            assert_eq!(live.format(), golden.format(), "tick {}", live.tick);
        } else {
            let drift = live.position.distance(golden.position);
            assert!(drift < 1.0e-3, "tick {}: drifted {drift}", live.tick);
        }
    }
}

/// The trace proves nothing unless the run reaches each mover contact.
#[test]
fn the_recorded_run_exercises_carry_displacement_release_and_ground_stick() {
    let trace = golden();
    let at = |tick: u32| trace[tick as usize];
    assert_eq!(at(70).ground, GroundRef::Mover(LIFT), "carried up");
    assert!(at(99).position.y > at(40).position.y + 1.5, "the lift rose");
    assert!(at(159).position.x > 2.5, "carried sideways");
    assert_eq!(
        at(219).ground,
        GroundRef::Mover(LIFT),
        "stuck on the descent"
    );
    assert_ne!(
        at(250).position.x,
        at(221).position.x,
        "carried by rotation"
    );
    assert!(
        trace[310..340]
            .iter()
            .any(|row| row.ground == GroundRef::Airborne),
        "jumped off the lift"
    );
    assert!(
        trace[340..470].iter().any(|row| {
            row.ground == GroundRef::Airborne && row.velocity.x > 1.0 && row.velocity.y <= 0.0
        }),
        "left the moving lift with its velocity"
    );
    assert_eq!(at(PUSH_START - 1).ground, GroundRef::World, "landed");
    assert!(
        at(TICKS - 1).position.x > at(PUSH_START).position.x + 1.0,
        "the pusher displaced the pawn"
    );
}

/// Rewrites the golden trace. Run it once with `--ignored` for a deliberate
/// movement change, and say so in the commit.
#[test]
#[ignore = "rewrites the golden trace"]
fn write_player_mover_golden_trace() {
    let text: String = record().iter().map(|row| row.format() + "\n").collect();
    std::fs::write(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/movement/mover_golden_trace.txt"
        ),
        text,
    )
    .unwrap();
}
