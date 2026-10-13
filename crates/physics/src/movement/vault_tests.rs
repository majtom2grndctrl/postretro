// Vault: ledge reach and clearance, the mantle path, exit speed and grounding.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use postretro_collision::EnvironmentView;
use postretro_foundation::{
    MovementState, MovementStateKind, PlayerMovementComponent, PlayerMovementDescriptor,
    VaultEntry, VaultParams, VaultTrigger,
};

use super::environment_tests::{descriptor, idle_input};
use super::substrate::step_up_limit;
use super::{MovementEvents, MovementInput, tick_with_immersion};
use crate::collision::{CollisionWorld, SKIN_DISTANCE};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
/// The fixture capsule's reach from center to feet (`half_height + radius`).
const REACH: f32 = 1.2;
const RADIUS: f32 = 0.4;
const STEP_HEIGHT: f32 = 0.3;
const MAX_HEIGHT: f32 = 1.6;
const DURATION_MS: f32 = 320.0;
/// The riser faces -X at this x; the pawn approaches along +X.
const RISER_X: f32 = 5.0;

pub(super) fn vault_params(from: VaultEntry) -> VaultParams {
    VaultParams {
        max_height: MAX_HEIGHT,
        swim_max_height: 0.9,
        duration_ms: DURATION_MS,
        from,
        allow_fire: false,
    }
}

fn vault_descriptor(from: VaultEntry) -> PlayerMovementDescriptor {
    let mut desc = descriptor();
    assert_eq!(desc.ground.step_height, STEP_HEIGHT);
    desc.vault = Some(vault_params(from));
    desc
}

fn ground_auto() -> VaultEntry {
    VaultEntry {
        ground: Some(VaultTrigger::Auto),
        ..VaultEntry::default()
    }
}

/// Push a quad whose winding faces `normal`.
fn quad(points: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, corners: [Vec3; 4], normal: Vec3) {
    let base = points.len() as u32;
    points.extend(corners);
    let face = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
    if face.dot(normal) >= 0.0 {
        tris.push([base, base + 1, base + 2]);
        tris.push([base, base + 2, base + 3]);
    } else {
        tris.push([base, base + 2, base + 1]);
        tris.push([base, base + 3, base + 2]);
    }
}

/// Floor at y = 0 for x < `RISER_X`, a riser facing -X, and a top at `top_y`
/// beyond it, optionally under a ceiling at `ceiling_y`.
fn ledge_world(top_y: f32, ceiling_y: Option<f32>) -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    let (z0, z1) = (-50.0, 50.0);
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(-50.0, 0.0, z0),
            Vec3::new(RISER_X, 0.0, z0),
            Vec3::new(RISER_X, 0.0, z1),
            Vec3::new(-50.0, 0.0, z1),
        ],
        Vec3::Y,
    );
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(RISER_X, 0.0, z0),
            Vec3::new(RISER_X, top_y, z0),
            Vec3::new(RISER_X, top_y, z1),
            Vec3::new(RISER_X, 0.0, z1),
        ],
        Vec3::NEG_X,
    );
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(RISER_X, top_y, z0),
            Vec3::new(60.0, top_y, z0),
            Vec3::new(60.0, top_y, z1),
            Vec3::new(RISER_X, top_y, z1),
        ],
        Vec3::Y,
    );
    if let Some(ceiling_y) = ceiling_y {
        quad(
            &mut points,
            &mut tris,
            [
                Vec3::new(RISER_X, ceiling_y, z0),
                Vec3::new(60.0, ceiling_y, z0),
                Vec3::new(60.0, ceiling_y, z1),
                Vec3::new(RISER_X, ceiling_y, z1),
            ],
            Vec3::NEG_Y,
        );
    }
    CollisionWorld::from_triangles_for_test(points, tris)
}

fn forward() -> MovementInput {
    MovementInput {
        // `x` is right; at yaw 0 right is world +X, toward the riser.
        wish_dir: Vec2::new(1.0, 0.0),
        ..idle_input()
    }
}

fn step(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let (position, events, _) = tick_with_immersion(component, input, world, ENV, DT, position);
    (position, events)
}

