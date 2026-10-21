// Own-pawn movement under the dev lag profile, over the production channel layout.
// See: context/lib/networking.md §Testing the conditioned link · §Host input command queue

use std::f32::consts::TAU;
use std::fmt::Write as _;

use glam::{Vec2, Vec3};
use postretro_foundation::{MovementStateKind, VaultEntry, VaultTrigger};
use postretro_net::harness::{LinkConfig, PacketConditioner, ScriptedDrop};
use postretro_net::transport::Channel;
use postretro_net::wire::{
    self, ActivationOutcome, ClientMessage, ServerMessage, WireActivationToken,
};

use super::SERVER_TICK_MICROS;
use super::command_queue::ResolutionSource;
use super::endpoint::NetEndpoint;
use super::predict_reconcile_harness_test_fixtures::{
    CLIENT_ID, SETTLED_PAWN_TOLERANCE_M, forward_command, idle_command,
};
use super::predict_reconcile_harness_vault::{RISER_Z, TOP_Y, ledge_world, vault};
use super::predict_reconcile_relay_link_test_fixtures::{
    OUTAGE_MS, SINGLE_LOSS_BOUND_US, dev_links, first_poll_frame, fixed_delay_link, frame_start_us,
};
use super::predict_reconcile_relay_test_fixtures::{ClientCorrection, RelayHarness};
use super::prediction::{CorrectionClass, ORDINARY_CORRECTION_MAX_M};
use crate::collision::CollisionWorld;
use crate::sim::SimCommand;

/// Seed of the to-host link; the to-client link takes the next one.
const STEADY_RUN_SEED: u64 = 0x1502;
/// A minute of the course at 60 Hz.
const COURSE_FRAMES: u64 = 60 * 60;
/// One course cycle: run, slide, strafe one way, strafe the other.
const CYCLE_FRAMES: u64 = 240;
/// The facing turns once every 20 s, so a run at 11 m/s circles about 35 m
/// across and stays on the floor for any length.
const TURN_PER_FRAME: f32 = TAU / 1200.0;

