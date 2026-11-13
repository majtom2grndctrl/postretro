//! Session membership: a client's leave, every host-side ending, a host's
//! end, and the role-resolved membership and navigation verbs.
//! See: context/lib/networking.md §Slot lifecycle (Leaving and ending a
//! session) · context/lib/scripting.md §10.4 (Session membership)

use super::lifecycle_test_fixtures::{
    catalog_map, connect_test_app_as_client, script_ctx, test_app,
};
use super::session_membership::LeaveCause;
use super::spawn_pose_handoff_fixtures::{Relay, spawn_points};
use super::*;
use crate::netcode::{NetEndpoint, NetRole, SeatTable};
use crate::session::role_state::{RoleBoundState, SessionRole, apply_role};
use crate::startup::{BootState, LevelRequest, LevelSource};
use postretro_entities::{FireOrigin, MachineSource, SystemReactionCommand};
use postretro_foundation::Seat;
use postretro_net::wire::{ClosingCause, DivergenceReason, ServerControlMessage};
use postretro_scripting_core::runtime::{Frontend, MenuCamera};
use postretro_test_log_capture::LogCapture;

const MENU: &str = "mainMenu";
const BACKDROP: &str = "menuBackdrop";

fn own() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}

fn level_load() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::HostTimeline)
}

fn crossing() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::HostMirrored)
}

fn frontend_requests() -> Vec<LevelRequest> {
    vec![
        LevelRequest::Unload,
        LevelRequest::Load(LevelSource::Catalog(BACKDROP.to_string())),
    ]
}

fn requests(app: &App) -> Vec<LevelRequest> {
    app.level_requests.iter().cloned().collect()
}

fn set_role(app: &mut App, role: SessionRole) {
    let session = app.session.as_mut().unwrap();
    let registry = session.scripting.script_ctx.registry.clone();
    let _ = apply_role(
        &mut session.scripting,
        &mut session.seat_table,
        &mut registry.borrow_mut(),
        &RoleBoundState::derive(role, None),
    );
}

/// A single-player app whose mod declares a frontend with a backdrop.
fn single_player_app() -> App {
    let mut app = test_app();
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![
            catalog_map(BACKDROP, "maps/backdrop.prl", "Backdrop", &[]),
            catalog_map("e1m1", "maps/e1m1.prl", "E1M1", &[]),
            catalog_map("e1m2", "maps/e1m2.prl", "E1M2", &[]),
        ]);
    let session = app.session.as_mut().unwrap();
    session.modal_stack.registry_mut().register(
        MENU,
        postretro_ui::demo::build_frontend_menu_descriptor(),
        postretro_ui::modal_stack::ScopeTier::Mod,
        false,
    );
    session.frontend = Some(Frontend {
        menu_tree: MENU.to_string(),
        background_level: Some(BACKDROP.to_string()),
        camera: MenuCamera {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        },
    });
    set_role(&mut app, SessionRole::SinglePlayer);
    app
}

/// A connected client, as `--connect` builds one.
fn client_app() -> App {
    let mut app = single_player_app();
    connect_test_app_as_client(&mut app);
    set_role(&mut app, SessionRole::Client);
    app
}

/// A listen host with no clients, as `--host` builds one.
fn host_app() -> App {
    let mut app = single_player_app();
    let session = app.session.as_mut().unwrap();
    session.net_endpoint = Some(
        NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .unwrap()
            .unwrap(),
    );
    session.seat_table = Some(SeatTable::new().unwrap());
    set_role(&mut app, SessionRole::Host);
    app
}

fn run(app: &mut App, commands: impl IntoIterator<Item = SystemReactionCommand>) {
    let ctx = script_ctx(app);
    for command in commands {
        ctx.system_commands.push(command);
    }
    app.dispatch_system_commands();
}

fn leave(source: &str) -> SystemReactionCommand {
    SystemReactionCommand::LeaveSession {
        dispatch_source: source.to_string(),
        origin: own(),
    }
}

fn end(source: &str, origin: FireOrigin) -> SystemReactionCommand {
    SystemReactionCommand::EndSession {
        dispatch_source: source.to_string(),
        origin,
    }
}

