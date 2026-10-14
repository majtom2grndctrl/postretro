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
use super::vault_path::{VaultPath, vault_clock, vault_path_ticks};
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
        let path_ticks = vault_path_ticks(DURATION_MS, DT * 1000.0) as usize;
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
    let mut heights = vec![position.y];
    let mut path = None;
    for _ in 0..60 {
        let was_vaulting = is_vaulting(&component);
        position = step(&mut component, &world, &forward(), position).0;
        if !was_vaulting && !is_vaulting(&component) && heights.len() > 1 {
            break;
        }
        if let MovementState::Vault {
            target,
            start_offset,
            entry_speed,
            ..
        } = component.movement_state
        {
            path.get_or_insert(VaultPath::new(
                target,
                start_offset,
                entry_speed,
                DURATION_MS,
                DT * 1000.0,
                RADIUS,
            ));
        }
        if is_vaulting(&component) || was_vaulting {
            heights.push(position.y);
        }
    }
    // The path step: the largest rise between consecutive path ticks of the
    // path the intent steered along.
    let path = path.expect("the pawn vaulted");
    let ticks = vault_path_ticks(DURATION_MS, DT * 1000.0) + 1;
    let largest_path_step = (1..=ticks)
        .map(|k| path.aimed(k).0.y - path.aimed(k - 1).0.y)
        .fold(0.0_f32, f32::max)
        + 1.0e-3;
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

/// Every tick's centre from `start` until the vault exits: the entry tick's
/// end first, the exit tick's end last.
fn vault_track(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    start: Vec3,
) -> Vec<Vec3> {
    let mut track = vec![start];
    let (_, addresses, positions) = run_vault(component, world, &forward(), start);
    assert!(addresses.contains(&"vault_ended"), "the vault ends");
    track.extend(positions);
    track
}

#[test]
fn the_move_over_the_top_overlaps_the_rise_and_ends_at_the_exit_speed() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    for speed in [0.0, 3.0, 7.0, 11.0] {
        for gap in [SKIN_DISTANCE, 0.05] {
            let (mut component, start) = airborne_at_riser(&desc, 0.5, gap, speed);
            let track = vault_track(&mut component, &world, start);
            // Path ticks end at `track[1..exit]`; the last entry is the exit tick.
            let exit = track.len() - 1;
            let steps: Vec<Vec3> = track[..exit].windows(2).map(|w| w[1] - w[0]).collect();
            assert!(
                steps.iter().any(|step| step.x > 1.0e-3 && step.y > 1.0e-3),
                "{speed} m/s, gap {gap}: the forward move starts while the pawn still rises"
            );
            for step in &steps {
                assert!(
                    step.x >= -1.0e-4,
                    "{speed} m/s, gap {gap}: forward never reverses"
                );
            }
            let landing_x = track[exit - 1].x;
            assert!(
                track[..exit].iter().all(|p| p.x <= landing_x + 1.0e-4),
                "{speed} m/s, gap {gap}: never past the landing"
            );
            let last = steps.last().unwrap().x / DT;
            if speed >= 3.0 {
                // No crawl then jump: the last path tick already moves at the
                // speed the exit tick restores.
                assert!(
                    (last - speed).abs() < 0.02 * speed,
                    "{speed} m/s, gap {gap}: last path tick moves {last} m/s"
                );
            } else {
                assert!(
                    last < steps[steps.len() - 2].x / DT,
                    "{speed} m/s, gap {gap}: a still entry eases out onto the top"
                );
            }
        }
    }
}

/// Distance in the x–y plane from `point` to the solid `x ≥ RISER_X`,
/// `y ≤ top_y` (the ledge) or, with `above`, `y ≥ top_y` (a ceiling over the
/// top). Negative inside.
fn distance_to_block(point: Vec3, top_y: f32, above: bool) -> f32 {
    let ahead = RISER_X - point.x;
    let off = if above {
        top_y - point.y
    } else {
        point.y - top_y
    };
    match (ahead > 0.0, off > 0.0) {
        (true, true) => Vec2::new(ahead, off).length(),
        (true, false) => ahead,
        (false, true) => off,
        (false, false) => -ahead.abs().min(off.abs()),
    }
}

/// Least distance from the centre segment of a capsule centred at `centre` to
/// the block. The segment is vertical, so its nearest point is its end nearer
/// the block's face, or the face height when the segment spans it.
fn segment_to_block(centre: Vec3, half_height: f32, top_y: f32, above: bool) -> f32 {
    let (low, high) = (centre.y - half_height, centre.y + half_height);
    let y = top_y.clamp(low, high);
    distance_to_block(Vec3::new(centre.x, y, centre.z), top_y, above)
}

