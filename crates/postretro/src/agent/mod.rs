// Movable navigation agent: collide-and-slide harness.
//
// A minimal, standalone capsule sweep-and-slide for navmesh agents — iterative
// slide, step-up, ground-stick, and gravity — built directly on
// `collision::cast_capsule` / `cast_ray`. It deliberately does NOT touch the
// player movement substrate (`movement::integrate_collision` / `step_up_lift`
// are private and player-coupled); this copies the *pattern* (iterative
// project-and-advance, step-up probe, ground-stick down-cast) in an
// agent-shaped, much smaller form. The agent is a custom-kinematic capsule, not
// a simulated rigid body — collision queries only.
//
// See: context/lib/movement.md §1 (custom-kinematic capsule, collision-only)
//      context/lib/entity_model.md §7 (collision)

use glam::Vec3;
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;

use crate::collision::{CollisionWorld, SKIN_DISTANCE, cast_capsule, cast_ray};

/// Iteration cap for the slide loop — bounds work when a capsule wedges into a
/// corner. Matches the player substrate's budget; four projections resolve any
/// realistic concave-wall contact within one tick.
const SLIDE_ITERATIONS: u32 = 4;

/// A surface counts as walkable (floor, not wall) when its contact normal's Y
/// component is at least this. cos(50°) ≈ 0.643; the navmesh bake's default
/// `max_slope_deg` is in this neighborhood. A fixed threshold keeps the harness
/// self-contained — it does not read the per-agent slope budget (the navmesh
/// already eroded unwalkable slopes out of the agent's corridor).
const COS_WALKABLE: f32 = 0.643;

/// Vertical lift margin added on top of `step_height` when the step-up probe
/// commits, so the lifted capsule clears the step's top edge without parry
/// reporting an immediate skin-contact hit. Must exceed `SKIN_DISTANCE`.
const STEP_UP_LIFT_MARGIN: f32 = 0.05;
const _: () = assert!(STEP_UP_LIFT_MARGIN > SKIN_DISTANCE);

/// Separation nudge applied along the contact normal on a TOI=0 (resting)
/// contact, to break out of the skin band so the next sweep iteration makes
/// tangential progress. Consumes zero `dt` (not a physics step).
const NORMAL_NUDGE: f32 = 1.0e-4;

/// Termination guard: when remaining motion length squared falls below this,
/// the slide loop exits rather than spinning on sub-millimetre advances.
const SLIDE_REMAINING_EPSILON_SQ: f32 = 1.0e-10;

/// Capsule geometry the harness sweeps with. `step_height` is carried IN (not
/// hardcoded) so the step-up / ground-stick logic uses the baked step the
/// agent was seeded with — downstream steering builds this from the agent
/// component's stored `radius` / `height` / `step_height`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AgentCapsule {
    pub(crate) radius: f32,
    /// Distance from the capsule center to one cylinder endpoint, EXCLUDING the
    /// hemispherical end cap. From the agent component: `height / 2.0 - radius`.
    pub(crate) half_height: f32,
    /// Maximum vertical step the agent climbs in one tick.
    pub(crate) step_height: f32,
}

impl AgentCapsule {
    /// Build the parry capsule. The capsule's `+Y` axis maps directly to world
    /// `+Y`, matching the `cast_capsule` / player-capsule convention; endpoints
    /// sit at `center ± half_height * Y`, the end-cap spheres extend `radius`
    /// beyond.
    fn parry(&self) -> Capsule {
        Capsule::new(
            Point::new(0.0, -self.half_height, 0.0),
            Point::new(0.0, self.half_height, 0.0),
            self.radius,
        )
    }
}

/// Result of one collide-and-slide step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SlideResult {
    /// Resolved capsule-center position after the slide + gravity + ground-stick.
    pub(crate) position: Vec3,
    /// `true` when the agent rests on (or re-acquired) a walkable floor this step.
    pub(crate) grounded: bool,
    /// Live velocity after the step (horizontal projected by wall contacts,
    /// vertical carrying gravity / ground state). The caller stores this back on
    /// the agent so momentum and fall speed persist across ticks.
    pub(crate) velocity: Vec3,
}

