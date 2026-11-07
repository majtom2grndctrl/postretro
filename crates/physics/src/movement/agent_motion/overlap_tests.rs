// Tick overlap invariant for agents: own movement never ends a tick inside
// static geometry. Also the source gates on the controller's shared primitives,
// and how an agent leaves a mover it stands inside or is shoved off.
// See: context/lib/movement.md §1, §7 · context/lib/entity_model.md §7

use glam::Vec3;

use super::{AgentCapsule, SlideResult, collide_and_slide, settle_corpse};
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource};
use crate::collision::{CollisionCapsule, CollisionWorld, OVERLAP_TOLERANCE, SKIN_DISTANCE};
use crate::movement::mover_carry::displace_out_of_hull;
use postretro_collision::EnvironmentView;
use postretro_foundation::GroundRef;

const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const SPAN: (f32, f32) = (-50.0, 50.0);

/// The agent controller fixture capsule: radius 0.35, 1.8 m tall.
fn capsule(step_height: f32) -> AgentCapsule {
    AgentCapsule {
        radius: 0.35,
        half_height: 0.55,
        step_height,
    }
}

const STEP: f32 = 0.4;

fn rest_y(capsule: &AgentCapsule, floor_y: f32) -> f32 {
    floor_y + capsule.half_height + capsule.radius + SKIN_DISTANCE
}

fn height(capsule: &AgentCapsule) -> f32 {
    2.0 * (capsule.half_height + capsule.radius)
}

#[derive(Default)]
struct Geometry {
    points: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

impl Geometry {
    fn quad(mut self, corners: [Vec3; 4], normal: Vec3) -> Self {
        let base = self.points.len() as u32;
        self.points.extend(corners);
        let face = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if face.dot(normal) >= 0.0 {
            self.tris.push([base, base + 1, base + 2]);
            self.tris.push([base, base + 2, base + 3]);
        } else {
            self.tris.push([base, base + 2, base + 1]);
            self.tris.push([base, base + 3, base + 2]);
        }
        self
    }

    fn floor(self, x: (f32, f32), y: f32) -> Self {
        let (z0, z1) = SPAN;
        self.quad(
            [
                Vec3::new(x.0, y, z0),
                Vec3::new(x.1, y, z0),
                Vec3::new(x.1, y, z1),
                Vec3::new(x.0, y, z1),
            ],
            Vec3::Y,
        )
    }

    fn ceiling(self, x: (f32, f32), y: f32) -> Self {
        let (z0, z1) = SPAN;
        self.quad(
            [
                Vec3::new(x.0, y, z0),
                Vec3::new(x.1, y, z0),
                Vec3::new(x.1, y, z1),
                Vec3::new(x.0, y, z1),
            ],
            Vec3::NEG_Y,
        )
    }

    fn wall_x(self, x: f32, y: (f32, f32), facing: Vec3) -> Self {
        let (z0, z1) = SPAN;
        self.quad(
            [
                Vec3::new(x, y.0, z0),
                Vec3::new(x, y.1, z0),
                Vec3::new(x, y.1, z1),
                Vec3::new(x, y.0, z1),
            ],
            facing,
        )
    }