#[test]
fn the_eye_and_capsule_stay_clear_of_the_lip_and_the_headroom_along_the_path() {
    let desc = vault_descriptor(VaultEntry::default());
    let component = PlayerMovementComponent::from_descriptor(&desc);
    let half_height = component.capsule.half_height;
    let eye_height = component.capsule.eye_height;
    // Tops from a little over the step-up limit to near `maxHeight` over feet
    // at 0.5 m, and the exactly-fitting ceiling the headroom row vaults under.
    let fits = |top_y: f32| top_y + 2.0 * REACH + 3.0 * SKIN_DISTANCE + 0.005;
    for (top_y, ceiling) in [
        (0.9, None),
        (1.2, None),
        (2.05, None),
        (1.2, Some(fits(1.2))),
        (2.05, Some(fits(2.05))),
    ] {
        let world = ledge_world(top_y, ceiling);
        for speed in [0.0, 3.0, 7.0, 11.0] {
            for gap in [SKIN_DISTANCE, 0.05] {
                let label = format!("top {top_y}, ceiling {ceiling:?}, {speed} m/s, gap {gap}");
                let (mut component, start) = airborne_at_riser(&desc, 0.5, gap, speed);
                let track = vault_track(&mut component, &world, start);
                assert!(component.is_grounded(), "{label}: vaulted onto the top");
                // The camera presents the pose interpolated between ticks.
                for pair in track.windows(2) {
                    for alpha in [0.0, 0.25, 0.5, 0.75] {
                        let centre = pair[0].lerp(pair[1], alpha);
                        let eye = centre + Vec3::Y * eye_height;
                        let capsule = segment_to_block(centre, half_height, top_y, false);
                        assert!(
                            capsule >= RADIUS + 0.25 * SKIN_DISTANCE,
                            "{label}: capsule {capsule} m from the ledge at {centre}"
                        );
                        assert!(
                            distance_to_block(eye, top_y, false) >= RADIUS,
                            "{label}: eye at {eye} reaches into the ledge"
                        );
                        if let Some(ceiling_y) = ceiling {
                            let headroom = segment_to_block(centre, half_height, ceiling_y, true);
                            assert!(
                                headroom >= RADIUS + 0.25 * SKIN_DISTANCE,
                                "{label}: capsule {headroom} m from the ceiling at {centre}"
                            );
                            assert!(
                                distance_to_block(eye, ceiling_y, true) >= RADIUS,
                                "{label}: eye at {eye} reaches into the ceiling"
                            );
                        }
                    }
                }
            }
        }
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

// --- Committed motion, exit, blocked path and movers ------------------------

use crate::collision::moving::{CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource};
use glam::Quat;
use postretro_entities::Transform;
use postretro_foundation::GroundRef;

#[derive(Default)]
struct Poses(std::collections::HashMap<u32, MoverPose>);

impl Poses {
    fn place(&mut self, id: u32, position: Vec3, velocity: Vec3) {
        self.place_turned(id, position, Quat::IDENTITY, velocity);
    }

    fn place_turned(&mut self, id: u32, position: Vec3, rotation: Quat, velocity: Vec3) {
        self.0.insert(
            id,
            MoverPose {
                transform: Transform {
                    position,
                    rotation,
                    scale: Vec3::ONE,
                },
                linear_velocity: velocity,
                tick_delta: velocity * DT,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: DT,
            },
        );
    }
}

impl MoverPoseSource for Poses {
    fn pose(&self, id: u32) -> Option<MoverPose> {
        self.0.get(&id).copied()
    }
}

const MOVER: u32 = 9;

/// A mover-local block: a riser facing -X at local x = 0 from local y = 0 to
/// `height`, and its top, `depth` deep.
fn mover_block(height: f32, depth: f32) -> MoverCollider {
    let (z0, z1) = (-5.0, 5.0);
    MoverCollider::from_local_triangles(
        MOVER,
        &[
            Vec3::new(0.0, 0.0, z0),
            Vec3::new(0.0, height, z0),
            Vec3::new(0.0, height, z1),
            Vec3::new(0.0, 0.0, z1),
            Vec3::new(0.0, height, z0),
            Vec3::new(depth, height, z0),
            Vec3::new(depth, height, z1),
            Vec3::new(0.0, height, z1),
        ],
        &[[0, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]],
    )
    .unwrap()
}

/// A ceiling slab: mover-local quad at local y = 0 facing down, spanning `x`.
fn mover_slab(id: u32, x: (f32, f32)) -> MoverCollider {
    let (z0, z1) = (-5.0, 5.0);
    MoverCollider::from_local_triangles(
        id,
        &[
            Vec3::new(x.0, 0.0, z0),
            Vec3::new(x.1, 0.0, z0),
            Vec3::new(x.1, 0.0, z1),
            Vec3::new(x.0, 0.0, z1),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}

fn floor_only() -> CollisionWorld {
    ledge_world(0.0, None)
}

fn step_combined(
    component: &mut PlayerMovementComponent,
    collision: &CombinedCollisionWorld<'_>,
    input: &MovementInput,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let (position, events, _) = tick_with_immersion(component, input, collision, ENV, DT, position);
    (position, events)
}

/// Run a vault from an airborne approach until it exits, returning the named
/// addresses and positions it produced.
fn run_vault(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    mut position: Vec3,
) -> (Vec3, Vec<&'static str>, Vec<Vec3>) {
    let mut addresses = Vec::new();
    let mut path = Vec::new();
    for _ in 0..60 {
        let (next, events) = step(component, world, input, position);
        events.append_named_events(&mut addresses);
        position = next;
        path.push(position);
        if addresses.contains(&"vault_ended") {
            break;
        }
    }
    (position, addresses, path)
}

#[test]
fn exit_refreshes_ability_budgets_and_fires_one_start_and_one_end() {
    let mut desc = vault_descriptor(VaultEntry::default());
    desc.air.jumps = 1;
    let world = ledge_world(1.2, None);
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    component.air_jumps_remaining = 0;
    let (_, addresses, _) = run_vault(&mut component, &world, &forward(), position);
    let vault_addresses: Vec<_> = addresses
        .iter()
        .filter(|address| address.starts_with("vault_"))
        .collect();
    assert_eq!(vault_addresses, [&"vault_started", &"vault_ended"]);
    assert_eq!(
        component.air_jumps_remaining, 1,
        "landing on the top refreshes air jumps"
    );
}

#[test]
fn dash_jump_and_crouch_mid_vault_are_ignored() {
    let mut desc = with_crouch_slide_dash(vault_descriptor(VaultEntry::default()));
    desc.forgiveness = Some(ForgivenessParams {
        coyote_ms: 0.0,
        jump_buffer_ms: 150.0,
    });
    let world = ledge_world(1.2, None);
    let (mut quiet, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let (_, _, quiet_path) = run_vault(&mut quiet, &world, &forward(), position);

    let busy_input = MovementInput {
        dash_pressed: true,
        jump_pressed: true,
        crouch_intent: true,
        ..forward()
    };
    let (mut busy, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let mut busy_path = Vec::new();
    let mut position = step(&mut busy, &world, &forward(), position).0;
    busy_path.push(position);
    while is_vaulting(&busy) {
        position = step(&mut busy, &world, &busy_input, position).0;
        busy_path.push(position);
        if is_vaulting(&busy) {
            assert_eq!(
                busy.jump_buffer_timer_ms, 0.0,
                "a held jump never buffers mid-vault"
            );
            assert_eq!(
                busy.capsule.half_height, 0.8,
                "crouch never shrinks the capsule"
            );
        }
    }
    assert_eq!(
        busy_path, quiet_path,
        "held input leaves the vault path unchanged"
    );
}

#[test]
fn a_blocked_path_aborts_to_airborne_normal() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = floor_only();
    let block = mover_block(1.2, 6.0);
    let slab = mover_slab(MOVER + 1, (-2.0, 0.0));
    let movers = [block, slab];
    let mut poses = Poses::default();
    poses.place(MOVER, Vec3::new(RISER_X, 0.0, 0.0), Vec3::ZERO);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    position = step_combined(
        &mut component,
        &CombinedCollisionWorld::new(&world, &movers, &poses),
        &forward(),
        position,
    )
    .0;
    assert!(is_vaulting(&component));

    // A slab drops in just over the head: the rise stalls against it.
    let head = position.y + REACH;
    poses.place(MOVER + 1, Vec3::new(RISER_X, head + 0.05, 0.0), Vec3::ZERO);
    let mut aborted = false;
    for _ in 0..10 {
        let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
        position = step_combined(&mut component, &collision, &forward(), position).0;
        if !is_vaulting(&component) {
            aborted = true;
            break;
        }
    }
    assert!(aborted, "a stalled rise aborts");
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!(
        !component.is_grounded(),
        "the abort leaves the pawn airborne"
    );
    let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
    let next = step_combined(&mut component, &collision, &away(), position).0;
    assert!(next.x < position.x, "the pawn moves freely the next tick");
}

#[test]
fn a_vault_onto_a_mover_lands_on_it_still_or_rising_and_aborts_if_it_despawns() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = floor_only();
    let movers = [mover_block(1.2, 6.0)];
    for rise_speed in [0.0, 1.0] {
        let mut poses = Poses::default();
        let mut mover_y = 0.0;
        poses.place(
            MOVER,
            Vec3::new(RISER_X, mover_y, 0.0),
            Vec3::Y * rise_speed,
        );
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        let mut edges = Vec::new();
        for _ in 0..40 {
            mover_y += rise_speed * DT;
            poses.place(
                MOVER,
                Vec3::new(RISER_X, mover_y, 0.0),
                Vec3::Y * rise_speed,
            );
            let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
            let (next, events) = step_combined(&mut component, &collision, &forward(), position);
            edges.extend(events.state_edges);
            position = next;
            if edges
                .iter()
                .any(|edge| edge.from == MovementStateKind::Vault)
            {
                break;
            }
        }
        assert!(
            edges
                .first()
                .is_some_and(|edge| edge.to == MovementStateKind::Vault),
            "rise {rise_speed}: the pawn vaulted onto the mover"
        );
        assert_eq!(
            component.ground,
            GroundRef::Mover(MOVER),
            "rise {rise_speed}: the vault ends grounded on the mover"
        );
        assert!(
            (feet(position) - (mover_y + 1.2)).abs() < 2.0 * SKIN_DISTANCE,
            "rise {rise_speed}: feet {} on the mover top {}",
            feet(position),
            mover_y + 1.2
        );
    }

    let mut poses = Poses::default();
    poses.place(MOVER, Vec3::new(RISER_X, 0.0, 0.0), Vec3::ZERO);
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let position = step_combined(
        &mut component,
        &CombinedCollisionWorld::new(&world, &movers, &poses),
        &forward(),
        position,
    )
    .0;
    assert!(is_vaulting(&component));
    let MovementState::Vault { mover, .. } = component.movement_state else {
        unreachable!()
    };
    assert_eq!(mover, Some(MOVER), "the target rides the mover");
    let gone = Poses::default();
    step_combined(
        &mut component,
        &CombinedCollisionWorld::new(&world, &[], &gone),
        &forward(),
        position,
    );
    assert_eq!(
        component.movement_state,
        MovementState::Normal,
        "a despawned mover aborts"
    );
}

// --- Mantling out of fluid ---------------------------------------------------

use super::environment_tests::box_leaf;
use postretro_collision::{EnvironmentVolume, EnvironmentVolumes};
use postretro_foundation::SwimParams;

/// The pool's surface; its wall faces -X at `RISER_X` and rises to a deck.
const SURFACE_Y: f32 = 0.0;
const POOL_FLOOR_Y: f32 = -4.0;
const EYE: f32 = 0.5;

fn swim_params(fluid_jump_velocity: Option<f32>) -> SwimParams {
    SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.0,
        enter_depth: 0.5,
        // Low, so a swimmer treading with its eye out stays in swim.
        exit_depth: 0.2,
        fluid_jump_velocity,
    }
}

fn swim_vault_descriptor(from: VaultEntry, fluid_jump: Option<f32>) -> PlayerMovementDescriptor {
    let mut desc = vault_descriptor(from);
    desc.swim = Some(swim_params(fluid_jump));
    desc
}

/// The pool floor, its wall rising to a deck at `deck_y`, and the deck.
fn pool_world(deck_y: f32) -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    let (z0, z1) = (-50.0, 50.0);
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(-50.0, POOL_FLOOR_Y, z0),
            Vec3::new(RISER_X, POOL_FLOOR_Y, z0),
            Vec3::new(RISER_X, POOL_FLOOR_Y, z1),
            Vec3::new(-50.0, POOL_FLOOR_Y, z1),
        ],
        Vec3::Y,
    );
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(RISER_X, POOL_FLOOR_Y, z0),
            Vec3::new(RISER_X, deck_y, z0),
            Vec3::new(RISER_X, deck_y, z1),
            Vec3::new(RISER_X, POOL_FLOOR_Y, z1),
        ],
        Vec3::NEG_X,
    );
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(RISER_X, deck_y, z0),
            Vec3::new(60.0, deck_y, z0),
            Vec3::new(60.0, deck_y, z1),
            Vec3::new(RISER_X, deck_y, z1),
        ],
        Vec3::Y,
    );
    CollisionWorld::from_triangles_for_test(points, tris)
}