/// Resolve one tick of agent motion: take a desired horizontal velocity, the
/// current capsule-center position, the carried vertical velocity, and dt;
/// integrate gravity, sweep-and-slide horizontally against the world, run the
/// step-up probe, then ground-stick. Returns the resolved position, the
/// grounded flag, and the post-step velocity.
///
/// `desired_horizontal` is the steering velocity (XZ; any Y is ignored). The
/// vertical channel is owned by the harness: `vertical_velocity` carries the
/// fall speed in from the previous tick (0 when freshly grounded). `gravity` is
/// the world gravity scalar (negative — points down), supplied by the caller
/// (matching the player substrate, which takes gravity from the script ctx).
pub(crate) fn collide_and_slide(
    world: &CollisionWorld,
    capsule: &AgentCapsule,
    position: Vec3,
    desired_horizontal: Vec3,
    vertical_velocity: f32,
    gravity: f32,
    dt: f32,
) -> SlideResult {
    let parry = capsule.parry();

    // Compose the tick velocity: steering horizontal + integrated gravity. The
    // horizontal Y is dropped — steering never authors vertical motion.
    let mut velocity = Vec3::new(
        desired_horizontal.x,
        vertical_velocity + gravity * dt,
        desired_horizontal.z,
    );

    let mut current_pos = position;
    let mut remaining_dt = dt;
    let mut hit_floor = false;

    // Step-up probe before the slide loop: lift only commits when a wall-like
    // obstacle blocks horizontal motion AND a walkable surface sits within the
    // step beneath the lifted position. Pure walls skip the lift.
    let horiz = Vec3::new(velocity.x, 0.0, velocity.z);
    let horiz_speed = horiz.length();
    if let Some(lifted) = step_up_lift(
        world,
        &parry,
        capsule,
        current_pos,
        horiz,
        horiz_speed,
        remaining_dt,
    ) {
        current_pos = lifted;
    }

    // Iterative project-and-advance slide.
    for _ in 0..SLIDE_ITERATIONS {
        let speed = velocity.length();
        if speed < 1e-6 || remaining_dt <= 0.0 {
            break;
        }
        let dir = velocity / speed;
        let max_toi = speed * remaining_dt;
        if max_toi * max_toi < SLIDE_REMAINING_EPSILON_SQ {
            break;
        }
        let hit = cast_capsule(
            world,
            Point::new(current_pos.x, current_pos.y, current_pos.z),
            &parry,
            Vector::new(dir.x, dir.y, dir.z),
            max_toi,
        );
        match hit {
            None => {
                current_pos += velocity * remaining_dt;
                break;
            }
            Some(h) => {
                let toi = h.time_of_impact.max(0.0);
                let normal = Vec3::new(h.normal2.x, h.normal2.y, h.normal2.z);
                let consumed = if speed > 0.0 {
                    toi / speed
                } else {
                    remaining_dt
                };

                if normal.y >= COS_WALKABLE {
                    hit_floor = true;
                }
                current_pos += dir * toi;
                // Project velocity onto the contact plane (slide along surface).
                let v_dot_n = velocity.dot(normal);
                velocity -= normal * v_dot_n;

                if toi <= 1e-6 {
                    // Resting contact: nudge off the surface (zero-dt) so the
                    // next iteration's sweep makes tangential progress, rather
                    // than re-reporting TOI=0 in place.
                    current_pos += normal * NORMAL_NUDGE;
                } else {
                    remaining_dt = (remaining_dt - consumed).max(0.0);
                }
            }
        }
    }

    // Ground-stick: snap the capsule down onto a walkable floor within the step
    // envelope so it does not float a tick after cresting a step or sliding off
    // a wall corner. Only when not climbing (vertical velocity non-positive).
    let mut grounded = hit_floor;
    if velocity.y <= 1e-3 {
        if let Some(snapped) = ground_stick(world, &parry, capsule, current_pos) {
            current_pos = snapped;
            grounded = true;
        }
    }

    // Once resting on the floor, stop accumulating downward speed so the next
    // tick's gravity integration starts from rest (no runaway fall while
    // grounded). Wall-projected residual +Y is also cleared when grounded.
    if grounded {
        velocity.y = 0.0;
    }

    SlideResult {
        position: current_pos,
        grounded,
        velocity,
    }
}

