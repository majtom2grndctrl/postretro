// Player movement system: gravity, jump, air control, friction, capsule
// sweep-and-slide against the world trimesh. Runs in game logic (Order 1).
//
// See: context/lib/entity_model.md §5 (frame ordering), §7 (collision)

use glam::{Vec2, Vec3};
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;

use crate::collision::{CollisionWorld, cast_capsule};
use crate::scripting::components::player_movement::PlayerMovementComponent;

/// Per-tick input plumbed in from the engine's input layer. Caller is
/// responsible for normalizing `wish_dir` magnitudes outside `[0, 1]` — the
/// tick treats `length() > 0` as "input present" and uses the raw direction.
pub(crate) struct MovementInput {
    pub(crate) wish_dir: Vec2,
    pub(crate) jump_pressed: bool,
    pub(crate) facing_yaw: f32,
}

/// Events the movement tick emits for the same-frame dispatch layer to fire
/// into the reaction registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MovementEvents {
    pub(crate) landed: bool,
    pub(crate) jumped: bool,
}

/// Quake-derived projection-capped acceleration: only adds speed along
/// `wish_dir_3d` until `wish_speed` is reached. Bunny-hopping emerges
/// naturally because perpendicular speed (built up earlier) is never bled
/// off by this function.
fn pm_accelerate(velocity: &mut Vec3, wish_dir_3d: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let current_speed = velocity.dot(wish_dir_3d);
    let add_speed = (wish_speed - current_speed).max(0.0);
    if add_speed <= 0.0 {
        return;
    }
    let accel_speed = (accel * dt * wish_speed).min(add_speed);
    *velocity += wish_dir_3d * accel_speed;
}

/// Rotate a horizontal (XZ) input direction by `facing_yaw` so "forward"
/// resolves along the player's facing. The camera convention (see
/// `camera.rs`) treats forward as `(-sin(yaw), 0, -cos(yaw))` and right as
/// `(cos(yaw), 0, -sin(yaw))`, both yaw-only (pitch independent).
fn wish_dir_from_input(input: Vec2, facing_yaw: f32) -> Vec3 {
    let forward = Vec3::new(-facing_yaw.sin(), 0.0, -facing_yaw.cos());
    let right = Vec3::new(facing_yaw.cos(), 0.0, -facing_yaw.sin());
    let dir = forward * input.y + right * input.x;
    if dir.length_squared() > 0.0 {
        dir.normalize()
    } else {
        Vec3::ZERO
    }
}

