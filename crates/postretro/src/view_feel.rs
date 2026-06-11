// Pure first-person view-feel evaluator: head bob, strafe tilt, ambient sway.
// Render-rate, pawn-driven, owns no GPU or camera basis — the caller maps the
// scalar outputs onto its camera basis at the render-assembly site in `main.rs`.
// See: context/lib/movement.md

use std::f32::consts::TAU;

use glam::Vec3;

use crate::scripting::data_descriptors::{BobParams, SwayParams, TiltParams, ViewFeelParams};

/// Speed band (m/s) above `speed_threshold` over which bob eases in from 0 to
/// full amplitude. Exposed so the bob acceptance test references this band
/// rather than guessing the saturation speed. A small band keeps the onset
/// feeling responsive without a hard pop at the threshold.
pub(crate) const BOB_EASE_IN_BAND: f32 = 1.0;

/// Fixed spring damping ratio for strafe tilt. Slightly under-damped so the
/// roll leads and overshoots its target a touch before settling — the
/// "lead and settle" feel (D3). NOT author-exposed; `tension` is the only
/// authored spring knob (it sets the natural frequency).
const TILT_DAMPING_RATIO: f32 = 0.8;

/// Per-axis incommensurate frequency multipliers for ambient sway. Each axis
/// sums these sines at fixed irrational multiples of the authored base
/// frequency so the motion never visibly repeats (the alternative to Perlin
/// noise). The ratios are engine constants, not authored fields. Chosen near
/// irrational (√2, √3, golden-ratio neighbours) to avoid commensurate beats.
///
/// Decorrelation-ratio contract (applies to all three arrays): these literal
/// values are the contract — they sit near √2/√3/φ neighbours but are NOT
/// approximations of those constants. Do not replace them with
/// `f32::consts::SQRT_2` or similar; the stdlib constants would change the
/// value and reintroduce a commensurate beat. `approx_constant` is suppressed
/// on each array that actually trips the lint for exactly this reason.
#[allow(clippy::approx_constant)]
const SWAY_YAW_RATIOS: [f32; 3] = [1.0, 1.414_213_6, 2.236_068];
#[allow(clippy::approx_constant)]
const SWAY_PITCH_RATIOS: [f32; 3] = [1.103_516_6, 1.732_050_8, 2.645_751_3];
#[allow(clippy::approx_constant)]
const SWAY_ROLL_RATIOS: [f32; 3] = [0.870_551, 1.618_034, 2.094_395_2];

/// Engine-owned integrator state for the view-feel evaluator. Read AND updated
/// by [`evaluate`] each frame. Deliberately NOT on `PlayerMovementComponent`
/// and NOT on `InterpolableState`: view feel is render-rate, while tick state
/// stays position-only (D5). The caller holds one of these per camera and
/// passes it back in each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewFeelState {
    /// Current strafe-tilt roll angle (degrees), the spring's position.
    pub(crate) tilt_roll: f32,
    /// Current strafe-tilt roll angular velocity (degrees/sec), the spring's
    /// velocity. Carried across frames so the spring settles smoothly.
    pub(crate) tilt_roll_velocity: f32,
    /// Head-bob oscillator phase (radians). Advanced by distance travelled; held
    /// when bob is gated off so the cycle resumes in phase rather than popping.
    pub(crate) bob_phase: f32,
    /// Ambient-sway clock (seconds). Advanced by frame time only.
    pub(crate) sway_clock: f32,
}

impl Default for ViewFeelState {
    fn default() -> Self {
        Self {
            tilt_roll: 0.0,
            tilt_roll_velocity: 0.0,
            bob_phase: 0.0,
            sway_clock: 0.0,
        }
    }
}

/// One frame of view-feel motion, fully resolved and POST-SCALE (every channel
/// already multiplied by `global_scale`).
///
/// `bob_*` are offsets in METRES (the caller maps `bob_lateral` onto its camera
/// right vector and `bob_vertical` onto world up). The four angle channels are
/// in DEGREES (descriptor units) — Task 4 converts to radians before feeding the
/// camera roll. `tilt_roll` and `sway_roll` are emitted separately so the caller
/// sums them into the final roll; `sway_yaw` / `sway_pitch` add to the look
/// angles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewFeelOutput {
    pub(crate) bob_vertical: f32,
    pub(crate) bob_lateral: f32,
    pub(crate) tilt_roll: f32,
    pub(crate) sway_roll: f32,
    pub(crate) sway_yaw: f32,
    pub(crate) sway_pitch: f32,
}

impl ViewFeelOutput {
    /// All-zero output, used when view feel is fully disabled or every motion
    /// gates to zero.
    const ZERO: ViewFeelOutput = ViewFeelOutput {
        bob_vertical: 0.0,
        bob_lateral: 0.0,
        tilt_roll: 0.0,
        sway_roll: 0.0,
        sway_yaw: 0.0,
        sway_pitch: 0.0,
    };
}

