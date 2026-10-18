// Tick overlap invariant for agents: own movement never ends a tick inside
// static geometry.
// See: context/lib/movement.md §1, §7 · context/lib/entity_model.md §7

use glam::Vec3;

use super::{AgentCapsule, SlideResult, collide_and_slide};
use crate::collision::{CollisionCapsule, CollisionWorld, SKIN_DISTANCE};
use postretro_collision::EnvironmentView;

const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
/// Overlap past resting contact that counts as intersecting static geometry.
const OVERLAP_TOLERANCE: f32 = 5.0e-4;
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
#[should_panic(expected = "inside static geometry")]
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
#[should_panic(expected = "climbed the step")]
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
#[should_panic(expected = "inside static geometry")]
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
#[should_panic(expected = "inside static geometry")]
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
