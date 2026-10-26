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

// Regression: Completed and fall can both apply on one 144 Hz frame. The
// endpoint must reach 1 at intake before the ordered fall leaves the frame at 0.
#[test]
fn conditioned_reload_endpoints_stay_in_step_and_lost_endpoints_do_not_break_onset() {
    let mut applied_completed = 0;
    let mut same_frame_ordered_fall = 0;
    let mut lost_completed_deadline_before_fall = 0;
    for (drop_started, drop_completed, outage) in [
        (false, false, None),
        (true, false, None),
        (false, true, None),
        // Completion is due at tick 299 (~4.983 s). Keep the following fall
        // out until the timeline's tick-300 deadline, inside the 16-tick cap.
        (false, true, Some((4_970_000, 5_350_000))),
    ] {
        let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, true);
        harness.drop_started = drop_started;
        harness.drop_completed = drop_completed;
        harness.snapshot_outage = outage;
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
        let fall = &harness
            .frames
            .windows(2)
            .find(|p| p[0].reload_active && !p[1].reload_active)
            .unwrap()[1];
        let deadline = harness
            .frames
            .iter()
            .find(|frame| {
                frame.now_us >= completed.due_us
                    && frame
                        .playback_tick
                        .is_some_and(|tick| tick >= f64::from(completed.tick) + 1.0)
            })
            .expect("initialized timeline must cross the completion deadline");
        let first_full_frame = harness
            .frames
            .iter()
            .find(|frame| frame.reload_active && frame.targets[0] == Some(1.0));
        let first_full_intake = harness
            .intakes
            .iter()
            .find(|sample| sample.ramps[0].target == Some(1.0));
        let first_full_us = first_full_frame
            .map(|frame| frame.now_us)
            .into_iter()
            .chain(first_full_intake.map(|sample| sample.now_us))
            .min();
        if let Some(first_full_us) = first_full_us {
            assert!(
                first_full_us + 16_667 >= completed.due_us + 45_000,
                "completion projected too early: host={completed:?}, full={first_full_us}, drop_started={drop_started}, drop_completed={drop_completed}, outage={outage:?}"
            );
        }
        if let Some((position, endpoint)) = harness
            .intakes
            .iter()
            .enumerate()
            .find(|(_, sample)| sample.raw[0] == Some(1.0))
        {
            applied_completed += 1;
            assert_eq!(
                endpoint.ramps[0].target,
                Some(1.0),
                "Completed must set 1 at intake"
            );
            assert!(
                first_full_us.is_some_and(|at| at <= endpoint.now_us),
                "missed applied Completed endpoint"
            );
            let endpoint_frame = harness
                .frames
                .iter()
                .find(|frame| frame.now_us == endpoint.now_us)
                .unwrap();
            if endpoint_frame.targets[0] == Some(1.0) {
                assert!(first_full_frame.is_some_and(|frame| frame.now_us <= endpoint.now_us));
            } else {
                let same_frame_fall = harness.intakes[position + 1..]
                    .iter()
                    .find(|sample| sample.now_us == endpoint.now_us && sample.raw[0] == Some(0.0))
                    .expect("a Completed frame below 1 requires an ordered same-frame fall");
                assert!(endpoint.tick < same_frame_fall.tick);
                let completed_order = endpoint_frame
                    .applied_ticks
                    .iter()
                    .position(|tick| *tick == endpoint.tick)
                    .unwrap();
                let fall_order = endpoint_frame
                    .applied_ticks
                    .iter()
                    .position(|tick| *tick == same_frame_fall.tick)
                    .unwrap();
                assert!(completed_order < fall_order);
                assert_eq!(same_frame_fall.ramps[0].target, same_frame_fall.raw[0]);
                assert_eq!(endpoint_frame.targets[0], Some(0.0));
                assert_eq!(endpoint_frame.targets[0], endpoint_frame.raw[0]);
                assert!(!endpoint_frame.reload_active);
                same_frame_ordered_fall += 1;
            }
        } else {
            assert!(
                drop_completed,
                "ordinary run did not apply its Completed endpoint"
            );
            if fall.now_us > deadline.now_us {
                let first_full = first_full_frame.unwrap_or_else(|| panic!(
                    "lost Completed must project full before fall: completed={completed:?}, fall={fall:?}, deadline={deadline:?}, outage={outage:?}"
                ));
                assert!(
                    first_full.now_us <= deadline.now_us,
                    "lost completion missed one-timeline-tick deadline: full={first_full:?}, deadline={deadline:?}"
                );
                assert!(
                    first_full.raw[0].is_some_and(|raw| raw < 1.0),
                    "lost endpoint case must reach 1 by projection"
                );
                lost_completed_deadline_before_fall += 1;
            } else {
                // P6 explicitly lets a received fall retire the meter before
                // its missing endpoint's timeline deadline. It must be raw.
                assert!(
                    harness.intakes.iter().any(|sample| {
                        sample.now_us == fall.now_us
                            && sample.tick >= completed.tick
                            && sample.raw[0] == Some(0.0)
                            && fall.applied_ticks.contains(&sample.tick)
                    }),
                    "fall-first exemption requires an accepted post-completion fall sample"
                );
            }
            if outage.is_some() {
                assert!(
                    fall.now_us > deadline.now_us,
                    "outage must exercise the mandatory projected-full deadline"
                );
                let latest = harness
                    .intakes
                    .iter()
                    .rev()
                    .find(|sample| sample.now_us <= deadline.now_us)
                    .unwrap();
                assert!(
                    deadline.playback_tick.unwrap() - f64::from(latest.tick) < 16.0,
                    "deadline proof must precede the hold cap"
                );
            }
        }
        if drop_completed {
            if let Some((start, end)) = outage {
                // The outage drop short-circuits the explicit endpoint-drop
                // counter, so prove that the actual completion was suppressed.
                assert!((start..end).contains(&completed.executed_us));
                assert!(
                    harness
                        .intakes
                        .iter()
                        .all(|sample| sample.raw[0] != Some(1.0))
                );
            } else {
                assert!(harness.dropped_endpoints[1] > 0);
            }
        }
        assert_eq!(fall.targets[0], fall.raw[0]);
        assert_eq!(fall.targets[0], Some(0.0));
    }
    assert!(
        applied_completed > 0,
        "no applied Completed endpoint exercised"
    );
    assert!(
        same_frame_ordered_fall > 0,
        "no ordered Completed/fall frame exercised"
    );
    assert!(
        lost_completed_deadline_before_fall > 0,
        "no missing Completed forced projection by its deadline"
    );
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