    fn build(self) -> CollisionWorld {
        CollisionWorld::from_triangles_for_test(self.points, self.tris)
    }
}

fn step(
    world: &CollisionWorld,
    capsule: &AgentCapsule,
    position: Vec3,
    desired: Vec3,
    vertical: f32,
) -> SlideResult {
    step_on(
        &CombinedCollisionWorld::static_only(world),
        capsule,
        position,
        GroundRef::World,
        desired,
        vertical,
    )
}

fn step_on(
    collision: &CombinedCollisionWorld<'_>,
    capsule: &AgentCapsule,
    position: Vec3,
    ground: GroundRef,
    desired: Vec3,
    vertical: f32,
) -> SlideResult {
    let mut knockback = Vec3::ZERO;
    collide_and_slide(
        collision,
        capsule,
        position,
        ground,
        desired,
        vertical,
        &mut knockback,
        EnvironmentView::level_only(GRAVITY),
        DT,
    )
}

/// Run `ticks` agent ticks steering at `desired`, asserting each ends clear.
fn run_clear(
    world: &CollisionWorld,
    capsule: &AgentCapsule,
    mut position: Vec3,
    desired: Vec3,
    ticks: usize,
    what: &str,
) -> (Vec3, bool) {
    let shape = CollisionCapsule::new(capsule.radius, capsule.half_height);
    assert_eq!(
        world.overlap_depth_for_test(position, shape),
        0.0,
        "{what}: the fixture starts clear"
    );
    let mut vertical = 0.0;
    let mut grounded = false;
    for _ in 0..ticks {
        let result = step(world, capsule, position, desired, vertical);
        position = result.position;
        vertical = result.velocity.y;
        grounded = result.ground.is_grounded();
        let depth = world.overlap_depth_for_test(position, shape);
        assert!(
            depth <= OVERLAP_TOLERANCE,
            "{what}: tick ended {depth} m inside static geometry at {position:?}"
        );
    }
    (position, grounded)
}

/// Floor at 0, a riser facing -X at x = 5 of `rise`, its top beyond, under a
/// ceiling at `ceiling_y` over both.
fn step_under_ceiling(rise: f32, ceiling_y: f32) -> CollisionWorld {
    Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .wall_x(5.0, (0.0, rise), Vec3::NEG_X)
        .floor((5.0, 60.0), rise)
        .ceiling((-50.0, 60.0), ceiling_y)
        .build()
}

/// An upper floor at `top` for x < 5, a riser facing +X, a lower floor at 0.
fn ledge_drop(top: f32) -> CollisionWorld {
    Geometry::default()
        .floor((-50.0, 5.0), top)
        .wall_x(5.0, (0.0, top), Vec3::X)
        .floor((5.0, 60.0), 0.0)
        .build()
}

// Regression: the agent's step-up lift rose unswept into the ceiling over a
// step whose top cannot fit its capsule.
#[test]
fn an_agent_walking_into_a_step_without_headroom_stays_below_it_and_clear() {
    let capsule = capsule(STEP);
    // Ten centimetres short of fitting the capsule on the top, yet clear over
    // the lower floor, and close enough that the lifted capsule's core stays
    // out of the ceiling (a pierced core makes the lift's probe refuse).
    let rise = 0.6 * STEP;
    let world = step_under_ceiling(rise, rise + height(&capsule) - 0.1);
    for speed in [1.0, 4.0] {
        let (end, _) = run_clear(
            &world,
            &capsule,
            Vec3::new(4.0, rest_y(&capsule, 0.0), 0.0),
            Vec3::new(speed, 0.0, 0.0),
            40,
            "agent low step",
        );
        assert!(end.x < 5.0, "the agent stays below the step, at {end:?}");
    }
}

// Regression: the unswept lift needed `step_height + 0.05` of headroom, so an
// agent was refused a step its capsule fits on.
#[test]
fn an_agent_still_steps_up_with_room_for_its_capsule_but_not_the_lift_margin() {
    let capsule = capsule(STEP);
    let world = step_under_ceiling(STEP, STEP + height(&capsule) + 2.0 * SKIN_DISTANCE + 0.005);
    let (end, grounded) = run_clear(
        &world,
        &capsule,
        Vec3::new(4.0, rest_y(&capsule, 0.0), 0.0),
        Vec3::new(3.0, 0.0, 0.0),
        60,
        "agent tight step",
    );
    assert!(
        grounded && end.x > 5.5,
        "the agent climbed the step, at {end:?}"
    );
    assert!(
        (end.y - rest_y(&capsule, STEP)).abs() < 2.0e-3,
        "the agent stands on the step top, at {end:?}"
    );
}

// Regression: the agent's ground-stick fell back to a centre ray when its
// swept down-cast met the ledge edge, dropping the capsule beside the riser
// by the ray distance, unswept.
#[test]
fn agent_ground_stick_beside_a_riser_over_a_drop_ends_clear() {
    let capsule = capsule(STEP);
    // Taller than one step, within ground-stick reach.
    let top = STEP + 0.05;
    let world = ledge_drop(top);
    for offset in [0.22, 0.26, 0.3] {
        run_clear(
            &world,
            &capsule,
            Vec3::new(5.0 + offset, rest_y(&capsule, top), 0.0),
            Vec3::new(0.5, 0.0, 0.0),
            10,
            &format!("agent drop beside riser, offset {offset}"),
        );
    }
}

// Regression: an idle agent with no destination still settles through
// ground-stick every tick, so the centre-ray drop reached it standing still.
#[test]
fn an_idle_agent_settling_over_a_ledge_edge_ends_clear() {
    let capsule = capsule(STEP);
    let world = ledge_drop(STEP);
    for offset in [0.26, 0.3] {
        run_clear(
            &world,
            &capsule,
            Vec3::new(5.0 + offset, rest_y(&capsule, STEP), 0.0),
            Vec3::ZERO,
            10,
            &format!("idle agent over edge, offset {offset}"),
        );
    }
}

/// The controller's source, without its test modules.
fn controller_source() -> &'static str {
    let src = include_str!("mod.rs");
    &src[..src.find("#[cfg(test)]").expect("the controller has tests")]
}