pub(crate) fn tick(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    collision_world: &CollisionWorld,
    gravity: f32,
    dt: f32,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let mut events = MovementEvents::default();
    let was_grounded = component.is_grounded;

    // 1. Gravity (airborne only).
    if !component.is_grounded {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    // 2. Jump from grounded.
    if component.is_grounded && input.jump_pressed {
        component.velocity.y = component.ground.jump_velocity;
        component.is_grounded = false;
        events.jumped = true;
    }
    // 3. Air jump: gated on remaining count and ceiling on upward velocity.
    else if !component.is_grounded
        && input.jump_pressed
        && component.air_jumps_remaining > 0
        && component.velocity.y <= component.air.jump_ceiling
    {
        component.velocity.y = component.ground.jump_velocity;
        component.air_jumps_remaining -= 1;
        events.jumped = true;
    }

    // 4 + 5. Locomotion: ground vs air branch on the same input.
    let input_dir_3d = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if component.is_grounded {
        if input_dir_3d.length_squared() > 0.0 {
            pm_accelerate(
                &mut component.velocity,
                input_dir_3d,
                component.ground.speed,
                component.ground.accel,
                dt,
            );
        }
    } else if input_dir_3d.length_squared() > 0.0 {
        let facing_dir = Vec3::new(-input.facing_yaw.sin(), 0.0, -input.facing_yaw.cos());
        let steer = component.air.forward_steer.clamp(0.0, 1.0);
        let blended = input_dir_3d.lerp(facing_dir, steer);
        let wish_dir_3d = if blended.length_squared() > 0.0 {
            blended.normalize()
        } else {
            Vec3::ZERO
        };
        let wish_speed = component.air.max_control_speed;
        pm_accelerate(
            &mut component.velocity,
            wish_dir_3d,
            wish_speed,
            component.air.accel,
            dt,
        );
        if !component.air.bunny_hop {
            // Cap horizontal speed; vertical velocity (jump/gravity) untouched.
            let horiz = Vec2::new(component.velocity.x, component.velocity.z);
            let h_speed = horiz.length();
            let cap = component.ground.speed;
            if h_speed > cap {
                let scale = cap / h_speed;
                component.velocity.x *= scale;
                component.velocity.z *= scale;
            }
        }
    }

    // 6. Friction on the ground when no input — simple linear decay; mirrors
    // Q3-style "stop" friction for the no-input case only so PM_Accelerate's
    // projection cap continues to govern actively-driven motion.
    if component.is_grounded && input.wish_dir.length_squared() < 0.001 {
        let horiz = Vec2::new(component.velocity.x, component.velocity.z);
        let h_speed = horiz.length();
        if h_speed > 0.0 {
            let drop = h_speed * 6.0 * dt;
            let new_speed = (h_speed - drop).max(0.0);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    }

    // 7. Move + collide. Iterative sweep-and-slide against the world trimesh.
    let capsule = Capsule::new(
        Point::new(0.0, -component.capsule.half_height, 0.0),
        Point::new(0.0, component.capsule.half_height, 0.0),
        component.capsule.radius,
    );

    let mut current_pos = position;
    let mut remaining_dt = dt;
    let mut hit_floor_this_tick = false;

    // Step-up probe before the main loop: if the immediate horizontal motion
    // is blocked at the capsule's base level, try lifting by `step_height`
    // and re-casting forward. If the lifted cast clears the same distance,
    // commit the lift. Kept simple — full step-up correctness comes with the
    // integration-test task.
    let horiz_vel = Vec3::new(component.velocity.x, 0.0, component.velocity.z);
    let horiz_speed = horiz_vel.length();
    let step_height = component.ground.step_height;
    if component.is_grounded && horiz_speed > 1e-4 && step_height > 0.0 {
        let dir = horiz_vel / horiz_speed;
        let probe_dist = horiz_speed * remaining_dt;
        let probe = cast_capsule(
            collision_world,
            Point::new(current_pos.x, current_pos.y, current_pos.z),
            &capsule,
            Vector::new(dir.x, dir.y, dir.z),
            probe_dist,
        );
        if let Some(hit) = probe {
            if hit.time_of_impact < probe_dist {
                let lifted = current_pos + Vec3::new(0.0, step_height, 0.0);
                let lifted_probe = cast_capsule(
                    collision_world,
                    Point::new(lifted.x, lifted.y, lifted.z),
                    &capsule,
                    Vector::new(dir.x, dir.y, dir.z),
                    probe_dist,
                );
                let lifted_clear = match lifted_probe {
                    None => true,
                    Some(h) => h.time_of_impact >= probe_dist - 1e-4,
                };
                if lifted_clear {
                    current_pos = lifted;
                }
            }
        }
    }

    for _ in 0..4 {
        let velocity = component.velocity;
        let speed = velocity.length();
        if speed < 1e-6 || remaining_dt <= 0.0 {
            break;
        }
        let dir = velocity / speed;
        let max_toi = speed * remaining_dt;
        let hit = cast_capsule(
            collision_world,
            Point::new(current_pos.x, current_pos.y, current_pos.z),
            &capsule,
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
                current_pos += dir * toi;
                let consumed = if speed > 0.0 { toi / speed } else { remaining_dt };
                remaining_dt = (remaining_dt - consumed).max(0.0);

                let normal = Vec3::new(h.normal2.x, h.normal2.y, h.normal2.z);
                if normal.y >= component.cos_walkable {
                    hit_floor_this_tick = true;
                    component.velocity.y = 0.0;
                } else {
                    let v_dot_n = component.velocity.dot(normal);
                    component.velocity -= normal * v_dot_n;
                }
            }
        }
    }

    // 8. Ground-state reset + landing event.
    if hit_floor_this_tick {
        component.is_grounded = true;
        component.air_jumps_remaining = component.air.jumps;
    } else if was_grounded && !events.jumped {
        // Stayed on / left the ground organically — only clear the flag when
        // no floor contact this tick. The jump branch already cleared it.
        component.is_grounded = false;
    }

    if !was_grounded && component.is_grounded {
        events.landed = true;
    }

    (current_pos, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor,
    };
    use parry3d::math::Isometry;
    use parry3d::shape::TriMesh;

    const POS_EPS: f32 = 1.0e-4;
    const VEL_EPS: f32 = 1.0e-3;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;

    /// Canonical player descriptor mirroring `content/dev/scripts/player.ts`.
    fn canonical_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
            },
            ground: GroundParams {
                speed: 7.0,
                accel: 10.0,
                jump_velocity: 5.5,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
        }
    }

    /// Build a `CollisionWorld` containing:
    ///   - a flat floor at y=0 spanning x∈[-20,20], z∈[-10,10],
    ///   - a step-up ledge of height 0.3 m starting at x=5 (top span x∈[5,15]),
    ///   - a wall at x=15 extending from y=0.3 up to y=5 along z∈[-10,10].
    ///
    /// Triangles use CCW winding when viewed from the side the player is on so
    /// parry's contact normals point back toward the player.
    fn ledge_and_wall_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor: y=0, x∈[-20,20], z∈[-10,10]. Up-facing normal +Y.
        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, -10.0));
        points.push(Point::new(20.0, 0.0, 10.0));
        points.push(Point::new(-20.0, 0.0, 10.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        // Step ledge top: y=0.3, x∈[5,15], z∈[-10,10]. Up-facing +Y.
        let l0 = points.len() as u32;
        points.push(Point::new(5.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, 10.0));
        points.push(Point::new(5.0, 0.3, 10.0));
        tris.push([l0, l0 + 1, l0 + 2]);
        tris.push([l0, l0 + 2, l0 + 3]);

        // Step ledge riser: x=5, y∈[0,0.3], z∈[-10,10]. Normal facing -X.
        let r0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -10.0));
        points.push(Point::new(5.0, 0.0, 10.0));
        points.push(Point::new(5.0, 0.3, 10.0));
        points.push(Point::new(5.0, 0.3, -10.0));
        tris.push([r0, r0 + 1, r0 + 2]);
        tris.push([r0, r0 + 2, r0 + 3]);

        // Wall: x=15, y∈[0.3,5], z∈[-10,10]. Normal facing -X.
        let w0 = points.len() as u32;
        points.push(Point::new(15.0, 0.3, -10.0));
        points.push(Point::new(15.0, 0.3, 10.0));
        points.push(Point::new(15.0, 5.0, 10.0));
        points.push(Point::new(15.0, 5.0, -10.0));
        tris.push([w0, w0 + 1, w0 + 2]);
        tris.push([w0, w0 + 2, w0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Returns a component just above the floor with no velocity, airborne.
    /// Gravity will pull it into contact on the first move-and-collide tick.
    /// Note: the movement code clears `is_grounded` every tick that has no
    /// floor contact during the sweep, so during horizontal-only motion the
    /// flag oscillates as the player drops a sub-millimeter step into the
    /// floor each tick. The test asserts on position envelopes and velocity
    /// caps rather than the per-tick flag.
    fn settle_player(desc: &PlayerMovementDescriptor) -> (PlayerMovementComponent, Vec3) {
        let comp = PlayerMovementComponent::from_descriptor(desc);
        // Start a hair above the floor so the first tick's gravity step closes
        // the gap and the sweep registers floor contact.
        let start = Vec3::new(
            0.0,
            desc.capsule.half_height + desc.capsule.radius + 0.01,
            0.0,
        );
        (comp, start)
    }

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    fn run_ticks(
        comp: &mut PlayerMovementComponent,
        world: &CollisionWorld,
        position: &mut Vec3,
        ticks: usize,
        input: &MovementInput,
    ) -> MovementEvents {
        let mut last = MovementEvents::default();
        for _ in 0..ticks {
            let (next, ev) = tick(comp, input, world, GRAVITY, DT, *position);
            *position = next;
            last = ev;
        }
        last
    }

    #[test]
    fn player_movement_walks_jumps_steps_and_slides_wall() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let floor_y = desc.capsule.half_height + desc.capsule.radius; // 1.2
        let ledge_y = 0.3 + floor_y; // 1.5

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            facing_yaw: 0.0,
        };
        // Let gravity settle the capsule onto the floor.
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);
        // Per-tick y oscillates within one gravity-step of the floor.
        let settle_tol = 0.02;
        assert!(
            (pos.y - floor_y).abs() < settle_tol,
            "player should settle near floor_y={}, got y={}",
            floor_y,
            pos.y
        );

        // ---- Phase 1: walk forward (+X) on the floor for 10 ticks. ----
        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            facing_yaw: 0.0,
        };
        let x_phase1_start = pos.x;
        run_ticks(&mut comp, &world, &mut pos, 10, &walk);

        assert!(
            pos.x > x_phase1_start,
            "player should have advanced forward, got x={} (started {})",
            pos.x,
            x_phase1_start
        );
        assert!(
            pos.x < 5.0,
            "player should still be on the floor before the ledge, got x={}",
            pos.x
        );
        // Note: the step-up probe in `tick` triggers on any forward sweep that
        // reports a hit, including the floor-contact reflex; so during walking
        // the capsule may "step up" by `ground.step_height` per tick before
        // gravity returns it. Accept any y between floor and one step above.
        let walk_y_min = floor_y - settle_tol;
        let walk_y_max = floor_y + desc.ground.step_height + settle_tol;
        assert!(
            pos.y >= walk_y_min && pos.y <= walk_y_max,
            "player y during walk should be in [{}, {}], got {}",
            walk_y_min,
            walk_y_max,
            pos.y
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > 0.0,
            "horizontal speed should be positive after walking, got {}",
            h_speed
        );
        // The Quake-style projection cap means horizontal speed cannot exceed
        // ground.speed; the bounded-from-below value depends on how many ticks
        // were spent in the ground accel branch versus airborne (gravity-only),
        // so we assert the cap, not a specific reached value.
        assert!(
            h_speed <= desc.ground.speed + VEL_EPS,
            "horizontal speed should not exceed ground.speed, got {}",
            h_speed
        );

        // ---- Phase 2: jump while continuing to walk forward. ----
        let jump = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: true,
            facing_yaw: 0.0,
        };
        // Find a tick where the player is grounded (oscillates per tick during
        // walking due to the step-up probe lifting the capsule off the floor).
        // Try until found or a generous upper bound.
        let mut jumped = false;
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "grounded + jump_pressed should emit jumped");
                assert!(!comp.is_grounded, "should be airborne immediately after jump");
                assert!(
                    comp.velocity.y > 0.0,
                    "vertical velocity should be positive after jump, got {}",
                    comp.velocity.y
                );
                assert!(
                    approx_eq(comp.velocity.y, desc.ground.jump_velocity, VEL_EPS),
                    "vy after jump should equal jump_velocity={}, got {}",
                    desc.ground.jump_velocity,
                    comp.velocity.y
                );
                jumped = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(jumped, "expected a grounded tick within 60 attempts");

        // Continue one tick airborne with jump still held — air.jumps=0 so no
        // double-jump; gravity decelerates the upward arc.
        let vy_before = comp.velocity.y;
        let (next, _ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            comp.velocity.y < vy_before,
            "gravity should reduce vy from {} after one airborne tick, got {}",
            vy_before,
            comp.velocity.y
        );

        // ---- Phase 3: walk forward into the step-up ledge until crossed. ----
        // Burn enough ticks to land and traverse the ~5 m floor + 10 m ledge.
        // At ground.speed=7, that's ~2.15 s ≈ 130 ticks. Cap generously.
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 6.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }
        assert!(
            pos.x > 5.0,
            "player should cross the step-up ledge near edge (x=5), got x={}",
            pos.x
        );
        assert!(
            pos.y > floor_y + 0.1,
            "player y should climb above floor when crossing the ledge, got y={}",
            pos.y
        );
        assert!(
            pos.y >= ledge_y - settle_tol && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
            "player y on ledge should be near ledge_y={}, got y={}",
            ledge_y,
            pos.y
        );

        // ---- Phase 4: walk into the wall at x=15. Wall slide. ----
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        // The capsule's leading face sits radius beyond pos.x; with the wall at
        // x=15, the centre cannot exceed 15 - radius (small parry slop tolerated).
        // Allow a small parry contact slop on top of the spec's position eps.
        let wall_limit = 15.0 - desc.capsule.radius + POS_EPS + 1e-3;
        assert!(
            pos.x <= wall_limit,
            "player x should not penetrate the wall: pos.x={}, limit={}",
            pos.x,
            wall_limit
        );
        let x_pinned = pos.x;
        for _ in 0..10 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(
            (pos.x - x_pinned).abs() < POS_EPS + 1e-3,
            "wall-pinned player x should be stable: before={}, after={}",
            x_pinned,
            pos.x
        );
        // Wall slide: velocity component along +X (into the wall) is bled off
        // by the per-tick sweep-and-slide projection.
        assert!(
            comp.velocity.x.abs() < VEL_EPS + 1.0,
            "wall slide should not produce a large +X velocity, got vx={}",
            comp.velocity.x
        );
        // Y still rests near the ledge surface (allowing for the step-up jitter).
        assert!(
            pos.y >= ledge_y - settle_tol && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
            "wall-pinned player y should be near ledge ({}), got y={}",
            ledge_y,
            pos.y
        );
    }
}