fn pool_water() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(0),
        brushes: vec![box_leaf(
            Vec3::new(-50.0, POOL_FLOOR_Y, -50.0),
            Vec3::new(RISER_X, SURFACE_Y, 50.0),
        )],
    }])
}

fn step_in(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    volumes: &EnvironmentVolumes,
    input: &MovementInput,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let view = EnvironmentView::new(-20.0, volumes, &[2.0]);
    let (position, events, _) = tick_with_immersion(component, input, world, view, DT, position);
    (position, events)
}

/// A swimmer at the wall with its eye `eye_out` above the surface (negative:
/// under it), capsule surface `gap` short of the wall.
fn swimmer_at_wall(
    desc: &PlayerMovementDescriptor,
    eye_out: f32,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    component.movement_state = MovementState::Swim { fluid: 0 };
    component.coyote_timer_ms = 1000.0;
    let center_y = SURFACE_Y + eye_out - EYE;
    (component, Vec3::new(RISER_X - RADIUS - 0.05, center_y, 0.0))
}

#[test]
fn a_swimmer_mantles_out_and_swim_never_recaptures_the_rise() {
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let world = pool_world(SURFACE_Y + 0.5);
    let water = pool_water();
    let (mut component, mut position) = swimmer_at_wall(&desc, 0.05);
    let mut edges = Vec::new();
    let mut entered = false;
    // The latch holds until a tick starts with the capsule risen out of the
    // swim band (immersion under `exitDepth`), the rule that clears it.
    let exit_depth = desc.swim.as_ref().unwrap().exit_depth;
    let out_of_band_feet = SURFACE_Y - exit_depth * 2.0 * REACH;
    for _ in 0..60 {
        let start = position;
        let (next, events) = step_in(&mut component, &world, &water, &forward(), position);
        edges.extend(events.state_edges.iter().map(|edge| (edge.from, edge.to)));
        position = next;
        if is_vaulting(&component) {
            entered = true;
            assert!(
                component.swim_reentry_latched || feet(start) > out_of_band_feet,
                "the latch cleared with the feet {} still in the swim band",
                feet(start)
            );
        }
        if entered && !is_vaulting(&component) {
            break;
        }
    }
    assert_eq!(
        edges,
        [
            (MovementStateKind::Swim, MovementStateKind::Vault),
            (MovementStateKind::Vault, MovementStateKind::Normal),
        ],
        "one mantle out, no swim edge during the rise"
    );
    assert!(component.is_grounded(), "the pawn stands on the deck");
    assert!(position.x > RISER_X);
    assert!(
        !component.swim_reentry_latched,
        "the latch clears once the pawn is dry"
    );
}

