//! Profiles at the fixed-step boundary, including precedence over existing abilities.
use super::*;
use postretro_foundation::{
    LifeStateMovementProfile, LifeStatePosture, LifeStateProfile, enter_life_state,
};

fn profile(scale: f32, abilities: bool, posture: LifeStatePosture) -> LifeStateProfile {
    LifeStateProfile {
        movement: LifeStateMovementProfile {
            speed_scale: scale,
            abilities,
            posture,
        },
        ..Default::default()
    }
}

#[test]
fn life_state_speed_targets_scale_without_scaling_gravity() {
    let world = floor_and_ceiling_world(50.0);
    let desc = canonical_descriptor();
    let (mut pawn, mut position) = settle_player(&desc);
    enter_life_state(
        &mut pawn,
        0,
        &profile(0.25, true, LifeStatePosture::Stand),
        1,
    );
    let input = MovementInput {
        wish_dir: Vec2::Y,
        running: true,
        ..idle_input()
    };
    run_ticks(&mut pawn, &world, &mut position, 120, &input);
    assert!((horiz_speed(&pawn) - desc.ground.speed.run * 0.25).abs() < 0.01);
    pawn.set_grounded(false);
    pawn.velocity = Vec3::ZERO;
    let (_, _) = tick(
        &mut pawn,
        &idle_input(),
        &world,
        ENV,
        DT,
        position + Vec3::Y * 10.0,
    );
    assert!((pawn.velocity.y - GRAVITY * DT).abs() < VEL_EPS);
}

#[test]
fn life_state_forced_crouch_wins_over_dash_slide_release_and_jump_standup() {
    let mut desc = crouch_descriptor();
    desc.dash = Some(dash_params(18.0, 0.0, 0.2, 18.0, 250.0, 1, false));
    desc.slide = Some(SlideParams {
        min_speed: 8.0,
        slide_drag: 2.0,
        slope_assist: 1.0,
        steer_rate: 180.0,
        entry_boost: 1.5,
        min_duration_ms: 250.0,
    });
    let world = floor_and_ceiling_world(50.0);
    let (mut pawn, mut position) = settle_and_run(&desc, &world, 60);
    enter_life_state(
        &mut pawn,
        1,
        &profile(0.5, true, LifeStatePosture::Crouch),
        2,
    );
    let input = MovementInput {
        dash_pressed: true,
        jump_pressed: true,
        ..idle_input()
    };
    run_ticks(&mut pawn, &world, &mut position, 1, &input);
    assert!(is_crouching(&pawn));
    assert_eq!(
        pawn.capsule.half_height,
        desc.crouch.as_ref().unwrap().half_height
    );
    run_ticks(&mut pawn, &world, &mut position, 20, &idle_input());
    assert!(
        is_crouching(&pawn),
        "released crouch input cannot override forced posture"
    );
    enter_life_state(&mut pawn, 0, &LifeStateProfile::default(), 2);
    run_ticks(&mut pawn, &world, &mut position, 1, &idle_input());
    assert!(matches!(pawn.movement_state, MovementState::Normal));
}

#[test]
fn life_state_refused_jump_is_not_buffered_for_recovery() {
    let mut desc = crouch_descriptor();
    desc.forgiveness = Some(ForgivenessParams {
        coyote_ms: 100.0,
        jump_buffer_ms: 150.0,
    });
    desc.dash = Some(dash_params(18.0, 0.0, 0.2, 18.0, 250.0, 1, false));
    let world = floor_and_ceiling_world(50.0);
    let (mut pawn, mut position) = settle_player(&desc);
    run_ticks(&mut pawn, &world, &mut position, 10, &idle_input());
    enter_life_state(
        &mut pawn,
        1,
        &profile(1.0, false, LifeStatePosture::Stand),
        2,
    );
    let blocked = MovementInput {
        jump_pressed: true,
        dash_pressed: true,
        crouch_intent: true,
        ..idle_input()
    };
    let event = run_ticks(&mut pawn, &world, &mut position, 1, &blocked);
    assert!(!event.jumped);
    assert!(matches!(pawn.movement_state, MovementState::Normal));
    assert_eq!(pawn.jump_buffer_timer_ms, 0.0);
    enter_life_state(&mut pawn, 0, &LifeStateProfile::default(), 2);
    assert!(!run_ticks(&mut pawn, &world, &mut position, 1, &idle_input()).jumped);
}

#[test]
fn life_state_forced_crouch_interrupts_an_active_dash_and_stands_only_with_clearance() {
    let mut desc = crouch_descriptor();
    desc.dash = Some(dash_params(18.0, 0.0, 0.2, 18.0, 250.0, 1, false));
    let world = floor_and_ceiling_world(50.0);
    let (mut pawn, mut position) = settle_and_run(&desc, &world, 20);
    run_ticks(
        &mut pawn,
        &world,
        &mut position,
        1,
        &MovementInput {
            wish_dir: Vec2::Y,
            dash_pressed: true,
            ..idle_input()
        },
    );
    assert!(matches!(pawn.movement_state, MovementState::Dash { .. }));
    enter_life_state(
        &mut pawn,
        1,
        &profile(1.0, true, LifeStatePosture::Crouch),
        2,
    );
    run_ticks(&mut pawn, &world, &mut position, 1, &idle_input());
    assert!(is_crouching(&pawn));
    let blocked = floor_and_ceiling_world(1.8);
    enter_life_state(&mut pawn, 0, &LifeStateProfile::default(), 2);
    run_ticks(&mut pawn, &blocked, &mut position, 1, &idle_input());
    assert!(
        is_crouching(&pawn),
        "return to stand uses the ordinary clearance probe"
    );
    run_ticks(&mut pawn, &world, &mut position, 1, &idle_input());
    assert!(matches!(pawn.movement_state, MovementState::Normal));
}

#[test]
fn life_state_dash_boost_and_crouch_wish_read_the_same_scale() {
    let mut descriptor = crouch_descriptor();
    descriptor.dash = Some(dash_params(18.0, 0.0, 0.0, 0.0, 250.0, 1, false));
    let world = floor_and_ceiling_world(50.0);
    let (mut pawn, mut position) = settle_player(&descriptor);
    run_ticks(&mut pawn, &world, &mut position, 10, &idle_input());
    enter_life_state(
        &mut pawn,
        0,
        &profile(0.25, true, LifeStatePosture::Stand),
        1,
    );
    run_ticks(
        &mut pawn,
        &world,
        &mut position,
        1,
        &MovementInput {
            wish_dir: Vec2::Y,
            dash_pressed: true,
            ..idle_input()
        },
    );
    let MovementState::Dash { boost, .. } = pawn.movement_state else {
        panic!("dash entered");
    };
    assert!((boost.length() - 18.0 * 0.25).abs() < VEL_EPS);
    enter_life_state(
        &mut pawn,
        1,
        &profile(0.25, true, LifeStatePosture::Crouch),
        1,
    );
    run_ticks(
        &mut pawn,
        &world,
        &mut position,
        120,
        &MovementInput {
            wish_dir: Vec2::Y,
            ..idle_input()
        },
    );
    assert!((horiz_speed(&pawn) - descriptor.ground.speed.crouch * 0.25).abs() < 0.01);
}
