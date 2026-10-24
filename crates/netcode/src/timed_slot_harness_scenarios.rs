use super::{
    assertions,
    fixtures::{Scenario, TimedSlotHarness},
};
use crate::sim::ReloadOutcome;

#[test]
fn actual_cell_regeneration_reaches_capacity_and_every_frame_stays_clamped() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Cell, true);
    harness.run_until_us(22_000_000);
    let counts = assertions::assert_quantitative_frames(&harness);
    assert!(
        counts.between > 100 && counts.bound > 100,
        "full-capacity ramp was not exercised: {counts:?}"
    );
    let full = harness
        .intakes
        .iter()
        .find(|s| s.raw[3] == Some(100.0))
        .expect("actual host regeneration must reach its authored capacity");
    assert_eq!(full.ramps[3].bound, Some(100.0));
    assert_eq!(
        full.ramps[3].target,
        Some(100.0),
        "capacity sample must clamp immediately at intake"
    );
    for frame in &harness.frames {
        if let Some(value) = frame.targets[3] {
            assert!(
                (0.0..=100.0).contains(&value),
                "cell exceeded capacity: {frame:?}"
            );
        }
    }
    assert_eq!(harness.frames.last().unwrap().targets[3], Some(100.0));
}

#[test]
fn real_completed_and_fall_snapshots_in_one_frame_are_both_sampled_in_order() {
    let mut harness = TimedSlotHarness::scenario(4, Scenario::Reload, false);
    harness.fixed_snapshot_delay();
    harness.run_until_us(6_000_000);
    let completed = harness
        .intakes
        .iter()
        .position(|s| s.raw[0] == Some(1.0))
        .expect("actual Completed snapshot delivered");
    let fall = harness.intakes[completed + 1..]
        .iter()
        .find(|s| s.raw[0] == Some(0.0))
        .expect("actual fall delivered");
    let endpoint = &harness.intakes[completed];
    assert_eq!(
        endpoint.now_us, fall.now_us,
        "fixture must drain Completed and fall on one client frame"
    );
    assert!(endpoint.tick < fall.tick);
    assert_eq!(
        endpoint.ramps[0].target,
        Some(1.0),
        "Completed sets bound at intake"
    );
    assert_eq!(
        fall.ramps[0].target,
        Some(0.0),
        "fall overrides the Completed target in order"
    );
    let frame = harness
        .frames
        .iter()
        .find(|f| f.now_us == fall.now_us)
        .unwrap();
    assert!(
        frame.applied_ticks.contains(&endpoint.tick) && frame.applied_ticks.contains(&fall.tick)
    );
    assert!(!frame.reload_active);
    assert_eq!(frame.targets[0], Some(0.0));
}

#[test]
fn completed_snapshot_overtaken_by_the_real_fall_never_becomes_a_sample() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, false);
    harness.fixed_snapshot_delay();
    harness.hold_completed_until_us = Some(5_500_000);
    harness.run_until_us(6_000_000);
    assert_eq!(
        harness.rejected_ticks.len(),
        1,
        "only the deliberately overtaken endpoint should hit newest gate"
    );
    let completion = harness
        .events
        .iter()
        .find(|e| matches!(e.outcome, ReloadOutcome::Completed { .. }))
        .unwrap();
    assert!((completion.tick..=completion.tick + 3).contains(&harness.rejected_ticks[0]));
    assert!(
        harness.intakes.iter().all(|s| s.raw[0] != Some(1.0)),
        "overtaken endpoint reached sample intake"
    );
    let fall = harness
        .frames
        .windows(2)
        .position(|p| p[0].reload_active && !p[1].reload_active)
        .unwrap()
        + 1;
    assert!(
        harness.frames[fall..]
            .iter()
            .all(|f| !f.reload_active && f.targets[0] == Some(0.0)),
        "old Completed resurrected meter"
    );
}

#[test]
fn a_lost_slope_change_retargets_when_the_next_real_timer_sample_breaks_agreement() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::SlowingHeat, false);
    harness.drop_ticks.push(121);
    harness.run_until_us(4_000_000);
    assert!(harness.intakes.iter().all(|s| s.tick != 121));
    let before = harness.intakes.iter().find(|s| s.tick == 119).unwrap();
    let after = harness.intakes.iter().find(|s| s.tick == 122).unwrap();
    assert!(before.ramps[2].slope.is_some());
    assert!(after.ramps[2].slope.is_none());
    assert_eq!(after.ramps[2].target, after.raw[2]);
    assert!(assertions::assert_quantitative_frames(&harness).breaks > 0);
}

