// The session-password admission gate over relay connections: challenge,
// proof, the held declarations, the proof bound and every refusal.
// See: context/lib/networking.md §Admission and content parity

use super::*;

const MOD_ID: &str = "postretro.test";
const A: ClientId = 51;
const B: ClientId = 52;

/// A host on `MOD_ID` with installed content and the given password.
fn host(password: &str) -> NetServer {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = socket.local_addr().unwrap();
    let mut server = NetServer::new(socket, addr, 8, Duration::from_secs(1), None).unwrap();
    server.set_mod_identity(MOD_ID.into(), "1".into());
    server.set_mod_digest(Some([7; 32]));
    server.set_level_parity(Some(("test-level".into(), [9; 32])));
    server.set_revealed_level(Some("test-level".into()));
    server.set_session_password(password);
    server
}

/// A relay client of `server` on `mod_id`, answering with `password`, whose
/// parity matches and which has revealed the host's level.
fn joiner(server: &mut NetServer, id: ClientId, mod_id: &str, password: Option<&str>) -> NetClient {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut client = NetClient::new(
        socket,
        server.local_addr()[0],
        id,
        Duration::from_secs(1),
        None,
        None,
    )
    .unwrap();
    client.set_mod_identity(mod_id.into(), "1".into());
    client.set_mod_digest(Some([7; 32]));
    client.set_level_parity(Some(("test-level".into(), [9; 32])));
    client.set_join_seed(BTreeMap::from([(
        "k-xp".to_owned(),
        JoinSeedValue::Number(7.0),
    )]));
    client.set_revealed_level(Some(RevealedLevel::new("test-level", None)));
    if let Some(password) = password {
        client.set_session_password(password);
    }
    server.add_relay_connection(id, None);
    client.set_connected();
    client
}

fn send(client: &mut NetClient, id: ClientId, server: &mut NetServer) {
    client.update_connections(Duration::from_millis(16));
    for packet in client.packets_to_send() {
        server.process_packet_from(&packet, id);
    }
}

/// Deliver the host's packets, then drain the client's Control as its poll
/// does: a challenge is answered there and never surfaces.
fn receive(
    server: &mut NetServer,
    id: ClientId,
    client: &mut NetClient,
) -> Vec<ServerControlMessage> {
    for packet in server.packets_to_send(id) {
        client.process_packet(&packet);
    }
    client.drain_control()
}

fn admitted(id: ClientId) -> HandshakeOutcome {
    HandshakeOutcome::Admitted { client_id: id }
}

fn refused_password(id: ClientId) -> HandshakeOutcome {
    HandshakeOutcome::Rejected {
        client_id: id,
        cause: ClosingCause::Password,
    }
}

fn closing(cause: ClosingCause) -> ServerControlMessage {
    ServerControlMessage::Divergence(DivergenceReason::Closing(cause))
}

/// One full challenge round for one joiner: its declarations, the
/// challenge, its proof. Returns the poll that judged the proof.
fn challenge_round(server: &mut NetServer, id: ClientId, client: &mut NetClient) -> ServerPoll {
    send(client, id, server);
    let first = server.poll_handshakes();
    assert!(
        first.handshakes.is_empty(),
        "nothing is decided before the proof"
    );
    assert!(
        first.join_seeds.is_empty(),
        "the join seed waits for admission"
    );
    let _ = receive(server, id, client);
    send(client, id, server);
    server.poll_handshakes()
}

// AC P-a, P-b (P6): the right password admits, and the parity, join seed
// and reveal sent before the proof apply then, so the slot participates.
#[test]
fn right_password_admits_and_applies_what_was_declared_before_the_proof() {
    let mut server = host("p");
    let mut client = joiner(&mut server, A, MOD_ID, Some("p"));

    send(&mut client, A, &mut server);
    let first = server.poll_handshakes();
    assert!(first.handshakes.is_empty());
    assert_eq!(server.slot_state(A), Some(SlotState::Pending));
    assert!(!server.parity_declarations.contains_key(&A), "parity waits");
    assert!(
        !server.revealed_declarations.contains_key(&A),
        "the reveal waits"
    );

    let _ = receive(&mut server, A, &mut client);
    send(&mut client, A, &mut server);
    let poll = server.poll_handshakes();
    assert_eq!(poll.handshakes, vec![admitted(A)]);
    assert_eq!(
        poll.join_seeds,
        vec![(
            A,
            BTreeMap::from([("k-xp".to_owned(), JoinSeedValue::Number(7.0))])
        )]
    );
    assert_eq!(
        poll.lifecycle,
        vec![SlotEvent::Participating { client_id: A }]
    );
    assert!(server.is_participating(A));
}