/// The course at `frame`: sustained running on a steady turn, with a slide
/// and both strafes every four seconds.
pub(crate) fn course_command(frame: u64) -> SimCommand {
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

/// Idle frames after the course before the pawns are compared: past the
/// hold window, deceleration to rest and a round trip under the profile.
const SETTLE_FRAMES: u64 = 120;

/// One course run: the reconciles the client ran during it (the join's
/// first-record adoption excluded), and a summary for failure messages.
struct SteadyRun {
    corrections: Vec<ClientCorrection>,
    summary: String,
    /// The host pawn's path length over the course.
    host_travel: f32,
    /// Client-to-host pawn distance once both settle after the course.
    settled_error: f32,
    /// Datagrams the seeded link lost to the host and to the client.
    dropped: (u64, u64),
    bystanders_alive: bool,
    /// The host pawn's settled position.
    host_final: Vec3,
    /// The host pawn's movement state at the end of each course tick.
    host_states: Vec<MovementStateKind>,
}

impl SteadyRun {
    /// How many times the host pawn entered `kind` during the course.
    fn host_entries(&self, kind: MovementStateKind) -> usize {
        let mut previous = None;
        self.host_states
            .iter()
            .filter(|&&state| {
                let entered = state == kind && previous != Some(kind);
                previous = Some(state);
                entered
            })
            .count()
    }
}

/// The floor course: a minute of [`course_command`], which slides on the host.
fn steady_run(to_server: LinkConfig, to_client: LinkConfig) -> SteadyRun {
    let run = run_course(
        RelayHarness::new(to_server, to_client),
        &format!("steady run seed {:#x}", to_server.seed),
        COURSE_FRAMES,
        course_command,
    );
    assert!(
        run.host_entries(MovementStateKind::Slide) > 0,
        "the course slides on the host"
    );
    run
}

/// Join, run `frames` frames of `command_at`, then settle idle.
fn run_course(
    mut h: RelayHarness,
    label: &str,
    frames: u64,
    command_at: impl Fn(u64) -> SimCommand,
) -> SteadyRun {
    h.join();
    let course_start = h.frame + 1;
    let trace_start = h.host_trace.len();
    for frame in 0..frames {
        h.frame(&command_at(frame));
    }

    let course_trace = &h.host_trace[trace_start..];
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
    let host_travel = course_trace
        .windows(2)
        .map(|pair| (pair[1].position - pair[0].position).length())
        .sum();
    eprintln!("[{label}] {summary}");
    let host_states = course_trace
        .iter()
        .map(|trace| trace.movement_state)
        .collect();

    for _ in 0..SETTLE_FRAMES {
        h.frame(&idle_command());
    }
    SteadyRun {
        corrections,
        summary,
        host_travel,
        settled_error: h.position_error().expect("the client stays armed"),
        dropped: (h.to_server.dropped(), h.to_client.dropped()),
        bystanders_alive: h.bystanders_alive(),
        host_final: h.host_trace.last().expect("the host ticked").position,
        host_states,
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
    assert_ordinary_band_and_no_snap(&run);
    assert_course_exercised_and_converged(&run);
}

/// No correction above the ordinary band, and none of Teleport class: a
/// composed offset at the teleport floor snaps at an ordinary magnitude, so
/// the band check alone would pass it.
fn assert_ordinary_band_and_no_snap(run: &SteadyRun) {
    let oversized = above_ordinary_band(run);
    assert!(
        oversized.is_empty(),
        "{} corrections above the {ORDINARY_CORRECTION_MAX_M} m ordinary band ({}):\n{}",
        oversized.len(),
        run.summary,
        oversized.join("\n"),
    );
    let snaps: Vec<u64> = run
        .corrections
        .iter()
        .filter(|correction| correction.sample.class == CorrectionClass::Teleport)
        .map(|correction| correction.frame)
        .collect();
    assert!(
        snaps.is_empty(),
        "Teleport-class corrections on frames {snaps:?} ({})",
        run.summary
    );
}

/// The run was not trivial: the pawn travelled, the seeded link lost datagrams
/// both ways and the client reconciled against them. Once both pawns settle,
/// the client's matches the host's, and the movement-only paths ran no
/// registry-wide system on either peer.
fn assert_course_exercised_and_converged(run: &SteadyRun) {
    assert!(
        run.host_travel > 5.0,
        "the course moved the host pawn {:.2} m",
        run.host_travel
    );
    assert!(
        run.dropped.0 > 0 && run.dropped.1 > 0,
        "the profile's loss dropped datagrams both ways: to host {}, to client {}",
        run.dropped.0,
        run.dropped.1
    );
    assert!(
        !run.corrections.is_empty(),
        "the client reconciled during the course"
    );
    assert!(
        run.settled_error <= SETTLED_PAWN_TOLERANCE_M,
        "the settled client pawn is within {SETTLED_PAWN_TOLERANCE_M} m of the host's: {:.5} m",
        run.settled_error
    );
    assert!(
        run.bystanders_alive,
        "the death-sweep bystanders survived: neither peer ran simulate_tick"
    );
}

// The seeded run is bit-for-bit reproducible: the same seed yields the same
// losses, corrections and final pose.
#[test]
fn steady_run_is_deterministic_under_its_seed() {
    let fingerprint = |run: &SteadyRun| {
        (
            run.dropped,
            run.corrections
                .iter()
                .map(|correction| (correction.frame, correction.sample.magnitude.to_bits()))
                .collect::<Vec<_>>(),
            run.host_final.to_array().map(f32::to_bits),
            run.settled_error.to_bits(),
        )
    };
    let (to_server, to_client) = dev_links(STEADY_RUN_SEED);
    let first = steady_run(to_server, to_client);
    let second = steady_run(to_server, to_client);
    assert_eq!(fingerprint(&first), fingerprint(&second));
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

const VAULT_COURSE_SEED: u64 = 0x150c;
/// Risers on the vault course, each [`TOP_Y`] above the last.
const VAULT_COURSE_RISERS: usize = 6;
/// Run-up between risers. The first stands [`RISER_Z`] ahead of the start, as
/// in `ledge_world`.
const VAULT_COURSE_SPACING_M: f32 = 8.0;
/// Ten seconds of running: the 46 m to the last riser and six vaults, with
/// margin.
const VAULT_COURSE_FRAMES: u64 = 10 * 60;

/// One quad of a test world. A horizontal quad lists its far (-Z) edge first;
/// a vertical one lists its bottom edge first and faces +Z.
fn push_quad(
    points: &mut Vec<Vec3>,
    triangles: &mut Vec<[u32; 3]>,
    corners: [Vec3; 4],
    horizontal: bool,
) {
    let base = points.len() as u32;
    points.extend(corners);
    let local = if horizontal {
        [[0, 2, 1], [0, 3, 2]]
    } else {
        [[0, 1, 2], [0, 2, 3]]
    };
    triangles.extend(local.map(|triangle| triangle.map(|corner| base + corner)));
}

/// `ledge_world` repeated: a step [`TOP_Y`] up at each riser, the first at
/// [`RISER_Z`] and each next one [`VAULT_COURSE_SPACING_M`] further along -Z,
/// all facing the pawn as it runs along -Z.
fn riser_steps_world() -> CollisionWorld {
    let (x0, x1) = (-500.0, 500.0);
    let riser_z = |riser: usize| RISER_Z - riser as f32 * VAULT_COURSE_SPACING_M;
    let mut points = Vec::new();
    let mut triangles = Vec::new();
    for level in 0..=VAULT_COURSE_RISERS {
        let y = level as f32 * TOP_Y;
        let near = if level == 0 {
            500.0
        } else {
            riser_z(level - 1)
        };
        let far = if level == VAULT_COURSE_RISERS {
            -500.0
        } else {
            riser_z(level)
        };
        push_quad(
            &mut points,
            &mut triangles,
            [
                Vec3::new(x0, y, far),
                Vec3::new(x1, y, far),
                Vec3::new(x1, y, near),
                Vec3::new(x0, y, near),
            ],
            true,
        );
        if level < VAULT_COURSE_RISERS {
            let z = riser_z(level);
            push_quad(
                &mut points,
                &mut triangles,
                [
                    Vec3::new(x0, y, z),
                    Vec3::new(x1, y, z),
                    Vec3::new(x1, y + TOP_Y, z),
                    Vec3::new(x0, y + TOP_Y, z),
                ],
                false,
            );
        }
    }
    CollisionWorld::from_triangles_for_test(points, triangles)
}

// Row 2's vault windows: a seeded run up a flight of risers under the dev
// profile, vaulting each one, keeps every correction inside the ordinary band,
// where the Dash band would otherwise pass up to 2 m.
#[test]
fn vault_course_stays_in_ordinary_band_under_dev_profile() {
    let (to_server, to_client) = dev_links(VAULT_COURSE_SEED);
    let mut h = RelayHarness::new(to_server, to_client).with_player_vault(vault(VaultEntry {
        ground: Some(VaultTrigger::Auto),
        ..VaultEntry::default()
    }));
    h.world = riser_steps_world();
    let run = run_course(
        h,
        &format!("vault course seed {VAULT_COURSE_SEED:#x}"),
        VAULT_COURSE_FRAMES,
        |_| forward_command(false),
    );
    let vaults = run.host_entries(MovementStateKind::Vault);
    let in_vault = run
        .corrections
        .iter()
        .filter(|correction| correction.movement_state == Some(MovementStateKind::Vault))
        .count();
    eprintln!("[vault course] host vaults {vaults}; client corrections in a vault {in_vault}");
    assert!(
        vaults >= VAULT_COURSE_RISERS,
        "the host vaulted {vaults} of {VAULT_COURSE_RISERS} risers"
    );
    assert_ordinary_band_and_no_snap(&run);
    assert_course_exercised_and_converged(&run);
}

/// The `index`th uplink Input datagram, scripted lost, carried the client's
/// command for `client_tick`.
fn assert_lost_datagram_carried_command(h: &RelayHarness, index: u64, client_tick: u32) {
    let messages = h.uplink_input(index);
    assert!(
        messages.iter().any(|message| matches!(
            message,
            ClientMessage::Input(command) if command.client_tick == client_tick
        )),
        "the lost Input datagram {index} carried command {client_tick}: {messages:?}"
    );
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
    h.join();
    let trace_start = h.host_trace.len();
    let mut client = None;
    let mut dropped = None;
    for frame in 0..frames {
        h.frame(&command_at(frame));
        if client.is_none() && h.client_movement_state() == Some(kind) {
            client = h.last_client_tick;
            if drop_entry {
                dropped = Some(h.drop_next_client_input_datagram());
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
    if let Some(dropped) = dropped {
        assert_eq!(
            h.to_server.scripted_dropped(),
            1,
            "the scripted drop, not the seeded link, lost the entry datagram"
        );
        assert_lost_datagram_carried_command(&h, dropped, client);
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
    h.join();
    for frame in 0..BUILD_UP_FRAMES {
        h.frame(&course_command(frame));
    }
    let window_start = h.host_trace.len();
    let lost_tick = h.last_client_tick.expect("the client predicts");
    let dropped = h.drop_next_client_input_datagram();
    for frame in BUILD_UP_FRAMES..BUILD_UP_FRAMES + AFTER_DROP_FRAMES {
        h.frame(&course_command(frame));
    }
    assert_eq!(h.to_server.scripted_dropped(), 1);
    assert_lost_datagram_carried_command(&h, dropped, lost_tick);
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
    let first_poll = first_poll_frame(arrives_at);
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
    h.log_datagrams = true;
    h.join();
    for frame in 0..BUILD_UP_FRAMES {
        h.frame(&course_command(frame));
    }

    // Both sends belong to this frame: the client sent its command in its
    // fixed tick, and the host's outcome goes out through the production host
    // Input send before the next poll.
    let send_frame = h.frame;
    let command_tick = h.last_client_tick.expect("the client predicts");
    let to_host_dropped = h.drop_next_client_input_datagram();
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
    assert_lost_datagram_carried_command(&h, to_host_dropped, command_tick);
    let lost_down = h.downlink_input(to_client_dropped);
    assert!(
        lost_down.iter().any(|message| matches!(
            message,
            ServerMessage::ActivationOutcomes(outcomes) if outcomes.contains(&outcome)
        )),
        "the lost host Input datagram carried the outcome: {lost_down:?}"
    );

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
    // The receiver's poll takes an arrival up to one frame after it lands, so
    // the message is handed over within the bound plus that frame.
    let receipt_bound_us = SINGLE_LOSS_BOUND_US + SERVER_TICK_MICROS;
    for (direction, loss) in [("client->host", &to_host), ("host->client", &to_client)] {
        assert!(
            loss.arrival_us <= SINGLE_LOSS_BOUND_US,
            "{direction}: the resend arrives {} us after the send, past {SINGLE_LOSS_BOUND_US} us",
            loss.arrival_us,
        );
        assert!(
            loss.receipt_us <= receipt_bound_us,
            "{direction}: the receiver's poll hands it over {} us after the send, past \
             {receipt_bound_us} us",
            loss.receipt_us,
        );
    }
}

const OUTAGE_SEED: u64 = 0x150b;
/// Frames of the course before the [`OUTAGE_MS`] cut.
const OUTAGE_LEAD_FRAMES: u64 = 240;
/// Frames of the course after the cut, the outage included.
const OUTAGE_TAIL_FRAMES: u64 = 8 * 60;
/// Frames after the link returns within which the client must be back to
/// ordinary corrections: the burst's transit, the trim, and one reconcile.
const OUTAGE_RECOVERY_FRAMES: u64 = 60;

// P12: a total outage longer than the redundancy falls back to the gap and
// catch-up policy. The host neutral-walks, the returning burst trims, the
// client's overflowed history trusts the authority once, and the pawn is back
// to ordinary corrections afterwards. Nothing disconnects.
#[test]
fn two_second_outage_falls_back_to_gap_policy_and_recovers_to_ordinary_corrections() {
    let (to_server, to_client) = dev_links(OUTAGE_SEED);
    let mut h = RelayHarness::new(to_server, to_client);
    h.join();
    for frame in 0..OUTAGE_LEAD_FRAMES {
        h.frame(&course_command(frame));
    }
    let trace_start = h.host_trace.len();
    let (_, link_returns_ms) = h.script_outage(OUTAGE_MS);
    let return_frame = first_poll_frame(link_returns_ms);
    for frame in OUTAGE_LEAD_FRAMES..OUTAGE_LEAD_FRAMES + OUTAGE_TAIL_FRAMES {
        h.frame(&course_command(frame));
    }
    assert!(h.both_participating(), "the outage disconnects nobody");

    let resolved: Vec<(u32, ResolutionSource)> = h.host_trace[trace_start..]
        .iter()
        .filter_map(|trace| trace.resolved.as_ref())
        .map(|resolved| (resolved.client_tick, resolved.source))
        .collect();
    assert!(
        resolved
            .iter()
            .any(|&(_, source)| source == ResolutionSource::Neutral),
        "the host neutral-walks the outage"
    );
    let trims: Vec<u32> = resolved
        .windows(2)
        .map(|pair| pair[1].0.wrapping_sub(pair[0].0))
        .filter(|&step| step > 1)
        .collect();
    assert!(
        !trims.is_empty(),
        "the returning burst takes the catch-up trim"
    );

    let settled = return_frame + OUTAGE_RECOVERY_FRAMES;
    let describe = |correction: &&ClientCorrection| {
        format!(
            "frame {} (+{}): {:.3} m {:?}",
            correction.frame,
            correction.frame.saturating_sub(return_frame),
            correction.sample.magnitude,
            correction.sample.class
        )
    };
    let after: Vec<&ClientCorrection> = h
        .corrections
        .iter()
        .filter(|correction| correction.frame >= return_frame)
        .collect();
    let (recovering, settled_corrections): (Vec<&ClientCorrection>, Vec<&ClientCorrection>) = after
        .into_iter()
        .partition(|correction| correction.frame < settled);
    let recovery: Vec<String> = recovering
        .iter()
        .filter(|correction| correction.sample.class != CorrectionClass::Ordinary)
        .map(describe)
        .collect();
    let late: Vec<String> = settled_corrections
        .iter()
        .filter(|correction| correction.sample.class != CorrectionClass::Ordinary)
        .map(describe)
        .collect();
    eprintln!(
        "[outage movement] link returns on frame {return_frame}; client-tick steps {trims:?}; \
         recovery corrections {recovery:?}; {} corrections after frame {settled}",
        settled_corrections.len()
    );
    assert!(
        !settled_corrections.is_empty(),
        "the client keeps reconciling after the recovery"
    );
    assert!(
        late.is_empty(),
        "corrections past the recovery window are all Ordinary:\n{}",
        late.join("\n")
    );
}
