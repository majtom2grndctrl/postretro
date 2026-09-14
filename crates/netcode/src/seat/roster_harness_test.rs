// End-to-end session-roster coverage over the relay transport seam.
// See: context/lib/networking.md §Session-state ledger

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_foundation::Seat;
use postretro_net::slots::CloseCause;
use postretro_net::transport::{HandshakeOutcome, NetClient, NetServer};
use postretro_net::wire::{
    self, ConnectClaim, PlayerClaimId, RosterEntry, ServerControlMessage, SessionRosterMessage,
};

use postretro_entities::EntityRegistry;

use super::{HOLD_WINDOW, SeatTable, finish_host_poll};
use crate::netcode::endpoint::ClientSessionStatus;

const FRAME_DT: Duration = Duration::from_millis(16);
const MOD_ID: &str = "test.session-roster";
const MOD_VERSION: &str = "1.0.0";
const PARITY_DIGEST: [u8; 32] = [0x5a; 32];

fn claim(byte: u8, display_name: &str) -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([byte; 16]),
        display_name: display_name.to_owned(),
    }
}

fn server() -> (NetServer, SocketAddr) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind roster relay server");
    let address = socket.local_addr().expect("resolve roster relay address");
    let mut server =
        NetServer::new(socket, address, 8, Duration::from_secs(1), None).expect("create server");
    server.set_mod_identity(MOD_ID.to_owned(), MOD_VERSION.to_owned());
    server.set_mod_digest(Some(PARITY_DIGEST));
    server.set_level_parity(Some(("roster-test-level".to_owned(), PARITY_DIGEST)));
    (server, address)
}

fn client(server_address: SocketAddr, client_id: u64) -> NetClient {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind roster relay client");
    let mut client = NetClient::new(
        socket,
        server_address,
        client_id,
        Duration::from_secs(1),
        None,
        None,
    )
    .expect("create client");
    client.set_mod_identity(MOD_ID.to_owned(), MOD_VERSION.to_owned());
    client.set_mod_digest(Some(PARITY_DIGEST));
    client.set_level_parity(Some(("roster-test-level".to_owned(), PARITY_DIGEST)));
    client.set_connected();
    client
}

fn admit(
    server: &mut NetServer,
    seats: &mut SeatTable,
    client: &mut NetClient,
    client_id: u64,
) -> Seat {
    client.update_connections(FRAME_DT);
    for packet in client.packets_to_send() {
        server.process_packet_from(&packet, client_id);
    }
    server.update_connections(FRAME_DT);
    let poll = server.poll_handshakes();
    assert!(
        matches!(
            poll.handshakes.as_slice(),
            [HandshakeOutcome::Admitted { client_id: admitted }] if *admitted == client_id
        ),
        "the relay client clears immutable admission before it can receive a roster"
    );

    let seat = seats
        .admit_or_reclaim(
            client_id,
            server.connect_claim(client_id).cloned(),
            server.is_closed(client_id),
        )
        .expect("test session has a free durable seat")
        .seat;
    finish_host_poll(server, seats);
    seat
}

fn drain_rosters(
    server: &mut NetServer,
    client: &mut NetClient,
    client_id: u64,
) -> Vec<SessionRosterMessage> {
    server.update_connections(FRAME_DT);
    for packet in server.packets_to_send(client_id) {
        client.process_packet(&packet);
    }
    client.update_connections(FRAME_DT);
    client
        .drain_control()
        .into_iter()
        .filter_map(|message| match message {
            ServerControlMessage::SessionRoster(roster) => Some(roster),
            ServerControlMessage::Divergence(_)
            | ServerControlMessage::Relevel(_)
            | ServerControlMessage::Tuning(_)
            | ServerControlMessage::SwitchAccepted(_)
            | ServerControlMessage::SwitchRefused(_) => None,
        })
        .collect()
}

fn only_roster(
    server: &mut NetServer,
    client: &mut NetClient,
    client_id: u64,
) -> SessionRosterMessage {
    let mut rosters = drain_rosters(server, client, client_id);
    assert_eq!(
        rosters.len(),
        1,
        "each dirty roster revision publishes exactly one status frame to this recipient"
    );
    rosters.pop().expect("length checked")
}

fn status_entries(alpha: Seat, bravo: Seat, alpha_connected: bool) -> Vec<RosterEntry> {
    vec![
        RosterEntry {
            seat: 0,
            connected: true,
        },
        RosterEntry {
            seat: alpha.0,
            connected: alpha_connected,
        },
        RosterEntry {
            seat: bravo.0,
            connected: true,
        },
    ]
}

