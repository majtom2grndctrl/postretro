// Tick overlap invariant for agents: own movement never ends a tick inside
// static geometry.
// See: context/lib/movement.md §1, §7 · context/lib/entity_model.md §7

use glam::Vec3;

use super::{AgentCapsule, SlideResult, collide_and_slide};
use crate::collision::{CollisionCapsule, CollisionWorld, OVERLAP_TOLERANCE, SKIN_DISTANCE};
use postretro_collision::EnvironmentView;

const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const SPAN: (f32, f32) = (-50.0, 50.0);

/// The agent harness fixture capsule: radius 0.35, 1.8 m tall.
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
    let mut knockback = Vec3::ZERO;
    collide_and_slide(
        world,
        capsule,
        position,
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
        grounded = result.grounded;
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
        "current_pos = position;",
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
// agent reaches step-up and snap-to-floor only through `movement::swept`.
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
    ] {
        assert!(
            !src.contains(copy),
            "the agent controller holds its own `{copy}`"
        );
    }
    assert!(
        src.contains("use crate::movement::swept::{StepParams, snap_to_floor, step_up};"),
        "the agent controller calls the shared primitives"
    );
}

// --- The warning -------------------------------------------------------------------

#[test]
fn an_overlap_warns_once_per_level_load_naming_descriptor_texture_and_position() {
    use log::Level;
    use postretro_entities::{
        DescriptorProvenance, DescriptorSpawnPath, EntityRegistry, Transform,
    };
    use postretro_test_log_capture::LogCapture;

    use crate::movement::overlap_check::{check_own_movement, disarm_overlap_assert_for_test};

    // A floor whose two triangles carry texture 0 of the level's names.
    let level = || {
        CollisionWorld::from_textured_triangles_for_test(
            vec![
                Vec3::new(-50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, 50.0),
                Vec3::new(-50.0, 0.0, 50.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
            vec![0, 0],
            vec![String::from("metal/floor_grate")],
        )
    };
    let mut registry = EntityRegistry::new();
    let mut spawn = |name: &str| {
        let body = registry.spawn(Transform::default());
        registry
            .set_component(
                body,
                DescriptorProvenance {
                    canonical_name: String::from(name),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        body
    };
    let grunt = spawn("enemy.grunt");
    let drone = spawn("enemy.drone");
    let shape = CollisionCapsule::new(0.35, 0.55);
    let clear = Vec3::new(0.0, 0.92, 0.0);
    // Engine (x, y, z) = (-2.54, 0.85, 5.08) is TrenchBroom (-200 100 33).
    let sunk = Vec3::new(-2.54, 0.85, 5.08);
    let _disarmed = disarm_overlap_assert_for_test();
    let capture = LogCapture::start();

    let world = level();
    let report = check_own_movement(&world, clear, sunk, shape)
        .expect("an overlapping end pose handed to the check is reported");
    assert_eq!(report.position, sunk, "the check moves nothing");
    crate::sim::report_static_overlap(&world, &registry, grunt, &report);
    capture.assert_logged_once(
        Level::Warn,
        "[Movement] `enemy.grunt` ended a tick inside static geometry against texture \
         `metal/floor_grate` at TrenchBroom (-200 100 33)",
    );
    // A second overlap in the same level load is silent, whoever it is.
    crate::sim::report_static_overlap(&world, &registry, drone, &report);
    capture.assert_logged_once(Level::Warn, "inside static geometry");
    capture.assert_not_logged(Level::Warn, "enemy.drone");

    // A level reload builds a new world, which warns again.
    let reloaded = level();
    let report = check_own_movement(&reloaded, clear, sunk, shape).unwrap();
    crate::sim::report_static_overlap(&reloaded, &registry, grunt, &report);
    let warnings = capture
        .records()
        .iter()
        .filter(|record| {
            record.level == Level::Warn && record.message.contains("inside static geometry")
        })
        .count();
    assert_eq!(warnings, 2, "the reload re-arms the one warning");

    // An untextured face names the position alone.
    let untextured = Geometry::default().floor((-50.0, 50.0), 0.0).build();
    let report = check_own_movement(&untextured, clear, sunk, shape).unwrap();
    crate::sim::report_static_overlap(&untextured, &registry, drone, &report);
    capture.assert_logged_once(
        Level::Warn,
        "[Movement] `enemy.drone` ended a tick inside static geometry at TrenchBroom \
         (-200 100 33); further overlaps this level stay silent",
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
        grounded = result.grounded;
    }
    assert!(
        grounded && position.x > 5.0 && (position.y - rest_y(&capsule, 0.0)).abs() < 2.0e-3,
        "the falling agent stepped up onto the top, at {position:?}"
    );
}

#[test]
fn an_agent_ignores_a_mover_top_its_pawn_neighbour_steps_onto() {
    use crate::collision::moving::{
        CombinedCollisionWorld, MoverCollider, MoverPose, MoverPoseSource,
    };

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
    // The pawn's query sees the mover and steps onto its top...
    let combined = CombinedCollisionWorld::new(&world, &movers, &Still);
    let start = Vec3::new(3.0, rest_y(&capsule(STEP), 0.0), 0.0);
    let lifted = crate::movement::swept::step_up(
        &combined,
        CollisionCapsule::new(0.35, 0.55),
        Vec3::new(4.5, 0.92, 0.0),
        Vec3::new(3.0, 0.0, 0.0),
        DT,
        crate::movement::swept::StepParams {
            step_height: STEP,
            lift_margin: crate::collision::STEP_UP_LIFT_MARGIN,
            cos_walkable: crate::collision::COS_WALKABLE,
        },
    );
    assert!(
        lifted.is_some(),
        "the combined query steps up onto the mover"
    );
    // ...while the agent, querying static geometry only, walks on at floor
    // height through where the mover stands, as it always has.
    let (end, grounded) = run_clear(
        &world,
        &capsule(STEP),
        start,
        Vec3::new(3.0, 0.0, 0.0),
        80,
        "agent past mover",
    );
    assert!(grounded && end.x > 6.0, "the agent walked on, at {end:?}");
    assert!(
        (end.y - start.y).abs() < 1.0e-3,
        "at floor height, never on the mover"
    );
}