/// A pawn resting on the floor with its capsule surface `gap` short of the
/// riser, after idle ticks settle it.
fn grounded_at_riser(
    desc: &PlayerMovementDescriptor,
    world: &CollisionWorld,
    gap: f32,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    let mut position = Vec3::new(RISER_X - RADIUS - gap, REACH + 0.05, 0.0);
    for _ in 0..30 {
        position = step(&mut component, world, &idle_input(), position).0;
    }
    assert!(
        component.is_grounded(),
        "fixture pawn settles onto the floor"
    );
    (component, position)
}

/// An airborne pawn with its feet at `feet_y`, capsule surface `gap` short of
/// the riser, moving toward it at `speed`.
fn airborne_at_riser(
    desc: &PlayerMovementDescriptor,
    feet_y: f32,
    gap: f32,
    speed: f32,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    component.velocity = Vec3::new(speed, 0.0, 0.0);
    // Long airborne: the coyote window has run out, as it has mid-jump.
    component.coyote_timer_ms = 1000.0;
    (
        component,
        Vec3::new(RISER_X - RADIUS - gap, feet_y + REACH, 0.0),
    )
}

fn is_vaulting(component: &PlayerMovementComponent) -> bool {
    component.movement_state.kind() == MovementStateKind::Vault
}

/// Feet height of a capsule centred at `position`.
fn feet(position: Vec3) -> f32 {
    position.y - REACH
}

#[test]
fn step_up_limit_is_the_boundary_between_stepping_up_and_vaulting() {
    let desc = vault_descriptor(ground_auto());
    let limit = step_up_limit(STEP_HEIGHT);

    // Just under the limit: step-up climbs it and the pawn never vaults.
    let rest_feet = grounded_at_riser(&desc, &ledge_world(1.0, None), 0.05).1.y - REACH;
    let top_y = rest_feet + limit - 0.001;
    let world = ledge_world(top_y, None);
    let (mut component, mut position) = grounded_at_riser(&desc, &world, 0.05);
    for _ in 0..30 {
        position = step(&mut component, &world, &forward(), position).0;
        assert!(
            !is_vaulting(&component),
            "a ledge under the step-up limit steps up"
        );
    }
    assert!(
        position.x > RISER_X,
        "step-up carried the pawn onto the top"
    );
    assert!(
        (feet(position) - top_y).abs() < 0.05,
        "the pawn stands on the top"
    );

    // Just over it: the first tick vaults.
    let top_y = rest_feet + limit + 0.01;
    let world = ledge_world(top_y, None);
    let (mut component, position) = grounded_at_riser(&desc, &world, 0.05);
    step(&mut component, &world, &forward(), position);
    assert!(
        is_vaulting(&component),
        "a ledge over the step-up limit vaults"
    );
}

#[test]
fn max_height_is_the_last_reachable_top() {
    let desc = vault_descriptor(ground_auto());
    let rest_feet = grounded_at_riser(&desc, &ledge_world(1.0, None), 0.05).1.y - REACH;

    for (top_above_feet, vaults) in [(MAX_HEIGHT - 0.001, true), (MAX_HEIGHT + 0.01, false)] {
        let world = ledge_world(rest_feet + top_above_feet, None);
        let (mut component, position) = grounded_at_riser(&desc, &world, 0.05);
        step(&mut component, &world, &forward(), position);
        assert_eq!(
            is_vaulting(&component),
            vaults,
            "a top {top_above_feet} m above the feet against maxHeight {MAX_HEIGHT}"
        );
    }
}

#[test]
fn the_top_needs_room_for_the_standing_capsule() {
    let desc = vault_descriptor(VaultEntry::default());
    let top_y = 1.2;
    // The probe clears the standing capsule one skin over its rest height, and
    // the sweep keeps one more skin from the ceiling.
    let fits = top_y + 2.0 * REACH + 3.0 * SKIN_DISTANCE;
    for (ceiling_y, vaults) in [(fits + 0.005, true), (fits - 0.01, false)] {
        let world = ledge_world(top_y, Some(ceiling_y));
        let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        step(&mut component, &world, &forward(), position);
        assert_eq!(
            is_vaulting(&component),
            vaults,
            "a ceiling {:.3} m over a top needing {fits:.3} m",
            ceiling_y - top_y
        );
    }
}

