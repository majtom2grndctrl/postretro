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