fn return_to_frontend(source: &str, origin: FireOrigin) -> SystemReactionCommand {
    SystemReactionCommand::ReturnToFrontend {
        dispatch_source: source.to_string(),
        origin,
    }
}

fn load_level(map: &str) -> SystemReactionCommand {
    SystemReactionCommand::LoadLevel {
        map: map.to_string(),
        dispatch_source: "named:pick".to_string(),
    }
}

fn session_ended() -> ServerControlMessage {
    ServerControlMessage::Divergence(DivergenceReason::Closing(ClosingCause::SessionEnded))
}

fn observe(app: &App) -> RoleBoundState {
    let session = app.session.as_ref().unwrap();
    RoleBoundState::observe(&session.scripting, session.seat_table.as_ref())
}

fn single_player_state() -> RoleBoundState {
    RoleBoundState::derive(SessionRole::SinglePlayer, None)
}

// AC 1: a client's leave drops its endpoint, presents the frontend and queues
// the unload, then the backdrop; afterwards its own level verbs work.
#[test]
fn client_leave_session_lands_at_single_player_frontend() {
    let capture = LogCapture::start();
    let mut app = client_app();

    run(&mut app, [leave("ui:pause.leave")]);

    assert_eq!(app.session_role(), SessionRole::SinglePlayer);
    assert!(app.frontend_menu_is_present());
    assert_eq!(requests(&app), frontend_requests());

    run(&mut app, [load_level("e1m1")]);
    assert_eq!(
        app.level_requests.back(),
        Some(&LevelRequest::Load(LevelSource::Catalog(
            "e1m1".to_string()
        )))
    );
    capture.assert_not_logged(log::Level::Warn, "ignored");
}

// AC 2.
#[test]
fn quit_to_menu_resolves_membership_by_role() {
    let mut single_player = single_player_app();
    single_player.quit_to_menu();
    assert_eq!(requests(&single_player), frontend_requests());

    let mut client = client_app();
    client.quit_to_menu();
    assert_eq!(client.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&client), frontend_requests());

    let mut host = host_app();
    host.quit_to_menu();
    assert_eq!(host.session_role(), SessionRole::SinglePlayer);
    assert!(
        host.session
            .as_ref()
            .unwrap()
            .closing_host_transport
            .is_some()
    );
    assert_eq!(requests(&host), frontend_requests());
}

// AC 3.
#[test]
fn return_to_frontend_refusals_follow_role_and_origin() {
    let capture = LogCapture::start();
    let mut host = host_app();
    for _ in 0..2 {
        run(
            &mut host,
            [
                return_to_frontend("named:levelLoad", level_load()),
                return_to_frontend("crossing:0", crossing()),
                return_to_frontend("ui:quit", own()),
            ],
        );
    }
    assert_eq!(host.session_role(), SessionRole::Host);
    assert!(requests(&host).is_empty());
    for source in ["named:levelLoad", "crossing:0", "ui:quit"] {
        capture.assert_logged_once(
            log::Level::Warn,
            &format!("returnToFrontend from `{source}` ignored on a host"),
        );
    }
    capture.assert_logged(log::Level::Warn, "`endSession()`");

    let mut client = client_app();
    for _ in 0..2 {
        run(
            &mut client,
            [
                return_to_frontend("named:levelLoad", level_load()),
                return_to_frontend("crossing:1", crossing()),
                return_to_frontend("ui:client-quit", own()),
            ],
        );
    }
    assert_eq!(client.session_role(), SessionRole::Client);
    assert!(requests(&client).is_empty());
    capture.assert_logged_once(
        log::Level::Warn,
        "returnToFrontend from `ui:client-quit` ignored on a connected client",
    );
    capture.assert_not_logged(
        log::Level::Warn,
        "returnToFrontend from `crossing:1` ignored on a connected client",
    );
    capture.assert_logged(
        log::Level::Debug,
        "returnToFrontend from `crossing:1` ignored on a connected client",
    );

    let mut single_player = single_player_app();
    run(&mut single_player, [return_to_frontend("ui:quit", own())]);
    assert_eq!(requests(&single_player), frontend_requests());
}

