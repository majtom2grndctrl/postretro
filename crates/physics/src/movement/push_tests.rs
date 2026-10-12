// Movement under push volumes: lift-off, terminal speed, the protected drift layer (caps, decay, walls), slide isolation, upward level gravity.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md (Decisions: gravity and push)

use glam::{Vec2, Vec3};

use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_foundation::{
    CrouchParams, MovementState, PlayerMovementComponent, PlayerMovementDescriptor, SlideParams,
};

use super::environment_tests::{descriptor, idle_input, push_box};
use super::intents::GROUND_STOP_FRICTION;
use super::{MovementInput, tick};
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const LEVEL_GRAVITY: f32 = -20.0;
const LEVEL: EnvironmentView<'static> = EnvironmentView::level_only(LEVEL_GRAVITY);
const VEL_EPS: f32 = 1.0e-4;
const POS_EPS: f32 = 1.0e-4;

/// A flat floor at y = 0 spanning x, z in [-200, 200].
fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, 200.0),
        Vec3::new(-200.0, 0.0, 200.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 2, 1], [0, 3, 2]])
}

/// One push volume spanning the whole test area, from just below the floor to
/// `top`.
fn push_field(push: Vec3, top: f32) -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![push_box(
        Vec3::new(-500.0, -1.0, -500.0),
        Vec3::new(500.0, top, 500.0),
        push,
    )])
}

fn step(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    environment: EnvironmentView<'_>,
    input: &MovementInput,
    position: &mut Vec3,
) {
    *position = tick(component, input, world, environment, DT, *position).0;
}

/// A pawn resting grounded on `floor_world`, settled under level gravity.
fn grounded_pawn(
    desc: &PlayerMovementDescriptor,
    world: &CollisionWorld,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    let mut position = Vec3::new(
        0.0,
        desc.capsule.half_height + desc.capsule.radius + 0.01,
        0.0,
    );
    for _ in 0..8 {
        step(&mut component, world, LEVEL, &idle_input(), &mut position);
    }
    assert!(
        component.is_grounded(),
        "fixture: the pawn must settle grounded"
    );
    (component, position)
}

// M14: lift-off.
#[test]
fn grounded_player_lifts_off_and_rises_in_updraft_stronger_than_gravity() {
    let world = floor_world();
    let push = Vec3::new(0.0, 30.0, 0.0);
    let volumes = push_field(push, 50.0);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let (mut component, mut position) = grounded_pawn(&descriptor(), &world);
    let rest_y = position.y;

    step(
        &mut component,
        &world,
        environment,
        &idle_input(),
        &mut position,
    );
    assert!(
        !component.is_grounded(),
        "the push beats gravity, so the pawn leaves the ground on the entry tick"
    );
    let lift_off_vy = (push.y + LEVEL_GRAVITY) * DT;
    assert!(
        (component.velocity.y - lift_off_vy).abs() < VEL_EPS,
        "the lift-off tick integrates gravity and push: vy={}",
        component.velocity.y
    );
    assert!(
        position.y > rest_y + lift_off_vy * DT - POS_EPS,
        "ground-stick must not snap the lifting pawn back: y={} rest={rest_y}",
        position.y
    );

    let mut previous_y = position.y;
    for _ in 0..60 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        assert!(!component.is_grounded(), "the updraft keeps the pawn aloft");
        assert!(position.y > previous_y, "the pawn keeps rising");
        previous_y = position.y;
    }
    assert!(position.y > rest_y + 1.0, "the pawn rose: y={}", position.y);
}

// M14: terminal speed along the push.
#[test]
fn tall_updraft_caps_speed_along_push_at_fall_terminal_velocity() {
    let desc = descriptor();
    let terminal = desc.fall.terminal_velocity;
    let volumes = push_field(Vec3::new(0.0, 30.0, 0.0), 5_000.0);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let world = CollisionWorld::new();
    let mut component = PlayerMovementComponent::from_descriptor(&desc);
    // Speed across the push is never clamped: it must survive untouched.
    component.velocity = Vec3::new(0.0, 0.0, 2.0);
    let mut position = Vec3::new(0.0, 10.0, 0.0);

    let mut speeds = Vec::new();
    for _ in 0..400 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        speeds.push(component.velocity.y);
    }
    for vy in &speeds {
        assert!(
            *vy <= terminal + VEL_EPS,
            "speed along the push never exceeds the terminal cap: vy={vy}"
        );
    }
    for vy in &speeds[300..] {
        assert!(
            (vy - terminal).abs() < 1.0e-3,
            "speed along the push stops at the terminal cap: vy={vy}"
        );
    }
    assert!(
        (component.velocity.z - 2.0).abs() < VEL_EPS,
        "speed across the push is untouched: vz={}",
        component.velocity.z
    );
}

