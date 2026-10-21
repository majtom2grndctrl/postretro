// Time sync over the relay: the min-RTT gate under the dev profile, a probe lost
// once, and a two-second outage in both directions.
// See: context/lib/networking.md §Time sync · context/lib/testing_guide.md
//
// The probe and its echo ride the production reliable-ordered Input channel at
// its 10 ms resend. Each echo's fate is read from the estimator's observation
// log, so a test sees which samples folded and whether a reseed fired; the
// round trip the link realized is read from the conditioners.

#![cfg(test)]

use std::collections::{BTreeMap, HashMap};

use postretro_net::harness::{LinkConfig, PacketConditioner, VirtualMillis};
use postretro_net::timesync::{
    DEFAULT_MICROS_PER_TICK, EchoFate, EchoOutcome, RTT_WINDOW_SAMPLES, gate_tolerance_us,
};
use postretro_net::transport::Channel;
use postretro_net::wire::{self, ClientMessage, ServerMessage};

use super::SERVER_TICK_MICROS;
use super::predict_reconcile_harness_test_fixtures::idle_command;
use super::predict_reconcile_relay_link_test_fixtures::{
    LoggedDatagram, OUTAGE_MS, decode_client_input, dev_links, first_poll_frame, fixed_delay_link,
    frame_start_us, input_messages, largest_one_way_us,
};
use super::predict_reconcile_relay_test_fixtures::RelayHarness;