#[test]
fn fluid_reach_is_measured_from_the_surface_not_the_feet() {
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let water = pool_water();
    // Shallow and deep: the feet sit 0.65 m and 1.65 m below the surface.
    for eye_out in [0.05, 0.05 + 1.0] {
        let shallow = eye_out < 0.5;
        let feet_depth = if shallow { 0.65 } else { 1.65 };
        let (_, start) = swimmer_at_wall(&desc, 0.05);
        let center = Vec3::new(start.x, SURFACE_Y - feet_depth + REACH, 0.0);
        for (edge_above_surface, vaults) in [(0.85, true), (0.95, false)] {
            let world = pool_world(SURFACE_Y + edge_above_surface);
            let (mut component, _) = swimmer_at_wall(&desc, 0.05);
            step_in(&mut component, &world, &water, &forward(), center);
            assert_eq!(
                is_vaulting(&component),
                vaults,
                "feet {feet_depth} m under, edge {edge_above_surface} m over the surface"
            );
        }
    }
}

#[test]
fn an_edge_out_of_land_reach_is_in_fluid_reach_from_deep_feet() {
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let water = pool_water();
    // Feet 1.65 m under the surface: an edge 1.8 m over the feet is 0.15 m over
    // the surface, within swimMaxHeight 0.9.
    let feet_y = SURFACE_Y - 1.65;
    let world = pool_world(feet_y + 1.8);
    let (mut component, start) = swimmer_at_wall(&desc, 0.05);
    let center = Vec3::new(start.x, feet_y + REACH, 0.0);
    step_in(&mut component, &world, &water, &forward(), center);
    assert!(
        is_vaulting(&component),
        "the mantle reaches it from the water"
    );

    // The same 1.8 m over the feet on land is past maxHeight 1.6.
    let land = ledge_world(1.8, None);
    let (mut component, position) = airborne_at_riser(&desc, 0.0, 0.05, 0.0);
    step(&mut component, &land, &forward(), position);
    assert!(
        !is_vaulting(&component),
        "a land ledge 1.8 m over the feet refuses"
    );
}

#[test]
fn out_of_reach_or_without_swim_entry_jump_is_the_fluid_jump() {
    // The dev fluid jump: strong enough to clear the water from a treading
    // depth, so the seam lets it fire.
    const FLUID_JUMP: f32 = 9.35;
    let water = pool_water();
    let jump = jump_forward();
    for (label, desc, deck) in [
        (
            "edge past swimMaxHeight",
            swim_vault_descriptor(VaultEntry::default(), Some(FLUID_JUMP)),
            SURFACE_Y + 1.0,
        ),
        (
            "no swim context",
            swim_vault_descriptor(
                VaultEntry {
                    swim: None,
                    ..VaultEntry::default()
                },
                Some(FLUID_JUMP),
            ),
            SURFACE_Y + 0.5,
        ),
    ] {
        let world = pool_world(deck);
        let (mut component, position) = swimmer_at_wall(&desc, 0.05);
        let (_, events) = step_in(&mut component, &world, &water, &jump, position);
        assert!(!is_vaulting(&component), "{label}: no mantle");
        assert!(events.jumped, "{label}: the fluid jump fires");
        assert_eq!(component.velocity.y, FLUID_JUMP, "{label}");
    }

    // Eye under the surface: no mantle at a reachable edge.
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let world = pool_world(SURFACE_Y + 0.5);
    let (mut component, position) = swimmer_at_wall(&desc, -0.05);
    step_in(&mut component, &world, &water, &forward(), position);
    assert!(
        !is_vaulting(&component),
        "an eye under the fluid never mantles"
    );
}

