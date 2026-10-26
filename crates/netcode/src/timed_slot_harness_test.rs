// Timed weapon meters over production timers, snapshot cadence/newest gate and
// time sync. The fixture has independent host and presentation frame clocks.
// See: context/lib/networking.md §What gates, and what replicates instead
// and context/lib/testing_guide.md.

#[path = "timed_slot_harness_assertions.rs"]
mod assertions;
#[path = "timed_slot_harness_test_fixtures.rs"]
mod fixtures;
#[path = "timed_slot_harness_scenarios.rs"]
mod scenarios;

use fixtures::Scenario;
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

#[test]
fn conditioned_all_timed_ramps_obey_frame_bounds_using_host_sample_slopes() {
    for scenario in [
        Scenario::Reload,
        Scenario::Cooldown,
        Scenario::Heat,
        Scenario::Cell,
    ] {
        let mut harness = TimedSlotHarness::scenario(144, scenario, true);
        harness.run_until_us(4_000_000);
        let counts = assertions::assert_quantitative_frames(&harness);
        assert!(
            counts.onset > 0,
            "{scenario:?}: no agreement onset exercised: {counts:?}"
        );
        assert!(
            counts.between > 100,
            "{scenario:?}: insufficient inter-sample ramp: {counts:?}"
        );
        assert!(
            counts.sample_or_feed > 10,
            "{scenario:?}: no sample/feed frames: {counts:?}"
        );
        assert!(
            counts.advances > 100,
            "{scenario:?}: projection did not advance: {counts:?}"
        );
        assert!(
            harness.echo_fates.iter().any(|(at, fate)| *at > 1_000_000
                && *fate == postretro_net::timesync::EchoOutcome::Folded),
            "{scenario:?}: no folded in-ramp echo"
        );
    }
}

#[test]
fn real_clock_reseed_and_folds_slew_without_a_meter_retreat() {
    for scenario in [
        Scenario::Reload,
        Scenario::Cooldown,
        Scenario::Heat,
        Scenario::Cell,
    ] {
        let mut harness = TimedSlotHarness::scenario(144, scenario, true);
        // Keep the first genuine echo in flight until the reload/cooldown has
        // begun. Later real round trips use the ordinary profile and reseed.
        harness.stall_first_echo_until_us = Some(1_100_000);
        harness.run_until_us(4_000_000);
        assert!(
            harness.echo_fates.iter().any(|(at, fate)| *at > 1_000_000
                && *fate == postretro_net::timesync::EchoOutcome::Reseeded),
            "{scenario:?}: no actual reseed: {:?}",
            harness.echo_fates
        );
        assert!(
            harness.echo_fates.iter().any(|(at, fate)| *at > 1_000_000
                && *fate == postretro_net::timesync::EchoOutcome::Folded),
            "no actual fold"
        );
        assertions::assert_quantitative_frames(&harness);
        assertions::assert_playback_slew(&harness);
    }
}

#[test]
fn one_tick_host_frame_echoes_queued_probe_before_emitting_final_tick_snapshot() {
    TimedSlotHarness::assert_queued_probe_echoes_before_frame_ticks(1);
}

#[test]
fn three_tick_host_frame_echoes_queued_probe_before_emitting_final_tick_snapshot() {
    TimedSlotHarness::assert_queued_probe_echoes_before_frame_ticks(3);
}