// AC P-f, P-l: a wrong password, and the empty proof of a client with no
// password, are refused with `Password` in the challenge's own round, and the
// refused slot applies nothing it declared.
#[test]
fn wrong_or_missing_password_is_refused_and_applies_nothing() {
    for (case, password) in [("wrong", Some("q")), ("empty", None)] {
        let mut server = host("p");
        let mut client = joiner(&mut server, A, MOD_ID, password);
        let poll = challenge_round(&mut server, A, &mut client);
        assert_eq!(poll.handshakes, vec![refused_password(A)], "{case}");
        assert!(poll.join_seeds.is_empty(), "{case}: no join seed applies");
        assert!(
            !server.parity_declarations.contains_key(&A),
            "{case}: no parity"
        );
        assert!(
            !server.revealed_declarations.contains_key(&A),
            "{case}: no reveal"
        );
        assert_eq!(
            receive(&mut server, A, &mut client),
            vec![closing(ClosingCause::Password)],
            "{case}: the client learns why"
        );
    }
}

// AC P-c (P7): two pending clients get different nonces; a proof replayed
// from one onto the other is refused; a proof with no challenge outstanding
// changes no slot state.
#[test]
fn nonces_differ_per_connection_replays_refuse_and_stray_proofs_change_nothing() {
    let mut server = host("p");
    let mut a = joiner(&mut server, A, MOD_ID, Some("p"));
    let mut b = joiner(&mut server, B, MOD_ID, Some("p"));
    send(&mut a, A, &mut server);
    send(&mut b, B, &mut server);
    let _ = server.poll_handshakes();
    let nonce_a = server.challenges[&A].nonce;
    let nonce_b = server.challenges[&B].nonce;
    assert_ne!(nonce_a, nonce_b);

    // A's valid proof, replayed on B's connection.
    let replayed = wire::password_proof(&wire::session_password_key("p"), &nonce_a);
    b.client.send_message(
        Channel::Control,
        wire::encode(&ClientControlMessage::PasswordProof { proof: replayed }),
    );
    send(&mut b, B, &mut server);
    assert_eq!(
        server.poll_handshakes().handshakes,
        vec![refused_password(B)]
    );

    // A proves; a second proof afterwards changes nothing.
    let _ = receive(&mut server, A, &mut a);
    send(&mut a, A, &mut server);
    assert_eq!(server.poll_handshakes().handshakes, vec![admitted(A)]);
    a.client.send_message(
        Channel::Control,
        wire::encode(&ClientControlMessage::PasswordProof { proof: [0; 32] }),
    );
    send(&mut a, A, &mut server);
    assert!(server.poll_handshakes().handshakes.is_empty());
    assert!(
        matches!(
            server.slot_state(A),
            Some(SlotState::Admitted | SlotState::Participating)
        ),
        "a stray proof leaves the slot admitted"
    );

    // A passwordless host ignores a proof.
    let mut open = host("");
    let mut c = joiner(&mut open, A, MOD_ID, Some("p"));
    c.client.send_message(
        Channel::Control,
        wire::encode(&ClientControlMessage::PasswordProof { proof: [0; 32] }),
    );
    send(&mut c, A, &mut open);
    assert_eq!(open.poll_handshakes().handshakes, vec![admitted(A)]);
}

// AC P-d (P9): a client on the wrong mod is refused for its mod and never
// challenged, whatever its password.
#[test]
fn wrong_mod_is_refused_before_any_challenge() {
    let mut server = host("p");
    let mut client = joiner(&mut server, A, "another.mod", Some("p"));
    send(&mut client, A, &mut server);
    let poll = server.poll_handshakes();
    assert!(matches!(
        poll.handshakes.as_slice(),
        [HandshakeOutcome::Rejected {
            cause: ClosingCause::ModId { .. },
            ..
        }]
    ));
    assert!(server.challenges.is_empty());
    let received = receive(&mut server, A, &mut client);
    assert!(
        matches!(
            received.as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Closing(
                ClosingCause::ModId { .. }
            ))]
        ),
        "{received:?}"
    );
}