// Own position changes go through a swept move: a convention, so this gate
// holds it. Outside the wall-contact nudge (below the overlap tolerance), every
// write is the slide sweep's own advance or a `movement::swept` result.
#[test]
fn the_agent_controller_writes_no_body_position_outside_a_swept_move() {
    // Every write, once per occurrence: a new write fails, and so does one
    // this list no longer matches.
    let mut allowed = vec![
        "current_pos = apply_mover_carry(position, previous_ground, collision);",
        "current_pos = displace_out_of_hull(previous_ground, collision, &parry_shape, current_pos)",
        "current_pos = lifted;",
        "current_pos += velocity * remaining_dt;",
        "current_pos += dir * toi;",
        "current_pos += normal * NORMAL_NUDGE;",
        "current_pos = snapped;",
    ];
    let mut writes: Vec<&str> = controller_source()
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .map(|line| line.trim_start_matches("let ").trim_start_matches("mut "))
        .filter(|line| {
            [
                "current_pos =",
                "current_pos +=",
                "current_pos -=",
                "current_pos.",
            ]
            .iter()
            .any(|write| line.starts_with(write) && !line.contains("=="))
        })
        .collect();
    writes.sort_unstable();
    allowed.sort_unstable();
    assert_eq!(
        writes, allowed,
        "the agent controller's position writes changed: an unswept write, or a stale allowlist"
    );
}

// Players and enemies share the geometry primitives, not controllers: the
// agent reaches step-up, snap-to-floor, mover carry, displacement, release
// velocity and the push-blocked test only through `movement::swept` and
// `movement::mover_carry`, and the player substrate calls the same ones.
#[test]
fn the_agent_controller_holds_no_step_up_or_snap_to_floor_of_its_own() {
    let src = controller_source();
    for copy in [
        "fn step_up",
        "fn ground_stick",
        "fn snap_to_floor",
        // A floor probe or lift of its own.
        "cast_ray",
        "step_height +",
        // Mover contact of its own.
        "fn apply_mover_carry",
        "fn displace_from_movers",
        "fn mover_release_velocity",
        "fn ground_ref_from_hit",
        "angular_velocity",
        "linear_velocity",
        "deepest_mover",
        "mover_push_is_blocked_by_static",
        "cast_capsule_parry",
    ] {
        assert!(
            !src.contains(copy),
            "the agent controller holds its own `{copy}`"
        );
    }
    assert!(
        src.contains("snap_to_floor, step_up};"),
        "the agent controller calls the shared primitives"
    );
    let substrate = include_str!("../substrate.rs");
    for call in [
        "apply_mover_carry(",
        "displace_from_movers(",
        "mover_release_velocity(",
        "ground_ref_from_hit(",
    ] {
        assert!(src.contains(call), "the agent controller calls `{call}`");
        assert!(
            substrate.contains(call),
            "the player substrate calls `{call}`"
        );
    }
    let primitives = include_str!("../mover_carry.rs");
    assert!(
        primitives.contains("mover_push_is_blocked_by_static("),
        "displacement holds the one push-blocked test"
    );
}

// --- Controller call rules ---------------------------------------------------------