#[test]
fn real_cooldown_reaches_zero_then_a_new_shot_resets_it_raw() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::CooldownReset, false);
    harness.run_until_us(7_000_000);
    assertions::assert_quantitative_frames(&harness);
    let reset = harness
        .intakes
        .windows(2)
        .find(|p| p[1].tick >= 360 && p[1].raw[1].unwrap() > p[0].raw[1].unwrap() + 1000.0)
        .unwrap();
    assert_eq!(reset[0].raw[1], Some(0.0));
    assert_eq!(reset[1].ramps[1].target, reset[1].raw[1]);
    assert!(reset[1].ramps[1].slope.is_none());
}

#[test]
fn long_real_cool_delay_samples_held_resends_omissions_and_empty_snapshots() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::HeatShots, false);
    harness.run_until_us(4_500_000);
    let delay: Vec<_> = harness
        .intakes
        .iter()
        .filter(|s| s.tick > 124 && s.tick < 175)
        .collect();
    assert!(delay.len() > 10);
    assert!(
        delay.iter().any(|s| s.held_resend[2]),
        "no held-id heat resend sampled"
    );
    assert!(
        delay.iter().any(|s| !s.timed_record_present[2]),
        "no omitted unchanged heat sampled"
    );
    assert!(
        delay.iter().any(|s| s.only_held_resends),
        "no entire held-resend batch sampled"
    );
    assert!(
        delay.iter().any(|s| s.records == 0),
        "no empty state batch sampled"
    );
    for sample in delay {
        assert_eq!(
            sample.ramps[2].newest_tick,
            Some(f64::from(sample.tick)),
            "held value missed header tick"
        );
        assert_eq!(sample.ramps[2].target, sample.raw[2]);
        assert!(
            sample.ramps[2].slope.is_none(),
            "a held cool delay extrapolated"
        );
    }
}

#[test]
fn a_single_loss_between_two_four_tick_gaps_preserves_the_real_reload_ramp() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, false);
    harness.drop_ticks.push(74);
    harness.run_until_us(3_000_000);
    let gap = harness
        .intakes
        .windows(2)
        .find(|p| p[0].tick == 70 && p[1].tick == 78)
        .expect("lost stamp 74 must join the adjacent four-tick gaps");
    assert!(gap[0].ramps[0].slope.is_some() && gap[1].ramps[0].slope.is_some());
    assert!(
        (gap[0].ramps[0].slope.unwrap() - gap[1].ramps[0].slope.unwrap()).abs()
            <= 0.08 * gap[0].ramps[0].slope.unwrap().abs()
    );
    let frames: Vec<_> = harness
        .frames
        .windows(2)
        .filter(|p| {
            p[1].now_us > gap[0].now_us
                && p[1].now_us < gap[1].now_us
                && p[1].applied_ticks.is_empty()
        })
        .collect();
    assert!(frames.len() > 5);
    assert!(frames.iter().all(|p| p[1].targets[0] >= p[0].targets[0]));
    assert!(frames.iter().any(|p| p[1].targets[0] > p[0].targets[0]));
    assertions::assert_quantitative_frames(&harness);
}

