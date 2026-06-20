// Shared collision substrate for player movement.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;

use crate::collision::{CollisionWorld, SKIN_DISTANCE, cast_capsule, cast_ray};
use crate::movement::SubstrateResult;
use crate::scripting::components::player_movement::PlayerMovementComponent;

/// Separation nudge along the contact normal applied when parry reports a
/// TOI=0 hit during the slide loop. `SKIN_DISTANCE` (the sweep's
/// `target_distance`) already provides geometric clearance — this nudge is a
/// tiny perturbation to break out of resting-contact ties so the next
/// iteration's sweep makes progress along the tangent. Matches rapier3d's
/// `KinematicCharacterController::normal_nudge_factor` default. Critically
/// it is NOT a physics step: it consumes zero `remaining_dt`, so a grounded
/// player resting on the floor does not accumulate vertical drift across
/// iterations.
const NORMAL_NUDGE: f32 = 1.0e-4;

/// Vertical lift margin added on top of `step_height` when the step-up probe
/// commits to a lifted position. Must exceed `SKIN_DISTANCE` so the lifted
/// hemisphere clears the step's top edge without parry reporting an
/// immediate skin-contact hit.
const STEP_UP_LIFT_MARGIN: f32 = 0.05;
const _: () = assert!(STEP_UP_LIFT_MARGIN > SKIN_DISTANCE);

/// Cosine threshold (≈ 60°) for detecting that a second wall contact within
/// the same tick points in a "significantly different" horizontal direction
/// from the first — the geometric signature of an interior corner wedge.
const CORNER_NORMAL_COS_THRESHOLD: f32 = 0.5;

/// Termination guard for the slide loop: when remaining motion length squared
/// falls below this, the loop exits rather than spinning on residual
/// sub-millimetre advances.
const SLIDE_REMAINING_EPSILON_SQ: f32 = 1.0e-10;

