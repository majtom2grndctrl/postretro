// Movement in fluid volumes: swim thresholds, drag, sink, current, fluid jump and its re-entry latch, standing capsule, wade-only.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md (Decisions: Swim is a native movement state)

use glam::{Vec2, Vec3};

use postretro_collision::{
    BodyProbe, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, ResolvedEnvironment,
    fluid_at_eye, resolve_environment,
};
use postretro_foundation::{
    BoolOrIr, CrouchParams, DashParams, MovementState, MovementStateKind, NumberOrIr,
    PlayerMovementComponent, PlayerMovementDescriptor, SlideParams, SwimParams,
};

use super::environment_tests::{box_leaf, descriptor, idle_input, push_box};
use super::{MovementEvents, MovementInput, MovementStateEdge, tick, tick_with_immersion};
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const LEVEL_GRAVITY: f32 = -20.0;
const STANDARD_GRAVITY: f32 = 9.81;
const WATER_VISCOSITY: f32 = 2.0;
const VEL_EPS: f32 = 1.0e-4;
const POS_EPS: f32 = 1.0e-4;
/// The fixture capsule's reach from center to feet or head (`half_height +
/// radius`), and its eye above center, from `environment_tests::descriptor`.
const REACH: f32 = 1.2;
const EYE: f32 = 0.5;

fn swim_params() -> SwimParams {
    SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.8,
        enter_depth: 0.5,
        exit_depth: 0.35,
        fluid_jump_velocity: 6.5,
    }
}

fn swim_descriptor() -> PlayerMovementDescriptor {
    let mut desc = descriptor();
    desc.swim = Some(swim_params());
    desc
}

fn with_swim(mut desc: PlayerMovementDescriptor, swim: SwimParams) -> PlayerMovementDescriptor {
    desc.swim = Some(swim);
    desc
}

/// Swim plus crouch, slide and dash, for the any-state entry rows.
fn full_descriptor() -> PlayerMovementDescriptor {
    let mut desc = swim_descriptor();
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
    desc.dash = Some(DashParams {
        boost_speed: NumberOrIr::Literal(18.0),
        momentum_retention: NumberOrIr::Literal(0.65),
        steer_control: NumberOrIr::Literal(0.2),
        dash_drag: NumberOrIr::Literal(18.0),
        cooldown_ms: NumberOrIr::Literal(250.0),
        air_dashes: 0,
        preserve_vertical: BoolOrIr::Literal(false),
    });
    desc
}

/// A fluid box naming fluid 0, optionally setting gravity too.
fn fluid_box(min: Vec3, max: Vec3, gravity: Option<f32>) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity,
        push: None,
        fluid: Some(0),
        brushes: vec![box_leaf(min, max)],
    }
}

/// Open water whose surface is `y = top`, far from any solid.
fn open_water(top: f32) -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![fluid_box(
        Vec3::new(-50.0, -200.0, -50.0),
        Vec3::new(50.0, top, 50.0),
        None,
    )])
}

fn water_view(volumes: &EnvironmentVolumes) -> EnvironmentView<'_> {
    EnvironmentView::new(LEVEL_GRAVITY, volumes, &[WATER_VISCOSITY])
}

/// The capsule center whose immersion below a surface at `top` is `immersion`.
fn center_at_immersion(top: f32, immersion: f32) -> f32 {
    top - immersion * 2.0 * REACH + REACH
}

fn resolve_at(
    view: EnvironmentView<'_>,
    component: &PlayerMovementComponent,
    position: Vec3,
) -> ResolvedEnvironment {
    resolve_environment(
        view,
        &BodyProbe {
            position,
            half_height: component.capsule.half_height,
            radius: component.capsule.radius,
            eye_height: component.capsule.eye_height,
        },
    )
}

fn swimming(desc: &PlayerMovementDescriptor) -> PlayerMovementComponent {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    component.movement_state = MovementState::Swim { fluid: 0 };
    component
}

fn input(f: impl FnOnce(&mut MovementInput)) -> MovementInput {
    let mut input = idle_input();
    f(&mut input);
    input
}

fn jump() -> MovementInput {
    input(|i| i.jump_pressed = true)
}

fn edge(from: MovementStateKind, to: MovementStateKind) -> MovementStateEdge {
    MovementStateEdge { from, to }
}

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

/// The floor plus a ceiling at `ceiling_y` over x in [20, 40].
fn floor_and_crawlspace_world(ceiling_y: f32) -> CollisionWorld {
    let points = vec![
        Vec3::new(-200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, 200.0),
        Vec3::new(-200.0, 0.0, 200.0),
        Vec3::new(20.0, ceiling_y, -200.0),
        Vec3::new(40.0, ceiling_y, -200.0),
        Vec3::new(40.0, ceiling_y, 200.0),
        Vec3::new(20.0, ceiling_y, 200.0),
    ];
    CollisionWorld::from_triangles_for_test(
        points,
        vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]],
    )
}

/// A pool over the floor: x in [20, 40], surface at `top`.
fn floor_pool(top: f32) -> EnvironmentVolume {
    fluid_box(
        Vec3::new(20.0, -1.0, -200.0),
        Vec3::new(40.0, top, 200.0),
        None,
    )
}

fn step(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    view: EnvironmentView<'_>,
    input: &MovementInput,
    position: &mut Vec3,
) -> MovementEvents {
    let (next, events) = tick(component, input, world, view, DT, *position);
    *position = next;
    events
}

/// A pawn settled grounded on `world`'s floor at x = 0, dry.
fn grounded_pawn(
    desc: &PlayerMovementDescriptor,
    world: &CollisionWorld,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    let mut position = Vec3::new(0.0, REACH + 0.01, 0.0);
    let level = EnvironmentView::level_only(LEVEL_GRAVITY);
    for _ in 0..8 {
        step(&mut component, world, level, &idle_input(), &mut position);
    }
    assert!(
        component.is_grounded(),
        "fixture: the pawn settles grounded"
    );
    (component, position)
}