/// Compute one frame of view-feel motion and advance the integrator state.
///
/// Inputs the caller derives from the pawn and its own camera basis:
/// - `horizontal_speed`: horizontal speed magnitude (m/s).
/// - `lateral_velocity`: SIGNED projection of pawn velocity onto the camera
///   RIGHT vector (m/s). Sign drives strafe-tilt direction; the evaluator never
///   sees the basis itself.
/// - `is_grounded`: pawn floor-contact state, gating each motion per its
///   resolved `grounded_only` flag (D8).
/// - `frame_dt`: render-frame delta (seconds). `0.0` leaves the integrator
///   untouched and outputs the current resting state.
/// - `global_scale`: master view-feel scale; multiplies every channel. Plain
///   parameter here — clamping/default ownership lives in the options module.
///
/// Absent sub-objects (`None` on [`ViewFeelParams`]) contribute zero for that
/// motion; the others are unaffected.
pub(crate) fn evaluate(
    params: &ViewFeelParams,
    horizontal_speed: f32,
    lateral_velocity: f32,
    is_grounded: bool,
    state: &mut ViewFeelState,
    frame_dt: f32,
    global_scale: f32,
) -> ViewFeelOutput {
    let (bob_vertical, bob_lateral) = match &params.bob {
        Some(bob) => evaluate_bob(bob, horizontal_speed, is_grounded, state, frame_dt),
        None => (0.0, 0.0),
    };

    let tilt_roll = match &params.tilt {
        Some(tilt) => evaluate_tilt(tilt, lateral_velocity, is_grounded, state, frame_dt),
        // No tilt spring: hold the roll at rest so a later-enabled tilt does not
        // inherit stale velocity. (`None` means the motion is absent entirely.)
        None => 0.0,
    };

    let (sway_yaw, sway_pitch, sway_roll) = match &params.sway {
        Some(sway) => evaluate_sway(sway, horizontal_speed, is_grounded, state, frame_dt),
        None => (0.0, 0.0, 0.0),
    };

    let output = ViewFeelOutput {
        bob_vertical: bob_vertical * global_scale,
        bob_lateral: bob_lateral * global_scale,
        tilt_roll: tilt_roll * global_scale,
        sway_roll: sway_roll * global_scale,
        sway_yaw: sway_yaw * global_scale,
        sway_pitch: sway_pitch * global_scale,
    };

    // Short-circuit is placed AFTER the sub-evaluators intentionally: the
    // integrator (spring, bob phase, sway clock) must keep advancing even
    // when scale is zero, so that re-enabling the scale resumes smoothly
    // with no frozen-state snap. This check zeroes only the OUTPUT.
    if global_scale == 0.0 {
        return ViewFeelOutput::ZERO;
    }
    output
}

/// Derive the evaluator's two velocity-space inputs from the pawn velocity and
/// the camera RIGHT vector, so the basis projection is testable apart from the
/// render loop (the evaluator itself never sees the basis).
///
/// - `horizontal_speed`: magnitude of the velocity with the world-up (Y)
///   component dropped — bob and sway read pawn speed in the ground plane, not
///   vertical fall/jump speed.
/// - `lateral_velocity`: SIGNED projection of velocity onto `camera_right`. A
///   right-strafe (velocity aligned with the camera's right) is positive, which
///   the tilt spring turns into the expected roll direction.
///
/// `camera_right` is expected to be the horizontal (Y-free), unit-length right
/// vector the view uses (`Camera::right`); the dot product is the signed lateral
/// speed regardless, but a non-unit basis would scale it.
pub(crate) fn view_feel_inputs(velocity: Vec3, camera_right: Vec3) -> (f32, f32) {
    let horizontal_speed = Vec3::new(velocity.x, 0.0, velocity.z).length();
    let lateral_velocity = velocity.dot(camera_right);
    (horizontal_speed, lateral_velocity)
}

/// Map a [`ViewFeelOutput`] onto the camera basis, producing the arguments the
/// render chokepoint (`InterpolableState::view_projection`) consumes. Kept pure
/// and separate from the render loop so the angle conversions, channel sums, and
/// offset basis mapping are unit-testable.
///
/// Returns `(roll, yaw_offset, pitch_offset, eye_offset)`:
/// - `roll` (radians): tilt's velocity-driven roll summed with sway's ambient
///   roll, both descriptor degrees converted to radians.
/// - `yaw_offset` / `pitch_offset` (radians): sway's look-angle channels, folded
///   into the caller's yaw/pitch.
/// - `eye_offset` (world-space metres): `bob_vertical` along world up (Y) plus
///   `bob_lateral` along `camera_right`. Bob channels are already metres — no
///   unit conversion.
pub(crate) fn map_output_to_camera(
    output: &ViewFeelOutput,
    camera_right: Vec3,
) -> (f32, f32, f32, Vec3) {
    let roll = (output.tilt_roll + output.sway_roll).to_radians();
    let yaw_offset = output.sway_yaw.to_radians();
    let pitch_offset = output.sway_pitch.to_radians();
    let eye_offset = Vec3::Y * output.bob_vertical + camera_right * output.bob_lateral;
    (roll, yaw_offset, pitch_offset, eye_offset)
}