fn assert_claim_is_absent_from_roster(roster: &SessionRosterMessage, claim: &ConnectClaim) {
    let encoded = wire::encode(&ServerControlMessage::SessionRoster(roster.clone()));
    assert!(
        !encoded
            .windows(claim.player_id.0.len())
            .any(|bytes| bytes == claim.player_id.0.as_slice()),
        "the roster payload never carries the host-local player identity"
    );
    assert!(
        !encoded
            .windows(claim.display_name.len())
            .any(|bytes| bytes == claim.display_name.as_bytes()),
        "the roster payload never carries a player display name"
    );
}

#[test]
fn pending_peer_receives_no_roster_before_admission() {
    const PENDING_CLIENT: u64 = 41;
    let (mut server, address) = server();
    let mut client = client(address, PENDING_CLIENT);
    let mut seats = SeatTable::from_test_session_id([0x41; 16]);

    server.add_relay_connection(
        PENDING_CLIENT,
        Some(wire::encode_connect_claim(&claim(0x41, "Pending Runner"))),
    );
    finish_host_poll(&mut server, &mut seats);

    let mut client_status = ClientSessionStatus::default();
    let rosters = drain_rosters(&mut server, &mut client, PENDING_CLIENT);
    for roster in rosters.iter().cloned() {
        client_status.retain(roster);
    }
    assert!(
        rosters.is_empty(),
        "a peer below admission receives no session roster, including an empty seat count"
    );
    assert_eq!(
        client_status.open_seats(),
        None,
        "a pending peer cannot populate the client-owned open-seat presentation"
    );
}

#[test]
fn admitted_peer_retains_open_seat_count_for_presentation() {
    const ADMITTED_CLIENT: u64 = 42;
    let (mut server, address) = server();
    let mut client = client(address, ADMITTED_CLIENT);
    let mut seats = SeatTable::from_test_session_id([0x42; 16]);
    let mut client_status = ClientSessionStatus::default();

    server.add_relay_connection(
        ADMITTED_CLIENT,
        Some(wire::encode_connect_claim(&claim(0x42, "Admitted Runner"))),
    );
    admit(&mut server, &mut seats, &mut client, ADMITTED_CLIENT);
    let roster = only_roster(&mut server, &mut client, ADMITTED_CLIENT);
    let expected_open_seats = roster.open_seats;

    assert!(client_status.retain(roster));
    assert_eq!(
        client_status.open_seats(),
        Some(expected_open_seats),
        "an admitted peer retains the host's open-seat count for client presentation"
    );
}

