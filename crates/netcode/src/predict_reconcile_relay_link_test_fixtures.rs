// The relay link layer both predict/reconcile harnesses run on: a renet
// `NetServer`/`NetClient` pair on the production channel configuration, joined by
// two `PacketConditioner`s, flushed at each peer's poll on a virtual 60 Hz clock.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/testing_guide.md §4
//
// Test infrastructure only. `RelayHarness` (`predict_reconcile_relay_test_fixtures`)
// and `LoopbackHarness` (`predict_reconcile_loopback_test_fixtures`) build their
// peers on this; neither stands in for the transport.

#![cfg(test)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_net::harness::{
    LinkConfig, PacketConditioner, RenetDatagram, ScriptedDrop, parse_renet_datagram,
    renet_packet_channel, server_control_payload,
};
use postretro_net::transport::{Channel, NetClient, NetServer};
use postretro_net::wire::{self, ServerControlMessage};

use super::predict_reconcile_harness_test_fixtures::CLIENT_ID;

/// The owner's dev lag proxy preset (`networking.md` §Testing the conditioned
/// link), for one direction: 45 ms + up to 60 ms jitter + 5% loss.
pub(crate) fn dev_profile(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed,
    }
}

/// An arbitrary origin for the client's monotonic clock, unrelated to the host
/// tick, so time sync must recover the offset rather than read it.
pub(crate) const CLIENT_CLOCK_ORIGIN_US: u64 = 7_000_000;

/// Virtual time at the start of frame `frame` of a 60 Hz run, in microseconds.
pub(crate) fn frame_start_us(frame: u64) -> u64 {
    frame * 1_000_000 / 60
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
    let fingerprint = [0x5a; 32];
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
    server.set_level_parity(Some(("test-level".to_string(), fingerprint)));
    client.set_level_parity(Some(("test-level".to_string(), fingerprint)));
    postretro_net::harness::reveal_both(&mut server, &mut client, "test-level");
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