#[test]
fn a_vault_stands_the_pawn_on_the_top_with_its_entry_speed() {
    let desc = vault_descriptor(VaultEntry::default());
    let top_y = 1.2;
    let world = ledge_world(top_y, None);
    let speed = 7.0;
    // Resting contact leaves a body one skin from the wall it pressed into.
    for gap in [0.05, SKIN_DISTANCE] {
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, gap, speed);
        let mut vault_ticks = 0;
        let mut edges = Vec::new();
        for _ in 0..60 {
            let (next, events) = step(&mut component, &world, &forward(), position);
            edges.extend(events.state_edges);
            if is_vaulting(&component) {
                vault_ticks += 1;
            }
            let exited = edges
                .iter()
                .any(|edge| edge.from == MovementStateKind::Vault);
            if exited {
                position = next;
                break;
            }
            position = next;
        }
        let path_ticks = (DURATION_MS / (DT * 1000.0)).ceil() as usize;
        assert!(
            vault_ticks <= path_ticks + 1,
            "gap {gap}: {vault_ticks} vault ticks against {path_ticks} path ticks plus one"
        );
        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.from, edge.to))
                .collect::<Vec<_>>(),
            vec![
                (MovementStateKind::Normal, MovementStateKind::Vault),
                (MovementStateKind::Vault, MovementStateKind::Normal),
            ],
            "gap {gap}"
        );
        assert_eq!(component.movement_state, MovementState::Normal);
        assert!(
            component.is_grounded(),
            "gap {gap}: the vault leaves the pawn grounded"
        );
        assert!(
            position.x > RISER_X + RADIUS,
            "gap {gap}: the pawn is over the top"
        );
        assert!(
            (feet(position) - top_y).abs() < 2.0 * SKIN_DISTANCE,
            "gap {gap}: feet {} rest on the top at {top_y}",
            feet(position)
        );
        let horizontal = Vec2::new(component.velocity.x, component.velocity.z).length();
        assert!(
            (horizontal - speed).abs() < 1.0e-3,
            "gap {gap}: exit speed {horizontal} restores entry speed {speed}"
        );
    }
}

#[test]
fn the_vault_rises_monotonically_without_steps_larger_than_the_path() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let start_feet = feet(position);
    let mut heights = vec![position.y];
    for _ in 0..60 {
        let was_vaulting = is_vaulting(&component);
        position = step(&mut component, &world, &forward(), position).0;
        if !was_vaulting && !is_vaulting(&component) && heights.len() > 1 {
            break;
        }
        if is_vaulting(&component) || was_vaulting {
            heights.push(position.y);
        }
    }
    let rise = 1.2 + SKIN_DISTANCE - start_feet;
    let ticks = DURATION_MS / (DT * 1000.0);
    // Smoothstep over the rise share peaks at 1.5x its mean slope.
    let largest_path_step = 1.5 * rise / (0.6 * ticks) + 1.0e-3;
    for pair in heights.windows(2) {
        let step = pair[1] - pair[0];
        assert!(
            step >= -1.0e-3,
            "eye height fell {step} m mid-vault: {heights:?}"
        );
        assert!(
            step <= largest_path_step,
            "eye rose {step} m in one tick, path step is {largest_path_step}"
        );
    }
}

// --- Entry contexts, triggers and probe cost -------------------------------

use super::ledge_probe::take_probe_casts;
use postretro_foundation::{
    BoolOrIr, CrouchParams, DashParams, ForgivenessParams, NumberOrIr, SlideParams,
};

fn with_crouch_slide_dash(mut desc: PlayerMovementDescriptor) -> PlayerMovementDescriptor {
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
        air_dashes: 1,
        preserve_vertical: BoolOrIr::Literal(false),
    });
    desc
}

fn away() -> MovementInput {
    MovementInput {
        wish_dir: Vec2::new(-1.0, 0.0),
        ..idle_input()
    }
}