#[test]
fn actual_shots_delays_wraps_and_tuning_jumps_show_unagreed_samples_raw() {
    for scenario in [
        Scenario::HeatShots,
        Scenario::HeatDelay,
        Scenario::HeatJump,
        Scenario::SlowingHeat,
        Scenario::Shells,
        Scenario::Cancel,
    ] {
        let mut harness = TimedSlotHarness::scenario(144, scenario, false);
        harness.run_until_us(4_500_000);
        let index = scenario.meter();
        let counts = assertions::assert_quantitative_frames(&harness);
        assertions::assert_error_within_newest_sample_step(&harness);
        assert!(
            counts.between > 20,
            "{scenario:?}: did not resume a real timer ramp: {counts:?}"
        );
        let breaks = harness
            .intakes
            .windows(2)
            .filter(|pair| {
                pair[0].ramps[index].slope.is_some()
                    && pair[1].ramps[index].slope.is_none()
                    && pair[1].raw[index] != pair[0].raw[index]
            })
            .count();
        assert!(
            breaks > 0,
            "{scenario:?}: did not exercise an actual discontinuity or slope break"
        );
        if scenario == Scenario::HeatShots {
            let firing_samples: Vec<_> = harness
                .intakes
                .iter()
                .filter(|s| (62..=120).contains(&s.tick))
                .collect();
            assert!(firing_samples.len() > 10);
            for sample in firing_samples {
                assert!(
                    sample.ramps[2].slope.is_none(),
                    "regular per-shot steps extrapolated: {sample:?}"
                );
                assert_eq!(sample.ramps[2].target, sample.raw[2]);
            }
            assert!(
                harness.intakes.windows(2).any(|p| p[0].tick >= 60
                    && p[1].tick <= 120
                    && p[1].raw[2].unwrap() - p[0].raw[2].unwrap() >= 19.99),
                "no two-shot sample interval exercised"
            );
        }
        if scenario == Scenario::HeatJump {
            let jump = harness
                .intakes
                .windows(2)
                .find(|p| p[1].raw[2].unwrap() < p[0].raw[2].unwrap() - 10.0)
                .unwrap();
            assert!(jump[1].ramps[2].slope.is_none());
            assert_eq!(jump[1].ramps[2].target, jump[1].raw[2]);
        }
        if scenario == Scenario::Shells {
            assert!(
                harness
                    .events
                    .iter()
                    .any(|e| e.outcome == ReloadOutcome::ShellLoaded)
            );
            let wraps = harness
                .intakes
                .windows(2)
                .filter(|p| p[1].raw[0] < p[0].raw[0])
                .count();
            assert!(wraps >= 2, "per-shell boundaries did not wrap the meter");
        }
        if scenario == Scenario::Cancel {
            assert!(
                harness
                    .events
                    .iter()
                    .any(|e| matches!(e.outcome, ReloadOutcome::Cancelled { .. })),
                "no real host cancellation"
            );
            let fall = harness
                .frames
                .windows(2)
                .find(|p| p[0].reload_active && !p[1].reload_active)
                .unwrap();
            assert_eq!(fall[1].targets[0], fall[1].raw[0]);
            assert_eq!(fall[1].targets[0], Some(0.0));
        }
    }
}

#[test]
fn conditioned_reload_endpoints_stay_in_step_and_lost_endpoints_do_not_break_onset() {
    for (drop_started, drop_completed) in [(false, false), (true, false), (false, true)] {
        let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, true);
        harness.drop_started = drop_started;
        harness.drop_completed = drop_completed;
        harness.run_until_us(6_000_000);
        assertions::assert_quantitative_frames(&harness);
        let rise = harness
            .frames
            .windows(2)
            .find(|p| !p[0].reload_active && p[1].reload_active)
            .unwrap();
        assert_eq!(
            rise[1].targets[0], rise[1].raw[0],
            "flag rise must show first active sample"
        );
        if drop_started {
            assert!(harness.dropped_endpoints[0] > 0);
            assert!(rise[1].raw[0].unwrap() > 0.0);
        }
        let completed = harness
            .events
            .iter()
            .find(|e| matches!(e.outcome, ReloadOutcome::Completed { .. }))
            .unwrap();
        assert_eq!(completed.due_us, u64::from(completed.tick) * 1_000_000 / 60);
        assert!(completed.executed_us >= completed.due_us);
        let first_full = harness
            .frames
            .iter()
            .find(|f| f.reload_active && f.targets[0] == Some(1.0))
            .unwrap();
        assert!(
            first_full.now_us + 16_667 >= completed.due_us + 45_000,
            "completion projected too early: host due {completed:?}, full at {first_full:?}"
        );
        if let Some(endpoint) = harness
            .intakes
            .iter()
            .find(|sample| sample.raw[0] == Some(1.0))
        {
            assert!(
                first_full.now_us <= endpoint.now_us,
                "missed applied Completed endpoint"
            );
        } else {
            assert!(
                drop_completed,
                "ordinary run did not apply its completed endpoint"
            );
            let deadline = harness
                .frames
                .iter()
                .find(|f| {
                    f.playback_tick
                        .is_some_and(|t| t >= f64::from(completed.tick) + 1.0)
                        && f.now_us >= completed.due_us
                })
                .unwrap();
            assert!(
                first_full.now_us <= deadline.now_us,
                "lost completion was not reached within one timeline tick"
            );
        }
        if drop_completed {
            assert!(harness.dropped_endpoints[1] > 0);
        }
        let fall = harness
            .frames
            .windows(2)
            .find(|p| p[0].reload_active && !p[1].reload_active)
            .unwrap();
        assert_eq!(fall[1].targets[0], fall[1].raw[0]);
        assert_eq!(fall[1].targets[0], Some(0.0));
    }
}