/// Quake-derived projection-capped acceleration: only adds speed along
/// `wish_dir_3d` until `wish_speed` is reached. Bunny-hopping emerges
/// naturally because perpendicular speed (built up earlier) is never bled
/// off by this function.
pub(super) fn pm_accelerate(
    velocity: &mut Vec3,
    wish_dir_3d: Vec3,
    wish_speed: f32,
    accel: f32,
    dt: f32,
) {
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
pub(super) fn wish_dir_from_input(input: Vec2, facing_yaw: f32) -> Vec3 {
    let forward = Vec3::new(-facing_yaw.sin(), 0.0, -facing_yaw.cos());
    let right = Vec3::new(facing_yaw.cos(), 0.0, -facing_yaw.sin());
    let dir = forward * input.y + right * input.x;
    if dir.length_squared() > 0.0 {
        dir.normalize()
    } else {
        Vec3::ZERO
    }
}

/// Returns the lifted position if a step-up commit is warranted: horizontal
/// motion is blocked by a wall-like surface AND a walkable surface exists
/// within `step_height + skin` below the lifted capsule. Returns `None` for
/// pure walls (nothing walkable above) so the slide loop handles them via
/// plane projection without a 0.35 m intra-tick excursion.
// Cohesive single-call physics probe params; grouping would add an abstraction
// with one production caller and no reuse.
#[allow(clippy::too_many_arguments)]
pub(super) fn step_up_lift(
    collision_world: &CollisionWorld,
    capsule: &Capsule,
    current_pos: Vec3,
    horiz_vel: Vec3,
    horiz_speed: f32,
    step_height: f32,
    cos_walkable: f32,
    remaining_dt: f32,
    radius: f32,
) -> Option<Vec3> {
    if horiz_speed <= 1e-4 || step_height <= 0.0 {
        return None;
    }
    let dir = horiz_vel / horiz_speed;
    let probe_dist = (horiz_speed * remaining_dt).max(step_height + radius);
    let probe = cast_capsule(
        collision_world,
        Point::new(current_pos.x, current_pos.y, current_pos.z),
        capsule,
        Vector::new(dir.x, dir.y, dir.z),
        probe_dist,
    )?;
    if !(probe.time_of_impact < probe_dist && probe.normal2.y.abs() < cos_walkable) {
        return None;
    }
    let lifted = current_pos + Vec3::new(0.0, step_height + STEP_UP_LIFT_MARGIN, 0.0);
    let lifted_probe = cast_capsule(
        collision_world,
        Point::new(lifted.x, lifted.y, lifted.z),
        capsule,
        Vector::new(dir.x, dir.y, dir.z),
        probe_dist,
    );
    let lifted_clear = match lifted_probe {
        None => true,
        Some(h) => h.time_of_impact >= probe_dist - SKIN_DISTANCE,
    };
    if !lifted_clear {
        return None;
    }
    // Sample beneath a point advanced past the obstacle: at decision time the
    // capsule center is still over whatever was below `current_pos` (typically
    // the lower floor for a step), so probing straight down from `lifted`
    // would miss a higher walkable surface that only exists past the riser.
    // Cap the forward offset at `radius + step_height` so narrow platforms
    // (riser plus a small top) aren't overshot when `probe.time_of_impact`
    // happens to be large (e.g. when probe_dist was inflated for high-speed
    // motion).
    let forward_offset = (probe.time_of_impact + radius + SKIN_DISTANCE).min(radius + step_height);
    let sample = lifted + dir * forward_offset;
    let down_probe = cast_capsule(
        collision_world,
        Point::new(sample.x, sample.y, sample.z),
        capsule,
        Vector::new(0.0, -1.0, 0.0),
        step_height + 0.1,
    );
    let lifted_lands_on_walkable = match down_probe {
        Some(h) => h.normal2.y >= cos_walkable,
        None => false,
    };
    if lifted_lands_on_walkable {
        Some(lifted)
    } else {
        None
    }
}

/// Shared physics substrate (movement-tick steps 7–8). Takes the desired
/// velocity (already authored on `component.velocity` by the active state's
/// intent) plus the current position/state, integrates it against the world,
/// and returns the resolved position and contact/landing results.
///
/// Runs regardless of movement state: states change *intent*, not collision.
/// This is the collide-and-slide spine — iterative sweep-and-slide, the
/// step-up probe, the per-tick floor-push budget, stuck-stop corner-wedge
/// mitigation, the ground-stick down-cast, and the `is_grounded`/`air_ticks`
/// collision-state resolution.
///
/// Carve-out: the substrate resolves *collision* state (`is_grounded`,
/// `air_ticks`) and reports landing/contact via `SubstrateResult`, but it does
/// NOT touch gameplay ability budgets. The `air_jumps_remaining` refresh stays
/// in `tick` (the landing-refresh point), driven by `SubstrateResult::hit_floor`.
///
/// `was_grounded` is the pre-intent grounded flag carried from the start of the
/// tick; `jumped` is whether step 2/3 launched a jump this tick.
pub(super) fn integrate_collision(
    component: &mut PlayerMovementComponent,
    collision_world: &CollisionWorld,
    dt: f32,
    position: Vec3,
    was_grounded: bool,
    jumped: bool,
) -> (Vec3, SubstrateResult) {
    // 7. Move + collide. Iterative sweep-and-slide against the world trimesh.
    let capsule = Capsule::new(
        Point::new(0.0, -component.capsule.half_height, 0.0),
        Point::new(0.0, component.capsule.half_height, 0.0),
        component.capsule.radius,
    );

    let mut current_pos = position;
    let mut remaining_dt = dt;
    let mut hit_floor_this_tick = false;

    // Step-up probe before the main loop: lift only commits when a wall-like
    // obstacle is in front AND a walkable surface sits beneath the lifted
    // position. Pure walls skip the lift to avoid intra-tick camera jitter.
    let horiz_vel = Vec3::new(component.velocity.x, 0.0, component.velocity.z);
    let horiz_speed = horiz_vel.length();
    let step_height = component.ground.step_height;
    if component.is_grounded {
        if let Some(lifted) = step_up_lift(
            collision_world,
            &capsule,
            current_pos,
            horiz_vel,
            horiz_speed,
            step_height,
            component.cos_walkable,
            remaining_dt,
            component.capsule.radius,
        ) {
            current_pos = lifted;
        }
    }

    if component.is_grounded && component.velocity.y.abs() < 1e-3 {
        component.velocity.y = 0.0;
    }

    // Stuck-stop deadzone bookkeeping. `slide_start_xz` lets the post-loop
    // check measure horizontal progress this tick. `last_wall_normal` carries
    // the most recent non-floor contact normal so we can detect a *second*
    // wall normal pointing in a significantly different horizontal direction
    // within the same tick — the corner-wedge case that produces orbital
    // jitter.
    let slide_start_xz = Vec2::new(current_pos.x, current_pos.z);
    let mut last_wall_normal: Option<Vec3> = None;
    let mut multi_wall_contact_seen = false;
    // Cap the cumulative vertical lift from TOI=0 floor-skin contacts at
    // `SKIN_DISTANCE + NORMAL_NUDGE` per tick. The pre-fix code unconditionally
    // pushed +0.025 on every TOI=0 floor iteration (up to 4 per tick = +0.1),
    // producing orbital jitter when a grounded player walked into a flat
    // wall. We still need a small lift to break out of the SKIN_DISTANCE band
    // and let the next sweep iteration find the real wall — but only enough
    // to clear the skin, applied at most once per tick.
    let max_floor_push = SKIN_DISTANCE + NORMAL_NUDGE;
    let mut floor_push_remaining = max_floor_push;

    for _ in 0..4 {
        let velocity = component.velocity;
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
                let natural_consumed = if speed > 0.0 {
                    toi / speed
                } else {
                    remaining_dt
                };

                let normal = Vec3::new(h.normal2.x, h.normal2.y, h.normal2.z);
                let consumed;
                if normal.y >= component.cos_walkable {
                    hit_floor_this_tick = true;
                    current_pos += dir * toi;
                    // Project velocity tangent to the surface FIRST, then
                    // enforce velocity.y = 0 as a hard rail. Zeroing y
                    // before the projection lets the dot product
                    // re-introduce a small non-zero y component
                    // (= -normal.y * (vx*nx + vz*nz)) on the next iteration.
                    let v_dot_n = component.velocity.dot(normal);
                    component.velocity -= normal * v_dot_n;
                    component.velocity.y = 0.0;
                    if toi <= 1e-6 {
                        // TOI=0 floor contact: the capsule's lower hemisphere
                        // sits inside the SKIN_DISTANCE band. Push y up by
                        // just enough to clear the skin so the next sweep
                        // iteration can find the real obstacle (e.g. a wall
                        // ahead). The push is budgeted per tick — see
                        // `floor_push_remaining` — so the cumulative lift
                        // stays bounded at one skin width regardless of how
                        // many iterations the loop runs. This kills the
                        // orbital pump the pre-fix code produced (+0.025 ×
                        // up to 4 = +0.1 m per tick).
                        if floor_push_remaining > 0.0 {
                            let push = floor_push_remaining;
                            current_pos.y += push;
                            floor_push_remaining = 0.0;
                            consumed = 0.0;
                        } else {
                            // Already pushed this tick — no further lift is
                            // safe. Free-advance tangentially and exit the
                            // loop. Ground-stick at end of tick re-establishes
                            // contact.
                            current_pos += component.velocity * remaining_dt;
                            break;
                        }
                    } else {
                        consumed = natural_consumed;
                    }
                } else {
                    current_pos += dir * toi;
                    // Non-floor (wall-ish) contact. Track for the corner-wedge
                    // detector: a second wall normal pointing in a different
                    // horizontal direction within the same tick means the
                    // slide loop is bouncing between two walls — a signature
                    // of the orbital-jitter pattern.
                    let horiz_n = Vec3::new(normal.x, 0.0, normal.z);
                    let cur_len_sq = horiz_n.length_squared();
                    if let Some(prev) = last_wall_normal {
                        let prev_len_sq = prev.length_squared();
                        if prev_len_sq > 1e-6 && cur_len_sq > 1e-6 {
                            let cos_between =
                                prev.dot(horiz_n) / (prev_len_sq.sqrt() * cur_len_sq.sqrt());
                            // A clean wall slide re-hits the same surface
                            // (cos ~ 1); any significantly different
                            // horizontal normal in the same tick means we've
                            // contacted a second wall (interior corner), the
                            // wedge case that produces orbital jitter.
                            if cos_between <= CORNER_NORMAL_COS_THRESHOLD {
                                multi_wall_contact_seen = true;
                            }
                        }
                    }
                    // Only overwrite with a real horizontal wall normal —
                    // skip near-vertical contacts (e.g. an awkward triangle
                    // edge with `n.y` close to 1) so the corner-wedge
                    // detector keeps a meaningful reference.
                    if cur_len_sq > 1e-6 {
                        last_wall_normal = Some(horiz_n);
                    }

                    let v_dot_n = component.velocity.dot(normal);
                    component.velocity -= normal * v_dot_n;
                    if toi <= 1e-6 {
                        // Separation nudge, not a physics step: see floor
                        // branch above for rationale. Zero remaining_dt
                        // consumption keeps `target_distance` separation
                        // alone from being double-counted.
                        current_pos += normal * NORMAL_NUDGE;
                        consumed = 0.0;
                    } else {
                        consumed = natural_consumed;
                    }
                }
                remaining_dt = (remaining_dt - consumed).max(0.0);
            }
        }
    }

    // Stuck-stop deadzone: classic Quake/Source corner-wedge mitigation.
    // Fires only when the slide loop saw two wall contacts whose horizontal
    // normals differ by >= 60° within the same tick AND total net horizontal
    // displacement was below `stuck_stop_threshold`. That is the geometric
    // signature of a corner wedge — the capsule alternated between two
    // distinct wall projections (commonly perpendicular interior corners)
    // without making meaningful forward progress.
    //
    // When triggered we zero `velocity.x`/`velocity.z` and roll back the
    // horizontal component of `current_pos` to its pre-slide value, killing
    // the residual XZ wobble that wall nudges and alternating projections
    // leave at a corner wedge. `velocity.y` and any vertical motion from
    // gravity / step-up / ground-stick are preserved so the rest of the
    // tick (and future ticks) handle gravity correctly.
    //
    // We deliberately do NOT trigger on "max iterations + low displacement"
    // alone: a player walking straight into a flat wall also exhausts the
    // iteration budget and has near-zero net displacement, yet must keep
    // their tangential velocity for natural wall slide. Net displacement
    // cannot distinguish the two cases — second-wall contact can.
    if component.stuck_stop_enabled && multi_wall_contact_seen {
        let horiz_disp = (Vec2::new(current_pos.x, current_pos.z) - slide_start_xz).length();
        if horiz_disp < component.stuck_stop_threshold {
            component.velocity.x = 0.0;
            component.velocity.z = 0.0;
            current_pos.x = slide_start_xz.x;
            current_pos.z = slide_start_xz.y;
        }
    }
    // Wall slide can project a small +vy when the capsule corners the edge of a
    // riser; clamp here so the ground-stick guard below still fires and prevents
    // the corner contact from latching the player above the floor.
    if was_grounded && !jumped && component.velocity.y > 0.0 {
        component.velocity.y = 0.0;
    }

    // Ground-stick also fires when the slide loop applied a floor_push this
    // tick: that push lifts y by one skin width and must be snapped back to
    // keep a wall-pinned player at the floor. Without this branch a tick that
    // clears `is_grounded` (wall-only contact, no floor) followed by a tick
    // that re-acquires floor via the push would leave the player latched at
    // settle_y + skin permanently.
    let floor_push_fired = floor_push_remaining < max_floor_push;
    // `velocity.y <= 1e-3` rather than `<= 0`: the slide loop's per-iteration
    // velocity projection can leave a sub-millimetre positive y from
    // floating-point round-off even when the player is plainly grounded.
    if (was_grounded || floor_push_fired) && component.velocity.y <= 1.0e-3 {
        let step_height = component.ground.step_height;
        if step_height > 0.0 {
            // covers step_height + STEP_UP_LIFT_MARGIN + SKIN_DISTANCE + headroom
            let max_down = step_height + STEP_UP_LIFT_MARGIN + SKIN_DISTANCE + 0.03;
            let down_hit = cast_capsule(
                collision_world,
                Point::new(current_pos.x, current_pos.y, current_pos.z),
                &capsule,
                Vector::new(0.0, -1.0, 0.0),
                max_down,
            );
            let mut snapped = false;
            if let Some(h) = down_hit {
                let n = Vec3::new(h.normal2.x, h.normal2.y, h.normal2.z);
                if n.y >= component.cos_walkable {
                    current_pos.y -= h.time_of_impact;
                    hit_floor_this_tick = true;
                    snapped = true;
                }
            }
            // Wall-normal preference fallback: when the capsule is pressed
            // against a wall the swept downcast may report the wall's normal
            // first (n.y ≈ 0). Without a fallback that silently latches the
            // player above the floor. A thin ray from the capsule center
            // straight down ignores wall geometry on the side and finds the
            // floor below.
            if !snapped {
                let half_height = component.capsule.half_height;
                let radius = component.capsule.radius;
                let ray_max = max_down + half_height + radius;
                let ray_hit = cast_ray(
                    collision_world,
                    Point::new(current_pos.x, current_pos.y, current_pos.z),
                    Vector::new(0.0, -1.0, 0.0),
                    ray_max,
                );
                if let Some(h) = ray_hit {
                    if h.normal.y >= component.cos_walkable {
                        // Ray TOI is distance from capsule center to the
                        // surface; the capsule rests with its lower hemisphere
                        // at `half_height + radius` below center, separated by
                        // SKIN_DISTANCE.
                        let target_gap = half_height + radius + SKIN_DISTANCE;
                        let drop = h.time_of_impact - target_gap;
                        // Only snap downward, and only if the floor is within
                        // the same envelope the swept downcast would have
                        // covered.
                        if drop > 0.0 && drop <= max_down {
                            current_pos.y -= drop;
                            hit_floor_this_tick = true;
                        }
                    }
                }
            }
        }
    }

    // 8. Ground-state reset + landing result. The collision-state writes
    // (`is_grounded`, `air_ticks`) resolve here; the `air_jumps_remaining`
    // ability-budget refresh is the tick's responsibility, driven by
    // `SubstrateResult::hit_floor`.
    if hit_floor_this_tick {
        component.is_grounded = true;
    } else if was_grounded && !jumped {
        // Stayed on / left the ground organically — only clear the flag when
        // no floor contact this tick. The jump branch already cleared it.
        component.is_grounded = false;
    }

    // Air-tick hysteresis. The step-up probe lifts the capsule only at genuine
    // walkable steps (pure walls skip the lift), but cornering events or the
    // single-tick gap between the sweep and the ground-stick snap can briefly
    // clear `is_grounded`. Gating `landed` on >=3 consecutive airborne ticks
    // suppresses those blips while still firing for real jumps and falls
    // (tens of ticks airborne).
    let prev_air_ticks = component.air_ticks;
    if component.is_grounded {
        component.air_ticks = 0;
    } else {
        component.air_ticks = component.air_ticks.saturating_add(1);
    }

    let landed = !was_grounded && component.is_grounded && prev_air_ticks >= 3;

    (
        current_pos,
        SubstrateResult {
            hit_floor: hit_floor_this_tick,
            landed,
        },
    )
}

