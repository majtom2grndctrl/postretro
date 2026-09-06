//! Velocity-driven strafe-tilt evaluation.

use postretro_foundation::TiltParams;

use super::ViewFeelState;

/// Fixed spring damping ratio for strafe tilt. Slightly under-damped so the
/// roll leads and overshoots its target a touch before settling — the
/// "lead and settle" feel (D3). NOT author-exposed; `tension` is the only
/// authored spring knob (it sets the natural frequency).
const TILT_DAMPING_RATIO: f32 = 0.8;

/// Strafe tilt: a slightly under-damped spring settling the roll toward a
/// velocity-derived target. Returns the roll angle in degrees (pre-scale) and
/// advances the spring in the integrator with a frame-rate-independent step.
pub(super) fn evaluate(
    tilt: &TiltParams,
    lateral_velocity: f32,
    is_grounded: bool,
    state: &mut ViewFeelState,
    frame_dt: f32,
) -> f32 {
    // Target roll tracks the signed lateral velocity, clamped at +/- max_angle
    // once lateral speed reaches speed_reference. Sign carried by the input.
    // Airborne (D8, grounded-only): the target becomes level (zero) while the
    // spring KEEPS stepping — the roll settles out rather than freezing.
    let target = if tilt.grounded_only && !is_grounded {
        0.0
    } else {
        let normalized = (lateral_velocity / tilt.speed_reference).clamp(-1.0, 1.0);
        tilt.max_angle * normalized
    };

    advance_spring(
        &mut state.tilt_roll,
        &mut state.tilt_roll_velocity,
        target,
        tilt.tension,
        frame_dt,
    );
    state.tilt_roll
}

/// Advance a damped harmonic oscillator one step toward `target` using an
/// analytic (closed-form) solution of the spring ODE over `dt`. Closed-form is
/// frame-rate independent — stepping to a fixed wall-clock time in many small
/// steps or a few large ones converges to the same state — unlike naive
/// explicit Euler, which depends on step size and can diverge at large `dt`.
///
/// The spring is parameterized by its undamped natural frequency `omega`
/// (the authored `tension`) and the fixed [`TILT_DAMPING_RATIO`] `zeta`. For the
/// slightly-under-damped case (`zeta < 1`) the homogeneous solution is a
/// decaying sinusoid; we solve it directly for position and velocity.
fn advance_spring(position: &mut f32, velocity: &mut f32, target: f32, omega: f32, dt: f32) {
    // A zero-length step leaves the spring untouched (frame_dt == 0 contract).
    if dt <= 0.0 || omega <= 0.0 {
        return;
    }

    let zeta = TILT_DAMPING_RATIO;
    // Work in displacement from the target; the target is treated as constant
    // over the step (it is recomputed each frame from current velocity).
    let x0 = *position - target;
    let v0 = *velocity;

    let exp = (-zeta * omega * dt).exp();

    // Under-damped (zeta < 1): decaying oscillation. TILT_DAMPING_RATIO is fixed
    // below 1, so this is the operative branch; the critical/over-damped arms
    // are kept for correctness should the ratio ever change.
    if zeta < 1.0 {
        let omega_d = omega * (1.0 - zeta * zeta).sqrt();
        let (sin_d, cos_d) = (omega_d * dt).sin_cos();
        // x(t) = e^{-zeta*omega*t} [ x0 cos(wd t) + (v0 + zeta*omega*x0)/wd sin(wd t) ]
        let c2 = (v0 + zeta * omega * x0) / omega_d;
        *position = exp * (x0 * cos_d + c2 * sin_d) + target;
        *velocity = analytic_underdamped_velocity(x0, v0, zeta, omega, omega_d, dt);
    } else if (zeta - 1.0).abs() < f32::EPSILON {
        // Critically damped: x(t) = e^{-omega t} (x0 + (v0 + omega x0) t).
        let new_x = exp * (x0 + (v0 + omega * x0) * dt);
        let new_v = exp * (v0 - omega * (v0 + omega * x0) * dt);
        *position = new_x + target;
        *velocity = new_v;
    } else {
        // Over-damped: two real roots.
        let root = omega * (zeta * zeta - 1.0).sqrt();
        let r1 = -zeta * omega + root;
        let r2 = -zeta * omega - root;
        let c1 = (v0 - r2 * x0) / (r1 - r2);
        let c2 = x0 - c1;
        let e1 = (r1 * dt).exp();
        let e2 = (r2 * dt).exp();
        *position = c1 * e1 + c2 * e2 + target;
        *velocity = c1 * r1 * e1 + c2 * r2 * e2;
    }
}

/// Exact velocity of the under-damped homogeneous solution at time `dt`.
/// Split out so the position/velocity expressions stay legible.
fn analytic_underdamped_velocity(
    x0: f32,
    v0: f32,
    zeta: f32,
    omega: f32,
    omega_d: f32,
    dt: f32,
) -> f32 {
    let exp = (-zeta * omega * dt).exp();
    let (sin_d, cos_d) = (omega_d * dt).sin_cos();
    let c2 = (v0 + zeta * omega * x0) / omega_d;
    // x(t) = exp * (x0 cos + c2 sin)
    // v(t) = exp' * (...) + exp * (...)'
    //      = -zeta*omega*exp*(x0 cos + c2 sin)
    //        + exp*(-x0 omega_d sin + c2 omega_d cos)
    -zeta * omega * exp * (x0 * cos_d + c2 * sin_d)
        + exp * (-x0 * omega_d * sin_d + c2 * omega_d * cos_d)
}