#[test]
fn swim_jump_treads_on_a_push_and_mantles_on_a_jump() {
    let desc = swim_vault_descriptor(
        VaultEntry {
            swim: Some(VaultTrigger::Jump),
            ..VaultEntry::default()
        },
        Some(6.5),
    );
    let world = pool_world(SURFACE_Y + 0.5);
    let water = pool_water();
    let (mut component, position) = swimmer_at_wall(&desc, 0.05);
    step_in(&mut component, &world, &water, &forward(), position);
    assert_eq!(
        component.movement_state.kind(),
        MovementStateKind::Swim,
        "a push treads"
    );
    let (mut component, position) = swimmer_at_wall(&desc, 0.05);
    let (_, events) = step_in(&mut component, &world, &water, &jump_forward(), position);
    assert!(
        is_vaulting(&component),
        "a jump at a reachable edge mantles"
    );
    assert!(!events.jumped, "the mantle wins over the fluid jump");
}

#[test]
fn without_a_fluid_jump_jump_treads_and_never_leaves_the_water() {
    let desc = swim_vault_descriptor(VaultEntry::default(), None);
    let world = pool_world(SURFACE_Y + 0.5);
    let water = pool_water();
    // Open water, far from the wall.
    let (mut component, mut position) = swimmer_at_wall(&desc, 0.05);
    position.x = -20.0;
    let jump = MovementInput {
        jump_pressed: true,
        ..idle_input()
    };
    for _ in 0..120 {
        let (next, events) = step_in(&mut component, &world, &water, &jump, position);
        position = next;
        assert!(!events.jumped, "no fluid jump fires");
        assert_eq!(component.movement_state.kind(), MovementStateKind::Swim);
    }
    assert!(feet(position) < SURFACE_Y, "the pawn is still in the water");
}

#[test]
fn swim_entry_wins_over_a_land_vault_on_the_same_tick() {
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let water = pool_water();
    // Airborne in `Normal`, already deep enough to swim, at a wall whose top is
    // within land reach.
    let feet_y = SURFACE_Y - 1.4;
    let world = pool_world(feet_y + 1.2);
    let (mut component, position) = airborne_at_riser(&desc, feet_y, 0.05, 0.0);
    let (_, events) = step_in(&mut component, &world, &water, &forward(), position);
    assert_eq!(
        events.state_edges.first().map(|edge| (edge.from, edge.to)),
        Some((MovementStateKind::Normal, MovementStateKind::Swim)),
        "the swim entry takes the tick"
    );
    assert!(!is_vaulting(&component));
}

#[test]
fn a_ground_only_from_forbids_swim_entry() {
    let desc = swim_vault_descriptor(
        VaultEntry {
            air: None,
            ground: Some(VaultTrigger::Jump),
            swim: None,
        },
        None,
    );
    let world = pool_world(SURFACE_Y + 0.5);
    let water = pool_water();
    for input in [forward(), jump_forward()] {
        let (mut component, position) = swimmer_at_wall(&desc, 0.05);
        take_probe_casts();
        step_in(&mut component, &world, &water, &input, position);
        assert!(
            !is_vaulting(&component),
            "swim entry is forbidden (jump {})",
            input.jump_pressed
        );
        assert_eq!(take_probe_casts(), 0);
    }
}

// --- Early ends, protected layers, movers and short paths -------------------

/// The pool floor alone; a mover supplies the wall and the deck.
fn pool_floor() -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(-50.0, POOL_FLOOR_Y, -50.0),
            Vec3::new(60.0, POOL_FLOOR_Y, -50.0),
            Vec3::new(60.0, POOL_FLOOR_Y, 50.0),
            Vec3::new(-50.0, POOL_FLOOR_Y, 50.0),
        ],
        Vec3::Y,
    );
    CollisionWorld::from_triangles_for_test(points, tris)
}

// Regression: a swim-entered vault that ended early kept the re-entry latch,
// so a pawn still in deep water could never swim again and sank.
#[test]
fn an_early_end_to_a_swim_mantle_lets_the_pawn_swim_again() {
    let desc = swim_vault_descriptor(VaultEntry::default(), Some(6.5));
    let floor = pool_floor();
    let water = pool_water();
    let view = EnvironmentView::new(-20.0, &water, &[2.0]);
    let deck = [mover_block(SURFACE_Y + 0.5 - POOL_FLOOR_Y, 6.0)];
    let mut poses = Poses::default();
    poses.place(MOVER, Vec3::new(RISER_X, POOL_FLOOR_Y, 0.0), Vec3::ZERO);
    let gone = Poses::default();
    let present = CombinedCollisionWorld::new(&floor, &deck, &poses);
    let despawned = CombinedCollisionWorld::new(&floor, &[], &gone);

    for case in ["knockback", "mover despawn", "death", "tuning drop"] {
        let (mut component, position) = swimmer_at_wall(&desc, 0.05);
        let (mut position, _, _) =
            tick_with_immersion(&mut component, &forward(), &present, view, DT, position);
        assert!(is_vaulting(&component), "{case}: the mantle starts");
        assert!(component.swim_reentry_latched, "{case}: the mantle latches");

        let mut collision = present;
        match case {
            "knockback" => assert!(component.add_knockback(Vec3::new(-30.0, 0.0, 0.0))),
            "mover despawn" => collision = despawned,
            // What the sim's death drop calls.
            "death" => component.break_off_vault(),
            _ => {
                component.vault = None;
                component.leave_vault_without_vault_block();
            }
        }
        let mut swims = false;
        for _ in 0..4 {
            position = tick_with_immersion(
                &mut component,
                &idle_input(),
                &collision,
                view,
                DT,
                position,
            )
            .0;
            if component.movement_state.kind() == MovementStateKind::Swim {
                swims = true;
                break;
            }
        }
        assert!(
            swims,
            "{case}: the pawn, still deep, swims again; got {:?}, latched {}",
            component.movement_state, component.swim_reentry_latched
        );
    }
}