/// Which extreme of the capsule stays geometrically fixed when the capsule is
/// resized in place. A resize changes `half_height`, which moves both the top
/// and the bottom of the capsule away from (or toward) the center; the anchor
/// picks which one must NOT move, and the resize helper returns the center
/// y-delta that keeps it pinned.
///
/// State-agnostic by design (D8): the substrate exposes the anchor as a
/// geometric mode, not a crouch flag. A grounded shrink/grow anchors at the
/// `Feet` (the planted contact point stays put, the head rises/drops); an
/// airborne resize anchors at the `Head` (the head stays put, the feet
/// rise/drop toward center). `movement--slide` reuses both modes without the
/// helper knowing which state called it.
// Consumed by the `Crouching` intent (grounded entry / stand-up `Feet`,
// airborne entry `Head`); `movement--slide` reuses both modes later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResizeAnchor {
    /// Lowest point `position.y - (half_height + radius)` stays fixed — feet
    /// planted on the ground (D2). Center moves by `new_half_height -
    /// old_half_height` (DOWN on a shrink).
    Feet,
    /// Highest point `position.y + (half_height + radius)` stays fixed — head
    /// pinned while airborne (D4). Center moves by `old_half_height -
    /// new_half_height` (the feet rise toward center on a shrink).
    Head,
}