fn jump_forward() -> MovementInput {
    MovementInput {
        jump_pressed: true,
        ..forward()
    }
}

/// One tick from a fresh counter, returning whether it entered `Vault` and how
/// many casts the probe made.
fn probe_tick(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    position: Vec3,
) -> (bool, u32) {
    take_probe_casts();
    step(component, world, input, position);
    (is_vaulting(component), take_probe_casts())
}

#[test]
fn without_a_vault_block_a_valid_ledge_never_vaults_and_never_probes() {
    let desc = descriptor();
    let world = ledge_world(1.2, None);
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    assert_eq!(
        probe_tick(&mut component, &world, &forward(), position),
        (false, 0)
    );
    let (mut component, position) = grounded_at_riser(&desc, &world, 0.05);
    assert_eq!(
        probe_tick(&mut component, &world, &forward(), position),
        (false, 0)
    );
}

#[test]
fn the_probe_casts_nothing_on_a_tick_where_no_entry_could_fire() {
    let world = ledge_world(1.2, None);
    let default_desc = with_crouch_slide_dash(vault_descriptor(VaultEntry::default()));

    // Grounded under the default `from`, walking into the riser.
    let (mut component, position) = grounded_at_riser(&default_desc, &world, 0.05);
    assert_eq!(
        probe_tick(&mut component, &world, &forward(), position),
        (false, 0),
        "grounded, default from"
    );

    // Airborne at the riser with no horizontal wish.
    let (mut component, position) = airborne_at_riser(&default_desc, 0.5, 0.05, 0.0);
    assert_eq!(
        probe_tick(&mut component, &world, &idle_input(), position),
        (false, 0),
        "no wish"
    );

    // Crouching or sliding into the riser, airborne so `air` would apply.
    for state in [
        MovementState::Crouching { eye_current: 0.2 },
        MovementState::Sliding {
            elapsed_ms: 0.0,
            boost: Vec3::ZERO,
            eye_current: 0.2,
        },
    ] {
        let (mut component, position) = airborne_at_riser(&default_desc, 0.5, 0.05, 7.0);
        component.movement_state = state;
        let (_, casts) = probe_tick(&mut component, &world, &forward(), position);
        assert!(!is_vaulting(&component), "{state:?} never vaults");
        assert_eq!(casts, 0, "{state:?}");
    }

    // A `"jump"` context with jump neither held nor buffered.
    let jump_air = with_crouch_slide_dash(vault_descriptor(VaultEntry {
        air: Some(VaultTrigger::Jump),
        ..VaultEntry::default()
    }));
    let (mut component, position) = airborne_at_riser(&jump_air, 0.5, 0.05, 7.0);
    assert_eq!(
        probe_tick(&mut component, &world, &forward(), position),
        (false, 0),
        "jump context without jump"
    );
}

#[test]
fn the_probe_stops_at_one_sweep_on_open_floor_and_never_exceeds_four() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);

    // Airborne and pushing away from the riser: nothing ahead.
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 0.0);
    assert_eq!(
        probe_tick(&mut component, &world, &away(), position),
        (false, 1)
    );

    // A passing tick runs the full probe.
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let (vaulted, casts) = probe_tick(&mut component, &world, &forward(), position);
    assert!(vaulted);
    assert!(casts <= 4, "a passing tick made {casts} casts");

    // Once vaulting, the probe stays idle.
    let (_, casts) = probe_tick(&mut component, &world, &forward(), position);
    assert_eq!(casts, 0, "a vaulting pawn never probes");
}

#[test]
fn air_auto_vaults_into_the_ledge_and_refuses_without_or_against_the_wish() {
    let desc = with_crouch_slide_dash(vault_descriptor(VaultEntry::default()));
    let world = ledge_world(1.2, None);
    for (label, input, vaults) in [
        ("into the ledge", forward(), true),
        ("no wish", idle_input(), false),
        ("away from it", away(), false),
    ] {
        let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 0.0);
        step(&mut component, &world, &input, position);
        assert_eq!(is_vaulting(&component), vaults, "{label}");
    }

    // An airborne dash into the riser vaults, keeping the dash's speed.
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 18.0);
    component.movement_state = MovementState::Dash {
        elapsed_ms: 16.0,
        boost: Vec3::new(11.0, 0.0, 0.0),
    };
    step(&mut component, &world, &forward(), position);
    let MovementState::Vault { entry_speed, .. } = component.movement_state else {
        panic!(
            "an airborne dash vaults, got {:?}",
            component.movement_state
        );
    };
    assert!(
        (entry_speed - 18.0).abs() < 1.0e-3,
        "dash speed {entry_speed}"
    );
}

