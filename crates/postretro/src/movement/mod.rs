// Player movement state machine: per-state velocity intent (Normal, Dash), the
// shared physics substrate (capsule sweep-and-slide, step-up, ground-stick), and
// the tick dispatcher.
// Caller supplies the world gravity scalar (from `ScriptCtx::gravity`). See: context/lib/movement.md §4, context/lib/entity_model.md §5, §7

use glam::{Vec2, Vec3};
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;

use crate::collision::{CollisionWorld, SKIN_DISTANCE, cast_capsule, cast_ray};
use crate::scripting::components::player_movement::{MovementState, PlayerMovementComponent};

/// Exponential-style ground deceleration (`v *= max(0, 1 - k*dt)`) — not the Q3
/// stop/slide-threshold friction model. Value matches Quake's default
/// `sv_friction` (6.0). Two use sites:
///   1. `Normal` step 6 / `apply_normal_horizontal_decay`: grounded with no
///      movement input held — the stop-friction that bleeds idle speed. Gated
///      on no-input because `pm_accelerate` already caps in-band grounded speed.
///   2. `dash_intent`'s `dash_drag == 0` grounded boost path: applied
///      UNCONDITIONALLY (no no-input gate). The boost deliberately sits above
///      the grounded cap, so the no-input gate cannot apply — a held stick must
///      still bleed the over-cap boost rather than freezing it.
///
/// Promote to `GroundParams` if per-entity friction tuning becomes necessary.
const GROUND_STOP_FRICTION: f32 = 6.0;

/// Hard upper bound on how long the `Dash` state can persist, in milliseconds.
/// A seamed engine constant (not a descriptor field): it bounds the state so a
/// dash with high retained momentum or zero drag cannot linger indefinitely.
/// When the elapsed-time guard reaches this, the dash exits into `Normal`
/// regardless of speed. 200 ms ≈ 12 ticks at 60 Hz — a snappy Doom-Eternal /
/// Titanfall-shaped burst.
const DASH_MAX_MS: f32 = 200.0;

/// Fractional margin above the run cap before the held-input grounded
/// over-speed bleed (`normal_intent` step 6) engages. `pm_accelerate`'s
/// projection clamp leaves sub-unit floating-point overshoot just above the cap
/// during normal running (~1e-4); reacting to that would perturb steady-state
/// motion. Real banked momentum (a dash handing off above the cap) clears this
/// margin by a wide margin, so 0.2 % cleanly separates signal from float noise.
const OVERSPEED_BLEED_MARGIN: f32 = 1.002;

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

/// Per-tick input plumbed in from the engine's input layer. Keep `wish_dir`
/// component magnitudes within `[0, 1]` — the raw x/y values drive threshold
/// checks (`.length_squared() < 0.001`, `.y.abs() > 1e-3`) that are
/// sensitive to diagonal magnitudes. The 3D world-space direction derived from
/// `wish_dir` is normalized internally before being applied to locomotion.
pub(crate) struct MovementInput {
    pub(crate) wish_dir: Vec2, // x = right, y = forward
    pub(crate) jump_pressed: bool,
    /// Dash rising edge: TRUE only on the tick the dash button is first pressed,
    /// not while held. Unlike `jump_pressed` (a level signal — `Pressed|Held`),
    /// a held dash would re-fire every cooldown-ready tick, so the edge is
    /// mandatory. The call site derives it from `ButtonState::Pressed` only.
    pub(crate) dash_pressed: bool,
    /// Sprint held this tick. Selects `ground.speed.run` over `.walk` as the
    /// omnidirectional horizontal speed target; affects strafe and forward
    /// motion equally (standard shooter sprint, not forward-only).
    pub(crate) running: bool,
    pub(crate) facing_yaw: f32,
}

/// Events the movement tick emits for the same-frame dispatch layer to fire
/// into the reaction registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MovementEvents {
    pub(crate) landed: bool,
    pub(crate) jumped: bool,
}

/// Contact/landing results returned by `integrate_collision` (the shared
/// physics substrate). The substrate resolves collision state on the
/// component itself (`is_grounded`, `air_ticks`, `velocity`); these fields
/// report the gameplay-relevant outcomes the tick maps onto events and
/// ability-budget refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SubstrateResult {
    /// Floor contact was (re)acquired this tick — either through the slide
    /// loop or the ground-stick down-cast. The tick uses this as the
    /// landing-refresh point for ability budgets (e.g. `air_jumps_remaining`),
    /// a gameplay-state write the substrate deliberately does not perform.
    pub(crate) hit_floor: bool,
    /// A genuine landing transition (airborne → grounded) cleared the
    /// air-tick hysteresis gate. Maps directly to `MovementEvents::landed`.
    pub(crate) landed: bool,
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