// AC 4.
#[test]
fn membership_verb_noops_log_by_role_and_origin() {
    let capture = LogCapture::start();
    let mut host = host_app();
    run(&mut host, [leave("ui:h"), leave("ui:h")]);
    assert_eq!(host.session_role(), SessionRole::Host);
    capture.assert_logged_once(
        log::Level::Warn,
        "leaveSession from `ui:h` ignored on a host",
    );

    let mut single_player = single_player_app();
    run(&mut single_player, [leave("ui:s"), end("ui:s", own())]);
    assert!(requests(&single_player).is_empty());
    capture.assert_logged(
        log::Level::Debug,
        "leaveSession from `ui:s`: no session to leave",
    );
    capture.assert_logged(
        log::Level::Debug,
        "endSession from `ui:s`: no session to end",
    );
    capture.assert_not_logged(log::Level::Warn, "from `ui:s`");

    let mut client = client_app();
    run(
        &mut client,
        [
            end("ui:c", own()),
            end("ui:c", own()),
            end("named:levelLoad", level_load()),
            end("crossing:2", crossing()),
        ],
    );
    assert_eq!(client.session_role(), SessionRole::Client);
    capture.assert_logged_once(
        log::Level::Warn,
        "endSession from `ui:c` ignored on a connected client",
    );
    capture.assert_logged(log::Level::Warn, "a client leaves with `leaveSession()`");
    capture.assert_not_logged(log::Level::Warn, "endSession from `named:levelLoad`");
    capture.assert_not_logged(log::Level::Warn, "endSession from `crossing:2`");
}

// AC 5 (pin P15): steps after a leave drop at debug, so one frontend return
// is queued and the level never loads.
#[test]
fn steps_after_leave_drop_at_debug() {
    for after in [return_to_frontend("ui:leave", own()), load_level("e1m2")] {
        let capture = LogCapture::start();
        let mut client = client_app();
        run(&mut client, [leave("ui:leave"), after]);
        assert_eq!(client.session_role(), SessionRole::SinglePlayer);
        assert_eq!(requests(&client), frontend_requests());
        capture.assert_logged(log::Level::Debug, "left its session earlier in this drain");
        capture.assert_nothing_above(log::Level::Info);
    }
}

// AC 7, the direct case: `[endSession(), returnToFrontend()]` in every role.
#[test]
fn run_end_composition_behaves_per_role() {
    let capture = LogCapture::start();
    let mut single_player = single_player_app();
    run(
        &mut single_player,
        [
            end("run.end", crossing()),
            return_to_frontend("run.end", crossing()),
        ],
    );
    assert_eq!(requests(&single_player), frontend_requests());

    let mut host = host_app();
    run(
        &mut host,
        [
            end("run.end", crossing()),
            return_to_frontend("run.end", crossing()),
        ],
    );
    assert_eq!(host.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&host), frontend_requests());
    capture.assert_not_logged(log::Level::Warn, "ignored");

    // A client lands at the frontend when the host's cause arrives.
    let mut client = client_app();
    run(
        &mut client,
        [
            end("run.end", crossing()),
            return_to_frontend("run.end", crossing()),
        ],
    );
    assert_eq!(client.session_role(), SessionRole::Client);
    assert!(requests(&client).is_empty());
    capture.assert_not_logged(log::Level::Warn, "ignored");
    crate::client_drain_control(&mut client, vec![session_ended()]);
    assert_eq!(client.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&client), frontend_requests());

    // From a UI press each refused verb warns, `endSession` naming the leave.
    let mut client = client_app();
    run(
        &mut client,
        [
            end("ui:run.end", own()),
            return_to_frontend("ui:run.end", own()),
        ],
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "endSession from `ui:run.end` ignored on a connected client: only the host ends a session; a client leaves with `leaveSession()`",
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "returnToFrontend from `ui:run.end` ignored on a connected client",
    );
}

