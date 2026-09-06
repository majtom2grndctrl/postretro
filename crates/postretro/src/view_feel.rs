// Pure first-person view-feel evaluator: head bob, strafe tilt, ambient sway.
// Render-rate, pawn-driven, owns no GPU or camera basis — the caller maps the
// scalar outputs onto its camera basis at the render-assembly site in `main.rs`.
// See: context/lib/movement.md

use glam::Vec3;

use postretro_foundation::{ImpulseChannels, ViewFeelParams};

use crate::movement::MovementStateEdge;

#[cfg(test)]
use postretro_foundation::{
    BobParams, ImpulseParams, ImpulseStateParams, ImpulseStates, MovementStateKind, SwayParams,
    TiltParams,
};

mod bob;
mod impulse;
mod sway;
mod tilt;

#[cfg(test)]
pub(crate) use bob::BOB_EASE_IN_BAND;

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
    /// Vertical head-bob oscillator phase (radians). Advanced by distance
    /// travelled; held when bob is gated off.
    pub(crate) bob_vertical_phase: f32,
    /// Lateral head-bob oscillator phase (radians). Independent from the
    /// vertical cadence and held when bob is gated off.
    pub(crate) bob_lateral_phase: f32,
    /// Ambient-sway clock (seconds). Advanced by frame time only.
    pub(crate) sway_clock: f32,
    /// One critically-damped transient spring per closed movement-state key.
    /// The state remains app-owned and presentation-only, never replicated.
    impulse_springs: [impulse::ImpulseSpring; 4],
}