/// Returns the lifted position if a step-up commit is warranted: horizontal
/// motion is blocked by a wall-like surface AND a walkable surface exists
/// within `step_height + skin` below the lifted capsule. Returns `None` for
/// pure walls (nothing walkable above) so the slide loop handles them via
/// plane projection without a 0.35 m intra-tick excursion.
// Cohesive single-call physics probe params; grouping would add an abstraction
// with one production caller and no reuse.
#[allow(clippy::too_many_arguments)]
fn step_up_lift(
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
fn integrate_collision(
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

/// Air-jump (double-jump) gate: the airborne jump fires only while a charge
/// remains in the budget AND upward velocity is still under `air.jump_ceiling`.
/// The budget itself replenishes on floor contact via `refresh_on_landing`, so
/// double-jump and the air-dash budget reset through one mechanism. The
/// ceiling rule keeps the charge from being spent at the apex of the rising arc.
fn air_jump_ready(component: &PlayerMovementComponent) -> bool {
    component.air_jumps_remaining > 0 && component.velocity.y <= component.air.jump_ceiling
}

/// The `Normal` state's per-tick velocity intent (movement-tick steps 1–6):
/// gravity, jump/air-jump, ground/air acceleration, ground friction, and the
/// airborne horizontal cap. This is the walk/run/jump/air-control baseline —
/// the behavior-unchanged locomotion.
///
/// Operates on `component.velocity`, reading the grounded flag carried from the
/// previous tick (`component.is_grounded`). Steps 2/3 may clear `is_grounded`
/// when a jump launches; that clear is part of the intent (a jump is no longer
/// grounded) and the substrate reads the post-intent flag.
///
/// Sets `events.jumped` when a jump launches. Returns the next movement state if
/// a transition is warranted, or `None` to stay in `Normal`. Today `Normal`
/// transitions to `Dash` on a rising-edge dash input (see `try_enter_dash`);
/// future states (crouch, slide, wall-run) plug in behind the same seam without
/// reshaping callers.
fn normal_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    gravity: f32,
    dt: f32,
    events: &mut MovementEvents,
) -> Option<MovementState> {
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
    // 3. Air-jump (double-jump): a named airborne ability under the budget
    // model. Consumes one charge from `air_jumps_remaining`, which refreshes
    // uniformly on floor contact through `refresh_on_landing` (the single
    // landing-refresh point shared with other air-budget abilities, e.g. air-dash). The
    // ceiling gate (`velocity.y <= air.jump_ceiling`) keeps it from firing at
    // the top of the rising arc; the launch reuses the ground jump velocity.
    else if !component.is_grounded && input.jump_pressed && air_jump_ready(component) {
        component.velocity.y = component.ground.jump_velocity;
        component.air_jumps_remaining -= 1;
        events.jumped = true;
    }

    // 4 + 5. Locomotion: ground vs air branch on the same input. Sprint picks
    // the run speed; the same value caps airborne horizontal speed so a
    // sprint-then-jump arc doesn't instantly decelerate mid-air.
    let ground_speed = if input.running {
        component.ground.speed.run
    } else {
        component.ground.speed.walk
    };
    let input_dir_3d = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if component.is_grounded {
        if input_dir_3d.length_squared() > 0.0 {
            pm_accelerate(
                &mut component.velocity,
                input_dir_3d,
                ground_speed,
                component.ground.accel,
                dt,
            );
        }
    } else if input_dir_3d.length_squared() > 0.0 {
        // Blend toward facing only on forward/back input: strafing left/right
        // should not redirect the capsule toward the player's nose.
        let wish_dir_3d = if input.wish_dir.y.abs() > 1e-3 {
            let facing_dir = Vec3::new(-input.facing_yaw.sin(), 0.0, -input.facing_yaw.cos());
            let steer = component.air.forward_steer.clamp(0.0, 1.0);
            let blended = input_dir_3d.lerp(facing_dir, steer);
            if blended.length_squared() > 0.0 {
                blended.normalize()
            } else {
                Vec3::ZERO
            }
        } else {
            input_dir_3d
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
            let cap = ground_speed;
            if h_speed > cap {
                let scale = cap / h_speed;
                component.velocity.x *= scale;
                component.velocity.z *= scale;
            }
        }
    }

    // 6. Ground friction. With no directional input, bleed toward a stop. With
    // input held, bleed only the *over-speed* above the run cap back toward the
    // cap: `pm_accelerate` governs actively-driven motion up to the cap but
    // cannot remove speed already above it, and the stop-friction is
    // no-input-only. In normal play a grounded player never exceeds the cap, so
    // the input-held branch is a no-op there; it exists so post-dash over-speed
    // decays even while the stick is held, and a dash hands back into the steady
    // band cleanly after the `DASH_MAX_MS` guard.
    if component.is_grounded && input.wish_dir.length_squared() < 0.001 {
        let horiz = Vec2::new(component.velocity.x, component.velocity.z);
        let h_speed = horiz.length();
        if h_speed > 0.0 {
            let drop = h_speed * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(0.0);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    } else if component.is_grounded {
        // Input held: `pm_accelerate` governs motion up to the run cap but cannot
        // remove speed already above it, and the stop-friction above is
        // no-input-only. Bleed only the *over-speed* above the cap back toward it
        // so a dash (which deliberately exceeds the cap) decays even while the
        // stick is held. The `OVERSPEED_BLEED_MARGIN` guard keeps this a no-op in
        // normal play, where running sits at the cap modulo float overshoot.
        let h_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
        if h_speed > ground_speed * OVERSPEED_BLEED_MARGIN {
            let drop = (h_speed - ground_speed) * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(ground_speed);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    }

    // `Normal` → `Dash`: fire on the dash rising edge when the cooldown is ready
    // and — if airborne — an air-dash charge remains. Disabled when no
    // `DashParams` is materialized (descriptor omitted `movement.dash`). The
    // entry blends velocity (retained base + boost), applies `preserve_vertical`,
    // consumes the airborne charge, and arms the cooldown; it returns the seeded
    // `Dash` state for `tick` to apply after the substrate resolves collision.
    if input.dash_pressed {
        if let Some(next) = try_enter_dash(component, input) {
            return Some(next);
        }
    }

    None
}

/// Attempt the `Normal` → `Dash` transition this tick. Returns the seeded `Dash`
/// state when the dash fires, or `None` when it is suppressed (dash disabled,
/// cooldown active, or airborne with no charge left).
///
/// Grounded vs airborne is read from the last-tick `is_grounded` flag — the same
/// one-tick staleness the jump gate uses (acceptable, consistent tradeoff).
/// Grounded dashes are gated by cooldown ONLY and consume no air-dash charge;
/// airborne dashes additionally require (and consume) one air-dash charge.
/// Cooldown applies to every dash.
fn try_enter_dash(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
) -> Option<MovementState> {
    let dash = component.dash.as_ref()?;
    if component.dash_cooldown_ms > 0.0 {
        return None;
    }
    if !component.is_grounded {
        // Airborne dash additionally requires (and consumes) one air-dash charge.
        if component.air_dashes_remaining == 0 {
            return None;
        }
        component.air_dashes_remaining -= 1;
    }

    let boost_speed = dash.boost_speed;
    let momentum_retention = dash.momentum_retention;
    let cooldown_ms = dash.cooldown_ms;
    let preserve_vertical = dash.preserve_vertical;

    // Dash direction: the player's input `wish_dir` when non-zero (already
    // rotated into world space and normalized by `wish_dir_from_input`), else
    // the pure `facing_yaw` forward direction.
    let dash_dir = {
        let from_input = wish_dir_from_input(input.wish_dir, input.facing_yaw);
        if from_input.length_squared() > 0.0 {
            from_input
        } else {
            let yaw = input.facing_yaw;
            Vec3::new(-yaw.sin(), 0.0, -yaw.cos())
        }
    };

    // Layered velocity (D4). The retained term is the BASE (keeps decaying under
    // `Normal`'s friction during the dash); `dash_direction × boost_speed` is the
    // additive BOOST layer that `dash_drag` decays. Entry horizontal velocity =
    // base + boost. At `momentum_retention = 0` the dash replaces prior
    // horizontal velocity; at `1` it is fully additive.
    let prior_horiz = Vec3::new(component.velocity.x, 0.0, component.velocity.z);
    let base = prior_horiz * momentum_retention;
    let boost = dash_dir * boost_speed;
    component.velocity.x = base.x + boost.x;
    component.velocity.z = base.z + boost.z;

    // `preserve_vertical` is applied ONCE on entry: false zeroes vertical
    // velocity; true keeps the entering value (gravity resumes from there).
    if !preserve_vertical {
        component.velocity.y = 0.0;
    }

    // Arm the cooldown for every dash. It decrements unconditionally each tick in
    // `tick`, outside the per-state dispatch. Note: `tick` decrements by `dt*1000`
    // on this same entry tick, so the effective cooldown is `cooldown_ms - dt*1000`
    // (one tick short). Accepted as harmless — reordering the arm risks the
    // cooldown test, and a sub-tick of cooldown makes no observable difference.
    component.dash_cooldown_ms = cooldown_ms;

    Some(MovementState::Dash {
        elapsed_ms: 0.0,
        boost,
    })
}

/// The `Dash` state's per-tick velocity intent. Gravity runs normally; the
/// jump/air-jump branch is omitted by design — the dash is a short committed
/// burst (hard-bounded by `DASH_MAX_MS`), so jump input is intentionally dropped
/// for its duration; full jump access returns on exit to `Normal`. Input
/// steering (`pm_accelerate`) is scaled by
/// `steer_control` — omitted entirely at 0 (committed dash). Horizontal decay
/// acts on the BOOST layer (D4); the retained base keeps decaying under
/// `Normal`'s contextual friction throughout. Exits into `Normal` when total
/// horizontal speed falls back into the steady band, or when the `DASH_MAX_MS`
/// elapsed guard fires, whichever is first.
fn dash_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    gravity: f32,
    dt: f32,
    elapsed_ms: f32,
    boost: Vec3,
) -> Option<MovementState> {
    // Dash params must exist to be in this state (entry required `Some`). A
    // descriptor swap that cleared `dash` mid-dash drops back to `Normal` rather
    // than panicking.
    let Some(dash) = component.dash.as_ref() else {
        return Some(MovementState::Normal);
    };
    let steer_control = dash.steer_control;
    let dash_drag = dash.dash_drag;

    // Gravity runs normally (FPS-shaped: the dash does not suspend it).
    if !component.is_grounded {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    let ground_speed = if input.running {
        component.ground.speed.run
    } else {
        component.ground.speed.walk
    };

    // Input steering, scaled by `steer_control`. At 0 the term is omitted
    // entirely (committed dash); at 1 it is `Normal`'s full `pm_accelerate`.
    // Steering adds to the composite velocity (base-level authority); it does
    // not feed the tracked boost layer.
    let input_dir_3d = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if steer_control > 0.0 && input_dir_3d.length_squared() > 0.0 {
        let context_accel = if component.is_grounded {
            component.ground.accel
        } else {
            component.air.accel
        };
        pm_accelerate(
            &mut component.velocity,
            input_dir_3d,
            ground_speed,
            context_accel * steer_control,
            dt,
        );
    }

    // Reconcile the tracked boost with what collision actually realized before
    // splitting velocity into base/boost. Between ticks the substrate projects
    // `component.velocity` against geometry (collide-and-slide); driving the dash
    // into a wall zeroes the velocity component along the contact normal, but the
    // stored `boost` keeps its full pre-collision magnitude. Without this step
    // `base = velocity - boost` reconstructs a vector pointing OPPOSITE the dash
    // direction — a phantom backward kick away from the wall (head-on into the
    // x=5 wall: vx = -1.5 with base.x reconstructed as -15). Head-on self-corrects
    // in one tick, but a glancing clip (slope, step, angled wall) leaves the
    // phantom base alive across multiple dash ticks and breaks clean wall-slide.
    //
    // Fix: clamp the boost's magnitude along its OWN direction to the realized
    // horizontal velocity's projection on that axis (floored at 0, capped at the
    // tracked magnitude). When collision zeroes the boost axis the projection
    // drops to ~0, so the clamped boost shrinks to match and `base = velocity -
    // boost` can no longer point back out of the wall. An angled dash keeps its
    // surviving tangential velocity in `base`, yielding the same clean slide a
    // `Normal`-state approach would produce.
    let mut boost = boost;
    let boost_len = Vec2::new(boost.x, boost.z).length();
    if boost_len > 0.0 {
        let boost_dir = Vec3::new(boost.x / boost_len, 0.0, boost.z / boost_len);
        let realized_along_boost =
            (component.velocity.x * boost_dir.x + component.velocity.z * boost_dir.z).max(0.0);
        let clamped_len = boost_len.min(realized_along_boost);
        boost.x = boost_dir.x * clamped_len;
        boost.z = boost_dir.z * clamped_len;
    }

    // Decay. The base is the composite horizontal velocity minus the tracked
    // boost; only the boost is `dash_drag`-decayed, while the base always decays
    // under `Normal`'s contextual friction/cap so it never lingers above the
    // steady band.
    let mut base = Vec3::new(
        component.velocity.x - boost.x,
        0.0,
        component.velocity.z - boost.z,
    );
    apply_normal_horizontal_decay(&mut base, component, input, ground_speed, dt);

    if dash_drag <= 0.0 {
        // `dash_drag == 0`: the boost bleeds off as `Normal` momentum would —
        // fast on the ground, slow in air. On the ground, decay the boost toward
        // zero with ground friction *regardless of input*: `Normal`'s
        // stop-friction is no-input-only (because `pm_accelerate` caps grounded
        // speed), but the boost is deliberately above that cap, so a held stick
        // must not freeze it. Airborne, fold into `Normal`'s contextual cap.
        if component.is_grounded {
            let bspeed = Vec2::new(boost.x, boost.z).length();
            if bspeed > 0.0 {
                let drop = bspeed * GROUND_STOP_FRICTION * dt;
                let scale = (bspeed - drop).max(0.0) / bspeed;
                boost.x *= scale;
                boost.z *= scale;
            }
        } else {
            apply_normal_horizontal_decay(&mut boost, component, input, ground_speed, dt);
        }
    } else {
        // `dash_drag > 0`: constant LINEAR deceleration of the boost only
        // (world-units/sec², units consistent with `ground.accel`/`air.accel`),
        // decoupled from friction context. LINEAR, not exponential.
        let boost_speed = boost.length();
        if boost_speed > 0.0 {
            let new_speed = (boost_speed - dash_drag * dt).max(0.0);
            boost *= new_speed / boost_speed;
        }
    }

    component.velocity.x = base.x + boost.x;
    component.velocity.z = base.z + boost.z;

    // Exit: total horizontal speed back inside `Normal`'s steady band (run speed
    // grounded / air cap airborne) OR the `DASH_MAX_MS` elapsed guard.
    let elapsed_ms = elapsed_ms + dt * 1000.0;
    let horiz_speed = (component.velocity.x * component.velocity.x
        + component.velocity.z * component.velocity.z)
        .sqrt();
    // Steady band is `ground_speed` whether grounded or airborne: when `bunny_hop`
    // is off it matches `Normal`'s air cap; when on, `Normal` enforces no air cap,
    // so `ground_speed` is the band we choose to exit into rather than one `Normal`
    // maintains in that mode. Either way the dash is hard-bounded by `DASH_MAX_MS`.
    let steady_cap = ground_speed;
    if horiz_speed <= steady_cap || elapsed_ms >= DASH_MAX_MS {
        return Some(MovementState::Normal);
    }

    Some(MovementState::Dash { elapsed_ms, boost })
}

/// Apply `Normal`'s contextual horizontal decay to a horizontal velocity vector
/// in place: when grounded, the no-input stop-friction branch of `Normal` step 6
/// only; when airborne, the horizontal cap (mirroring steps 4/5). Step 6 has a
/// second grounded branch — the held-input over-speed bleed above
/// `OVERSPEED_BLEED_MARGIN` — which this helper deliberately omits: the vectors
/// `dash_intent` passes in (the retained base; the boost only when
/// `dash_drag == 0`) are already bounded below the run cap, so there is no
/// over-cap residue for that branch to act on. Reads the grounded flag and
/// friction params off the component.
fn apply_normal_horizontal_decay(
    velocity: &mut Vec3,
    component: &PlayerMovementComponent,
    input: &MovementInput,
    ground_speed: f32,
    dt: f32,
) {
    if component.is_grounded {
        if input.wish_dir.length_squared() < 0.001 {
            let h_speed = Vec2::new(velocity.x, velocity.z).length();
            if h_speed > 0.0 {
                let drop = h_speed * GROUND_STOP_FRICTION * dt;
                let new_speed = (h_speed - drop).max(0.0);
                let scale = new_speed / h_speed;
                velocity.x *= scale;
                velocity.z *= scale;
            }
        }
    } else if !component.air.bunny_hop {
        let h_speed = Vec2::new(velocity.x, velocity.z).length();
        if h_speed > ground_speed {
            let scale = ground_speed / h_speed;
            velocity.x *= scale;
            velocity.z *= scale;
        }
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

    // Per-state velocity intent: dispatch to the active state's intent step. It
    // authors `component.velocity` (gravity, jump, acceleration, friction, caps)
    // reading the grounded flag carried from last tick, and returns an optional
    // transition to apply after the substrate resolves collision.
    let transition = match component.movement_state {
        MovementState::Normal => normal_intent(component, input, gravity, dt, &mut events),
        MovementState::Dash { elapsed_ms, boost } => {
            dash_intent(component, input, gravity, dt, elapsed_ms, boost)
        }
    };

    // 7 + 8. Shared physics substrate: sweep-and-slide, step-up, floor-push,
    // ground-stick, and collision-state/landing resolution. States change
    // intent (steps 1–6 above), not collision.
    let (current_pos, substrate) = integrate_collision(
        component,
        collision_world,
        dt,
        position,
        was_grounded,
        events.jumped,
    );

    // Landing-refresh point: the ability-budget reset is a gameplay-state write
    // the substrate deliberately leaves to the tick. Driven by the substrate's
    // floor-contact result so every budget (air-jump charges, air-dash charges)
    // replenishes uniformly on every floor touch, through one method.
    if substrate.hit_floor {
        component.refresh_on_landing();
    }

    events.landed = substrate.landed;

    // Apply any state transition the intent returned, after the substrate has
    // resolved collision/landing. `Normal` transitions to `Dash` on rising-edge
    // dash input; `Dash` returns to `Normal` on speed-band exit or the
    // DASH_MAX_MS elapsed guard. Transition gating reads the same last-tick
    // grounded flag the intent used — the one-tick staleness is consistent with
    // how jump/air-jump already gate (no fresh ground probe).
    if let Some(next_state) = transition {
        component.movement_state = next_state;
    }

    // Decrement the dash cooldown UNCONDITIONALLY each tick, outside the
    // per-state intent dispatch, so it advances in every state (including
    // `Dash`) and never inside a state intent. Armed to `dash.cooldown_ms` on
    // dash entry; counts down off the same `dt` (seconds → ms via `dt * 1000`).
    if component.dash_cooldown_ms > 0.0 {
        component.dash_cooldown_ms = (component.dash_cooldown_ms - dt * 1000.0).max(0.0);
    }

    (current_pos, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, DashParams, FallParams, GroundParams, PlayerMovementDescriptor,
        SpeedParams,
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
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                },
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
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
        }
    }

    /// Descriptor with the air-jump (double-jump) budget enabled: one air
    /// charge and a finite upward-velocity ceiling. Mirrors the canonical
    /// descriptor otherwise so the double-jump tests vary only the air-jump
    /// fields under test. `jump_ceiling = 2.0` sits below the 5.5 jump launch
    /// velocity, so the charge cannot be spent at the top of the rising arc but
    /// fires once the arc has decayed past the ceiling (or while falling).
    fn double_jump_descriptor() -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.air.jumps = 1;
        desc.air.jump_ceiling = 2.0;
        desc
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
    /// Tests assert position envelopes and velocity caps rather than the
    /// per-tick `is_grounded` flag, which can briefly clear on tick
    /// boundaries before the ground-stick snap re-establishes contact.
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
            dash_pressed: false,
            running: false,
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
            dash_pressed: false,
            running: false,
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
        // The step-up probe is gated on wall-like normals (|ny| < cos_walkable),
        // so flat-floor walking does not trigger it and y stays near floor_y.
        // A small tolerance covers gravity's sub-millimeter settle each tick.
        let walk_y_min = floor_y - settle_tol;
        let walk_y_max = floor_y + settle_tol;
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
        // the active speed — walk speed here, since this phase does not sprint.
        // The bounded-from-below value depends on how many ticks were spent in
        // the ground accel branch versus airborne (gravity-only), so we assert
        // the cap, not a specific reached value.
        assert!(
            h_speed <= desc.ground.speed.walk + VEL_EPS,
            "horizontal speed should not exceed walk speed, got {}",
            h_speed
        );

        // ---- Phase 2: jump while continuing to walk forward. ----
        let jump = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: true,
            dash_pressed: false,
            running: false,
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
                assert!(
                    !comp.is_grounded,
                    "should be airborne immediately after jump"
                );
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
            pos.y >= ledge_y - settle_tol
                && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
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
        // Wall-unstick nudge is `NORMAL_NUDGE` (1e-4) — well under the skin
        // width — so a pinned player stays within one skin width of the wall.
        assert!(
            (pos.x - x_pinned).abs() < 0.03,
            "wall-pinned player x should stay within skin width: before={}, after={}",
            x_pinned,
            pos.x
        );
        // Wall slide: velocity component along +X (into the wall) is bled off
        // by the per-tick sweep-and-slide projection.
        assert!(
            comp.velocity.x.abs() < 0.1,
            "wall slide should not produce a large +X velocity, got vx={}",
            comp.velocity.x
        );
        // Y still rests near the ledge surface (allowing for the step-up jitter).
        assert!(
            pos.y >= ledge_y - settle_tol
                && pos.y <= ledge_y + desc.ground.step_height + settle_tol,
            "wall-pinned player y should be near ledge ({}), got y={}",
            ledge_y,
            pos.y
        );
    }

    /// Flat floor at y=0 spanning x,z ∈ [-20,20] with a vertical wall at x=5
    /// (y∈[0,5], z∈[-20,20]). Used to isolate wall-slide behavior from the
    /// step-up probe path.
    fn flat_floor_and_wall_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, 20.0));
        points.push(Point::new(-20.0, 0.0, 20.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        let w0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -20.0));
        points.push(Point::new(5.0, 0.0, 20.0));
        points.push(Point::new(5.0, 5.0, 20.0));
        points.push(Point::new(5.0, 5.0, -20.0));
        tris.push([w0, w0 + 1, w0 + 2]);
        tris.push([w0, w0 + 2, w0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    // Regression: step-up lift (step_height + 0.05) exceeded the ground-stick
    // down-cast range (step_height), so a wall-walking player oscillated between
    // lifted-and-airborne and snapped-to-floor states each tick — visible as
    // ~0.35 m camera jitter.
    #[test]
    fn wall_slide_does_not_bounce_y() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }
        assert!(
            pos.x > 13.5,
            "setup: player should reach ledge near wall, got x={}",
            pos.x
        );

        let mut y_samples = Vec::with_capacity(60);
        let mut grounded_after_settle = Vec::with_capacity(60);
        for i in 0..60 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            y_samples.push(pos.y);
            if i >= 5 {
                grounded_after_settle.push(comp.is_grounded);
            }
        }
        let y_min = y_samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let y_max = y_samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            y_max - y_min < 0.03,
            "wall-pinned y envelope should stay within skin width, got {} (min={}, max={})",
            y_max - y_min,
            y_min,
            y_max
        );
        let non_grounded = grounded_after_settle.iter().filter(|g| !*g).count();
        assert!(
            non_grounded <= 2,
            "is_grounded should be stable; got {} non-grounded ticks out of {}",
            non_grounded,
            grounded_after_settle.len()
        );
    }

    #[test]
    fn walking_along_wall_keeps_horizontal_speed() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }

        let z_start = pos.z;
        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let z_advance = (pos.z - z_start).abs();
        let min_z_advance = desc.ground.speed.walk * (120.0 / 60.0) * 0.5;
        assert!(
            z_advance >= min_z_advance,
            "tangential -Z advance should be >= {}, got {}",
            min_z_advance,
            z_advance
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > desc.ground.speed.walk * 0.4,
            "horizontal speed after 120 ticks should exceed {}, got {}",
            desc.ground.speed.walk * 0.4,
            h_speed
        );
    }

    #[test]
    fn walking_into_wall_y_stable_per_tick() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let ledge_y = 0.3 + desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..300 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 14.0 && pos.y > ledge_y - 0.05 {
                break;
            }
        }

        let wall_contact_x = 15.0 - desc.capsule.radius - 0.05;
        let mut prev_y = pos.y;
        let mut in_contact = false;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= wall_contact_x {
                in_contact = true;
            }
            if in_contact {
                let dy = (pos.y - prev_y).abs();
                assert!(
                    dy < 0.03,
                    "per-tick |dy| should stay within skin width after wall contact, got {} (prev_y={}, y={})",
                    dy,
                    prev_y,
                    pos.y
                );
            }
            prev_y = pos.y;
        }
        assert!(in_contact, "test setup: should have reached wall contact");
    }

    // Regression: step-up probe lifted ~0.35 m every tick when walking into a
    // pure wall (no walkable surface above). Ground-stick snapped back within
    // the tick, but the intra-tick excursion produced visible camera jitter.
    // Direct unit test: pure wall → None.
    #[test]
    fn step_up_lift_returns_none_at_pure_wall() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let capsule = Capsule::new(
            Point::new(0.0, -desc.capsule.half_height, 0.0),
            Point::new(0.0, desc.capsule.half_height, 0.0),
            desc.capsule.radius,
        );
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let floor_y = desc.capsule.half_height + desc.capsule.radius;
        // Position the capsule just shy of the wall (wall at x=5) so the
        // forward probe hits it on the first cast. Lifted slightly above the
        // floor so the forward probe finds the wall (not the floor contact).
        let current_pos = Vec3::new(5.0 - desc.capsule.radius - 0.05, floor_y + 0.05, 0.0);
        let horiz_vel = Vec3::new(desc.ground.speed.walk, 0.0, 0.0);
        let horiz_speed = horiz_vel.length();

        let result = step_up_lift(
            &world,
            &capsule,
            current_pos,
            horiz_vel,
            horiz_speed,
            desc.ground.step_height,
            cos_walkable,
            DT,
            desc.capsule.radius,
        );
        assert!(
            result.is_none(),
            "step_up_lift should return None at a pure wall, got {:?}",
            result
        );
    }

    // Direct unit test: walkable step → Some(lifted) at step_height + 0.05.
    #[test]
    fn step_up_lift_returns_some_at_walkable_step() {
        let desc = canonical_descriptor();
        let world = ledge_and_wall_world();
        let capsule = Capsule::new(
            Point::new(0.0, -desc.capsule.half_height, 0.0),
            Point::new(0.0, desc.capsule.half_height, 0.0),
            desc.capsule.radius,
        );
        let cos_walkable = desc.ground.max_slope.to_radians().cos();
        let floor_y = desc.capsule.half_height + desc.capsule.radius;
        // Approach the step riser at x=5 from the floor side. Lift the
        // capsule slightly above the floor so the forward probe doesn't
        // return the floor contact (toi=0, normal +Y) before reaching the
        // riser.
        let current_pos = Vec3::new(5.0 - desc.capsule.radius - 0.05, floor_y + 0.05, 0.0);
        let horiz_vel = Vec3::new(desc.ground.speed.walk, 0.0, 0.0);
        let horiz_speed = horiz_vel.length();

        let result = step_up_lift(
            &world,
            &capsule,
            current_pos,
            horiz_vel,
            horiz_speed,
            desc.ground.step_height,
            cos_walkable,
            DT,
            desc.capsule.radius,
        );
        let lifted = result.expect("step_up_lift should return Some at a walkable step");
        let expected_y = current_pos.y + desc.ground.step_height + 0.05;
        assert!(
            approx_eq(lifted.y, expected_y, POS_EPS),
            "lifted y should be {} (current + step_height + 0.05), got {}",
            expected_y,
            lifted.y
        );
        assert!(
            approx_eq(lifted.x, current_pos.x, POS_EPS)
                && approx_eq(lifted.z, current_pos.z, POS_EPS),
            "lift should preserve horizontal position, got {:?}",
            lifted
        );
    }

    // Regression: walking into a pure wall produced visible vertical camera
    // jitter even though tick-boundary `pos.y` snapped back via ground-stick.
    #[test]
    fn walking_into_wall_y_stays_at_floor() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let floor_y = desc.capsule.half_height + desc.capsule.radius;

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);
        let settle_y = pos.y;
        assert!(
            (settle_y - floor_y).abs() < 0.02,
            "test setup: should settle near floor_y={}, got {}",
            floor_y,
            settle_y
        );

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let wall_contact_x = 5.0 - desc.capsule.radius - 0.05;
        let mut in_contact = false;
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= wall_contact_x {
                in_contact = true;
            }
            if in_contact {
                // Compare to settle_y, not floor_y: cast_capsule's 0.02 skin
                // means the resting capsule sits ~0.02 above geometric floor_y.
                assert!(
                    (pos.y - settle_y).abs() < 0.01,
                    "wall-walking y should stay within 0.01 of settle_y={}, got {}",
                    settle_y,
                    pos.y
                );
            }
        }
        assert!(in_contact, "test setup: should have reached wall contact");
    }

    // Regression: the floor TOI=0 branch in the slide loop unconditionally
    // pushed the player up by 0.025 m per iteration (up to 4× per tick) when
    // a grounded capsule walked into a flat wall. Ground-stick snapped back
    // most ticks but at a wall/floor inside corner the downcast could pick
    // the wall normal first and silently latch the player above the floor.
    // Tight envelope across the last 30 ticks catches the orbital pump that
    // the looser 0.01-bounded test missed.
    #[test]
    fn walking_into_wall_no_orbital_jitter() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        // Run 120 ticks; the player reaches the wall well within the first
        // 60 (7 m at canonical 7 m/s ≈ 60 ticks of accel + wall approach).
        // Sample the last 30 ticks so all samples come from the
        // post-stabilisation wall-pinned regime.
        let mut ys = Vec::with_capacity(120);
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
            ys.push(pos.y);
        }
        let tail_y = &ys[90..];
        let y_min = tail_y.iter().cloned().fold(f32::INFINITY, f32::min);
        let y_max = tail_y.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        // 3e-3 m tolerance bounds the vertical wobble the floor_push budget
        // (skin + nudge ≈ 0.02 m, applied at most once per tick and snapped
        // back by ground-stick) can leak. The pre-fix code's orbital pump
        // produced 0.025–0.1 m vertical jitter — an order of magnitude past
        // this bound. Horizontal drift is not asserted here: floor triangle
        // normals tilt a few thousandths off pure +Y, so projection of
        // tangential motion introduces a slow sub-millimetre-per-tick
        // horizontal creep that accumulates over many ticks but does not
        // reflect the orbital-pump bug.
        let envelope = 3.0e-3;
        assert!(
            y_max - y_min < envelope,
            "wall-pinned y envelope across last 30 ticks should be < {} m, got {} (min={}, max={})",
            envelope,
            y_max - y_min,
            y_min,
            y_max
        );
    }

    // Regression: capsule pressed against a wall produced TOI=0 every sweep
    // iteration, burning all 4 slots without advancing — player froze instead
    // of sliding tangentially along the wall.
    #[test]
    fn player_slides_along_wall_when_approaching_diagonally() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };

        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05 {
                break;
            }
        }
        let wall_limit = 5.0 - desc.capsule.radius + POS_EPS + 1e-3;
        assert!(
            pos.x <= wall_limit,
            "diagonal approach should not penetrate the wall: pos.x={}, limit={}",
            pos.x,
            wall_limit
        );
        assert!(
            pos.x > 5.0 - desc.capsule.radius - 0.1,
            "player should have reached the wall: pos.x={}",
            pos.x
        );

        let z_before = pos.z;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        // facing_yaw=0 makes forward=-Z, so diagonal input (right+forward)
        // produces (+X, 0, -Z) motion. Wall projects out +X; -Z slide remains.
        assert!(
            pos.z < z_before - 0.5,
            "player should slide along -Z while pinned to the wall: z_before={}, z_after={}",
            z_before,
            pos.z
        );
    }

    /// Two perpendicular walls forming an interior corner at (x=5, z=5),
    /// floor below at y=0. The east wall (x=5, y∈[0,5], z∈[-20,5]) and the
    /// north wall (x∈[-20,5], y∈[0,5], z=5) meet at a 90° interior corner so
    /// a player driven into the corner experiences both wall normals (-X, -Z)
    /// in the same tick — the geometric setup the deadzone targets.
    fn corner_world() -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor.
        let f0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, -20.0));
        points.push(Point::new(20.0, 0.0, 20.0));
        points.push(Point::new(-20.0, 0.0, 20.0));
        tris.push([f0, f0 + 1, f0 + 2]);
        tris.push([f0, f0 + 2, f0 + 3]);

        // East wall at x=5 facing -X.
        let e0 = points.len() as u32;
        points.push(Point::new(5.0, 0.0, -20.0));
        points.push(Point::new(5.0, 0.0, 5.0));
        points.push(Point::new(5.0, 5.0, 5.0));
        points.push(Point::new(5.0, 5.0, -20.0));
        tris.push([e0, e0 + 1, e0 + 2]);
        tris.push([e0, e0 + 2, e0 + 3]);

        // North wall at z=5 facing -Z.
        let n0 = points.len() as u32;
        points.push(Point::new(-20.0, 0.0, 5.0));
        points.push(Point::new(-20.0, 5.0, 5.0));
        points.push(Point::new(5.0, 5.0, 5.0));
        points.push(Point::new(5.0, 0.0, 5.0));
        tris.push([n0, n0 + 1, n0 + 2]);
        tris.push([n0, n0 + 2, n0 + 3]);

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Drive the player diagonally toward the corner at (x=5, z=5) until the
    /// capsule is firmly wedged, then return the last 10 tick-boundary (x,z)
    /// samples. Used by the deadzone tests below.
    fn wedge_player_in_corner(
        desc: &PlayerMovementDescriptor,
        comp: &mut PlayerMovementComponent,
        pos: &mut Vec3,
        world: &CollisionWorld,
    ) -> Vec<(f32, f32)> {
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(comp, world, pos, 10, &idle);

        // facing_yaw=0 ⇒ forward=-Z, so wish_dir=(1,-1).norm() gives input
        // (+X, 0, +Z): straight at the +X / +Z corner.
        let toward_corner = MovementInput {
            wish_dir: Vec2::new(1.0, -1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        // Approach and reach the corner.
        for _ in 0..240 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05
                && pos.z >= 5.0 - desc.capsule.radius - 0.05
            {
                break;
            }
        }
        // Press into the corner for 50 ticks so any pinned pattern stabilises.
        for _ in 0..50 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
        }
        // Sample the last 10 tick-boundary positions.
        let mut samples = Vec::with_capacity(10);
        for _ in 0..10 {
            let (next, _ev) = tick(comp, &toward_corner, world, GRAVITY, DT, *pos);
            *pos = next;
            samples.push((pos.x, pos.z));
        }
        samples
    }

    #[test]
    fn wedging_into_corner_zeros_horizontal_velocity_when_deadzone_enabled() {
        let desc = canonical_descriptor();
        let world = corner_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled, "deadzone is on by default");

        let samples = wedge_player_in_corner(&desc, &mut comp, &mut pos, &world);
        let xs: Vec<f32> = samples.iter().map(|(x, _)| *x).collect();
        let zs: Vec<f32> = samples.iter().map(|(_, z)| *z).collect();
        let x_range = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let z_range = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - zs.iter().cloned().fold(f32::INFINITY, f32::min);

        // With the deadzone on, the corner-wedge detector zeroes horizontal
        // velocity and rolls back XZ when contradictory wall normals appear,
        // so the player's XZ position is flat across consecutive ticks.
        let eps = 5.0e-4;
        assert!(
            x_range < eps,
            "deadzone enabled: x range across last 10 ticks should be < {} m, got {}",
            eps,
            x_range
        );
        assert!(
            z_range < eps,
            "deadzone enabled: z range across last 10 ticks should be < {} m, got {}",
            eps,
            z_range
        );
    }

    #[test]
    fn wedging_into_corner_keeps_motion_when_deadzone_disabled() {
        let desc = canonical_descriptor();
        let world = corner_world();
        let (mut comp, mut pos) = settle_player(&desc);
        // Disable the deadzone so the slide loop's wall projections govern
        // the final wedge XZ trajectory without the velocity zero-out.
        comp.stuck_stop_enabled = false;

        let samples = wedge_player_in_corner(&desc, &mut comp, &mut pos, &world);
        let xs: Vec<f32> = samples.iter().map(|(x, _)| *x).collect();
        let zs: Vec<f32> = samples.iter().map(|(_, z)| *z).collect();
        let x_range = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let z_range = zs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - zs.iter().cloned().fold(f32::INFINITY, f32::min);

        // Without the deadzone the player is not snapped to a frozen XZ —
        // velocity is still alive, projection just bleeds horizontal
        // components against both walls. Per-tick XZ should still be small
        // (the wedge is stable) but not exactly zero across consecutive
        // ticks the way the deadzone produces. Loose bound (> 0) is enough
        // to prove the flag gates the velocity zero-out — the
        // deadzone-enabled test asserts the much tighter bound.
        assert!(
            comp.velocity.x.abs() + comp.velocity.z.abs() < 1.0,
            "deadzone disabled wedge should still come to near-rest, got vx={} vz={}",
            comp.velocity.x,
            comp.velocity.z
        );
        // Sanity: with the deadzone OFF, the explicit XZ zero-out branch in
        // `tick()` did not fire, so the player retains whatever the slide
        // loop's natural projection leaves. Document via assertion that the
        // XZ wobble is observably non-negative (a no-op assertion that
        // makes the test's purpose explicit).
        assert!(x_range >= 0.0 && z_range >= 0.0);
    }

    #[test]
    fn sliding_along_wall_diagonally_not_affected_by_deadzone() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 5, &idle);

        let diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0).normalize(),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
            if pos.x >= 5.0 - desc.capsule.radius - 0.05 {
                break;
            }
        }
        // facing_yaw=0 ⇒ forward=-Z; diagonal input (right+forward) gives
        // (+X, 0, -Z). Wall projects out +X but -Z slide must remain.
        let z_before = pos.z;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &diag, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let z_advance = z_before - pos.z;
        assert!(
            z_advance > 1.0,
            "diagonal wall slide should still produce tangential -Z motion with deadzone on: z_before={}, z_after={}, advance={}",
            z_before,
            pos.z,
            z_advance
        );
    }

    #[test]
    fn walking_along_flat_floor_not_affected_by_deadzone() {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        assert!(comp.stuck_stop_enabled);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let x_start = pos.x;
        // 30 ticks (0.5 s) at 7 m/s ⇒ comfortably >1.5 m on open floor.
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &walk, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let advance = pos.x - x_start;
        assert!(
            advance > 1.5,
            "flat-floor walk should advance > 1.5 m in 30 ticks, got {}",
            advance
        );
        let h_speed = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_speed > desc.ground.speed.walk * 0.8,
            "flat-floor walk should keep h_speed near ground.speed={}, got {}",
            desc.ground.speed.walk,
            h_speed
        );
    }

    /// Steady-state horizontal speed on flat ground after enough ticks to
    /// reach the projection cap, given a fixed input direction.
    fn steady_state_ground_speed(running: bool) -> f32 {
        let desc = canonical_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Walk along -Z (away from the wall at x=5) so the move never contacts
        // geometry and the projection cap is the only speed limiter. 60 ticks
        // (~11 m at run speed) reaches the cap while staying on the floor
        // (z ∈ [-20, 20]).
        let mv = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 60, &mv);
        (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt()
    }

    // Sprint is omnidirectional and selects `ground.speed.run`: steady-state
    // horizontal speed while running must reach the run cap and exceed the
    // walk steady-state.
    #[test]
    fn running_reaches_higher_steady_state_than_walking() {
        let desc = canonical_descriptor();
        let walk_speed = steady_state_ground_speed(false);
        let run_speed = steady_state_ground_speed(true);

        assert!(
            approx_eq(walk_speed, desc.ground.speed.walk, 0.05),
            "walk steady-state should reach walk cap {}, got {}",
            desc.ground.speed.walk,
            walk_speed
        );
        assert!(
            approx_eq(run_speed, desc.ground.speed.run, 0.05),
            "run steady-state should reach run cap {}, got {}",
            desc.ground.speed.run,
            run_speed
        );
        assert!(
            run_speed > walk_speed + 1.0,
            "running ({run_speed}) should be meaningfully faster than walking ({walk_speed})"
        );
    }

    // Regression: the airborne horizontal speed cap used `ground.speed` (now
    // `.walk` after the rename). Sprinting then jumping must not instantly
    // decelerate mid-air — the cap honors run speed while running is held.
    #[test]
    fn airborne_cap_honors_run_speed_while_sprinting() {
        let desc = canonical_descriptor();
        // bunny_hop must be off for the air cap to apply at all.
        assert!(!desc.air.bunny_hop, "canonical descriptor has air cap on");
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);

        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Build up to the run cap on the ground (heading -Z, away from the
        // wall). Keep the buildup short enough that the player stays on the
        // floor (floor spans z ∈ [-20, 20]; ~30 ticks at 11 m/s ≈ 5.5 m).
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 30, &run);
        let h_ground = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_ground > desc.ground.speed.walk + 0.5,
            "setup: ground run speed should exceed walk cap, got {h_ground}"
        );

        // Jump while still sprinting, then run several airborne ticks. The cap
        // is the run speed, so horizontal speed must stay above the walk cap —
        // a walk-capped airborne path would bleed it down to ~7. Flat-floor
        // walking can clear `is_grounded` for a tick, so find a grounded tick
        // before jumping (mirrors the walk/jump/step integration test).
        let run_jump = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: true,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        let mut jumped = false;
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(&mut comp, &run_jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "grounded + jump should emit jumped");
                jumped = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(jumped, "setup: should have jumped from a grounded tick");

        // A few airborne ticks with sprint held; jump_pressed released so we
        // don't re-trigger (air.jumps = 0 anyway).
        for _ in 0..5 {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
        }
        let h_air = (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt();
        assert!(
            h_air > desc.ground.speed.walk + 0.5,
            "airborne sprint speed should stay above the walk cap (run cap honored), got {h_air}"
        );
        assert!(
            h_air <= desc.ground.speed.run + VEL_EPS,
            "airborne sprint speed should not exceed the run cap {}, got {}",
            desc.ground.speed.run,
            h_air
        );
    }

    /// Drive the player off a grounded tick into a single jump and return once
    /// airborne. Mirrors the grounded-tick search the other jump tests use so
    /// flat-floor `is_grounded` blips don't desync the launch.
    fn ground_jump_into_air(
        comp: &mut PlayerMovementComponent,
        world: &CollisionWorld,
        pos: &mut Vec3,
    ) {
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..60 {
            if comp.is_grounded {
                let (next, ev) = tick(comp, &jump, world, GRAVITY, DT, *pos);
                *pos = next;
                assert!(ev.jumped, "grounded + jump_pressed should emit jumped");
                return;
            }
            let (next, _ev) = tick(comp, &idle, world, GRAVITY, DT, *pos);
            *pos = next;
        }
        panic!("expected a grounded tick within 60 attempts");
    }

    // Double-jump fires while airborne once the rising arc has decayed past the
    // ceiling, consuming one air charge. Proves the named air-jump ability under
    // the budget model: a second jump airborne, gated by `air.jump_ceiling`.
    #[test]
    fn air_jump_fires_second_jump_while_airborne_under_ceiling() {
        let desc = double_jump_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "budget should start full on the ground"
        );

        // First jump from the ground. Launch vy is 5.5, well above the 2.0
        // ceiling, so the air-jump must NOT fire on the launch tick.
        ground_jump_into_air(&mut comp, &world, &mut pos);
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "air-jump budget untouched by the ground jump"
        );

        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        // While vy is still above the ceiling, holding jump must not consume a
        // charge (the ceiling gate blocks it).
        let blocked = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        pos = blocked.0;
        assert!(
            comp.velocity.y > desc.air.jump_ceiling,
            "setup: vy should still be above the ceiling one tick after launch, got {}",
            comp.velocity.y
        );
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "air-jump must not fire while vy is above the ceiling"
        );

        // Let gravity decay vy under the ceiling, holding jump released so the
        // charge isn't spent the instant the ceiling is crossed.
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(
            comp.velocity.y <= desc.air.jump_ceiling,
            "setup: vy should have decayed under the ceiling, got {}",
            comp.velocity.y
        );
        assert!(!comp.is_grounded, "setup: player must still be airborne");

        // Now the air-jump fires: a charge is consumed and vy relaunches to the
        // jump velocity.
        let (_next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
        assert!(ev.jumped, "air-jump under the ceiling should emit jumped");
        assert_eq!(
            comp.air_jumps_remaining,
            desc.air.jumps - 1,
            "air-jump should consume exactly one charge"
        );
        assert!(
            approx_eq(comp.velocity.y, desc.ground.jump_velocity, VEL_EPS),
            "air-jump should relaunch vy to jump_velocity={}, got {}",
            desc.ground.jump_velocity,
            comp.velocity.y
        );
    }

    // The air-jump budget replenishes on landing through `refresh_on_landing`:
    // after spending the charge airborne, returning to the floor restores it.
    #[test]
    fn air_jump_budget_restored_on_landing() {
        let desc = double_jump_descriptor();
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        // Spend the air charge directly while airborne: jump, drop vy under the
        // ceiling, then air-jump.
        ground_jump_into_air(&mut comp, &world, &mut pos);
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                let (next, _ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert_eq!(
            comp.air_jumps_remaining, 0,
            "setup: the single air charge should be spent"
        );

        // Fall and land; the landing-refresh point restores the budget.
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
            if comp.is_grounded {
                break;
            }
        }
        assert!(comp.is_grounded, "setup: player should have landed");
        assert_eq!(
            comp.air_jumps_remaining, desc.air.jumps,
            "landing should restore the air-jump budget via refresh_on_landing"
        );
    }

    // Budget exhaustion: with a one-charge budget, a second airborne jump cannot
    // fire until landing replenishes it. Proves the air-jump count gates repeated
    // airborne jumps within one airborne window.
    #[test]
    fn air_jump_exhausts_after_configured_count() {
        let desc = double_jump_descriptor();
        assert_eq!(desc.air.jumps, 1, "fixture uses a one-charge budget");
        let world = flat_floor_and_wall_world();
        let (mut comp, mut pos) = settle_player(&desc);
        let idle = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, &world, &mut pos, 10, &idle);

        ground_jump_into_air(&mut comp, &world, &mut pos);
        let jump = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };

        // Spend the only charge once vy is under the ceiling.
        let mut spent = false;
        for _ in 0..60 {
            if comp.velocity.y <= desc.air.jump_ceiling {
                let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
                pos = next;
                assert!(ev.jumped, "first air-jump should fire");
                spent = true;
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle, &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(spent, "setup: should have spent the air charge");
        assert_eq!(comp.air_jumps_remaining, 0, "budget exhausted");

        // Keep holding jump while airborne and under the ceiling: with the
        // budget at zero, no further air-jump may fire until landing.
        for _ in 0..30 {
            if comp.is_grounded {
                break;
            }
            let vy_before = comp.velocity.y;
            let (next, ev) = tick(&mut comp, &jump, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                !ev.jumped,
                "no air-jump should fire with an exhausted budget while airborne"
            );
            assert_eq!(
                comp.air_jumps_remaining, 0,
                "budget must stay exhausted until landing"
            );
            // Vy keeps decaying under gravity — it is not relaunched.
            assert!(
                comp.velocity.y < vy_before + VEL_EPS,
                "vy should not relaunch with an exhausted budget: before={}, after={}",
                vy_before,
                comp.velocity.y
            );
        }
    }

    // ---- Dash (Task 4) ----------------------------------------------------

    /// Build a `DashParams` with the three orthogonal knobs explicit so each
    /// dash test can place itself on the rigid↔fluid spectrum (D3).
    fn dash_params(
        boost_speed: f32,
        momentum_retention: f32,
        steer_control: f32,
        dash_drag: f32,
        cooldown_ms: f32,
        air_dashes: u32,
        preserve_vertical: bool,
    ) -> DashParams {
        DashParams {
            boost_speed,
            momentum_retention,
            steer_control,
            dash_drag,
            cooldown_ms,
            air_dashes,
            preserve_vertical,
        }
    }

    /// Canonical descriptor with a dash configured. Defaults to a committed,
    /// rigid dash (no steer, no retention, finite drag); tests override the
    /// `dash` field for the corner they exercise.
    fn dash_descriptor(dash: DashParams) -> PlayerMovementDescriptor {
        let mut desc = canonical_descriptor();
        desc.dash = Some(dash);
        desc
    }

    fn idle_input() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        }
    }

    fn horiz_speed(comp: &PlayerMovementComponent) -> f32 {
        (comp.velocity.x.powi(2) + comp.velocity.z.powi(2)).sqrt()
    }

    /// Settle the player on flat ground, then build up run speed along -Z (away
    /// from the wall at x=5) so dash tests start from a known grounded velocity.
    fn settle_and_run(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
        run_ticks_n: usize,
    ) -> (PlayerMovementComponent, Vec3) {
        let (mut comp, mut pos) = settle_player(desc);
        run_ticks(&mut comp, world, &mut pos, 10, &idle_input());
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        run_ticks(&mut comp, world, &mut pos, run_ticks_n, &run);
        (comp, pos)
    }

    // Fluid corner: momentumRetention=1, dashDrag=0. A dash in the direction of
    // travel while already running stacks (peak exceeds a standing dash), then
    // decays through Normal's ground friction back into the run-speed band, at
    // which point control returns to Normal — before the DASH_MAX_MS guard.
    #[test]
    fn dash_fluid_corner_stacks_then_decays_into_band() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 0.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run; // 11.0

        // Reference: a standing dash (no prior velocity) peaks at boost_speed.
        let (mut standing, mut spos) = settle_player(&desc);
        run_ticks(&mut standing, &world, &mut spos, 10, &idle_input());
        let dash_in_place = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let _ = tick(&mut standing, &dash_in_place, &world, GRAVITY, DT, spos);
        let standing_peak = horiz_speed(&standing);

        // Running dash: build up to the run cap, then dash forward.
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let pre_dash = horiz_speed(&comp);
        assert!(
            pre_dash > desc.ground.speed.walk,
            "setup: should be running above walk speed, got {pre_dash}"
        );
        let (next, _ev) = tick(&mut comp, &dash_in_place, &world, GRAVITY, DT, pos);
        pos = next;
        let running_peak = horiz_speed(&comp);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "should have entered Dash"
        );
        assert!(
            running_peak > standing_peak + 1.0,
            "running dash should stack over a standing dash: running={running_peak}, standing={standing_peak}"
        );

        // Release directional input; ground friction bleeds the dash back into
        // the run-speed band, at which point control returns to Normal.
        let mut returned = false;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                returned = true;
                break;
            }
        }
        assert!(returned, "dash should decay back into Normal via friction");
        assert!(
            horiz_speed(&comp) <= run_cap + VEL_EPS,
            "post-dash speed should be within the run band, got {}",
            horiz_speed(&comp)
        );
    }

    // Regression: a grounded dash held in the move direction must decay back to
    // the run cap even while the stick stays down. Ground friction used to be
    // no-input-only (it relies on `pm_accelerate` to cap grounded speed), so a
    // held-input dash — which is deliberately above the cap — stayed locked at
    // boost speed indefinitely until the player released the button.
    #[test]
    fn dash_held_input_decays_back_to_run_speed_on_ground() {
        let world = flat_floor_and_wall_world();
        // dashDrag = 0 (decay through Normal friction), momentumRetention = 0.5,
        // steerControl = 0.3 — the dev-player tuning that exposed the bug.
        let desc = dash_descriptor(dash_params(22.0, 0.5, 0.3, 0.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run;

        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_held = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_held, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > run_cap + 1.0,
            "dash should briefly exceed the run cap, got {peak}"
        );

        // Keep holding the SAME direction. Only the first tick was the dash
        // rising edge, so `dash_pressed` is false hereafter (no re-entry).
        let hold = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        // Keep holding the direction and confirm the speed bleeds back to the run
        // cap *while still grounded* — the actual bug was a held-input grounded
        // dash never decaying. Break as soon as it reaches the band on the ground;
        // asserting the grounded state rules out the speed merely being clamped by
        // the airborne cap if the player later runs off the floor edge.
        let mut grounded_in_band = false;
        for _ in 0..120 {
            let (next, _ev) = tick(&mut comp, &hold, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal)
                && comp.is_grounded
                && horiz_speed(&comp) <= run_cap + 0.05
            {
                grounded_in_band = true;
                break;
            }
        }
        assert!(
            grounded_in_band,
            "held-input dash never bled back to the run cap on the ground; final speed {:.3}, grounded {}",
            horiz_speed(&comp),
            comp.is_grounded,
        );
        // It settles at the cap, not below — the stick is still held.
        assert!(
            horiz_speed(&comp) > run_cap - 1.0,
            "held-input dash should hold the run cap, not slow further: {}",
            horiz_speed(&comp)
        );
    }

    // Rigid corner: momentumRetention=0, steerControl=0, dashDrag>0. The dash
    // outcome (peak speed and the linear dash_drag decay curve) is identical
    // regardless of entry velocity — bit-exact repeatability.
    #[test]
    fn dash_rigid_corner_identical_regardless_of_entry_velocity() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 30.0, 0.0, 0, false));
        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };

        // Capture the dash speed curve from a standing entry.
        let (mut standing, mut spos) = settle_player(&desc);
        run_ticks(&mut standing, &world, &mut spos, 10, &idle_input());
        let mut standing_curve = Vec::new();
        let (next, _ev) = tick(&mut standing, &dash, &world, GRAVITY, DT, spos);
        spos = next;
        standing_curve.push(horiz_speed(&standing));
        for _ in 0..10 {
            let (next, _ev) = tick(&mut standing, &idle_input(), &world, GRAVITY, DT, spos);
            spos = next;
            standing_curve.push(horiz_speed(&standing));
        }

        // Capture the same curve from a fast running entry.
        let (mut running, mut rpos) = settle_and_run(&desc, &world, 60);
        let mut running_curve = Vec::new();
        let (next, _ev) = tick(&mut running, &dash, &world, GRAVITY, DT, rpos);
        rpos = next;
        running_curve.push(horiz_speed(&running));
        for _ in 0..10 {
            let (next, _ev) = tick(&mut running, &idle_input(), &world, GRAVITY, DT, rpos);
            rpos = next;
            running_curve.push(horiz_speed(&running));
        }

        // momentumRetention=0 ⇒ entry velocity is fully replaced; the dash speed
        // curve is the same regardless of entry state. The two runs differ only
        // by sub-millimetre collision-position float noise (each run is at a
        // different world position), so compare within a tight epsilon rather
        // than bit-for-bit.
        for (i, (a, b)) in standing_curve.iter().zip(running_curve.iter()).enumerate() {
            assert!(
                approx_eq(*a, *b, 1.0e-2),
                "rigid dash speed at step {i} must match regardless of entry: standing={a}, running={b}"
            );
        }
    }

    // steerControl: at 0 input does not alter the dash trajectory mid-dash
    // (committed); at >0 input steers it. One test capturing the contrast.
    #[test]
    fn dash_steer_control_committed_vs_steerable() {
        let world = flat_floor_and_wall_world();
        // Dash forward (-Z) from a standing start; mid-dash hold a +X steer.
        let steer_input = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let dash_forward = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };

        // Committed: steer_control = 0. Mid-dash +X input must not add +X
        // velocity.
        let committed_desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 20.0, 0.0, 0, false));
        let (mut c, mut cpos) = settle_player(&committed_desc);
        run_ticks(&mut c, &world, &mut cpos, 10, &idle_input());
        let (next, _ev) = tick(&mut c, &dash_forward, &world, GRAVITY, DT, cpos);
        cpos = next;
        for _ in 0..4 {
            let (next, _ev) = tick(&mut c, &steer_input, &world, GRAVITY, DT, cpos);
            cpos = next;
        }
        let committed_vx = c.velocity.x;

        // Steerable: steer_control = 1. The same mid-dash +X input adds +X
        // velocity.
        let steer_desc = dash_descriptor(dash_params(15.0, 0.0, 1.0, 20.0, 0.0, 0, false));
        let (mut s, mut spos) = settle_player(&steer_desc);
        run_ticks(&mut s, &world, &mut spos, 10, &idle_input());
        let (next, _ev) = tick(&mut s, &dash_forward, &world, GRAVITY, DT, spos);
        spos = next;
        for _ in 0..4 {
            let (next, _ev) = tick(&mut s, &steer_input, &world, GRAVITY, DT, spos);
            spos = next;
        }
        let steered_vx = s.velocity.x;

        assert!(
            committed_vx.abs() < VEL_EPS,
            "committed dash (steer_control=0) must not gain +X from mid-dash input, got vx={committed_vx}"
        );
        assert!(
            steered_vx > 0.5,
            "steerable dash (steer_control=1) should gain +X from mid-dash input, got vx={steered_vx}"
        );
    }

    // momentumRetention: at 0 the dash replaces prior horizontal velocity
    // (input+facing held constant ⇒ outcome independent of entry velocity); at 1
    // it adds to prior horizontal velocity.
    #[test]
    fn dash_momentum_retention_replace_vs_additive() {
        let world = flat_floor_and_wall_world();
        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };

        // Retention 0: a standing dash and a running dash reach the same peak
        // (prior velocity replaced).
        let replace_desc = dash_descriptor(dash_params(8.0, 0.0, 0.0, 20.0, 0.0, 0, false));
        let (mut standing, mut sp) = settle_player(&replace_desc);
        run_ticks(&mut standing, &world, &mut sp, 10, &idle_input());
        let (_next, _ev) = tick(&mut standing, &dash, &world, GRAVITY, DT, sp);
        let replace_standing = horiz_speed(&standing);

        let (mut running, rp) = settle_and_run(&replace_desc, &world, 60);
        let (_next, _ev) = tick(&mut running, &dash, &world, GRAVITY, DT, rp);
        let replace_running = horiz_speed(&running);
        assert!(
            approx_eq(replace_standing, replace_running, VEL_EPS),
            "retention=0: dash peak must be independent of entry velocity, standing={replace_standing}, running={replace_running}"
        );
        assert!(
            approx_eq(replace_standing, 8.0, VEL_EPS),
            "retention=0 standing dash peak should equal boost_speed=8.0, got {replace_standing}"
        );

        // Retention 1: a running dash adds the boost on top of prior velocity.
        let add_desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 20.0, 0.0, 0, false));
        let (mut running2, mut rp2) = settle_and_run(&add_desc, &world, 60);
        let pre = horiz_speed(&running2);
        let (next, _ev) = tick(&mut running2, &dash, &world, GRAVITY, DT, rp2);
        rp2 = next;
        let _ = rp2;
        let add_running = horiz_speed(&running2);
        assert!(
            add_running > pre + 4.0,
            "retention=1: dash should add to prior velocity (pre={pre}, peak={add_running})"
        );
    }

    // Layered decay (D4): momentumRetention=1, dashDrag>0. Only the boost decays
    // at the dash_drag rate while the retained base continues under Normal's
    // friction; post-dash steady speed settles into Normal's band (the dash_drag
    // bleed does NOT drag the retained base below it).
    #[test]
    fn dash_layered_decay_base_survives_boost_drag() {
        let world = flat_floor_and_wall_world();
        // High drag bleeds the boost fast; retention=1 keeps the run-speed base.
        let desc = dash_descriptor(dash_params(8.0, 1.0, 0.0, 60.0, 0.0, 0, false));
        let run_cap = desc.ground.speed.run;

        // Build up to the run cap, then dash forward and KEEP running forward so
        // the base is sustained by ground accel (no friction while input held).
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_run, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > run_cap + 4.0,
            "setup: dash should stack over the run cap, got {peak}"
        );

        // Continue running forward; the boost drags off but the base is held at
        // the run cap by ongoing input.
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        let mut returned = false;
        for _ in 0..30 {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                returned = true;
                break;
            }
        }
        assert!(returned, "dash should exit once the boost has bled off");
        let settled = horiz_speed(&comp);
        // The retained base settles back at the run cap — the dash_drag bleed did
        // not drag it below the band.
        assert!(
            approx_eq(settled, run_cap, 0.2),
            "retained base should settle at the run cap {run_cap}, got {settled}"
        );
    }

    // DASH_MAX_MS guard: the Dash state cannot persist past DASH_MAX_MS even if
    // momentum stays high.
    #[test]
    fn dash_max_ms_guard_bounds_state() {
        let world = flat_floor_and_wall_world();
        // Zero drag + full retention + sustained input keeps speed pinned above
        // the band indefinitely, so only the elapsed guard can end the dash.
        let desc = dash_descriptor(dash_params(8.0, 1.0, 1.0, 0.0, 0.0, 0, false));
        let (mut comp, mut pos) = settle_and_run(&desc, &world, 60);
        let dash_run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_run, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));

        // DASH_MAX_MS / (dt*1000) ticks bound the state. Run a few past that.
        let max_ticks = (DASH_MAX_MS / (DT * 1000.0)).ceil() as usize;
        let mut exited_by = None;
        let run = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        for i in 0..(max_ticks + 5) {
            let (next, _ev) = tick(&mut comp, &run, &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                exited_by = Some(i + 1);
                break;
            }
        }
        let exit_tick = exited_by.expect("dash must exit by the DASH_MAX_MS guard");
        // +1 for the entry tick already consumed above.
        assert!(
            exit_tick <= max_ticks,
            "dash exited after {} ticks; DASH_MAX_MS bounds it at ~{} ticks",
            exit_tick + 1,
            max_ticks
        );
    }

    // Cooldown: a dash requested while cooldown is active is suppressed — no
    // second impulse until the cooldown elapses.
    #[test]
    fn dash_cooldown_suppresses_until_elapsed() {
        let world = flat_floor_and_wall_world();
        // 500 ms cooldown, instant drag so the first dash ends quickly and we
        // return to Normal while the cooldown is still counting down.
        let desc = dash_descriptor(dash_params(12.0, 0.0, 0.0, 200.0, 500.0, 0, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        // Zero wish_dir so the dash takes its direction from facing (forward =
        // -Z) and the suppression check sees ONLY the dash impulse — no Normal
        // locomotion accelerating the player when the dash is gated off.
        let dash = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(comp.dash_cooldown_ms > 0.0, "cooldown should be armed");

        // Let the first dash decay back to Normal (drag is strong).
        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(matches!(comp.movement_state, MovementState::Normal));
        assert!(
            comp.dash_cooldown_ms > 0.0,
            "cooldown should still be active well within 500ms"
        );

        // Request a dash while the cooldown is active: suppressed.
        let speed_before = horiz_speed(&comp);
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "dash must be suppressed while cooldown is active"
        );
        assert!(
            horiz_speed(&comp) <= speed_before + VEL_EPS,
            "no dash impulse should be applied during cooldown"
        );

        // Run out the cooldown, then a fresh dash fires again.
        for _ in 0..40 {
            if comp.dash_cooldown_ms <= 0.0 {
                break;
            }
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
        }
        assert!(comp.dash_cooldown_ms <= 0.0, "cooldown should have elapsed");
        let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "dash should fire again once the cooldown has elapsed"
        );
    }

    // Rising edge: holding the dash button does not re-trigger after the
    // cooldown elapses; only a fresh press fires. `dash_pressed` is a true rising
    // edge derived at the call site, so a held button presents as a single
    // `true` then `false` — modeled here by holding `dash_pressed = false` after
    // the initial press.
    #[test]
    fn dash_rising_edge_held_button_does_not_refire() {
        let world = flat_floor_and_wall_world();
        // Short cooldown so it elapses within the test window.
        let desc = dash_descriptor(dash_params(12.0, 0.0, 0.0, 200.0, 100.0, 0, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        // Initial press: the rising edge fires the dash.
        let press = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &press, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));

        // The button stays physically held, but the call site only emits a
        // rising edge once — so dash_pressed is false for the held duration.
        // Run long enough for the dash to end AND the cooldown to elapse.
        let held = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let mut redashed = false;
        for _ in 0..60 {
            let (next, _ev) = tick(&mut comp, &held, &world, GRAVITY, DT, pos);
            pos = next;
            if comp.dash_cooldown_ms <= 0.0
                && matches!(comp.movement_state, MovementState::Dash { .. })
            {
                redashed = true;
                break;
            }
        }
        assert!(
            !redashed,
            "a held button (no fresh rising edge) must not re-trigger the dash after cooldown"
        );
        assert!(
            comp.dash_cooldown_ms <= 0.0,
            "setup: cooldown should have elapsed"
        );

        // A fresh press now fires again.
        let (next, _ev) = tick(&mut comp, &press, &world, GRAVITY, DT, pos);
        pos = next;
        let _ = pos;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "a fresh rising edge after cooldown should fire the dash"
        );
    }

    /// Place the player high above the floor, airborne, with ability budgets
    /// full — giving ample air time for several airborne dashes before the long
    /// fall back to y=0. Avoids the short, altitude-sensitive window a single
    /// jump provides (a horizontal dash that zeroes vy lands almost immediately
    /// from a low apex). The first tick the test runs establishes the airborne
    /// `is_grounded=false` flag via the substrate.
    fn airborne_aloft(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
    ) -> (PlayerMovementComponent, Vec3) {
        let mut comp = PlayerMovementComponent::from_descriptor(desc);
        comp.is_grounded = false;
        comp.air_ticks = 10; // already settled into the airborne regime
        let mut pos = Vec3::new(0.0, 20.0, 0.0);
        // A few idle ticks let the substrate confirm airborne and build up a
        // clearly-nonzero downward vy (so preserve-vertical tests can tell a
        // retained fall from a zeroed one).
        run_ticks(&mut comp, world, &mut pos, 8, &idle_input());
        (comp, pos)
    }

    // Air-dash budget: dashes fire while airborne up to the configured budget,
    // are exhausted after that many airborne dashes, and are restored on landing.
    #[test]
    fn dash_air_budget_exhausts_then_restores_on_landing() {
        let world = flat_floor_and_wall_world();
        // 2 air dashes, no cooldown so the budget (not cooldown) is the gate.
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, false));
        let (mut comp, mut pos) = airborne_aloft(&desc, &world);
        assert!(!comp.is_grounded, "setup: should be airborne");
        assert_eq!(comp.air_dashes_remaining, 2, "budget starts full aloft");

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };

        // First airborne dash consumes one charge.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(matches!(comp.movement_state, MovementState::Dash { .. }));
        assert_eq!(
            comp.air_dashes_remaining, 1,
            "first air-dash consumes a charge"
        );

        // Return to Normal (drag bleeds the dash) so the next press is a fresh
        // Normal→Dash transition, still airborne.
        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(
            !comp.is_grounded,
            "setup: still airborne for the second dash"
        );

        // Second airborne dash consumes the last charge.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert_eq!(
            comp.air_dashes_remaining, 0,
            "second air-dash exhausts the budget"
        );

        for _ in 0..20 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                break;
            }
        }
        assert!(
            !comp.is_grounded,
            "setup: still airborne for the exhausted attempt"
        );

        // Third attempt while exhausted: suppressed.
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Normal),
            "an airborne dash must not fire with an exhausted budget"
        );

        // Fall and land; the budget is restored through refresh_on_landing.
        for _ in 0..180 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if comp.is_grounded {
                break;
            }
        }
        assert!(comp.is_grounded, "setup: player should have landed");
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "landing should restore the air-dash budget"
        );
    }

    // Airborne classification spends exactly one charge: a dash fired on a tick
    // whose last-tick `is_grounded` flag is airborne (and which makes no floor
    // contact) consumes one air-dash charge in the intent step. With no floor
    // touch there is no landing-refresh, so the spend is the sole effect on the
    // budget — directly observable. This is the consume half of the
    // landing-tick behavior, isolated so a silently-skipped consume (grounded
    // misclassification) fails here rather than being masked by a same-tick
    // refresh.
    #[test]
    fn dash_airborne_classification_spends_one_charge() {
        let world = flat_floor_and_wall_world();
        // 2 air dashes so a single consume (→1) is distinct from both a full
        // budget (→2, consume skipped) and an exhausted one (→0).
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, true));
        let (mut comp, pos) = airborne_aloft(&desc, &world);
        // Aloft at y=20 with no nearby floor: this tick's sweep cannot contact
        // the floor, so refresh_on_landing will not run.
        assert!(!comp.is_grounded, "setup: airborne entering the tick");
        assert_eq!(comp.air_dashes_remaining, 2, "setup: budget full aloft");

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);

        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have fired"
        );
        assert!(
            !comp.is_grounded,
            "setup: no floor contact, so no landing-refresh this tick"
        );
        // Fails (stays at 2) if the airborne consume is skipped via a grounded
        // misclassification — the spend is observable precisely because nothing
        // refreshes it back.
        assert_eq!(
            comp.air_dashes_remaining, 1,
            "airborne-classified dash spends exactly one air-dash charge"
        );
    }

    // Landing-tick ordering: a dash fired on the landing tick consumes an
    // air-dash charge in the intent/transition step (classified airborne via the
    // stale last-tick `is_grounded` flag), and the landing-refresh runs AFTERWARD
    // in the substrate-result step. Seeded one charge short of full so the
    // consume-then-refresh order leaves a FULL budget, while the inverted
    // refresh-then-consume order would leave it one short — making the ordering
    // directly observable in the post-tick budget.
    #[test]
    fn dash_on_landing_tick_consumes_then_refreshes() {
        let world = flat_floor_and_wall_world();
        // air_dashes = 2 is the refresh target. Seed remaining = 1 (one short):
        //   consume-then-refresh: 1 → 0 (consume) → 2 (refresh)  ⇒ 2 (full)
        //   refresh-then-consume: 1 → 2 (refresh) → 1 (consume)  ⇒ 1 (one short)
        // The post-tick value distinguishes the two orderings. preserve_vertical
        // so the entering downward velocity carries the capsule into the floor on
        // this single landing tick.
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, true));
        let floor_y = desc.capsule.half_height + desc.capsule.radius;

        let mut comp = PlayerMovementComponent::from_descriptor(&desc);
        comp.is_grounded = false; // last-tick flag is airborne (stale on landing)
        comp.air_ticks = 5;
        comp.air_dashes_remaining = 1; // one short of the refresh target (2)
        // A hair above the floor with a downward velocity so this single tick's
        // sweep registers floor contact (the landing tick).
        let pos = Vec3::new(0.0, floor_y + 0.02, 0.0);
        comp.velocity = Vec3::new(0.0, -2.0, 0.0);

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);

        assert!(
            comp.is_grounded,
            "setup: this tick should have landed the player"
        );
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne-classified dash should have fired on the landing tick"
        );
        // Full budget proves consume (1→0) ran BEFORE refresh (0→2). The inverted
        // order would leave 1 here, so this also fails on an ordering inversion.
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "landing-tick dash spends a charge in the intent step, then the \
             landing-refresh restores the full budget"
        );
    }

    // Disabled: absent DashParams ⇒ Normal→Dash never fires, no impulse
    // regardless of input.
    #[test]
    fn dash_disabled_when_no_params() {
        let world = flat_floor_and_wall_world();
        let desc = canonical_descriptor();
        assert!(desc.dash.is_none(), "canonical descriptor has no dash");
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());

        let speed_before = horiz_speed(&comp);
        // Zero wish_dir: with dash disabled and no locomotion input, any speed
        // gain could only come from a (forbidden) dash impulse.
        let dash = MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        for _ in 0..10 {
            let (next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                matches!(comp.movement_state, MovementState::Normal),
                "dash must never fire when DashParams is absent"
            );
        }
        assert!(
            horiz_speed(&comp) <= speed_before + VEL_EPS,
            "no dash impulse should ever be applied with dash disabled"
        );
    }

    // preserveVertical: on an airborne dash, false zeroes vertical velocity and
    // true retains it (gravity then resumes).
    #[test]
    fn dash_preserve_vertical_airborne() {
        let world = flat_floor_and_wall_world();

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };

        // The dash ENTERS in `Normal`'s intent step, which zeroes/keeps vy at the
        // very end of that tick; the dash's own gravity resumes the NEXT tick (in
        // `dash_intent`). So the entry-tick result is exactly zeroed/retained, and
        // a following tick shows gravity resume.

        // preserve_vertical = false: vy is zeroed on entry.
        let zero_desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 3, false));
        let (mut z, zpos) = airborne_aloft(&zero_desc, &world);
        // Aloft and falling: a clearly-nonzero downward vy distinguishes a
        // zeroed entry from a retained one.
        assert!(
            z.velocity.y < -1.0,
            "setup: should have a clearly-nonzero downward vy aloft, got {}",
            z.velocity.y
        );
        let (znext, _ev) = tick(&mut z, &air_dash, &world, GRAVITY, DT, zpos);
        assert!(
            matches!(z.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have fired"
        );
        // Entry zeroed vy exactly.
        assert!(
            approx_eq(z.velocity.y, 0.0, VEL_EPS),
            "preserve_vertical=false should zero entering vy, got {}",
            z.velocity.y
        );
        // Gravity resumes the next (in-Dash) tick: vy goes negative again.
        let (_n, _e) = tick(&mut z, &idle_input(), &world, GRAVITY, DT, znext);
        assert!(
            z.velocity.y < -VEL_EPS,
            "gravity should resume after a zeroed-vertical dash entry, got {}",
            z.velocity.y
        );

        // preserve_vertical = true: vy is retained on entry.
        let keep_desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 3, true));
        let (mut k, kpos) = airborne_aloft(&keep_desc, &world);
        let vy_before = k.velocity.y;
        assert!(
            vy_before < -1.0,
            "setup: downward vy aloft, got {vy_before}"
        );
        let (_knext, _ev) = tick(&mut k, &air_dash, &world, GRAVITY, DT, kpos);
        // The entry runs inside `Normal`'s intent step, so Normal's gravity for
        // this tick already advanced vy before the keep; the retained value is
        // therefore `vy_before + g*dt`, clearly distinct from the false case's 0.
        let expected = vy_before + GRAVITY * DT;
        assert!(
            approx_eq(k.velocity.y, expected, VEL_EPS),
            "preserve_vertical=true should keep entering vy, expected ~{expected}, got {}",
            k.velocity.y
        );
    }

    /// Walk a grounded player toward +X until just shy of the wall at x=5, then
    /// return the settled component/position primed to dash into it.
    fn approach_wall_grounded(
        desc: &PlayerMovementDescriptor,
        world: &CollisionWorld,
    ) -> (PlayerMovementComponent, Vec3) {
        let (mut comp, mut pos) = settle_player(desc);
        run_ticks(&mut comp, world, &mut pos, 10, &idle_input());
        let walk = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            facing_yaw: 0.0,
        };
        // Walk toward +X until close to the wall but still clear of the capsule
        // standoff (radius 0.4 ⇒ center stops near x≈4.6).
        for _ in 0..200 {
            let (next, _ev) = tick(&mut comp, &walk, world, GRAVITY, DT, pos);
            pos = next;
            if pos.x > 3.5 {
                break;
            }
        }
        (comp, pos)
    }

    // Regression: dashing head-on into geometry left a phantom backward velocity.
    // Collide-and-slide zeroed the velocity component along the boost axis, but
    // the tracked `boost` kept full magnitude, so `base = velocity - boost`
    // reconstructed a vector pointing back out of the wall (empirically vx=-1.5,
    // base.x=-15 the tick after entry). The boost/realized reconciliation in
    // `dash_intent` clamps it. This test fails on the pre-fix code.
    #[test]
    fn dash_head_on_into_wall_does_not_reverse() {
        let world = flat_floor_and_wall_world();
        // High boost, no drag-into-band complications; rigid committed dash.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 0, false));
        let capsule_standoff = 5.0 - desc.capsule.radius; // wall x=5, radius 0.4
        let (mut comp, mut pos) = approach_wall_grounded(&desc, &world);

        let dash_into_wall = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_into_wall, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: dash should have entered, got {:?}",
            comp.movement_state
        );

        // Hold the dash direction into the wall; track velocity and penetration
        // across the whole dash AND the exit tick. No tick — including the one
        // that transitions back to Normal — may leave a backward velocity. The
        // phantom-base bug surfaces precisely on the tick after the wall zeroes
        // the boost axis, which is also the tick the dash exits, so the check
        // must run after each tick rather than only at the top of the loop.
        let mut returned_to_normal = false;
        for _ in 0..40 {
            let (next, _ev) = tick(&mut comp, &dash_into_wall, &world, GRAVITY, DT, pos);
            pos = next;
            assert!(
                comp.velocity.x > -VEL_EPS,
                "dash into wall must not produce backward velocity, got vx={}",
                comp.velocity.x
            );
            assert!(
                pos.x < capsule_standoff + 0.05,
                "player must not penetrate the wall (standoff {capsule_standoff}), got x={}",
                pos.x
            );
            if matches!(comp.movement_state, MovementState::Normal) {
                returned_to_normal = true;
                break;
            }
        }
        assert!(
            returned_to_normal,
            "dash blocked by the wall should exit cleanly into Normal"
        );
    }

    // An angled dash into the wall (toward +X, along -Z) should slide along the
    // wall: the tangential -Z speed survives while the into-wall +X component is
    // clipped. The boost reconciliation must not stick or reverse the slide.
    #[test]
    fn dash_angled_into_wall_slides_along_it() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 0, false));
        let (mut comp, mut pos) = approach_wall_grounded(&desc, &world);

        // wish_dir=(1,1): into +X (the wall) and along -Z (tangent, free). With
        // facing_yaw=0, forward=-Z and right=+X, so this resolves to (1,0,-1).
        let dash_diag = MovementInput {
            wish_dir: Vec2::new(1.0, 1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: true,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &dash_diag, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: angled dash should have entered"
        );

        // The tick after entry, collision has clipped the +X component but the
        // tangential -Z slide must remain (clearly negative, not stuck/reversed).
        let (next, _ev) = tick(&mut comp, &dash_diag, &world, GRAVITY, DT, pos);
        pos = next;
        assert!(
            comp.velocity.z < -1.0,
            "angled dash into the wall should retain tangential -Z slide, got vz={}",
            comp.velocity.z
        );
        assert!(
            comp.velocity.x > -VEL_EPS,
            "angled dash must not reverse out of the wall, got vx={}",
            comp.velocity.x
        );
        // The player should keep advancing along -Z (sliding), not stick.
        let z_before = pos.z;
        run_ticks(&mut comp, &world, &mut pos, 3, &dash_diag);
        assert!(
            pos.z < z_before - VEL_EPS,
            "player should slide along the wall in -Z, z went {z_before} -> {}",
            pos.z
        );
    }

    // Finding #3: a grounded dash must not spend an air-dash charge — the consume
    // is gated on `!is_grounded` in `try_enter_dash`.
    #[test]
    fn dash_grounded_preserves_air_dash_budget() {
        let world = flat_floor_and_wall_world();
        let desc = dash_descriptor(dash_params(10.0, 0.0, 0.0, 50.0, 0.0, 2, false));
        let (mut comp, mut pos) = settle_player(&desc);
        run_ticks(&mut comp, &world, &mut pos, 10, &idle_input());
        assert!(comp.is_grounded, "setup: player should be grounded");
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "setup: full air-dash budget before the grounded dash"
        );

        let dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (_next, _ev) = tick(&mut comp, &dash, &world, GRAVITY, DT, pos);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: grounded dash should have fired"
        );
        assert_eq!(
            comp.air_dashes_remaining, 2,
            "a grounded dash must not consume an air-dash charge"
        );
    }

    // Finding #7: jump input during the Dash state is dropped by design —
    // `dash_intent` omits the jump branch. vy should only follow gravity.
    #[test]
    fn dash_ignores_jump_input() {
        let world = flat_floor_and_wall_world();
        // preserve_vertical so entry does not zero vy, isolating the jump check.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 50.0, 0.0, 3, true));
        let (mut comp, pos) = airborne_aloft(&desc, &world);

        // Enter the dash (airborne) without jump.
        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (pos, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        assert!(
            matches!(comp.movement_state, MovementState::Dash { .. }),
            "setup: airborne dash should have entered"
        );

        // Now hold jump while in Dash; vy must only advance by gravity.
        let dash_with_jump = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: true,
            dash_pressed: false,
            running: false,
            facing_yaw: 0.0,
        };
        let vy_before = comp.velocity.y;
        let (_next, _ev) = tick(&mut comp, &dash_with_jump, &world, GRAVITY, DT, pos);
        let expected = vy_before + GRAVITY * DT;
        assert!(
            approx_eq(comp.velocity.y, expected, VEL_EPS),
            "jump during dash must not add impulse: expected gravity-only vy ~{expected}, got {}",
            comp.velocity.y
        );
    }

    // Finding #6: an airborne dash with `dash_drag == 0` and a large boost should
    // decay back into the steady band rather than stay pinned at boost speed —
    // the boost folds into `Normal`'s contextual air cap.
    #[test]
    fn dash_airborne_zero_drag_decays_into_band() {
        let world = flat_floor_and_wall_world();
        // dash_drag=0, momentum_retention=0; large boost well above the band.
        let desc = dash_descriptor(dash_params(15.0, 0.0, 0.0, 0.0, 0.0, 3, true));
        let band = desc.ground.speed.run; // exit band is ground run speed
        let (mut comp, mut pos) = airborne_aloft(&desc, &world);

        let air_dash = MovementInput {
            wish_dir: Vec2::new(0.0, -1.0),
            jump_pressed: false,
            dash_pressed: true,
            running: false,
            facing_yaw: 0.0,
        };
        let (next, _ev) = tick(&mut comp, &air_dash, &world, GRAVITY, DT, pos);
        pos = next;
        let peak = horiz_speed(&comp);
        assert!(
            peak > band,
            "setup: airborne dash should start above the band, got {peak}"
        );

        // Idle airborne; the boost must bleed back into the band before the
        // DASH_MAX_MS guard would force the exit.
        let mut settled = false;
        for _ in 0..40 {
            let (next, _ev) = tick(&mut comp, &idle_input(), &world, GRAVITY, DT, pos);
            pos = next;
            if matches!(comp.movement_state, MovementState::Normal) {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "zero-drag airborne dash should decay into Normal, not stay at boost speed"
        );
        assert!(
            horiz_speed(&comp) <= band + VEL_EPS,
            "post-dash horizontal speed should settle into the band, got {}",
            horiz_speed(&comp)
        );
    }
}