// M1: thresholds with hysteresis, and the swim addresses the edges dispatch.
#[test]
fn swim_enters_at_enter_depth_and_leaves_below_exit_depth_without_toggling_between() {
    use MovementStateKind::{Normal, Swim};
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    // Wade in, bob between the thresholds, rise out, bob again, wade back in.
    let script = [
        (0.2, Normal),
        (0.4, Normal),
        (0.49, Normal),
        (0.6, Swim),
        (0.45, Swim),
        (0.55, Swim),
        (0.4, Swim),
        (0.36, Swim),
        (0.45, Swim),
        (0.3, Normal),
        (0.4, Normal),
        (0.45, Normal),
        (0.49, Normal),
        (0.55, Swim),
    ];
    let mut edges = Vec::new();
    let mut addresses = Vec::new();
    for (immersion, expected) in script {
        let position = Vec3::new(0.0, center_at_immersion(0.0, immersion), 0.0);
        let (_, events) = tick(&mut component, &idle_input(), &world, view, DT, position);
        assert_eq!(
            component.movement_state.kind(),
            expected,
            "after a tick at immersion {immersion}"
        );
        events.append_named_events(&mut addresses);
        edges.extend(events.state_edges);
    }
    assert_eq!(
        edges,
        [edge(Normal, Swim), edge(Swim, Normal), edge(Normal, Swim)],
        "exactly one edge per crossing, none while bobbing between thresholds"
    );
    assert_eq!(addresses, ["swim_started", "swim_ended", "swim_started"]);
}

// M1: immersion exactly at `enter_depth` enters.
#[test]
fn immersion_exactly_at_enter_depth_enters_swim() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let position = Vec3::new(0.0, 0.3, 0.0);
    let at = resolve_at(
        view,
        &PlayerMovementComponent::from_descriptor(&swim_descriptor()),
        position,
    )
    .immersion;
    assert!(at > 0.1 && at < 1.0, "fixture: partly immersed, got {at}");

    for (enter_depth, expected) in [
        (at, MovementStateKind::Swim),
        (at.next_up(), MovementStateKind::Normal),
    ] {
        let desc = with_swim(
            descriptor(),
            SwimParams {
                enter_depth,
                exit_depth: at - 0.1,
                ..swim_params()
            },
        );
        let mut component = PlayerMovementComponent::from_descriptor(&desc);
        tick(
            &mut component,
            &idle_input(),
            &CollisionWorld::new(),
            view,
            DT,
            position,
        );
        assert_eq!(
            component.movement_state.kind(),
            expected,
            "immersion {at} against enter_depth {enter_depth}"
        );
    }
}

// M2: immersion exactly at `exit_depth` stays in swim.
#[test]
fn immersion_exactly_at_exit_depth_stays_in_swim() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let position = Vec3::new(0.0, 0.5, 0.0);
    let at = resolve_at(
        view,
        &PlayerMovementComponent::from_descriptor(&swim_descriptor()),
        position,
    )
    .immersion;
    assert!(at > 0.1 && at < 0.5, "fixture: partly immersed, got {at}");

    for (exit_depth, expected) in [
        (at, MovementStateKind::Swim),
        (at.next_up(), MovementStateKind::Normal),
    ] {
        let desc = with_swim(
            descriptor(),
            SwimParams {
                enter_depth: exit_depth + 0.1,
                exit_depth,
                ..swim_params()
            },
        );
        let mut component = swimming(&desc);
        tick(
            &mut component,
            &idle_input(),
            &CollisionWorld::new(),
            view,
            DT,
            position,
        );
        assert_eq!(
            component.movement_state.kind(),
            expected,
            "immersion {at} against exit_depth {exit_depth}"
        );
    }
}

// M3, swim half: viscosity is the swimmer's drag.
#[test]
fn no_input_swimmer_loses_speed_faster_in_the_more_viscous_fluid() {
    // A weightless pool isolates drag: with no input and zero gravity the wish
    // is zero, so only the fluid acts.
    let volumes = EnvironmentVolumes::build(vec![fluid_box(
        Vec3::splat(-100.0),
        Vec3::splat(100.0),
        Some(0.0),
    )]);
    let speeds = |viscosity: f32| {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, std::slice::from_ref(&viscosity));
        let mut component = swimming(&swim_descriptor());
        component.velocity = Vec3::new(5.0, 0.0, 0.0);
        let mut position = Vec3::ZERO;
        (0..30)
            .map(|_| {
                step(
                    &mut component,
                    &CollisionWorld::new(),
                    view,
                    &idle_input(),
                    &mut position,
                );
                assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
                component.velocity.length()
            })
            .collect::<Vec<_>>()
    };
    let water = speeds(1.0);
    let sludge = speeds(4.0);
    assert!(
        (water[0] - 5.0 * (1.0 - DT)).abs() < VEL_EPS,
        "{}",
        water[0]
    );
    assert!(
        (sludge[0] - 5.0 * (1.0 - 4.0 * DT)).abs() < VEL_EPS,
        "{}",
        sludge[0]
    );
    for (tick, (water, sludge)) in water.iter().zip(&sludge).enumerate() {
        assert!(
            sludge < water,
            "tick {tick}: the more viscous fluid is slower ({sludge} vs {water})"
        );
    }
}