#[test]
fn with_step_height_zero_an_agent_neither_steps_up_nor_snaps_to_the_floor() {
    let capsule = capsule(0.0);
    // Taller than the lower sphere's centre, so the slide cannot ride its edge.
    let low = capsule.radius + 0.15;
    let world = Geometry::default()
        .floor((-50.0, 5.0), 0.0)
        .wall_x(5.0, (0.0, low), Vec3::NEG_X)
        .floor((5.0, 60.0), low)
        .build();
    let (end, _) = run_clear(
        &world,
        &capsule,
        Vec3::new(4.0, rest_y(&capsule, 0.0), 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        40,
        "agent step zero",
    );
    assert!(end.x < 5.0, "no step-up with step height zero, at {end:?}");

    // Over a 5 cm ledge with the centre far enough past the edge that the
    // hemisphere's down-sweep meets the lower floor before the edge:
    // snap-to-floor drops the agent onto it in one tick, gravity alone does
    // not.
    let top = 0.05;
    let world = ledge_drop(top);
    let over_edge = Vec3::new(5.3, rest_y(&capsule, top), 0.0);
    let one_tick =
        |capsule: &AgentCapsule| step(&world, capsule, over_edge, Vec3::ZERO, 0.0).position;
    let snapped = one_tick(&self::capsule(STEP));
    assert!(
        (snapped.y - rest_y(&capsule, 0.0)).abs() < 2.0e-3,
        "control: a step height snaps to the lower floor, at {snapped:?}"
    );
    let unsnapped = one_tick(&capsule);
    assert!(
        unsnapped.y > rest_y(&capsule, 0.0) + 0.02,
        "no snap to the lower floor with step height zero, at {unsnapped:?}"
    );
}

#[test]
fn a_falling_agent_beside_a_step_still_steps_up() {
    // The agent's own call rule: step-up runs whenever it is not rising,
    // grounded or not; the pawn's runs only grounded. Over a pit, nothing
    // drops the lifted agent back, so the lift shows: it climbs out onto the
    // step's top instead of falling.
    let capsule = capsule(STEP);
    let world = Geometry::default()
        .floor((-50.0, 5.0), -10.0)
        .wall_x(5.0, (-10.0, 0.0), Vec3::NEG_X)
        .floor((5.0, 60.0), 0.0)
        .build();
    // Falling, feet a fifth of a step below the top's edge.
    let feet = -0.2 * STEP;
    let start = Vec3::new(
        5.0 - capsule.radius - SKIN_DISTANCE,
        feet + capsule.half_height + capsule.radius,
        0.0,
    );
    let mut position = start;
    let mut vertical = -1.0;
    let mut grounded = false;
    for _ in 0..30 {
        let result = step(
            &world,
            &capsule,
            position,
            Vec3::new(3.0, 0.0, 0.0),
            vertical,
        );
        position = result.position;
        vertical = result.velocity.y;
        grounded = result.ground.is_grounded();
    }
    assert!(
        grounded && position.x > 5.0 && (position.y - rest_y(&capsule, 0.0)).abs() < 2.0e-3,
        "the falling agent stepped up onto the top, at {position:?}"
    );
}

#[test]
fn an_agent_steps_onto_a_mover_top_as_its_pawn_neighbour_does() {
    struct Still;
    impl MoverPoseSource for Still {
        fn pose(&self, id: u32) -> Option<MoverPose> {
            (id == 1).then_some(MoverPose {
                transform: postretro_entities::Transform {
                    position: Vec3::new(5.0, 0.0, 0.0),
                    ..Default::default()
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: glam::Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: DT,
            })
        }
    }
    let rise = 0.25;
    let (z0, z1) = (-4.0, 4.0);
    let movers = [MoverCollider::from_local_triangles(
        1,
        &[
            Vec3::new(0.0, 0.0, z0),
            Vec3::new(0.0, rise, z0),
            Vec3::new(0.0, rise, z1),
            Vec3::new(0.0, 0.0, z1),
            Vec3::new(0.0, rise, z0),
            Vec3::new(4.0, rise, z0),
            Vec3::new(4.0, rise, z1),
            Vec3::new(0.0, rise, z1),
        ],
        &[[0, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]],
    )
    .unwrap()];
    let world = Geometry::default().floor((-50.0, 60.0), 0.0).build();
    let collision = CombinedCollisionWorld::new(&world, &movers, &Still);
    let capsule = capsule(STEP);
    let mut position = Vec3::new(3.0, rest_y(&capsule, 0.0), 0.0);
    let mut ground = GroundRef::World;
    let mut vertical = 0.0;
    for _ in 0..80 {
        let result = step_on(
            &collision,
            &capsule,
            position,
            ground,
            Vec3::new(3.0, 0.0, 0.0),
            vertical,
        );
        position = result.position;
        ground = result.ground;
        vertical = result.velocity.y;
    }
    assert_eq!(ground, GroundRef::Mover(1), "the agent stands on the mover");
    assert!(
        position.x > 6.0 && (position.y - rest_y(&capsule, rise)).abs() < 2.0e-3,
        "on the mover top, at {position:?}"
    );
}

// --- Leaving a mover ---------------------------------------------------------------

const HULL: u32 = 7;

impl Geometry {
    /// A closed box, wound outward. Each horizontal face splits along the line
    /// from its (min x, min z) corner to its (max x, max z) one.
    fn solid_box(self, min: Vec3, max: Vec3) -> Self {
        let c = Vec3::new;
        self.quad(
            [
                c(min.x, min.y, min.z),
                c(max.x, min.y, min.z),
                c(max.x, min.y, max.z),
                c(min.x, min.y, max.z),
            ],
            Vec3::NEG_Y,
        )
        .quad(
            [
                c(min.x, max.y, min.z),
                c(max.x, max.y, min.z),
                c(max.x, max.y, max.z),
                c(min.x, max.y, max.z),
            ],
            Vec3::Y,
        )
        .quad(
            [
                c(min.x, min.y, min.z),
                c(min.x, max.y, min.z),
                c(min.x, max.y, max.z),
                c(min.x, min.y, max.z),
            ],
            Vec3::NEG_X,
        )
        .quad(
            [
                c(max.x, min.y, min.z),
                c(max.x, max.y, min.z),
                c(max.x, max.y, max.z),
                c(max.x, min.y, max.z),
            ],
            Vec3::X,
        )
        .quad(
            [
                c(min.x, min.y, min.z),
                c(max.x, min.y, min.z),
                c(max.x, max.y, min.z),
                c(min.x, max.y, min.z),
            ],
            Vec3::NEG_Z,
        )
        .quad(
            [
                c(min.x, min.y, max.z),
                c(max.x, min.y, max.z),
                c(max.x, max.y, max.z),
                c(min.x, max.y, max.z),
            ],
            Vec3::Z,
        )
    }

    fn mover(self, mover_id: u32) -> MoverCollider {
        MoverCollider::from_local_triangles(mover_id, &self.points, &self.tris)
            .expect("the box is a valid mover")
    }
}

/// A crusher 4 m square and 3 m tall, its origin at its bottom face.
fn crusher() -> MoverCollider {
    Geometry::default()
        .solid_box(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0))
        .mover(HULL)
}

/// A lift deck 4 m square and 0.2 m thick, its origin at its top face.
fn thin_deck() -> MoverCollider {
    Geometry::default()
        .solid_box(Vec3::new(-2.0, -0.2, -2.0), Vec3::new(2.0, 0.0, 2.0))
        .mover(HULL)
}

/// The mover at `at`, having moved by `delta` this tick.
struct Posed {
    at: Vec3,
    delta: Vec3,
}

impl MoverPoseSource for Posed {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        (mover_id == HULL).then_some(MoverPose {
            transform: postretro_entities::Transform {
                position: self.at,
                ..Default::default()
            },
            linear_velocity: self.delta / DT,
            tick_delta: self.delta,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: glam::Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: DT,
        })
    }
}

