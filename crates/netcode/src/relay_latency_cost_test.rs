// Latency and cost of the production channel layout over the relay: a lost ack
// on a new entity, sliced tuning on Control beside Input, tuning across a
// demotion or level change, the client's upstream bytes, and a lost tuning
// datagram on join.
// See: context/lib/networking.md §Channel model · §Wire/codec invariants ·
// §What gates, and what replicates instead · context/lib/testing_guide.md
//
// Every datagram is read at its flush (`LoggedDatagram`), so a test names the
// datagram carrying a message by its contents and reads its fate from the
// conditioner (`channel_datagram`). Drops are scripted on top of the seeded
// link model, which keeps every other datagram's fate.

#![cfg(test)]

use std::collections::HashSet;
use std::fmt::Write as _;

use glam::Vec3;
use postretro_combat_model::{FluidMovementTable, FluidMovementTuning};
use postretro_entities::Transform;
use postretro_entities::components::inventory::Inventory;
use postretro_net::harness::{
    LinkConfig, RenetDatagram, participation_frame_payload, server_control_payload,
};
use postretro_net::transport::Channel;
use postretro_net::wire::{
    self, ActivationOutcome, ClientMessage, DivergenceReason, NetworkId, RECORD_KIND_DELTA,
    RECORD_KIND_FULL_BASELINE, RawSnapshotMessage, ServerControlMessage, ServerMessage,
    WireActivationToken,
};

use super::endpoint::NetEndpoint;
use super::predict_reconcile_harness_test_fixtures::{CLIENT_ID, idle_command};
use super::predict_reconcile_relay_harness::course_command;
use super::predict_reconcile_relay_link_test_fixtures::{
    JOIN_FRAMES_MAX, LARGEST_ONE_WAY_US, LoggedDatagram, RELAY_FINGERPRINT, RELAY_LEVEL,
    SINGLE_LOSS_BOUND_US, dev_links, dev_profile, fixed_delay_link, frame_start_us, server_inputs,
};
use super::predict_reconcile_relay_test_fixtures::{DrainedControl, RelayHarness};

/// Control's resend (`connection_config`).
const CONTROL_RESEND_US: u64 = 100_000;

/// A level declaring `count` fluids with long names. Its table rides every
/// tuning payload, so the payload grows past one 1200-byte slice while staying
/// a payload the client validates and installs.
fn many_fluids(count: usize) -> FluidMovementTable {
    FluidMovementTable::new(
        (0..count)
            .map(|index| FluidMovementTuning {
                name: format!("reactor-coolant-loop-{index:03}-secondary-containment"),
                viscosity: 0.5 + index as f32 * 0.01,
            })
            .collect(),
    )
    .expect("the fixture fluid table validates")
}

/// The Control messages a host datagram carries whole, decoded.
fn control_messages(datagram: &LoggedDatagram) -> Vec<ServerControlMessage> {
    match &datagram.contents {
        Some(RenetDatagram::Reliable { messages, .. })
            if datagram.channel == Some(Channel::Control) =>
        {
            messages
                .iter()
                .filter_map(|(_, bytes)| server_control_payload(bytes))
                .filter_map(|payload| wire::decode::<ServerControlMessage>(&payload).ok())
                .collect()
        }
        _ => Vec::new(),
    }
}

fn carries_whole_tuning(datagram: &LoggedDatagram) -> bool {
    control_messages(datagram)
        .iter()
        .any(|message| matches!(message, ServerControlMessage::Tuning(_)))
}

fn carries_holding(datagram: &LoggedDatagram) -> bool {
    control_messages(datagram).iter().any(|message| {
        matches!(
            message,
            ServerControlMessage::Divergence(DivergenceReason::Holding(_))
        )
    })
}

/// A Control slice: `(message_id, slice_index, num_slices)`. Tuning is the
/// only Control message past one slice.
fn control_slice(datagram: &LoggedDatagram) -> Option<(u64, u64, u64)> {
    match datagram.contents {
        Some(RenetDatagram::Slice {
            reliable: true,
            message_id,
            slice_index,
            num_slices,
            ..
        }) if datagram.channel == Some(Channel::Control) => {
            Some((message_id, slice_index, num_slices))
        }
        _ => None,
    }
}

/// The snapshots a host datagram carries, decoded.
fn snapshots(datagram: &LoggedDatagram) -> Vec<RawSnapshotMessage> {
    match &datagram.contents {
        Some(RenetDatagram::Unreliable { messages, .. })
            if datagram.channel == Some(Channel::Snapshot) =>
        {
            messages
                .iter()
                .filter_map(|message| participation_frame_payload(message))
                .filter_map(|payload| wire::decode::<RawSnapshotMessage>(&payload).ok())
                .collect()
        }
        _ => Vec::new(),
    }
}