/// Resize the player capsule in place to a target `half_height` / `eye_height`,
/// keeping the anchored extreme geometrically fixed, and return the center
/// y-delta the CALLER must apply to `position`.
///
/// Two-party contract (D8): the helper OWNS the capsule size fields — it writes
/// `component.capsule.half_height` and `component.capsule.eye_height` to the
/// targets — and RETURNS the center y-delta. The helper MUST NOT touch
/// `position`; the caller owns `position` and applies the returned delta. This
/// keeps the helper state-agnostic (no crouch flag): the `Crouching` intent and
/// `movement--slide` both call it with their own target sizes and anchor mode.
///
/// The resize honors the substrate's per-tick `Capsule` rebuild: `radius` is
/// unchanged and `integrate_collision` reads `half_height`/`radius` fresh each
/// tick, so there is no second capsule cache to update. `eye_height` is
/// camera-only (the collision capsule never reads it).
///
/// Anchor math (radius is constant, so it cancels):
///   - `Feet`: keep `position.y - (half_height + radius)` fixed ⇒
///     `delta = new_half_height - old_half_height` (negative on a shrink: the
///     center drops so the planted feet stay put).
///   - `Head`: keep `position.y + (half_height + radius)` fixed ⇒
///     `delta = old_half_height - new_half_height` (positive on a shrink: the
///     center rises so the pinned head stays put).
// Production caller: the `Crouching` intent (entry shrink + stand-up grow);
// `movement--slide` reuses it later.
pub(super) fn resize_capsule(
    component: &mut PlayerMovementComponent,
    target_half_height: f32,
    target_eye_height: f32,
    anchor: ResizeAnchor,
) -> f32 {
    let old_half_height = component.capsule.half_height;
    component.capsule.half_height = target_half_height;
    component.capsule.eye_height = target_eye_height;
    match anchor {
        ResizeAnchor::Feet => target_half_height - old_half_height,
        ResizeAnchor::Head => old_half_height - target_half_height,
    }
}

