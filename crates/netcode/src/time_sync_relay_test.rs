// Time sync over the relay: the min-RTT gate under the dev profile, a probe lost
// once, and a two-second outage in both directions.
// See: context/lib/networking.md §Time sync · context/lib/testing_guide.md
//
// The probe and its echo ride the production reliable-ordered Input channel at
// its 10 ms resend. Each echo's fate is read from the estimator's observation
// log, so a test sees which samples folded and whether a reseed fired.

#![cfg(test)]

use std::collections::HashMap;

use postretro_net::harness::{LinkConfig, ScriptedDrop};
use postretro_net::timesync::{
    DEFAULT_MICROS_PER_TICK, EchoFate, EchoOutcome, RTT_WINDOW_SAMPLES, gate_tolerance_us,
};
use postretro_net::transport::Channel;

use super::predict_reconcile_harness_test_fixtures::idle_command;
use super::predict_reconcile_relay_test_fixtures::{RelayHarness, dev_profile};

/// Frames the client may take to join and start predicting.
const JOIN_FRAMES_MAX: u64 = 600;

fn dev_links(seed: u64) -> (LinkConfig, LinkConfig) {
    (dev_profile(seed), dev_profile(seed.wrapping_add(1)))
}

/// The profile's base one-way delay with no jitter and no loss.
fn fixed_delay_link(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 0,
        loss_probability: 0.0,
        seed,
    }
}

fn joined(to_server: LinkConfig, to_client: LinkConfig) -> RelayHarness {
    let mut h = RelayHarness::new(to_server, to_client);
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    h
}

fn fates(h: &RelayHarness) -> Vec<EchoFate> {
    h.time_sync().estimator.echo_fates().copied().collect()
}

fn last_issued(h: &RelayHarness) -> Option<u32> {
    h.time_sync().estimator.last_issued_id()
}

/// How far the client's server-tick estimate sits from the host's tick this
/// frame. Both advance one tick per frame, so it moves only when the estimate
/// folds or reseeds a sample.
fn offset_error_ticks(h: &RelayHarness) -> f64 {
    let estimate = h
        .time_sync()
        .estimated_server_tick()
        .expect("time sync is initialized");
    estimate - f64::from(h.host_tick)
}

const TRACK_SEED: u64 = 0x1507;
/// Thirty seconds of probes: 150 at the 5 Hz cadence.
const TRACK_FRAMES: u64 = 30 * 60;

// Regression guard for the min-RTT gate under the profile: the smoothed RTT
// stays within one gate tolerance of the true round trip. True RTT is the probe
// round trip the relay realizes (conditioner arrival plus poll quantization),
// timed here from the frame that sent the probe to the frame whose poll handed
// its echo over, and compared at the minimum of the last window of probes.
#[test]
fn rtt_estimate_tracks_the_realized_round_trip_under_dev_profile() {
    let (to_server, to_client) = dev_links(TRACK_SEED);
    let mut h = joined(to_server, to_client);
    let mut sent_at: HashMap<u32, u64> = HashMap::new();
    let mut issued = last_issued(&h);
    let mut seen = fates(&h).len();
    let mut realized: Vec<f64> = Vec::new();
    let mut compared = 0;
    let mut worst: Option<(f64, f64, f64)> = None;
    for _ in 0..TRACK_FRAMES {
        h.frame(&idle_command());
        let now = h.client_clock_us();
        let newly_issued = last_issued(&h);
        if newly_issued != issued {
            issued = newly_issued;
            sent_at.insert(newly_issued.expect("a probe was issued"), now);
        }
        let all = fates(&h);
        let mut arrived = false;
        for fate in &all[seen..] {
            assert_ne!(
                fate.outcome,
                EchoOutcome::Rejected,
                "no echo arrives twice or unsent: {fate:?}"
            );
            let Some(&send) = sent_at.get(&fate.sample_id) else {
                continue;
            };
            let trip = (now - send) as f64;
            assert_eq!(
                trip, fate.rtt_us,
                "the estimator measures the round trip the relay realized"
            );
            realized.push(trip);
            arrived = true;
        }
        seen = all.len();
        if !arrived || realized.len() < RTT_WINDOW_SAMPLES {
            continue;
        }
        let window_min = realized[realized.len() - RTT_WINDOW_SAMPLES..]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let estimate = h
            .time_sync()
            .rtt_micros()
            .expect("time sync is initialized");
        let tolerance = gate_tolerance_us(window_min);
        let error = estimate - window_min;
        compared += 1;
        if worst.is_none_or(|(worst_error, _, _)| error.abs() > worst_error.abs()) {
            worst = Some((error, window_min, tolerance));
        }
        assert!(
            error.abs() <= tolerance,
            "smoothed RTT {estimate:.0} us is {error:.0} us from the true window minimum \
             {window_min:.0} us, past the {tolerance:.0} us gate tolerance"
        );
    }
    let (error, window_min, tolerance) = worst.expect("the run compares the estimate");
    eprintln!(
        "[rtt tracking] {compared} comparisons over {} probes; worst error {error:.0} us \
         at a {window_min:.0} us true minimum (tolerance {tolerance:.0} us)",
        realized.len()
    );
    assert!(
        compared >= 100,
        "the run compares most of its probes: {compared}"
    );
}