fn joined(to_server: LinkConfig, to_client: LinkConfig) -> RelayHarness {
    let mut h = RelayHarness::new(to_server, to_client);
    h.join();
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

/// When each Input message on one direction of the link first reached its
/// receiver, by renet message id: the earliest arrival among its copies, the
/// first send and every resend. `None` while every copy so far was lost.
fn first_arrivals(
    log: &[LoggedDatagram],
    link: &PacketConditioner,
) -> BTreeMap<u64, Option<VirtualMillis>> {
    let mut arrivals: BTreeMap<u64, Option<VirtualMillis>> = BTreeMap::new();
    for datagram in log {
        let messages = input_messages(datagram);
        if messages.is_empty() {
            continue;
        }
        let index = datagram
            .index
            .expect("an Input datagram has a channel index");
        let arrives_at = link
            .channel_datagram(Channel::Input, index)
            .expect("a logged datagram was enqueued")
            .arrives_at;
        for (id, _) in messages {
            let first = arrivals.entry(*id).or_insert(None);
            *first = match (*first, arrives_at) {
                (Some(earlier), Some(arrival)) => Some(earlier.min(arrival)),
                (earlier, arrival) => earlier.or(arrival),
            };
        }
    }
    let ids: Vec<u64> = arrivals.keys().copied().collect();
    assert!(
        ids.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "the log holds every Input message from the first one flushed"
    );
    arrivals
}

/// The frame whose poll hands Input message `id` over. Input is
/// reliable-ordered, so the message waits for itself and every earlier message
/// to arrive, then for the receiver's next poll.
fn handed_over_frame(arrivals: &BTreeMap<u64, Option<VirtualMillis>>, id: u64) -> u64 {
    assert!(arrivals.contains_key(&id), "Input message {id} was flushed");
    let released = arrivals
        .range(..=id)
        .map(|(earlier, arrival)| {
            arrival
                .unwrap_or_else(|| panic!("Input message {earlier}, at or ahead of {id}, arrived"))
        })
        .max()
        .expect("the range holds the message itself");
    first_poll_frame(released)
}

/// The uplink message id carrying each probe and the frame whose poll first
/// flushed it, by sample id.
fn probe_messages(h: &RelayHarness) -> HashMap<u32, (u64, u64)> {
    let mut ids = HashMap::new();
    for datagram in &h.up_log {
        for (id, bytes) in input_messages(datagram) {
            if let Some(ClientMessage::TimeSync(probe)) = decode_client_input(bytes) {
                ids.entry(probe.sample_id).or_insert((*id, datagram.frame));
            }
        }
    }
    ids
}

/// The downlink message id carrying each echo and the frame whose poll first
/// flushed it, by sample id.
fn echo_messages(h: &RelayHarness) -> HashMap<u32, (u64, u64)> {
    let mut ids = HashMap::new();
    for datagram in &h.down_log {
        for (id, bytes) in input_messages(datagram) {
            if let Ok(ServerMessage::TimeSync(echo)) = wire::decode::<ServerMessage>(bytes) {
                ids.entry(echo.sample_id).or_insert((*id, datagram.frame));
            }
        }
    }
    ids
}

/// The echoes one frame's Input drain handed the estimator, and its smoothed
/// RTT after them.
struct EchoFrame {
    echoes: Vec<EchoFate>,
    estimate_us: f64,
}

const TRACK_SEED: u64 = 0x1507;
/// Thirty seconds of probes: 150 at the 5 Hz cadence.
const TRACK_FRAMES: u64 = 30 * 60;

// Regression guard for the min-RTT gate under the profile: the smoothed RTT
// stays within one gate tolerance of the true round trip, compared at the
// minimum of the last window of probes. True RTT is read from the link, not
// the estimator: each probe's and echo's first arrival among their copies
// (`channel_datagram`), held for every earlier message on the ordered channel,
// then quantized to the receiver's next poll, from the frame that sent the
// probe to the frame whose poll handed its echo over.
#[test]
fn rtt_estimate_tracks_the_realized_round_trip_under_dev_profile() {
    let (to_server, to_client) = dev_links(TRACK_SEED);
    let mut h = RelayHarness::new(to_server, to_client);
    h.log_datagrams = true;
    h.join();
    let mut sent_frame: HashMap<u32, u64> = HashMap::new();
    let mut issued = last_issued(&h);
    let mut seen = fates(&h).len();
    let mut echo_frames = Vec::new();
    for _ in 0..TRACK_FRAMES {
        h.frame(&idle_command());
        let newly_issued = last_issued(&h);
        if newly_issued != issued {
            issued = newly_issued;
            sent_frame.insert(newly_issued.expect("a probe was issued"), h.frame);
        }
        let all = fates(&h);
        let mut echoes = Vec::new();
        for fate in &all[seen..] {
            assert_ne!(
                fate.outcome,
                EchoOutcome::Rejected,
                "no echo arrives twice or unsent: {fate:?}"
            );
            if sent_frame.contains_key(&fate.sample_id) {
                echoes.push(*fate);
            }
        }
        seen = all.len();
        if !echoes.is_empty() {
            echo_frames.push((
                h.frame,
                EchoFrame {
                    echoes,
                    estimate_us: h
                        .time_sync()
                        .rtt_micros()
                        .expect("time sync is initialized"),
                },
            ));
        }
    }

    let up = first_arrivals(&h.up_log, &h.to_server);
    let down = first_arrivals(&h.down_log, &h.to_client);
    let probes = probe_messages(&h);
    let echo_ids = echo_messages(&h);
    let true_rtt_us = |sample_id: u32| -> u64 {
        let (probe, probe_flush) = probes[&sample_id];
        assert_eq!(
            probe_flush,
            sent_frame[&sample_id] + 1,
            "the client issues probe {sample_id} after its poll's flush, and its next \
             poll flushes the probe"
        );
        let host_frame = handed_over_frame(&up, probe);
        let (echo, echo_flush) = echo_ids[&sample_id];
        assert_eq!(
            echo_flush,
            host_frame + 1,
            "the host echoes probe {sample_id} on the poll that hands it over, and its \
             next poll flushes the echo"
        );
        frame_start_us(handed_over_frame(&down, echo)) - frame_start_us(sent_frame[&sample_id])
    };

    // The link bounds the window minimum. Below: the two flush waits asserted
    // above, a frame each, and each direction's base delay. A frame spans at
    // least `SERVER_TICK_MICROS - 1` (`frame_start_us` rounds down), and the
    // conditioners stamp sends in whole milliseconds, so a leg can undercut its
    // base delay by up to 999 us. Above: both directions' largest delay, two
    // frames and one 60 Hz resend poll, which the quickest of sixteen probes
    // beats unless the relay held up every one of them.
    let floor_us =
        (to_server.delay + to_client.delay) * 1000 + 2 * (SERVER_TICK_MICROS - 1) - 2 * 999;
    let ceiling_us =
        largest_one_way_us(to_server) + largest_one_way_us(to_client) + 3 * SERVER_TICK_MICROS;
    let mut realized: Vec<f64> = Vec::new();
    let mut compared = 0;
    let mut worst: Option<(f64, f64, f64)> = None;
    for (frame, echo_frame) in &echo_frames {
        for fate in &echo_frame.echoes {
            let trip = true_rtt_us(fate.sample_id) as f64;
            assert_eq!(
                fate.rtt_us, trip,
                "the estimator's sample for probe {} on frame {frame} is the round trip \
                 the link realized",
                fate.sample_id
            );
            realized.push(trip);
        }
        if realized.len() < RTT_WINDOW_SAMPLES {
            continue;
        }
        let window_min = realized[realized.len() - RTT_WINDOW_SAMPLES..]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        assert!(
            (floor_us as f64..=ceiling_us as f64).contains(&window_min),
            "the true window minimum {window_min:.0} us on frame {frame} lies outside the \
             link's {floor_us}..={ceiling_us} us"
        );
        let estimate = echo_frame.estimate_us;
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
    // The probe leaves at the next poll, in the Input datagram carrying this
    // frame's command.
    let dropped = drop_probe.then(|| h.drop_next_client_input_datagram());
    for _ in 0..60 {
        h.frame(&idle_command());
    }
    if let Some(dropped) = dropped {
        assert_eq!(h.to_server.scripted_dropped(), 1);
        assert_eq!(
            h.to_server.dropped(),
            1,
            "the scripted drop is the only loss"
        );
        let lost = h.uplink_input(dropped);
        assert!(
            lost.iter().any(|message| matches!(
                message,
                ClientMessage::TimeSync(probe) if probe.sample_id == sample_id
            )),
            "the lost Input datagram {dropped} carried probe {sample_id}: {lost:?}"
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
