// The per-tick mover paths allocate nothing: carry, a body beside a mover, the gate
// update and read, and board, ride and exit. `tick_impl` allocates its own per-tick
// lists, so the window scopes to the paths this contact added. A body overlapping a
// mover allocates inside parry's EPA, which `contact` builds per penetrating
// triangle; the ignored test holds that open.
// See: context/lib/development_guide.md §1.4

use glam::{Quat, Vec3};
use postretro_collision::EnvironmentView;
use postretro_entities::components::agent::{AgentComponent, PlannedRide};
use postretro_entities::{
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, Transform,
};
use postretro_foundation::GroundRef;

use super::mover_link::{RideStep, advance};
use crate::alloc_probe::AllocSnapshot;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource};
use crate::collision::{CollisionWorld, SKIN_DISTANCE};
use crate::kinematic_mover::MoverDockTable;
use crate::movement::agent_motion::{AgentCapsule, collide_and_slide};
use crate::nav::NavGate;

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
const LIFT: u32 = 2;
const SWEEPER: u32 = 3;
const REST: f32 = 0.9 + SKIN_DISTANCE;

/// Two movers' poses for one tick, on the stack.
struct Poses {
    lift: MoverPose,
    sweeper: MoverPose,
}

impl MoverPoseSource for Poses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        match mover_id {
            LIFT => Some(self.lift),
            SWEEPER => Some(self.sweeper),
            _ => None,
        }
    }
}

fn moving_pose(position: Vec3, velocity: Vec3) -> MoverPose {
    MoverPose {
        transform: Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
        linear_velocity: velocity,
        tick_delta: velocity * DT,
        angular_velocity: Vec3::ZERO,
        tick_rotation_delta: Quat::IDENTITY,
        carry_yaw: false,
        tick_dt: DT,
    }
}

fn panel(id: u32, corners: [Vec3; 4]) -> MoverCollider {
    MoverCollider::from_local_triangles(id, &corners, &[[0, 2, 1], [0, 3, 2]]).unwrap()
}

fn mover(id: u32, at_rest: bool) -> KinematicMoverComponent {
    let mut mover = KinematicMoverComponent::new(
        id,
        KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::Y * 4.0],
            waypoint_names: vec!["low".into(), "high".into()],
            speed_mps: 2.0,
            wait_ms: 500.0,
            mode: KinematicMoverMode::PingPong,
            started: true,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    if !at_rest {
        mover.segment_elapsed_ms = 100.0;
        mover.current_linear_velocity = Vec3::Y * 2.0;
    }
    mover
}

/// Everything the window drives, built before it arms.
struct Scene {
    world: CollisionWorld,
    movers: [MoverCollider; 2],
    docks: MoverDockTable,
    docked: KinematicMoverComponent,
    moving: KinematicMoverComponent,
    capsule: AgentCapsule,
    rider: AgentComponent,
}

impl Scene {
    fn new() -> Self {
        let half = 6.0;
        Self {
            world: CollisionWorld::from_triangles_for_test(
                vec![
                    Vec3::new(-40.0, 0.0, -40.0),
                    Vec3::new(40.0, 0.0, -40.0),
                    Vec3::new(40.0, 0.0, 40.0),
                    Vec3::new(-40.0, 0.0, 40.0),
                ],
                vec![[0, 2, 1], [0, 3, 2]],
            ),
            movers: [
                panel(
                    LIFT,
                    [
                        Vec3::new(-half, 0.0, -half),
                        Vec3::new(half, 0.0, -half),
                        Vec3::new(half, 0.0, half),
                        Vec3::new(-half, 0.0, half),
                    ],
                ),
                panel(
                    SWEEPER,
                    [
                        Vec3::new(0.0, 0.0, -half),
                        Vec3::new(0.0, 3.0, -half),
                        Vec3::new(0.0, 3.0, half),
                        Vec3::new(0.0, 0.0, half),
                    ],
                ),
            ],
            docks: MoverDockTable::default(),
            docked: mover(LIFT, true),
            moving: mover(LIFT, false),
            capsule: AgentCapsule {
                radius: 0.35,
                half_height: 0.55,
                step_height: 0.4,
            },
            rider: AgentComponent::new(0.35, 1.8, 0.4, 4.0),
        }
    }

    /// A body the sweeper overlaps, displaced clear. Returns its new x.
    fn displace(&self) -> f32 {
        let poses = Self::poses();
        let collision = CombinedCollisionWorld::new(&self.world, &self.movers, &poses);
        let mut knockback = Vec3::ZERO;
        collide_and_slide(
            &collision,
            &self.capsule,
            Vec3::new(20.2, REST, 0.0),
            GroundRef::World,
            Vec3::ZERO,
            0.0,
            &mut knockback,
            ENV,
            DT,
        )
        .position
        .x
    }