// Regression: leaving a vault by death or a tuning drop kept the path velocity,
// throwing the pawn on along the rise.
#[test]
fn breaking_off_a_vault_drops_its_rise_velocity_and_keeps_knockback() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    for case in ["death", "tuning drop"] {
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        for _ in 0..5 {
            position = step(&mut component, &world, &forward(), position).0;
        }
        assert!(is_vaulting(&component), "{case}");
        assert!(component.velocity.y > 1.0, "{case}: the pawn is mid-rise");
        let knockback = Vec3::new(-2.0, 0.0, 0.0);
        component.knockback_velocity = knockback;
        component.velocity += knockback;

        match case {
            "death" => component.break_off_vault(),
            _ => {
                component.vault = None;
                component.leave_vault_without_vault_block();
            }
        }
        assert_eq!(component.movement_state, MovementState::Normal, "{case}");
        assert_eq!(
            component.velocity, knockback,
            "{case}: only the knockback stays"
        );
        let next = step(&mut component, &world, &idle_input(), position).0;
        assert!(
            next.y < position.y,
            "{case}: the pawn falls instead of rising on"
        );
    }
}

#[test]
fn a_barred_pawn_pressing_into_a_ledge_never_vaults_or_probes() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    component.vault_barred = true;
    for _ in 0..10 {
        take_probe_casts();
        position = step(&mut component, &world, &forward(), position).0;
        assert!(
            !is_vaulting(&component),
            "a barred (dead) pawn never vaults"
        );
        assert_eq!(take_probe_casts(), 0);
    }
}

// Regression: knockback carried into a vault rode on top of the path, aborted it
// on its second tick and then launched the pawn.
#[test]
fn carried_in_knockback_folds_into_the_vault_and_the_climb_completes() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let knockback = Vec3::new(12.0, 3.0, 0.0);
    component.knockback_velocity = knockback;
    component.velocity += knockback;

    let (position, addresses, _) = run_vault(&mut component, &world, &forward(), position);
    assert!(addresses.contains(&"vault_ended"));
    assert_eq!(component.knockback_velocity, Vec3::ZERO, "entry folded it");
    assert!(component.is_grounded(), "the climb ends grounded");
    assert!(position.x > RISER_X + RADIUS, "over the top");
    assert!(
        (feet(position) - 1.2).abs() < 2.0 * SKIN_DISTANCE,
        "feet {} rest on the top",
        feet(position)
    );
    let horizontal = Vec2::new(component.velocity.x, component.velocity.z).length();
    assert!(
        (horizontal - 19.0).abs() < 1.0e-3,
        "exit restores the speed the pawn really moved at, got {horizontal}"
    );
}

#[test]
fn knockback_landing_mid_vault_aborts_and_the_pawn_keeps_it() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    for _ in 0..3 {
        position = step(&mut component, &world, &forward(), position).0;
    }
    assert!(is_vaulting(&component));
    let shove = Vec3::new(-20.0, 0.0, 0.0);
    assert!(component.add_knockback(shove));

    let mut ended = false;
    for _ in 0..3 {
        let (next, events) = step(&mut component, &world, &forward(), position);
        position = next;
        if events
            .state_edges
            .iter()
            .any(|edge| edge.from == MovementStateKind::Vault)
        {
            ended = true;
            break;
        }
    }
    assert!(ended, "the shove moves the pawn off the path and aborts");
    assert_eq!(component.movement_state, MovementState::Normal);
    assert!(!component.is_grounded());
    // Air drag is zero by default, so the whole shove survives.
    assert!((component.knockback_velocity - shove).length() < 1.0e-4);
    assert!(
        (component.velocity - shove).length() < 1.0e-4,
        "the abort drops the path velocity and keeps the shove, got {}",
        component.velocity
    );
}

// Regression: push drift accrued on top of the vault path every tick and
// aborted it.
#[test]
fn a_vault_inside_a_push_volume_completes() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = ledge_world(1.2, None);
    let wind = EnvironmentVolumes::build(vec![EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: Some(Vec3::new(0.0, 0.0, 30.0)),
        force: 1.0,
        fluid: None,
        brushes: vec![box_leaf(
            Vec3::new(-50.0, -10.0, -50.0),
            Vec3::new(60.0, 20.0, 50.0),
        )],
    }]);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    let mut edges = Vec::new();
    for _ in 0..40 {
        let (next, events) = step_in(&mut component, &world, &wind, &forward(), position);
        edges.extend(events.state_edges.iter().map(|edge| (edge.from, edge.to)));
        position = next;
        if edges
            .iter()
            .any(|(from, _)| *from == MovementStateKind::Vault)
        {
            break;
        }
    }
    assert_eq!(
        edges,
        [
            (MovementStateKind::Normal, MovementStateKind::Vault),
            (MovementStateKind::Vault, MovementStateKind::Normal),
        ]
    );
    assert!(component.is_grounded(), "the vault ends grounded");
    assert!(position.x > RISER_X + RADIUS, "over the top");
    assert!(
        (feet(position) - 1.2).abs() < 2.0 * SKIN_DISTANCE,
        "feet {} rest on the top",
        feet(position)
    );
}

// Regression: an airborne vaulter trailed a mover's target by one tick of its
// motion, so above 6 m/s every vault onto the mover aborted on its second tick.
#[test]
fn a_vault_onto_a_fast_mover_lands_on_it() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = floor_only();
    let movers = [mover_block(1.2, 6.0)];
    // Rising, sliding sideways, and moving away from the vaulter.
    for velocity in [Vec3::Y * 8.0, Vec3::Z * 8.0, Vec3::X * 8.0] {
        let mut origin = Vec3::new(RISER_X, 0.0, 0.0) - velocity * DT;
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        let mut edges = Vec::new();
        for _ in 0..40 {
            origin += velocity * DT;
            let mut poses = Poses::default();
            poses.place(MOVER, origin, velocity);
            let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
            let (next, events) = step_combined(&mut component, &collision, &forward(), position);
            edges.extend(events.state_edges.iter().map(|edge| (edge.from, edge.to)));
            position = next;
            if edges
                .iter()
                .any(|(from, _)| *from == MovementStateKind::Vault)
            {
                break;
            }
        }
        assert_eq!(
            edges,
            [
                (MovementStateKind::Normal, MovementStateKind::Vault),
                (MovementStateKind::Vault, MovementStateKind::Normal),
            ],
            "mover at {velocity}"
        );
        assert_eq!(
            component.ground,
            GroundRef::Mover(MOVER),
            "mover at {velocity}: the vault ends grounded on it"
        );
        assert!(
            position.x - origin.x > RADIUS,
            "mover at {velocity}: the pawn is over its top"
        );
        assert!(
            (feet(position) - (origin.y + 1.2)).abs() < 2.0 * SKIN_DISTANCE,
            "mover at {velocity}: feet {} on its top {}",
            feet(position),
            origin.y + 1.2
        );
    }
}

