use super::*;
use crate::startup::lifecycle::tests::test_app;

fn published() -> Option<SocketAddr> {
    Some(SocketAddr::from(([192, 168, 1, 20], 27_960)))
}

fn apply_and_observe(app: &mut crate::App, state: &RoleBoundState) -> RoleBoundState {
    let session = app.session.as_mut().expect("test session");
    let registry = session.scripting.script_ctx.registry.clone();
    let _ = apply_role(
        &mut session.scripting,
        &mut session.seat_table,
        &mut registry.borrow_mut(),
        state,
    );
    RoleBoundState::observe(&session.scripting, session.seat_table.as_ref())
}

// The derivation, the write and the read agree for every role, so an
// observation of a session is what its role derives.
#[test]
fn apply_then_observe_reproduces_every_role_derivation() {
    for role in [
        SessionRole::Client,
        SessionRole::Host,
        SessionRole::SinglePlayer,
    ] {
        let mut app = test_app();
        let derived = RoleBoundState::derive(role, published());
        assert_eq!(apply_and_observe(&mut app, &derived), derived, "{role:?}");
    }
}

#[test]
fn only_a_host_publishes_an_address() {
    for role in [SessionRole::Client, SessionRole::SinglePlayer] {
        let derived = RoleBoundState::derive(role, published());
        assert!(
            !derived.hosting && derived.host_address.is_empty(),
            "{role:?}"
        );
    }
    let host = RoleBoundState::derive(SessionRole::Host, published());
    assert!(host.hosting);
    assert_eq!(host.host_address, "192.168.1.20:27960");
}

// A role moves one way, to single-player: re-applying single-player over a
// client or a host leaves exactly what a session that started single-player
// holds. A host keeps its own seat and releases the rest.
#[test]
fn single_player_over_any_role_matches_a_single_player_boot() {
    let single_player = RoleBoundState::derive(SessionRole::SinglePlayer, None);
    for role in [SessionRole::Client, SessionRole::Host] {
        let mut app = test_app();
        let _ = apply_and_observe(&mut app, &RoleBoundState::derive(role, published()));
        assert_eq!(
            apply_and_observe(&mut app, &single_player),
            single_player,
            "{role:?} to single-player"
        );
    }
}