// AC 16 and 17: every host-side ending takes the leave path; the log names
// the cause once.
#[test]
fn host_side_endings_take_the_leave_path() {
    let capture = LogCapture::start();
    let mut ended = client_app();
    crate::client_drain_control(&mut ended, vec![session_ended()]);
    assert_eq!(ended.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&ended), frontend_requests());
    capture.assert_logged_once(log::Level::Info, "[Net] the host ended the session");
    capture.assert_not_logged(log::Level::Error, "incompatible host");
    capture.assert_not_logged(log::Level::Error, "lost connection");

    let mut refused = client_app();
    crate::client_drain_control(
        &mut refused,
        vec![ServerControlMessage::Divergence(DivergenceReason::Closing(
            ClosingCause::ModId {
                expected: "host".to_string(),
                received: "client".to_string(),
                expected_version: "1".to_string(),
                received_version: "1".to_string(),
            },
        ))],
    );
    assert_eq!(refused.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&refused), frontend_requests());

    // `connect_test_app_as_client` dials a port nothing answers.
    let mut unanswered = client_app();
    let poll = |app: &mut App| {
        if let Some(NetEndpoint::Client { client, .. }) =
            app.session.as_mut().unwrap().net_endpoint.as_mut()
        {
            let _ = client.update(std::time::Duration::from_secs(16));
        }
        app.follow_client_connection_end();
    };
    poll(&mut unanswered);
    poll(&mut unanswered);
    assert_eq!(
        unanswered.session_role(),
        SessionRole::Client,
        "a first connect still retrying has not ended"
    );
    for _ in 0..postretro_net::transport::CONNECT_ATTEMPTS * 2 {
        poll(&mut unanswered);
    }
    assert_eq!(unanswered.session_role(), SessionRole::SinglePlayer);
    assert_eq!(requests(&unanswered), frontend_requests());
}

// AC 21: a left client and an ended host hold what a no-flag session holds;
// the comparison covers every role-bound field.
#[test]
fn left_and_ended_role_state_matches_single_player() {
    let mut client = client_app();
    assert_ne!(observe(&client), single_player_state());
    assert!(client.leave_session(LeaveCause::Own));
    assert_eq!(observe(&client), single_player_state());

    let mut host = host_app();
    host.end_session();
    assert_eq!(observe(&host), single_player_state());
    assert_eq!(observe(&single_player_app()), single_player_state());
}

// AC 22 and 24 (pin P3): nothing the host named survives the leave, a
// relevel before the cause in the same drain included, and a relevel after
// it loads nothing.
#[test]
fn nothing_from_the_host_steers_after_leave() {
    let mut client = client_app();
    crate::client_drain_control(
        &mut client,
        vec![
            ServerControlMessage::Relevel {
                catalog_id: "e1m1".to_string(),
                level_life: 4,
            },
            session_ended(),
            ServerControlMessage::Relevel {
                catalog_id: "e1m2".to_string(),
                level_life: 5,
            },
        ],
    );
    assert_eq!(requests(&client), frontend_requests());
    assert_eq!(client.last_relevel, None);
    assert!(!client.active_level_host_named);

    // With the endpoint gone, no later relevel can arrive.
    assert!(client.session.as_ref().unwrap().net_endpoint.is_none());
}

// AC 25 (pin P8): the cause and an own leave in one frame, or two leaves in
// one drain, leave once.
#[test]
fn double_leave_is_idempotent() {
    let capture = LogCapture::start();
    let mut client = client_app();
    crate::client_drain_control(&mut client, vec![session_ended()]);
    run(&mut client, [leave("ui:again")]);
    assert_eq!(requests(&client), frontend_requests());
    capture.assert_logged(log::Level::Debug, "no session to leave");

    let mut client = client_app();
    run(&mut client, [leave("ui:a"), leave("ui:b")]);
    assert_eq!(requests(&client), frontend_requests());
    capture.assert_logged_once(log::Level::Info, "[Net] leaving the session");
}

// AC 23 (pin P1): a client whose session ends with a level installed stops
// that level at once, and only the leave's requests are queued.
#[test]
fn ended_client_level_stops_before_unload() {
    let mut client = client_app();
    client.boot_state = BootState::Running;
    client.enqueue_level_request(LevelRequest::HostLevel("e1m2".to_string()));
    crate::client_drain_control(&mut client, vec![session_ended()]);
    assert!(client.level_stopped_by_session_end());
    assert_eq!(requests(&client), frontend_requests());

    client.unload_level();
    assert!(!client.level_stopped_by_session_end());
}