#[test]
fn a_ledge_on_a_tilted_mover_is_refused_and_a_yawed_one_vaults() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = floor_only();
    let movers = [mover_block(1.2, 6.0)];
    for (label, rotation, vaults) in [
        (
            "rolled 10°",
            Quat::from_rotation_x(10.0_f32.to_radians()),
            false,
        ),
        (
            "yawed 10°",
            Quat::from_rotation_y(10.0_f32.to_radians()),
            true,
        ),
    ] {
        let mut poses = Poses::default();
        poses.place_turned(MOVER, Vec3::new(RISER_X, 0.0, 0.0), rotation, Vec3::ZERO);
        let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        position = step_combined(&mut component, &collision, &forward(), position).0;
        assert_eq!(is_vaulting(&component), vaults, "{label}");
        if !vaults {
            continue;
        }
        let mut ended = false;
        for _ in 0..40 {
            let (next, events) = step_combined(&mut component, &collision, &forward(), position);
            position = next;
            if !events.state_edges.is_empty() {
                ended = true;
                break;
            }
        }
        assert!(ended, "{label}: the vault ends");
        assert_eq!(
            component.ground,
            GroundRef::Mover(MOVER),
            "{label}: the vault ends grounded on the mover"
        );
    }
}

// Regression: a progress step crossing the rise share with rise left aimed past
// the lip, cut the corner into the riser and aborted.
#[test]
fn a_short_duration_vault_still_ends_on_the_top() {
    let world = ledge_world(1.2, None);
    for duration_ms in [10.0, 50.0] {
        let mut desc = vault_descriptor(VaultEntry::default());
        desc.vault.as_mut().unwrap().duration_ms = duration_ms;
        let (mut component, position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        let (position, addresses, _) = run_vault(&mut component, &world, &forward(), position);
        assert!(addresses.contains(&"vault_ended"), "{duration_ms} ms");
        assert!(component.is_grounded(), "{duration_ms} ms: grounded");
        assert!(
            position.x > RISER_X + RADIUS,
            "{duration_ms} ms: over the top"
        );
        assert!(
            (feet(position) - 1.2).abs() < 2.0 * SKIN_DISTANCE,
            "{duration_ms} ms: feet {} rest on the top",
            feet(position)
        );
    }
}

// Regression: an auto vault left a jump buffered before it armed, so the jump
// fired on the exit landing.
#[test]
fn an_auto_vault_clears_a_buffered_jump() {
    let mut desc = vault_descriptor(VaultEntry::default());
    desc.forgiveness = Some(ForgivenessParams {
        coyote_ms: 0.0,
        jump_buffer_ms: 1000.0,
    });
    let world = ledge_world(1.2, None);
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

    position.x = RISER_X - RADIUS - 0.05;
    position = step(&mut component, &world, &forward(), position).0;
    assert!(is_vaulting(&component), "the auto vault fires");
    assert_eq!(
        component.jump_buffer_timer_ms, 0.0,
        "entry clears the buffer"
    );

    let (mut position, addresses, _) = run_vault(&mut component, &world, &idle_input(), position);
    assert!(addresses.contains(&"vault_ended"));
    assert!(!addresses.contains(&"jumped"));
    for _ in 0..5 {
        let (next, events) = step(&mut component, &world, &idle_input(), position);
        position = next;
        assert!(!events.jumped, "no buffered jump fires after the vault");
    }
    assert!(component.is_grounded(), "the pawn stays on the top");
}

// Descriptor validation restates the step-up limit's margin, since foundation
// cannot depend on physics; the two must not drift apart. They can differ by an
// f32 ulp (`step + 0.05 - 0.04` against `step + 0.01`), which only fails safe: a
// `maxHeight` inside that ulp validates and never vaults.
#[test]
fn validation_restates_the_step_up_limit_margin() {
    use crate::collision::STEP_UP_LIFT_MARGIN;
    use postretro_foundation::STEP_UP_LIMIT_MARGIN;
    assert!((STEP_UP_LIFT_MARGIN - 2.0 * SKIN_DISTANCE - STEP_UP_LIMIT_MARGIN).abs() < 1.0e-6);
}

#[test]
fn max_height_validation_refuses_tops_the_step_up_limit_already_covers() {
    let at_limit = VaultParams {
        max_height: STEP_HEIGHT + postretro_foundation::STEP_UP_LIMIT_MARGIN,
        ..vault_params(VaultEntry::default())
    };
    assert!(
        at_limit.validated(STEP_HEIGHT).is_err(),
        "a maxHeight at validation's step-up floor is refused"
    );
    let past_limit = VaultParams {
        max_height: step_up_limit(STEP_HEIGHT) + 0.001,
        ..vault_params(VaultEntry::default())
    };
    assert!(past_limit.validated(STEP_HEIGHT).is_ok());
}

// Regression: a vault aborted while riding its target's mover lost the mover's
// velocity, because release keyed off the airborne tick-start ground.
#[test]
fn a_vault_aborted_on_a_moving_mover_keeps_the_mover_velocity() {
    let desc = vault_descriptor(VaultEntry::default());
    let world = floor_only();
    let movers = [mover_block(1.2, 6.0)];
    let shove = Vec3::new(-20.0, 0.0, 0.0);
    // Rising and sliding sideways.
    for velocity in [Vec3::Y * 8.0, Vec3::Z * 8.0] {
        let mut origin = Vec3::new(RISER_X, 0.0, 0.0) - velocity * DT;
        let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
        let mut aborted = false;
        for tick in 0..10 {
            origin += velocity * DT;
            let mut poses = Poses::default();
            poses.place(MOVER, origin, velocity);
            let collision = CombinedCollisionWorld::new(&world, &movers, &poses);
            if tick == 3 {
                assert!(is_vaulting(&component), "mover at {velocity}");
                assert!(component.add_knockback(shove));
            }
            let (next, events) = step_combined(&mut component, &collision, &forward(), position);
            position = next;
            if events
                .state_edges
                .iter()
                .any(|edge| edge.from == MovementStateKind::Vault)
            {
                aborted = true;
                break;
            }
        }
        assert!(aborted, "mover at {velocity}: the shove aborts the vault");
        assert!(!component.is_grounded(), "mover at {velocity}");
        assert!(
            (component.velocity - (shove + velocity)).length() < 1.0e-3,
            "mover at {velocity}: the pawn leaves with the shove and the mover's velocity, got {}",
            component.velocity
        );
    }
}

// Regression: the exit tick read dispatch's `Normal` placeholder, so a jump
// pressed on it armed the buffer and fired after the vault.
#[test]
fn a_jump_pressed_on_the_exit_tick_is_never_buffered() {
    let mut desc = vault_descriptor(VaultEntry::default());
    desc.forgiveness = Some(ForgivenessParams {
        coyote_ms: 0.0,
        jump_buffer_ms: 150.0,
    });
    let world = ledge_world(1.2, None);
    let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, 7.0);
    position = step(&mut component, &world, &forward(), position).0;
    let mut exited = false;
    for _ in 0..40 {
        let MovementState::Vault { elapsed_ms, .. } = component.movement_state else {
            break;
        };
        let exit_tick = elapsed_ms >= DURATION_MS;
        let input = MovementInput {
            jump_pressed: exit_tick,
            ..idle_input()
        };
        position = step(&mut component, &world, &input, position).0;
        if exit_tick {
            exited = !is_vaulting(&component);
            break;
        }
    }
    assert!(exited, "the jump went in on the exit tick");
    assert_eq!(
        component.jump_buffer_timer_ms, 0.0,
        "the exit tick never buffers"
    );
    for _ in 0..5 {
        let (next, events) = step(&mut component, &world, &idle_input(), position);
        position = next;
        assert!(!events.jumped, "no jump fires after the vault");
    }
}