// M14: a weak upward push leaves the player grounded.
#[test]
fn grounded_player_stays_grounded_under_upward_push_weaker_than_gravity() {
    let world = floor_world();
    let volumes = push_field(Vec3::new(0.0, 12.0, 0.0), 50.0);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let (mut component, mut position) = grounded_pawn(&descriptor(), &world);
    let rest_y = position.y;
    for _ in 0..60 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        assert!(component.is_grounded(), "gravity still holds the pawn down");
        assert!(
            (position.y - rest_y).abs() < POS_EPS,
            "the pawn stays on the floor: y={} rest={rest_y}",
            position.y
        );
    }
}

// M15: a standing player drifts with a sideways push and settles.
#[test]
fn standing_player_drifts_with_sideways_push_and_settles_at_steady_speed() {
    let world = floor_world();
    let push = Vec3::new(8.0, 0.0, 0.0);
    let volumes = push_field(push, 50.0);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let (mut component, mut position) = grounded_pawn(&descriptor(), &world);
    let start = position;

    for _ in 0..299 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        assert!(
            component.is_grounded(),
            "a sideways push never lifts the pawn"
        );
    }
    let previous_vx = component.velocity.x;
    step(
        &mut component,
        &world,
        environment,
        &idle_input(),
        &mut position,
    );
    // Stop friction removes `GROUND_STOP_FRICTION * dt` of the speed each tick
    // before the push adds `push * dt`, so the drift settles at push / friction.
    let steady = push.x / GROUND_STOP_FRICTION;
    assert!(
        (component.velocity.x - steady).abs() < 1.0e-3,
        "the drift settles at push / friction: vx={} expected {steady}",
        component.velocity.x
    );
    assert!(
        (component.velocity.x - previous_vx).abs() < 1.0e-5,
        "the drift is steady: vx={} previous {previous_vx}",
        component.velocity.x
    );
    assert!(
        (component.push_velocity.x - steady).abs() < 1.0e-3,
        "the drift lives in the protected layer: {:?}",
        component.push_velocity
    );
    assert!(position.x > start.x + 1.0, "the pawn drifted with the wind");
    assert!((position.z - start.z).abs() < POS_EPS);
}

fn forward_input() -> MovementInput {
    MovementInput {
        wish_dir: Vec2::new(0.0, 1.0),
        ..idle_input()
    }
}

