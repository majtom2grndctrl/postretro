// Player-addressed presentation over a real relay: one host, two clients.
// See: context/lib/networking.md §Presentation events vs. replicated state

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_entities::SystemReactionCommand;
use postretro_entities::reactions::system_commands::SystemCommandQueue;
use postretro_entities::{Audience, EntityId, FireOrigin, FireSubject, SystemCommandFireContext};
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
        audience: None,
        color: [1.0, 0.9, 0.3, 0.4],
        duration_ms: 300.0,
    }
}

fn sound(name: &str) -> SystemReactionCommand {
    SystemReactionCommand::PlaySound {
        audience: None,
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
        let mut server = NetServer::new(server_socket, server_addr, 2, origin, Some(FINGERPRINT))
            .expect("server");
        server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        server.set_mod_digest(Some(FINGERPRINT));
        server.set_level_parity(Some(("test-level".to_string(), FINGERPRINT)));
        server.set_revealed_level(Some("test-level".to_string()));
        let mut seats = SeatTable::new().expect("seat ledger");
        let mut clients = Vec::new();
        for client_id in [CLIENT_A, CLIENT_B] {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("client socket");
            let mut client = NetClient::new(
                socket,
                server_addr,
                client_id,
                origin,
                Some(FINGERPRINT),
                None,
            )
            .expect("client");
            client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
            client.set_mod_digest(Some(FINGERPRINT));
            client.set_level_parity(Some(("test-level".to_string(), FINGERPRINT)));
            client.set_revealed_level(Some("test-level".to_string()));
            server.add_relay_connection(client_id, None);
            client.set_connected();
            seats
                .admit_or_reclaim(client_id, None, false)
                .expect("seat admitted");
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
        self.seats
            .seat_for_client(client)
            .expect("client has a seat")
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
        origin: FireOrigin::HostOnly(FireSubject::PlayerEvent { player: seat }),
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
    assert_eq!(
        host_queue.take(),
        vec![sound("fanfare")],
        "the host's own player presents on the host only"
    );
    session.relay_to_clients();
    let frames = session.client_frames();
    assert_eq!(
        frames,
        vec![(CLIENT_A, vec![flash()]), (CLIENT_B, Vec::new())]
    );
}

#[test]
fn a_remote_seat_with_no_client_bound_presents_nothing() {
    let mut session = Session::new();
    let host_queue = SystemCommandQueue::new();
    // A seat in range of the table but bound to no client.
    fire_for(&host_queue, Some(Seat(9)), flash());
    route_player_presentation(&host_queue, Some(&mut session.server), Some(&session.seats));
    assert!(
        host_queue.take().is_empty(),
        "it does not fall back to the host's screen"
    );
    assert!(host_queue.take_routed().is_empty());
    session.relay_to_clients();
    assert!(
        session
            .client_frames()
            .iter()
            .all(|(_, commands)| commands.is_empty()),
        "nothing is sent to any client"
    );
}

#[test]
fn single_player_presents_routed_commands_locally() {
    let queue = SystemCommandQueue::new();
    fire_for(&queue, Some(Seat(0)), flash());
    fire_for(&queue, None, sound("chime"));
    route_player_presentation(&queue, None, None);
    assert_eq!(queue.take(), vec![flash(), sound("chime")]);
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
    assert!(
        session
            .client_frames()
            .iter()
            .all(|(_, commands)| commands.is_empty())
    );

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
                at: None,
            },
        ],
        &queue,
    );
    assert_eq!(queue.take(), vec![sound("chime")]);
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(
                |record| record.level == log::Level::Warn && record.message.contains("non-finite")
            )
            .count(),
        2
    );
}

#[test]
fn a_forwarded_sound_position_becomes_a_world_contact_anchor() {
    let queue = SystemCommandQueue::new();
    ingest_presentation_commands(
        vec![PresentationCommand::PlaySound {
            sound: "door.open".to_string(),
            bus: Some("sfx".to_string()),
            at: Some([12.0, -3.0, 7.0]),
        }],
        &queue,
    );
    let commands = queue.take();
    let [
        SystemReactionCommand::PlaySound {
            audience: None,
            sound,
            bus,
            at: Some(Emitter::Contacts(contacts)),
        },
    ] = commands.as_slice()
    else {
        panic!("the forwarded position must reach the local sound anchor");
    };
    assert_eq!(sound, "door.open");
    assert_eq!(bus.as_deref(), Some("sfx"));
    let [contact] = contacts.as_slice() else {
        panic!("one wire point supplies exactly one contact");
    };
    assert!((contact.point - Vec3::new(12.0, -3.0, 7.0)).length() < 1e-6);
    assert_eq!(contact.hit, ContactHit::World);
}