// M3, wading half: viscosity only acts while swimming.
#[test]
fn wading_below_enter_depth_walks_identically_in_fluids_of_any_viscosity() {
    let world = floor_world();
    // A 0.6 m pool: a standing capsule is a quarter immersed, below enter_depth.
    let volumes = EnvironmentVolumes::build(vec![floor_pool(0.6)]);
    let walk = |viscosity: f32| {
        let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, std::slice::from_ref(&viscosity));
        let (mut component, mut position) = grounded_pawn(&swim_descriptor(), &world);
        position.x = 22.0;
        let strafe = input(|i| i.wish_dir = Vec2::new(1.0, 0.0));
        for _ in 0..90 {
            step(&mut component, &world, view, &strafe, &mut position);
            assert_eq!(component.movement_state, MovementState::Normal);
            assert!(resolve_at(view, &component, position).immersion < 0.5);
        }
        (position, component.velocity)
    };
    let (water_position, water_velocity) = walk(1.0);
    let (sludge_position, sludge_velocity) = walk(9.0);
    assert!(
        (water_velocity.x - 7.0).abs() < 1.0e-3,
        "the wader reaches walk speed: {water_velocity:?}"
    );
    assert_eq!(water_position, sludge_position);
    assert_eq!(water_velocity, sludge_velocity);
}

// M4: the fluid jump from the surface, and none fully submerged.
#[test]
fn fluid_jump_with_the_tick_eye_out_sets_jump_velocity_and_leaves_swim() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();

    // Eye 26 cm above the surface, immersion 0.6, nothing solid in reach.
    let mut position = Vec3::new(0.0, center_at_immersion(0.0, 0.6), 0.0);
    let mut component = swimming(&swim_descriptor());
    assert!(!resolve_at(view, &component, position).tick_eye_in_fluid);
    let events = step(&mut component, &world, view, &jump(), &mut position);
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!((component.velocity.y - 6.5).abs() < VEL_EPS);
    assert!(events.jumped);
    assert!(component.swim_reentry_latched);
    assert_eq!(
        events.state_edges,
        [edge(MovementStateKind::Swim, MovementStateKind::Normal)]
    );

    // Fully submerged: jump just swims up.
    let mut position = Vec3::new(0.0, -10.0, 0.0);
    let mut component = swimming(&swim_descriptor());
    assert!(resolve_at(view, &component, position).tick_eye_in_fluid);
    let events = step(&mut component, &world, view, &jump(), &mut position);
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    assert!(!events.jumped);
    assert!(!component.swim_reentry_latched);
    assert!(
        component.velocity.y > 0.0 && component.velocity.y <= 6.0 * DT + VEL_EPS,
        "no boost, only swim acceleration: vy={}",
        component.velocity.y
    );
}

// M5 / P10: the re-entry latch.
#[test]
fn fluid_jump_latches_swim_reentry_until_immersion_falls_below_exit_depth() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, center_at_immersion(0.0, 0.6), 0.0);
    step(&mut component, &world, view, &jump(), &mut position);
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!(component.swim_reentry_latched);

    // Tick N+1: one tick's rise leaves the capsule still at or past
    // enter_depth, yet no swim, and the jump's vertical velocity carries under
    // gravity with no swim drag.
    assert!(resolve_at(view, &component, position).immersion >= 0.5);
    step(&mut component, &world, view, &idle_input(), &mut position);
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!(
        (component.velocity.y - (6.5 + LEVEL_GRAVITY * DT)).abs() < VEL_EPS,
        "vy={}",
        component.velocity.y
    );

    // Deep, between the thresholds, then below exit_depth: the latch holds
    // until immersion first falls below exit_depth, then swim re-enters.
    for (immersion, latched) in [(0.9, true), (0.4, true), (0.3, false)] {
        let position = Vec3::new(0.0, center_at_immersion(0.0, immersion), 0.0);
        tick(&mut component, &idle_input(), &world, view, DT, position);
        assert_eq!(
            component.movement_state,
            MovementState::Normal,
            "at {immersion}"
        );
        assert_eq!(component.swim_reentry_latched, latched, "at {immersion}");
    }
    let position = Vec3::new(0.0, center_at_immersion(0.0, 0.6), 0.0);
    tick(&mut component, &idle_input(), &world, view, DT, position);
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
}

// Q1 (review panel 1): a fluid jump in open water rises clear of `exit_depth`,
// so its latch clears on the way up and the pawn swims again when it falls
// back in. No latch outlives the leap.
#[test]
fn fluid_jump_in_open_water_clears_its_latch_on_the_way_up_and_swims_on_falling_back() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, center_at_immersion(0.0, 0.6), 0.0);
    let events = step(&mut component, &world, view, &jump(), &mut position);
    assert!(events.jumped && component.swim_reentry_latched);

    let mut cleared_rising = false;
    let mut swims_again = false;
    for _ in 0..240 {
        let rising = component.velocity.y > 0.0;
        step(&mut component, &world, view, &idle_input(), &mut position);
        if !component.swim_reentry_latched && component.movement_state == MovementState::Normal {
            cleared_rising |= rising;
        }
        if component.movement_state.kind() == MovementStateKind::Swim {
            swims_again = true;
            break;
        }
    }
    assert!(cleared_rising, "the latch clears before the apex");
    assert!(swims_again, "falling back in re-enters swim");
}

/// Open water whose surface is `y = 0`, a ceiling at `ceiling_y` over it.
fn ceiling_world(ceiling_y: f32) -> CollisionWorld {
    let points = vec![
        Vec3::new(-200.0, ceiling_y, -200.0),
        Vec3::new(200.0, ceiling_y, -200.0),
        Vec3::new(200.0, ceiling_y, 200.0),
        Vec3::new(-200.0, ceiling_y, 200.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 2, 1], [0, 3, 2]])
}