// M15: airborne with movement input held, the air cap never clips the push.
#[test]
fn airborne_push_grows_past_air_cap_with_input_held_until_terminal_speed() {
    let desc = descriptor();
    let walk = desc.ground.speed.walk;
    let terminal = desc.fall.terminal_velocity;
    let push = Vec3::new(8.0, 0.0, 0.0);
    let volumes = EnvironmentVolumes::build(vec![push_box(
        Vec3::splat(-10_000.0),
        Vec3::splat(10_000.0),
        push,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let world = CollisionWorld::new();
    let mut component = PlayerMovementComponent::from_descriptor(&desc);
    let mut position = Vec3::new(0.0, 10.0, 0.0);

    // Forward (-Z at yaw 0) is held throughout, so the airborne cap runs every
    // tick; the wind blows across it, along +X.
    let mut speeds = Vec::new();
    for _ in 0..420 {
        step(
            &mut component,
            &world,
            environment,
            &forward_input(),
            &mut position,
        );
        assert!(!component.is_grounded());
        speeds.push(component.velocity.x);
        let locomotion = component.velocity - component.push_velocity;
        assert!(
            Vec2::new(locomotion.x, locomotion.z).length() <= walk + VEL_EPS,
            "the air cap still bounds the pawn's own steering: {locomotion:?}"
        );
    }
    assert!(
        speeds[119] > 2.0 * walk,
        "two seconds in, the wind carries the pawn well past the air cap: vx={}",
        speeds[119]
    );
    for pair in speeds[..280].windows(2) {
        assert!(
            pair[1] > pair[0],
            "below the terminal cap the wind keeps accelerating the pawn: {pair:?}"
        );
    }
    for vx in &speeds {
        assert!(
            *vx <= terminal + VEL_EPS,
            "vx={vx} exceeds the terminal cap"
        );
    }
    assert!(
        (speeds[419] - terminal).abs() < 1.0e-3,
        "speed along the push stops at the terminal cap: vx={}",
        speeds[419]
    );
}

// M15: drift carries out of a wind airborne, and ground friction bleeds it on landing.
#[test]
fn drift_carries_out_of_wind_until_landing_then_ground_friction_decays_it() {
    // A wind band from 12 m up; the pawn falls from inside it to the floor.
    const WIND_FLOOR: f32 = 12.0;
    let push = Vec3::new(8.0, 0.0, 0.0);
    let volumes = EnvironmentVolumes::build(vec![push_box(
        Vec3::new(-500.0, WIND_FLOOR, -500.0),
        Vec3::new(500.0, 50.0, 500.0),
        push,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let world = floor_world();
    let mut component = PlayerMovementComponent::from_descriptor(&descriptor());
    let mut position = Vec3::new(0.0, 30.0, 0.0);

    let mut ticks_out_of_wind = 0;
    while !component.is_grounded() {
        let drift_before = component.push_velocity;
        let started_outside = position.y < WIND_FLOOR;
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        if started_outside {
            ticks_out_of_wind += 1;
            assert!(
                (component.push_velocity - drift_before).length() < VEL_EPS,
                "airborne outside the wind the drift holds: {:?} -> {:?}",
                drift_before,
                component.push_velocity
            );
        }
        assert!(ticks_out_of_wind < 600, "fixture: the pawn must land");
    }
    assert!(
        ticks_out_of_wind > 10,
        "fixture: a real fall outside the wind"
    );
    let landed_drift = component.push_velocity.x;
    assert!(
        landed_drift > 5.0,
        "the drift built in the wind survives to the landing: {landed_drift}"
    );

    let mut previous = landed_drift;
    for _ in 0..30 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
        let drift = component.push_velocity.x;
        let expected = previous * (1.0 - GROUND_STOP_FRICTION * DT);
        assert!(
            (drift - expected).abs() < VEL_EPS,
            "grounded, friction bleeds the drift: {drift} expected {expected}"
        );
        previous = drift;
    }
    for _ in 0..200 {
        step(
            &mut component,
            &world,
            environment,
            &idle_input(),
            &mut position,
        );
    }
    assert_eq!(component.push_velocity, Vec3::ZERO, "the drift stops");
    assert!(component.velocity.x.abs() < VEL_EPS);
}

/// A two-sided wall at x = 5, spanning y and z in [-50, 50].
fn wall_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(5.0, -50.0, -50.0),
        Vec3::new(5.0, -50.0, 50.0),
        Vec3::new(5.0, 50.0, 50.0),
        Vec3::new(5.0, 50.0, -50.0),
    ];
    CollisionWorld::from_triangles_for_test(
        points,
        vec![[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]],
    )
}

// The wall absorbs drift: collision projects the layer like knockback.
#[test]
fn wall_absorbs_push_drift() {
    let world = wall_world();
    let weightless = EnvironmentView::level_only(0.0);
    let mut component = PlayerMovementComponent::from_descriptor(&descriptor());
    // Drift carried out of a wind, heading into the wall, with nothing to bleed it.
    component.push_velocity = Vec3::new(10.0, 0.0, 0.0);
    component.velocity = component.push_velocity;
    let mut position = Vec3::new(0.0, 0.0, 0.0);

    for _ in 0..60 {
        step(
            &mut component,
            &world,
            weightless,
            &idle_input(),
            &mut position,
        );
    }
    assert!(
        position.x < 5.0 - descriptor().capsule.radius + 0.01,
        "the wall stops the pawn: x={}",
        position.x
    );
    assert!(
        component.push_velocity.length() < VEL_EPS,
        "the wall absorbed the drift: {:?}",
        component.push_velocity
    );
    assert!(
        component.velocity.length() < VEL_EPS,
        "no drift returns as velocity: {:?}",
        component.velocity
    );
    let resting_x = position.x;
    step(
        &mut component,
        &world,
        weightless,
        &idle_input(),
        &mut position,
    );
    assert!((position.x - resting_x).abs() < POS_EPS);
}

fn slide_descriptor() -> PlayerMovementDescriptor {
    let mut desc = descriptor();
    desc.crouch = Some(CrouchParams {
        half_height: 0.4,
        eye_height: 0.2,
        transition_rate: 15.0,
    });
    desc.slide = Some(SlideParams {
        min_speed: 8.0,
        slide_drag: 2.0,
        slope_assist: 1.0,
        steer_rate: 180.0,
        entry_boost: 1.5,
        min_duration_ms: 250.0,
    });
    desc
}

/// Enter a slide along +X on `floor_world`, then run `ticks` sliding ticks in
/// `environment`; returns each tick's slide boost and the final component.
fn slide_through(
    environment: EnvironmentView<'_>,
    ticks: usize,
) -> (Vec<Vec3>, PlayerMovementComponent) {
    let world = floor_world();
    let (mut component, mut position) = grounded_pawn(&slide_descriptor(), &world);
    let crouch = MovementInput {
        crouch_intent: true,
        ..idle_input()
    };
    component.velocity = Vec3::new(12.0, 0.0, 0.0);
    step(&mut component, &world, LEVEL, &crouch, &mut position);
    let mut boosts = Vec::new();
    for _ in 0..ticks {
        step(&mut component, &world, environment, &crouch, &mut position);
        let MovementState::Sliding { boost, .. } = component.movement_state else {
            panic!("fixture: the pawn must keep sliding");
        };
        boosts.push(boost);
    }
    (boosts, component)
}

// M16: push never feeds the slide's slope boost.
#[test]
fn slide_in_push_volume_gets_no_slope_boost_from_push() {
    // A push along the slide on a flat floor: were the push projected onto the
    // floor like gravity, the slope assist would grow the boost by it.
    let push = Vec3::new(10.0, 0.0, 0.0);
    let volumes = push_field(push, 50.0);
    let ticks = 12;
    let (pushed_boosts, pushed) =
        slide_through(EnvironmentView::new(LEVEL_GRAVITY, &volumes), ticks);
    let (calm_boosts, calm) = slide_through(LEVEL, ticks);

    // The two runs cross the floor's triangles at different ticks, so the
    // forwarded floor normal differs by float noise (~1e-4 m/s of boost). A
    // push-fed boost would differ by `push * slope_assist * dt` per tick, 2 m/s
    // over the run.
    const BOOST_EPS: f32 = 1.0e-3;
    for (pushed_boost, calm_boost) in pushed_boosts.iter().zip(&calm_boosts) {
        assert!(
            (*pushed_boost - *calm_boost).length() < BOOST_EPS,
            "the push leaves the slide boost unchanged: {pushed_boost:?} vs {calm_boost:?}"
        );
    }
    // The push still moves the slider, through the drift layer, outside the boost.
    assert!(pushed.push_velocity.x > 0.5, "{:?}", pushed.push_velocity);
    let gained = pushed.velocity.x - calm.velocity.x;
    assert!(
        (gained - pushed.push_velocity.x).abs() < 1.0e-3,
        "the slider's extra speed is the drift: gained {gained}, drift {:?}",
        pushed.push_velocity
    );
}

// M18: upward level gravity alone never lifts a grounded player.
#[test]
fn upward_level_gravity_keeps_grounded_player_grounded_until_it_jumps() {
    let world = floor_world();
    let desc = descriptor();
    let (mut component, mut position) = grounded_pawn(&desc, &world);
    let rest_y = position.y;
    let upward = EnvironmentView::level_only(5.0);

    for _ in 0..60 {
        step(&mut component, &world, upward, &idle_input(), &mut position);
        assert!(component.is_grounded(), "upward gravity alone never lifts");
        assert!(
            (position.y - rest_y).abs() < POS_EPS,
            "the pawn stays on the floor: y={} rest={rest_y}",
            position.y
        );
    }

    let jump = MovementInput {
        jump_pressed: true,
        ..idle_input()
    };
    step(&mut component, &world, upward, &jump, &mut position);
    assert!(!component.is_grounded(), "the jump leaves the ground");
    assert!((component.velocity.y - desc.air.jump_velocity).abs() < VEL_EPS);
    for _ in 0..10 {
        let vy_before = component.velocity.y;
        step(&mut component, &world, upward, &idle_input(), &mut position);
        assert!(
            (component.velocity.y - vy_before - 5.0 * DT).abs() < VEL_EPS,
            "airborne, upward gravity accelerates the pawn up: vy={}",
            component.velocity.y
        );
    }
}
