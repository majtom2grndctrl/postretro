// Movement under resolved environment volumes: per-tick gravity resolution at the tick-start position.
// See: context/lib/movement.md §4 (Environment resolution), §6 (Regional gravity and push)

use glam::{Vec2, Vec3};

use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, ForgivenessParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams,
};

use super::{MovementInput, tick};
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const LEVEL_GRAVITY: f32 = -20.0;
const VOLUME_GRAVITY: f32 = -3.0;
const VEL_EPS: f32 = 1.0e-4;

pub(super) fn descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        sounds: None,
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
        knockback: Default::default(),
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        slide: None,
        swim: None,
        view_feel: None,
    }
}

/// An airborne pawn with vertical velocity `vy`. The empty collision world keeps
/// it airborne for the whole test, so every tick integrates gravity.
fn airborne_pawn(vy: f32) -> PlayerMovementComponent {
    let mut component = PlayerMovementComponent::from_descriptor(&descriptor());
    component.velocity = Vec3::new(0.0, vy, 0.0);
    component
}

pub(super) fn idle_input() -> MovementInput {
    MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        facing_pitch: 0.0,
        use_pressed: false,
        drop_pressed: false,
    }
}

/// An axis-aligned box leaf, as the level loader would hand movement.
pub(super) fn box_leaf(min: Vec3, max: Vec3) -> ConvexLeaf {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    ConvexLeaf {
        planes: vec![
            plane(Vec3::X, max.x),
            plane(Vec3::NEG_X, -min.x),
            plane(Vec3::Y, max.y),
            plane(Vec3::NEG_Y, -min.y),
            plane(Vec3::Z, max.z),
            plane(Vec3::NEG_Z, -min.z),
        ],
        aabb_min: min,
        aabb_max: max,
    }
}

/// An axis-aligned box volume that sets only gravity.
fn gravity_box(min: Vec3, max: Vec3, gravity: f32) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: Some(gravity),
        push: None,
        fluid: None,
        brushes: vec![box_leaf(min, max)],
    }
}

/// An axis-aligned box volume that sets only push.
pub(super) fn push_box(min: Vec3, max: Vec3, push: Vec3) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: Some(push),
        fluid: None,
        brushes: vec![box_leaf(min, max)],
    }
}

/// One idle tick in an empty world; returns the new position and the tick's
/// change in vertical velocity.
fn step(
    component: &mut PlayerMovementComponent,
    environment: EnvironmentView<'_>,
    position: Vec3,
) -> (Vec3, f32) {
    let vy_before = component.velocity.y;
    let (next, _) = tick(
        component,
        &idle_input(),
        &CollisionWorld::new(),
        environment,
        DT,
        position,
    );
    (next, component.velocity.y - vy_before)
}

#[test]
fn player_falls_at_volume_gravity_inside_low_gravity_volume() {
    let volumes = EnvironmentVolumes::build(vec![gravity_box(
        Vec3::splat(-50.0),
        Vec3::splat(50.0),
        VOLUME_GRAVITY,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut component = airborne_pawn(0.0);
    let mut position = Vec3::new(0.0, 10.0, 0.0);
    for _ in 0..30 {
        let (next, dv) = step(&mut component, environment, position);
        assert!(
            (dv - VOLUME_GRAVITY * DT).abs() < VEL_EPS,
            "inside the volume the pawn integrates its gravity: dv={dv}"
        );
        position = next;
    }
    assert!((component.velocity.y - 30.0 * VOLUME_GRAVITY * DT).abs() < VEL_EPS);
}

#[test]
fn player_keeps_vertical_velocity_inside_zero_gravity_volume() {
    let volumes = EnvironmentVolumes::build(vec![gravity_box(
        Vec3::splat(-50.0),
        Vec3::splat(50.0),
        0.0,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    for vy in [1.5, -2.0] {
        let mut component = airborne_pawn(vy);
        let mut position = Vec3::new(0.0, 10.0, 0.0);
        for _ in 0..30 {
            position = step(&mut component, environment, position).0;
        }
        assert!(
            (component.velocity.y - vy).abs() < VEL_EPS,
            "zero gravity holds vy={vy}, got {}",
            component.velocity.y
        );
    }
}

// The environment resolves once, at the tick-start position, so the crossing
// tick still integrates the volume's gravity.
#[test]
fn player_crossing_out_of_gravity_volume_integrates_volume_gravity_then_level_gravity() {
    // The pawn starts 5 cm above the volume's floor face, falling at 6 m/s: the
    // first tick carries its center out through the bottom face.
    let volumes = EnvironmentVolumes::build(vec![gravity_box(
        Vec3::new(-50.0, 0.0, -50.0),
        Vec3::new(50.0, 20.0, 50.0),
        VOLUME_GRAVITY,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut component = airborne_pawn(-6.0);
    let start = Vec3::new(0.0, 0.05, 0.0);

    let (after_crossing, crossing_dv) = step(&mut component, environment, start);
    assert!(
        after_crossing.y < 0.0,
        "fixture: the crossing tick must leave the volume, ended at {after_crossing:?}"
    );
    assert!(
        (crossing_dv - VOLUME_GRAVITY * DT).abs() < VEL_EPS,
        "the crossing tick integrates the volume's gravity: dv={crossing_dv}"
    );

    let (_, next_dv) = step(&mut component, environment, after_crossing);
    assert!(
        (next_dv - LEVEL_GRAVITY * DT).abs() < VEL_EPS,
        "the first tick outside integrates level gravity: dv={next_dv}"
    );
}

#[test]
fn player_entering_gravity_volume_takes_its_gravity_from_the_next_tick() {
    let volumes = EnvironmentVolumes::build(vec![gravity_box(
        Vec3::new(-50.0, -20.0, -50.0),
        Vec3::new(50.0, 0.0, 50.0),
        VOLUME_GRAVITY,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut component = airborne_pawn(-6.0);
    let start = Vec3::new(0.0, 0.05, 0.0);

    let (inside, entering_dv) = step(&mut component, environment, start);
    assert!(inside.y < 0.0, "fixture: the tick must enter the volume");
    assert!(
        (entering_dv - LEVEL_GRAVITY * DT).abs() < VEL_EPS,
        "the entering tick started outside: dv={entering_dv}"
    );
    let (_, inside_dv) = step(&mut component, environment, inside);
    assert!(
        (inside_dv - VOLUME_GRAVITY * DT).abs() < VEL_EPS,
        "inside, the volume's gravity applies: dv={inside_dv}"
    );
}
