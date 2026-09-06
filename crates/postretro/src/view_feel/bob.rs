//! Distance-phased head-bob evaluation.

use std::f32::consts::TAU;

use postretro_foundation::BobParams;

use super::ViewFeelState;

/// Speed band (m/s) above `speed_threshold` over which bob eases in from 0 to
/// full amplitude. Exposed so the bob acceptance test references this band
/// rather than guessing the saturation speed. A small band keeps the onset
/// feeling responsive without a hard pop at the threshold.
pub(crate) const BOB_EASE_IN_BAND: f32 = 1.0;

/// Head bob: a distance-phased oscillator that self-gates below a speed
/// threshold. Returns `(vertical, lateral)` offsets in metres (pre-scale).
pub(super) fn evaluate(
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

    // Advance each phase by distance travelled this frame. Frequencies are
    // cycles per metre, so a full cycle elapses per `1/frequency` metres.
    let distance = horizontal_speed * frame_dt;
    state.bob_vertical_phase =
        (state.bob_vertical_phase + distance * bob.vertical_frequency * TAU).rem_euclid(TAU);
    state.bob_lateral_phase =
        (state.bob_lateral_phase + distance * bob.lateral_frequency * TAU).rem_euclid(TAU);

    // Ease in from 0 at the threshold to 1 over BOB_EASE_IN_BAND m/s above it,
    // so amplitude ramps in rather than popping on at the gate.
    let ease = ((horizontal_speed - bob.speed_threshold) / BOB_EASE_IN_BAND).clamp(0.0, 1.0);

    let vertical = state.bob_vertical_phase.sin() * bob.vertical_amplitude * ease;
    let lateral = state.bob_lateral_phase.sin() * bob.lateral_amplitude * ease;
    (vertical, lateral)
}
