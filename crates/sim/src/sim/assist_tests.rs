//! The fixed-point hold conversion: required counts and increments.
//! See: context/lib/scripting.md §12 (Assist)

use super::*;

/// A constant required count n completes on exactly the n-th advancing tick
/// (I6): n increments reach one whole, n − 1 do not.
fn assert_completes_on_nth_tick(required_ticks: u32) {
    let step = increment(required_ticks);
    let n = u64::from(required_ticks);
    assert!(step * n >= PROGRESS_UNIT, "n = {n} must complete");
    assert!(
        step * (n - 1) < PROGRESS_UNIT,
        "n = {n} must not complete a tick early"
    );
}

#[test]
fn assist_required_count_and_increment_cover_zero_default_cap_and_fractional_holds() {
    // (holdMs, required count): 0 completes on the first held tick; 3000 is
    // the default; 60000 is the cap; 100 ms is 5.9998 ticks, rounded up.
    for (hold_ms, expected) in [
        (0.0, 1),
        (-0.0, 1),
        (3000.0, 180),
        (60000.0, 3600),
        (100.0, 6),
    ] {
        let required = required_count(hold_ms);
        assert_eq!(required, expected, "holdMs {hold_ms}");
        assert_completes_on_nth_tick(required);
    }
    assert_eq!(increment(1), PROGRESS_UNIT);
    assert_eq!(increment(180), PROGRESS_UNIT.div_ceil(180));
    // The "kept, not advanced" sentinel never advances.
    assert_eq!(increment(0), 0);
}

#[test]
fn assist_every_required_count_up_to_the_cap_completes_on_its_nth_tick() {
    for required in 1..=required_count(postretro_foundation::ASSIST_MAX_HOLD_MS) {
        assert_completes_on_nth_tick(required);
    }
}