fn client_holds_baseline(h: &RelayHarness, network_id: NetworkId) -> bool {
    match &h.endpoint {
        NetEndpoint::Client { replication, .. } => {
            replication.stored_baseline(network_id).is_some()
        }
        NetEndpoint::Host { .. } => false,
    }
}

// --- P15: a lost ack on a new entity ---

/// What the host sent for one new entity, read from its snapshot flushes.
#[derive(Debug)]
struct NewEntityRun {
    /// Snapshot sequence of the entity's FullBaseline.
    baseline_sequence: u32,
    /// Snapshot sequence of the host's first Delta for it.
    first_delta_sequence: u32,
    /// Snapshots the host sent between them with no record for the entity.
    frozen_snapshots: usize,
    /// FullBaselines the host sent for it before its first Delta.
    full_baselines: usize,
    /// The uplink Input datagram that carried the client's ack of the baseline.
    ack_datagram: u64,
    /// Whether that datagram arrived.
    ack_arrived: bool,
}

/// Spawn a moving entity on the host once the client predicts, and follow it
/// to the host's first Delta. With `drop_ack`, the Input datagram carrying the
/// client's ack of the entity's FullBaseline is lost once.
fn new_entity_run(to_server: LinkConfig, to_client: LinkConfig, drop_ack: bool) -> NewEntityRun {
    const FRAMES_MAX: u64 = 240;
    let mut h = RelayHarness::new(to_server, to_client);
    h.log_datagrams = true;
    h.join();
    for _ in 0..30 {
        h.frame(&idle_command());
    }
    let origin = Vec3::new(4.0, 1.0, 4.0);
    let (entity, network_id) = h.spawn_replicated_host_entity(Transform {
        position: origin,
        ..Transform::default()
    });
    let mut ack_datagram = None;
    let mut scanned = h.down_log.len();
    for step in 0..FRAMES_MAX {
        // It moves every tick, so every snapshot after the ack carries a Delta.
        let moved = Transform {
            position: origin + Vec3::X * (0.01 * step as f32),
            ..Transform::default()
        };
        h.host_registry.set_component(entity, moved).unwrap();
        h.frame(&idle_command());
        if ack_datagram.is_none() && client_holds_baseline(&h, network_id) {
            // The apply queued its ack on Input this frame; the next poll
            // flushes it in this Input datagram.
            ack_datagram = Some(if drop_ack {
                h.drop_next_client_input_datagram()
            } else {
                h.to_server.enqueued_on_channel(Channel::Input)
            });
        }
        let first_delta_sent = h.down_log[scanned..]
            .iter()
            .flat_map(snapshots)
            .any(|snapshot| {
                snapshot.records.iter().any(|record| {
                    record.network_id == network_id.0 && record.record_kind == RECORD_KIND_DELTA
                })
            });
        scanned = h.down_log.len();
        if first_delta_sent {
            break;
        }
    }
    let ack_datagram = ack_datagram.expect("the client applied the entity's baseline");
    let acked = h.uplink_input(ack_datagram).iter().any(|message| {
        matches!(message, ClientMessage::Ack(ack)
                if ack.entity_baselines.iter().any(|&(id, _)| id == network_id.0))
    });
    assert!(
        acked,
        "the datagram carries the ack of the entity's baseline"
    );

    // Every snapshot the host sent from the baseline on, in order.
    let sent: Vec<RawSnapshotMessage> = h.down_log.iter().flat_map(snapshots).collect();
    let record_kind = |snapshot: &RawSnapshotMessage| {
        snapshot
            .records
            .iter()
            .find(|record| record.network_id == network_id.0)
            .map(|record| record.record_kind)
    };
    let baseline_at = sent
        .iter()
        .position(|snapshot| record_kind(snapshot) == Some(RECORD_KIND_FULL_BASELINE))
        .expect("the host sent the entity's FullBaseline");
    let delta_at = sent
        .iter()
        .position(|snapshot| record_kind(snapshot) == Some(RECORD_KIND_DELTA))
        .expect("the host sent a Delta for the entity");
    assert!(baseline_at < delta_at);
    NewEntityRun {
        baseline_sequence: sent[baseline_at].sequence,
        first_delta_sequence: sent[delta_at].sequence,
        frozen_snapshots: sent[baseline_at + 1..delta_at]
            .iter()
            .filter(|snapshot| record_kind(snapshot).is_none())
            .count(),
        full_baselines: sent[..delta_at]
            .iter()
            .filter(|snapshot| record_kind(snapshot) == Some(RECORD_KIND_FULL_BASELINE))
            .count(),
        ack_datagram,
        ack_arrived: h
            .to_server
            .channel_datagram(Channel::Input, ack_datagram)
            .and_then(|fate| fate.arrives_at)
            .is_some(),
    }
}