/// Step-up probe: returns the lifted position when horizontal motion is blocked
/// by a wall-like surface AND a walkable surface exists within `step_height`
/// below the lifted capsule. Returns `None` for pure walls (handled by the
/// slide loop's plane projection) so the agent does not make a spurious
/// intra-tick vertical excursion.
fn step_up_lift(
    world: &CollisionWorld,
    parry: &Capsule,
    capsule: &AgentCapsule,
    current_pos: Vec3,
    horiz_vel: Vec3,
    horiz_speed: f32,
    remaining_dt: f32,
) -> Option<Vec3> {
    let step_height = capsule.step_height;
    if horiz_speed <= 1e-4 || step_height <= 0.0 {
        return None;
    }
    let radius = capsule.radius;
    let dir = horiz_vel / horiz_speed;
    let probe_dist = (horiz_speed * remaining_dt).max(step_height + radius);

    // 1. Is a wall-like obstacle in front?
    let probe = cast_capsule(
        world,
        Point::new(current_pos.x, current_pos.y, current_pos.z),
        parry,
        Vector::new(dir.x, dir.y, dir.z),
        probe_dist,
    )?;
    if !(probe.time_of_impact < probe_dist && probe.normal2.y.abs() < COS_WALKABLE) {
        return None;
    }

    // 2. Is the lifted capsule clear of the obstacle?
    let lifted = current_pos + Vec3::new(0.0, step_height + STEP_UP_LIFT_MARGIN, 0.0);
    let lifted_clear = match cast_capsule(
        world,
        Point::new(lifted.x, lifted.y, lifted.z),
        parry,
        Vector::new(dir.x, dir.y, dir.z),
        probe_dist,
    ) {
        None => true,
        Some(h) => h.time_of_impact >= probe_dist - SKIN_DISTANCE,
    };
    if !lifted_clear {
        return None;
    }

    // 3. Does a walkable surface sit beneath a point advanced past the riser?
    let forward_offset = (probe.time_of_impact + radius + SKIN_DISTANCE).min(radius + step_height);
    let sample = lifted + dir * forward_offset;
    let down = cast_capsule(
        world,
        Point::new(sample.x, sample.y, sample.z),
        parry,
        Vector::new(0.0, -1.0, 0.0),
        step_height + 0.1,
    );
    match down {
        Some(h) if h.normal2.y >= COS_WALKABLE => Some(lifted),
        _ => None,
    }
}

