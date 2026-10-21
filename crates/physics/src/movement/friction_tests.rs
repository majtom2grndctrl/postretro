// Ground friction and capped acceleration: stops, turns, aim sweeps, banked speed, bunny-hop.
// See: context/lib/movement.md §4 (Ground friction)

use glam::{Vec2, Vec3};

use postretro_collision::EnvironmentView;
use postretro_foundation::{PlayerMovementComponent, PlayerMovementDescriptor};

use super::environment_tests::{descriptor, idle_input};
use super::{MovementInput, tick};
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);

/// A flat floor at y = 0 spanning x, z in [-500, 500]: room for long runs.
fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, 500.0),
        Vec3::new(-500.0, 0.0, 500.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 2, 1], [0, 3, 2]])
}

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
    for _ in 0..10 {
        step(&mut component, world, &idle_input(), &mut position);
    }
    assert!(
        component.is_grounded(),
        "fixture: the pawn settles grounded"
    );
    (component, position)
}

fn step(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    position: &mut Vec3,
) {
    *position = tick(component, input, world, ENV, DT, *position).0;
}

fn forward(facing_yaw: f32, running: bool) -> MovementInput {
    MovementInput {
        wish_dir: Vec2::new(0.0, 1.0),
        running,
        facing_yaw,
        ..idle_input()
    }
}

fn horizontal(velocity: Vec3) -> Vec2 {
    Vec2::new(velocity.x, velocity.z)
}

/// The view's forward direction at `yaw` (`wish_dir_from_input`'s convention).
fn heading(yaw: f32) -> Vec2 {
    Vec2::new(-yaw.sin(), -yaw.cos())
}

// A 90° turn at run speed sheds the old heading within 0.3 s and never climbs
// past the run cap on the way. Without friction while input is held, the old
// heading's momentum lingered for seconds and the speed peaked near 15 m/s.
#[test]
fn a_ninety_degree_turn_at_run_speed_settles_quickly_without_overshoot() {
    let desc = descriptor();
    let world = floor_world();
    let (mut component, mut position) = grounded_pawn(&desc, &world);
    let run = desc.ground.speed.run;
    for _ in 0..120 {
        step(&mut component, &world, &forward(0.0, true), &mut position);
    }
    assert!((horizontal(component.velocity).length() - run).abs() < 1.0e-3);

    let yaw = std::f32::consts::FRAC_PI_2;
    let target = heading(yaw);
    let mut settled_at = None;
    for tick in 1..=60 {
        step(&mut component, &world, &forward(yaw, true), &mut position);
        let velocity = horizontal(component.velocity);
        let speed = velocity.length();
        assert!(
            speed <= run + 1.0e-3,
            "tick {tick}: turning never lifts speed past the cap: {speed}"
        );
        let off_heading = velocity.angle_to(target).abs().to_degrees();
        if settled_at.is_none() && off_heading < 5.0 && (speed - run).abs() < run * 0.02 {
            settled_at = Some(tick);
        }
    }
    let settled_at = settled_at.expect("the turn settles onto the new heading");
    assert!(
        settled_at <= 18,
        "the turn settles within 0.3 s, took {settled_at} ticks"
    );
}

// Sweeping the aim while walking turns the wish every tick. Ground speed must
// not climb past the walk cap at any sweep rate, and a gentle sweep keeps the
// pawn moving near full speed.
#[test]
fn sweeping_the_aim_never_lifts_walk_speed() {
    let desc = descriptor();
    let walk = desc.ground.speed.walk;
    for degrees_per_second in [45.0_f32, 180.0, 360.0] {
        let world = floor_world();
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        let mut yaw = 0.0_f32;
        for _ in 0..60 {
            step(&mut component, &world, &forward(yaw, false), &mut position);
        }
        let mut slowest = f32::MAX;
        for tick in 0..120 {
            yaw += degrees_per_second.to_radians() * DT;
            step(&mut component, &world, &forward(yaw, false), &mut position);
            let speed = horizontal(component.velocity).length();
            assert!(
                speed <= walk * (1.0 + 1.0e-4),
                "{degrees_per_second}°/s tick {tick}: speed {speed} above walk {walk}"
            );
            slowest = slowest.min(speed);
        }
        if degrees_per_second <= 45.0 {
            assert!(
                slowest > walk * 0.98,
                "a gentle sweep keeps walk speed: slowest {slowest}"
            );
        }
    }
}