/// How many snapshots later the host's first Delta comes with the ack lost
/// once than on the same link without the loss.
fn lost_ack_cost(to_server: LinkConfig, to_client: LinkConfig) -> (NewEntityRun, NewEntityRun) {
    let clean = new_entity_run(to_server, to_client, false);
    let lost = new_entity_run(to_server, to_client, true);
    assert!(
        clean.ack_arrived,
        "precondition: the clean run delivers the ack"
    );
    assert!(!lost.ack_arrived, "the scripted drop lost the ack");
    assert_eq!(
        (clean.ack_datagram, clean.baseline_sequence),
        (lost.ack_datagram, lost.baseline_sequence),
        "both runs match up to the drop"
    );
    (clean, lost)
}

// P15: the client's ack of a new entity's FullBaseline rides Input. Lost once,
// it rides the next tick's datagram, so the host, which holds the entity until
// an ack covers its baseline, sends its first Delta at most one snapshot later.
// A fixed-delay link isolates the loss from jitter.
#[test]
fn lost_ack_delays_a_new_entitys_first_delta_by_at_most_one_snapshot() {
    let (clean, lost) = lost_ack_cost(fixed_delay_link(0x1610), fixed_delay_link(0x1611));
    eprintln!(
        "[lost ack, fixed delay] first Delta at sequence {} clean, {} with the ack lost \
         (baseline {}); frozen snapshots {} clean, {} lost",
        clean.first_delta_sequence,
        lost.first_delta_sequence,
        clean.baseline_sequence,
        clean.frozen_snapshots,
        lost.frozen_snapshots,
    );
    assert!(
        lost.first_delta_sequence <= clean.first_delta_sequence + 1,
        "the lost ack froze the entity {} snapshots past the clean run",
        lost.first_delta_sequence - clean.first_delta_sequence
    );
    assert_eq!(
        (clean.full_baselines, lost.full_baselines),
        (1, 1),
        "the late ack repairs nothing: one FullBaseline in each run"
    );
}

// Finding, not yet a contract, about the baseline ack window. Under the dev
// profile the snapshot carrying a new entity's FullBaseline can arrive after
// the next one. The client then acks the newer sequence without the entity, and
// only then the late baseline. An ack whose sequence covers the baseline's send
// but not its id closes the host's in-flight window
// (`ServerReplication::apply_ack`), so when the two acks reach the host on
// different polls with a snapshot encode between, the host sends a second
// FullBaseline and the entity freezes a further round trip. Here the entity
// ack's datagram is lost while the earlier ack's arrives, splitting them: the
// first Delta comes 6 snapshots past the clean run. The same split happens with
// no loss when jitter alone separates the two ack datagrams (2 of 63 clean
// dev-profile seeds swept).
#[test]
#[ignore = "finding: a reordered FullBaseline snapshot plus a split ack re-sends the baseline"]
fn reordered_baseline_with_a_lost_ack_sends_the_baseline_once() {
    let (to_server, to_client) = dev_links(0x1720);
    let (clean, lost) = lost_ack_cost(to_server, to_client);
    eprintln!(
        "[lost ack, dev profile 0x1720] first Delta at sequence {} clean, {} lost; FullBaselines {} clean, {} lost",
        clean.first_delta_sequence,
        lost.first_delta_sequence,
        clean.full_baselines,
        lost.full_baselines,
    );
    assert_eq!(lost.full_baselines, 1, "the host re-sent the FullBaseline");
}

// --- P16: sliced tuning on Control beside Input ---

/// The host Input message sent on frame `send_frame` carrying `token`.
struct HostInputProbe {
    send_frame: u64,
    token: WireActivationToken,
}

fn probe_outcome(token: WireActivationToken) -> ActivationOutcome {
    ActivationOutcome::InitiationRejected {
        token,
        recovery_ticks: 0,
    }
}