#[test]
fn roster_keeps_session_and_status_through_level_rejoin_and_expiry() {
    let mut registry = EntityRegistry::new();
    const ALPHA: u64 = 51;
    const BRAVO: u64 = 52;
    const ALPHA_REJOIN: u64 = 53;
    let (mut server, address) = server();
    let mut seats = SeatTable::from_test_session_id([0x51; 16]);
    let alpha_claim = claim(0xa1, "Alpha Runner");
    let bravo_claim = claim(0xb2, "Bravo Runner");

    let mut alpha_client = client(address, ALPHA);
    server.add_relay_connection(ALPHA, Some(wire::encode_connect_claim(&alpha_claim)));
    let alpha_seat = admit(&mut server, &mut seats, &mut alpha_client, ALPHA);
    let initial_alpha = only_roster(&mut server, &mut alpha_client, ALPHA);

    let mut bravo_client = client(address, BRAVO);
    server.add_relay_connection(BRAVO, Some(wire::encode_connect_claim(&bravo_claim)));
    let bravo_seat = admit(&mut server, &mut seats, &mut bravo_client, BRAVO);
    let alpha_view = only_roster(&mut server, &mut alpha_client, ALPHA);
    let bravo_view = only_roster(&mut server, &mut bravo_client, BRAVO);
    let connected_entries = status_entries(alpha_seat, bravo_seat, true);

    assert_eq!(alpha_seat, Seat(1));
    assert_eq!(bravo_seat, Seat(2));
    assert_eq!(alpha_view.session_id, initial_alpha.session_id);
    assert_eq!(alpha_view.session_id, bravo_view.session_id);
    assert_eq!(alpha_view.entries, connected_entries);
    assert_eq!(bravo_view.entries, connected_entries);
    assert_eq!(alpha_view.open_seats, bravo_view.open_seats);
    assert_eq!(alpha_view.your_seat, Some(alpha_seat.0));
    assert_eq!(bravo_view.your_seat, Some(bravo_seat.0));
    assert_ne!(alpha_view.your_seat, bravo_view.your_seat);
    assert_claim_is_absent_from_roster(&alpha_view, &alpha_claim);
    assert_claim_is_absent_from_roster(&alpha_view, &bravo_claim);

    // The level boundary clears only pawn-level bindings. The durable seat/status
    // projection stays intact and a later publication keeps the same session id.
    seats.clear_pawn_bindings_for_level_unload(&mut registry);
    assert_eq!(seats.session_id(), initial_alpha.session_id);
    assert_eq!(seats.roster_entries(), connected_entries);

    assert!(
        server
            .close_relay_connection(ALPHA, CloseCause::Disconnect)
            .is_some(),
        "the live transport slot closes before its durable seat enters the hold"
    );
    assert_eq!(
        seats.hold_disconnected_client(&mut registry, ALPHA),
        Some(alpha_seat),
        "the level-independent seat remains rostered while its connection is held"
    );
    finish_host_poll(&mut server, &mut seats);
    let held_view = only_roster(&mut server, &mut bravo_client, BRAVO);
    assert_eq!(held_view.session_id, initial_alpha.session_id);
    assert_eq!(
        held_view.entries,
        status_entries(alpha_seat, bravo_seat, false),
        "a held seat stays visible as disconnected instead of losing its durable row"
    );

    let mut alpha_rejoined = client(address, ALPHA_REJOIN);
    server.add_relay_connection(ALPHA_REJOIN, Some(wire::encode_connect_claim(&alpha_claim)));
    assert_eq!(
        admit(&mut server, &mut seats, &mut alpha_rejoined, ALPHA_REJOIN,),
        alpha_seat,
        "a rejoin keeps the original durable seat after a level transition"
    );
    let bravo_rejoin_view = only_roster(&mut server, &mut bravo_client, BRAVO);
    let alpha_rejoin_view = only_roster(&mut server, &mut alpha_rejoined, ALPHA_REJOIN);
    assert_eq!(alpha_rejoin_view.session_id, initial_alpha.session_id);
    assert_eq!(bravo_rejoin_view.session_id, initial_alpha.session_id);
    assert_eq!(alpha_rejoin_view.entries, connected_entries);
    assert_eq!(bravo_rejoin_view.entries, connected_entries);
    assert_eq!(alpha_rejoin_view.open_seats, bravo_rejoin_view.open_seats);
    assert_eq!(alpha_rejoin_view.your_seat, Some(alpha_seat.0));
    assert_eq!(bravo_rejoin_view.your_seat, Some(bravo_seat.0));
    assert_claim_is_absent_from_roster(&alpha_rejoin_view, &alpha_claim);
    assert_claim_is_absent_from_roster(&alpha_rejoin_view, &bravo_claim);

    assert!(
        server
            .close_relay_connection(ALPHA_REJOIN, CloseCause::Disconnect)
            .is_some(),
        "the rebound connection can enter a new hold"
    );
    assert_eq!(
        seats.hold_disconnected_client(&mut registry, ALPHA_REJOIN),
        Some(alpha_seat)
    );
    seats.advance_hold_clock(HOLD_WINDOW);
    finish_host_poll(&mut server, &mut seats);
    let expired_view = only_roster(&mut server, &mut bravo_client, BRAVO);
    assert_eq!(expired_view.session_id, initial_alpha.session_id);
    assert_eq!(
        expired_view.entries,
        vec![
            RosterEntry {
                seat: 0,
                connected: true,
            },
            RosterEntry {
                seat: bravo_seat.0,
                connected: true,
            },
        ],
        "hold expiry removes the stale seat from the published status roster"
    );
    assert_eq!(
        expired_view.open_seats,
        bravo_rejoin_view.open_seats + 1,
        "expiring a hold frees its reserved join capacity without reusing its seat number"
    );
}

#[test]
fn expired_seat_cycles_bound_host_local_seat_rows() {
    const CYCLES: u64 = 32;
    let mut registry = EntityRegistry::new();
    let mut seats = SeatTable::from_test_session_id([0x60; 16]);

    for cycle in 0..CYCLES {
        let client_id = 100 + cycle;
        let seat = seats
            .admit_or_reclaim(client_id, Some(claim(cycle as u8, "Cycle Runner")), false)
            .expect("each cycle mints one fresh seat")
            .seat;
        assert_eq!(seat, Seat((cycle + 1) as u16));
        assert_eq!(
            seats.hold_disconnected_client(&mut registry, client_id),
            Some(seat)
        );
        seats.advance_hold_clock(HOLD_WINDOW);
        seats.release_expired_holds();

        assert_eq!(seats.carried.len(), 1, "only the local seat remains stored");
        assert!(seats.client_bindings.is_empty());
        assert!(seats.pawn_bindings.is_empty());
        assert!(
            seats.connect_claims.is_empty(),
            "expired claims cannot accumulate"
        );
        assert!(seats.hold_deadlines.is_empty());
        assert!(seats.hold_order.is_empty());
        assert_eq!(
            seats.roster_entries(),
            vec![RosterEntry {
                seat: 0,
                connected: true,
            }],
            "released seats leave no stale roster rows"
        );
    }

    assert_eq!(
        seats.next_seat,
        CYCLES as u32 + 1,
        "only the monotonic scalar advances; released seat records are not retained"
    );
}