    fn poses() -> Poses {
        Poses {
            lift: moving_pose(Vec3::new(0.0, 1.0, 0.0), Vec3::Y * 2.0),
            sweeper: moving_pose(Vec3::new(20.0, 0.0, 0.0), Vec3::X * 3.0),
        }
    }

    /// One pass over every allocation-free path. Returns proof each one ran.
    fn drive(&mut self) -> (GroundRef, Vec3, u64, bool) {
        let poses = Self::poses();
        let collision = CombinedCollisionWorld::new(&self.world, &self.movers, &poses);

        // Carry.
        let mut knockback = Vec3::ZERO;
        let carried = collide_and_slide(
            &collision,
            &self.capsule,
            Vec3::new(0.0, 1.0 + REST, 0.0),
            GroundRef::Mover(LIFT),
            Vec3::ZERO,
            0.0,
            &mut knockback,
            ENV,
            DT,
        );

        // Beside the sweeper: inside its bound, clear of its face.
        let beside = collide_and_slide(
            &collision,
            &self.capsule,
            Vec3::new(20.6, REST, 0.0),
            GroundRef::World,
            Vec3::ZERO,
            0.0,
            &mut knockback,
            ENV,
            DT,
        );

        // Gate flip: dock, undock, dock.
        self.docks.update(&self.moving);
        self.docks.update(&self.docked);
        let generation = self.docks.generation();
        let gate = NavGate::new(&self.docks);

        // Board, ride, exit.
        self.rider.ground = GroundRef::Mover(LIFT);
        self.rider.planned_ride = Some(PlannedRide {
            mover_id: LIFT,
            board_waypoint: 1,
            exit_waypoint: 0,
            board_point: Vec3::ZERO,
            exit_point: Vec3::X * 9.0,
        });
        self.rider.ride = None;
        let _ = advance(&mut self.rider, Vec3::ZERO, gate, &collision);
        let rode = self.rider.ride.is_some();
        let exit = matches!(
            advance(&mut self.rider, Vec3::ZERO, gate, &collision),
            RideStep::Walk(_)
        );
        self.rider.ground = GroundRef::World;
        let _ = advance(&mut self.rider, Vec3::X * 9.0, gate, &collision);

        (
            carried.ground,
            beside.position,
            generation,
            rode && exit && self.rider.ride.is_none(),
        )
    }
}

#[test]
fn the_per_tick_mover_paths_allocate_nothing() {
    let mut scene = Scene::new();
    scene.docks = {
        let mut movers = crate::kinematic_mover::MoverTickStateTable::default();
        movers.watch_mover_docks(&[LIFT]);
        movers.docks().clone()
    };
    for _ in 0..3 {
        scene.drive();
    }

    let control = AllocSnapshot::arm();
    let boxed = std::hint::black_box(Box::new(7_u64));
    assert_eq!(control.allocs_since(), 1, "the control window reads one");
    drop(boxed);

    let window = AllocSnapshot::arm();
    let (ground, beside, generation, rode_and_exited) = scene.drive();
    let allocations = window.allocs_since();
    assert_eq!(
        allocations, 0,
        "carry, contact, gate and ride allocate nothing"
    );

    assert_eq!(ground, GroundRef::Mover(LIFT), "the window carried a rider");
    assert!(
        beside.x >= 20.6,
        "the window left a clear body where it stood"
    );
    assert!(generation > 3, "the window flipped the gate");
    assert!(rode_and_exited, "the window boarded and exited a rider");
}

#[test]
fn a_plan_boarded_on_a_lift_already_moving_keeps_its_exit() {
    let mut scene = Scene::new();
    scene.drive();
    let poses = Scene::poses();
    let collision = CombinedCollisionWorld::new(&scene.world, &scene.movers, &poses);
    scene.rider.ground = GroundRef::Mover(LIFT);
    scene.rider.planned_ride = Some(PlannedRide {
        mover_id: LIFT,
        board_waypoint: 1,
        exit_waypoint: 0,
        board_point: Vec3::ZERO,
        exit_point: Vec3::X * 9.0,
    });
    let _ = advance(
        &mut scene.rider,
        Vec3::ZERO,
        NavGate::new(&scene.docks),
        &collision,
    );
    let ride = scene.rider.ride.expect("it boarded");
    assert_eq!(ride.exit_waypoint, Some(0));
}

#[test]
#[ignore = "row 35 open: parry's EPA allocates for a body overlapping a mover"]
fn displacing_a_body_allocates_nothing() {
    let scene = Scene::new();
    for _ in 0..3 {
        scene.displace();
    }
    let window = AllocSnapshot::arm();
    let displaced_x = scene.displace();
    let allocations = window.allocs_since();
    assert!(displaced_x > 20.2, "the window displaced a body");
    assert_eq!(allocations, 0);
}