/// When the probe's message first reached the client's side of the link, in µs
/// after its send call, and how many copies were lost before that.
fn probe_arrival(h: &RelayHarness, probe: &HostInputProbe) -> (u64, usize) {
    let outcome = probe_outcome(probe.token);
    let carries = |datagram: &&LoggedDatagram| {
        server_inputs(datagram).iter().any(|message| {
            matches!(message,
                ServerMessage::ActivationOutcomes(outcomes) if outcomes.contains(&outcome))
        })
    };
    // Copies in flush order: the first copy and each resend until acked. A
    // later copy can overtake an earlier one under jitter, so the message
    // lands with whichever copy arrives first.
    let arrivals: Vec<Option<u64>> = h
        .down_log
        .iter()
        .filter(carries)
        .map(|datagram| {
            h.to_client
                .channel_datagram(Channel::Input, datagram.index.expect("an Input index"))
                .expect("the datagram was enqueued")
                .arrives_at
        })
        .collect();
    let lost = arrivals
        .iter()
        .take_while(|arrival| arrival.is_none())
        .count();
    let arrives_at = arrivals
        .iter()
        .flatten()
        .min()
        .unwrap_or_else(|| panic!("probe {:?} never arrived", probe.token));
    (arrives_at * 1000 - frame_start_us(probe.send_frame), lost)
}

const SLICED_TUNING_SEED: u64 = 0x1620;
/// Fluids on the level, enough to push the payload past one slice.
const SLICED_FLUIDS: usize = 90;

// P16: a tuning payload spanning several slices goes out on Control while the
// host sends an Input message every tick. Two of its slices are lost and resent
// at Control's 100 ms. No Input message waits on them: each still arrives
// within the single-loss bound, the largest one-way delay plus two ticks.
#[test]
fn sliced_tuning_under_loss_delays_no_input_message_past_the_single_loss_bound() {
    const NEW_GRAVITY: f32 = -15.0;
    const PROBE_FRAMES: u64 = 45;
    let (to_server, to_client) = dev_links(SLICED_TUNING_SEED);
    let mut h =
        RelayHarness::new(to_server, to_client).with_host_fluids(many_fluids(SLICED_FLUIDS));
    h.log_datagrams = true;
    h.join();
    for _ in 0..30 {
        h.frame(&idle_command());
    }

    // Lose the first copy of slices 0 and 2 of the next sliced Control message.
    let mut dropped = HashSet::new();
    h.down_drop = Some(Box::new(move |datagram| {
        control_slice(datagram).is_some_and(|(message_id, slice, _)| {
            [0, 2].contains(&slice) && dropped.insert((message_id, slice))
        })
    }));
    let log_start = h.down_log.len();
    // A mid-level gravity change resends the whole payload this frame.
    h.host_level_gravity = NEW_GRAVITY;
    let tuning_frame = h.frame + 1;
    let mut probes = Vec::new();
    for step in 0..PROBE_FRAMES {
        h.frame(&idle_command());
        let token = WireActivationToken {
            start_tick: 0x1600 + step as u32,
            lane: 0,
        };
        h.server.send_input(
            CLIENT_ID,
            wire::encode(&ServerMessage::ActivationOutcomes(vec![probe_outcome(
                token,
            )])),
        );
        probes.push(HostInputProbe {
            send_frame: h.frame,
            token,
        });
    }
    for _ in 0..30 {
        h.frame(&idle_command());
    }

    // The payload went out sliced, two slices were lost, and the client
    // installed it once the resends landed.
    let slices: Vec<(u64, u64, u64, u64)> = h.down_log[log_start..]
        .iter()
        .filter_map(|datagram| {
            control_slice(datagram).map(|(id, slice, count)| (datagram.frame, id, slice, count))
        })
        .collect();
    let num_slices = slices.first().map(|&(.., count)| count).unwrap_or(0);
    assert!(num_slices >= 2, "the payload spans slices: {slices:?}");
    assert_eq!(h.to_client.scripted_dropped(), 2);
    let installed_frame = h
        .drained_controls
        .iter()
        .find(|(frame, control)| {
            *frame >= tuning_frame
                && *control
                    == DrainedControl::Tuning {
                        level_gravity: Some(NEW_GRAVITY),
                    }
        })
        .map(|&(frame, _)| frame)
        .expect("the client installs the resent payload");
    let last_slice_frame = slices.iter().map(|&(frame, ..)| frame).max().unwrap();
    assert!(
        last_slice_frame > tuning_frame + 1,
        "the lost slices were resent after the first flush: {slices:?}"
    );

    // Every Input message sent while the slices were in flight.
    let in_flight: Vec<&HostInputProbe> = probes
        .iter()
        .filter(|probe| probe.send_frame < installed_frame)
        .collect();
    assert!(
        in_flight.len() >= 10,
        "the slices stayed in flight across {} Input sends",
        in_flight.len()
    );
    let mut report = String::new();
    let mut worst = 0;
    for probe in &in_flight {
        let (arrival_us, lost) = probe_arrival(&h, probe);
        assert!(
            lost <= 1,
            "precondition: probe {} lost {lost} copies, past a single loss",
            probe.token.start_tick
        );
        worst = worst.max(arrival_us);
        let _ = write!(
            report,
            "{}{} ",
            arrival_us / 1000,
            if lost == 1 { "*" } else { "" }
        );
        assert!(
            arrival_us <= SINGLE_LOSS_BOUND_US,
            "Input sent on frame {} arrived {arrival_us} us after its send, past \
             {SINGLE_LOSS_BOUND_US} us, while tuning slices were in flight",
            probe.send_frame
        );
    }
    eprintln!(
        "[sliced tuning] {num_slices} slices, 2 lost; sent frame {tuning_frame}, last slice \
         flushed frame {last_slice_frame}, installed frame {installed_frame}; {} Input \
         messages in flight, worst {worst} us of {SINGLE_LOSS_BOUND_US} us; arrivals ms \
         (* = own copy lost once): {report}",
        in_flight.len(),
    );
}