/// Stand-up clearance probe: is there headroom to grow the capsule from the
/// crouched size back to the standing size with the FEET PLANTED?
///
/// Collision exposes only `cast_capsule` (a sweep) — there is no overlap/
/// intersection query — so headroom is tested by sweeping the CROUCHED-size
/// capsule straight UP by the head-rise delta. With feet planted, growing the
/// half-height from crouched to standing raises the head by
/// `2 × (standing_half_height − crouched_half_height)` (the center rises by the
/// half-height delta and the top extends a further half-height delta above the
/// center). A hit within that distance means a ceiling blocks the standing
/// capsule; clear means the player can stand.
///
/// State-agnostic (D8): takes the crouched and standing half-heights as plain
/// sizes, not a crouch flag — `movement--slide` reuses it to test whether a
/// slide can stand up. Returns `true` when headroom is CLEAR.
// Production caller: the `Crouching` intent (stand-up release + crouch-jump);
// `movement--slide` reuses it later.
pub(super) fn standup_clearance_probe(
    component: &PlayerMovementComponent,
    collision_world: &CollisionWorld,
    position: Vec3,
    crouched_half_height: f32,
    standing_half_height: f32,
) -> bool {
    let head_rise = 2.0 * (standing_half_height - crouched_half_height);
    if head_rise <= 0.0 {
        // Already standing-or-taller: no growth needed, so nothing to clear.
        return true;
    }
    // Build the crouched-size parry capsule (radius unchanged). nalgebra types
    // stay inside this collision-boundary call; the result crosses back as a
    // plain bool.
    let capsule = Capsule::new(
        Point::new(0.0, -crouched_half_height, 0.0),
        Point::new(0.0, crouched_half_height, 0.0),
        component.capsule.radius,
    );
    let hit = cast_capsule(
        collision_world,
        Point::new(position.x, position.y, position.z),
        &capsule,
        Vector::new(0.0, 1.0, 0.0),
        head_rise,
    );
    // A hit strictly within the head-rise distance blocks standing. `None`
    // (nothing within range) or a hit at/after the full rise is clear.
    match hit {
        Some(h) => h.time_of_impact >= head_rise,
        None => true,
    }
}