/// Where a body stands for these tests: well off every face's split and edges,
/// so a face it crosses reads that face's own normal.
fn body_on(capsule: &AgentCapsule, floor_y: f32) -> Vec3 {
    Vec3::new(0.8, rest_y(capsule, floor_y), -0.8)
}

fn parry_shape(capsule: &AgentCapsule) -> parry3d::shape::Capsule {
    CollisionCapsule::new(capsule.radius, capsule.half_height).parry()
}

// Regression: the way out of a hull assumed the capsule spans only its diameter
// across the face it leaves by. A crusher bottom pierced along the capsule's
// axis then read a negative exit, and the body was dragged up inside.
#[test]
fn a_body_inside_a_rising_crusher_leaves_through_its_bottom_or_stays_on_the_floor() {
    let capsule = capsule(STEP);
    let shape = parry_shape(&capsule);
    let body = body_on(&capsule, 0.0);
    let movers = [crusher()];
    // The bottom face sits under the centre by less than the axis half-length
    // less the radius.
    let posed = Posed {
        at: Vec3::new(0.0, body.y - 0.1, 0.0),
        delta: Vec3::Y * 0.02,
    };

    let floor = Geometry::default().floor((-50.0, 60.0), 0.0).build();
    let collision = CombinedCollisionWorld::new(&floor, &movers, &posed);
    assert_eq!(
        displace_out_of_hull(GroundRef::World, &collision, &shape, body),
        Some(body),
        "the floor blocks the way out: it stays"
    );

    let pit = Geometry::default().floor((-50.0, 60.0), -50.0).build();
    let collision = CombinedCollisionWorld::new(&pit, &movers, &posed);
    let out = displace_out_of_hull(GroundRef::World, &collision, &shape, body)
        .expect("the body is inside the hull");
    let top = out.y + capsule.half_height + capsule.radius;
    assert!(
        (top - (posed.at.y - SKIN_DISTANCE)).abs() < 5.0e-3,
        "out through the bottom face, a skin under it: top {top}, face {}",
        posed.at.y
    );
}

