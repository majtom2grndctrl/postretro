//! Ambient sway evaluation.

use std::f32::consts::TAU;

use postretro_foundation::SwayParams;

use super::ViewFeelState;

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

/// Ambient sway: summed incommensurate sines per axis (yaw, pitch, roll), each
/// scaled by an effective amplitude that grows with speed. Returns
/// `(yaw, pitch, roll)` in degrees (pre-scale) and advances the sway clock.
pub(super) fn evaluate(
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