// --- P17: tuning across a demotion or level change ---

#[derive(Debug, Clone, Copy)]
enum Retire {
    /// The host holds the slot for a parity change.
    Demotion,
    /// The host installs another level, which holds every slot; the client
    /// unloads and follows.
    LevelChange,
}

const NEXT_LEVEL: &str = "test-level-2";

fn host_retires(h: &mut RelayHarness, retire: Retire) {
    match retire {
        Retire::Demotion => h.server.set_level_parity(None),
        Retire::LevelChange => {
            h.server
                .set_level_parity(Some((NEXT_LEVEL.to_string(), RELAY_FINGERPRINT)));
            h.server.set_revealed_level(Some(NEXT_LEVEL.to_string()));
        }
    }
}

/// The re-promotion: the host's parity returns, or the client follows the
/// host to its new level.
fn slot_returns(h: &mut RelayHarness, retire: Retire) {
    match retire {
        Retire::Demotion => h
            .server
            .set_level_parity(Some((RELAY_LEVEL.to_string(), RELAY_FINGERPRINT))),
        Retire::LevelChange => {
            h.endpoint.reset_level_scoped_client_state();
            let NetEndpoint::Client { client, .. } = &mut h.endpoint else {
                unreachable!("the relay endpoint is a client");
            };
            client.set_level_parity(Some((NEXT_LEVEL.to_string(), RELAY_FINGERPRINT)));
            client.set_revealed_level(Some(NEXT_LEVEL.to_string()));
        }
    }
}

/// Send a sliced payload, lose its first slice, and retire the slot one frame
/// later, so the slice's resend leaves after the Holding. The payload must
/// never apply after the Holding, and the payload sent after re-promotion
/// installs.
fn tuning_across_retirement(retire: Retire) {
    const OLD_GRAVITY: f32 = -15.0;
    const NEW_GRAVITY: f32 = -12.0;
    let mut h = RelayHarness::new(fixed_delay_link(0x1630), fixed_delay_link(0x1631))
        .with_host_fluids(many_fluids(SLICED_FLUIDS));
    h.log_datagrams = true;
    h.join();
    for _ in 0..20 {
        h.frame(&idle_command());
    }
    let mut dropped = false;
    h.down_drop = Some(Box::new(move |datagram| {
        let first_slice = control_slice(datagram).is_some_and(|(_, slice, _)| slice == 0);
        let drop = first_slice && !dropped;
        dropped |= drop;
        drop
    }));
    let log_start = h.down_log.len();
    let drained_start = h.drained_controls.len();

    // The old payload is queued this frame and flushed, minus its first
    // slice, at the next poll. The poll after that holds the slot.
    h.host_level_gravity = OLD_GRAVITY;
    h.frame(&idle_command());
    h.frame(&idle_command());
    host_retires(&mut h, retire);
    for _ in 0..30 {
        h.frame(&idle_command());
        assert!(
            !h.drained_controls[drained_start..]
                .iter()
                .any(|(_, control)| *control == DrainedControl::Holding)
                || h.client_tuning().is_none(),
            "{retire:?}: a tuning payload is installed after the Holding"
        );
    }

    // The Holding left before the lost slice's resend, and the client still
    // drained the old payload first: Control's order held across the loss.
    let down = &h.down_log[log_start..];
    let holding_flush = down
        .iter()
        .find(|datagram| carries_holding(datagram))
        .map(|datagram| datagram.frame)
        .expect("the host sent the Holding");
    let first_slice_flushes: Vec<u64> = down
        .iter()
        .filter(|datagram| control_slice(datagram).is_some_and(|(_, slice, _)| slice == 0))
        .map(|datagram| datagram.frame)
        .collect();
    assert_eq!(h.to_client.scripted_dropped(), 1);
    assert!(
        first_slice_flushes.len() >= 2 && first_slice_flushes[1] > holding_flush,
        "{retire:?}: the lost slice is resent after the Holding was flushed \
         (slice 0 flushes {first_slice_flushes:?}, Holding {holding_flush})"
    );
    let drained = &h.drained_controls[drained_start..];
    assert_eq!(
        drained
            .iter()
            .map(|&(_, control)| control)
            .collect::<Vec<_>>(),
        [
            DrainedControl::Tuning {
                level_gravity: Some(OLD_GRAVITY)
            },
            DrainedControl::Holding,
        ],
        "{retire:?}: the old payload drains before the Holding, never after it"
    );
    assert_eq!(drained[0].0, drained[1].0, "one drain released both");
    assert!(h.client_tuning().is_none(), "the Holding dropped it");

    // Permit: the payload sent after re-promotion installs.
    h.host_level_gravity = NEW_GRAVITY;
    slot_returns(&mut h, retire);
    let mut installed = None;
    for _ in 0..JOIN_FRAMES_MAX {
        h.frame(&idle_command());
        if let Some(payload) = h.client_tuning() {
            installed = Some(payload.environment.level_gravity);
            break;
        }
    }
    assert_eq!(
        installed,
        Some(NEW_GRAVITY),
        "{retire:?}: the payload sent after re-promotion installs"
    );
    assert!(h.both_participating());
    let after: Vec<DrainedControl> = h.drained_controls[drained_start + 2..]
        .iter()
        .map(|&(_, control)| control)
        .collect();
    assert_eq!(
        after,
        [DrainedControl::Tuning {
            level_gravity: Some(NEW_GRAVITY)
        }],
        "{retire:?}: nothing but the new payload reached the client after the Holding"
    );
}

