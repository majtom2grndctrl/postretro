// Own-pawn movement under the dev lag profile, over the production channel layout.
// See: context/lib/networking.md §Testing the conditioned link · §Host input command queue

use std::f32::consts::TAU;
use std::fmt::Write as _;

use glam::Vec2;
use postretro_foundation::{MovementStateKind, VaultEntry, VaultTrigger};
use postretro_net::harness::{LinkConfig, PacketConditioner, ScriptedDrop};
use postretro_net::transport::Channel;
use postretro_net::wire::{self, ActivationOutcome, ServerMessage, WireActivationToken};

use super::SERVER_TICK_MICROS;
use super::command_queue::ResolutionSource;
use super::endpoint::NetEndpoint;
use super::predict_reconcile_harness_test_fixtures::{CLIENT_ID, forward_command, idle_command};
use super::predict_reconcile_harness_vault::{ledge_world, vault};
use super::predict_reconcile_relay_test_fixtures::{
    ClientCorrection, RelayHarness, dev_profile, frame_start_us,
};
use super::prediction::{CorrectionClass, ORDINARY_CORRECTION_MAX_M};
use crate::sim::SimCommand;

/// Seed of the to-host link; the to-client link takes the next one, so the two
/// directions lose and jitter independently.
const STEADY_RUN_SEED: u64 = 0x1502;
/// Frames the client may take to join and start predicting.
const JOIN_FRAMES_MAX: u64 = 600;
/// A minute of the course at 60 Hz.
const COURSE_FRAMES: u64 = 60 * 60;
/// One course cycle: run, slide, strafe one way, strafe the other.
const CYCLE_FRAMES: u64 = 240;
/// The facing turns once every 20 s, so a run at 11 m/s circles about 35 m
/// across and stays on the floor for any length.
const TURN_PER_FRAME: f32 = TAU / 1200.0;

/// The course at `frame`: sustained running on a steady turn, with a slide
/// and both strafes every four seconds.
fn course_command(frame: u64) -> SimCommand {
    let mut command = forward_command(false);
    command.movement.facing_yaw = (frame as f32 * TURN_PER_FRAME) % TAU;
    match frame % CYCLE_FRAMES {
        0..90 => {}
        90..120 => command.movement.crouch_intent = true,
        120..180 => command.movement.wish_dir = Vec2::new(-1.0, 1.0).normalize(),
        _ => command.movement.wish_dir = Vec2::new(1.0, 1.0).normalize(),
    }
    command
}

fn dev_links(seed: u64) -> (LinkConfig, LinkConfig) {
    (dev_profile(seed), dev_profile(seed.wrapping_add(1)))
}

/// One course run: the reconciles the client ran during it (the join's
/// first-record adoption excluded), and a summary for failure messages.
struct SteadyRun {
    corrections: Vec<ClientCorrection>,
    summary: String,
}

fn steady_run(to_server: LinkConfig, to_client: LinkConfig) -> SteadyRun {
    let mut h = RelayHarness::new(to_server, to_client);
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    let course_start = h.frame + 1;
    let trace_start = h.host_trace.len();
    for frame in 0..COURSE_FRAMES {
        h.frame(&course_command(frame));
    }

    let course_trace = &h.host_trace[trace_start..];
    assert!(
        course_trace
            .iter()
            .any(|trace| trace.movement_state == MovementStateKind::Slide),
        "the course slides on the host"
    );
    let corrections: Vec<ClientCorrection> = h
        .corrections
        .iter()
        .filter(|correction| {
            correction.frame >= course_start && !correction.sample.first_record_adoption
        })
        .copied()
        .collect();

    let mut summary = String::new();
    let classes = [
        CorrectionClass::Ordinary,
        CorrectionClass::Dash,
        CorrectionClass::OversizedSmoothed,
        CorrectionClass::Teleport,
    ];
    for class in classes {
        let of_class = corrections
            .iter()
            .filter(|correction| correction.sample.class == class);
        let (count, max) = of_class.fold((0, 0.0_f32), |(count, max), correction| {
            (count + 1, max.max(correction.sample.magnitude))
        });
        let _ = write!(summary, "{class:?}: {count} (max {max:.3} m); ");
    }
    let source_count = |source: ResolutionSource| {
        course_trace
            .iter()
            .filter(|trace| {
                trace
                    .resolved
                    .as_ref()
                    .is_some_and(|resolved| resolved.source == source)
            })
            .count()
    };
    let _ = write!(
        summary,
        "host ticks Real {} / Held {} / Neutral {} of {}; dropped to host {}, to client {}",
        source_count(ResolutionSource::Real),
        source_count(ResolutionSource::Held),
        source_count(ResolutionSource::Neutral),
        course_trace.len(),
        h.to_server.dropped(),
        h.to_client.dropped(),
    );
    eprintln!("[steady run] seed {:#x}: {summary}", to_server.seed);
    SteadyRun {
        corrections,
        summary,
    }
}