/// Hold jump at the surface for `ticks` ticks and report whether any tick
/// fired a fluid jump; asserts the pawn treads in swim, never latched, on every
/// tick that did not.
fn holds_jump_at_the_surface(
    world: &CollisionWorld,
    view: EnvironmentView<'_>,
    immersion: f32,
    ticks: u32,
) -> bool {
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, center_at_immersion(0.0, immersion), 0.0);
    assert!(
        !resolve_at(view, &component, position).tick_eye_in_fluid,
        "fixture: the tick eye is out"
    );
    for _ in 0..ticks {
        let events = step(&mut component, world, view, &jump(), &mut position);
        if events.jumped {
            return true;
        }
        assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
        assert!(
            !component.swim_reentry_latched,
            "a jump that never fired never latches"
        );
    }
    false
}

// Q1: a ceiling too low for the jump to clear the water keeps the swimmer
// treading, never latched; with headroom the same jump fires.
#[test]
fn fluid_jump_under_a_low_ceiling_does_nothing_and_keeps_swimming() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    // Immersion 0.6 needs a 0.61 m rise; the head starts 0.96 m up.
    assert!(!holds_jump_at_the_surface(
        &ceiling_world(1.3),
        view,
        0.6,
        60
    ));
    assert!(holds_jump_at_the_surface(&ceiling_world(1.8), view, 0.6, 1));
}

// Q1: under strong gravity the ballistic rise falls short of what clearing
// exit_depth needs, so jump at the surface does nothing.
#[test]
fn fluid_jump_under_strong_gravity_does_nothing_and_keeps_swimming() {
    let world = CollisionWorld::new();
    // Immersion 0.7 (tick eye 2 cm out) needs a 0.85 m rise: 6.5² / 60 = 0.70 m
    // under −30 falls short, 6.5² / 40 = 1.06 m under the level's −20 clears.
    let strong = EnvironmentVolumes::build(vec![fluid_box(
        Vec3::new(-50.0, -200.0, -50.0),
        Vec3::new(50.0, 0.0, 50.0),
        Some(-30.0),
    )]);
    assert!(!holds_jump_at_the_surface(
        &world,
        water_view(&strong),
        0.7,
        60
    ));
    let level = open_water(0.0);
    assert!(holds_jump_at_the_surface(
        &world,
        water_view(&level),
        0.7,
        1
    ));
}

// Q1: a downward push above the surface adds to gravity in the ballistic rise.
#[test]
fn fluid_jump_under_a_downward_push_above_the_surface_does_nothing() {
    let world = CollisionWorld::new();
    let water = fluid_box(
        Vec3::new(-50.0, -200.0, -50.0),
        Vec3::new(50.0, 0.0, 50.0),
        None,
    );
    let downdraft = push_box(
        Vec3::new(-50.0, 0.0, -50.0),
        Vec3::new(50.0, 10.0, 50.0),
        Vec3::new(0.0, -80.0, 0.0),
    );
    // Immersion 0.45 puts the capsule center above the surface, inside the
    // downdraft: a 0.25 m rise is needed, 6.5² / 200 = 0.21 m is available.
    let pushed = EnvironmentVolumes::build(vec![water.clone(), downdraft]);
    let center = Vec3::new(0.0, center_at_immersion(0.0, 0.45), 0.0);
    let probe = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    assert_eq!(
        resolve_at(water_view(&pushed), &probe, center).push.y,
        -80.0
    );
    assert!(!holds_jump_at_the_surface(
        &world,
        water_view(&pushed),
        0.45,
        1
    ));
    let calm = EnvironmentVolumes::build(vec![water]);
    assert!(holds_jump_at_the_surface(
        &world,
        water_view(&calm),
        0.45,
        1
    ));
}

// Review panel 1: no same-tick trampoline. A pawn falling into water with jump
// held enters swim on that tick and does not bounce straight back out; the
// fluid jump needs swim at tick start.
#[test]
fn pawn_falling_into_water_with_jump_held_enters_swim_without_bouncing_out() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let mut component = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    component.velocity = Vec3::new(0.0, -5.0, 0.0);
    let mut position = Vec3::new(0.0, center_at_immersion(0.0, 0.6), 0.0);
    let start = resolve_at(view, &component, position);
    assert!(start.immersion >= 0.5 && !start.tick_eye_in_fluid);

    let events = step(
        &mut component,
        &CollisionWorld::new(),
        view,
        &jump(),
        &mut position,
    );
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    assert!(!events.jumped);
    assert!(!component.swim_reentry_latched);
    assert_eq!(
        events.state_edges,
        [edge(MovementStateKind::Normal, MovementStateKind::Swim)]
    );
    assert!(component.velocity.y < 0.0, "vy={}", component.velocity.y);
}

// M6: exit and fluid jump on one tick, the jump wins.
#[test]
fn fluid_jump_wins_over_exit_on_the_same_tick() {
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let start = Vec3::new(0.0, center_at_immersion(0.0, 0.3), 0.0);

    let mut component = swimming(&swim_descriptor());
    let resolved = resolve_at(view, &component, start);
    assert!(resolved.immersion < 0.35 && !resolved.tick_eye_in_fluid);
    let mut position = start;
    let events = step(&mut component, &world, view, &jump(), &mut position);
    assert!((component.velocity.y - 6.5).abs() < VEL_EPS);
    assert!(events.jumped);
    assert!(component.swim_reentry_latched);
    assert_eq!(
        events.state_edges,
        [edge(MovementStateKind::Swim, MovementStateKind::Normal)]
    );

    // Without jump the same tick is a plain exit into falling `Normal`.
    let mut component = swimming(&swim_descriptor());
    let mut position = start;
    let events = step(&mut component, &world, view, &idle_input(), &mut position);
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!((component.velocity.y - LEVEL_GRAVITY * DT).abs() < VEL_EPS);
    assert!(!events.jumped);
    assert!(!component.swim_reentry_latched);
}