/// Air-jump (double-jump) gate: the airborne jump fires only while a charge
/// remains in the budget AND upward velocity is still under `air.jump_ceiling`.
/// The budget itself replenishes on floor contact via `refresh_on_landing`, so
/// double-jump and the air-dash budget reset through one mechanism. The
/// ceiling rule keeps the charge from being spent at the apex of the rising arc.
pub(super) fn air_jump_ready(component: &PlayerMovementComponent) -> bool {
    component.air_jumps_remaining > 0 && component.velocity.y <= component.air.jump_ceiling
}

/// Derived jump edges for the tick, computed ONCE before the per-state intents
/// run (D5). Intents consume these in place of the raw `jump_pressed` button bit
/// so forgiveness (coyote time + jump buffering) is never re-derived per state.
///
/// `grounded` routes through `Normal`'s grounded-jump step (consumes NO air-jump
/// charge); `air` routes through the air-jump step (consumes a charge). At most
/// one is true in a tick where they would compete — the derivation prefers the
/// grounded edge so a coyote/buffered jump never also spends an air charge.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct JumpEdges {
    /// Fire a grounded jump this tick: a fresh grounded press, a coyote-granted
    /// press (ground recently lost, no jump spent), or a buffered press landing.
    pub(super) grounded: bool,
    /// Fire an air jump this tick (airborne press while the grounded edge did
    /// not claim it).
    pub(super) air: bool,
    /// The grounded edge was satisfied by consuming the pending jump buffer —
    /// the tick must clear `jump_buffer_timer_ms` so it fires exactly once.
    pub(super) consumed_buffer: bool,
}

