//! Host-seat rider rebase at the avatar seam: a walking client pawn on a turning
//! mover keeps its legs' heading relative to its aim whether or not the rebase
//! carries its yaw.

use glam::{EulerRot, Quat, Vec3};
use postretro_entities::components::mesh::{MeshAnimation, MeshComponent};
use postretro_entities::{
    EntityRegistry, KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, Transform,
};
use postretro_foundation::{GroundRef, PoseInputs};

use super::{present_client_pawns, record_client_pawn_poses};
use crate::NetworkIdAllocator;
use crate::command_queue::MovementOwners;
use crate::interpolation::RemoteInterpolationBuffer;
use crate::kinematic_mover::{MoverTickState, MoverTickStateTable};

const MOVER: u32 = 5;
const SPIN_PER_TICK: f32 = 0.05;

fn mover_component(carry_yaw: bool) -> KinematicMoverComponent {
    KinematicMoverComponent::new(
        MOVER,
        KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO],
            waypoint_names: vec!["pivot".into()],
            speed_mps: 0.0,
            wait_ms: 0.0,
            mode: KinematicMoverMode::Once,
            started: true,
            spin_axis: Vec3::Y,
            initial_spin_rate_rad_s: 3.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw,
        },
    )
}

/// `(aim_yaw, heading_yaw)` the host seat presents for a client pawn walking
/// on a mover that turns `SPIN_PER_TICK` each tick, sampled mid-interval.
fn presented_avatar(carry_yaw: bool, world_space: bool) -> (f32, f32) {
    let mut registry = EntityRegistry::new();
    let mover = registry.spawn(Transform::default());
    registry
        .set_component(mover, mover_component(carry_yaw))
        .unwrap();
    let pawn = registry.spawn(Transform::default());
    let mut mesh = MeshComponent::animated(
        "avatar".into(),
        MeshAnimation::new(Default::default(), "idle".into()),
    );
    mesh.pose_inputs = Some(PoseInputs::default());
    registry.set_component(pawn, mesh).unwrap();
    let mut movement = crate::predict_reconcile_harness_test_fixtures::component();
    movement.ground = GroundRef::Mover(MOVER);
    movement.velocity = Vec3::new(0.0, 0.0, -1.5);
    registry.set_component(pawn, movement).unwrap();

    let mut owners = MovementOwners::default();
    owners.set(pawn, 7);
    let mut allocator = NetworkIdAllocator::new();
    allocator.stamp(pawn);
    let mut buffer = RemoteInterpolationBuffer::default();
    buffer.set_world_space_riders(world_space);
    let mut mover_states = MoverTickStateTable::default();

    for tick in 0..6u32 {
        let mover_pose = Transform {
            rotation: Quat::from_rotation_y(SPIN_PER_TICK * tick as f32),
            ..Transform::default()
        };
        registry.snapshot_transforms();
        registry.set_component(mover, mover_pose).unwrap();
        // The pawn stands one metre out, facing 0.4 rad off the mover.
        registry
            .set_component(
                pawn,
                Transform {
                    position: mover_pose.rotation * Vec3::new(1.0, 1.2, 0.0),
                    rotation: Quat::from_rotation_y(SPIN_PER_TICK * tick as f32 + 0.4),
                    scale: Vec3::ONE,
                },
            )
            .unwrap();
        mover_states.publish(
            MOVER,
            MoverTickState {
                entity: mover,
                transform: mover_pose,
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y * 3.0,
                tick_rotation_delta: Quat::from_rotation_y(SPIN_PER_TICK),
                carry_yaw,
                tick_dt: 1.0 / 60.0,
            },
        );
        record_client_pawn_poses(
            &mut buffer,
            &owners,
            &allocator,
            &registry,
            &mover_states,
            tick,
        );
    }
    present_client_pawns(&buffer, &owners, &allocator, &mut registry, 6, 0.5);
    let inputs = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .pose_inputs
        .unwrap();
    let (transform_yaw, _, _) = registry
        .get_component::<Transform>(pawn)
        .unwrap()
        .rotation
        .to_euler(EulerRot::YXZ);
    assert!((transform_yaw - inputs.aim_yaw).abs() < 1.0e-5);
    (inputs.aim_yaw, inputs.heading_yaw)
}

fn wrap(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[test]
fn rebased_rider_legs_keep_heading_relative_to_aim() {
    let (world_aim, world_heading) = presented_avatar(true, true);
    let (aim, heading) = presented_avatar(true, false);
    assert!(
        (wrap(aim - world_aim)).abs() > 0.01,
        "carry_yaw turns the presented aim with the drawn mover"
    );
    assert!(
        (wrap(heading - aim) - wrap(world_heading - world_aim)).abs() < 1.0e-5,
        "legs relative to aim: rebased {} vs world {}",
        wrap(heading - aim),
        wrap(world_heading - world_aim)
    );

    let (world_aim, world_heading) = presented_avatar(false, true);
    let (aim, heading) = presented_avatar(false, false);
    assert_eq!((aim, heading), (world_aim, world_heading));
}