// M7: sink scales with the resolved gravity.
#[test]
fn swimmer_sinks_at_sink_speed_scaled_by_the_resolved_gravity() {
    for gravity in [
        -STANDARD_GRAVITY,
        -0.5 * STANDARD_GRAVITY,
        0.0,
        STANDARD_GRAVITY,
    ] {
        let volumes = EnvironmentVolumes::build(vec![fluid_box(
            Vec3::splat(-100.0),
            Vec3::splat(100.0),
            Some(gravity),
        )]);
        let view = water_view(&volumes);
        let expected = 0.8 * gravity / STANDARD_GRAVITY;
        let mut component = swimming(&swim_descriptor());
        let mut position = Vec3::ZERO;
        for _ in 0..120 {
            let before = position;
            step(
                &mut component,
                &CollisionWorld::new(),
                view,
                &idle_input(),
                &mut position,
            );
            assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
            if gravity == 0.0 {
                assert_eq!(component.velocity, Vec3::ZERO, "a weightless pool floats");
                assert_eq!(position, before);
            }
        }
        assert!(
            (component.velocity.y - expected).abs() < VEL_EPS,
            "gravity {gravity}: vy {} vs {expected}",
            component.velocity.y
        );
        let before = position;
        step(
            &mut component,
            &CollisionWorld::new(),
            view,
            &idle_input(),
            &mut position,
        );
        assert!(
            (position.y - before.y - expected * DT).abs() < POS_EPS * DT,
            "gravity {gravity}: the swimmer drifts at {expected} m/s"
        );
    }
}

// M7: a fluid with a coincident push volume is a current.
#[test]
fn swimmer_in_a_fluid_with_a_coincident_push_volume_drifts_with_the_current() {
    let push = Vec3::new(3.0, 0.0, 0.0);
    let min = Vec3::new(-500.0, -200.0, -500.0);
    let max = Vec3::new(500.0, 0.0, 500.0);
    let volumes =
        EnvironmentVolumes::build(vec![fluid_box(min, max, None), push_box(min, max, push)]);
    let view = water_view(&volumes);
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, -50.0, 0.0);
    for _ in 0..600 {
        step(
            &mut component,
            &CollisionWorld::new(),
            view,
            &idle_input(),
            &mut position,
        );
        assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    }
    // The fluid decays the drift: it settles at push / viscosity, not at the
    // ground-friction drift push / 6. A sideways current adds no vertical drift.
    let current = push.x / WATER_VISCOSITY;
    assert!(
        (component.push_velocity.x - current).abs() < 1.0e-3,
        "{:?}",
        component.push_velocity
    );
    assert_eq!(component.push_velocity.y, 0.0);
    assert!(
        (component.velocity.x - current).abs() < 1.0e-3,
        "the swimmer moves with the current: {:?}",
        component.velocity
    );
}

// M8: crouch, dash and slide all enter swim at standing size.
#[test]
fn crouching_dashing_and_sliding_players_enter_swim_with_a_standing_capsule() {
    use MovementStateKind::{Crouch, Dash, Slide, Swim};
    let world = floor_world();
    let pool = EnvironmentVolumes::build(vec![floor_pool(2.0)]);
    let view = water_view(&pool);
    let level = EnvironmentView::level_only(LEVEL_GRAVITY);
    let desc = full_descriptor();
    let crouch = input(|i| i.crouch_intent = true);

    let enter = |state: MovementStateKind,
                 setup: &dyn Fn(&mut PlayerMovementComponent, &mut Vec3)| {
        let (mut component, mut position) = grounded_pawn(&desc, &world);
        setup(&mut component, &mut position);
        assert_eq!(component.movement_state.kind(), state, "fixture state");
        let feet = position.y - component.capsule.half_height - component.capsule.radius;
        position.x = 30.0;
        let held = if state == Dash {
            idle_input()
        } else {
            input(|i| i.crouch_intent = true)
        };
        let events = step(&mut component, &world, view, &held, &mut position);
        assert_eq!(
            component.movement_state.kind(),
            MovementStateKind::Swim,
            "from {state:?}"
        );
        assert_eq!(events.state_edges.first(), Some(&edge(state, Swim)));
        assert!((component.capsule.half_height - 0.8).abs() < 1e-6);
        assert!((component.capsule.eye_height - 0.5).abs() < 1e-6);
        let new_feet = position.y - component.capsule.half_height - component.capsule.radius;
        assert!(
            new_feet > feet - 0.05,
            "the stand-up keeps the feet planted: {new_feet} vs {feet}"
        );
    };

    enter(Crouch, &|component, position| {
        step(component, &world, level, &crouch, position);
    });
    enter(Dash, &|component, position| {
        let dash = input(|i| i.dash_pressed = true);
        step(component, &world, level, &dash, position);
    });
    enter(Slide, &|component, position| {
        component.velocity = Vec3::new(12.0, 0.0, 0.0);
        step(component, &world, level, &crouch, position);
    });
}

// M8: blocked headroom keeps the crouch; swim waits for standing room.
#[test]
fn crouched_pawn_in_a_flooded_crawlspace_stays_crouched_until_it_can_stand() {
    let world = floor_and_crawlspace_world(2.0);
    let pool = EnvironmentVolumes::build(vec![fluid_box(
        Vec3::new(20.0, -1.0, -200.0),
        Vec3::new(60.0, 2.0, 200.0),
        None,
    )]);
    let view = water_view(&pool);
    let level = EnvironmentView::level_only(LEVEL_GRAVITY);
    let crouch = input(|i| i.crouch_intent = true);
    let (mut component, mut position) = grounded_pawn(&full_descriptor(), &world);
    step(&mut component, &world, level, &crouch, &mut position);
    for _ in 0..10 {
        step(&mut component, &world, level, &crouch, &mut position);
    }
    assert!(matches!(
        component.movement_state,
        MovementState::Crouching { .. }
    ));

    position.x = 30.0;
    for _ in 0..30 {
        step(&mut component, &world, view, &crouch, &mut position);
        assert!(
            matches!(component.movement_state, MovementState::Crouching { .. }),
            "the ceiling blocks the standing capsule: {:?}",
            component.movement_state
        );
        assert!(resolve_at(view, &component, position).immersion > 0.9);
    }

    position.x = 50.0;
    step(&mut component, &world, view, &crouch, &mut position);
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    assert!((component.capsule.half_height - 0.8).abs() < 1e-6);
}