/// Head bob: a distance-phased oscillator that self-gates below a speed
/// threshold. Returns `(vertical, lateral)` offsets in metres (pre-scale).
fn evaluate_bob(
    bob: &BobParams,
    horizontal_speed: f32,
    is_grounded: bool,
    state: &mut ViewFeelState,
    frame_dt: f32,
) -> (f32, f32) {
    // Airborne gating (D8): when grounded-only and off the floor, bob HOLDS its
    // phase (does not advance) and outputs zero, so it resumes in-cycle on
    // landing rather than snapping.
    if bob.grounded_only && !is_grounded {
        return (0.0, 0.0);
    }

    // Self-gate at or below the speed threshold: no advance, no output. The
    // phase is held so the cycle resumes coherently when motion picks up.
    if horizontal_speed <= bob.speed_threshold {
        return (0.0, 0.0);
    }

    // Advance phase by distance travelled this frame. `frequency` is cycles per
    // metre, so a full cycle (TAU radians) elapses per `1/frequency` metres:
    // dphase = distance * frequency * TAU.
    let distance = horizontal_speed * frame_dt;
    state.bob_phase = (state.bob_phase + distance * bob.frequency * TAU).rem_euclid(TAU);

    // Ease in from 0 at the threshold to 1 over BOB_EASE_IN_BAND m/s above it,
    // so amplitude ramps in rather than popping on at the gate.
    let ease = ((horizontal_speed - bob.speed_threshold) / BOB_EASE_IN_BAND).clamp(0.0, 1.0);

    // Lateral runs at half the vertical rate: the classic figure-eight gait
    // (one lateral sway per two vertical bobs).
    let vertical = state.bob_phase.sin() * bob.vertical_amplitude * ease;
    let lateral = (state.bob_phase * 0.5).sin() * bob.lateral_amplitude * ease;
    (vertical, lateral)
}