// Releasing input stops the pawn at exactly zero. Above the stop speed the
// decay is exponential; below it each tick removes stop speed × friction × dt,
// so the last stretch takes a fixed number of ticks. With no stop speed, the
// same release is still creeping when the floored one has stopped.
#[test]
fn releasing_input_stops_at_exactly_zero_after_a_linear_final_approach() {
    let desc = descriptor();
    let (friction, stop_speed) = (desc.ground.friction, desc.ground.stop_speed);
    let linear_drop = stop_speed * friction * DT;
    for running in [false, true] {
        let world = floor_world();
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        for _ in 0..90 {
            step(
                &mut component,
                &world,
                &forward(0.0, running),
                &mut position,
            );
        }
        let start = horizontal(component.velocity).length();
        let exponential_ticks = ((stop_speed / start).ln() / (1.0 - friction * DT).ln()).ceil();
        let linear_ticks = (1.0 / (friction * DT)).ceil();
        let bound = (exponential_ticks + linear_ticks) as usize + 1;

        let mut floorless = component.clone();
        floorless.ground_params.stop_speed = 0.0;
        let mut floorless_position = position;

        let mut previous = start;
        let mut stopped_at = None;
        for tick in 1..=bound {
            step(&mut component, &world, &idle_input(), &mut position);
            step(
                &mut floorless,
                &world,
                &idle_input(),
                &mut floorless_position,
            );
            let speed = horizontal(component.velocity).length();
            if previous <= stop_speed && speed > 0.0 {
                assert!(
                    (previous - speed - linear_drop).abs() < 1.0e-4,
                    "tick {tick}: below the stop speed the drop is linear: {previous} -> {speed}"
                );
            }
            if speed == 0.0 {
                stopped_at = Some(tick);
                break;
            }
            previous = speed;
        }
        let stopped_at = stopped_at.unwrap_or_else(|| {
            panic!("running={running}: the pawn stops at exactly zero within {bound} ticks")
        });
        assert!(
            horizontal(floorless.velocity).length() > 1.0e-3,
            "running={running}: with no stop speed the pawn still creeps at tick {stopped_at}"
        );
    }
}

// With acceleration at or below friction, ground speed settles at
// accel / friction of the cap instead of reaching it: an allowed, documented
// sluggish tuning. Acceleration above friction reaches the cap.
#[test]
fn accel_at_or_below_friction_settles_below_the_cap() {
    let world = floor_world();
    for (accel, expected) in [(4.0_f32, 4.0 / 8.0), (8.0, 1.0), (10.0, 1.0)] {
        let mut desc = descriptor();
        desc.ground.friction = 8.0;
        desc.ground.accel = accel;
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        for _ in 0..300 {
            step(&mut component, &world, &forward(0.0, false), &mut position);
        }
        let speed = horizontal(component.velocity).length();
        let walk = desc.ground.speed.walk;
        assert!(
            (speed - walk * expected).abs() < 0.05,
            "accel {accel}: settled at {speed}, expected {}",
            walk * expected
        );
    }
}

// Speed banked above the cap (a dash or slide exit) bleeds back to the cap
// with input held and never rises on the way down.
#[test]
fn held_input_above_the_cap_bleeds_without_rising() {
    let desc = descriptor();
    let world = floor_world();
    let (mut component, mut position) = grounded_pawn(&desc, &world);
    let walk = desc.ground.speed.walk;
    let dir = heading(0.0);
    component.velocity = Vec3::new(dir.x * 16.0, 0.0, dir.y * 16.0);
    let mut previous = 16.0;
    for tick in 0..60 {
        step(&mut component, &world, &forward(0.0, false), &mut position);
        let speed = horizontal(component.velocity).length();
        assert!(
            speed <= previous + 1.0e-4,
            "tick {tick}: banked speed only bleeds: {previous} -> {speed}"
        );
        previous = speed;
    }
    assert!(
        (previous - walk).abs() < 1.0e-3,
        "banked speed settles at the walk cap: {previous}"
    );
}

// A jump on a grounded tick leaves the ground before friction runs, so a
// bunny-hop keeps its banked speed; the same tick without the jump bleeds it.
#[test]
fn a_jump_skips_ground_friction_and_a_grounded_tick_applies_it() {
    let mut desc = descriptor();
    desc.air.bunny_hop = true;
    let world = floor_world();
    let banked = 15.0;
    let dir = heading(0.0);
    for jump in [true, false] {
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        component.velocity = Vec3::new(dir.x * banked, 0.0, dir.y * banked);
        let input = MovementInput {
            jump_pressed: jump,
            ..forward(0.0, false)
        };
        step(&mut component, &world, &input, &mut position);
        let speed = horizontal(component.velocity).length();
        if jump {
            assert!(
                !component.is_grounded(),
                "fixture: the jump leaves the ground"
            );
            assert!(
                speed >= banked - 1.0e-3,
                "the jump tick keeps banked speed: {speed}"
            );
        } else {
            let bled = banked * (1.0 - desc.ground.friction * DT);
            assert!(
                (speed - bled).abs() < 1.0e-3,
                "a grounded tick bleeds banked speed: {speed}, expected {bled}"
            );
        }
    }
}

// With input held, friction has no stop speed: a tier whose settled speed sits
// below the stop speed still settles at accel / friction of it (or reaches it),
// rather than being braked to a tick-rate creep every tick.
#[test]
fn held_input_below_the_stop_speed_settles_without_a_creep() {
    let world = floor_world();
    for (walk, accel, expected) in [(1.0_f32, 15.0_f32, 1.0_f32), (7.0, 2.0, 7.0 * 2.0 / 8.0)] {
        let mut desc = descriptor();
        desc.ground.speed.walk = walk;
        desc.ground.accel = accel;
        desc.ground.friction = 8.0;
        assert!(
            expected < desc.ground.stop_speed,
            "fixture: below the stop speed"
        );
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        for _ in 0..300 {
            step(&mut component, &world, &forward(0.0, false), &mut position);
        }
        let speed = horizontal(component.velocity).length();
        assert!(
            (speed - expected).abs() < 0.05,
            "walk {walk}, accel {accel}: settled at {speed}, expected {expected}"
        );
    }
}
