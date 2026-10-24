// Timed weapon meters over production timers, snapshot cadence/newest gate and
// time sync. The fixture has independent host and presentation frame clocks.
// See: context/plans/in-progress/coop--timed-slot-projection/index.md

#[path = "timed_slot_harness_test_fixtures.rs"]
mod fixtures;

use fixtures::TimedSlotHarness;

#[test]
fn conditioned_reload_target_advances_between_samples() {
    let mut harness = TimedSlotHarness::new(144);
    harness.run_until_us(4_000_000);

    // Check the proof setup before the regression, so a broken fixture cannot
    // masquerade as the expected red result.
    harness.assert_proof_setup();
    let ramp_frames: Vec<_> = harness
        .frames
        .windows(2)
        .filter(|pair| {
            pair[1].now_us >= 2_000_000
                && pair[1].applied_ticks.is_empty()
                && pair[0].reload_active
                && pair[1].reload_active
                && pair[0].reload_target > 0.1
                && pair[1].reload_target < 0.9
        })
        .collect();
    assert!(
        ramp_frames.len() > 100,
        "must observe a sustained inter-sample ramp"
    );
    let advanced = ramp_frames
        .iter()
        .filter(|pair| pair[1].reload_target > pair[0].reload_target + 1.0e-6)
        .count();
    assert!(
        advanced > 0,
        "reload target held on all {} non-sample client frames despite a live host reload, \
         initialized conditioned time sync and 144 Hz presentation",
        ramp_frames.len(),
    );
}