// AC 11, 13 and the host half of pins P13 and P18: an ended host stays in its
// level as single-player with its own seat; every connection that has
// participated, held ones included, gets its final character before the
// cause, and one that never participated gets the cause alone.
#[test]
fn ended_host_keeps_its_seat_and_returns_each_participants_character() {
    const PARTICIPATING: u64 = 21;
    const HELD: u64 = 22;
    const NEVER: u64 = 23;
    let mut relay = Relay::admitted(&[PARTICIPATING, HELD, NEVER]);
    let (host_pawn, _) = relay.install(spawn_points(4)).expect("host pawn");
    relay.host_reveal();
    relay.reveal(PARTICIPATING);
    relay.reveal(HELD);
    let held_pawn = relay.await_pawn(HELD);
    let pawn = relay.await_pawn(PARTICIPATING);
    // A level change holds both; only one reveals the new install.
    relay.unload();
    relay.install(spawn_points(4));
    relay.host_reveal();
    relay.reveal(PARTICIPATING);
    let pawn_after = relay.await_pawn(PARTICIPATING);
    for client in [PARTICIPATING, HELD, NEVER] {
        let _ = relay.forward(client);
    }

    relay.app.end_session();

    assert_eq!(relay.app.session_role(), SessionRole::SinglePlayer);
    assert_eq!(observe(&relay.app), single_player_state());
    let registry = script_ctx(&relay.app).registry.clone();
    for remote in [pawn, held_pawn, pawn_after] {
        assert!(!registry.borrow().exists(remote), "remote pawns go");
    }
    let session = relay.app.session.as_ref().unwrap();
    assert_eq!(session.seat_table.as_ref().unwrap().seats(), vec![Seat(0)]);
    let _ = host_pawn;
    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    for client in [PARTICIPATING, HELD] {
        let messages = relay.forward(client);
        assert!(
            matches!(
                messages.as_slice(),
                [.., ServerControlMessage::FinalCharacter { .. }, cause] if *cause == session_ended()
            ),
            "client {client}: the final character, then the cause, last: {messages:?}"
        );
    }
    assert_eq!(relay.forward(NEVER), vec![session_ended()]);
}

// AC 13 (pin P7): a host at its frontend, with no level, ends the same way.
#[test]
fn frontend_host_end_closes_a_world_less_client() {
    const CLIENT: u64 = 31;
    let mut relay = Relay::admitted(&[CLIENT]);
    let _ = relay.forward(CLIENT);
    relay.app.end_session();
    assert_eq!(relay.app.session_role(), SessionRole::SinglePlayer);
    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    let messages = relay.forward(CLIENT);
    assert_eq!(messages.last(), Some(&session_ended()));
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, ServerControlMessage::FinalCharacter { .. })),
        "a client that never participated keeps its saved character: {messages:?}"
    );
}

// AC 19: every exit path runs through `exiting`, which leaves or ends first.
#[test]
fn exit_paths_leave_or_end_before_exit() {
    let mut client = client_app();
    client.leave_or_end_for_exit();
    assert_eq!(client.session_role(), SessionRole::SinglePlayer);

    let mut relay = Relay::admitted(&[41]);
    let started = std::time::Instant::now();
    relay.app.leave_or_end_for_exit();
    assert!(started.elapsed() < std::time::Duration::from_millis(2_500));
    assert_eq!(relay.app.session_role(), SessionRole::SinglePlayer);
    assert!(
        relay
            .app
            .session
            .as_ref()
            .unwrap()
            .closing_host_transport
            .is_none()
    );

    // Window close and the quit chord call `event_loop.exit()`, and
    // `ui.exitToDesktop` sets the exit flag; winit reaches `exiting` for
    // each, which leaves or ends before any save.
    let source = include_str!("../main.rs");
    let exiting = &source[source.find("fn exiting(").expect("`exiting` exists")..];
    let leave = exiting
        .find("self.leave_or_end_for_exit();")
        .expect("`exiting` leaves or ends");
    let save = exiting
        .find("save_connected_client_per_owner_state")
        .expect("`exiting` saves");
    assert!(leave < save, "the leave or end runs before the exit saves");
}