// M9, physics half: no swim block means wading.
#[test]
fn player_without_a_swim_block_walks_the_pool_floor() {
    let world = floor_world();
    let pool = EnvironmentVolumes::build(vec![floor_pool(2.0)]);
    let view = water_view(&pool);
    let desc = descriptor();
    assert!(desc.swim.is_none());
    let (mut component, mut position) = grounded_pawn(&desc, &world);
    let strafe = input(|i| i.wish_dir = Vec2::new(1.0, 0.0));
    let mut deepest = 0.0_f32;
    for _ in 0..420 {
        step(&mut component, &world, view, &strafe, &mut position);
        assert_eq!(component.movement_state, MovementState::Normal);
        assert!(component.is_grounded(), "the wader stays on the floor");
        deepest = deepest.max(resolve_at(view, &component, position).immersion);
    }
    assert!(position.x > 40.0, "fixture: the wader crossed the pool");
    assert!(deepest > 0.8, "fixture: the pool is deep, {deepest}");
}

// M10 / P5: the fluid jump keys on the tick eye, never the camera eye.
#[test]
fn fluid_jump_keys_on_the_tick_eye_never_the_presented_camera_eye() {
    // `movement::tick` takes no camera eye: it resolves the tick eye from the
    // position and capsule. A view-feel offset (here a 5 cm head-bob lift) moves
    // only the presented eye; the jump outcome follows the tick eye regardless.
    let volumes = open_water(0.0);
    let view = water_view(&volumes);
    let bob = Vec3::new(0.0, 0.05, 0.0);
    for (tick_eye_y, camera_offset, boosts) in [(-0.02, bob, false), (0.02, -bob, true)] {
        let mut position = Vec3::new(0.0, tick_eye_y - EYE, 0.0);
        let tick_eye = position + Vec3::new(0.0, EYE, 0.0);
        let camera_eye = tick_eye + camera_offset;
        assert_eq!(fluid_at_eye(view, tick_eye).is_some(), !boosts);
        assert_eq!(
            fluid_at_eye(view, camera_eye).is_some(),
            boosts,
            "fixture: the camera eye is across the surface from the tick eye"
        );
        let mut component = swimming(&swim_descriptor());
        step(
            &mut component,
            &CollisionWorld::new(),
            view,
            &jump(),
            &mut position,
        );
        assert_eq!(
            (component.velocity.y - 6.5).abs() < VEL_EPS,
            boosts,
            "tick eye at {tick_eye_y}: vy={}",
            component.velocity.y
        );
        assert_eq!(
            component.movement_state.kind() == MovementStateKind::Swim,
            !boosts,
            "{:?}",
            component.movement_state
        );
    }
}

/// A floating fluid block in mid-air: y in [5, 10].
fn floating_block() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![fluid_box(
        Vec3::new(-50.0, 5.0, -50.0),
        Vec3::new(50.0, 10.0, 50.0),
        None,
    )])
}

// M11: out through the bottom of a floating block.
#[test]
fn swimming_down_out_of_a_floating_block_leaves_swim_and_level_gravity_applies() {
    let volumes = floating_block();
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, 7.5, 0.0);
    let dive = input(|i| i.crouch_intent = true);
    let mut left = false;
    for _ in 0..240 {
        let start = resolve_at(view, &component, position);
        let vy_before = component.velocity.y;
        let events = step(&mut component, &world, view, &dive, &mut position);
        if component.movement_state == MovementState::Normal {
            assert!(
                start.immersion < 0.35,
                "left at immersion {}",
                start.immersion
            );
            assert_eq!(
                events.state_edges,
                [edge(MovementStateKind::Swim, MovementStateKind::Normal)]
            );
            assert!(
                (component.velocity.y - vy_before - LEVEL_GRAVITY * DT).abs() < VEL_EPS,
                "the exit tick falls at level gravity"
            );
            left = true;
            break;
        }
    }
    assert!(left, "the swimmer must leave through the bottom");
    let vy_before = component.velocity.y;
    step(&mut component, &world, view, &idle_input(), &mut position);
    assert!((component.velocity.y - vy_before - LEVEL_GRAVITY * DT).abs() < VEL_EPS);
}

// M12: up into the bottom of a floating block.
#[test]
fn jumping_up_into_a_floating_block_enters_swim_once_immersion_reaches_enter_depth() {
    let volumes = floating_block();
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    component.velocity = Vec3::new(0.0, 12.0, 0.0);
    let mut position = Vec3::new(0.0, 2.0, 0.0);
    let mut entered = false;
    for _ in 0..120 {
        let start = resolve_at(view, &component, position);
        step(&mut component, &world, view, &idle_input(), &mut position);
        let swims = component.movement_state.kind() == MovementStateKind::Swim;
        assert_eq!(
            swims,
            start.immersion >= 0.5,
            "swim tracks the tick-start immersion {}",
            start.immersion
        );
        if swims {
            entered = true;
            break;
        }
    }
    assert!(entered, "the rising pawn must reach enter_depth");
}