// P17: Control's ordering keeps a payload sent before a demotion from landing
// after it, even when one of its slices is lost and resent after the Holding.
#[test]
fn tuning_sent_before_a_demotion_never_applies_after_it_and_the_next_one_installs() {
    tuning_across_retirement(Retire::Demotion);
}

// P17, the level-change path: the hold that a host level change sends drops
// the old level's payload, and the next level's installs after re-promotion.
#[test]
fn tuning_sent_before_a_level_change_never_applies_after_it_and_the_next_one_installs() {
    tuning_across_retirement(Retire::LevelChange);
}

// --- Upstream cost ---

const UPSTREAM_SEED: u64 = 0x1640;
/// A minute of the course at 60 Hz: fifteen course cycles, each with a slide
/// and both strafes.
const UPSTREAM_FRAMES: u64 = 60 * 60;
/// The bound, in bytes per second (1 KB = 1000 B).
const UPSTREAM_MAX_BYTES_PER_SECOND: f64 = 40_000.0;
/// renetcode's per-datagram overhead on a payload packet: one prefix byte, a
/// sequence of up to three bytes at this run's packet count, and the 16-byte
/// AEAD tag (`renetcode` 2.0.0 `Packet::encode`).
const NETCODE_OVERHEAD_BYTES: u64 = 1 + 3 + 16;
/// UDP (8) and IPv4 (20) headers.
const UDP_IPV4_OVERHEAD_BYTES: u64 = 28;
/// A round, conservative per-datagram allowance for everything below renet:
/// twice the 48 B netcode plus UDP/IPv4 estimate above, leaving room for link
/// framing. A margin, not a measurement.
const CONSERVATIVE_OVERHEAD_BYTES: u64 = 96;