#[test]
fn a_non_finite_forwarded_sound_position_presents_nothing() {
    let capture = LogCapture::start();
    let queue = SystemCommandQueue::new();
    for axis in 0..3 {
        for non_finite in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut point = [0.0; 3];
            point[axis] = non_finite;
            ingest_presentation_commands(
                vec![PresentationCommand::PlaySound {
                    sound: "door.open".to_string(),
                    bus: None,
                    at: Some(point),
                }],
                &queue,
            );
        }
    }
    assert!(queue.take().is_empty());
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| {
                record.level == log::Level::Warn && record.message.contains("non-finite")
            })
            .count(),
        9
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
        session
            .client_frames()
            .iter()
            .all(|(_, commands)| commands.is_empty()),
        "a held client receives nothing"
    );
    assert!(
        host_queue.take().is_empty(),
        "nor does it fall back to the host's screen"
    );

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
            audience: None,
            color: Some([0.8, 0.0, 0.0]),
            strength: 0.6,
            duration_ms: 800.0,
        },
        SystemReactionCommand::Vignette {
            audience: None,
            color: None,
            strength: 0.3,
            duration_ms: 200.0,
        },
        SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 0.4,
            duration_ms: 250.0,
            frequency: Some(18.0),
        },
        SystemReactionCommand::Rumble {
            audience: None,
            strong: 0.75,
            weak: Some(0.25),
            duration_ms: 120.0,
        },
        SystemReactionCommand::PlaySound {
            audience: None,
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
fn every_machine_presentation_stays_local_while_a_player_event_routes() {
    let queue = SystemCommandQueue::new();
    // Every-machine drains keep their own presentation local.
    queue.push(flash());
    fire_for(&queue, Some(Seat(4)), flash());
    assert_eq!(queue.take(), vec![flash()]);
    assert_eq!(
        queue.take_routed(),
        vec![(Recipients::Seats(vec![Seat(4)]), flash())]
    );
}

fn with_audience(
    mut command: SystemReactionCommand,
    value: Option<Audience>,
) -> SystemReactionCommand {
    match &mut command {
        SystemReactionCommand::PlaySound { audience, .. }
        | SystemReactionCommand::Rumble { audience, .. }
        | SystemReactionCommand::FlashScreen { audience, .. }
        | SystemReactionCommand::Vignette { audience, .. }
        | SystemReactionCommand::ScreenShake { audience, .. } => *audience = value,
        _ => panic!("presentation fixture"),
    }
    command
}

fn five_commands() -> Vec<SystemReactionCommand> {
    vec![
        sound("alarm"),
        flash(),
        SystemReactionCommand::Rumble {
            audience: None,
            strong: 0.5,
            weak: None,
            duration_ms: 120.0,
        },
        SystemReactionCommand::ScreenShake {
            audience: None,
            amplitude: 4.0,
            duration_ms: 300.0,
            frequency: None,
        },
        SystemReactionCommand::Vignette {
            audience: None,
            color: None,
            strength: 0.5,
            duration_ms: 200.0,
        },
    ]
}

fn fire_origin(queue: &SystemCommandQueue, origin: FireOrigin, command: SystemReactionCommand) {
    let previous = queue.replace_fire_context(SystemCommandFireContext {
        source: "routing-fixture".to_string(),
        origin,
        ..Default::default()
    });
    queue.push(command);
    queue.replace_fire_context(previous);
}

fn trigger_origin(player: Option<Seat>, occupants: Vec<Seat>) -> FireOrigin {
    FireOrigin::HostOnly(FireSubject::Trigger {
        player,
        trigger: EntityId::from_raw(7),
        occupants,
    })
}

#[test]
fn every_legal_audience_row_routes_all_five_kinds_over_the_real_relay() {
    let mut session = Session::new();
    let a = session.seat(CLIENT_A);
    let b = session.seat(CLIENT_B);
    let trigger = trigger_origin(Some(a), vec![Seat(0), b]);
    let player = FireOrigin::HostOnly(FireSubject::PlayerEvent { player: Some(a) });
    let no_subject = FireOrigin::HostOnly(FireSubject::None);
    // Expected machines (host, A, B), separately for sounds and the four effects.
    let local = [true, false, false];
    let everyone = [true, true, true];
    let only_a = [false, true, false];
    let host_b = [true, false, true];
    let nowhere = [false; 3];
    let cases = [
        (
            "every-machine default",
            FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
            None,
            local,
            local,
        ),
        (
            "every-machine everyone",
            FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
            Some(Audience::Everyone),
            local,
            local,
        ),
        ("trigger default", trigger.clone(), None, everyone, only_a),
        (
            "trigger activator",
            trigger.clone(),
            Some(Audience::Activators),
            only_a,
            only_a,
        ),
        (
            "trigger occupants",
            trigger.clone(),
            Some(Audience::Trigger),
            host_b,
            host_b,
        ),
        (
            "trigger everyone",
            trigger,
            Some(Audience::Everyone),
            everyone,
            everyone,
        ),
        ("player default", player.clone(), None, only_a, only_a),
        (
            "player token",
            player.clone(),
            Some(Audience::Player),
            only_a,
            only_a,
        ),
        (
            "player everyone",
            player,
            Some(Audience::Everyone),
            everyone,
            everyone,
        ),
        (
            "no subject default",
            no_subject.clone(),
            None,
            everyone,
            local,
        ),
        (
            "no subject everyone",
            no_subject,
            Some(Audience::Everyone),
            everyone,
            everyone,
        ),
        (
            "unseated player default",
            FireOrigin::HostOnly(FireSubject::PlayerEvent { player: None }),
            None,
            local,
            local,
        ),
        (
            "unseated player token",
            FireOrigin::HostOnly(FireSubject::PlayerEvent { player: None }),
            Some(Audience::Player),
            local,
            local,
        ),
        (
            "unseated trigger default",
            trigger_origin(None, vec![]),
            None,
            everyone,
            nowhere,
        ),
        (
            "unseated activator",
            trigger_origin(None, vec![]),
            Some(Audience::Activators),
            nowhere,
            nowhere,
        ),
    ];
    for (name, origin, audience, sounds, effects) in cases {
        let queue = SystemCommandQueue::new();
        let commands = five_commands();
        for command in &commands {
            fire_origin(
                &queue,
                origin.clone(),
                with_audience(command.clone(), audience),
            );
        }
        route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
        let host = queue.take();
        assert!(queue.is_empty(), "{name}: no recursively routed work");
        session.relay_to_clients();
        let frames = session.client_frames();
        for (machine, actual) in [host, frames[0].1.clone(), frames[1].1.clone()]
            .into_iter()
            .enumerate()
        {
            let expected: Vec<_> = commands
                .iter()
                .enumerate()
                .filter_map(|(kind, command)| {
                    (if kind == 0 {
                        sounds[machine]
                    } else {
                        effects[machine]
                    })
                    .then_some(command.clone())
                })
                .collect();
            assert_eq!(actual, expected, "{name}: machine {machine}");
        }
    }
}

#[test]
fn routed_and_local_commands_keep_push_order_and_restore_ambient_fire_context() {
    let mut session = Session::new();
    let queue = SystemCommandQueue::new();
    fire_origin(
        &queue,
        FireOrigin::HostOnly(FireSubject::None),
        sound("first"),
    );
    queue.push(SystemReactionCommand::PushTree {
        tree: "menu".into(),
        on_commit: None,
    });
    queue.push(sound("third"));
    fire_origin(
        &queue,
        trigger_origin(Some(Seat(0)), vec![]),
        with_audience(sound("fourth"), Some(Audience::Activators)),
    );
    fire_for(&queue, Some(session.seat(CLIENT_A)), sound("remote-only"));
    queue.push(sound("last"));
    let ambient = SystemCommandFireContext {
        source: "still-firing".into(),
        origin: trigger_origin(Some(Seat(0)), vec![]),
        ..Default::default()
    };
    queue.replace_fire_context(ambient.clone());
    route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(queue.fire_context(), ambient);
    assert_eq!(
        queue.take(),
        vec![
            sound("first"),
            SystemReactionCommand::PushTree {
                tree: "menu".into(),
                on_commit: None
            },
            sound("third"),
            sound("fourth"),
            sound("last")
        ]
    );
    assert!(queue.is_empty());
    session.relay_to_clients();
    assert_eq!(
        session.client_frames(),
        vec![
            (CLIENT_A, vec![sound("first"), sound("remote-only")]),
            (CLIENT_B, vec![sound("first")])
        ]
    );
}

#[test]
fn duplicate_occupant_seats_present_once_and_unbound_seats_never_fall_back() {
    let mut session = Session::new();
    let queue = SystemCommandQueue::new();
    let a = session.seat(CLIENT_A);
    fire_origin(
        &queue,
        trigger_origin(None, vec![Seat(0), a, a, Seat(9), Seat(0)]),
        with_audience(flash(), Some(Audience::Trigger)),
    );
    route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(queue.take(), vec![flash()]);
    session.relay_to_clients();
    assert_eq!(
        session.client_frames(),
        vec![(CLIENT_A, vec![flash()]), (CLIENT_B, vec![])]
    );
}

#[test]
fn everyone_and_explicit_seats_skip_a_demoted_client_while_other_clients_present() {
    let mut session = Session::new();
    session.clients[1].1.set_level_parity(None);
    session.relay_to_server();
    assert!(session.server.is_participating(CLIENT_A));
    assert!(!session.server.is_participating(CLIENT_B));
    let queue = SystemCommandQueue::new();
    fire_origin(
        &queue,
        FireOrigin::HostOnly(FireSubject::None),
        sound("everyone"),
    );
    fire_origin(
        &queue,
        trigger_origin(None, vec![session.seat(CLIENT_A), session.seat(CLIENT_B)]),
        with_audience(flash(), Some(Audience::Trigger)),
    );
    route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(queue.take(), vec![sound("everyone")]);
    session.relay_to_clients();
    assert_eq!(
        session.client_frames(),
        vec![
            (CLIENT_A, vec![sound("everyone"), flash()]),
            (CLIENT_B, vec![])
        ]
    );
}

#[test]
fn standalone_routed_empty_seats_and_everyone_present_once_in_push_order() {
    let queue = SystemCommandQueue::new();
    fire_origin(
        &queue,
        trigger_origin(None, vec![]),
        with_audience(sound("empty"), Some(Audience::Trigger)),
    );
    fire_origin(
        &queue,
        FireOrigin::HostOnly(FireSubject::None),
        sound("all"),
    );
    queue.push(flash());
    route_player_presentation(&queue, None, None);
    assert_eq!(queue.take(), vec![sound("empty"), sound("all"), flash()]);
    assert!(queue.is_empty());
}

#[test]
fn illegal_audience_tokens_skip_with_one_warning_per_source_and_primitive() {
    let capture = LogCapture::start();
    let queue = SystemCommandQueue::new();
    for _ in 0..2 {
        for command in five_commands() {
            fire_origin(
                &queue,
                FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
                with_audience(command, Some(Audience::Activators)),
            );
        }
    }
    route_player_presentation(&queue, None, None);
    assert!(queue.is_empty());
    let warnings: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("does not publish audience")
        })
        .collect();
    assert_eq!(warnings.len(), 5);
    for primitive in [
        "playSound",
        "rumble",
        "flashScreen",
        "screenShake",
        "vignette",
    ] {
        assert_eq!(
            warnings
                .iter()
                .filter(|record| record.message.contains(primitive)
                    && record.message.contains("routing-fixture"))
                .count(),
            1
        );
    }
}