// Regression: the same short exit pushed a body inside a thin descending deck
// down into the floor, unchecked against it.
#[test]
fn a_body_inside_a_thin_descending_deck_leaves_through_its_nearer_top() {
    let capsule = capsule(STEP);
    let body = body_on(&capsule, 0.0);
    let movers = [thin_deck()];
    let posed = Posed {
        at: Vec3::new(0.0, body.y + 0.05, 0.0),
        delta: Vec3::NEG_Y * 0.03,
    };
    let floor = Geometry::default().floor((-50.0, 60.0), 0.0).build();
    let collision = CombinedCollisionWorld::new(&floor, &movers, &posed);
    let out = displace_out_of_hull(GroundRef::World, &collision, &parry_shape(&capsule), body)
        .expect("the body is inside the hull");
    let bottom = out.y - capsule.half_height - capsule.radius;
    assert!(
        (bottom - (posed.at.y + SKIN_DISTANCE)).abs() < 5.0e-3,
        "out through the top face, a skin over it: bottom {bottom}, top {}",
        posed.at.y
    );
}

// Regression: on the tick the crusher's bottom face rose past the body's
// centre, its swept push, aimed along the mover's motion, dragged the body up
// after it by the whole tick's travel and more.
#[test]
fn a_crusher_rising_off_a_corpse_or_a_crush_immune_agent_leaves_it_in_place() {
    let capsule = capsule(STEP);
    let floor = Geometry::default().floor((-50.0, 60.0), 0.0).build();
    let movers = [crusher()];
    let delta = Vec3::Y * DT;
    for corpse in [true, false] {
        let start = body_on(&capsule, 0.0);
        let (mut body, mut ground, mut vertical) = (start, GroundRef::World, 0.0);
        // Half a tick off, so no tick ends with the bottom face on the centre,
        // where inside and outside are undefined.
        let mut bottom = start.y - 0.15 - 0.5 * delta.y;
        for tick in 0..90 {
            bottom += delta.y;
            let posed = Posed {
                at: Vec3::new(0.0, bottom, 0.0),
                delta,
            };
            let collision = CombinedCollisionWorld::new(&floor, &movers, &posed);
            let result = if corpse {
                settle_corpse(
                    &collision,
                    &capsule,
                    body,
                    ground,
                    vertical,
                    EnvironmentView::level_only(GRAVITY),
                    DT,
                )
            } else {
                step_on(&collision, &capsule, body, ground, Vec3::ZERO, vertical)
            };
            (body, ground, vertical) = (result.position, result.ground, result.velocity.y);
            assert!(
                (body - start).length() < 1.0e-2,
                "corpse {corpse}, tick {tick}: moved from {start:?} to {body:?}"
            );
        }
    }
}

#[test]
fn a_thin_lift_descending_through_a_corpse_never_pushes_it_into_the_floor() {
    let capsule = capsule(STEP);
    let floor = Geometry::default().floor((-50.0, 60.0), 0.0).build();
    let movers = [thin_deck()];
    let start = body_on(&capsule, 0.0);
    let (mut body, mut ground, mut vertical) = (start, GroundRef::World, 0.0);
    let mut top: f32 = 2.5;
    for tick in 0..150 {
        let next = (top - 2.0 * DT).max(0.2);
        let posed = Posed {
            at: Vec3::new(0.0, next, 0.0),
            delta: Vec3::Y * (next - top),
        };
        top = next;
        let collision = CombinedCollisionWorld::new(&floor, &movers, &posed);
        let result = settle_corpse(
            &collision,
            &capsule,
            body,
            ground,
            vertical,
            EnvironmentView::level_only(GRAVITY),
            DT,
        );
        (body, ground, vertical) = (result.position, result.ground, result.velocity.y);
        assert!(
            body.y >= start.y - 1.0e-2,
            "tick {tick}: pushed down to {body:?} under the deck top {top}"
        );
    }
    assert!(
        (body.y - rest_y(&capsule, 0.2)).abs() < 1.0e-2,
        "the corpse came out on top and rode the deck down, at {body:?}"
    );
}