// The swim edges carry their fluid for presentation (Task 11's enter and exit
// sounds): the entered fluid, and the one left even when the exit tick starts
// already dry, as leaving a pool through its side face does.
#[test]
fn swim_edges_name_their_fluid_even_when_the_exit_tick_starts_dry() {
    // Fluid 1 of a two-fluid table, so the index is not a default.
    let volumes = EnvironmentVolumes::build(vec![EnvironmentVolume {
        fluid: Some(1),
        ..fluid_box(Vec3::new(-5.0, -50.0, -5.0), Vec3::new(5.0, 0.0, 5.0), None)
    }]);
    let view = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[WATER_VISCOSITY, 4.0]);
    let world = CollisionWorld::new();
    let mut component = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    let deep = Vec3::new(0.0, -5.0, 0.0);

    let (_, entered) = tick(&mut component, &idle_input(), &world, view, DT, deep);
    assert_eq!(component.movement_state, MovementState::Swim { fluid: 1 });
    assert_eq!(entered.swim_fluid, Some(1));

    let (_, swam) = tick(&mut component, &idle_input(), &world, view, DT, deep);
    assert_eq!(swam.swim_fluid, None, "no edge, no fluid");

    let beside = Vec3::new(8.0, -5.0, 0.0);
    assert_eq!(resolve_at(view, &component, beside).fluid, None);
    let (_, left) = tick(&mut component, &idle_input(), &world, view, DT, beside);
    assert_eq!(component.movement_state, MovementState::Normal);
    assert_eq!(left.swim_fluid, Some(1));
}

// Swim slots: the immersion a tick reports for `player.immersion` is the
// resolve its intent used. A crouched pawn entering swim reports the standing
// capsule's immersion it swims with, not the crouched capsule's.
#[test]
fn reported_immersion_is_the_standing_resolve_on_a_crouched_swim_entry() {
    let world = floor_world();
    let pool = EnvironmentVolumes::build(vec![floor_pool(2.0)]);
    let view = water_view(&pool);
    let level = EnvironmentView::level_only(LEVEL_GRAVITY);
    let crouch = input(|i| i.crouch_intent = true);
    let (mut component, mut position) = grounded_pawn(&full_descriptor(), &world);
    step(&mut component, &world, level, &crouch, &mut position);
    position.x = 30.0;
    let crouched = resolve_at(view, &component, position).immersion;
    let standing = resolve_environment(
        view,
        &BodyProbe {
            position: Vec3::new(
                position.x,
                position.y + component.standing_half_height - component.capsule.half_height,
                position.z,
            ),
            half_height: component.standing_half_height,
            radius: component.capsule.radius,
            eye_height: component.standing_eye_height,
        },
    )
    .immersion;
    assert!(
        (crouched - standing).abs() > 0.1,
        "fixture: the two capsules differ: {crouched} vs {standing}"
    );

    let (_, _, reported) = tick_with_immersion(&mut component, &crouch, &world, view, DT, position);
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    assert!(
        (reported - standing).abs() < 1e-6,
        "reported {reported}, standing {standing}"
    );
}

// Swim slots: the reported immersion is the tick-start resolve, never the
// tick-end position's. A pawn falling into a pool from just above its surface
// reports dry on the tick its feet break the surface.
#[test]
fn reported_immersion_is_the_tick_start_resolve_not_the_tick_end_position() {
    let world = floor_world();
    let pool = EnvironmentVolumes::build(vec![floor_pool(2.0)]);
    let view = water_view(&pool);
    let mut component = PlayerMovementComponent::from_descriptor(&swim_descriptor());
    component.velocity = Vec3::new(0.0, -30.0, 0.0);
    let start = Vec3::new(30.0, 2.0 + 0.1 + REACH, 0.0);
    assert_eq!(resolve_at(view, &component, start).immersion, 0.0);

    let (end, _, reported) =
        tick_with_immersion(&mut component, &idle_input(), &world, view, DT, start);
    let at_end = resolve_at(view, &component, end).immersion;
    assert!(
        at_end > 0.05,
        "fixture: the feet end the tick under: {at_end}"
    );
    assert_eq!(reported, 0.0, "the tick-start resolve was dry");
}

/// The idle swimmer's settled sink velocity under the level's gravity.
fn level_sink() -> f32 {
    0.8 * LEVEL_GRAVITY / STANDARD_GRAVITY
}

// Q2 (review panel 1): while swimming the drift layer carries vertical push
// too, so an upward current lifts a mid-water swimmer and its drift settles at
// push / viscosity vertically, while swim steering keeps sinking locomotion.
#[test]
fn upward_current_lifts_a_mid_water_swimmer_and_settles_at_push_over_viscosity() {
    let push = Vec3::new(0.0, 8.0, 0.0);
    let min = Vec3::new(-500.0, -200.0, -500.0);
    let max = Vec3::new(500.0, 0.0, 500.0);
    let volumes =
        EnvironmentVolumes::build(vec![fluid_box(min, max, None), push_box(min, max, push)]);
    let view = water_view(&volumes);
    let mut component = swimming(&swim_descriptor());
    let start = Vec3::new(0.0, -150.0, 0.0);
    let mut position = start;
    for _ in 0..600 {
        step(
            &mut component,
            &CollisionWorld::new(),
            view,
            &idle_input(),
            &mut position,
        );
        assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    }
    let current = push.y / WATER_VISCOSITY;
    assert!(
        (component.push_velocity - Vec3::new(0.0, current, 0.0)).length() < 1.0e-3,
        "{:?}",
        component.push_velocity
    );
    assert!(
        (component.velocity.y - (current + level_sink())).abs() < 1.0e-3,
        "the swimmer rises at the current less its sink: {:?}",
        component.velocity
    );
    assert!(position.y > start.y + 10.0, "lifted to {position:?}");
}