/// A probe on a fixed-delay link and what came back for it.
struct ProbeTrip {
    sample_id: u32,
    /// Every echo the estimator saw for the probe.
    echoes: Vec<EchoFate>,
}

/// Wait for the client's next probe after a settled join, optionally lose the
/// Input datagram that carries it once, then let it come back.
fn probe_trip(drop_probe: bool) -> ProbeTrip {
    let mut h = joined(fixed_delay_link(0x1508), fixed_delay_link(0x1509));
    for _ in 0..120 {
        h.frame(&idle_command());
    }
    let before = last_issued(&h);
    let sample_id = (0..30)
        .find_map(|_| {
            h.frame(&idle_command());
            let now = last_issued(&h);
            (now != before).then_some(now).flatten()
        })
        .expect("the 5 Hz cadence sends a probe within 30 frames");
    if drop_probe {
        // The probe leaves at the next poll, in the Input datagram carrying
        // this frame's command.
        let index = h.to_server.enqueued_on_channel(Channel::Input);
        h.to_server.script_drop(ScriptedDrop::NthOnChannel {
            channel: Channel::Input,
            index,
        });
    }
    for _ in 0..60 {
        h.frame(&idle_command());
    }
    if drop_probe {
        assert_eq!(h.to_server.scripted_dropped(), 1);
        assert_eq!(
            h.to_server.dropped(),
            1,
            "the scripted drop is the only loss"
        );
    }
    ProbeTrip {
        sample_id,
        echoes: fates(&h)
            .into_iter()
            .filter(|fate| fate.sample_id == sample_id)
            .collect(),
    }
}

// P14: renet resends the lost probe on the next poll, so it costs one fixed tick
// on its round trip, and its id dedupes the copies down to one echo.
#[test]
fn probe_lost_once_yields_one_echo_at_most_one_tick_late() {
    let unlost = probe_trip(false);
    let lost = probe_trip(true);
    assert_eq!(
        lost.sample_id, unlost.sample_id,
        "the runs match up to the drop"
    );
    let [unlost_echo] = unlost.echoes.as_slice() else {
        panic!("the unlost probe yields one echo: {:?}", unlost.echoes);
    };
    let [lost_echo] = lost.echoes.as_slice() else {
        panic!("the lost probe yields one echo: {:?}", lost.echoes);
    };
    assert_ne!(lost_echo.outcome, EchoOutcome::Rejected);
    let inflation = lost_echo.rtt_us - unlost_echo.rtt_us;
    eprintln!(
        "[lost probe] unlost RTT {:.0} us, lost-once RTT {:.0} us: +{inflation:.0} us",
        unlost_echo.rtt_us, lost_echo.rtt_us
    );
    assert!(inflation > 0.0, "the loss delayed the probe");
    assert!(
        inflation <= DEFAULT_MICROS_PER_TICK as f64,
        "the lost probe's RTT is {inflation:.0} us above the unlost one, past one tick"
    );
}

const OUTAGE_SEED: u64 = 0x150a;
/// Six seconds of probing settle the window and the estimate before the cut.
const OUTAGE_SETTLE_FRAMES: u64 = 6 * 60;
const OUTAGE_MS: u64 = 2_000;
/// The outage and six seconds after it, so its probes roll out of the window.
const OUTAGE_RUN_FRAMES: u64 = 8 * 60;