/// Durations and tick rates where a step count from one division disagreed
/// with a clock accumulated step by step: the accumulated clock read just
/// under the duration on the path's last tick.
const CLOCK_EDGE_CASES: [(f32, &[f32]); 5] = [
    (30.0, &[1000.0]),
    (60.0, &[400.0, 500.0, 320.0, 420.0]),
    (120.0, &[200.0, 250.0, 1000.0]),
    (144.0, &[250.0]),
    (240.0, &[100.0, 400.0, 500.0, 600.0]),
];

#[test]
fn the_vault_clock_reads_whole_ticks_and_reaches_the_duration_on_the_path_ticks() {
    for (hz, durations) in CLOCK_EDGE_CASES {
        let step_ms = (1.0 / hz) * 1000.0;
        for &duration_ms in durations {
            let ticks = vault_path_ticks(duration_ms, step_ms);
            assert!(
                vault_clock(ticks, step_ms) >= duration_ms
                    && vault_clock(ticks - 1, step_ms) < duration_ms,
                "{duration_ms} ms at {hz} Hz: {ticks} path ticks"
            );
            assert!(
                ticks as f32 >= duration_ms / step_ms - 1.0e-3
                    && (ticks as f32) < duration_ms / step_ms + 1.0,
                "{duration_ms} ms at {hz} Hz: {ticks} path ticks is the duration rounded up"
            );
        }
    }
}

// Regression: at 400 ms and 60 Hz the pawn parked a tick on the target, gate shut, before the exit.
#[test]
fn the_path_the_exit_and_the_fire_gate_end_on_the_same_tick_at_every_tick_rate() {
    let world = ledge_world(1.2, None);
    let speed = 7.0;
    for (hz, durations) in CLOCK_EDGE_CASES {
        let dt = 1.0 / hz;
        let step_ms = dt * 1000.0;
        for &duration_ms in durations {
            let label = format!("{duration_ms} ms at {hz} Hz");
            let mut desc = vault_descriptor(VaultEntry::default());
            desc.vault.as_mut().unwrap().duration_ms = duration_ms;
            let path_ticks = vault_path_ticks(duration_ms, step_ms);
            let (mut component, mut position) = airborne_at_riser(&desc, 0.5, 0.05, speed);
            let mut moves = Vec::new();
            let mut exit = None;
            for tick in 1..=path_ticks + 8 {
                // The fire gate reads the clock as the tick starts.
                let clock = match component.movement_state {
                    MovementState::Vault { elapsed_ms, .. } => Some(elapsed_ms),
                    _ => None,
                };
                let (next, _, _) =
                    tick_with_immersion(&mut component, &forward(), &world, ENV, dt, position);
                moves.push(next - position);
                position = next;
                match clock {
                    None => {
                        assert!(
                            tick == 1 && is_vaulting(&component),
                            "{label}: the vault starts on the first tick"
                        );
                    }
                    Some(clock) if is_vaulting(&component) => {
                        assert!(
                            clock < duration_ms,
                            "{label}: tick {tick} is still on the path, so the gate stays shut"
                        );
                    }
                    Some(clock) => {
                        exit = Some((tick, clock));
                        break;
                    }
                }
            }
            let (exit_tick, exit_clock) = exit.unwrap_or_else(|| panic!("{label}: the vault ends"));
            assert_eq!(
                exit_tick,
                path_ticks + 1,
                "{label}: the exit follows the last of {path_ticks} path ticks"
            );
            assert_eq!(exit_clock, vault_clock(path_ticks, step_ms), "{label}");
            assert!(
                exit_clock >= duration_ms,
                "{label}: the fire gate opens on the exit tick"
            );
            assert_eq!(component.movement_state, MovementState::Normal, "{label}");
            // No tick parked on the target: the last path tick still moves at
            // the speed the exit restores.
            let last_path = moves[moves.len() - 2];
            let last_speed = Vec2::new(last_path.x, last_path.z).length() / dt;
            assert!(
                (last_speed - speed).abs() < 0.05 * speed,
                "{label}: the last path tick moves {last_speed} m/s, not a stall"
            );
            let exit_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
            assert!(
                (exit_speed - speed).abs() < 1.0e-3,
                "{label}: exit speed {exit_speed}"
            );
        }
    }
}
