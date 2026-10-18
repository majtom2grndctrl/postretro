// Tick overlap invariant: own movement never ends a tick inside static geometry.
// See: context/lib/movement.md §1 · context/lib/entity_model.md §7

use glam::{Quat, Vec2, Vec3};

use postretro_collision::EnvironmentView;
use postretro_foundation::{MovementStateKind, PlayerMovementComponent, PlayerMovementDescriptor};

use super::environment_tests::{descriptor, idle_input};
use super::vault_tests::{ground_auto, vault_descriptor};
use super::{MovementInput, tick_with_immersion};
use crate::collision::{CollisionCapsule, CollisionWorld, SKIN_DISTANCE};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
pub(super) use crate::collision::OVERLAP_TOLERANCE;
const SPAN: (f32, f32) = (-50.0, 50.0);

/// How deep the pawn's live capsule at `position` overlaps static geometry.
pub(super) fn overlap_depth(
    world: &CollisionWorld,
    position: Vec3,
    capsule: &postretro_foundation::CapsuleParams,
) -> f32 {
    world.overlap_depth_for_test(
        position,
        CollisionCapsule::new(capsule.radius, capsule.half_height),
    )
}

/// Push a quad whose winding faces `normal`.
pub(super) fn quad(
    points: &mut Vec<Vec3>,
    tris: &mut Vec<[u32; 3]>,
    corners: [Vec3; 4],
    normal: Vec3,
) {
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

/// Static test geometry from quads spanning `SPAN` in Z unless given corners.
#[derive(Default)]
pub(super) struct Geometry {
    points: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

impl Geometry {
    /// A floor-like quad at height `y` over x in `x`, facing up.
    pub(super) fn floor(self, x: (f32, f32), y: f32) -> Self {
        self.slope(x, (y, y))
    }

    /// A walkable quad rising from `y.0` at `x.0` to `y.1` at `x.1`, facing up.
    pub(super) fn slope(mut self, x: (f32, f32), y: (f32, f32)) -> Self {
        let (z0, z1) = SPAN;
        let up = Vec3::new(y.0 - y.1, x.1 - x.0, 0.0).normalize();
        quad(
            &mut self.points,
            &mut self.tris,
            [
                Vec3::new(x.0, y.0, z0),
                Vec3::new(x.1, y.1, z0),
                Vec3::new(x.1, y.1, z1),
                Vec3::new(x.0, y.0, z1),
            ],
            up,
        );
        self
    }

    /// A vertical wall in the plane x = `x` over heights `y`, facing `facing`
    /// (`Vec3::X` or `Vec3::NEG_X`).
    pub(super) fn wall_x(mut self, x: f32, y: (f32, f32), facing: Vec3) -> Self {
        let (z0, z1) = SPAN;
        quad(
            &mut self.points,
            &mut self.tris,
            [
                Vec3::new(x, y.0, z0),
                Vec3::new(x, y.1, z0),
                Vec3::new(x, y.1, z1),
                Vec3::new(x, y.0, z1),
            ],
            facing,
        );
        self
    }

    /// A vertical wall in the plane z = `z` over x in `x` and heights `y`,
    /// facing `facing` (`Vec3::Z` or `Vec3::NEG_Z`).
    pub(super) fn wall_z(mut self, z: f32, x: (f32, f32), y: (f32, f32), facing: Vec3) -> Self {
        quad(
            &mut self.points,
            &mut self.tris,
            [
                Vec3::new(x.0, y.0, z),
                Vec3::new(x.1, y.0, z),
                Vec3::new(x.1, y.1, z),
                Vec3::new(x.0, y.1, z),
            ],
            facing,
        );
        self
    }

    /// A ceiling at height `y` over x in `x`, facing down.
    pub(super) fn ceiling(mut self, x: (f32, f32), y: f32) -> Self {
        let (z0, z1) = SPAN;
        quad(
            &mut self.points,
            &mut self.tris,
            [
                Vec3::new(x.0, y, z0),
                Vec3::new(x.1, y, z0),
                Vec3::new(x.1, y, z1),
                Vec3::new(x.0, y, z1),
            ],
            Vec3::NEG_Y,
        );
        self
    }

    pub(super) fn build(self) -> CollisionWorld {
        CollisionWorld::from_triangles_for_test(self.points, self.tris)
    }
}

/// Centre height of a pawn resting on a floor at `floor_y`.
pub(super) fn rest_y(desc: &PlayerMovementDescriptor, floor_y: f32) -> f32 {
    floor_y + desc.capsule.half_height + desc.capsule.radius + SKIN_DISTANCE
}

pub(super) fn walk(wish_dir: Vec2) -> MovementInput {
    MovementInput {
        wish_dir,
        ..idle_input()
    }
}

/// `x` is right; at yaw 0 right is world +X.
pub(super) fn toward_x() -> MovementInput {
    walk(Vec2::new(1.0, 0.0))
}

/// Advance one tick and assert it ends clear of static geometry.
pub(super) fn step_clear(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    position: Vec3,
    what: &str,
) -> Vec3 {
    let next = tick_with_immersion(component, input, world, ENV, DT, position).0;
    let depth = overlap_depth(world, next, &component.capsule);
    assert!(
        depth <= OVERLAP_TOLERANCE,
        "{what}: tick ended {depth} m inside static geometry at {next:?}"
    );
    next
}

pub(super) fn run_clear(
    component: &mut PlayerMovementComponent,
    world: &CollisionWorld,
    input: &MovementInput,
    mut position: Vec3,
    ticks: usize,
    what: &str,
) -> Vec3 {
    assert_eq!(
        overlap_depth(world, position, &component.capsule),
        0.0,
        "{what}: the fixture starts clear"
    );
    for _ in 0..ticks {
        position = step_clear(component, world, input, position, what);
    }
    position
}

/// A pawn settled on the floor under it at `position` by idle ticks.
pub(super) fn settled(
    desc: &PlayerMovementDescriptor,
    world: &CollisionWorld,
    mut position: Vec3,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    for _ in 0..20 {
        position = tick_with_immersion(&mut component, &idle_input(), world, ENV, DT, position).0;
    }
    assert!(component.is_grounded(), "the fixture pawn settles");
    (component, position)
}

/// A pawn grounded with its centre at `position`, moving at `velocity`, with
/// no settling: for poses only one tick can reach.
fn placed(
    desc: &PlayerMovementDescriptor,
    position: Vec3,
    velocity: Vec3,
) -> (PlayerMovementComponent, Vec3) {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    component.set_grounded(true);
    component.velocity = velocity;
    (component, position)
}

// --- Free advance ------------------------------------------------------------

// Regression: with the per-tick floor push spent, a second TOI-0 walkable
// contact free-advanced the capsule into a tilted surface, unswept.
#[test]
fn a_pawn_walking_onto_a_chamfered_lip_ends_every_tick_clear() {
    let desc = descriptor();
    let rise = desc.ground.step_height;
    let run = rise / 25.0_f32.to_radians().tan();
    let world = Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .slope((5.0, 5.0 + run), (0.0, rise))
        .floor((5.0 + run, 60.0), rise)
        .build();
    let running = MovementInput {
        running: true,
        ..toward_x()
    };
    for speed in [desc.ground.speed.walk, desc.ground.speed.run] {
        let (mut component, position) =
            settled(&desc, &world, Vec3::new(3.0, rest_y(&desc, 0.0), 0.0));
        component.velocity = Vec3::new(speed, 0.0, 0.0);
        run_clear(
            &mut component,
            &world,
            &running,
            position,
            60,
            "chamfered lip",
        );
    }
}

// Regression: free advance into a ramp, then ground-stick from the embedded
// pose (a TOI-0 walkable hit, drop 0), kept the pawn inside the ramp.
#[test]
fn free_advance_then_ground_stick_at_a_ramp_foot_ends_clear() {
    let desc = descriptor();
    let world = Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .slope((5.0, 5.0 + 2.0 / 25.0_f32.to_radians().tan()), (0.0, 2.0))
        .build();
    for speed in [desc.ground.speed.walk, desc.ground.speed.run] {
        let (mut component, position) =
            settled(&desc, &world, Vec3::new(3.0, rest_y(&desc, 0.0), 0.0));
        component.velocity = Vec3::new(speed, 0.0, 0.0);
        let input = MovementInput {
            running: true,
            ..toward_x()
        };
        run_clear(&mut component, &world, &input, position, 60, "ramp foot");
    }
}

// --- Floor push ----------------------------------------------------------------

// Regression: the floor push lifted a pawn resting within a skin of a ceiling
// into it, unswept.
#[test]
fn a_floor_push_under_a_ceiling_within_a_skin_ends_clear() {
    for step_height in [0.0, descriptor().ground.step_height] {
        let mut desc = descriptor();
        desc.ground.step_height = step_height;
        let rest = rest_y(&desc, 0.0);
        let head = rest + desc.capsule.half_height + desc.capsule.radius;
        let world = Geometry::default()
            .floor((-50.0, 60.0), 0.0)
            .ceiling((-50.0, 60.0), head + SKIN_DISTANCE - 1.0e-3)
            .build();
        let (mut component, position) = placed(&desc, Vec3::new(0.0, rest, 0.0), Vec3::ZERO);
        run_clear(
            &mut component,
            &world,
            &toward_x(),
            position,
            10,
            &format!("floor push, step height {step_height}"),
        );
    }
}

// --- Step-up -------------------------------------------------------------------

/// Floor at 0, a riser at x = 5 of `rise`, its top beyond, under a ceiling at
/// `ceiling_y` over both.
fn step_under_ceiling(rise: f32, ceiling_y: f32) -> CollisionWorld {
    Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .wall_x(5.0, (0.0, rise), Vec3::NEG_X)
        .floor((5.0, 60.0), rise)
        .ceiling((-50.0, 60.0), ceiling_y)
        .build()
}

fn standing_height(desc: &PlayerMovementDescriptor) -> f32 {
    2.0 * (desc.capsule.half_height + desc.capsule.radius)
}

// Regression: the step-up lift rose unswept, so a ceiling over a step whose
// top cannot fit the standing capsule took the pawn's head.
#[test]
fn a_step_without_room_for_the_standing_capsule_leaves_the_pawn_below_it_and_clear() {
    let desc = descriptor();
    // A low step, so the lift rises clear of its riser and into the ceiling.
    let rise = desc.ground.step_height / 3.0;
    // Five centimetres short of fitting the capsule on the top.
    let ceiling = rise + standing_height(&desc) - 0.05;
    let world = step_under_ceiling(rise, ceiling);
    for speed in [1.0, desc.ground.speed.walk] {
        let (mut component, position) =
            settled(&desc, &world, Vec3::new(4.0, rest_y(&desc, 0.0), 0.0));
        component.velocity = Vec3::new(speed, 0.0, 0.0);
        let end = run_clear(
            &mut component,
            &world,
            &toward_x(),
            position,
            40,
            "low step",
        );
        // The step's edge is a walkable contact the pawn may ride until the
        // ceiling stops it; its feet never reach the top.
        let feet = end.y - desc.capsule.half_height - desc.capsule.radius;
        assert!(
            end.x < 5.0 && feet < rise,
            "the pawn stays below the step, at {end:?}"
        );
    }
}

// Regression: the unswept lift needed `step_height + STEP_UP_LIFT_MARGIN` of
// headroom, so a step the standing capsule fits on was refused, the head
// sinking into the ceiling on the way.
#[test]
fn a_step_with_room_for_the_capsule_but_not_the_lift_margin_still_steps_up() {
    let desc = descriptor();
    let rise = desc.ground.step_height;
    // Room for the standing capsule a skin over the top, plus a little; the
    // full lift would need rise + 0.05 m over the lower floor.
    let ceiling = rise + standing_height(&desc) + 2.0 * SKIN_DISTANCE + 0.005;
    let world = step_under_ceiling(rise, ceiling);
    let (mut component, position) = settled(&desc, &world, Vec3::new(4.0, rest_y(&desc, 0.0), 0.0));
    let end = run_clear(
        &mut component,
        &world,
        &toward_x(),
        position,
        60,
        "tight step",
    );
    assert!(end.x > 5.5, "the pawn climbed onto the step, at {end:?}");
    assert!(
        (end.y - rest_y(&desc, rise)).abs() < 2.0e-3,
        "the pawn stands on the step top, at {end:?}"
    );
}

// The step-up rise followed by a floor push on one tick, under an overhang.
// Neither moves the pawn into it once both are swept.
#[test]
fn a_step_up_then_a_floor_push_under_an_overhang_ends_clear() {
    let desc = descriptor();
    let rise = desc.ground.step_height;
    // A walkable lip rising off the step top keeps a floor contact under the
    // lifted pawn, so the slide pushes it up a skin after the rise.
    let world = Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .wall_x(5.0, (0.0, rise), Vec3::NEG_X)
        .slope((5.0, 6.0), (rise, rise + 0.4))
        .floor((6.0, 60.0), rise + 0.4)
        .ceiling((-50.0, 60.0), rise + 0.05 + standing_height(&desc) + 0.03)
        .build();
    for speed in [1.0, desc.ground.speed.walk] {
        let (mut component, position) =
            settled(&desc, &world, Vec3::new(4.0, rest_y(&desc, 0.0), 0.0));
        component.velocity = Vec3::new(speed, 0.0, 0.0);
        run_clear(
            &mut component,
            &world,
            &toward_x(),
            position,
            40,
            "overhang",
        );
    }
}

// --- Ground-stick ----------------------------------------------------------------

/// An upper floor at `top` for x < 5, a riser facing +X, a lower floor at 0.
fn ledge_drop(top: f32) -> CollisionWorld {
    Geometry::default()
        .floor((-50.0, 5.0), top)
        .wall_x(5.0, (0.0, top), Vec3::X)
        .floor((5.0, 60.0), 0.0)
        .build()
}

// Regression: when ground-stick's swept down-cast first met the ledge edge's
// steep normal, its centre-ray fallback dropped the capsule beside the riser
// by the ray distance, unswept.
#[test]
fn ground_stick_beside_a_riser_over_a_drop_ends_clear() {
    let desc = descriptor();
    let step = desc.ground.step_height;
    // Taller than one step, within ground-stick reach.
    let world = ledge_drop(step + 0.05);
    for offset in [0.25, 0.3, 0.33] {
        for speed in [0.5, 2.0] {
            let (mut component, position) = placed(
                &desc,
                Vec3::new(5.0 + offset, rest_y(&desc, step + 0.05), 0.0),
                Vec3::new(speed, 0.0, 0.0),
            );
            run_clear(
                &mut component,
                &world,
                &toward_x(),
                position,
                10,
                &format!("drop beside riser, offset {offset}, speed {speed}"),
            );
        }
    }
}

// Regression: same centre-ray drop, reached by a pawn standing still with its
// capsule over the edge (zero velocity: no slide iteration runs).
#[test]
fn a_still_pawn_over_a_ledge_edge_ends_clear_after_ground_stick() {
    let desc = descriptor();
    let step = desc.ground.step_height;
    let world = ledge_drop(step);
    for offset in [0.3, 0.33] {
        let (mut component, position) = placed(
            &desc,
            Vec3::new(5.0 + offset, rest_y(&desc, step), 0.0),
            Vec3::ZERO,
        );
        run_clear(
            &mut component,
            &world,
            &idle_input(),
            position,
            10,
            &format!("still over edge, offset {offset}"),
        );
    }
}

// --- Stuck-stop ------------------------------------------------------------------

// Regression: the stuck-stop rollback restored the tick-start X/Z but kept the
// new Y, so a corner-wedged pawn whose blocked tick slid downhill sank into
// the ramp.
//
// At the default threshold (1 mm) the rollback sinks at most 1 mm · sin(slope),
// which stays inside the skin band: only a wider descriptor threshold drives
// it into the ramp, so the fixture widens it.
#[test]
fn a_corner_wedge_on_the_steepest_walkable_ramp_ends_clear_after_the_rollback() {
    let mut desc = descriptor();
    desc.stuck_stop_threshold = 0.1;
    let slope = (desc.ground.max_slope - 0.5).to_radians().tan();
    // The ramp falls along +X into a corner formed by a wall facing -X at x = 5
    // and a wall facing +Z at z = -1.
    let world = Geometry::default()
        .slope((-20.0, 5.0), (25.0 * slope, 0.0))
        .wall_x(5.0, (-5.0, 30.0), Vec3::NEG_X)
        .wall_z(-1.0, (-20.0, 5.0), (-5.0, 30.0), Vec3::Z)
        .build();
    let ramp_y = |x: f32| (5.0 - x) * slope;
    // Resting on a slope puts the lower sphere `(radius + skin) / cos` over the
    // surface.
    let rest_over_slope = desc.capsule.half_height
        + (desc.capsule.radius + SKIN_DISTANCE) * (1.0 + slope * slope).sqrt();
    let radius = desc.capsule.radius;
    let wall_contact_x = 5.0 - radius - SKIN_DISTANCE;
    let side_contact_z = -1.0 + radius + SKIN_DISTANCE;
    // Downhill (+X) into the front wall and back (-Z) into the side wall.
    let input = walk(Vec2::new(1.0, 1.0));
    // Falling into the corner a few centimetres uphill of it: the blocked tick
    // slides down the walls before the rollback restores its X/Z.
    for (uphill, fall) in [(0.03, -3.0), (0.06, -3.0), (0.06, -6.0)] {
        let x = wall_contact_x - uphill;
        let mut component = PlayerMovementComponent::from_descriptor(&desc);
        component.velocity = Vec3::new(4.0, fall, -0.5);
        component.coyote_timer_ms = 1000.0;
        run_clear(
            &mut component,
            &world,
            &input,
            Vec3::new(x, ramp_y(x) + rest_over_slope, side_contact_z),
            10,
            &format!("corner wedge, {uphill} m uphill falling at {fall} m/s"),
        );
    }
}

// --- Vault -----------------------------------------------------------------------

/// Floor at 0, a riser facing -X at x = 5, then a flat top at `top`. With a
/// non-zero `chamfer_deg` the lip is cut back 0.15 m at that angle, so the
/// path crosses a walkable contact tilted at it.
fn vault_world(top: f32, chamfer_deg: f32) -> CollisionWorld {
    let geometry = Geometry::default().floor((-50.0, 5.0), 0.0);
    let geometry = if chamfer_deg > 0.0 {
        let cut = 0.15;
        let run = cut / chamfer_deg.to_radians().tan();
        geometry
            .wall_x(5.0, (0.0, top - cut), Vec3::NEG_X)
            .slope((5.0, 5.0 + run), (top - cut, top))
            .floor((5.0 + run, 60.0), top)
    } else {
        geometry
            .wall_x(5.0, (0.0, top), Vec3::NEG_X)
            .floor((5.0, 60.0), top)
    };
    geometry.build()
}

fn vault_case(
    top: f32,
    chamfer_deg: f32,
    yaw: f32,
    abort_at: Option<usize>,
    after: &MovementInput,
) {
    let desc = vault_descriptor(ground_auto());
    let world = vault_world(top, chamfer_deg);
    let (mut component, mut position) =
        settled(&desc, &world, Vec3::new(4.0, rest_y(&desc, 0.0), 0.0));
    // At yaw 0 the wish's `x` is world +X and its `y` world -Z: head into the
    // riser `yaw` radians off its normal, toward +Z.
    let forward_input = walk(Vec2::new(yaw.cos(), -yaw.sin()));
    let what = format!("vault top {top}, chamfer {chamfer_deg}, yaw {yaw}, abort {abort_at:?}");
    let mut vaulted = false;
    for tick in 0..60 {
        if abort_at == Some(tick) && component.movement_state.kind() == MovementStateKind::Vault {
            // A shove off the path, as knockback lands mid-vault.
            component.knockback_velocity = Quat::from_rotation_y(-yaw) * Vec3::new(-6.0, 0.0, 0.0);
        }
        position = step_clear(&mut component, &world, &forward_input, position, &what);
        vaulted |= component.movement_state.kind() == MovementStateKind::Vault;
    }
    assert!(vaulted, "{what}: the fixture vaults");
    run_clear(&mut component, &world, after, position, 30, &what);
}

#[test]
fn vaults_never_end_a_tick_overlapping_static_geometry() {
    let max_slope = descriptor().ground.max_slope;
    // A flat lip, and one chamfered at exactly the walkable limit.
    for chamfer in [0.0, max_slope] {
        // On the path, then held forward input after the exit.
        vault_case(1.0, chamfer, 0.0, None, &toward_x());
        // Then neutral input after the exit.
        vault_case(1.0, chamfer, 0.0, None, &idle_input());
        // Oblique entry.
        vault_case(1.0, chamfer, 0.5, None, &idle_input());
        // Aborted mid-path.
        vault_case(1.0, chamfer, 0.0, Some(3), &idle_input());
    }
}
