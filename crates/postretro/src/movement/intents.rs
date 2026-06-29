// Per-state velocity intents for player movement.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::collision::CollisionWorld;
use crate::movement::carry::CarryRule;
use crate::movement::dispatch::{stand_up_resize, stand_up_transition};
use crate::movement::substrate::{
    JumpEdges, ResizeAnchor, air_jump_ready, pm_accelerate, resize_capsule,
    standup_clearance_probe, wish_dir_from_input,
};
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_foundation::{
    BoolOrIr, BoundProgram, IrValue, MovementScope, MovementState, NumberOrIr,
    PlayerMovementComponent, eval_value,
};

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
pub(super) const DASH_MAX_MS: f32 = 200.0;

/// Fractional margin above the run cap before the held-input grounded
/// over-speed bleed (`normal_intent` step 6) engages. `pm_accelerate`'s
/// projection clamp leaves sub-unit floating-point overshoot just above the cap
/// during normal running (~1e-4); reacting to that would perturb steady-state
/// motion. Real banked momentum (a dash handing off above the cap) clears this
/// margin by a wide margin, so 0.2 % cleanly separates signal from float noise.
const OVERSPEED_BLEED_MARGIN: f32 = 1.002;

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
/// Sets `events.jumped` when a jump launches. Returns the warranted transition
/// (next state + its carry-rule) or `None` to stay in `Normal`. `Normal`
/// transitions to `Dash` on a rising-edge dash input (see `try_enter_dash`) and
/// to `Crouching` on the resolved `crouch_intent` bit when `CrouchParams` is
/// present; future states (slide, wall-run) plug in behind the same seam without
/// reshaping callers.
///
/// `jump_edges` are the forgiveness-derived edges (coyote + buffer), computed
/// ONCE per tick by `derive_jump_edges` before this intent runs (D5). The jump
/// steps consume `jump_edges.grounded` / `jump_edges.air` IN PLACE OF the raw
/// `jump_pressed` bit so forgiveness is never re-derived here.
pub(super) fn normal_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    position: &mut Vec3,
    events: &mut MovementEvents,
) -> Option<Transition> {
    // 1. Gravity (airborne only).
    if !component.is_grounded {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    // 2. Grounded jump — fired off the DERIVED grounded edge (a fresh grounded
    // press, a coyote-granted press, or a buffered press landing) rather than
    // raw `jump_pressed`. Consumes NO air-jump charge: a coyote/buffered jump is
    // a grounded jump. Sets `jump_spent` so coyote cannot re-arm this stretch.
    if jump_edges.grounded {
        component.velocity.y = component.air.jump_velocity;
        component.is_grounded = false;
        component.jump_spent = true;
        events.jumped = true;
    }
    // 3. Air-jump (double-jump): a named airborne ability under the budget
    // model. Fires off the DERIVED air edge (an airborne press the grounded edge
    // did not claim) AND the budget/ceiling gate. Consumes one charge from
    // `air_jumps_remaining`, which refreshes uniformly on floor contact through
    // `refresh_on_landing` (the single landing-refresh point shared with other
    // air-budget abilities, e.g. air-dash). The ceiling gate
    // (`velocity.y <= air.jump_ceiling`) keeps it from firing at the top of the
    // rising arc; the launch reuses the ground jump velocity. Spends the
    // jump-spent flag so coyote cannot re-arm after an air jump.
    else if jump_edges.air && air_jump_ready(component) {
        component.velocity.y = component.air.jump_velocity;
        component.air_jumps_remaining -= 1;
        component.jump_spent = true;
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
        if let Some(transition) = try_enter_dash(component, input) {
            return Some(transition);
        }
    }

    // `Normal` → `Crouching`: fire on the resolved `crouch_intent` bit when a
    // `CrouchParams` is materialized. Absent `crouch` ⇒ the transition NEVER
    // fires (crouch disabled — no resize, no effect). The entry resize runs here
    // (the edge): shrink the collision capsule to the crouched size with the
    // anchor chosen by grounded-vs-airborne — `Feet` when grounded (plant the
    // feet, drop the center, D2), `Head` when airborne (pin the head, raise the
    // feet, D4) — and apply the helper-returned center delta to `position`. The
    // eye smooths from the current standing eye toward the crouched target inside
    // the `Crouching` intent; seed `eye_current` at the standing eye so the first
    // tick begins the descent. The carry is `KEEP_ALL`: crouch is a resize, not a
    // velocity reset, so momentum is preserved unchanged (the §6 parity no-op).
    if input.crouch_intent {
        if let Some(crouch) = component.crouch.as_ref() {
            let target_half_height = crouch.half_height;
            let target_eye_height = crouch.eye_height;
            let anchor = if component.is_grounded {
                ResizeAnchor::Feet
            } else {
                ResizeAnchor::Head
            };
            let eye_current = component.capsule.eye_height;
            let delta = resize_capsule(component, target_half_height, target_eye_height, anchor);
            position.y += delta;
            // `resize_capsule` snapped `eye_height` to the crouched target; the
            // eye must SMOOTH instead (D3). Restore the live `eye_height` to the
            // pre-entry value so the camera does not pop on the entry tick — the
            // `Crouching` intent advances `eye_current` toward the crouched target
            // from here and writes the smoothed value each tick. (`half_height`
            // keeps the crouched value the helper set: collision shrinks
            // immediately; only the camera eye eases.)
            component.capsule.eye_height = eye_current;
            return Some(Transition {
                next: MovementState::Crouching { eye_current },
                carry: CarryRule::KEEP_ALL,
            });
        }
    }

    None
}

/// Resolve a dash NUMBER field to its consumption value against a refreshed
/// `scope`. A literal returns its bare value bit-identically (no eval); an
/// expression evaluates its bound program and clamps the result to `[lo, hi]`.
///
/// `eval_value`'s per-node finite guard already excludes `NaN`/`±Inf`, so the
/// clamp only bounds the field's authored range. `program` is `Some` exactly
/// when `field` is an expression — the pairing is set up by `from_descriptor`.
fn resolve_number(
    field: &NumberOrIr,
    program: &Option<BoundProgram<MovementScope>>,
    scope: &MovementScope,
    lo: f32,
    hi: f32,
) -> f32 {
    match field {
        // Literal path stays bit-identical to the pre-expression behavior.
        NumberOrIr::Literal(v) => *v,
        NumberOrIr::Ir(_) => match program {
            Some(p) => match eval_value(p, scope) {
                IrValue::Number(n) => n.clamp(lo, hi),
                // Bind proved a number root; a bool here is a bind bug. Stay
                // total by clamping the type-zero rather than panicking.
                IrValue::Bool(_) => 0.0_f32.clamp(lo, hi),
            },
            // An expression field with no bound program means dash was disabled
            // at bind time; this resolve site is unreachable then. Floor to `lo`.
            None => lo,
        },
    }
}

/// Boolean analogue of [`resolve_number`]: a literal returns its bare value; an
/// expression evaluates its bound program. Booleans carry no range to clamp.
fn resolve_bool(
    field: &BoolOrIr,
    program: &Option<BoundProgram<MovementScope>>,
    scope: &MovementScope,
) -> bool {
    match field {
        BoolOrIr::Literal(v) => *v,
        BoolOrIr::Ir(_) => match program {
            Some(p) => match eval_value(p, scope) {
                IrValue::Bool(b) => b,
                IrValue::Number(_) => false,
            },
            None => false,
        },
    }
}

/// Attempt the `Normal` → `Dash` transition this tick. Returns the seeded `Dash`
/// state paired with its carry-rule when the dash fires, or `None` when it is
/// suppressed (dash disabled, cooldown active, or airborne with no charge left).
///
/// Grounded vs airborne is read from the last-tick `is_grounded` flag — the same
/// one-tick staleness the jump gate uses (acceptable, consistent tradeoff).
/// Grounded dashes are gated by cooldown ONLY and consume no air-dash charge;
/// airborne dashes additionally require (and consume) one air-dash charge.
/// Cooldown applies to every dash.
pub(super) fn try_enter_dash(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
) -> Option<Transition> {
    component.dash.as_ref()?;
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

    // Resolve the four entry-moment dash values BEFORE any velocity mutation.
    // The snapshot is refreshed AFTER the air-charge spend above, so an authored
    // `chargesRemaining` reads the POST-spend value; `elapsedMs` is 0 at entry.
    // Each value is evaluated into a local here; the velocity writes below see
    // only those locals, so the program borrows never overlap the mutation.
    // Literal fields skip eval and stay bit-identical to the pre-expression path.
    let (boost_speed, momentum_retention, cooldown_ms, preserve_vertical) = {
        let dash = component.dash.as_ref()?;
        let programs = &component.dash_programs;
        let mut scope = MovementScope::for_validation();
        scope.refresh(component, 0.0);
        // `boostSpeed`: floor 0 (an EXPRESSION evaluating to 0 yields a
        // zero-boost dash; a literal 0 was already rejected at declaration — its
        // bound is exclusive `> 0`, which no clamp can reproduce, so the eval
        // floor is the open bound's reflection). `momentumRetention` ∈ [0, 1].
        let boost_speed = resolve_number(
            &dash.boost_speed,
            &programs.boost_speed,
            &scope,
            0.0,
            f32::INFINITY,
        );
        let momentum_retention = resolve_number(
            &dash.momentum_retention,
            &programs.momentum_retention,
            &scope,
            0.0,
            1.0,
        );
        // `cooldownMs` ≥ 0.
        let cooldown_ms = resolve_number(
            &dash.cooldown_ms,
            &programs.cooldown_ms,
            &scope,
            0.0,
            f32::INFINITY,
        );
        let preserve_vertical =
            resolve_bool(&dash.preserve_vertical, &programs.preserve_vertical, &scope);
        (
            boost_speed,
            momentum_retention,
            cooldown_ms,
            preserve_vertical,
        )
    };

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

    // `Normal` → `Dash` carries no momentum transform at the seam: the entry
    // blend (retained base + boost, `preserve_vertical`) is authored above on
    // `component.velocity`, so the dispatch-applied carry must leave it exactly
    // as authored. `KEEP_ALL` is that no-op (the parity guarantee). `Normal`
    // carries no boost vector, so `keepBoost` operates on a zero boost here.
    Some(Transition {
        next: MovementState::Dash {
            elapsed_ms: 0.0,
            boost,
        },
        carry: CarryRule::KEEP_ALL,
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
///
/// Per-state live data (`elapsed_ms`, `boost`) is borrowed in place from the
/// active `Dash` variant — the dispatch resolves the component-vs-state borrow
/// once (see `dispatch_state_intent`), so this intent mutates its own data
/// directly rather than receiving it by value and re-packing it. The return is
/// purely a transition: `Some({ Normal, KEEP_ALL })` on exit, `None` to stay in
/// `Dash`. The exit carry is `KEEP_ALL` because the dash already hands velocity
/// back into the steady band itself — the seam must not perturb it (parity).
pub(super) fn dash_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    gravity: f32,
    dt: f32,
    elapsed_ms: &mut f32,
    boost: &mut Vec3,
) -> Option<Transition> {
    // Dash params must exist to be in this state (entry required `Some`). A
    // descriptor swap that cleared `dash` mid-dash drops back to `Normal` rather
    // than panicking.
    if component.dash.is_none() {
        return Some(Transition {
            next: MovementState::Normal,
            carry: CarryRule::KEEP_ALL,
        });
    }

    // Resolve the two per-tick dash values BEFORE any velocity mutation. The
    // snapshot's `elapsedMs` reads the dash state's `elapsed_ms` as it stands at
    // the TOP of the intent — 0 on the first dash tick, accumulating thereafter;
    // the increment of `*elapsed_ms` happens later in this tick. Eval into locals
    // here so the program borrows never overlap the velocity writes. Literal
    // fields skip eval and stay bit-identical to the pre-expression path.
    let (steer_control, dash_drag) = {
        let dash = component
            .dash
            .as_ref()
            .expect("dash present (checked above)");
        let programs = &component.dash_programs;
        let mut scope = MovementScope::for_validation();
        scope.refresh(component, *elapsed_ms);
        // `steerControl` ∈ [0, 1]; `dashDrag` ≥ 0.
        let steer_control = resolve_number(
            &dash.steer_control,
            &programs.steer_control,
            &scope,
            0.0,
            1.0,
        );
        let dash_drag = resolve_number(
            &dash.dash_drag,
            &programs.dash_drag,
            &scope,
            0.0,
            f32::INFINITY,
        );
        (steer_control, dash_drag)
    };

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
            apply_normal_horizontal_decay(boost, component, input, ground_speed, dt);
        }
    } else {
        // `dash_drag > 0`: constant LINEAR deceleration of the boost only
        // (world-units/sec², units consistent with `ground.accel`/`air.accel`),
        // decoupled from friction context. LINEAR, not exponential.
        let boost_speed = boost.length();
        if boost_speed > 0.0 {
            let new_speed = (boost_speed - dash_drag * dt).max(0.0);
            *boost *= new_speed / boost_speed;
        }
    }

    component.velocity.x = base.x + boost.x;
    component.velocity.z = base.z + boost.z;

    // Exit: total horizontal speed back inside `Normal`'s steady band (run speed
    // grounded / air cap airborne) OR the `DASH_MAX_MS` elapsed guard. The live
    // `elapsed_ms` accumulates in place; the dispatch writes the mutated `Dash`
    // data back when this returns `None` (stay).
    *elapsed_ms += dt * 1000.0;
    let horiz_speed = (component.velocity.x * component.velocity.x
        + component.velocity.z * component.velocity.z)
        .sqrt();
    // Steady band is `ground_speed` whether grounded or airborne: when `bunny_hop`
    // is off it matches `Normal`'s air cap; when on, `Normal` enforces no air cap,
    // so `ground_speed` is the band we choose to exit into rather than one `Normal`
    // maintains in that mode. Either way the dash is hard-bounded by `DASH_MAX_MS`.
    let steady_cap = ground_speed;
    if horiz_speed <= steady_cap || *elapsed_ms >= DASH_MAX_MS {
        return Some(Transition {
            next: MovementState::Normal,
            carry: CarryRule::KEEP_ALL,
        });
    }

    None
}

/// The `Crouching` state's per-tick velocity intent. Locomotion mirrors
/// `Normal` (gravity, jump/air-jump, ground/air acceleration, friction, the
/// airborne cap) with ONE substitution: the omnidirectional horizontal speed
/// target is the crouch tier `ground.speed.crouch` instead of walk/run (D5).
/// Jump access is NEVER suppressed (D10) — the grounded/air jump branch and the
/// `Dash` transition stay available exactly as in `Normal`.
///
/// Beyond locomotion the intent owns three crouch-specific responsibilities:
///   - Eye smoothing (D3): `eye_current` eases toward the crouched eye target
///     by a framerate-independent exponential approach at `transition_rate` per
///     second, written into `component.capsule.eye_height` each tick for the
///     camera follow.
///   - Stand-up (D7): while `crouch_intent` is INACTIVE, sweep the standing
///     capsule up via `standup_clearance_probe`; when CLEAR, resize back to
///     standing (apply the center delta to `position` — the feet stay planted,
///     the center rises), transition to `Normal` with `KEEP_ALL` (a resize, not
///     a velocity reset). When BLOCKED, stay crouched and retry next tick.
///   - Crouch-jump (D10): when a jump edge fires while `crouch_intent` is STILL
///     ACTIVE, run the same stand-up probe FIRST — clear headroom ⇒ stand
///     (resize, shift position) and transition to `Normal`, then apply the jump
///     this tick; blocked ⇒ apply the jump from the crouched state (lower arc,
///     crouched capsule retained). The jump is never swallowed.
///
/// `eye_current` is borrowed in place from the active `Crouching` variant (the
/// dispatch resolves the component-vs-state borrow once), so the intent advances
/// its own smoothing source directly. Returns the warranted transition (always
/// `KEEP_ALL` — crouch never transforms momentum at the seam) or `None` to stay
/// `Crouching`.
// Mirrors `normal_intent`'s shape; grouping the substrate/position handles would
// add an abstraction with one production caller and no reuse.
#[allow(clippy::too_many_arguments)]
pub(super) fn crouching_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    collision_world: &CollisionWorld,
    position: &mut Vec3,
    events: &mut MovementEvents,
    eye_current: &mut f32,
) -> Option<Transition> {
    let crouched_half_height = component.capsule.half_height;
    let standing_half_height = component.standing_half_height;

    // Stand-up anchor, decided from grounded-AT-TICK-START — BEFORE the jump
    // branch below may clear `is_grounded`. This must mirror the crouch-ENTRY
    // anchor so an entry→exit cycle nets to no center drift (D4): grounded entry
    // anchors `Feet`, airborne entry anchors `Head`. A ground-origin crouch-jump
    // clears `is_grounded` before the stand-up resize runs, so reading the flag
    // at the call site would wrongly pick `Head` and drive the launching feet into
    // the floor — hence the snapshot here.
    let stand_up_anchor = if component.is_grounded {
        ResizeAnchor::Feet
    } else {
        ResizeAnchor::Head
    };

    // 1. Gravity (airborne only) — identical to `Normal`.
    if !component.is_grounded {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    // 2. Jump — NEVER suppressed while crouched (D10). A grounded/coyote/buffered
    // edge or an air-jump edge fires exactly as in `Normal`. The crouch-jump
    // stand-if-clear behavior is resolved AFTER the velocity is authored (below):
    // here we only launch the arc and record that a jump fired this tick.
    let mut jumped_this_tick = false;
    if jump_edges.grounded {
        component.velocity.y = component.air.jump_velocity;
        component.is_grounded = false;
        component.jump_spent = true;
        events.jumped = true;
        jumped_this_tick = true;
    } else if jump_edges.air && air_jump_ready(component) {
        component.velocity.y = component.air.jump_velocity;
        component.air_jumps_remaining -= 1;
        component.jump_spent = true;
        events.jumped = true;
        jumped_this_tick = true;
    }

    // 3. Locomotion: ground vs air branch, mirroring `Normal` steps 4/5 but with
    // the crouch speed tier as the target (and airborne cap). Crouch is
    // omnidirectional like walk/run — the tier just sits below them.
    let ground_speed = component.ground.speed.crouch;
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
            let horiz = Vec2::new(component.velocity.x, component.velocity.z);
            let h_speed = horiz.length();
            if h_speed > ground_speed {
                let scale = ground_speed / h_speed;
                component.velocity.x *= scale;
                component.velocity.z *= scale;
            }
        }
    }

    // 4. Ground friction — same contextual decay as `Normal` step 6, using the
    // crouch tier as the cap so a crouched player bleeds to a stop / back to the
    // crouch cap rather than the run cap.
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
        let h_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
        if h_speed > ground_speed * OVERSPEED_BLEED_MARGIN {
            let drop = (h_speed - ground_speed) * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(ground_speed);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    }

    // 5. Eye smoothing (D3). Ease `eye_current` toward the crouched eye target
    // with a framerate-independent exponential approach and write it into the
    // live capsule for the camera follow. `crouch` is `Some` here — the state was
    // only entered when it was — but fall back gracefully (no eye change) if a
    // descriptor swap cleared it mid-crouch.
    if let Some(crouch) = component.crouch.as_ref() {
        let target_eye = crouch.eye_height;
        let rate = crouch.transition_rate;
        let alpha = 1.0 - (-rate * dt).exp();
        *eye_current += (target_eye - *eye_current) * alpha;
        component.capsule.eye_height = *eye_current;
    }

    // 6. Stand-up decision. The `Dash` transition stays available from
    // `Crouching` (D10) — check it first so a dash press exits crouch into the
    // dash burst regardless of crouch/jump state.
    if input.dash_pressed {
        if let Some(transition) = try_enter_dash(component, input) {
            return Some(transition);
        }
    }

    // Crouch-jump (D10): a jump fired this tick while `crouch_intent` is STILL
    // active. Probe headroom — clear ⇒ stand (resize, shift the center up) and
    // exit to `Normal` carrying the jump just launched; blocked ⇒ stay crouched,
    // the jump still applies (lower arc). Either way the jump is never swallowed.
    if jumped_this_tick && input.crouch_intent {
        if standup_clearance_probe(
            component,
            collision_world,
            *position,
            crouched_half_height,
            standing_half_height,
        ) {
            stand_up_resize(component, position, stand_up_anchor);
            *eye_current = component.capsule.eye_height;
            return Some(stand_up_transition());
        }
        // Blocked: remain `Crouching` with the crouched capsule, jump applied.
        return None;
    }

    // Stand-up on release (D7): `crouch_intent` inactive. Probe the standing
    // capsule upward; CLEAR ⇒ resize to standing (center rises, feet planted) and
    // exit to `Normal`; BLOCKED ⇒ stay crouched and retry next tick.
    if !input.crouch_intent
        && standup_clearance_probe(
            component,
            collision_world,
            *position,
            crouched_half_height,
            standing_half_height,
        )
    {
        stand_up_resize(component, position, stand_up_anchor);
        *eye_current = component.capsule.eye_height;
        return Some(stand_up_transition());
    }

    None
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