// Regression: a partial Started interval delayed projection past the first two
// full, agreeing host intervals because the oracle trusted implementation state.
#[test]
fn partial_started_interval_projects_on_first_two_full_irregular_host_intervals() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, false);
    harness.fixed_snapshot_delay();
    harness.run_until_us(1_400_000);
    let samples: Vec<_> = [61, 62, 65, 68]
        .into_iter()
        .map(|tick| {
            harness
                .intakes
                .iter()
                .find(|sample| sample.tick == tick)
                .unwrap()
        })
        .collect();
    assert_eq!(samples[0].raw[0], Some(0.0), "tick 61 must replay Started");
    let slopes: Vec<_> = samples
        .windows(2)
        .map(|pair| {
            (pair[1].raw[0].unwrap() - pair[0].raw[0].unwrap())
                / f64::from(pair[1].tick - pair[0].tick)
        })
        .collect();
    assert!((slopes[0] - slopes[1]).abs() > 0.08 * slopes[0].min(slopes[1]));
    assert!((slopes[1] - slopes[2]).abs() <= 0.08 * slopes[1].min(slopes[2]));
    assert!(samples[2].ramps[0].slope.is_none());
    assert!(samples[3].ramps[0].slope.is_some());
    assertions::assert_quantitative_frames(&harness);
}

#[test]
fn quantitative_oracle_rejects_projection_delayed_past_first_qualifying_sample() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Reload, false);
    harness.fixed_snapshot_delay();
    harness.run_until_us(1_400_000);
    // Synthetic delayed-onset mutant: the real host observations stay intact,
    // while implementation traces claim that tick 68 is still unagreed/raw.
    let onset = harness.intakes.iter_mut().find(|s| s.tick == 68).unwrap();
    assert!(
        onset.ramps[0].slope.is_some(),
        "mutant must remove a real onset"
    );
    onset.ramps[0].slope = None;
    onset.ramps[0].target = onset.raw[0];
    onset.ramps[0].fed_tick = None;
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assertions::assert_quantitative_frames(&harness);
    }))
    .expect_err("oracle accepted a delayed-onset mutant");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("first qualifying host sample"),
        "wrong rejection: {message}"
    );
}

// Regression: target == capacity used to bypass every movement budget, allowing
// a projected cell value near 50 to jump to 100 without a host bound sample.
#[test]
fn quantitative_oracle_rejects_a_projection_jump_to_capacity_without_a_bound_sample() {
    let mut harness = TimedSlotHarness::scenario(144, Scenario::Cell, false);
    harness.run_until_us(10_200_000);
    let frame = harness
        .frames
        .iter_mut()
        .find(|frame| {
            frame.now_us >= 10_000_000
                && frame.applied_ticks.is_empty()
                && frame.raw[3].is_some_and(|value| (49.0..51.0).contains(&value))
                && frame.ramps[3].slope.is_some()
        })
        .expect("mutant needs a live half-capacity projection between samples");
    assert_eq!(frame.bounds[3], Some(100.0));
    frame.targets[3] = Some(100.0);
    frame.ramps[3].target = Some(100.0);
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assertions::assert_quantitative_frames(&harness);
    }))
    .expect_err("oracle accepted a jump from half capacity to the bound");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("between-sample") || message.contains("sample/feed"),
        "bound mutant must fail the ordinary frame budget: {message}"
    );
}