/// Every correction above the ordinary band, one per line.
fn above_ordinary_band(run: &SteadyRun) -> Vec<String> {
    run.corrections
        .iter()
        .filter(|correction| correction.sample.magnitude > ORDINARY_CORRECTION_MAX_M)
        .map(|correction| {
            format!(
                "frame {} server tick {} acked client tick {:?}: {:.3} m {:?} in {:?}",
                correction.frame,
                correction.sample.server_tick,
                correction.sample.acked_tick,
                correction.sample.magnitude,
                correction.sample.class,
                correction.movement_state,
            )
        })
        .collect()
}

// Regression: a connected client's own pawn rubber-banded under loss, because one lost
// datagram stalled reliable-ordered Input for its former 300 ms resend.
#[test]
fn steady_run_stays_in_ordinary_band_under_dev_profile() {
    let (to_server, to_client) = dev_links(STEADY_RUN_SEED);
    let run = steady_run(to_server, to_client);
    let oversized = above_ordinary_band(&run);
    assert!(
        oversized.is_empty(),
        "{} corrections above the {ORDINARY_CORRECTION_MAX_M} m ordinary band ({}):\n{}",
        oversized.len(),
        run.summary,
        oversized.join("\n"),
    );
}

// The loss-off control: if this records a correction above the band, jitter rather
// than loss is the cause.
#[test]
fn steady_run_loss_off_records_no_oversized_correction() {
    let (to_server, to_client) = dev_links(STEADY_RUN_SEED);
    let lossless = |link: LinkConfig| LinkConfig {
        loss_probability: 0.0,
        ..link
    };
    let run = steady_run(lossless(to_server), lossless(to_client));
    let oversized = above_ordinary_band(&run);
    assert!(
        oversized.is_empty(),
        "{} corrections above the {ORDINARY_CORRECTION_MAX_M} m ordinary band with loss off ({}):\n{}",
        oversized.len(),
        run.summary,
        oversized.join("\n"),
    );
}

/// The dev profile's base one-way delay with no jitter and no loss, so the only
/// loss is a scripted one.
fn fixed_delay_link(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 0,
        loss_probability: 0.0,
        seed,
    }
}

/// Script the loss of the next Input datagram the client flushes: the one
/// carrying the command it predicted this frame, which leaves at the next
/// frame's poll. Returns that datagram's index among the client's Input
/// datagrams.
fn drop_next_client_input_datagram(h: &mut RelayHarness) -> u64 {
    let index = h.to_server.enqueued_on_channel(Channel::Input);
    h.to_server.script_drop(ScriptedDrop::NthOnChannel {
        channel: Channel::Input,
        index,
    });
    index
}

/// The client tick a slide or vault started on, on each peer.
#[derive(Debug, PartialEq, Eq)]
struct EntryTicks {
    client: u32,
    host: u32,
}

/// Run `command_at(frame)` for `frames` frames after the join, and find the
/// client tick each peer first entered `kind` on. With `drop_entry`, the Input
/// datagram carrying the client's entry command is lost once, and the test
/// requires that scripted drop to have been the loss.
fn entry_ticks(
    mut h: RelayHarness,
    kind: MovementStateKind,
    frames: u64,
    command_at: impl Fn(u64) -> SimCommand,
    drop_entry: bool,
) -> EntryTicks {
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    let trace_start = h.host_trace.len();
    let mut client = None;
    for frame in 0..frames {
        h.frame(&command_at(frame));
        if client.is_none() && h.client_movement_state() == Some(kind) {
            client = h.last_client_tick;
            if drop_entry {
                drop_next_client_input_datagram(&mut h);
            }
        }
    }
    let client = client.unwrap_or_else(|| panic!("the client never entered {kind:?}"));
    let host = h.host_trace[trace_start..]
        .iter()
        .find(|trace| trace.movement_state == kind)
        .and_then(|trace| trace.resolved.as_ref())
        .map(|resolved| resolved.client_tick)
        .unwrap_or_else(|| panic!("the host never entered {kind:?}"));
    if drop_entry {
        assert_eq!(
            h.to_server.scripted_dropped(),
            1,
            "the scripted drop, not the seeded link, lost the entry datagram"
        );
    }
    EntryTicks { client, host }
}

/// Frames of running before the slide: enough to reach run speed.
const SLIDE_RUN_UP_FRAMES: u64 = 90;
/// Frames after the run-up: the slide and the host catching up on it.
const SLIDE_FRAMES: u64 = 60;
/// Frames of running at the riser, which `ledge_world` puts 6 m ahead.
const VAULT_FRAMES: u64 = 120;
const ENTRY_SEED: u64 = 0x1503;

