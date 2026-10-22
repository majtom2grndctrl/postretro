// The relay link layer both predict/reconcile harnesses run on: a renet
// `NetServer`/`NetClient` pair on the production channel configuration, joined by
// two `PacketConditioner`s, flushed at each peer's poll on a virtual clock. Both
// peers poll at 60 Hz, or the client polls faster to measure its upstream bytes
// at a render rate.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/testing_guide.md §4
//
// Test infrastructure only. `RelayHarness` (`predict_reconcile_relay_test_fixtures`)
// and `LoopbackHarness` (`predict_reconcile_loopback_test_fixtures`) build their
// peers on this; neither stands in for the transport.

#![cfg(test)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_net::harness::{
    LinkConfig, PacketConditioner, RenetDatagram, ScriptedDrop, VirtualMillis,
    parse_renet_datagram, participation_frame_payload, renet_packet_channel,
    server_control_payload,
};
use postretro_net::transport::{Channel, NetClient, NetServer};
use postretro_net::wire::{self, ClientMessage, ServerControlMessage, ServerMessage};

use super::SERVER_TICK_MICROS;
use super::predict_reconcile_harness_test_fixtures::CLIENT_ID;

/// The level both relay peers load and reveal, so the slot participates.
pub(crate) const RELAY_LEVEL: &str = "test-level";
/// [`RELAY_LEVEL`]'s content fingerprint, which both peers also declare as
/// their mod digest.
pub(crate) const RELAY_FINGERPRINT: [u8; 32] = [0x5a; 32];

/// Frames a relay client may take to join and start predicting.
pub(crate) const JOIN_FRAMES_MAX: u64 = 600;

/// The scripted total outage, both directions: longer than any redundancy.
pub(crate) const OUTAGE_MS: VirtualMillis = 2_000;

/// The owner's dev lag proxy preset (`networking.md` §Testing the conditioned
/// link), for one direction: 45 ms + up to 60 ms jitter + 5% loss.
pub(crate) const fn dev_profile(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed,
    }
}

/// The dev profile both ways. The to-client link takes the next seed, so the
/// two directions lose and jitter independently.
pub(crate) const fn dev_links(seed: u64) -> (LinkConfig, LinkConfig) {
    (dev_profile(seed), dev_profile(seed.wrapping_add(1)))
}

/// The dev profile's base one-way delay with no jitter and no loss, so a
/// scripted drop is the only thing that moves an arrival.
pub(crate) const fn fixed_delay_link(seed: u64) -> LinkConfig {
    LinkConfig {
        jitter: 0,
        loss_probability: 0.0,
        ..dev_profile(seed)
    }
}

/// The largest one-way delay `link` draws, in µs: its base delay plus all of
/// its jitter.
pub(crate) const fn largest_one_way_us(link: LinkConfig) -> u64 {
    (link.delay + link.jitter) * 1000
}

/// The dev profile's largest one-way delay, in µs.
pub(crate) const LARGEST_ONE_WAY_US: u64 = largest_one_way_us(dev_profile(0));

/// The single-loss bound on an Input message under the dev profile: the
/// largest one-way delay, plus one frame waiting for the poll that flushes the
/// send and one for the poll that resends it.
pub(crate) const SINGLE_LOSS_BOUND_US: u64 = LARGEST_ONE_WAY_US + 2 * SERVER_TICK_MICROS;

/// An arbitrary origin for the client's monotonic clock, unrelated to the host
/// tick, so time sync must recover the offset rather than read it.
pub(crate) const CLIENT_CLOCK_ORIGIN_US: u64 = 7_000_000;

/// Virtual time at the start of frame `frame` of a 60 Hz run, in microseconds.
pub(crate) fn frame_start_us(frame: u64) -> u64 {
    frame * 1_000_000 / 60
}

/// The first frame whose poll takes a datagram arriving at `arrives_at`: the
/// conditioners advance in whole milliseconds of [`frame_start_us`], and a
/// poll takes everything due at or before its time.
pub(crate) fn first_poll_frame(arrives_at: VirtualMillis) -> u64 {
    (0..)
        .find(|&frame| frame_start_us(frame) / 1000 >= arrives_at)
        .expect("a poll follows every arrival")
}

/// The relay's four channels by renet channel id.
pub(crate) fn channel_from_id(id: u8) -> Option<Channel> {
    [
        Channel::Control,
        Channel::Snapshot,
        Channel::Input,
        Channel::Presentation,
    ]
    .into_iter()
    .find(|channel| u8::from(*channel) == id)
}

/// One datagram a peer flushed onto its link, read at the flush.
#[derive(Debug, Clone)]
pub(crate) struct LoggedDatagram {
    /// The frame whose poll flushed it.
    pub(crate) frame: u64,
    pub(crate) channel: Option<Channel>,
    /// Its index among `channel`'s datagrams on its link: the
    /// [`ScriptedDrop::NthOnChannel`] index and the
    /// [`PacketConditioner::channel_datagram`] key.
    pub(crate) index: Option<u64>,
    pub(crate) contents: Option<RenetDatagram>,
}

/// A datagram predicate: `true` drops it on the link (a scripted drop).
pub(crate) type DatagramDrop = Box<dyn FnMut(&LoggedDatagram) -> bool>;