impl Default for ViewFeelState {
    fn default() -> Self {
        Self {
            tilt_roll: 0.0,
            tilt_roll_velocity: 0.0,
            bob_vertical_phase: 0.0,
            bob_lateral_phase: 0.0,
            sway_clock: 0.0,
            impulse_springs: [impulse::ImpulseSpring::ZERO; 4],
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
    /// State-transition FOV displacement in degrees. Camera projection owns
    /// the final clamp/application; this output remains presentation-only.
    pub(crate) impulse_fov: f32,
    /// State-transition pitch displacement in degrees.
    pub(crate) impulse_pitch: f32,
    /// State-transition roll displacement in degrees.
    pub(crate) impulse_roll: f32,
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
        impulse_fov: 0.0,
        impulse_pitch: 0.0,
        impulse_roll: 0.0,
    };
}

/// A tick-produced movement edge annotated with how long ago its fixed tick
/// ended in the current render frame. Catch-up frames preserve every edge and
/// age it before it joins its state spring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TimedMovementEdge {
    pub(crate) edge: MovementStateEdge,
    pub(crate) age: f32,
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
#[cfg(test)]
pub(crate) fn evaluate(
    params: &ViewFeelParams,
    horizontal_speed: f32,
    lateral_velocity: f32,
    is_grounded: bool,
    state: &mut ViewFeelState,
    frame_dt: f32,
    global_scale: f32,
) -> ViewFeelOutput {
    evaluate_with_edges(
        params,
        horizontal_speed,
        lateral_velocity,
        is_grounded,
        &[],
        state,
        frame_dt,
        global_scale,
    )
}

/// As [`evaluate`], additionally consuming the frame's ordered local movement
/// edges for state-transition impulse presentation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_with_edges(
    params: &ViewFeelParams,
    horizontal_speed: f32,
    lateral_velocity: f32,
    is_grounded: bool,
    movement_edges: &[TimedMovementEdge],
    state: &mut ViewFeelState,
    frame_dt: f32,
    global_scale: f32,
) -> ViewFeelOutput {
    let (bob_vertical, bob_lateral) = match &params.bob {
        Some(bob) => bob::evaluate(bob, horizontal_speed, is_grounded, state, frame_dt),
        None => (0.0, 0.0),
    };

    let tilt_roll = match &params.tilt {
        Some(tilt) => tilt::evaluate(tilt, lateral_velocity, is_grounded, state, frame_dt),
        // No tilt spring: hold the roll at rest so a later-enabled tilt does not
        // inherit stale velocity. (`None` means the motion is absent entirely.)
        None => 0.0,
    };

    let (sway_yaw, sway_pitch, sway_roll) = match &params.sway {
        Some(sway) => sway::evaluate(sway, horizontal_speed, is_grounded, state, frame_dt),
        None => (0.0, 0.0, 0.0),
    };

    let impulse = match &params.impulse {
        Some(impulse) => impulse::evaluate(impulse, movement_edges, state, frame_dt),
        None => {
            // A descriptor that removes impulse must not leave a prior
            // presentation displacement in flight.
            state.impulse_springs = [impulse::ImpulseSpring::ZERO; 4];
            ImpulseChannels {
                fov: 0.0,
                pitch: 0.0,
                roll: 0.0,
            }
        }
    };

    let output = ViewFeelOutput {
        bob_vertical: bob_vertical * global_scale,
        bob_lateral: bob_lateral * global_scale,
        tilt_roll: tilt_roll * global_scale,
        sway_roll: sway_roll * global_scale,
        sway_yaw: sway_yaw * global_scale,
        sway_pitch: sway_pitch * global_scale,
        impulse_fov: impulse.fov * global_scale,
        impulse_pitch: impulse.pitch * global_scale,
        impulse_roll: impulse.roll * global_scale,
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
/// render chokepoint (`camera::RenderCamera`) consumes. Kept pure
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
    let roll = (output.tilt_roll + output.sway_roll + output.impulse_roll).to_radians();
    let yaw_offset = output.sway_yaw.to_radians();
    let pitch_offset = (output.sway_pitch + output.impulse_pitch).to_radians();
    let eye_offset = Vec3::Y * output.bob_vertical + camera_right * output.bob_lateral;
    (roll, yaw_offset, pitch_offset, eye_offset)
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
            vertical_frequency: 1.0,
            lateral_frequency: 0.5,
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
            impulse: None,
        }
    }

    fn tilt_only(t: TiltParams) -> ViewFeelParams {
        ViewFeelParams {
            bob: None,
            tilt: Some(t),
            sway: None,
            impulse: None,
        }
    }

    fn sway_only(s: SwayParams) -> ViewFeelParams {
        ViewFeelParams {
            bob: None,
            tilt: None,
            sway: Some(s),
            impulse: None,
        }
    }

    fn impulse_params(states: ImpulseStates) -> ViewFeelParams {
        ViewFeelParams {
            bob: None,
            tilt: None,
            sway: None,
            impulse: Some(ImpulseParams {
                tension: 12.0,
                max: ImpulseChannels {
                    fov: 30.0,
                    pitch: 20.0,
                    roll: 20.0,
                },
                states,
            }),
        }
    }

    fn channels(fov: f32, pitch: f32, roll: f32) -> ImpulseChannels {
        ImpulseChannels { fov, pitch, roll }
    }

    fn state(enter: Option<ImpulseChannels>, exit: Option<ImpulseChannels>) -> ImpulseStateParams {
        ImpulseStateParams {
            tension: None,
            enter,
            exit,
        }
    }

    fn timed_edge(from: MovementStateKind, to: MovementStateKind, age: f32) -> TimedMovementEdge {
        TimedMovementEdge {
            edge: MovementStateEdge { from, to },
            age,
        }
    }

    /// Drive `evaluate` for `n` frames at a fixed `dt`, holding the inputs
    /// constant. Returns the final output and leaves the state advanced.
    #[allow(clippy::too_many_arguments)] // test harness threads the full evaluate context
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
    fn bob_vertical_and_lateral_frequencies_advance_independently() {
        let params = bob_only(bob(true));
        let mut state = ViewFeelState::default();

        // At 2 m/s for 0.25 s, distance is 0.5 m. The fixture's vertical
        // frequency (1 cycle/m) reaches PI while its lateral frequency
        // (0.5 cycles/m) reaches PI/2.
        let out = evaluate(&params, 2.0, 0.0, true, &mut state, 0.25, 1.0);

        assert!(approx_eq(out.bob_vertical, 0.0));
        assert!(approx_eq(out.bob_lateral, 0.05));
        assert!(approx_eq(state.bob_vertical_phase, std::f32::consts::PI));
        assert!(approx_eq(
            state.bob_lateral_phase,
            std::f32::consts::FRAC_PI_2
        ));
    }

    #[test]
    fn bob_holds_phase_and_outputs_zero_when_airborne_and_grounded_only() {
        let params = bob_only(bob(true));
        let mut state = ViewFeelState::default();
        // Advance a few grounded frames to build a non-trivial phase.
        run_frames(&params, 3.0, 0.0, true, &mut state, 1.0 / 60.0, 1.0, 5);
        let held_vertical_phase = state.bob_vertical_phase;
        let held_lateral_phase = state.bob_lateral_phase;
        assert!(
            held_vertical_phase != 0.0 && held_lateral_phase != 0.0,
            "phases should have advanced while grounded"
        );

        // Airborne: outputs zero AND phase does not advance.
        let air = evaluate(&params, 3.0, 0.0, false, &mut state, 1.0 / 60.0, 1.0);
        assert!(approx_eq(air.bob_vertical, 0.0));
        assert!(approx_eq(air.bob_lateral, 0.0));
        assert!(
            approx_eq(state.bob_vertical_phase, held_vertical_phase)
                && approx_eq(state.bob_lateral_phase, held_lateral_phase),
            "phases must hold airborne"
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
            impulse: None,
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
            impulse: None,
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
            impulse: None,
        };
        // Advance to a non-trivial state first.
        let mut state = ViewFeelState::default();
        run_frames(&params, 5.0, 3.0, true, &mut state, 1.0 / 60.0, 1.0, 30);
        let before = state;

        // A zero-dt frame must not advance bob phase, the spring, or the sway clock.
        evaluate(&params, 5.0, 3.0, true, &mut state, 0.0, 1.0);
        assert!(approx_eq(
            state.bob_vertical_phase,
            before.bob_vertical_phase
        ));
        assert!(approx_eq(state.bob_lateral_phase, before.bob_lateral_phase));
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
            impulse: None,
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
            impulse: None,
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
            impulse: None,
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

    // --- State-transition impulses ----------------------------------------

    #[test]
    fn impulse_transition_edges_sum_exit_and_entry_in_one_frame() {
        let params = impulse_params(ImpulseStates {
            normal: None,
            dash: None,
            crouch: Some(state(Some(channels(3.0, 2.0, 1.0)), None)),
            slide: Some(state(None, Some(channels(-4.0, 5.0, -2.0)))),
        });
        let mut state = ViewFeelState::default();
        let output = evaluate_with_edges(
            &params,
            0.0,
            0.0,
            true,
            &[timed_edge(
                MovementStateKind::Slide,
                MovementStateKind::Crouch,
                0.0,
            )],
            &mut state,
            0.0,
            1.0,
        );
        assert!(approx_eq(output.impulse_fov, -1.0));
        assert!(approx_eq(output.impulse_pitch, 7.0));
        assert!(approx_eq(output.impulse_roll, -1.0));
    }

    #[test]
    fn impulse_spring_is_monotonic_and_ages_backlog_edges() {
        let params = impulse_params(ImpulseStates {
            normal: None,
            dash: Some(state(Some(channels(10.0, 0.0, 0.0)), None)),
            crouch: None,
            slide: None,
        });
        let edge = timed_edge(MovementStateKind::Normal, MovementStateKind::Dash, 0.0);
        let mut fresh = ViewFeelState::default();
        let first =
            evaluate_with_edges(&params, 0.0, 0.0, true, &[edge], &mut fresh, 0.0, 1.0).impulse_fov;
        let mut prior = first;
        for _ in 0..120 {
            let next =
                evaluate_with_edges(&params, 0.0, 0.0, true, &[], &mut fresh, 1.0 / 120.0, 1.0)
                    .impulse_fov;
            assert!(
                next >= 0.0 && next <= prior + EPSILON,
                "critical damping must not rebound"
            );
            prior = next;
        }

        let mut aged = ViewFeelState::default();
        let aged_output = evaluate_with_edges(
            &params,
            0.0,
            0.0,
            true,
            &[TimedMovementEdge { age: 0.1, ..edge }],
            &mut aged,
            0.0,
            1.0,
        );
        assert!(aged_output.impulse_fov > 0.0 && aged_output.impulse_fov < first);
    }

    // Regression: catch-up presentation aged every queued edge by the render
    // delta twice.
    #[test]
    fn impulse_catch_up_matches_equivalent_separate_frame_progression() {
        let params = impulse_params(ImpulseStates {
            normal: None,
            dash: Some(state(Some(channels(9.0, 0.0, 0.0)), None)),
            crouch: Some(state(Some(channels(0.0, 7.0, 0.0)), None)),
            slide: Some(state(Some(channels(0.0, 0.0, 5.0)), None)),
        });
        let tick_dt = 1.0 / 60.0;
        let transitions = [
            (MovementStateKind::Normal, MovementStateKind::Dash),
            (MovementStateKind::Dash, MovementStateKind::Crouch),
            (MovementStateKind::Crouch, MovementStateKind::Slide),
        ];

        let catch_up_edges = [
            timed_edge(transitions[0].0, transitions[0].1, 2.0 * tick_dt),
            timed_edge(transitions[1].0, transitions[1].1, tick_dt),
            timed_edge(transitions[2].0, transitions[2].1, 0.0),
        ];
        let mut catch_up_state = ViewFeelState::default();
        let catch_up = evaluate_with_edges(
            &params,
            0.0,
            0.0,
            true,
            &catch_up_edges,
            &mut catch_up_state,
            3.0 * tick_dt,
            1.0,
        );

        let mut separate_state = ViewFeelState::default();
        let mut separate = ViewFeelOutput::ZERO;
        for (from, to) in transitions {
            separate = evaluate_with_edges(
                &params,
                0.0,
                0.0,
                true,
                &[timed_edge(from, to, 0.0)],
                &mut separate_state,
                tick_dt,
                1.0,
            );
        }

        assert!(approx_eq(catch_up.impulse_fov, separate.impulse_fov));
        assert!(approx_eq(catch_up.impulse_pitch, separate.impulse_pitch));
        assert!(approx_eq(catch_up.impulse_roll, separate.impulse_roll));
        assert!(
            approx_eq(catch_up.impulse_roll, 5.0),
            "the fresh final edge must present at its authored peak"
        );
    }

    #[test]
    fn impulse_clamps_presentation_but_scale_zero_keeps_integrating() {
        let mut params = impulse_params(ImpulseStates {
            normal: None,
            dash: Some(state(Some(channels(20.0, 0.0, 0.0)), None)),
            crouch: Some(state(Some(channels(20.0, 0.0, 0.0)), None)),
            slide: None,
        });
        params.impulse.as_mut().unwrap().max.fov = 5.0;
        let edges = [
            timed_edge(MovementStateKind::Normal, MovementStateKind::Dash, 0.0),
            timed_edge(MovementStateKind::Normal, MovementStateKind::Crouch, 0.0),
        ];
        let mut state = ViewFeelState::default();
        let muted = evaluate_with_edges(&params, 0.0, 0.0, true, &edges, &mut state, 0.0, 0.0);
        assert!(approx_eq(muted.impulse_fov, 0.0));
        let restored = evaluate_with_edges(&params, 0.0, 0.0, true, &[], &mut state, 0.1, 1.0);
        assert!(restored.impulse_fov > 0.0 && restored.impulse_fov <= 5.0);
        assert!(
            state
                .impulse_springs
                .iter()
                .any(|spring| spring.position.fov > 5.0)
        );
    }

    #[test]
    fn impulse_pitch_and_roll_map_without_moving_the_eye_and_reset_when_removed() {
        let params = impulse_params(ImpulseStates {
            normal: None,
            dash: Some(state(Some(channels(0.0, 3.0, -4.0)), None)),
            crouch: None,
            slide: None,
        });
        let edge = timed_edge(MovementStateKind::Normal, MovementStateKind::Dash, 0.0);
        let mut state = ViewFeelState::default();
        let output = evaluate_with_edges(&params, 0.0, 0.0, true, &[edge], &mut state, 0.0, 1.0);
        let (roll, _, pitch, eye) = map_output_to_camera(&output, Vec3::X);
        assert!(approx_eq(roll, (-4.0_f32).to_radians()));
        assert!(approx_eq(pitch, 3.0_f32.to_radians()));
        assert_eq!(eye, Vec3::ZERO);

        let no_impulse = ViewFeelParams {
            bob: None,
            tilt: None,
            sway: None,
            impulse: None,
        };
        let cleared = evaluate_with_edges(&no_impulse, 0.0, 0.0, true, &[], &mut state, 0.0, 1.0);
        assert!(approx_eq(cleared.impulse_fov, 0.0));
        assert!(
            state
                .impulse_springs
                .iter()
                .all(|spring| *spring == impulse::ImpulseSpring::ZERO)
        );
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