fn slide_entry(drop_entry: bool) -> EntryTicks {
    let (to_server, to_client) = dev_links(ENTRY_SEED);
    entry_ticks(
        RelayHarness::new(to_server, to_client),
        MovementStateKind::Slide,
        SLIDE_RUN_UP_FRAMES + SLIDE_FRAMES,
        |frame| {
            let mut command = forward_command(false);
            command.movement.crouch_intent = frame >= SLIDE_RUN_UP_FRAMES;
            command
        },
        drop_entry,
    )
}

/// A ground-triggered vault onto `ledge_world`'s riser.
fn vault_entry(drop_entry: bool) -> EntryTicks {
    let (to_server, to_client) = dev_links(ENTRY_SEED);
    let mut h = RelayHarness::new(to_server, to_client).with_player_vault(vault(VaultEntry {
        ground: Some(VaultTrigger::Auto),
        ..VaultEntry::default()
    }));
    h.world = ledge_world();
    entry_ticks(
        h,
        MovementStateKind::Vault,
        VAULT_FRAMES,
        |_| forward_command(false),
        drop_entry,
    )
}

#[test]
fn slide_entry_enters_on_host_same_client_tick() {
    let entry = slide_entry(false);
    assert_eq!(entry.host, entry.client);
}

// P3: the entry command's datagram is lost; its resend rides the next frame's
// datagram, inside the host's playout margin.
#[test]
fn slide_entry_lost_datagram_enters_on_host_same_client_tick() {
    let entry = slide_entry(true);
    assert_eq!(entry.host, entry.client);
}

#[test]
fn vault_entry_enters_on_host_same_client_tick() {
    let entry = vault_entry(false);
    assert_eq!(entry.host, entry.client);
}

// P3, vault twin.
#[test]
fn vault_entry_lost_datagram_enters_on_host_same_client_tick() {
    let entry = vault_entry(true);
    assert_eq!(entry.host, entry.client);
}

/// Frames of running before the scripted drop and after it.
const BUILD_UP_FRAMES: u64 = 120;
const AFTER_DROP_FRAMES: u64 = 120;

// P2: at a resend under one tick the lost command rides the next datagram,
// so the host never holds, neutral-walks or trims across one loss.
#[test]
fn single_lost_input_datagram_resolves_real_on_its_own_tick() {
    let mut h = RelayHarness::new(fixed_delay_link(0x1504), fixed_delay_link(0x1505));
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    for frame in 0..BUILD_UP_FRAMES {
        h.frame(&course_command(frame));
    }
    let window_start = h.host_trace.len();
    let lost_tick = h.last_client_tick.expect("the client predicts");
    drop_next_client_input_datagram(&mut h);
    for frame in BUILD_UP_FRAMES..BUILD_UP_FRAMES + AFTER_DROP_FRAMES {
        h.frame(&course_command(frame));
    }
    assert_eq!(h.to_server.scripted_dropped(), 1);
    assert_eq!(
        h.to_server.dropped(),
        1,
        "the scripted drop is the only loss"
    );

    let resolved: Vec<_> = h.host_trace[window_start..]
        .iter()
        .map(|trace| {
            let resolved = trace
                .resolved
                .as_ref()
                .expect("the host resolves every tick");
            (resolved.client_tick, resolved.source)
        })
        .collect();
    assert!(
        resolved
            .iter()
            .all(|&(_, source)| source == ResolutionSource::Real),
        "every tick across the loss resolves Real: {resolved:?}"
    );
    assert!(
        resolved
            .windows(2)
            .all(|pair| pair[1].0 == pair[0].0.wrapping_add(1)),
        "the cursor advances one client tick per host tick: {resolved:?}"
    );
    assert!(
        resolved.iter().any(|&(tick, _)| tick == lost_tick),
        "the lost command {lost_tick} resolves on its own tick: {resolved:?}"
    );
}

/// The profile's largest one-way delay plus two fixed ticks: one frame waiting
/// for the poll that flushes the send, one for the poll that resends it.
const SINGLE_LOSS_BOUND_US: u64 = 105_000 + 2 * SERVER_TICK_MICROS;
const LATENCY_SEED: u64 = 0x1506;
/// Frames the dropped messages may take to arrive before the test gives up.
const DELIVERY_FRAMES_MAX: u64 = 60;

/// One direction's scripted single loss, timed from the send call.
struct SingleLoss {
    /// Virtual µs until the resend datagram reaches the receiver's side of the
    /// conditioner.
    arrival_us: u64,
    /// Virtual µs until the receiver's poll hands the message over: the
    /// arrival quantized up to the receiver's next frame.
    receipt_us: u64,
}