/// renet connection-level bytes one peer emitted, by datagram kind: what the
/// relay sees, renet payload plus renet headers, before netcode encryption
/// and UDP.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LinkBytes {
    pub(crate) datagrams: u64,
    pub(crate) bytes: u64,
    /// Per renet channel id: datagrams and bytes.
    pub(crate) by_channel: [(u64, u64); 4],
    /// renet's own ack packets: datagrams and bytes.
    pub(crate) acks: (u64, u64),
}

impl LinkBytes {
    fn count(&mut self, packet: &[u8]) {
        let bytes = packet.len() as u64;
        self.datagrams += 1;
        self.bytes += bytes;
        let slot = match renet_packet_channel(packet) {
            Some(channel) => self.by_channel.get_mut(usize::from(channel)),
            None => Some(&mut self.acks),
        };
        if let Some((datagrams, total)) = slot {
            *datagrams += 1;
            *total += bytes;
        }
    }
}

/// Enqueue one peer's flush on `link`, logging each datagram with its channel
/// index when `log` is given, dropping the ones `drop` picks, and counting
/// bytes into `bytes`.
pub(crate) fn flush(
    link: &mut PacketConditioner,
    packets: Vec<Vec<u8>>,
    frame: u64,
    mut log: Option<&mut Vec<LoggedDatagram>>,
    mut drop: Option<&mut DatagramDrop>,
    mut bytes: Option<&mut LinkBytes>,
) {
    for packet in packets {
        if let Some(bytes) = bytes.as_deref_mut() {
            bytes.count(&packet);
        }
        if log.is_some() || drop.is_some() {
            let channel = renet_packet_channel(&packet).and_then(channel_from_id);
            let datagram = LoggedDatagram {
                frame,
                channel,
                index: channel.map(|channel| link.enqueued_on_channel(channel)),
                contents: parse_renet_datagram(&packet),
            };
            if let (Some(drop), Some(channel), Some(index)) =
                (drop.as_deref_mut(), datagram.channel, datagram.index)
                && drop(&datagram)
            {
                link.script_drop(ScriptedDrop::NthOnChannel { channel, index });
            }
            if let Some(log) = log.as_deref_mut() {
                log.push(datagram);
            }
        }
        link.enqueue(packet);
    }
}

/// A server/client transport pair joined by in-memory relay, both declaring
/// the same mod, level and reveal so the slot will participate.
pub(crate) fn relay_pair() -> (NetServer, NetClient) {
    let origin = Duration::from_secs(1);
    let server_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("relay server binds loopback socket");
    let server_addr: SocketAddr = server_socket
        .local_addr()
        .expect("relay server resolves loopback address");
    let client_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("relay client binds loopback socket");
    let fingerprint = RELAY_FINGERPRINT;
    let mut server = NetServer::new(server_socket, server_addr, 1, origin, Some(fingerprint))
        .expect("relay server transport constructs");
    let mut client = NetClient::new(
        client_socket,
        server_addr,
        CLIENT_ID,
        origin,
        Some(fingerprint),
        None,
    )
    .expect("relay client transport constructs");
    server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    server.set_mod_digest(Some(fingerprint));
    client.set_mod_digest(Some(fingerprint));
    server.set_level_parity(Some((RELAY_LEVEL.to_string(), fingerprint)));
    client.set_level_parity(Some((RELAY_LEVEL.to_string(), fingerprint)));
    postretro_net::harness::reveal_both(&mut server, &mut client, RELAY_LEVEL);
    (server, client)
}

/// Whether a host datagram carries a tuning payload: whole in a reliable
/// Control datagram, or one slice of it (tuning is the only Control message
/// past one slice).
pub(crate) fn carries_tuning(datagram: &LoggedDatagram) -> bool {
    if datagram.channel != Some(Channel::Control) {
        return false;
    }
    match &datagram.contents {
        Some(RenetDatagram::Reliable { messages, .. }) => messages.iter().any(|(_, bytes)| {
            server_control_payload(bytes)
                .and_then(|payload| wire::decode::<ServerControlMessage>(&payload).ok())
                .is_some_and(|message| matches!(message, ServerControlMessage::Tuning(_)))
        }),
        Some(RenetDatagram::Slice { reliable: true, .. }) => true,
        _ => false,
    }
}

/// The reliable Input messages a datagram carries, each with its renet message
/// id, as the transport's bytes.
pub(crate) fn input_messages(datagram: &LoggedDatagram) -> &[(u64, Vec<u8>)] {
    match &datagram.contents {
        Some(RenetDatagram::Reliable { messages, .. })
            if datagram.channel == Some(Channel::Input) =>
        {
            messages.as_slice()
        }
        _ => &[],
    }
}

/// One client Input message, its participation frame removed and decoded.
pub(crate) fn decode_client_input(bytes: &[u8]) -> Option<ClientMessage> {
    participation_frame_payload(bytes).and_then(|payload| wire::decode(&payload).ok())
}

/// The client Input messages an uplink datagram carries, decoded.
pub(crate) fn client_inputs(datagram: &LoggedDatagram) -> Vec<ClientMessage> {
    input_messages(datagram)
        .iter()
        .filter_map(|(_, bytes)| decode_client_input(bytes))
        .collect()
}

/// The host Input messages a downlink datagram carries, decoded. Host Input
/// carries no participation frame.
pub(crate) fn server_inputs(datagram: &LoggedDatagram) -> Vec<ServerMessage> {
    input_messages(datagram)
        .iter()
        .filter_map(|(_, bytes)| wire::decode(bytes).ok())
        .collect()
}