#[test]
fn injected_every_machine_ai_activate_and_observer_origins_present_once_without_broadcast() {
    let mut session = Session::new();
    let queue = SystemCommandQueue::new();
    fire_origin(
        &queue,
        FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
        sound("activate"),
    );
    route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(queue.take(), vec![sound("activate")]);
    session.relay_to_clients();
    assert!(
        session
            .client_frames()
            .iter()
            .all(|(_, commands)| commands.is_empty())
    );
    // Observer cue dispatch runs locally on each client under the same origin.
    for _ in [CLIENT_A, CLIENT_B] {
        let observer_queue = SystemCommandQueue::new();
        fire_origin(
            &observer_queue,
            FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
            sound("activate"),
        );
        route_player_presentation(&observer_queue, None, None);
        assert_eq!(observer_queue.take(), vec![sound("activate")]);
    }
}

#[test]
fn positioned_entity_sound_keeps_fire_time_origin_through_wire_and_real_relay() {
    let mut session = Session::new();
    let queue = SystemCommandQueue::new();
    let original = SystemReactionCommand::PlaySound {
        audience: None,
        sound: "door.open".into(),
        bus: Some("sfx".into()),
        at: Some(Emitter::Entity {
            id: EntityId::from_raw(7),
            origin: Vec3::new(12.0, -3.0, 7.0),
        }),
    };
    let wire = presentation_command_to_wire(&original).unwrap();
    assert_eq!(
        wire,
        PresentationCommand::PlaySound {
            sound: "door.open".into(),
            bus: Some("sfx".into()),
            at: Some([12.0, -3.0, 7.0])
        }
    );
    let expected_remote = presentation_command_from_wire(wire).unwrap();
    fire_origin(
        &queue,
        FireOrigin::HostOnly(FireSubject::None),
        original.clone(),
    );
    route_player_presentation(&queue, Some(&mut session.server), Some(&session.seats));
    assert_eq!(queue.take(), vec![original]);
    session.relay_to_clients();
    assert_eq!(
        session.client_frames(),
        vec![
            (CLIENT_A, vec![expected_remote.clone()]),
            (CLIENT_B, vec![expected_remote.clone()])
        ]
    );
    assert!(
        presentation_command_to_wire(&expected_remote).is_none(),
        "received weapon/contact anchors are not a forwarding source"
    );
}
