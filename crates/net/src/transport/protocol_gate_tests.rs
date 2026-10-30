// Gate 1 run live: the real renet_netcode handshake over loopback UDP between
// this build's endpoints and peers packing an earlier protocol pair.
// See: context/lib/networking.md §Two-gate handshake

use super::*;

/// The pair the gameplay channel layout replaced: PRLB / wire 32.
const PRLB_32: u64 = (0x_5052_4C42_u64 << 32) | 32;
/// Mixed pairs: a stale app id or a stale wire version alone.
const PRLB_33: u64 = (0x_5052_4C42_u64 << 32) | 33;
const PRLC_32: u64 = (0x_5052_4C43_u64 << 32) | 32;
/// The pair the ghost-link record replaced: PRLC / wire 33.
const PRLC_33: u64 = (0x_5052_4C43_u64 << 32) | 33;
/// The pair the switch declaration kind replaced: PRLC / wire 34.
const PRLC_34: u64 = (0x_5052_4C43_u64 << 32) | 34;
/// The pair the snapshot's host level tick replaced: PRLC / wire 35.
const PRLC_35: u64 = (0x_5052_4C43_u64 << 32) | 35;
/// The pair `Relevel`'s host level life replaced: PRLC / wire 36.
const PRLC_36: u64 = (0x_5052_4C43_u64 << 32) | 36;

const STEP: Duration = Duration::from_millis(16);
/// Polls before the matching peer must have connected.
const CONNECT_POLLS: usize = 200;
/// Polls the stale peers keep trying after the matching one connected: ~1.6 s
/// of netcode time, past six of netcode's 250 ms connect-request resends.
const REFUSAL_POLLS: usize = 100;

fn loopback_socket() -> UdpSocket {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback socket")
}

/// A bare netcode client packing `protocol_id`, standing in for another build.
struct RawClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
}

impl RawClient {
    fn new(server_addr: SocketAddr, client_id: u64, protocol_id: u64) -> Self {
        let transport = NetcodeClientTransport::new(
            Duration::from_secs(1),
            ClientAuthentication::Unsecure {
                client_id,
                protocol_id,
                server_addr,
                user_data: None,
            },
            loopback_socket(),
        )
        .expect("construct raw client transport");
        Self {
            client: RenetClient::new(connection_config()),
            transport,
        }
    }

    fn update(&mut self) {
        self.client.update(STEP);
        self.transport
            .update(STEP, &mut self.client)
            .expect("raw client update");
        self.transport
            .send_packets(&mut self.client)
            .expect("raw client send");
    }
}

/// A bare netcode host packing `protocol_id`, standing in for another build.
struct RawServer {
    server: RenetServer,
    transport: NetcodeServerTransport,
    addr: SocketAddr,
}

impl RawServer {
    fn new(protocol_id: u64) -> Self {
        let socket = loopback_socket();
        let addr = socket.local_addr().expect("raw server address");
        let transport = NetcodeServerTransport::new(
            ServerConfig {
                current_time: Duration::from_secs(1),
                max_clients: 8,
                protocol_id,
                public_addresses: vec![addr],
                authentication: ServerAuthentication::Unsecure,
            },
            socket,
        )
        .expect("construct raw server transport");
        Self {
            server: RenetServer::new(connection_config()),
            transport,
            addr,
        }
    }

    fn update(&mut self) {
        self.server.update(STEP);
        self.transport
            .update(STEP, &mut self.server)
            .expect("raw server update");
        self.transport.send_packets(&mut self.server);
    }
}

/// Runs `poll`, which reports whether the control peer has connected, until it
/// does and then `REFUSAL_POLLS` more, so peers that should be refused get the
/// same link time and several more requests. Returns whether the control peer
/// connected within `CONNECT_POLLS`.
fn poll_past_connect(mut poll: impl FnMut() -> bool) -> bool {
    let mut polls_left = None;
    for _ in 0..CONNECT_POLLS + REFUSAL_POLLS {
        let connected = poll();
        if polls_left.is_none() && connected {
            polls_left = Some(REFUSAL_POLLS);
        }
        match &mut polls_left {
            Some(0) => return true,
            Some(left) => *left -= 1,
            None => {}
        }
        // Loopback delivery is near-instant; the pause only lets the datagrams
        // sent this poll land before the next receive.
        std::thread::sleep(Duration::from_millis(2));
    }
    polls_left.is_some()
}

// A peer packing an earlier (app id, wire version) pair never completes the
// encrypted netcode handshake with this build's host, while a matching client
// on the same host and the same polls connects: refusal, not a silent link.
#[test]
fn netcode_handshake_refuses_stale_protocol_clients_and_connects_a_matching_one() {
    assert_eq!(transport_protocol_id(), (0x_5052_4C43_u64 << 32) | 37);
    let socket = loopback_socket();
    let server_addr = socket.local_addr().expect("server address");
    let mut server = NetServer::new(socket, server_addr, 8, Duration::from_secs(1), None)
        .expect("construct server");
    let mut matching = NetClient::new(
        loopback_socket(),
        server_addr,
        7,
        Duration::from_secs(1),
        None,
        None,
    )
    .expect("construct client");
    let mut stale: Vec<(u64, RawClient)> = [
        (101, PRLB_32),
        (102, PRLB_33),
        (103, PRLC_32),
        (104, PRLC_33),
        (105, PRLC_34),
        (106, PRLC_35),
        (107, PRLC_36),
    ]
    .into_iter()
    .map(|(id, protocol_id)| (protocol_id, RawClient::new(server_addr, id, protocol_id)))
    .collect();

    let reached = poll_past_connect(|| {
        matching.update(STEP).expect("matching client update");
        for (_, client) in &mut stale {
            client.update();
        }
        let _ = server.update(STEP).expect("server update");
        matching.is_connected()
    });

    assert!(reached, "the matching PRLC / 37 client connects");
    assert_eq!(server.connected_clients(), vec![7]);
    for (protocol_id, client) in &stale {
        assert!(
            !client.client.is_connected(),
            "a peer packing {protocol_id:#018x} is refused at the netcode handshake"
        );
    }
}

// The same gate from the other side: this build's client never connects to a
// PRLB / 32 or PRLC / 33 to 36 host, while that build's own client does.
#[test]
fn netcode_handshake_never_connects_to_a_prlb_32_or_prlc_33_to_36_host() {
    for stale in [PRLB_32, PRLC_33, PRLC_34, PRLC_35, PRLC_36] {
        let mut host = RawServer::new(stale);
        let mut current = NetClient::new(
            loopback_socket(),
            host.addr,
            7,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("construct client");
        let mut same_build = RawClient::new(host.addr, 8, stale);

        let reached = poll_past_connect(|| {
            current.update(STEP).expect("client update");
            same_build.update();
            host.update();
            same_build.client.is_connected()
        });

        assert!(
            reached,
            "a {stale:#018x} client reaches its own build's host"
        );
        assert!(
            !current.is_connected(),
            "this build's client is refused by a {stale:#018x} host"
        );
        assert_eq!(host.server.clients_id(), vec![8]);
    }
}