/// The client's upstream since `h.uplink_bytes` was last reset, over `seconds`:
/// the renet-level rate in bytes per second and a per-channel summary.
fn upstream_report(h: &RelayHarness, seconds: f64, label: &str) -> (f64, String) {
    let rate = |bytes: u64| bytes as f64 / seconds;
    let up = h.uplink_bytes;
    let mut breakdown = String::new();
    for (channel, name) in [
        (Channel::Control, "Control"),
        (Channel::Snapshot, "Snapshot"),
        (Channel::Input, "Input"),
        (Channel::Presentation, "Presentation"),
    ] {
        let (datagrams, bytes) = up.by_channel[usize::from(u8::from(channel))];
        let _ = write!(
            breakdown,
            "{name} {:.0} B/s in {:.1} datagrams/s; ",
            rate(bytes),
            rate(datagrams)
        );
    }
    let _ = write!(
        breakdown,
        "renet acks {:.0} B/s in {:.1} datagrams/s",
        rate(up.acks.1),
        rate(up.acks.0)
    );
    let renet_rate = rate(up.bytes);
    let datagram_rate = rate(up.datagrams);
    let wire_rate =
        renet_rate + datagram_rate * (NETCODE_OVERHEAD_BYTES + UDP_IPV4_OVERHEAD_BYTES) as f64;
    let conservative_rate = renet_rate + datagram_rate * CONSERVATIVE_OVERHEAD_BYTES as f64;
    let summary = format!(
        "client upstream over {seconds:.0} s {label}: renet {renet_rate:.0} B/s \
         ({:.2} KB/s, 1 KB = 1000 B) in {datagram_rate:.1} datagrams/s, mean {:.1} B; \
         {breakdown}; estimated on the wire {wire_rate:.0} B/s at {} B/datagram \
         (netcode {NETCODE_OVERHEAD_BYTES} + UDP/IPv4 {UDP_IPV4_OVERHEAD_BYTES}), \
         {conservative_rate:.0} B/s at a conservative {CONSERVATIVE_OVERHEAD_BYTES} B/datagram",
        renet_rate / 1000.0,
        up.bytes as f64 / up.datagrams as f64,
        NETCODE_OVERHEAD_BYTES + UDP_IPV4_OVERHEAD_BYTES,
    );
    (renet_rate, summary)
}

/// Join over `to_server` / `to_client`, warm up two seconds of the course, then
/// count a minute of the client's upstream with the client polling at
/// `client_poll_hz` (60 is one poll per fixed tick, [`RelayHarness::frame`]).
fn measure_upstream(
    to_server: LinkConfig,
    to_client: LinkConfig,
    client_poll_hz: u64,
    label: &str,
) -> (f64, String) {
    let mut h = RelayHarness::new(to_server, to_client);
    h.join();
    // The course starts at its first frame once joined, whatever the join took.
    let joined = h.frame;
    let run = |h: &mut RelayHarness, frames: std::ops::Range<u64>| {
        if client_poll_hz == 60 {
            for frame in frames {
                h.frame(&course_command(frame));
            }
        } else {
            h.run_with_client_poll_rate(frames.end - frames.start, client_poll_hz, |tick| {
                course_command(tick - joined - 1)
            });
        }
    };
    run(&mut h, 0..120);
    h.uplink_bytes = Default::default();
    run(&mut h, 120..120 + UPSTREAM_FRAMES);
    assert!(h.both_participating());
    let (rate, summary) = upstream_report(&h, UPSTREAM_FRAMES as f64 / 60.0, label);
    eprintln!("[upstream] {summary}");
    (rate, summary)
}

// The client's upstream under the dev profile with no outage: input commands,
// acks, time-sync probes and renet acks, every unacked Input message resent on
// each 60 Hz poll. Measured at renet's connection level, what the relay sees.
#[test]
fn client_upstream_bytes_under_dev_profile_stay_within_40_kb_per_second() {
    let (to_server, to_client) = dev_links(UPSTREAM_SEED);
    let (renet_rate, summary) = measure_upstream(to_server, to_client, 60, "at 60 Hz");
    assert!(
        renet_rate <= UPSTREAM_MAX_BYTES_PER_SECOND,
        "client upstream past 40 KB/s: {summary}"
    );
}

// Measurement only, beside the 60 Hz bound: the same course under the dev
// profile with the client polling at 144 Hz. An unacked Input message is resent
// on every poll at least 10 ms after its last send, so a faster renderer resends
// more often. No bound is asserted; the figure is recorded in networking.md
// §Channel model.
#[test]
fn client_upstream_bytes_at_144_hz_polls_are_reported() {
    let (to_server, to_client) = dev_links(UPSTREAM_SEED);
    let (renet_rate, _) = measure_upstream(to_server, to_client, 144, "at 144 Hz client polls");
    assert!(renet_rate > 0.0);
}

/// About 250 ms RTT: 125 ms each way, no jitter, the dev profile's 5% loss.
fn long_rtt_link(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 125,
        jitter: 0,
        ..dev_profile(seed)
    }
}

// Measurement only, beside the 60 Hz bound: the same course at 60 Hz over a
// 250 ms RTT link. Every Input message stays unacked for about one RTT and is
// resent on each poll meanwhile, so upstream grows with RTT. No bound is
// asserted; the figure is recorded in networking.md §Channel model.
#[test]
fn client_upstream_bytes_at_250_ms_rtt_are_reported() {
    let (renet_rate, _) = measure_upstream(
        long_rtt_link(UPSTREAM_SEED),
        long_rtt_link(UPSTREAM_SEED.wrapping_add(1)),
        60,
        "at 60 Hz, 125 ms one-way, no jitter, 5% loss",
    );
    assert!(renet_rate > 0.0);
}