/// Ground-stick down-cast: find a walkable floor within the step envelope below
/// the capsule and return the position snapped to rest one `SKIN_DISTANCE` above
/// it. Returns `None` when no walkable floor is within range (the agent is
/// genuinely airborne). A swept down-cast first; a thin center ray as a fallback
/// when the sweep reports a wall normal (capsule pressed against a wall).
fn ground_stick(
    world: &CollisionWorld,
    parry: &Capsule,
    capsule: &AgentCapsule,
    position: Vec3,
) -> Option<Vec3> {
    let step_height = capsule.step_height;
    if step_height <= 0.0 {
        return None;
    }
    let max_down = step_height + STEP_UP_LIFT_MARGIN + SKIN_DISTANCE + 0.03;

    // Swept down-cast: returns the toi from the capsule's lower hemisphere.
    if let Some(h) = cast_capsule(
        world,
        Point::new(position.x, position.y, position.z),
        parry,
        Vector::new(0.0, -1.0, 0.0),
        max_down,
    ) {
        if h.normal2.y >= COS_WALKABLE {
            return Some(position - Vec3::new(0.0, h.time_of_impact, 0.0));
        }
    }

    // Fallback: a thin center ray ignores wall geometry on the side and finds
    // the floor below. The capsule rests with its lower hemisphere at
    // `half_height + radius` below center, separated by SKIN_DISTANCE.
    let half_height = capsule.half_height;
    let radius = capsule.radius;
    let ray_max = max_down + half_height + radius;
    let ray = cast_ray(
        world,
        Point::new(position.x, position.y, position.z),
        Vector::new(0.0, -1.0, 0.0),
        ray_max,
    )?;
    if ray.normal.y >= COS_WALKABLE {
        let target_gap = half_height + radius + SKIN_DISTANCE;
        let drop = ray.time_of_impact - target_gap;
        if drop > 0.0 && drop <= max_down {
            return Some(position - Vec3::new(0.0, drop, 0.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::Isometry;
    use parry3d::shape::TriMesh;

    /// Agent capsule fixture: 0.35 m radius, 1.8 m total height, 0.4 m step.
    /// half_height = 1.8/2 - 0.35 = 0.55.
    fn agent_capsule() -> AgentCapsule {
        AgentCapsule {
            radius: 0.35,
            half_height: 0.55,
            step_height: 0.4,
        }
    }

    /// A large floor at y=0 plus a vertical wall at x=2 (facing -X), built as a
    /// single trimesh so an agent driven toward +X must slide along the wall
    /// rather than pass through it.
    ///
    /// Floor: XZ square [-50, 50]. Wall: a tall quad in the YZ plane at x=2,
    /// spanning z in [-50, 50], y in [0, 4]. The wall is long in Z so an agent
    /// steered diagonally slides along it for the whole test rather than running
    /// off its end.
    fn floor_and_wall_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor quad.
        let base = points.len() as u32;
        points.push(Point::new(-50.0, 0.0, -50.0));
        points.push(Point::new(50.0, 0.0, -50.0));
        points.push(Point::new(50.0, 0.0, 50.0));
        points.push(Point::new(-50.0, 0.0, 50.0));
        tris.push([base, base + 1, base + 2]);
        tris.push([base, base + 2, base + 3]);

        // Wall quad at x=2 (two-sided not needed: agent approaches from -X).
        let base = points.len() as u32;
        points.push(Point::new(2.0, 0.0, -50.0));
        points.push(Point::new(2.0, 4.0, -50.0));
        points.push(Point::new(2.0, 4.0, 50.0));
        points.push(Point::new(2.0, 0.0, 50.0));
        tris.push([base, base + 1, base + 2]);
        tris.push([base, base + 2, base + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Resting height of a grounded capsule: its center sits
    /// `half_height + radius + SKIN_DISTANCE` above the floor contact (the
    /// sweep never touches geometry — it rests one skin width away).
    fn rest_height(capsule: &AgentCapsule) -> f32 {
        capsule.half_height + capsule.radius + SKIN_DISTANCE
    }

    #[test]
    fn agent_capsule_half_height_builds_centered_parry_capsule() {
        let capsule = agent_capsule();
        let parry = capsule.parry();
        assert!((parry.radius - 0.35).abs() < 1e-6);
        assert!((parry.segment.a.y - (-0.55)).abs() < 1e-6);
        assert!((parry.segment.b.y - 0.55).abs() < 1e-6);
    }

    #[test]
    fn agent_steered_into_wall_slides_and_stays_outside_collider() {
        // Drive the agent straight at the +X wall (and along +Z) over many
        // fixed ticks. It must slide along the wall, never penetrating x=2, and
        // make progress in +Z (the unobstructed tangent).
        let world = floor_and_wall_world();
        let capsule = agent_capsule();
        let dt = 1.0 / 60.0;
        let gravity = -20.0;

        // Start grounded at rest height, away from the wall.
        let mut pos = Vec3::new(0.0, rest_height(&capsule), 0.0);
        let mut vertical = 0.0;
        // Desired velocity points into the wall (+X) and along it (+Z).
        let desired = Vec3::new(4.0, 0.0, 3.0);

        // The capsule SURFACE (center + radius) must never cross the wall plane
        // at x=2 — that is "never ends inside the collider". The capsule rests
        // one SKIN_DISTANCE shy of the wall, but the resting-contact NORMAL_NUDGE
        // can let the center sit a hair inside the nominal skin band; the hard
        // invariant is the surface, not the full skin clearance. A tiny epsilon
        // absorbs float round-off at the contact.
        const PENETRATION_EPSILON: f32 = 1e-3;
        let wall_x = 2.0;

        let mut advanced_z = false;
        for _ in 0..240 {
            let result = collide_and_slide(&world, &capsule, pos, desired, vertical, gravity, dt);
            pos = result.position;
            vertical = result.velocity.y;

            assert!(
                pos.x + capsule.radius <= wall_x + PENETRATION_EPSILON,
                "capsule surface penetrated the wall: center x={} + radius {} crossed x={}",
                pos.x,
                capsule.radius,
                wall_x
            );
            if pos.z > 0.5 {
                advanced_z = true;
            }
        }

        assert!(
            advanced_z,
            "agent should slide along the wall and advance in +Z, ended at {pos:?}"
        );
        // It should have pressed up against the wall (within a capsule radius of
        // the resting clearance), proving it actually reached and slid the wall
        // rather than stopping short.
        assert!(
            pos.x > 2.0 - capsule.radius - SKIN_DISTANCE - 0.1,
            "agent should rest against the wall, x={}",
            pos.x
        );
    }

    #[test]
    fn grounded_agent_rests_one_skin_above_floor_contact() {
        // An agent with no horizontal intent, dropped slightly above the floor,
        // settles to the ground-stick rest height (one SKIN_DISTANCE above the
        // floor contact) and reports grounded.
        let world = floor_and_wall_world();
        let capsule = agent_capsule();
        let dt = 1.0 / 60.0;
        let gravity = -20.0;

        // Start a little above rest so gravity + ground-stick pull it down.
        let mut pos = Vec3::new(-1.0, rest_height(&capsule) + 0.2, 0.0);
        let mut vertical = 0.0;
        let mut grounded = false;

        for _ in 0..120 {
            let result =
                collide_and_slide(&world, &capsule, pos, Vec3::ZERO, vertical, gravity, dt);
            pos = result.position;
            vertical = result.velocity.y;
            grounded = result.grounded;
        }

        assert!(grounded, "agent should be grounded on the floor");
        let expected = rest_height(&capsule);
        assert!(
            (pos.y - expected).abs() < 1e-3,
            "rest height should be half_height + radius + SKIN_DISTANCE = {expected}, got {}",
            pos.y
        );
    }
}
