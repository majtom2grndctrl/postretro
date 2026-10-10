// Player-addressed presentation over a real relay: one host, two clients.
// See: context/lib/networking.md §Presentation events vs. replicated state

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_entities::SystemCommandFireContext;
use postretro_entities::SystemReactionCommand;
use postretro_entities::reactions::system_commands::SystemCommandQueue;
use postretro_foundation::Seat;
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::PresentationCommand;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::seat::SeatTable;

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;
const STEP: Duration = Duration::from_millis(16);
const FINGERPRINT: [u8; 32] = [0x5a; 32];

fn flash() -> SystemReactionCommand {
    SystemReactionCommand::FlashScreen {
        color: [1.0, 0.9, 0.3, 0.4],
        duration_ms: 300.0,
    }
}

fn sound(name: &str) -> SystemReactionCommand {
    SystemReactionCommand::PlaySound {
        sound: name.to_string(),
        bus: None,
        at: None,
    }
}

struct Session {
    server: NetServer,
    clients: Vec<(u64, NetClient)>,
    seats: SeatTable,
}

impl Session {
    fn new() -> Self {
        let origin = Duration::from_secs(1);
        let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("server socket");
        let server_addr: SocketAddr = server_socket.local_addr().expect("server address");
        let mut server =
            NetServer::new(server_socket, server_addr, 2, origin, Some(FINGERPRINT)).expect("server");
        server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        server.set_mod_digest(Some(FINGERPRINT));
        server.set_level_parity(Some(("test-level".to_string(), FINGERPRINT)));
        server.set_revealed_level(Some("test-level".to_string()));
        let mut seats = SeatTable::new().expect("seat ledger");
        let mut clients = Vec::new();
        for client_id in [CLIENT_A, CLIENT_B] {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("client socket");
            let mut client =
                NetClient::new(socket, server_addr, client_id, origin, Some(FINGERPRINT), None)
                    .expect("client");
            client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
            client.set_mod_digest(Some(FINGERPRINT));
            client.set_level_parity(Some(("test-level".to_string(), FINGERPRINT)));
            client.set_revealed_level(Some("test-level".to_string()));
            server.add_relay_connection(client_id, None);
            client.set_connected();
            seats.admit_or_reclaim(client_id, None, false).expect("seat admitted");
            clients.push((client_id, client));
        }
        let mut session = Self {
            server,
            clients,
            seats,
        };
        for _ in 0..128 {
            session.relay_to_server();
            if [CLIENT_A, CLIENT_B]
                .iter()
                .all(|client| session.server.is_participating(*client))
            {
                session.relay_to_clients();
                return session;
            }
        }
        panic!("relay did not complete the handshake");
    }

    fn seat(&self, client: u64) -> Seat {
        self.seats.seat_for_client(client).expect("client has a seat")
    }

    fn relay_to_server(&mut self) {
        for (client_id, client) in &mut self.clients {
            client.update_connections(STEP);
            for packet in client.packets_to_send() {
                self.server.process_packet_from(&packet, *client_id);
            }
        }
        self.server.update_connections(STEP);
        let _ = self.server.poll_handshakes();
    }

    fn relay_to_clients(&mut self) {
        self.server.update_connections(STEP);
        for (client_id, client) in &mut self.clients {
            for packet in self.server.packets_to_send(*client_id) {
                client.process_packet(&packet);
            }
            client.update_connections(STEP);
        }
    }

    /// Each client's frame: drain the presentation lane and intake its
    /// commands into that client's own system command queue.
    fn client_frames(&mut self) -> Vec<(u64, Vec<SystemReactionCommand>)> {
        self.clients
            .iter_mut()
            .map(|(client_id, client)| {
                let _ = client.drain_control();
                let queue = SystemCommandQueue::new();
                let (commands, _passive) = split_presentation_commands(client.drain_presentation());
                ingest_presentation_commands(commands, &queue);
                (*client_id, queue.take())
            })
            .collect()
    }
}

/// Push `command` as a player event fired for `seat` would.
fn fire_for(queue: &SystemCommandQueue, seat: Option<Seat>, command: SystemReactionCommand) {
    let previous = queue.replace_fire_context(SystemCommandFireContext {
        source: "playerEvent".to_string(),
        presentation_seat: seat,
        ..SystemCommandFireContext::default()
    });
    queue.push(command);
    queue.replace_fire_context(previous);
}