// --- Join: a lost tuning datagram ---

const JOIN_SEED: u64 = 0x1650;

/// The frame the client's armed pawn first composes its inventory from the
/// payload, on a join where `drop` picks the host datagrams to lose.
fn inventory_composed_frame(
    to_server: LinkConfig,
    to_client: LinkConfig,
    fluids: Option<FluidMovementTable>,
    drop: Option<fn(&LoggedDatagram) -> bool>,
) -> (u64, RelayHarness) {
    let mut h = RelayHarness::new(to_server, to_client);
    if let Some(fluids) = fluids {
        h = h.with_host_fluids(fluids);
    }
    h.log_datagrams = true;
    if let Some(drop) = drop {
        let mut dropped = false;
        h.down_drop = Some(Box::new(move |datagram| {
            let lose = !dropped && drop(datagram);
            dropped |= lose;
            lose
        }));
    }
    for _ in 0..JOIN_FRAMES_MAX {
        h.frame(&idle_command());
        let composed = h
            .client_pawn
            .is_some_and(|pawn| h.client_registry.get_component::<Inventory>(pawn).is_ok());
        if composed {
            return (h.frame, h);
        }
    }
    panic!("the client pawn never composed its inventory");
}

/// Compose on join with and without the drop, on the same seeds. Returns the
/// delay the drop caused in µs, after checking the drop was a single loss of
/// the datagram it targeted.
fn join_delay(fluids: Option<FluidMovementTable>, drop: fn(&LoggedDatagram) -> bool) -> u64 {
    let (to_server, to_client) = dev_links(JOIN_SEED);
    let (clean_frame, clean) = inventory_composed_frame(to_server, to_client, fluids.clone(), None);
    let (lost_frame, lost) = inventory_composed_frame(to_server, to_client, fluids, Some(drop));
    let target = lost
        .down_log
        .iter()
        .find(|datagram| drop(datagram))
        .expect("the targeted datagram was flushed");
    let index = target.index.expect("a Control index");
    let fate = |h: &RelayHarness| h.to_client.channel_datagram(Channel::Control, index);
    assert!(
        fate(&clean).and_then(|fate| fate.arrives_at).is_some(),
        "precondition: the clean run delivers the targeted datagram"
    );
    assert_eq!(lost.to_client.scripted_dropped(), 1);
    let resend_arrived = lost.down_log.iter().any(|datagram| {
        datagram.frame > target.frame
            && drop(datagram)
            && lost
                .to_client
                .channel_datagram(Channel::Control, datagram.index.unwrap())
                .and_then(|fate| fate.arrives_at)
                .is_some()
    });
    assert!(
        resend_arrived,
        "precondition: the first resend arrives, so this is a single loss"
    );
    assert!(lost_frame >= clean_frame);
    frame_start_us(lost_frame) - frame_start_us(clean_frame)
}

// A joiner's pawn holds no inventory until the tuning payload installs. The
// harness payload fits one slice, so "a lost tuning slice" is the lost Control
// datagram carrying it: Control resends it at 100 ms, so composition waits at
// most that resend plus one transit longer than on the same seed without it.
#[test]
fn lost_tuning_datagram_on_join_delays_inventory_by_at_most_one_control_resend_and_transit() {
    let delay = join_delay(None, carries_whole_tuning);
    eprintln!(
        "[join, one-datagram payload] inventory composed {delay} us later with the tuning \
         datagram lost, bound {} us",
        CONTROL_RESEND_US + LARGEST_ONE_WAY_US
    );
    assert!(
        delay <= CONTROL_RESEND_US + LARGEST_ONE_WAY_US,
        "the lost tuning datagram delayed inventory composition {delay} us"
    );
}

// The same join with a payload spanning several slices, one of them lost.
#[test]
fn lost_tuning_slice_on_join_delays_inventory_by_at_most_one_control_resend_and_transit() {
    fn second_slice(datagram: &LoggedDatagram) -> bool {
        control_slice(datagram).is_some_and(|(_, slice, _)| slice == 1)
    }
    let delay = join_delay(Some(many_fluids(SLICED_FLUIDS)), second_slice);
    eprintln!(
        "[join, sliced payload] inventory composed {delay} us later with one slice lost, \
         bound {} us",
        CONTROL_RESEND_US + LARGEST_ONE_WAY_US
    );
    assert!(
        delay <= CONTROL_RESEND_US + LARGEST_ONE_WAY_US,
        "the lost tuning slice delayed inventory composition {delay} us"
    );
}