// The player keeps a mover ground through an upward shove so the next tick
// carries it once more and hands it the mover's velocity. An agent's component
// keeps the ground the same way, and its controller then does the same.
#[test]
fn an_agent_shoved_up_off_a_moving_platform_keeps_the_platform_velocity() {
    let capsule = capsule(STEP);
    let world = Geometry::default().floor((-50.0, 60.0), -10.0).build();
    let movers = [thin_deck()];
    let delta = Vec3::X * (3.0 * DT);
    let posed = Posed { at: delta, delta };
    let collision = CombinedCollisionWorld::new(&world, &movers, &posed);
    let start = Vec3::new(0.0, rest_y(&capsule, 0.0), 0.0);
    let mut knockback = Vec3::Y * 4.0;
    let result = collide_and_slide(
        &collision,
        &capsule,
        start,
        GroundRef::Mover(HULL),
        Vec3::ZERO,
        4.0,
        &mut knockback,
        EnvironmentView::level_only(GRAVITY),
        DT,
    );
    assert_eq!(result.ground, GroundRef::Airborne, "it left the platform");
    assert!(
        (result.position.x - delta.x).abs() < 1.0e-4,
        "carried once more, to {:?}",
        result.position
    );
    assert!(
        (knockback.x - 3.0).abs() < 1.0e-3,
        "the platform's velocity rides on in its knockback: {knockback:?}"
    );
}

/// The mover at `at`, turned by `turn` about +Y this tick.
struct Turned {
    at: Vec3,
    turn: f32,
}

impl MoverPoseSource for Turned {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        (mover_id == HULL).then_some(MoverPose {
            transform: postretro_entities::Transform {
                position: self.at,
                ..Default::default()
            },
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::Y * (self.turn / DT),
            tick_rotation_delta: glam::Quat::from_rotation_y(self.turn),
            carry_yaw: false,
            tick_dt: DT,
        })
    }
}

#[test]
fn walking_into_a_mover_reports_it_as_the_wall_only_while_it_moves_or_turns() {
    let capsule = capsule(STEP);
    let floor = Geometry::default().floor(SPAN, 0.0).build();
    let movers = [crusher()];
    // The crusher's -x face stands at x 1; the agent starts just short of it.
    let at = Vec3::new(3.0, 0.0, 0.0);
    let start = Vec3::new(
        1.0 - capsule.radius - SKIN_DISTANCE - 0.01,
        rest_y(&capsule, 0.0),
        0.0,
    );
    let walk = Vec3::new(4.0, 0.0, 0.0);
    let walled = |poses: &dyn MoverPoseSource| {
        let collision = CombinedCollisionWorld::new(&floor, &movers, poses);
        step_on(&collision, &capsule, start, GroundRef::World, walk, 0.0).walled_by_moving_mover
    };

    let sliding = Posed {
        at,
        delta: Vec3::new(0.0, 0.0, 0.05),
    };
    assert_eq!(walled(&sliding), Some(HULL), "sliding across the way");
    let turning = Turned { at, turn: 0.05 };
    assert_eq!(walled(&turning), Some(HULL), "turning in place");
    let still = Posed {
        at,
        delta: Vec3::ZERO,
    };
    assert_eq!(walled(&still), None, "a mover at rest is a plain wall");

    // A rising deck is floor, not wall.
    let movers = [thin_deck()];
    let rising = Posed {
        at: Vec3::ZERO,
        delta: Vec3::new(0.0, 0.05, 0.0),
    };
    let collision = CombinedCollisionWorld::new(&floor, &movers, &rising);
    let on_deck = Vec3::new(0.0, rest_y(&capsule, 0.0), 0.0);
    let result = step_on(
        &collision,
        &capsule,
        on_deck,
        GroundRef::Mover(HULL),
        walk,
        0.0,
    );
    assert_eq!(
        result.walled_by_moving_mover, None,
        "its rider is not walled"
    );
}