/// One run of the outage course: every echo, and the offset error each frame
/// from the cut on.
struct OutageRun {
    fates: Vec<EchoFate>,
    /// The client clock at the cut and when the link returns.
    outage_us: (u64, u64),
    /// The offset error on the frame the cut was scripted.
    error_at_cut: f64,
    /// The offset error each frame after it.
    errors: Vec<f64>,
    participating: bool,
}

fn outage_run(outage: bool) -> OutageRun {
    let (to_server, to_client) = dev_links(OUTAGE_SEED);
    let mut h = joined(to_server, to_client);
    for _ in 0..OUTAGE_SETTLE_FRAMES {
        h.frame(&idle_command());
    }
    let cut_us = h.client_clock_us();
    if outage {
        h.script_outage(OUTAGE_MS);
    }
    let error_at_cut = offset_error_ticks(&h);
    let errors = (0..OUTAGE_RUN_FRAMES)
        .map(|_| {
            h.frame(&idle_command());
            offset_error_ticks(&h)
        })
        .collect();
    OutageRun {
        fates: fates(&h),
        outage_us: (cut_us, cut_us + OUTAGE_MS * 1000),
        error_at_cut,
        errors,
        participating: h.both_participating(),
    }
}

// P13: the outage's probes land in the window beside pre-outage samples, so the
// pre-outage minimum holds, every probe the outage lengthened past the gate
// tolerance is gated out, no reseed fires, and the offset stays within a tick.
// A probe sent within one tolerance of the link's return is lengthened by less
// and folds by design; its lengthening is measured against the same seed with
// no outage.
#[test]
fn two_second_outage_keeps_offset_within_a_tick_and_gates_lengthened_probes() {
    let unlost = outage_run(false);
    let run = outage_run(true);
    assert!(run.participating, "the outage disconnects nobody");
    let unlost_rtt: HashMap<u32, f64> = unlost
        .fates
        .iter()
        .filter(|fate| fate.outcome != EchoOutcome::Rejected)
        .map(|fate| (fate.sample_id, fate.rtt_us))
        .collect();

    let (cut_us, return_us) = run.outage_us;
    let mut gated = 0;
    let mut folded = Vec::new();
    for fate in &run.fates {
        if fate.recv_us >= cut_us {
            assert_ne!(
                fate.outcome,
                EchoOutcome::Reseeded,
                "no reseed fires across the outage: {fate:?}"
            );
        }
        let spans_outage = fate.send_us < return_us && fate.recv_us > cut_us;
        if !spans_outage {
            continue;
        }
        let unlost = *unlost_rtt
            .get(&fate.sample_id)
            .unwrap_or_else(|| panic!("probe {} came back with no outage", fate.sample_id));
        let lengthened = fate.rtt_us - unlost;
        let window_min = fate
            .window_min_us
            .expect("an accepted echo joins the window");
        let tolerance = gate_tolerance_us(window_min);
        match fate.outcome {
            EchoOutcome::Gated => gated += 1,
            EchoOutcome::Folded => {
                folded.push((fate.sample_id, lengthened));
                assert!(
                    lengthened <= tolerance,
                    "probe {} folded although the outage lengthened it {lengthened:.0} us, \
                     past the {tolerance:.0} us tolerance",
                    fate.sample_id
                );
            }
            outcome => panic!("probe {} across the outage was {outcome:?}", fate.sample_id),
        }
    }
    let drift = run
        .errors
        .iter()
        .map(|error| error - run.error_at_cut)
        .fold(0.0_f64, |worst, drift| {
            if drift.abs() > worst.abs() {
                drift
            } else {
                worst
            }
        });
    eprintln!(
        "[outage time sync] offset error vs host tick at the cut {:.3} ticks, worst drift \
         {drift:.3} ticks; {gated} outage probes gated, folded (id, lengthened us) {folded:?}",
        run.error_at_cut
    );
    assert!(
        gated >= 8,
        "the outage lengthens most of its ten probes past the gate: {gated} gated"
    );
    assert!(
        drift.abs() <= 1.0,
        "the server-tick offset drifted {drift:.3} ticks across the outage"
    );
}