/// Strafe tilt: a slightly under-damped spring settling the roll toward a
/// velocity-derived target. Returns the roll angle in degrees (pre-scale) and
/// advances the spring in the integrator with a frame-rate-independent step.
fn evaluate_tilt(
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

/// Ambient sway: summed incommensurate sines per axis (yaw, pitch, roll), each
/// scaled by an effective amplitude that grows with speed. Returns
/// `(yaw, pitch, roll)` in degrees (pre-scale) and advances the sway clock.
fn evaluate_sway(
    sway: &SwayParams,
    horizontal_speed: f32,
    is_grounded: bool,
    state: &mut ViewFeelState,
    frame_dt: f32,
) -> (f32, f32, f32) {
    // Airborne gating (D8): grounded-only sway contributes zero off the floor.
    // The early return leaves sway_clock untouched — the clock advances only
    // past this gate — so the sway phase resumes coherently when grounding
    // is restored (no clock jump).
    if sway.grounded_only && !is_grounded {
        return (0.0, 0.0, 0.0);
    }

    state.sway_clock += frame_dt;

    // Effective amplitude is nonzero at rest (when amplitude > 0) and grows with
    // speed when speed_scale > 0; constant in speed when speed_scale == 0.
    let effective_amplitude = sway.amplitude * (1.0 + sway.speed_scale * horizontal_speed);

    let base_omega = TAU * sway.frequency;
    let phase = base_omega * state.sway_clock;

    let yaw = summed_sines(phase, &SWAY_YAW_RATIOS) * effective_amplitude;
    let pitch = summed_sines(phase, &SWAY_PITCH_RATIOS) * effective_amplitude;
    let roll = summed_sines(phase, &SWAY_ROLL_RATIOS) * effective_amplitude;
    (yaw, pitch, roll)
}

/// Sum a fixed set of sines at the given frequency ratios, normalized by the
/// sine count so the result stays within `[-1, 1]` regardless of how many sines
/// are summed. This bounds each sway axis by its effective amplitude.
fn summed_sines(base_phase: f32, ratios: &[f32]) -> f32 {
    let sum: f32 = ratios.iter().map(|ratio| (base_phase * ratio).sin()).sum();
    sum / ratios.len() as f32
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

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    // --- Param fixtures ----------------------------------------------------
    //
    // Build minimal `ViewFeelParams` with one motion enabled at a time so each
    // test isolates the channel under examination. `grounded_only` is set
    // explicitly per fixture so the airborne-gating tests read clearly.

    fn bob(grounded_only: bool) -> BobParams {
        BobParams {
            frequency: 1.0, // 1 cycle per metre
            vertical_amplitude: 0.1,
            lateral_amplitude: 0.05,
            speed_threshold: 0.5,
            grounded_only,
        }
    }

    fn tilt(tension: f32, grounded_only: bool) -> TiltParams {
        TiltParams {
            max_angle: 10.0,
            speed_reference: 4.0,
            tension,
            grounded_only,
        }
    }

    fn sway(speed_scale: f32, grounded_only: bool) -> SwayParams {
        SwayParams {
            amplitude: 2.0,
            frequency: 1.5,
            speed_scale,
            grounded_only,
        }
    }

    fn bob_only(b: BobParams) -> ViewFeelParams {
        ViewFeelParams {
            bob: Some(b),
            tilt: None,
            sway: None,
        }
    }

    fn tilt_only(t: TiltParams) -> ViewFeelParams {
        ViewFeelParams {
            bob: None,
            tilt: Some(t),
            sway: None,
        }
    }

    fn sway_only(s: SwayParams) -> ViewFeelParams {
        ViewFeelParams {
            bob: None,
            tilt: None,
            sway: Some(s),
        }
    }

    /// Drive `evaluate` for `n` frames at a fixed `dt`, holding the inputs
    /// constant. Returns the final output and leaves the state advanced.
    fn run_frames(
        params: &ViewFeelParams,
        horizontal_speed: f32,
        lateral_velocity: f32,
        is_grounded: bool,
        state: &mut ViewFeelState,
        dt: f32,
        scale: f32,
        n: usize,
    ) -> ViewFeelOutput {
        let mut out = ViewFeelOutput::ZERO;
        for _ in 0..n {
            out = evaluate(
                params,
                horizontal_speed,
                lateral_velocity,
                is_grounded,
                state,
                dt,
                scale,
            );
        }
        out
    }

    // --- Bob ---------------------------------------------------------------

    #[test]
    fn bob_outputs_zero_at_or_below_speed_threshold() {
        let params = bob_only(bob(true));
        let mut state = ViewFeelState::default();
        // Exactly at threshold and just below: both gate to zero.
        let at = run_frames(&params, 0.5, 0.0, true, &mut state, 1.0 / 60.0, 1.0, 10);
        assert!(approx_eq(at.bob_vertical, 0.0));
        assert!(approx_eq(at.bob_lateral, 0.0));

        let mut state = ViewFeelState::default();
        let below = run_frames(&params, 0.25, 0.0, true, &mut state, 1.0 / 60.0, 1.0, 10);
        assert!(approx_eq(below.bob_vertical, 0.0));
        assert!(approx_eq(below.bob_lateral, 0.0));
    }

    #[test]
    fn bob_amplitude_increases_through_ease_in_band_and_saturates() {
        // Sample the peak vertical bob magnitude at several speeds across the
        // ease-in band. Because the ease factor scales the amplitude linearly
        // and the sine peaks at 1, the maximum |vertical| over a full cycle is
        // `vertical_amplitude * ease`. We reach the peak by sweeping phase.
        let b = bob(true);
        let params = bob_only(b);
        let threshold = b.speed_threshold;

        // Peak magnitude observed over a cycle at a given constant speed.
        let peak_vertical = |speed: f32| -> f32 {
            let mut state = ViewFeelState::default();
            let mut peak = 0.0_f32;
            // Many small steps span well over a full bob cycle at this speed.
            for _ in 0..2000 {
                let out = evaluate(&params, speed, 0.0, true, &mut state, 1.0 / 240.0, 1.0);
                peak = peak.max(out.bob_vertical.abs());
            }
            peak
        };

        let quarter = peak_vertical(threshold + BOB_EASE_IN_BAND * 0.25);
        let half = peak_vertical(threshold + BOB_EASE_IN_BAND * 0.5);
        let full = peak_vertical(threshold + BOB_EASE_IN_BAND);
        let saturated = peak_vertical(threshold + BOB_EASE_IN_BAND * 3.0);

        // Monotonic rise through the band.
        assert!(quarter < half, "{quarter} should be < {half}");
        assert!(half < full, "{half} should be < {full}");
        // Saturates at the authored amplitude once past the band.
        assert!(approx_eq(full, b.vertical_amplitude));
        assert!(approx_eq(saturated, b.vertical_amplitude));
    }

    #[test]
    fn bob_holds_phase_and_outputs_zero_when_airborne_and_grounded_only() {
        let params = bob_only(bob(true));
        let mut state = ViewFeelState::default();
        // Advance a few grounded frames to build a non-trivial phase.
        run_frames(&params, 3.0, 0.0, true, &mut state, 1.0 / 60.0, 1.0, 5);
        let held_phase = state.bob_phase;
        assert!(
            held_phase != 0.0,
            "phase should have advanced while grounded"
        );

        // Airborne: outputs zero AND phase does not advance.
        let air = evaluate(&params, 3.0, 0.0, false, &mut state, 1.0 / 60.0, 1.0);
        assert!(approx_eq(air.bob_vertical, 0.0));
        assert!(approx_eq(air.bob_lateral, 0.0));
        assert!(
            approx_eq(state.bob_phase, held_phase),
            "phase must hold airborne"
        );
    }

    #[test]
    fn bob_behaves_as_grounded_when_grounded_only_false() {
        // grounded_only = false: airborne bob still oscillates above threshold.
        let params = bob_only(bob(false));
        let mut state = ViewFeelState::default();
        let mut peak = 0.0_f32;
        for _ in 0..2000 {
            let out = evaluate(&params, 3.0, 0.0, false, &mut state, 1.0 / 240.0, 1.0);
            peak = peak.max(out.bob_vertical.abs());
        }
        assert!(peak > 0.0, "ungated bob should oscillate even airborne");
    }

    // --- Tilt --------------------------------------------------------------

    #[test]
    fn tilt_sign_is_opposite_for_left_versus_right_strafe() {
        let params = tilt_only(tilt(20.0, true));

        let mut left_state = ViewFeelState::default();
        let left = run_frames(
            &params,
            4.0,
            -4.0,
            true,
            &mut left_state,
            1.0 / 60.0,
            1.0,
            200,
        );

        let mut right_state = ViewFeelState::default();
        let right = run_frames(
            &params,
            4.0,
            4.0,
            true,
            &mut right_state,
            1.0 / 60.0,
            1.0,
            200,
        );

        assert!(left.tilt_roll.signum() != right.tilt_roll.signum());
        assert!(approx_eq(left.tilt_roll, -right.tilt_roll));
    }

    #[test]
    fn tilt_magnitude_rises_with_lateral_speed_and_clamps_at_max_angle() {
        let t = tilt(30.0, true);
        let params = tilt_only(t);

        // Settle to steady state at half the reference speed and at/above it.
        let settle = |lateral: f32| -> f32 {
            let mut state = ViewFeelState::default();
            run_frames(
                &params,
                lateral.abs(),
                lateral,
                true,
                &mut state,
                1.0 / 120.0,
                1.0,
                4000,
            )
            .tilt_roll
        };

        let half = settle(t.speed_reference * 0.5);
        let at_ref = settle(t.speed_reference);
        let over_ref = settle(t.speed_reference * 2.0);

        assert!(
            half.abs() < at_ref.abs(),
            "magnitude rises with lateral speed"
        );
        // Clamps at max_angle for lateral speed >= speed_reference.
        assert!(approx_eq(at_ref, t.max_angle));
        assert!(approx_eq(over_ref, t.max_angle), "clamped beyond reference");
    }

    #[test]
    fn tilt_higher_tension_reaches_target_fraction_in_fewer_frames() {
        // From zero roll with a fixed clamped target, count frames to reach 50%
        // of the target. Higher tension (natural frequency) converges faster.
        let target_fraction = 0.5;

        let frames_to_fraction = |tension: f32| -> usize {
            let params = tilt_only(tilt(tension, true));
            let t = tilt(tension, true);
            let target = t.max_angle; // lateral >= speed_reference => clamped to max
            let mut state = ViewFeelState::default();
            for frame in 1..=100_000 {
                let out = evaluate(
                    &params,
                    4.0,
                    t.speed_reference,
                    true,
                    &mut state,
                    1.0 / 240.0,
                    1.0,
                );
                if out.tilt_roll >= target * target_fraction {
                    return frame;
                }
            }
            panic!("never reached target fraction");
        };

        let low = frames_to_fraction(8.0);
        let high = frames_to_fraction(24.0);
        assert!(
            high < low,
            "higher tension ({high}) should reach faster than lower ({low})"
        );
    }

    #[test]
    fn tilt_spring_is_frame_rate_independent() {
        // Advance to the same wall-clock time with many small vs few large
        // steps; the analytic spring must converge to the same roll.
        let params = tilt_only(tilt(15.0, true));
        let total_time = 0.5_f32;

        let roll_after = |dt: f32| -> f32 {
            let steps = (total_time / dt).round() as usize;
            let mut state = ViewFeelState::default();
            run_frames(&params, 4.0, 4.0, true, &mut state, dt, 1.0, steps).tilt_roll
        };

        let fine = roll_after(total_time / 600.0); // 600 small steps
        let coarse = roll_after(total_time / 15.0); // 15 large steps
        assert!(
            (fine - coarse).abs() < 1e-2,
            "fine ({fine}) and coarse ({coarse}) should converge"
        );
    }

    #[test]
    fn tilt_settles_toward_zero_when_airborne_and_grounded_only() {
        // Grounded-only tilt: build a roll on the ground, then go airborne with
        // the same lateral velocity. The target becomes zero and the spring
        // keeps stepping, so the roll decays toward level.
        let params = tilt_only(tilt(20.0, true));
        let mut state = ViewFeelState::default();
        run_frames(&params, 4.0, 4.0, true, &mut state, 1.0 / 120.0, 1.0, 2000);
        let grounded_roll = state.tilt_roll;
        assert!(
            grounded_roll.abs() > 1.0,
            "should have a real roll while grounded"
        );

        // Many airborne frames: roll settles toward zero (spring keeps stepping).
        let air = run_frames(&params, 4.0, 4.0, false, &mut state, 1.0 / 120.0, 1.0, 4000);
        assert!(
            air.tilt_roll.abs() < grounded_roll.abs() * 0.05,
            "roll decays airborne"
        );
    }

    // --- Sway --------------------------------------------------------------

    #[test]
    fn sway_is_bounded_by_effective_amplitude() {
        let s = sway(0.0, false);
        let params = sway_only(s);
        let mut state = ViewFeelState::default();
        // Sample over many frames; each axis is the normalized sum of sines and
        // must never exceed the effective amplitude (== amplitude here).
        for _ in 0..5000 {
            let out = evaluate(&params, 0.0, 0.0, true, &mut state, 1.0 / 120.0, 1.0);
            assert!(out.sway_yaw.abs() <= s.amplitude + EPSILON);
            assert!(out.sway_pitch.abs() <= s.amplitude + EPSILON);
            assert!(out.sway_roll.abs() <= s.amplitude + EPSILON);
        }
    }

    #[test]
    fn sway_is_nonzero_at_zero_speed_when_amplitude_positive() {
        let params = sway_only(sway(0.0, false));
        let mut state = ViewFeelState::default();
        let mut peak = 0.0_f32;
        for _ in 0..2000 {
            let out = evaluate(&params, 0.0, 0.0, true, &mut state, 1.0 / 120.0, 1.0);
            peak = peak
                .max(out.sway_yaw.abs())
                .max(out.sway_pitch.abs())
                .max(out.sway_roll.abs());
        }
        assert!(peak > 0.0, "ambient sway should move at rest");
    }

    #[test]
    fn sway_effective_amplitude_grows_with_speed_when_speed_scale_positive() {
        // Peak sway magnitude over a sweep must be larger at higher speed when
        // speed_scale > 0, and identical regardless of speed when speed_scale = 0.
        let peak = |s: SwayParams, speed: f32| -> f32 {
            let params = sway_only(s);
            let mut state = ViewFeelState::default();
            let mut p = 0.0_f32;
            for _ in 0..4000 {
                let out = evaluate(&params, speed, 0.0, true, &mut state, 1.0 / 240.0, 1.0);
                p = p.max(out.sway_yaw.abs());
            }
            p
        };

        let scaled = sway(0.5, false);
        let slow = peak(scaled, 0.0);
        let fast = peak(scaled, 6.0);
        assert!(
            fast > slow,
            "speed_scale > 0: faster ({fast}) > slower ({slow})"
        );

        let flat = sway(0.0, false);
        let flat_slow = peak(flat, 0.0);
        let flat_fast = peak(flat, 6.0);
        assert!(
            approx_eq(flat_slow, flat_fast),
            "speed_scale = 0: amplitude constant"
        );
    }

    #[test]
    fn sway_is_unaffected_by_airborne_when_grounded_only_false() {
        // Default sway gate is grounded_only = false: airborne sway still moves.
        let params = sway_only(sway(0.0, false));
        let mut state = ViewFeelState::default();
        let mut peak = 0.0_f32;
        for _ in 0..2000 {
            let out = evaluate(&params, 3.0, 0.0, false, &mut state, 1.0 / 120.0, 1.0);
            peak = peak.max(out.sway_yaw.abs());
        }
        assert!(peak > 0.0, "ungated sway moves even airborne");
    }

    #[test]
    fn sway_contributes_zero_when_airborne_and_grounded_only() {
        let params = sway_only(sway(0.0, true));
        let mut state = ViewFeelState::default();
        let out = run_frames(&params, 3.0, 0.0, false, &mut state, 1.0 / 120.0, 1.0, 100);
        assert!(approx_eq(out.sway_yaw, 0.0));
        assert!(approx_eq(out.sway_pitch, 0.0));
        assert!(approx_eq(out.sway_roll, 0.0));
    }

    // --- Global scale ------------------------------------------------------

    #[test]
    fn global_scale_zero_produces_zero_for_all_motions() {
        let params = ViewFeelParams {
            bob: Some(bob(false)),
            tilt: Some(tilt(15.0, false)),
            sway: Some(sway(0.5, false)),
        };
        let mut state = ViewFeelState::default();
        // Even with strong velocity, scale = 0 zeroes everything.
        let out = run_frames(&params, 8.0, 6.0, true, &mut state, 1.0 / 60.0, 0.0, 50);
        assert!(approx_eq(out.bob_vertical, 0.0));
        assert!(approx_eq(out.bob_lateral, 0.0));
        assert!(approx_eq(out.tilt_roll, 0.0));
        assert!(approx_eq(out.sway_roll, 0.0));
        assert!(approx_eq(out.sway_yaw, 0.0));
        assert!(approx_eq(out.sway_pitch, 0.0));
    }

    #[test]
    fn global_scale_one_produces_unscaled_values() {
        // Compare scale = 1 against an independently scaled reference: scaling by
        // 2.0 must double every channel relative to scale = 1.
        let params = ViewFeelParams {
            bob: Some(bob(false)),
            tilt: Some(tilt(15.0, false)),
            sway: Some(sway(0.5, false)),
        };

        let sample = |scale: f32| -> ViewFeelOutput {
            let mut state = ViewFeelState::default();
            run_frames(&params, 5.0, 3.0, true, &mut state, 1.0 / 120.0, scale, 137)
        };

        let unit = sample(1.0);
        let doubled = sample(2.0);
        assert!(approx_eq(doubled.bob_vertical, unit.bob_vertical * 2.0));
        assert!(approx_eq(doubled.bob_lateral, unit.bob_lateral * 2.0));
        assert!(approx_eq(doubled.tilt_roll, unit.tilt_roll * 2.0));
        assert!(approx_eq(doubled.sway_roll, unit.sway_roll * 2.0));
        assert!(approx_eq(doubled.sway_yaw, unit.sway_yaw * 2.0));
        assert!(approx_eq(doubled.sway_pitch, unit.sway_pitch * 2.0));
    }

    // --- frame_dt = 0 ------------------------------------------------------

    #[test]
    fn zero_frame_dt_leaves_integrator_state_unchanged() {
        let params = ViewFeelParams {
            bob: Some(bob(false)),
            tilt: Some(tilt(15.0, false)),
            sway: Some(sway(0.5, false)),
        };
        // Advance to a non-trivial state first.
        let mut state = ViewFeelState::default();
        run_frames(&params, 5.0, 3.0, true, &mut state, 1.0 / 60.0, 1.0, 30);
        let before = state;

        // A zero-dt frame must not advance bob phase, the spring, or the sway clock.
        evaluate(&params, 5.0, 3.0, true, &mut state, 0.0, 1.0);
        assert!(approx_eq(state.bob_phase, before.bob_phase));
        assert!(approx_eq(state.tilt_roll, before.tilt_roll));
        assert!(approx_eq(
            state.tilt_roll_velocity,
            before.tilt_roll_velocity
        ));
        assert!(approx_eq(state.sway_clock, before.sway_clock));
    }

    // --- Absent sub-objects ------------------------------------------------

    #[test]
    fn absent_bob_disables_only_bob() {
        let params = ViewFeelParams {
            bob: None,
            tilt: Some(tilt(15.0, false)),
            sway: Some(sway(0.5, false)),
        };
        let mut state = ViewFeelState::default();
        let out = run_frames(&params, 5.0, 4.0, true, &mut state, 1.0 / 120.0, 1.0, 300);
        assert!(approx_eq(out.bob_vertical, 0.0));
        assert!(approx_eq(out.bob_lateral, 0.0));
        // Tilt and sway still active.
        assert!(out.tilt_roll.abs() > 0.0);
        assert!(out.sway_yaw.abs() + out.sway_pitch.abs() + out.sway_roll.abs() > 0.0);
    }

    #[test]
    fn absent_tilt_disables_only_tilt() {
        let params = ViewFeelParams {
            bob: Some(bob(false)),
            tilt: None,
            sway: Some(sway(0.5, false)),
        };
        let mut state = ViewFeelState::default();
        let mut bob_peak = 0.0_f32;
        let mut sway_peak = 0.0_f32;
        let mut out = ViewFeelOutput::ZERO;
        for _ in 0..2000 {
            out = evaluate(&params, 5.0, 4.0, true, &mut state, 1.0 / 240.0, 1.0);
            bob_peak = bob_peak.max(out.bob_vertical.abs());
            sway_peak = sway_peak.max(out.sway_yaw.abs());
        }
        assert!(approx_eq(out.tilt_roll, 0.0), "tilt absent => zero roll");
        assert!(approx_eq(state.tilt_roll, 0.0), "spring untouched");
        assert!(bob_peak > 0.0, "bob still active");
        assert!(sway_peak > 0.0, "sway still active");
    }

    #[test]
    fn absent_sway_disables_only_sway() {
        let params = ViewFeelParams {
            bob: Some(bob(false)),
            tilt: Some(tilt(15.0, false)),
            sway: None,
        };
        let mut state = ViewFeelState::default();
        let mut bob_peak = 0.0_f32;
        let mut out = ViewFeelOutput::ZERO;
        for _ in 0..600 {
            out = evaluate(&params, 5.0, 4.0, true, &mut state, 1.0 / 120.0, 1.0);
            bob_peak = bob_peak.max(out.bob_vertical.abs());
        }
        assert!(approx_eq(out.sway_yaw, 0.0));
        assert!(approx_eq(out.sway_pitch, 0.0));
        assert!(approx_eq(out.sway_roll, 0.0));
        assert!(approx_eq(state.sway_clock, 0.0), "sway clock untouched");
        assert!(bob_peak > 0.0, "bob still active");
        assert!(out.tilt_roll.abs() > 0.0, "tilt still active");
    }

    // --- Camera-basis helpers ----------------------------------------------

    #[test]
    fn view_feel_inputs_horizontal_speed_drops_vertical_component() {
        // A pure-vertical velocity has zero horizontal speed; a mixed velocity
        // reports only its XZ magnitude.
        let (h_up, _) = view_feel_inputs(Vec3::new(0.0, 9.0, 0.0), Vec3::X);
        assert!(approx_eq(h_up, 0.0), "vertical-only velocity is zero speed");

        // 3-4-5 in XZ, plus arbitrary vertical that must not contribute.
        let (h_mixed, _) = view_feel_inputs(Vec3::new(3.0, 100.0, 4.0), Vec3::X);
        assert!(
            approx_eq(h_mixed, 5.0),
            "horizontal speed is the XZ magnitude"
        );
    }

    #[test]
    fn view_feel_inputs_lateral_is_signed_projection_onto_right() {
        // Right-strafe (velocity along +right) is positive; left-strafe negative;
        // purely forward motion (perpendicular to right) projects to zero.
        let right = Vec3::new(1.0, 0.0, 0.0);
        let (_, strafe_right) = view_feel_inputs(Vec3::new(4.0, 0.0, 0.0), right);
        assert!(strafe_right > 0.0, "right-strafe is positive lateral");

        let (_, strafe_left) = view_feel_inputs(Vec3::new(-4.0, 0.0, 0.0), right);
        assert!(strafe_left < 0.0, "left-strafe is negative lateral");
        assert!(approx_eq(strafe_right, -strafe_left), "sign symmetric");

        let (_, forward) = view_feel_inputs(Vec3::new(0.0, 0.0, -6.0), right);
        assert!(
            approx_eq(forward, 0.0),
            "forward motion has no lateral component"
        );
    }

    #[test]
    fn map_output_roll_sums_tilt_and_sway_in_radians() {
        let out = ViewFeelOutput {
            tilt_roll: 6.0,
            sway_roll: 4.0,
            ..ViewFeelOutput::ZERO
        };
        let (roll, _, _, _) = map_output_to_camera(&out, Vec3::X);
        assert!(
            approx_eq(roll, 10.0_f32.to_radians()),
            "roll is the degree sum in radians"
        );
    }

    #[test]
    fn map_output_folds_sway_yaw_pitch_as_radians() {
        let out = ViewFeelOutput {
            sway_yaw: 2.0,
            sway_pitch: -1.5,
            ..ViewFeelOutput::ZERO
        };
        let (_, yaw_offset, pitch_offset, _) = map_output_to_camera(&out, Vec3::X);
        assert!(approx_eq(yaw_offset, 2.0_f32.to_radians()));
        assert!(approx_eq(pitch_offset, (-1.5_f32).to_radians()));
    }

    #[test]
    fn map_output_eye_offset_maps_bob_onto_up_and_right() {
        // bob_vertical along world up (Y); bob_lateral along the supplied right.
        // Use a yaw-rotated right vector to confirm the lateral term follows the
        // basis, not world X.
        let right = Vec3::new(0.0, 0.0, -1.0); // camera right at yaw = +90deg
        let out = ViewFeelOutput {
            bob_vertical: 0.1,
            bob_lateral: 0.05,
            ..ViewFeelOutput::ZERO
        };
        let (_, _, _, eye) = map_output_to_camera(&out, right);
        assert!(approx_eq(eye.x, 0.0), "no world-X component for this basis");
        assert!(approx_eq(eye.y, 0.1), "bob_vertical along world up");
        assert!(approx_eq(eye.z, -0.05), "bob_lateral along camera right");
    }

    #[test]
    fn map_output_zero_produces_zero_roll_and_offset() {
        // The pass-through invariant in helper terms: a zeroed output maps to
        // zero roll/yaw/pitch and a zero eye offset regardless of basis.
        let (roll, yaw_offset, pitch_offset, eye) =
            map_output_to_camera(&ViewFeelOutput::ZERO, Vec3::new(0.3, 0.0, -0.7));
        assert!(approx_eq(roll, 0.0));
        assert!(approx_eq(yaw_offset, 0.0));
        assert!(approx_eq(pitch_offset, 0.0));
        assert_eq!(eye, Vec3::ZERO);
    }
}