/// The resend of the message whose first datagram, `dropped`, was lost on
/// `link`, sent at `send_frame` and handed over by the receiver's poll at
/// `receipt_frame`.
fn single_loss(
    link: &PacketConditioner,
    dropped: u64,
    send_frame: u64,
    receipt_frame: u64,
) -> SingleLoss {
    let lost = link
        .channel_datagram(Channel::Input, dropped)
        .expect("the dropped datagram was flushed");
    assert_eq!(lost.arrives_at, None, "the scripted drop lost it");
    let resend = link
        .channel_datagram(Channel::Input, dropped + 1)
        .expect("the next Input datagram was flushed");
    let arrives_at = resend
        .arrives_at
        .expect("precondition: the seeded link delivers the resend, so this is a single loss");
    let send_us = frame_start_us(send_frame);
    // The receiver polls at frame starts; the first poll at or past the
    // arrival hands it over, unless a later copy overtook it.
    let first_poll = (send_frame..)
        .find(|&frame| frame_start_us(frame) / 1000 >= arrives_at)
        .expect("a poll follows the arrival");
    assert!(
        receipt_frame <= first_poll,
        "the message is handed over by the first poll after its resend arrives \
         (receipt frame {receipt_frame}, first poll {first_poll})"
    );
    SingleLoss {
        arrival_us: arrives_at * 1000 - send_us,
        receipt_us: frame_start_us(receipt_frame) - send_us,
    }
}

// P2: one lost Input datagram, in either direction, costs one resend poll on
// top of the transit, not a resend stall.
#[test]
fn single_input_drop_delivers_within_one_way_plus_two_ticks_both_directions() {
    let (to_server, to_client) = dev_links(LATENCY_SEED);
    let mut h = RelayHarness::new(to_server, to_client);
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    for frame in 0..BUILD_UP_FRAMES {
        h.frame(&course_command(frame));
    }

    // Both sends belong to this frame: the client sent its command in its
    // fixed tick, and the host's outcome goes out through the production host
    // Input send before the next poll.
    let send_frame = h.frame;
    let command_tick = h.last_client_tick.expect("the client predicts");
    let to_host_dropped = drop_next_client_input_datagram(&mut h);
    let to_client_dropped = h.to_client.enqueued_on_channel(Channel::Input);
    h.to_client.script_drop(ScriptedDrop::NthOnChannel {
        channel: Channel::Input,
        index: to_client_dropped,
    });
    let outcome = ActivationOutcome::InitiationRejected {
        token: WireActivationToken {
            start_tick: 0x0A15,
            lane: 0,
        },
        recovery_ticks: 0,
    };
    h.server.send_input(
        CLIENT_ID,
        wire::encode(&ServerMessage::ActivationOutcomes(vec![outcome])),
    );

    let mut host_receipt = None;
    let mut client_receipt = None;
    for frame in BUILD_UP_FRAMES..BUILD_UP_FRAMES + DELIVERY_FRAMES_MAX {
        h.frame(&course_command(frame));
        let host_has_command = h.command_queues.has_pending_tick(CLIENT_ID, command_tick)
            || h.host_trace.iter().any(|trace| {
                trace.resolved.as_ref().is_some_and(|resolved| {
                    resolved.client_tick == command_tick
                        && resolved.source == ResolutionSource::Real
                })
            });
        if host_receipt.is_none() && host_has_command {
            host_receipt = Some(h.frame);
        }
        let NetEndpoint::Client { time_sync, .. } = &mut h.endpoint else {
            unreachable!("the relay endpoint is a client");
        };
        if client_receipt.is_none()
            && time_sync
                .drain_activation_outcomes()
                .any(|received| received == outcome)
        {
            client_receipt = Some(h.frame);
        }
    }
    assert_eq!(h.to_server.scripted_dropped(), 1);
    assert_eq!(h.to_client.scripted_dropped(), 1);

    let to_host = single_loss(
        &h.to_server,
        to_host_dropped,
        send_frame,
        host_receipt.expect("the host receives the lost command"),
    );
    let to_client = single_loss(
        &h.to_client,
        to_client_dropped,
        send_frame,
        client_receipt.expect("the client receives the lost outcome"),
    );
    eprintln!(
        "[single loss] bound {SINGLE_LOSS_BOUND_US} us; client->host arrival {} us, \
         receipt {} us; host->client arrival {} us, receipt {} us",
        to_host.arrival_us, to_host.receipt_us, to_client.arrival_us, to_client.receipt_us,
    );
    for (direction, loss) in [("client->host", &to_host), ("host->client", &to_client)] {
        assert!(
            loss.arrival_us <= SINGLE_LOSS_BOUND_US,
            "{direction}: the resend arrives {} us after the send, past {SINGLE_LOSS_BOUND_US} us",
            loss.arrival_us,
        );
    }
}