/// Derive the grounded-jump and air-jump edges from the raw `jump_pressed` bit,
/// the pre-tick grounded flag, and the forgiveness timers/flags. Reads state
/// only; the timer advance and buffer arm/clear are applied by `tick` around the
/// dispatch (see `advance_forgiveness` / the buffer arm step).
///
/// Coyote: a grounded jump is permitted while `coyote_timer_ms <= coyote_ms`
/// after ground is lost, gated on `!jump_spent` so it cannot re-arm once any
/// jump (ground or air) has been spent this airborne stretch. When grounded the
/// timer is 0, so a normal grounded press always satisfies the edge.
///
/// Buffer: a pending buffered jump (`jump_buffer_timer_ms > 0`) fires as a
/// grounded jump the first tick the player is observed grounded again, even with
/// no fresh press — the landing-tick fire.
pub(super) fn derive_jump_edges(
    component: &PlayerMovementComponent,
    jump_pressed: bool,
) -> JumpEdges {
    let grounded = component.is_grounded;

    // A grounded jump from a fresh press: either truly grounded, or within the
    // coyote window after leaving the ground with no prior jump spent. When
    // `coyote_ms == 0` and airborne, `coyote_timer_ms` has already advanced past
    // 0 (the timer ticks at end of the leaving tick), so the coyote path is a
    // no-op — exact raw-edge timing is preserved.
    let coyote_ok = !component.jump_spent && component.coyote_timer_ms <= component.coyote_ms;
    let fresh_grounded_press = jump_pressed && (grounded || coyote_ok);

    // A pending buffered jump fires on the first observed-grounded tick (the
    // landing-tick fire), independent of a fresh press. Gated on `!jump_spent`
    // so a jump already taken this stretch does not also drain the buffer.
    let buffer_fires = grounded && component.jump_buffer_timer_ms > 0.0 && !component.jump_spent;

    let grounded_edge = fresh_grounded_press || buffer_fires;

    // Air jump only when the grounded edge did not claim the press: an airborne
    // raw press that coyote did not convert into a grounded jump.
    let air_edge = jump_pressed && !grounded_edge && !grounded;

    JumpEdges {
        grounded: grounded_edge,
        air: air_edge,
        consumed_buffer: buffer_fires,
    }
}

/// Advance the forgiveness timers for the NEXT tick, after the substrate has
/// resolved collision (so `component.is_grounded` reflects this tick's outcome).
/// Coyote accumulates airborne ms; reset to 0 each grounded tick here and at the
/// landing-refresh point (`refresh_on_landing`). The jump buffer counts down
/// while airborne and DROPS SILENTLY when it expires before landing. Windows
/// stay in ms, advanced off `dt * 1000` like the dash cooldown.
pub(super) fn advance_forgiveness(component: &mut PlayerMovementComponent, dt: f32) {
    let dt_ms = dt * 1000.0;
    if component.is_grounded {
        // Grounded: coyote timer is held at 0 (also reset by the landing-refresh
        // point). A pending buffer is left for the next tick's grounded edge to
        // consume — it is NOT counted down or dropped while grounded.
        component.coyote_timer_ms = 0.0;
    } else {
        component.coyote_timer_ms += dt_ms;
        if component.jump_buffer_timer_ms > 0.0 {
            // Expire toward zero; reaching 0 drops the buffer with no jump.
            component.jump_buffer_timer_ms = (component.jump_buffer_timer_ms - dt_ms).max(0.0);
        }
    }
}
