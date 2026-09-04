// Dash velocity intent and entry helpers.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::movement::carry::CarryRule;
use crate::movement::substrate::{pm_accelerate, wish_dir_from_input};
use crate::movement::{MovementInput, Transition};
use postretro_foundation::{
    BoolOrIr, BoundProgram, IrValue, MovementScope, MovementState, NumberOrIr,
    PlayerMovementComponent, eval_value,
};

use super::{GROUND_STOP_FRICTION, apply_normal_horizontal_decay};

/// Hard upper bound on how long the `Dash` state can persist, in milliseconds.
/// A seamed engine constant (not a descriptor field): it bounds the state so a
/// dash with high retained momentum or zero drag cannot linger indefinitely.
/// When the elapsed-time guard reaches this, the dash exits into `Normal`
/// regardless of speed. 200 ms ≈ 12 ticks at 60 Hz — a snappy Doom-Eternal /
/// Titanfall-shaped burst.
pub(crate) const DASH_MAX_MS: f32 = 200.0;

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
pub(crate) fn try_enter_dash(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
) -> Option<Transition> {
    component.dash.as_ref()?;
    if component.dash_cooldown_ms > 0.0 {
        return None;
    }
    if !component.is_grounded() {
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
pub(crate) fn dash_intent(
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
    if !component.is_grounded() {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    let ground_speed = if input.running {
        component.ground_params.speed.run
    } else {
        component.ground_params.speed.walk
    };

    // Input steering, scaled by `steer_control`. At 0 the term is omitted
    // entirely (committed dash); at 1 it is `Normal`'s full `pm_accelerate`.
    // Steering adds to the composite velocity (base-level authority); it does
    // not feed the tracked boost layer.
    let input_dir_3d = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if steer_control > 0.0 && input_dir_3d.length_squared() > 0.0 {
        let context_accel = if component.is_grounded() {
            component.ground_params.accel
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
        if component.is_grounded() {
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