/// A swimmer settled on the floor of a deep pool (`floor_pool(5.0)`), swimming
/// and grounded, under `view_without_current`.
fn swimmer_on_the_pool_floor(
    world: &CollisionWorld,
    view_without_current: EnvironmentView<'_>,
) -> (PlayerMovementComponent, Vec3) {
    let (mut component, mut position) = grounded_pawn(&swim_descriptor(), world);
    position.x = 30.0;
    for _ in 0..30 {
        step(
            &mut component,
            world,
            view_without_current,
            &idle_input(),
            &mut position,
        );
    }
    assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    assert!(
        component.is_grounded(),
        "fixture: the swimmer rests on the floor"
    );
    (component, position)
}

// Q2: the same current lifts a swimmer resting on the pool floor. Its drift
// settles at push / viscosity whether or not the floor bears it, so a current
// lifts from the floor exactly when it lifts mid-water — here one whose push
// (4 m/s²) is below the swim acceleration (6 m/s²) but whose settled drift
// (2 m/s) beats the sink (1.63 m/s). A current whose drift falls short of the
// sink leaves the swimmer on the floor.
#[test]
fn upward_current_lifts_a_swimmer_resting_on_the_pool_floor() {
    let world = floor_world();
    let pool = floor_pool(5.0);
    let calm = EnvironmentVolumes::build(vec![pool.clone()]);
    let current = |push_y: f32| {
        EnvironmentVolumes::build(vec![
            pool.clone(),
            push_box(
                Vec3::new(20.0, -1.0, -200.0),
                Vec3::new(40.0, 5.0, 200.0),
                Vec3::new(0.0, push_y, 0.0),
            ),
        ])
    };
    assert!(4.0 / WATER_VISCOSITY > -level_sink() && 4.0 < swim_params().accel);
    assert!(2.0 / WATER_VISCOSITY < -level_sink());

    let lifting = current(4.0);
    let (mut component, mut position) = swimmer_on_the_pool_floor(&world, water_view(&calm));
    let floor_y = position.y;
    let mut lifted_at = None;
    for tick in 0..240 {
        let events = step(
            &mut component,
            &world,
            water_view(&lifting),
            &idle_input(),
            &mut position,
        );
        assert!(!events.landed, "a swimmer never lands");
        if !component.is_grounded() && lifted_at.is_none() {
            lifted_at = Some(tick);
        }
    }
    assert!(
        lifted_at.is_some(),
        "the current lifts the swimmer off the floor"
    );
    assert!(
        position.y > floor_y + 0.5,
        "the swimmer rises off the floor: {} from {floor_y}",
        position.y
    );

    let weak = current(2.0);
    let (mut component, mut position) = swimmer_on_the_pool_floor(&world, water_view(&calm));
    for _ in 0..240 {
        step(
            &mut component,
            &world,
            water_view(&weak),
            &idle_input(),
            &mut position,
        );
        assert!(
            component.is_grounded(),
            "a current weaker than the sink holds"
        );
    }
    assert!(
        (component.push_velocity.y - 2.0 / WATER_VISCOSITY).abs() < 1.0e-3,
        "the floor leaves the vertical drift to the fluid: {:?}",
        component.push_velocity
    );
}

// Q2: leaving the water upward carries the vertical drift on as vertical
// velocity: total velocity is continuous across the exit edge, and outside
// water the drift layer is horizontal again.
#[test]
fn leaving_the_water_upward_keeps_the_vertical_drift_as_velocity_y() {
    let push = Vec3::new(0.0, 8.0, 0.0);
    let volumes = EnvironmentVolumes::build(vec![
        fluid_box(
            Vec3::new(-50.0, -200.0, -50.0),
            Vec3::new(50.0, 0.0, 50.0),
            None,
        ),
        push_box(
            Vec3::new(-50.0, -200.0, -50.0),
            Vec3::new(50.0, 50.0, 50.0),
            push,
        ),
    ]);
    let view = water_view(&volumes);
    let world = CollisionWorld::new();
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(0.0, -5.0, 0.0);
    for _ in 0..600 {
        let before = component.velocity;
        let drift_before = component.push_velocity;
        let events = step(&mut component, &world, view, &idle_input(), &mut position);
        if component.movement_state == MovementState::Normal {
            assert_eq!(
                events.state_edges,
                [edge(MovementStateKind::Swim, MovementStateKind::Normal)]
            );
            assert!(drift_before.y > 1.0, "fixture: a vertical drift built up");
            assert_eq!(
                component.push_velocity.y, 0.0,
                "the layer is horizontal again"
            );
            let expected = before.y + (LEVEL_GRAVITY + push.y) * DT;
            assert!(
                (component.velocity.y - expected).abs() < VEL_EPS,
                "vy {} vs {expected}: the drift carries on as vertical velocity",
                component.velocity.y
            );
            assert!(component.velocity.y > 0.0);
            return;
        }
    }
    panic!("the current must carry the swimmer out of the water");
}

// Review panel 1: a swimmer settling onto the pool floor fires no `landed`.
#[test]
fn swimmer_settling_onto_the_pool_floor_fires_no_landed_event() {
    let world = floor_world();
    let pool = EnvironmentVolumes::build(vec![floor_pool(5.0)]);
    let view = water_view(&pool);
    let mut component = swimming(&swim_descriptor());
    let mut position = Vec3::new(30.0, 3.0, 0.0);
    let mut touched_down = false;
    for _ in 0..240 {
        let events = step(&mut component, &world, view, &idle_input(), &mut position);
        assert!(
            !events.landed,
            "a swimmer settling on the floor is not a landing"
        );
        assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
        touched_down |= component.is_grounded();
    }
    assert!(touched_down, "fixture: the swimmer sinks onto the floor");
}