// AC P-g (P8), P-h (P22): a client that never answers is refused once the
// bound elapses; one whose proof is drained in that same poll is admitted.
#[test]
fn the_proof_bound_refuses_a_silent_client_and_admits_a_proof_on_its_last_poll() {
    let mut server = host("p");
    let mut silent = joiner(&mut server, A, MOD_ID, Some("p"));
    send(&mut silent, A, &mut server);
    let _ = server.poll_handshakes();
    server.update_connections(PASSWORD_PROOF_BOUND - Duration::from_millis(1));
    assert!(
        server.poll_handshakes().handshakes.is_empty(),
        "inside the bound"
    );
    server.update_connections(Duration::from_millis(1));
    assert_eq!(
        server.poll_handshakes().handshakes,
        vec![refused_password(A)]
    );

    let mut server = host("p");
    let mut late = joiner(&mut server, A, MOD_ID, Some("p"));
    send(&mut late, A, &mut server);
    let _ = server.poll_handshakes();
    let _ = receive(&mut server, A, &mut late);
    send(&mut late, A, &mut server);
    server.update_connections(PASSWORD_PROOF_BOUND);
    assert_eq!(server.poll_handshakes().handshakes, vec![admitted(A)]);
}

// AC P-h (P22): a host that ends while a challenge is outstanding sends
// `SessionEnded`; a level change while one is outstanding admits the proving
// client into the new level.
#[test]
fn an_outstanding_challenge_meets_the_session_end_and_a_level_change() {
    let mut server = host("p");
    let mut client = joiner(&mut server, A, MOD_ID, Some("p"));
    send(&mut client, A, &mut server);
    let _ = server.poll_handshakes();
    server.end_session();
    let received = receive(&mut server, A, &mut client);
    assert_eq!(received.last(), Some(&closing(ClosingCause::SessionEnded)));

    let mut server = host("p");
    server.set_relevel_catalog_id(Some("first".into()));
    let mut client = joiner(&mut server, A, MOD_ID, Some("p"));
    send(&mut client, A, &mut server);
    let _ = server.poll_handshakes();
    let _ = receive(&mut server, A, &mut client);
    server.set_relevel_catalog_id(Some("second".into()));
    send(&mut client, A, &mut server);
    // Admitted, then held until it reveals the new install.
    assert_eq!(
        server.poll_handshakes().handshakes.first(),
        Some(&admitted(A))
    );
    let received = receive(&mut server, A, &mut client);
    assert!(
        received.contains(&ServerControlMessage::Relevel {
            catalog_id: "second".into(),
            level_life: 2,
        }),
        "admitted into the new level: {received:?}"
    );
}

// AC P-j: a host with no password sends no challenge and admits a client
// with no password and one with a password.
#[test]
fn a_passwordless_host_admits_without_a_challenge() {
    for password in [None, Some("x")] {
        let mut server = host("");
        let mut client = joiner(&mut server, A, MOD_ID, password);
        send(&mut client, A, &mut server);
        assert_eq!(
            server.poll_handshakes().handshakes,
            vec![admitted(A)],
            "{password:?}"
        );
        assert!(server.challenges.is_empty());
    }
}

// AC P-n: the new Control variants round-trip.
#[test]
fn password_control_variants_round_trip() {
    let challenge = ServerControlMessage::PasswordChallenge { nonce: [3; 32] };
    let proof = ClientControlMessage::PasswordProof { proof: [4; 32] };
    let refused = closing(ClosingCause::Password);
    assert_eq!(
        wire::decode::<ServerControlMessage>(&wire::encode(&challenge)).unwrap(),
        challenge
    );
    assert_eq!(
        wire::decode::<ClientControlMessage>(&wire::encode(&proof)).unwrap(),
        proof
    );
    assert_eq!(
        wire::decode::<ServerControlMessage>(&wire::encode(&refused)).unwrap(),
        refused
    );
}