#[test]
fn a_player_events_flash_reaches_only_its_players_machine() {
    let mut session = Session::new();
    let host_queue = SystemCommandQueue::new();
    fire_for(&host_queue, Some(session.seat(CLIENT_A)), flash());
    // The host's own player sits on a seat no client is bound to.
    fire_for(&host_queue, Some(Seat(0)), sound("fanfare"));

    route_player_presentation(&host_queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(host_queue.take(), vec![sound("fanfare")], "the host's own player presents on the host only");
    session.relay_to_clients();
    let frames = session.client_frames();
    assert_eq!(frames, vec![(CLIENT_A, vec![flash()]), (CLIENT_B, Vec::new())]);
}

#[test]
fn single_player_presents_routed_commands_locally() {
    let queue = SystemCommandQueue::new();
    fire_for(&queue, Some(Seat(0)), flash());
    fire_for(&queue, None, sound("chime"));
    route_player_presentation(&queue, None, None);
    // An unseated local pawn's fire presents as it drains; a routed one joins
    // the same local drain once routing finds no client for its seat.
    assert_eq!(queue.take(), vec![sound("chime"), flash()]);
}

#[test]
fn a_dropped_command_presents_nothing_and_the_next_one_still_arrives() {
    let mut session = Session::new();
    let host_queue = SystemCommandQueue::new();
    fire_for(&host_queue, Some(session.seat(CLIENT_A)), sound("lost"));
    route_player_presentation(&host_queue, Some(&mut session.server), Some(&session.seats));
    // The unreliable lane drops it: nothing reaches the client, nothing resends.
    session.server.update_connections(STEP);
    let _ = session.server.packets_to_send(CLIENT_A);
    assert!(session.client_frames().iter().all(|(_, commands)| commands.is_empty()));

    fire_for(&host_queue, Some(session.seat(CLIENT_A)), sound("kept"));
    route_player_presentation(&host_queue, Some(&mut session.server), Some(&session.seats));
    session.relay_to_clients();
    assert_eq!(session.client_frames()[0], (CLIENT_A, vec![sound("kept")]));
}

#[test]
fn a_non_finite_forwarded_command_is_dropped_at_intake_and_finite_siblings_present() {
    let capture = LogCapture::start();
    let queue = SystemCommandQueue::new();
    ingest_presentation_commands(
        vec![
            PresentationCommand::FlashScreen {
                color: [1.0, f32::NAN, 0.0, 1.0],
                duration_ms: 300.0,
            },
            PresentationCommand::ScreenShake {
                amplitude: 0.4,
                duration_ms: f32::INFINITY,
                frequency: None,
            },
            PresentationCommand::PlaySound {
                sound: "chime".to_string(),
                bus: None,
            },
        ],
        &queue,
    );
    assert_eq!(queue.take(), vec![sound("chime")]);
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.level == log::Level::Warn
                && record.message.contains("non-finite"))
            .count(),
        2
    );
}

#[test]
fn a_demoted_client_and_a_level_transition_present_nothing() {
    let mut session = Session::new();
    let host_queue = SystemCommandQueue::new();
    session.server.set_level_parity(None);
    let _ = session.server.poll_handshakes();
    fire_for(&host_queue, Some(session.seat(CLIENT_A)), flash());
    route_player_presentation(&host_queue, Some(&mut session.server), Some(&session.seats));
    session.relay_to_clients();
    assert!(
        session.client_frames().iter().all(|(_, commands)| commands.is_empty()),
        "a held client receives nothing"
    );
    assert!(host_queue.take().is_empty(), "nor does it fall back to the host's screen");

    // A fire on the last tick before a transition: the transition discards
    // the routed command before any later drain.
    fire_for(&host_queue, Some(session.seat(CLIENT_A)), flash());
    host_queue.discard_all();
    assert!(host_queue.take_routed().is_empty());
}

#[test]
fn a_forwarded_command_is_the_same_local_command_a_local_reaction_enqueues() {
    let local = [
        flash(),
        SystemReactionCommand::Vignette {
            color: Some([0.8, 0.0, 0.0]),
            strength: 0.6,
            duration_ms: 800.0,
        },
        SystemReactionCommand::Vignette {
            color: None,
            strength: 0.3,
            duration_ms: 200.0,
        },
        SystemReactionCommand::ScreenShake {
            amplitude: 0.4,
            duration_ms: 250.0,
            frequency: Some(18.0),
        },
        SystemReactionCommand::Rumble {
            strong: 0.75,
            weak: Some(0.25),
            duration_ms: 120.0,
        },
        SystemReactionCommand::PlaySound {
            sound: "level_up".to_string(),
            bus: Some("ui".to_string()),
            at: None,
        },
    ];
    for command in local {
        let wire = presentation_command_to_wire(&command).expect("presentation converts");
        assert_eq!(
            presentation_command_from_wire(wire),
            Some(command.clone()),
            "the client's command equals the local one, so its accommodations apply alike"
        );
    }
    assert!(presentation_command_to_wire(&SystemReactionCommand::PopTree).is_none());
}

#[test]
fn trigger_presentation_stays_local_while_a_player_events_routes() {
    let queue = SystemCommandQueue::new();
    // A trigger's residual drains with no presentation seat.
    queue.push(flash());
    fire_for(&queue, Some(Seat(4)), flash());
    assert_eq!(queue.take(), vec![flash()]);
    assert_eq!(queue.take_routed(), vec![(Seat(4), flash())]);
}