#[test]
fn default_from_never_vaults_from_a_grounded_tick() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    for input in [forward(), jump_forward()] {
        let (mut component, mut position) = grounded_at_riser(&desc, &world, 0.05);
        for _ in 0..40 {
            let grounded = component.is_grounded();
            let before = component.movement_state.kind();
            position = step(&mut component, &world, &input, position).0;
            if grounded && before != MovementStateKind::Vault {
                assert!(
                    !is_vaulting(&component),
                    "a grounded tick entered a vault (jump {})",
                    input.jump_pressed
                );
            }
        }
    }
}

#[test]
fn ground_jump_vaults_on_the_jump_and_ground_auto_on_the_walk() {
    let world = ledge_world(1.2, None);
    let jump = vault_descriptor(VaultEntry {
        ground: Some(VaultTrigger::Jump),
        ..VaultEntry::default()
    });
    let (mut component, position) = grounded_at_riser(&jump, &world, 0.05);
    step(&mut component, &world, &forward(), position);
    assert!(
        !is_vaulting(&component),
        "walking in does not vault under ground: jump"
    );
    let (mut component, position) = grounded_at_riser(&jump, &world, 0.05);
    step(&mut component, &world, &jump_forward(), position);
    assert!(
        is_vaulting(&component),
        "a grounded jump into the ledge vaults"
    );
    assert!(component.jump_spent, "the vault spends the jump");

    let auto = vault_descriptor(ground_auto());
    let (mut component, position) = grounded_at_riser(&auto, &world, 0.05);
    step(&mut component, &world, &forward(), position);
    assert!(
        is_vaulting(&component),
        "walking in vaults under ground: auto"
    );
}

#[test]
fn air_jump_needs_jump_and_consumes_a_buffered_one() {
    let mut desc = vault_descriptor(VaultEntry {
        air: Some(VaultTrigger::Jump),
        ..VaultEntry::default()
    });
    desc.forgiveness = Some(ForgivenessParams {
        coyote_ms: 0.0,
        jump_buffer_ms: 150.0,
    });
    let world = ledge_world(1.2, None);

    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 0.0);
    step(&mut component, &world, &forward(), position);
    assert!(!is_vaulting(&component), "no jump, no vault");

    // A press out of reach arms the buffer; reaching the riser with the button
    // released fires the vault off it.
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 1.0, 0.0);
    position = step(
        &mut component,
        &world,
        &MovementInput {
            jump_pressed: true,
            ..idle_input()
        },
        position,
    )
    .0;
    assert!(
        component.jump_buffer_timer_ms > 0.0,
        "the press armed the buffer"
    );
    assert!(!is_vaulting(&component));
    position.x = RISER_X - RADIUS - 0.05;
    step(&mut component, &world, &forward(), position);
    assert!(is_vaulting(&component), "the buffered jump fires the vault");
    assert_eq!(
        component.jump_buffer_timer_ms, 0.0,
        "the vault consumes the buffer"
    );
    assert!(component.jump_spent);
}

#[test]
fn a_ground_only_from_forbids_air_entry() {
    let desc = vault_descriptor(VaultEntry {
        air: None,
        ground: Some(VaultTrigger::Jump),
        swim: None,
    });
    let world = ledge_world(1.2, None);
    for input in [forward(), jump_forward()] {
        let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        let (vaulted, casts) = probe_tick(&mut component, &world, &input, position);
        assert!(
            !vaulted,
            "air entry is forbidden (jump {})",
            input.jump_pressed
        );
        assert_eq!(casts, 0);
    }
}