#[test]
fn actual_overheat_cools_to_zero_in_step_with_the_raw_latch() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Overheat, true);
    harness.run_until_us(3_000_000);
    assertions::assert_quantitative_frames(&harness);
    let host_zero = harness
        .heat_zero_due_us
        .expect("real overheat must cool to zero");
    let rise = harness
        .frames
        .iter()
        .position(|f| f.overheated)
        .expect("crossing shot must latch");
    let first_zero = harness.frames[rise..]
        .iter()
        .find(|f| f.targets[2] == Some(0.0))
        .unwrap();
    assert!(
        first_zero.now_us + 16_667 >= host_zero + 45_000,
        "heat reached zero ahead of its sample timeline"
    );
    let latch_fall = harness.frames[rise..]
        .iter()
        .find(|f| !f.overheated)
        .unwrap();
    assert!(
        first_zero.now_us <= latch_fall.now_us,
        "projected heat lagged the latch-down sample"
    );
    assert_eq!(latch_fall.raw[2], Some(0.0));
}

#[test]
fn outage_caps_each_real_timer_then_recovers_at_the_new_sample_projection() {
    for scenario in [
        Scenario::Reload,
        Scenario::Cooldown,
        Scenario::Heat,
        Scenario::Cell,
    ] {
        let mut harness = TimedSlotHarness::scenario(144, scenario, true);
        harness.snapshot_outage = Some((2_000_000, 3_000_000));
        harness.run_until_us(4_000_000);
        let counts = assertions::assert_quantitative_frames(&harness);
        assert!(
            counts.capped > 30 && counts.recovery > 0,
            "{scenario:?}: {counts:?}"
        );
        let index = scenario.meter();
        let capped: Vec<_> = harness
            .frames
            .iter()
            .filter(|f| {
                f.ramps[index].cap_expired
                    && f.ramps[index].slope.is_some()
                    && f.now_us > 2_600_000
                    && f.now_us < 3_000_000
            })
            .collect();
        assert!(capped.len() > 30);
        for frame in capped {
            let ramp = frame.ramps[index];
            let projected = ramp.newest_value.unwrap() + ramp.slope.unwrap() * 16.0;
            let expected = if index == 0 || index == 3 {
                projected.min(ramp.bound.unwrap())
            } else {
                projected.max(0.0)
            };
            assert!(
                (frame.targets[index].unwrap() - expected).abs()
                    < if index == 1 { 0.5 } else { 0.001 },
                "{scenario:?}: cap is not newest + slope * 16: {frame:?}, expected {expected}"
            );
        }
    }
}

#[test]
fn duplicate_and_reordered_snapshots_are_not_samples_and_low_fps_projects_at_frame_time() {
    for hz in [144, 20] {
        let mut harness = TimedSlotHarness::scenario(hz, Scenario::Heat, true);
        harness.duplicate_packets = true;
        harness.run_until_us(4_000_000);
        assertions::assert_quantitative_frames(&harness);
        assert!(
            harness.frames.iter().any(|f| f.applied_ticks.len() >= 2),
            "no multi-snapshot client drain"
        );
        for pair in harness.intakes.windows(2) {
            assert!(
                pair[1].tick > pair[0].tick,
                "newest gate leaked old/duplicate snapshot"
            );
        }
        for frame in harness.frames.iter().filter(|f| f.now_us > 1_000_000) {
            assert_eq!(
                frame.ramps[2].samples,
                harness
                    .intakes
                    .iter()
                    .filter(|s| s.now_us <= frame.now_us)
                    .count()
            );
        }
        if hz == 20 {
            assert!(
                harness
                    .frames
                    .iter()
                    .filter(|f| f.targets[2] != f.raw[2] && f.ramps[2].slope.is_some())
                    .count()
                    > 20,
                "low-FPS frames showed the newest raw sample instead of their timeline projection"
            );
        }
    }
}

#[test]
fn first_real_sync_mid_ramp_preserves_the_raw_floor_then_advances() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, true);
    harness.sync_enabled_after_us = 2_000_000;
    harness.run_until_us(4_000_000);
    assertions::assert_quantitative_frames(&harness);
    let first_sync = harness
        .frames
        .iter()
        .position(|f| f.playback_tick.is_some())
        .unwrap();
    assert!(
        harness.frames[..first_sync]
            .iter()
            .all(|f| f.targets[0] == f.raw[0])
    );
    let shown = harness.frames[first_sync - 1].targets[0].unwrap();
    assert!(harness.frames[first_sync].targets[0].unwrap() >= shown);
    let after_two_intervals = harness
        .intakes
        .iter()
        .filter(|s| s.now_us > harness.frames[first_sync].now_us)
        .nth(1)
        .unwrap()
        .now_us;
    assert!(
        harness.frames[first_sync..]
            .windows(2)
            .any(|p| p[1].now_us <= after_two_intervals
                && p[1].applied_ticks.is_empty()
                && p[1].targets[0] > p[0].targets[0]),
        "projection did not start within two intervals after first echo"
    );
}
