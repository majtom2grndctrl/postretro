//! In-app session entry: a single-player engine at its frontend joins or
//! hosts over a real loopback transport, built on the path boot flags take.
//! See: context/lib/networking.md §Role model (Runtime entry)

use super::lifecycle_test_fixtures::test_app;
use super::session_membership::LeaveCause;
use super::spawn_pose_handoff_fixtures::{LEVEL, MOD_ID, Relay, spawn_points};
use super::spawn_pose_stand_in_fixtures::ClientView;
use super::*;
use crate::netcode::{NetEndpoint, NetRole};
use crate::session::net_entry::enter_role;
use crate::session::role_state::{RoleBoundState, SessionRole, apply_role};
use crate::startup::{BootState, LevelRequest};
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::Seat;
use postretro_net::slots::SlotState;
use postretro_net::wire::SessionId;
use postretro_net::wire::SpawnPoseHint;
use postretro_sim::scripting::map_entity::MapEntity;

/// One client frame's transport poll, at a frame's length.
const FRAME_SECONDS: f32 = 0.034;

/// A single-player engine at its frontend menu: its mod committed the relay
/// host's id, as a matching mod's init would.
fn single_player_at_menu(points: Vec<MapEntity>) -> ClientView {
    let mut view = ClientView::single_player(points).with_frontend();
    view.app
        .session
        .as_mut()
        .unwrap()
        .scripting
        .script_runtime
        .commit_mod_identity_for_test(MOD_ID, "1");
    // A single-player session holds its own seat ledger, as boot builds it.
    let session = view.app.session.as_mut().unwrap();
    let registry = session.scripting.script_ctx.registry.clone();
    let _ = apply_role(
        &mut session.scripting,
        &mut session.seat_table,
        &mut registry.borrow_mut(),
        &RoleBoundState::derive(SessionRole::SinglePlayer, None),
    );
    view.app.present_frontend_menu();
    assert!(view.app.at_frontend());
    view
}

/// A host whose catalog level is installed and revealed.
fn host_in_level(points: &[MapEntity]) -> Relay {
    let mut relay = Relay::host();
    relay.install(points.to_vec());
    relay.host_reveal();
    relay
}

/// The engine client's connection id on the host: the one connection there.
fn joined_client_id(relay: &mut Relay) -> Option<u64> {
    relay.server().connected_clients().first().copied()
}

/// One frame each side: the host's poll, then the engine client's, which
/// runs the frame path its boot state runs.
fn exchange(relay: &mut Relay, view: &mut ClientView) {
    relay.step();
    if view.app.level_is_installed_state() {
        view.app.net_poll_and_apply(FRAME_SECONDS);
    } else {
        let _ = view.app.poll_world_less_transport(FRAME_SECONDS);
    }
}

#[track_caller]
fn exchange_until(
    relay: &mut Relay,
    view: &mut ClientView,
    done: impl Fn(&mut Relay, &ClientView) -> bool,
) {
    // Dev builds relay both endpoints through lag proxies on their own
    // threads, so each exchange leaves them a moment of wall time.
    for _ in 0..1500 {
        exchange(relay, view);
        if done(relay, view) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the in-app session did not reach the expected state");
}

/// Follow the host into its level: the relevel installs it, the client
/// reveals, and the host promotes it. Returns the client's connection id and
/// the hint the client holds.
fn follow_into_level(relay: &mut Relay, view: &mut ClientView) -> (u64, SpawnPoseHint) {
    exchange_until(relay, view, |_, view| {
        view.app
            .level_requests
            .contains(&LevelRequest::HostLevel(LEVEL.to_string()))
    });
    view.install();
    assert!(
        !view.app.frontend_menu_is_present(),
        "the host's level clears the frontend menu"
    );
    view.reveal();
    exchange_until(relay, view, |relay, _| {
        joined_client_id(relay).is_some_and(|id| relay.pawn(id).is_some())
    });
    let id = joined_client_id(relay).unwrap();
    let hint = view
        .stored_hint()
        .expect("the host hinted the client's spawn");
    (id, hint)
}

#[track_caller]
fn assert_client_role(view: &ClientView) {
    let session = view.app.session.as_ref().unwrap();
    assert!(matches!(
        session.net_endpoint,
        Some(NetEndpoint::Client { .. })
    ));
    let mut observed = RoleBoundState::observe(&session.scripting, session.seat_table.as_ref());
    // The host's roster projection arrives once admitted; it is no role fact.
    observed.open_seats = None;
    assert_eq!(observed, RoleBoundState::derive(SessionRole::Client, None));
}

// AC E2: with no backdrop, and with the host's map installed as a backdrop.
#[test]
fn in_app_join_from_the_frontend_participates_in_the_hosts_level_at_the_hinted_spawn() {
    let points = spawn_points(3);
    for backdrop in [false, true] {
        let mut relay = host_in_level(&points);
        let mut view = single_player_at_menu(points.clone());
        if backdrop {
            view.app.populate_frontend();
            view.install();
            assert!(view.app.level_is_installed_state());
            assert!(view.app.at_frontend(), "a backdrop counts as the frontend");
        }

        assert!(view.app.join_from_frontend(relay.address()));
        assert_client_role(&view);

        let (id, hint) = follow_into_level(&mut relay, &mut view);
        assert!(relay.participating(id), "backdrop {backdrop}: participates");
        let pawn = relay.pawn(id).unwrap();
        relay.assert_pawn_at_hint(pawn, &hint);
        assert_eq!(view.app.boot_state, BootState::Running);
    }
}

// AC E3 (P3): a join cancels a backdrop load in flight instead of adopting
// it, and the host's level installs as a client's.
#[test]
fn in_app_join_cancels_a_backdrop_load_in_flight_and_installs_the_hosts_level_as_a_client() {
    let points = spawn_points(3);
    let mut relay = host_in_level(&points);
    let mut view = single_player_at_menu(points);
    view.app.populate_frontend();
    view.app.drain_level_requests();
    assert!(
        view.app.level_load_in_flight(),
        "fixture: the backdrop loads"
    );
    assert!(!view.app.level_load.as_ref().unwrap().host_named);

    assert!(view.app.join_from_frontend(relay.address()));
    assert!(
        !view.app.level_load_in_flight(),
        "the backdrop load is cancelled"
    );
    assert!(view.app.level_load.is_none());
    assert!(view.app.level_requests.is_empty());
    assert_eq!(view.app.boot_state, BootState::Frontend);
    assert!(view.app.frontend_menu_is_present());

    exchange_until(&mut relay, &mut view, |_, view| {
        view.app
            .level_requests
            .contains(&LevelRequest::HostLevel(LEVEL.to_string()))
    });
    // The host-named load dispatches as a client's: its model prediction and
    // install suppression read the role at dispatch and install.
    view.app.drain_level_requests();
    assert!(view.app.level_load.as_ref().unwrap().host_named);
    assert!(view.app.is_connected_client());
    view.commit();
    view.reveal();
    exchange_until(&mut relay, &mut view, |relay, _| {
        joined_client_id(relay).is_some_and(|id| relay.participating(id))
    });
    let declared = view
        .declared_reveal()
        .expect("the client declared its reveal");
    assert_eq!(declared.level_life, relay.server().installed_level_life());
    assert_client_role(&view);
}

// A join from anywhere but a single-player frontend changes nothing.
#[test]
fn in_app_join_off_the_frontend_is_refused_and_changes_nothing() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let mut view = single_player_at_menu(points);
    view.app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    assert!(!view.app.at_frontend());
    assert!(!view.app.join_from_frontend(relay.address()));
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);
    assert!(view.app.session.as_ref().unwrap().seat_table.is_some());
}

fn slot(app: &App, name: &str) -> Option<SlotValue> {
    script_ctx_of(app)
        .slot_table
        .borrow()
        .get(name)
        .and_then(|record| record.value.clone())
}

fn script_ctx_of(app: &App) -> postretro_entities::ScriptCtx {
    app.session.as_ref().unwrap().scripting.script_ctx.clone()
}

fn session_id(app: &App) -> SessionId {
    app.session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .session_id_for_test()
}

/// Pump an ended host's closing transport until it drops.
fn close_out(app: &mut App) {
    for _ in 0..200 {
        if app
            .session
            .as_ref()
            .unwrap()
            .closing_host_transport
            .is_none()
        {
            return;
        }
        app.pump_closing_host_transport(std::time::Duration::from_millis(34));
    }
    panic!("the closing transport did not drop");
}

// AC E1: a host at its frontend publishes its address, holds a joiner
// admitted below participating, and promotes it into the level it starts.
#[test]
fn in_app_host_holds_a_joiner_at_its_frontend_and_promotes_it_into_its_next_level() {
    let points = spawn_points(3);
    let mut host = single_player_at_menu(points.clone());
    assert!(host.app.host_from_frontend(0));
    assert_eq!(host.app.session_role(), SessionRole::Host);
    assert_eq!(
        slot(&host.app, "session.hosting"),
        Some(SlotValue::Boolean(true))
    );
    assert!(matches!(
        slot(&host.app, "session.hostAddress"),
        Some(SlotValue::String(address)) if !address.is_empty()
    ));

    let mut relay = Relay::adopt(host.app);
    let mut view = single_player_at_menu(points.clone());
    assert!(view.app.join_from_frontend(relay.address()));
    exchange_until(&mut relay, &mut view, |relay, _| {
        joined_client_id(relay)
            .is_some_and(|id| relay.server().slot_state(id) == Some(SlotState::Admitted))
    });
    for _ in 0..24 {
        exchange(&mut relay, &mut view);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let id = joined_client_id(&mut relay).unwrap();
    assert!(
        !relay.participating(id),
        "the joiner waits at the host's frontend"
    );
    assert!(
        view.app.level_requests.is_empty(),
        "the host names no level"
    );

    // The host starts a level: its menu clears and the level installs.
    relay
        .app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    relay.world_less = false;
    relay.install(points);
    relay.host_reveal();
    let (joined, hint) = follow_into_level(&mut relay, &mut view);
    assert_eq!(joined, id);
    let pawn = relay.pawn(id).unwrap();
    relay.assert_pawn_at_hint(pawn, &hint);
}

// AC E4: host, end, join elsewhere, leave, host again, in one process.
#[test]
fn one_process_hosts_ends_joins_leaves_and_hosts_again() {
    let points = spawn_points(3);
    let mut view = single_player_at_menu(points.clone());
    assert!(view.app.host_from_frontend(0));
    let first_run = session_id(&view.app);
    view.app.end_session();
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);
    close_out(&mut view.app);
    assert!(view.app.at_frontend());

    let mut elsewhere = host_in_level(&points);
    assert!(view.app.join_from_frontend(elsewhere.address()));
    let (id, _) = follow_into_level(&mut elsewhere, &mut view);
    assert!(elsewhere.participating(id));

    assert!(view.app.leave_session(LeaveCause::Own));
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);
    view.app.drain_level_requests();
    assert!(
        view.app.at_frontend(),
        "the left level unloaded under the menu"
    );

    assert!(view.app.host_from_frontend(0));
    assert_eq!(view.app.session_role(), SessionRole::Host);
    assert_ne!(session_id(&view.app), first_run, "a new hosted run");
}

// AC E5 (P15): a second hosted run mints a new session id, its first joiner
// takes the seat a fresh session gives, and seat 0 keeps its carry.
#[test]
fn hosting_again_mints_a_new_run_and_its_first_joiner_takes_seat_one() {
    let points = spawn_points(3);
    let mut view = single_player_at_menu(points);
    assert!(view.app.host_from_frontend(0));
    let first_run = session_id(&view.app);
    let mut relay = Relay::adopt(view.app);
    relay.admit(&[21]);
    assert_eq!(relay.seat(21), Some(Seat(1)));
    relay.app.end_session();
    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    close_out(&mut relay.app);
    let carried = relay.seats().carried_state(Seat(0)).cloned();

    let mut app = relay.app;
    assert!(app.host_from_frontend(0));
    assert_ne!(session_id(&app), first_run);
    let mut relay = Relay::adopt(app);
    assert_eq!(relay.seats().seats(), vec![Seat(0)]);
    assert_eq!(relay.seats().carried_state(Seat(0)).cloned(), carried);
    relay.admit(&[22]);
    assert_eq!(
        relay.seat(22),
        Some(Seat(1)),
        "seat ids restart with the run"
    );
}

// AC E8: a failed session-id mint leaves the ledger, the role and the
// endpoint as they were.
#[test]
fn failed_session_mint_at_host_entry_stays_single_player_with_its_ledger() {
    let mut view = single_player_at_menu(spawn_points(3));
    let before = session_id(&view.app);
    let hosted = view
        .app
        .host_from_frontend_with(0, |_| Err(getrandom::Error::UNSUPPORTED), "");
    assert!(!hosted);
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);
    assert_eq!(session_id(&view.app), before);
    assert_eq!(
        slot(&view.app, "session.hosting"),
        Some(SlotValue::Boolean(false))
    );
}

// AC E12: boot flags and in-app entry produce the same role-bound state.
#[test]
fn boot_flags_and_in_app_entry_derive_the_same_role_bound_state() {
    fn observe(app: &App) -> RoleBoundState {
        let session = app.session.as_ref().unwrap();
        RoleBoundState::observe(&session.scripting, session.seat_table.as_ref())
    }
    fn boot(role: &NetRole, password: &str) -> RoleBoundState {
        let mut app = test_app();
        let session = app.session.as_mut().unwrap();
        session.net_endpoint = enter_role(
            role,
            None,
            "",
            &mut session.scripting,
            &mut session.seat_table,
            getrandom::fill,
            crate::netcode::probe_lan_address,
            password,
        )
        .endpoint;
        observe(&app)
    }
    let relay = host_in_level(&spawn_points(3));

    let mut host = single_player_at_menu(spawn_points(3));
    assert!(host.app.host_from_frontend(0));
    assert_eq!(observe(&host.app), boot(&NetRole::Host { port: 0 }, ""));

    // A password-protected run says so, from either entry.
    let mut host = single_player_at_menu(spawn_points(3));
    assert!(host.app.host_from_frontend_with(0, getrandom::fill, "p"));
    let protected = observe(&host.app);
    assert!(protected.password_protected);
    assert_eq!(protected, boot(&NetRole::Host { port: 0 }, "p"));

    let mut client = single_player_at_menu(spawn_points(3));
    assert!(client.app.join_from_frontend(relay.address()));
    assert_eq!(
        observe(&client.app),
        boot(
            &NetRole::Connect {
                addr: relay.address()
            },
            ""
        )
    );
}

const PLAYER_ID: [u8; 16] = [5; 16];
const WORLD_SLOT: &str = "game.score";
const CHARACTER_SLOT: &str = "game.xp";

fn persisted_slot(per_owner: bool) -> postretro_entities::slot_table::SlotRecord {
    use postretro_entities::slot_table::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    };
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(0.0)),
        range: None,
        persist: true,
        readonly: false,
        ownership: SlotOwnership::Mod,
        // The world slot replicates, so a host's value replaces the client's
        // while it plays; a slot that never replicates keeps its writes.
        network: if per_owner {
            ReplicationScope::None
        } else {
            ReplicationScope::SharedGlobal
        },
        per_owner,
        accumulate: None,
    })
}

/// A single-player engine whose mod declares a persisted world slot and a
/// persisted character slot, whose boot restore completed with both at 0,
/// and which saves under `data_dir`.
fn with_saves(view: &mut ClientView, data_dir: &std::path::Path) {
    let session = view.app.session.as_mut().unwrap();
    session.data_dir = Some(data_dir.to_path_buf());
    session.player_options.player_id = Some(PLAYER_ID);
    session
        .scripting
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "game",
            vec![
                ("score".to_string(), persisted_slot(false)),
                ("xp".to_string(), persisted_slot(true)),
            ],
        )
        .unwrap();
    let mut ledger = postretro_scripting_core::store_identity::StoreIdentityLedger::empty();
    ledger
        .slots
        .insert(WORLD_SLOT.to_string(), "k-score".to_string());
    ledger
        .slots
        .insert(CHARACTER_SLOT.to_string(), "k-xp".to_string());
    session
        .scripting
        .script_runtime
        .commit_store_identity_for_test(
            ledger,
            [WORLD_SLOT.to_string(), CHARACTER_SLOT.to_string()].into(),
        );
    assert!(
        session
            .state_store_lifecycle
            .should_restore_after_mod_init(true)
    );
    crate::session::saves::boot_restore(session);
    session.state_store_lifecycle.mark_restore_completed();
    let boot = crate::session::progress_save::save_world_and_character(session)
        .expect("fixture: saves are allowed");
    session.persisted_state = Some(boot.state);
}

fn write_world(app: &App, value: f32) {
    script_ctx_of(app)
        .slot_table
        .borrow_mut()
        .get_mut(WORLD_SLOT)
        .unwrap()
        .write_value(Some(SlotValue::Number(value)));
}

fn write_character(app: &App, value: f32) {
    script_ctx_of(app)
        .slot_table
        .borrow_mut()
        .get_mut(CHARACTER_SLOT)
        .unwrap()
        .set_per_seat_value(Seat(0), SlotValue::Number(value));
}

/// The world file: what a host or single player's save point always writes.
fn saved_file(app: &App) -> std::path::PathBuf {
    let session = app.session.as_ref().unwrap();
    postretro_sim::scripting::save_files::SaveDir::new(session.data_dir.as_deref().unwrap(), MOD_ID)
        .world_path()
}

/// The world file and character slot 0 as the store document they compose.
fn on_disk_state(app: &App) -> crate::scripting::state_persistence::PersistedState {
    use postretro_sim::scripting::save_files::{SaveDir, read_document};
    let session = app.session.as_ref().unwrap();
    let dir = SaveDir::new(session.data_dir.as_deref().unwrap(), MOD_ID);
    let world = read_document(&dir.world_path()).unwrap();
    let character = read_document(&dir.character_path(0)).unwrap();
    crate::scripting::state_persistence::PersistedState::from_ledger(
        world.as_ref(),
        None,
        character
            .as_ref()
            .and_then(|document| document.seats.first()),
    )
}

fn character_in(state: Option<&crate::scripting::state_persistence::PersistedState>) -> f64 {
    let seed =
        crate::scripting::state_persistence::join_seed_from_persisted_state(state, Some(PLAYER_ID));
    match seed.get("k-xp") {
        Some(postretro_net::wire::JoinSeedValue::Number(value)) => *value,
        other => panic!("the character slot rides the join seed: {other:?}"),
    }
}

// AC S1: progress made after boot is on disk once a join starts, before the
// client's first poll sends a packet.
#[test]
fn join_saves_world_and_character_before_its_first_connect_packet() {
    let dir = tempfile::tempdir().unwrap();
    let relay = host_in_level(&spawn_points(3));
    let mut view = single_player_at_menu(spawn_points(3));
    with_saves(&mut view, dir.path());
    write_world(&view.app, 42.0);
    write_character(&view.app, 7.0);

    assert!(view.app.join_from_frontend(relay.address()));

    let on_disk = on_disk_state(&view.app);
    assert_eq!(
        Some(&on_disk),
        view.app.session.as_ref().unwrap().persisted_state.as_ref(),
        "the restore snapshot is what the join wrote"
    );
    assert_eq!(character_in(Some(&on_disk)), 7.0);
}

// AC S2: a leave restores the world at join, and the join seed carries the
// character at join, not at boot.
#[test]
fn join_then_leave_restores_the_world_at_join_and_seeds_the_character_at_join() {
    let dir = tempfile::tempdir().unwrap();
    let relay = host_in_level(&spawn_points(3));
    let mut view = single_player_at_menu(spawn_points(3));
    with_saves(&mut view, dir.path());
    write_world(&view.app, 42.0);
    write_character(&view.app, 7.0);

    assert!(view.app.join_from_frontend(relay.address()));
    assert_eq!(
        character_in(view.app.session.as_ref().unwrap().persisted_state.as_ref()),
        7.0
    );
    // The host's world replaces the client's while it plays.
    write_world(&view.app, 99.0);
    assert!(view.app.leave_session(LeaveCause::Own));

    assert_eq!(slot(&view.app, WORLD_SLOT), Some(SlotValue::Number(42.0)));
}

// AC S3: a join whose save cannot be written still rebases, and warns.
#[test]
fn failed_join_save_still_rebases_the_restore_snapshot_and_warns() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = host_in_level(&spawn_points(3));
    let mut view = single_player_at_menu(spawn_points(3));
    with_saves(&mut view, dir.path());
    // The mod's save directory is a file: the write fails.
    let blocked = saved_file(&view.app).parent().unwrap().to_path_buf();
    std::fs::remove_dir_all(&blocked).unwrap();
    std::fs::write(&blocked, b"not a directory").unwrap();
    write_world(&view.app, 42.0);

    assert!(view.app.join_from_frontend(relay.address()));
    capture.assert_logged(log::Level::Warn, "failed to save persistent slots");
    write_world(&view.app, 99.0);
    assert!(view.app.leave_session(LeaveCause::Own));
    assert_eq!(slot(&view.app, WORLD_SLOT), Some(SlotValue::Number(42.0)));
}

// AC S4: carried loadout from earlier single-player levels does not survive
// a join and leave; seat 0 holds what a fresh ledger holds.
#[test]
fn join_then_leave_leaves_seat_zero_with_a_fresh_ledgers_carry() {
    let relay = host_in_level(&spawn_points(3));
    let mut view = single_player_at_menu(spawn_points(3));
    {
        let session = view.app.session.as_mut().unwrap();
        let ctx = session.scripting.script_ctx.clone();
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(postretro_entities::Transform::default());
        let mut health = postretro_entities::components::health::HealthComponent::from_descriptor(
            &postretro_entities::data_descriptors::HealthDescriptor {
                max: 100.0,
                ..Default::default()
            },
        );
        health.current = 40.0;
        registry.set_component(pawn, health).unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        seats.bind_pawn(&mut registry, Seat(0), pawn);
        seats.harvest_bound_pawns(&registry, &[]);
        assert!(
            seats.carried_state(Seat(0)).is_some(),
            "fixture: seat 0 carries"
        );
    }

    assert!(view.app.join_from_frontend(relay.address()));
    assert!(view.app.leave_session(LeaveCause::Own));

    let fresh = crate::netcode::SeatTable::local_only();
    let session = view.app.session.as_ref().unwrap();
    assert_eq!(
        session.seat_table.as_ref().unwrap().carried_state(Seat(0)),
        fresh.carried_state(Seat(0))
    );
}

use super::session_entry::{ConnectionPhase, take_published_connections};
use postretro_entities::{FireOrigin, MachineSource, SystemReactionCommand};

fn own() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}

fn host_verb() -> SystemReactionCommand {
    host_from("named:menu.host")
}

fn join_verb() -> SystemReactionCommand {
    join_from("named:menu.join")
}

/// A press from its own firing source: refusals warn once per source.
fn host_from(source: &str) -> SystemReactionCommand {
    SystemReactionCommand::HostSession {
        dispatch_source: source.to_string(),
        origin: own(),
    }
}

fn join_from(source: &str) -> SystemReactionCommand {
    SystemReactionCommand::JoinSession {
        dispatch_source: source.to_string(),
        origin: own(),
    }
}

fn leave_verb() -> SystemReactionCommand {
    SystemReactionCommand::LeaveSession {
        dispatch_source: "named:menu.leave".to_string(),
        origin: own(),
    }
}

/// One reaction's steps, drained as the frame drains them.
fn run(app: &mut App, commands: impl IntoIterator<Item = SystemReactionCommand>) {
    let ctx = script_ctx_of(app);
    for command in commands {
        ctx.system_commands.push(command);
    }
    app.dispatch_system_commands();
}

fn write_text(app: &mut App, slot: &str, text: &str) {
    app.write_session_slot(slot, SlotValue::String(text.to_string()));
}

fn connection(app: &App) -> Option<SlotValue> {
    slot(app, "session.connection")
}

fn connection_is(app: &App, phase: &str) -> bool {
    connection(app) == Some(SlotValue::Enum(phase.to_string()))
}

/// A single-player engine at its menu that hosts in-app on an OS-chosen
/// port, so parallel tests never collide.
fn entering_engine(points: Vec<MapEntity>) -> ClientView {
    let mut view = single_player_at_menu(points);
    view.app.in_app_host_port = 0;
    view
}

/// What a refused press must leave as it was.
#[derive(Debug, PartialEq)]
struct EntryFacts {
    role: SessionRole,
    session_id: Option<SessionId>,
    host_password: Option<SlotValue>,
    join_password: Option<SlotValue>,
    end_cause: Option<SlotValue>,
    connection: Option<SlotValue>,
    saved: Option<Vec<u8>>,
}

fn entry_facts(app: &App) -> EntryFacts {
    let session = app.session.as_ref().unwrap();
    EntryFacts {
        role: app.session_role(),
        session_id: session
            .seat_table
            .as_ref()
            .map(|seats| seats.session_id_for_test()),
        host_password: slot(app, "session.hostPassword"),
        join_password: slot(app, "session.joinPassword"),
        end_cause: slot(app, "session.endCause"),
        connection: connection(app),
        saved: session.data_dir.as_deref().and_then(|dir| {
            std::fs::read(
                postretro_sim::scripting::save_files::SaveDir::new(dir, MOD_ID).world_path(),
            )
            .ok()
        }),
    }
}

/// A single-player engine with saves, a password, a stale end cause, an
/// address to dial, and unsaved world progress a join would write.
fn primed_engine(points: Vec<MapEntity>, dir: &std::path::Path, address: &str) -> ClientView {
    let mut view = entering_engine(points);
    with_saves(&mut view, dir);
    write_world(&view.app, 42.0);
    write_text(&mut view.app, "session.hostPassword", "hunter2");
    write_text(&mut view.app, "session.joinPassword", "hunter3");
    write_text(&mut view.app, "session.joinAddress", address);
    view.app
        .publish_end_cause(Some(super::session_entry::EndCause::ConnectFailed));
    view
}

#[track_caller]
fn assert_refused_without_change(view: &mut ClientView, verb: SystemReactionCommand, case: &str) {
    let capture = postretro_test_log_capture::LogCapture::start();
    let before = entry_facts(&view.app);
    let endpoint_before = view.app.session.as_ref().unwrap().net_endpoint.is_some();
    let waiting_before = format!("{:?}", view.app.pending_entry);
    run(&mut view.app, [verb]);
    assert_eq!(entry_facts(&view.app), before, "{case}: nothing changes");
    assert_eq!(
        view.app.session.as_ref().unwrap().net_endpoint.is_some(),
        endpoint_before,
        "{case}: no endpoint"
    );
    assert_eq!(
        format!("{:?}", view.app.pending_entry),
        waiting_before,
        "{case}: no press starts waiting, and a waiting one stays"
    );
    capture.assert_logged(log::Level::Warn, "ignored");
}

// AC E6 (P4): entry is refused with a warning, changing nothing, with a
// played level installed, a played load queued or in flight, on a host and
// on a client; each refused case has a permitted neighbour.
#[test]
fn entry_off_the_frontend_or_outside_single_player_is_refused_without_change() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let address = relay.address().to_string();

    // A played level installed, with no menu and then under the frontend
    // menu before its unload is queued.
    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &address);
    view.app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    view.app
        .enqueue_level_request(LevelRequest::Load(crate::startup::LevelSource::Catalog(
            LEVEL.to_string(),
        )));
    view.install();
    assert!(
        view.app
            .session
            .as_ref()
            .unwrap()
            .session_players
            .session_level()
    );
    assert_refused_without_change(
        &mut view,
        host_from("named:played-level-host"),
        "played level, host",
    );
    assert_refused_without_change(
        &mut view,
        join_from("named:played-level-join"),
        "played level, join",
    );
    view.app.present_frontend_menu();
    assert_refused_without_change(
        &mut view,
        host_from("named:played-level-under-the-menu"),
        "played level under the menu",
    );

    // Its neighbour: a backdrop installed under the frontend menu hosts.
    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &address);
    view.app.populate_frontend();
    view.install();
    run(&mut view.app, [host_verb()]);
    assert_eq!(
        view.app.session_role(),
        SessionRole::Host,
        "backdrop installed: hosts"
    );

    // On a host and on a client, the second press changes nothing.
    assert_refused_without_change(
        &mut view,
        host_from("named:already-hosting"),
        "already hosting",
    );
    assert_refused_without_change(&mut view, join_from("named:hosting-join"), "hosting, join");
    let dir = tempfile::tempdir().unwrap();
    let mut client = primed_engine(points.clone(), dir.path(), &address);
    run(&mut client.app, [join_verb()]);
    assert_eq!(client.app.session_role(), SessionRole::Client);
    assert_refused_without_change(
        &mut client,
        join_from("named:already-a-client"),
        "already a client",
    );
    assert_refused_without_change(&mut client, host_from("named:client-host"), "client, host");

    // A played load queued, then in flight: `loadLevel` clears the menu.
    // Its neighbour, a backdrop load in flight under the menu, joins.
    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &address);
    view.app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    view.app
        .enqueue_level_request(LevelRequest::Load(crate::startup::LevelSource::Catalog(
            LEVEL.to_string(),
        )));
    assert_refused_without_change(
        &mut view,
        join_from("named:played-load-queued"),
        "played load queued",
    );
    view.app.drain_level_requests();
    assert!(view.app.level_load_in_flight());
    assert_refused_without_change(
        &mut view,
        host_from("named:played-load-in-flight"),
        "played load in flight",
    );

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points, dir.path(), &address);
    view.app.populate_frontend();
    view.app.drain_level_requests();
    assert!(view.app.level_load_in_flight(), "fixture: backdrop loading");
    run(&mut view.app, [join_verb()]);
    assert_eq!(
        view.app.session_role(),
        SessionRole::Client,
        "backdrop loading: joins"
    );
}

/// A client of `relay` in its level, whose next press joins `rejoin`.
fn client_in_level(relay: &mut Relay, points: &[MapEntity], rejoin: &str) -> ClientView {
    let mut view = entering_engine(points.to_vec());
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    let _ = follow_into_level(relay, &mut view);
    write_text(&mut view.app, "session.joinAddress", rejoin);
    view
}

// AC E7 (P5) and E10 (P2): a join after a leave in one reaction waits for
// the left level's unload, publishing `connecting`, then joins; a second
// press meanwhile is refused; any other step after the leave still drops at
// debug.
#[test]
fn join_after_a_leave_waits_for_the_unload_then_joins() {
    let points = spawn_points(3);
    let mut first = host_in_level(&points);
    let mut second = host_in_level(&points);
    let mut view = client_in_level(&mut first, &points, &second.address().to_string());
    let _ = take_published_connections();

    let capture = postretro_test_log_capture::LogCapture::start();
    run(
        &mut view.app,
        [leave_verb(), SystemReactionCommand::PopTree, join_verb()],
    );
    assert_eq!(
        view.app.session_role(),
        SessionRole::SinglePlayer,
        "it left"
    );
    assert!(view.app.pending_entry.is_some(), "the join waits");
    assert!(connection_is(&view.app, "connecting"));
    capture.assert_logged(log::Level::Debug, "PopTree dropped");
    drop(capture);
    assert_refused_without_change(
        &mut view,
        host_from("named:a-press-while-one-waits"),
        "a press while one waits",
    );

    view.app.drain_level_requests();
    view.app.resume_pending_entry();
    assert_eq!(
        view.app.session_role(),
        SessionRole::Client,
        "joined after the unload"
    );
    assert!(view.app.pending_entry.is_none());
    let _ = follow_into_level(&mut second, &mut view);
}

// AC E7 (P5): a host press after its own `endSession` waits for the close,
// then hosts.
#[test]
fn host_after_an_end_waits_for_the_close_then_hosts() {
    let mut view = entering_engine(spawn_points(3));
    run(&mut view.app, [host_verb()]);
    let first_run = session_id(&view.app);
    let _ = take_published_connections();
    let ended = SystemReactionCommand::EndSession {
        dispatch_source: "named:menu.end".to_string(),
        origin: own(),
    };
    // A client keeps the close open, so the press must wait for it.
    let mut relay = Relay::adopt(view.app);
    relay.admit(&[61]);
    run(&mut relay.app, [ended, host_verb()]);
    assert!(
        relay.app.pending_entry.is_some(),
        "the host press waits for the close"
    );
    assert!(connection_is(&relay.app, "connecting"));
    relay.app.resume_pending_entry();
    assert!(
        relay.app.pending_entry.is_some(),
        "still closing: it still waits"
    );

    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    close_out(&mut relay.app);
    relay.app.resume_pending_entry();
    assert_eq!(relay.app.session_role(), SessionRole::Host);
    assert_ne!(session_id(&relay.app), first_run);
    assert_eq!(
        take_published_connections(),
        [ConnectionPhase::Connecting, ConnectionPhase::Offline],
        "P18: a deferred host steps offline, connecting, offline"
    );
}

// AC E9 (P1): two entry verbs in one reaction: the first takes effect, the
// second is refused, with one endpoint and at most one join save.
#[test]
fn two_entry_verbs_in_one_reaction_take_the_first_and_refuse_the_second() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let address = relay.address().to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &address);
    let saved_before = std::fs::read(saved_file(&view.app)).unwrap();
    let capture = postretro_test_log_capture::LogCapture::start();
    run(&mut view.app, [host_verb(), join_verb()]);
    assert_eq!(view.app.session_role(), SessionRole::Host);
    capture.assert_logged(
        log::Level::Warn,
        "joinSession from `named:menu.join` ignored",
    );
    assert_eq!(
        std::fs::read(saved_file(&view.app)).unwrap(),
        saved_before,
        "the refused join wrote no save"
    );
    drop(capture);

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points, dir.path(), &address);
    let capture = postretro_test_log_capture::LogCapture::start();
    run(&mut view.app, [join_verb(), join_verb()]);
    assert_eq!(view.app.session_role(), SessionRole::Client);
    capture.assert_logged_once(log::Level::Info, "saved persistent slots");
    capture.assert_logged(
        log::Level::Warn,
        "joinSession from `named:menu.join` ignored",
    );
}

// AC E11 (P16): a waiting press runs with what it captured; a malformed
// address never shows `connecting`; a later load or navigation cancels a
// waiting press and publishes `offline`.
#[test]
fn a_waiting_press_runs_with_what_it_captured_and_a_load_or_navigation_cancels_it() {
    let points = spawn_points(3);
    let mut first = host_in_level(&points);
    let mut second = host_in_level(&points);
    let mut view = client_in_level(&mut first, &points, &second.address().to_string());
    run(&mut view.app, [leave_verb(), join_verb()]);
    assert!(view.app.pending_entry.is_some());
    // Edits while it waits change nothing it runs with, nor what an
    // admission remembers.
    write_text(&mut view.app, "session.joinAddress", "not an address");
    view.app.drain_level_requests();
    view.app.resume_pending_entry();
    assert_eq!(view.app.session_role(), SessionRole::Client);
    let _ = follow_into_level(&mut second, &mut view);
    assert_eq!(
        view.app
            .session
            .as_ref()
            .unwrap()
            .player_options
            .last_join_address,
        Some(second.address().to_string()),
        "the admission remembers the address the press dialed"
    );

    // A malformed address ends the press at once, never `connecting`.
    let mut view = entering_engine(points.clone());
    write_text(&mut view.app, "session.joinPassword", "kept");
    write_text(&mut view.app, "session.joinAddress", "example.com");
    let _ = take_published_connections();
    run(&mut view.app, [join_verb()]);
    assert_eq!(take_published_connections(), []);
    assert_eq!(
        slot(&view.app, "session.endCause"),
        Some(SlotValue::Enum("badAddress".to_string()))
    );
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String("kept".to_string()))
    );
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);

    // A level load, then a navigation, cancel a waiting press.
    for cancel in ["load", "navigation"] {
        let mut first = host_in_level(&points);
        let rejoin = first.address().to_string();
        let mut view = client_in_level(&mut first, &points, &rejoin);
        run(&mut view.app, [leave_verb(), join_verb()]);
        assert!(view.app.pending_entry.is_some());
        match cancel {
            "load" => view.app.enqueue_level_request(LevelRequest::Load(
                crate::startup::LevelSource::Catalog(LEVEL.to_string()),
            )),
            _ => view.app.navigate_to_frontend(),
        }
        assert!(view.app.pending_entry.is_none(), "{cancel} cancels it");
        assert!(connection_is(&view.app, "offline"), "{cancel}: offline");
    }
}

use super::session_entry::EndCause;
use postretro_net::wire::{ClosingCause, DivergenceReason, HoldingCause, ServerControlMessage};

fn end_cause(app: &App) -> String {
    match slot(app, "session.endCause") {
        Some(SlotValue::Enum(cause)) => cause,
        other => panic!("endCause is an enum: {other:?}"),
    }
}

fn wait_reason_of(app: &App) -> String {
    match slot(app, "session.waitReason") {
        Some(SlotValue::Enum(reason)) => reason,
        other => panic!("waitReason is an enum: {other:?}"),
    }
}

/// An engine that pressed Join toward `relay` and is dialing it.
fn joining(relay: &Relay, points: &[MapEntity]) -> ClientView {
    let mut view = entering_engine(points.to_vec());
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    assert_eq!(view.app.session_role(), SessionRole::Client);
    view
}

/// One client frame's Control drain, as its poll runs it.
fn drain(view: &mut ClientView, messages: Vec<ServerControlMessage>) {
    crate::client_drain_control(&mut view.app, messages);
}

fn closing(cause: ClosingCause) -> ServerControlMessage {
    ServerControlMessage::Divergence(DivergenceReason::Closing(cause))
}

fn holding(cause: HoldingCause) -> ServerControlMessage {
    ServerControlMessage::Divergence(DivergenceReason::Holding(cause))
}

// AC F1 (P11), F6 (P17): each ending publishes its own cause, and every
// ending before admission but an explicit refusal reads `connectFailed`.
#[test]
fn each_ending_publishes_its_own_end_cause() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let protocol = ClosingCause::Protocol {
        expected: postretro_net::handshake::protocol_version(),
        received: postretro_net::handshake::protocol_version(),
    };
    let mod_id = ClosingCause::ModId {
        expected: "a".into(),
        received: "b".into(),
        expected_version: "1".into(),
        received_version: "1".into(),
    };
    let admission = holding(HoldingCause::HostLevelAbsent);
    let cases: Vec<(&str, Vec<ServerControlMessage>, &str)> = vec![
        (
            "ended after admission",
            vec![admission.clone(), closing(ClosingCause::SessionEnded)],
            "sessionEnded",
        ),
        (
            "ended before admission",
            vec![closing(ClosingCause::SessionEnded)],
            "connectFailed",
        ),
        ("wrong mod", vec![closing(mod_id)], "refusedMod"),
        ("other build", vec![closing(protocol)], "connectFailed"),
    ];
    for (case, messages, expected) in cases {
        let mut view = joining(&relay, &points);
        assert_eq!(end_cause(&view.app), "", "{case}: the attempt cleared it");
        drain(&mut view, messages);
        assert_eq!(view.app.session_role(), SessionRole::SinglePlayer, "{case}");
        assert_eq!(end_cause(&view.app), expected, "{case}");
        assert!(connection_is(&view.app, "offline"), "{case}");
    }
    for (case, cause, admitted, expected) in [
        (
            "lost after admission",
            LeaveCause::LostHost,
            true,
            "hostLost",
        ),
        (
            "lost before admission",
            LeaveCause::LostHost,
            false,
            "connectFailed",
        ),
        (
            "never connected",
            LeaveCause::NeverConnected,
            false,
            "connectFailed",
        ),
    ] {
        let mut view = joining(&relay, &points);
        if admitted {
            drain(&mut view, vec![admission.clone()]);
        }
        assert!(view.app.leave_session(cause));
        assert_eq!(end_cause(&view.app), expected, "{case}");
    }
    // A client's own leave sets none, and the next attempt clears the last.
    let mut view = joining(&relay, &points);
    drain(&mut view, vec![closing(ClosingCause::SessionEnded)]);
    assert_eq!(end_cause(&view.app), "connectFailed");
    run(&mut view.app, [join_verb()]);
    assert_eq!(end_cause(&view.app), "", "the next attempt clears it");
    assert!(view.app.leave_session(LeaveCause::Own));
    assert_eq!(end_cause(&view.app), "", "an own leave sets none");
}

// AC F1: a client on another mod is refused over the real transport, and the
// cause that arrives with the disconnect still publishes.
#[test]
fn a_wrong_mod_join_publishes_refused_mod_over_the_transport() {
    let points = spawn_points(3);
    let mut relay = host_in_level(&points);
    let mut view = entering_engine(points);
    view.app
        .session
        .as_mut()
        .unwrap()
        .scripting
        .script_runtime
        .commit_mod_identity_for_test("another.mod", "1");
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    exchange_until(&mut relay, &mut view, |_, view| {
        view.app.session_role() == SessionRole::SinglePlayer
    });
    assert_eq!(end_cause(&view.app), "refusedMod");
}

// AC F1, F2 (P13): a malformed, IPv6 or hostname address opens no socket,
// writes no save, never leaves `offline` and keeps the password.
#[test]
fn a_bad_address_opens_no_socket_writes_no_save_and_keeps_the_password() {
    for address in ["1.2.3", "[::1]:27015", "::1", "host.example"] {
        let dir = tempfile::tempdir().unwrap();
        let mut view = primed_engine(spawn_points(3), dir.path(), address);
        let saved = std::fs::read(saved_file(&view.app)).unwrap();
        let _ = take_published_connections();
        run(&mut view.app, [join_verb()]);
        assert!(
            view.app.session.as_ref().unwrap().net_endpoint.is_none(),
            "{address}"
        );
        assert_eq!(
            std::fs::read(saved_file(&view.app)).unwrap(),
            saved,
            "{address}"
        );
        assert_eq!(take_published_connections(), [], "{address}");
        assert_eq!(end_cause(&view.app), "badAddress", "{address}");
        assert_eq!(
            slot(&view.app, "session.joinPassword"),
            Some(SlotValue::String("hunter3".to_string())),
            "{address}"
        );
    }
}

// AC F3 (P14): a host's own `endSession` sets no end cause; a failed boot
// `--host` sets `hostFailed`, and a malformed `--connect` sets `badAddress`.
#[test]
fn boot_and_own_endings_set_the_causes_entry_sets() {
    let mut view = entering_engine(spawn_points(3));
    run(&mut view.app, [host_verb()]);
    view.app.end_session();
    assert_eq!(end_cause(&view.app), "");

    let argv = |args: &[&str]| -> Vec<String> {
        std::iter::once("postretro")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect()
    };
    for (args, expected) in [
        (argv(&["--connect", "not-an-address"]), "badAddress"),
        (argv(&["--connect", "[::1]:27015"]), "badAddress"),
        (argv(&["--host", "0"]), "hostFailed"),
        // A bad `--password` is not a bad address.
        (argv(&["--connect", "10.0.0.2", "--password"]), ""),
    ] {
        let app = test_app();
        let ctx = script_ctx_of(&app);
        let parsed = crate::netcode::parse_net_config(&args);
        let entry = crate::session::net_entry::RoleEntry {
            endpoint: None,
            failure: None,
            released: Vec::new(),
        };
        crate::session::net_entry::publish_boot_facts(
            &ctx,
            parsed.as_ref().err(),
            &NetRole::SinglePlayer,
            &entry,
            None,
            None,
        );
        assert_eq!(end_cause(&app), expected, "{args:?}");
    }
    // A `--host` whose bind failed.
    let app = test_app();
    let entry = crate::session::net_entry::RoleEntry {
        endpoint: None,
        failure: Some(crate::session::net_entry::EntryFailure::Endpoint),
        released: Vec::new(),
    };
    crate::session::net_entry::publish_boot_facts(
        &script_ctx_of(&app),
        None,
        &NetRole::Host { port: 27015 },
        &entry,
        None,
        None,
    );
    assert_eq!(end_cause(&app), "hostFailed");
}

// AC F4 (P23): every published `session.connection` value through an
// in-app host's frontend, its first level, a level change and a leave.
#[test]
fn connection_steps_through_waiting_and_playing_and_back_to_offline() {
    let points = spawn_points(3);
    let mut host = entering_engine(points.clone());
    assert!(host.app.host_from_frontend(0));
    let mut relay = Relay::adopt(host.app);
    let mut view = entering_engine(points.clone());
    let _ = take_published_connections();
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    exchange_until(&mut relay, &mut view, |_, view| {
        connection_is(&view.app, "waiting")
    });
    assert_eq!(
        wait_reason_of(&view.app),
        "host",
        "the host is at its frontend"
    );

    relay
        .app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    relay.world_less = false;
    relay.install(points.clone());
    relay.host_reveal();
    let _ = follow_into_level(&mut relay, &mut view);
    exchange_until(&mut relay, &mut view, |_, view| {
        connection_is(&view.app, "playing")
    });
    assert_eq!(wait_reason_of(&view.app), "");

    // The host restarts its level: the client waits, follows and plays.
    relay.unload();
    relay.install(points.clone());
    relay.host_reveal();
    exchange_until(&mut relay, &mut view, |_, view| {
        connection_is(&view.app, "waiting")
    });
    let _ = follow_into_level(&mut relay, &mut view);
    exchange_until(&mut relay, &mut view, |_, view| {
        connection_is(&view.app, "playing")
    });

    // A hold and its re-promotion reach the client in one update: both
    // values publish, in order.
    relay.server().set_mod_digest(Some([0xEE; 32]));
    for _ in 0..4 {
        relay.step();
    }
    relay.restore_mod_digest();
    for _ in 0..4 {
        relay.step();
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    view.app.net_poll_and_apply(FRAME_SECONDS);
    assert!(connection_is(&view.app, "playing"));

    assert!(view.app.leave_session(LeaveCause::Own));
    use ConnectionPhase::*;
    assert_eq!(
        take_published_connections(),
        [
            Connecting, Waiting, Playing, Waiting, Playing, Waiting, Playing, Offline
        ]
    );
}

// AC F4 (P23): a refused attempt never shows `waiting`; an admission and a
// session-ended cause in one update show it once, then `offline`.
#[test]
fn a_refused_attempt_never_waits_and_an_admission_ended_in_one_update_waits_once() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let mut view = joining(&relay, &points);
    let _ = take_published_connections();
    drain(
        &mut view,
        vec![closing(ClosingCause::ModId {
            expected: "a".into(),
            received: "b".into(),
            expected_version: "1".into(),
            received_version: "1".into(),
        })],
    );
    assert_eq!(take_published_connections(), [ConnectionPhase::Offline]);

    let mut view = joining(&relay, &points);
    let _ = take_published_connections();
    drain(
        &mut view,
        vec![
            holding(HoldingCause::HostLevelAbsent),
            closing(ClosingCause::SessionEnded),
        ],
    );
    assert_eq!(
        take_published_connections(),
        [ConnectionPhase::Waiting, ConnectionPhase::Offline]
    );
    assert_eq!(end_cause(&view.app), "sessionEnded");
}

// AC F5 (P18): an undeferred host press never leaves `offline`, and a
// deferred one whose build fails ends `offline` with `hostFailed`.
#[test]
fn host_presses_publish_offline_and_a_failed_build_publishes_host_failed() {
    let mut view = entering_engine(spawn_points(3));
    let _ = take_published_connections();
    run(&mut view.app, [host_verb()]);
    assert_eq!(view.app.session_role(), SessionRole::Host);
    assert_eq!(take_published_connections(), []);

    // A port another socket holds: the build fails after the press waited.
    let busy = std::net::UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    let mut view = entering_engine(spawn_points(3));
    run(&mut view.app, [host_verb()]);
    let mut relay = Relay::adopt(view.app);
    relay.admit(&[71]);
    relay.app.in_app_host_port = busy.local_addr().unwrap().port();
    let ended = SystemReactionCommand::EndSession {
        dispatch_source: "named:menu.end".to_string(),
        origin: own(),
    };
    let _ = take_published_connections();
    run(&mut relay.app, [ended, host_verb()]);
    for _ in 0..16 {
        if !relay.step_closing() {
            break;
        }
    }
    close_out(&mut relay.app);
    relay.app.resume_pending_entry();
    assert_eq!(relay.app.session_role(), SessionRole::SinglePlayer);
    assert_eq!(end_cause(&relay.app), "hostFailed");
    assert_eq!(
        take_published_connections(),
        [ConnectionPhase::Connecting, ConnectionPhase::Offline]
    );
}

// AC F7 (P19): the wait reason for each hold, and empty while playing or
// offline.
#[test]
fn wait_reason_names_each_hold() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let digest = [1; 32];
    for (hold, expected) in [
        (HoldingCause::HostLevelAbsent, "host"),
        (
            HoldingCause::HostNotRevealed {
                identity: "x".into(),
            },
            "host",
        ),
        (
            HoldingCause::ClientNotRevealed {
                identity: "x".into(),
            },
            "host",
        ),
        (
            HoldingCause::LevelAbsent {
                expected_identity: "x".into(),
            },
            "host",
        ),
        (
            HoldingCause::ModDigest {
                expected: digest,
                received: [2; 32],
            },
            "modDiffers",
        ),
        (
            HoldingCause::LevelIdentity {
                expected: "a".into(),
                received: "b".into(),
            },
            "levelDiffers",
        ),
        (
            HoldingCause::LevelDigest {
                identity: "a".into(),
                expected: digest,
                received: [2; 32],
            },
            "levelDiffers",
        ),
    ] {
        let mut view = joining(&relay, &points);
        drain(&mut view, vec![holding(hold.clone())]);
        assert_eq!(wait_reason_of(&view.app), expected, "{hold:?}");
    }
    // A host level this client's catalog lacks.
    let mut view = joining(&relay, &points);
    drain(
        &mut view,
        vec![ServerControlMessage::Relevel {
            catalog_id: "not-in-this-catalog".into(),
            level_life: 1,
        }],
    );
    assert_eq!(wait_reason_of(&view.app), "levelDiffers");
    // The host unloads to load another level: it lacks nothing of ours.
    drain(&mut view, vec![holding(HoldingCause::HostLevelAbsent)]);
    assert_eq!(wait_reason_of(&view.app), "host");
    assert!(view.app.leave_session(LeaveCause::Own));
    assert_eq!(wait_reason_of(&view.app), "", "offline");

    // A host map change: the client follows the relevel, and the mismatch
    // of its old level against the new one reads as the host's move.
    let mut view = joining(&relay, &points);
    drain(
        &mut view,
        vec![
            ServerControlMessage::Relevel {
                catalog_id: LEVEL.to_string(),
                level_life: 9,
            },
            holding(HoldingCause::LevelIdentity {
                expected: "next".into(),
                received: "previous".into(),
            }),
        ],
    );
    assert_eq!(wait_reason_of(&view.app), "host");

    // A mod mismatch outlives a host map change: the host does not resend an
    // unchanged cause, so the relevel keeps it.
    let mut view = joining(&relay, &points);
    drain(
        &mut view,
        vec![
            holding(HoldingCause::ModDigest {
                expected: digest,
                received: [2; 32],
            }),
            ServerControlMessage::Relevel {
                catalog_id: LEVEL.to_string(),
                level_life: 9,
            },
        ],
    );
    assert_eq!(wait_reason_of(&view.app), "modDiffers");
}

// AC F8 (P20, as corrected): the host password clears when hosting starts
// and stays on `hostFailed`; the join password clears only on admission and
// stays through `connectFailed`, `refusedPassword` and `badAddress`. Each
// verb reads only its own slot.
#[test]
fn the_password_clears_when_consumed_and_stays_for_a_retry() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let host_password = |app: &App| slot(app, "session.hostPassword");
    let join_password = |app: &App| slot(app, "session.joinPassword");
    let host_set = Some(SlotValue::String("hunter2".to_string()));
    let join_set = Some(SlotValue::String("hunter3".to_string()));
    let empty = Some(SlotValue::String(String::new()));

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &relay.address().to_string());
    run(&mut view.app, [host_verb()]);
    assert_eq!(host_password(&view.app), empty, "hosting consumes it");
    assert_eq!(
        join_password(&view.app),
        join_set,
        "hosting leaves the join's"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &relay.address().to_string());
    run(&mut view.app, [join_verb()]);
    assert_eq!(join_password(&view.app), join_set, "dialing keeps it");
    drain(&mut view, vec![holding(HoldingCause::HostLevelAbsent)]);
    assert_eq!(join_password(&view.app), empty, "admission consumes it");
    assert_eq!(
        host_password(&view.app),
        host_set,
        "joining leaves the host's"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &relay.address().to_string());
    run(&mut view.app, [join_verb()]);
    assert!(view.app.leave_session(LeaveCause::NeverConnected));
    assert_eq!(join_password(&view.app), join_set, "connectFailed keeps it");
    run(&mut view.app, [join_verb()]);
    assert!(
        view.app
            .leave_session(LeaveCause::Refused(EndCause::RefusedPassword))
    );
    assert_eq!(
        join_password(&view.app),
        join_set,
        "refusedPassword keeps it for the player to correct"
    );

    let busy = std::net::UdpSocket::bind(("0.0.0.0", 0)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points, dir.path(), "1.2.3");
    run(&mut view.app, [join_verb()]);
    assert_eq!(join_password(&view.app), join_set, "badAddress keeps it");
    view.app.in_app_host_port = busy.local_addr().unwrap().port();
    run(&mut view.app, [host_from("named:busy-host")]);
    assert_eq!(end_cause(&view.app), "hostFailed");
    assert_eq!(host_password(&view.app), host_set, "hostFailed keeps it");
}

// AC F9 (P12): the address is remembered once a join is admitted, never on
// a refusal, failed connect or bad address, and seeds the next boot.
#[test]
fn the_join_address_is_remembered_on_admission_and_seeds_the_next_boot() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.toml");
    let remembered = |app: &App| {
        app.session
            .as_ref()
            .unwrap()
            .player_options
            .last_join_address
            .clone()
    };
    let address = relay.address().to_string();
    let engine = || {
        let mut view = entering_engine(points.clone());
        view.app.session.as_mut().unwrap().settings_path = Some(settings.clone());
        write_text(&mut view.app, "session.joinAddress", &address);
        view
    };

    for (case, cause) in [
        ("refused", LeaveCause::Refused(EndCause::RefusedMod)),
        ("failed connect", LeaveCause::NeverConnected),
    ] {
        let mut view = engine();
        run(&mut view.app, [join_verb()]);
        assert!(view.app.leave_session(cause));
        assert_eq!(remembered(&view.app), None, "{case}");
    }
    let mut view = engine();
    write_text(&mut view.app, "session.joinAddress", "1.2.3");
    run(&mut view.app, [join_verb()]);
    assert_eq!(remembered(&view.app), None, "bad address");
    assert!(!settings.exists());

    let mut view = engine();
    run(&mut view.app, [join_verb()]);
    drain(&mut view, vec![holding(HoldingCause::HostLevelAbsent)]);
    assert_eq!(remembered(&view.app), Some(address.clone()));

    // The next boot reads the file and seeds the slot.
    let restarted = crate::options::PlayerOptions::load(&settings);
    assert_eq!(
        restarted.last_join_address.as_deref(),
        Some(address.as_str())
    );
    let app = test_app();
    crate::session::net_entry::publish_boot_facts(
        &script_ctx_of(&app),
        None,
        &NetRole::SinglePlayer,
        &crate::session::net_entry::RoleEntry {
            endpoint: None,
            failure: None,
            released: Vec::new(),
        },
        restarted.last_join_address.as_deref(),
        None,
    );
    assert_eq!(
        slot(&app, "session.joinAddress"),
        Some(SlotValue::String(address))
    );
}

/// An engine that pressed Join toward `relay` answering with `password`.
fn joining_with(relay: &Relay, points: &[MapEntity], password: &str) -> ClientView {
    let mut view = entering_engine(points.to_vec());
    write_text(&mut view.app, "session.joinPassword", password);
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    assert_eq!(view.app.session_role(), SessionRole::Client);
    view
}

/// Exchange until the joiner is admitted (`waiting` or `playing`) or refused
/// (back to single-player). Returns whether it was admitted.
fn admitted_or_refused(relay: &mut Relay, view: &mut ClientView) -> bool {
    exchange_until(relay, view, |_, view| {
        view.app.session_role() == SessionRole::SinglePlayer
            || connection_is(&view.app, "waiting")
            || connection_is(&view.app, "playing")
    });
    view.app.session_role() == SessionRole::Client
}

// AC P-a, P-b (P6), P-f: a password host admits the right password into its
// level, the parity, join seed and reveal the client sent before its proof
// included; a wrong password or none refuses with `refusedPassword` and lands
// the client at its frontend.
#[test]
fn a_password_host_admits_the_right_password_and_refuses_a_wrong_or_missing_one() {
    let points = spawn_points(3);
    let mut relay = host_in_level(&points);
    relay.server().set_session_password("p");

    let mut view = joining_with(&relay, &points, "p");
    let (id, hint) = follow_into_level(&mut relay, &mut view);
    relay.assert_pawn_at_hint(relay.pawn(id).unwrap(), &hint);

    for (case, password) in [("wrong", "q"), ("missing", "")] {
        let mut view = joining_with(&relay, &points, password);
        assert!(!admitted_or_refused(&mut relay, &mut view), "{case}");
        assert_eq!(end_cause(&view.app), "refusedPassword", "{case}");
        assert!(
            view.app.frontend_menu_is_present(),
            "{case}: at its frontend"
        );
        assert_eq!(
            slot(&view.app, "session.joinPassword"),
            Some(SlotValue::String(password.to_string())),
            "{case}: the refusal keeps it to correct"
        );
    }
}

// AC P-e (P10): writing `session.hostPassword` on a live host changes which
// proof admits only at its next hosting.
#[test]
fn a_live_password_write_takes_effect_at_the_next_hosting() {
    let points = spawn_points(3);
    let mut host = entering_engine(points.clone());
    write_text(&mut host.app, "session.hostPassword", "p");
    run(&mut host.app, [host_verb()]);
    write_text(&mut host.app, "session.hostPassword", "q");
    let mut relay = Relay::adopt(host.app);

    let mut late = joining_with(&relay, &points, "q");
    assert!(
        !admitted_or_refused(&mut relay, &mut late),
        "the run keeps `p`"
    );
    let mut first = joining_with(&relay, &points, "p");
    assert!(admitted_or_refused(&mut relay, &mut first));

    relay.app.end_session();
    close_out(&mut relay.app);
    run(&mut relay.app, [host_from("named:host-again")]);
    let mut relay = Relay::adopt(relay.app);
    let mut now = joining_with(&relay, &points, "q");
    assert!(
        admitted_or_refused(&mut relay, &mut now),
        "the next run takes `q`"
    );
}

// AC P-m: `--host --password p` admits `p` and refuses `q`; `--connect
// --password p` is admitted by a host holding `p`. Boot hands the flag to
// the role builder as in-app entry hands it the slot's password.
#[test]
fn the_password_flag_serves_host_and_connect() {
    let points = spawn_points(3);
    let mut host = entering_engine(points.clone());
    {
        let session = host.app.session.as_mut().unwrap();
        let role = NetRole::Host { port: 0 };
        let endpoint = enter_role(
            &role,
            None,
            "",
            &mut session.scripting,
            &mut session.seat_table,
            getrandom::fill,
            crate::netcode::probe_lan_address,
            "p",
        )
        .endpoint;
        session.net_endpoint = endpoint;
    }
    host.app.install_network_mod_content();
    let mut relay = Relay::adopt(host.app);
    let mut right = joining_with(&relay, &points, "p");
    assert!(admitted_or_refused(&mut relay, &mut right));
    let mut wrong = joining_with(&relay, &points, "q");
    assert!(!admitted_or_refused(&mut relay, &mut wrong));

    let mut relay = host_in_level(&points);
    relay.server().set_session_password("p");
    let mut client = entering_engine(points);
    {
        let session = client.app.session.as_mut().unwrap();
        let role = NetRole::Connect {
            addr: relay.address(),
        };
        let endpoint = enter_role(
            &role,
            None,
            "",
            &mut session.scripting,
            &mut session.seat_table,
            getrandom::fill,
            crate::netcode::probe_lan_address,
            "p",
        )
        .endpoint;
        session.net_endpoint = endpoint;
    }
    client.app.install_network_mod_content();
    assert!(admitted_or_refused(&mut relay, &mut client));
}

/// Publish the boot facts a `--password` boot in `net_role` publishes,
/// against `entry`.
fn boot_with_password(app: &App, net_role: &NetRole, entry: &crate::session::net_entry::RoleEntry) {
    crate::session::net_entry::publish_boot_facts(
        &script_ctx_of(app),
        None,
        net_role,
        entry,
        None,
        Some("p"),
    );
}

fn no_endpoint() -> crate::session::net_entry::RoleEntry {
    crate::session::net_entry::RoleEntry {
        endpoint: None,
        failure: None,
        released: Vec::new(),
    }
}

// A `--password` no boot endpoint consumed serves the in-app verbs: with no
// role flag, a menu join answers with it and a menu host challenges for it.
// A `--connect` boot seeds only the join's.
#[test]
fn the_password_flag_serves_in_app_host_and_join() {
    let points = spawn_points(3);
    let mut relay = host_in_level(&points);
    relay.server().set_session_password("p");
    let mut view = entering_engine(points.clone());
    boot_with_password(&view.app, &NetRole::SinglePlayer, &no_endpoint());
    write_text(
        &mut view.app,
        "session.joinAddress",
        &relay.address().to_string(),
    );
    run(&mut view.app, [join_verb()]);
    assert!(
        admitted_or_refused(&mut relay, &mut view),
        "the join answers `p`"
    );

    let mut host = entering_engine(points.clone());
    boot_with_password(&host.app, &NetRole::SinglePlayer, &no_endpoint());
    run(&mut host.app, [host_verb()]);
    assert_eq!(
        slot(&host.app, "session.passwordProtected"),
        Some(SlotValue::Boolean(true))
    );
    let mut relay = Relay::adopt(host.app);
    let mut wrong = joining_with(&relay, &points, "");
    assert!(
        !admitted_or_refused(&mut relay, &mut wrong),
        "the host wants `p`"
    );

    let view = entering_engine(points);
    boot_with_password(
        &view.app,
        &NetRole::Connect {
            addr: "127.0.0.1:27015".parse().unwrap(),
        },
        &no_endpoint(),
    );
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String("p".to_string()))
    );
    assert_eq!(
        slot(&view.app, "session.hostPassword"),
        Some(SlotValue::String(String::new())),
        "the password a player joins with is never the one they host with"
    );
}

// AC P-i (P21): on a password host, a returning player whose proof lands on
// its hold's deadline poll reclaims its seat; one whose hold expired first
// takes a fresh seat.
#[test]
fn a_password_proof_on_the_hold_deadline_reclaims_and_one_after_it_does_not() {
    /// A password host holding seat 1 for player 81, and 82 dialing back in
    /// as that player. Returns the relay and the held seat.
    fn returning() -> (Relay, Seat) {
        let mut relay = Relay::host();
        relay.server().set_session_password("p");
        relay.connect_with_password(81, 81, "p");
        relay.pump_until(|relay| relay.seat(81).is_some());
        let held = relay.seat(81).unwrap();
        relay.disconnect(81);
        relay.connect_with_password(82, 81, "p");
        (relay, held)
    }
    // The relay is deterministic: count the polls the challenge round takes.
    let (mut dry, _) = returning();
    let mut polls = 0;
    while dry.seat(82).is_none() {
        dry.step();
        polls += 1;
        assert!(polls < 64, "the dry run admits");
    }

    for (case, past_deadline) in [("on the deadline", false), ("after it", true)] {
        let (mut relay, held) = returning();
        for _ in 0..polls - 1 {
            relay.step();
        }
        assert_eq!(relay.seat(82), None, "{case}: nothing is admitted unproven");
        let extra = if past_deadline {
            std::time::Duration::from_millis(1)
        } else {
            std::time::Duration::ZERO
        };
        // The hold began at clock zero: this poll is its deadline's, or past.
        relay
            .seats_mut()
            .advance_hold_clock(crate::netcode::HOLD_WINDOW + extra);
        relay.step();
        if past_deadline {
            assert_eq!(relay.seat(82), Some(Seat(2)), "{case}: a fresh seat");
        } else {
            assert_eq!(relay.seat(82), Some(held), "{case}: reclaimed");
        }
    }
}

// AC P-k: no datagram, save file, settings file, slot or captured log line
// holds the password, no uncaptured record does either, and
// `session.joinPassword` holds it only until admission consumes it.
#[test]
fn the_password_never_leaves_the_slot_that_held_it() {
    const PROBE: &str = "zq-probe-7731-never-in-a-fixture";
    let capture = postretro_test_log_capture::LogCapture::start();
    let orphans = postretro_test_log_capture::OrphanWatch::start(PROBE);
    let points = spawn_points(3);
    let mut relay = host_in_level(&points);
    relay.server().set_session_password(PROBE);
    relay.record_packets();
    relay.connect_with_password(91, 91, PROBE);
    relay.pump_until(|relay| relay.seat(91).is_some());

    let dir = tempfile::tempdir().unwrap();
    let mut view = primed_engine(points.clone(), dir.path(), &relay.address().to_string());
    let settings = dir.path().join("settings.toml");
    view.app.session.as_mut().unwrap().settings_path = Some(settings.clone());
    write_text(&mut view.app, "session.joinPassword", PROBE);
    run(&mut view.app, [join_verb()]);
    assert!(
        admitted_or_refused(&mut relay, &mut view),
        "the probe admits"
    );
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String(String::new())),
        "admission consumed it"
    );
    assert!(view.app.leave_session(LeaveCause::Own));

    let probe = PROBE.as_bytes();
    let holds = |bytes: &[u8]| bytes.windows(probe.len()).any(|window| window == probe);
    let recorded = relay.recorded.as_ref().unwrap();
    assert!(!recorded.is_empty(), "fixture: packets were recorded");
    assert!(!recorded.iter().any(|packet| holds(packet)), "a datagram");
    for file in [saved_file(&view.app), settings] {
        if let Ok(bytes) = std::fs::read(&file) {
            assert!(!holds(&bytes), "{}", file.display());
        }
    }
    for app in [&view.app, &relay.app] {
        let ctx = script_ctx_of(app);
        let table = ctx.slot_table.borrow();
        for (name, record) in table.iter() {
            assert!(
                !format!("{:?}", record.value).contains(PROBE),
                "slot `{name}` holds it"
            );
        }
    }
    assert!(
        !capture
            .records()
            .iter()
            .any(|record| record.message.contains(PROBE)),
        "a captured log line"
    );
    assert_eq!(orphans.count(), 0, "an uncaptured record");
}

// AC E11 (P16): a waiting join answers with the password it captured when
// pressed, not one written while it waited.
#[test]
fn a_waiting_join_answers_with_the_password_it_captured() {
    let points = spawn_points(3);
    let mut first = host_in_level(&points);
    let mut second = host_in_level(&points);
    second.server().set_session_password("p");
    let mut view = client_in_level(&mut first, &points, &second.address().to_string());
    write_text(&mut view.app, "session.joinPassword", "p");
    run(&mut view.app, [leave_verb(), join_verb()]);
    assert!(view.app.pending_entry.is_some());
    write_text(&mut view.app, "session.joinPassword", "q");
    view.app.drain_level_requests();
    view.app.resume_pending_entry();
    assert!(
        admitted_or_refused(&mut second, &mut view),
        "answered with `p`"
    );
}

/// Install the shipped dev frontend's menus, its reactions and keyboard, over
/// the engine keyboard it shadows, into `app` as mod init does. Returns each
/// menu tree as JSON.
fn install_dev_frontend(app: &mut App) -> Vec<(String, serde_json::Value)> {
    use postretro_level_format::data_script::DataScriptSection;
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts/frontend-menu.ts");
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("session-menus.ts");
    std::fs::write(
        &entry,
        format!(
            "{}\nexport function setupLevel() {{ return {{ uiTrees: [frontendMenu, hostMenu, joinMenu, devKeyboard, devLevelSelectMenu, optionsMenu, exitConfirm, quitConfirm], reactions: frontendReactions }}; }}\n",
            include_str!("../../../../content/dev/scripts/frontend-menu.ts")
        ),
    )
    .unwrap();
    let session = app.session.as_mut().unwrap();
    let manifest = session.scripting.script_runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: postretro_script_compiler::bundle_entry(&entry)
                .expect("the shipped frontend menu bundles")
                .into_bytes(),
            source_path: entry.to_string_lossy().into_owned(),
        },
        source.parent().unwrap(),
    );
    let trees: Vec<(String, serde_json::Value)> = manifest
        .ui_trees
        .iter()
        .map(|tree| (tree.name.clone(), serde_json::to_value(&tree.tree).unwrap()))
        .collect();
    session
        .modal_stack
        .register_script_trees(manifest.ui_trees, postretro_ui::modal_stack::ScopeTier::Mod);
    session
        .scripting
        .script_ctx
        .data_registry
        .borrow_mut()
        .populate_level(manifest.reactions, manifest.crossings, &[]);
    crate::scripting::reactions::system_commands::register_system_reaction_primitives(
        &mut session.scripting.system_registry,
    );
    let keyboard: postretro_ui::descriptor::AnchoredTree =
        serde_json::from_str(include_str!("../../../../core/ui/keyboard.json")).unwrap();
    session.modal_stack.registry_mut().register(
        postretro_ui::keyboard_asset::KEYBOARD_TREE_NAME,
        keyboard,
        postretro_ui::modal_stack::ScopeTier::Engine,
        false,
    );
    trees
}

/// Fire the menu reaction `name`, as its button's press does, then drain.
fn press(app: &mut App, name: &str) {
    {
        let session = app.session.as_ref().unwrap();
        let ctx = &session.scripting.script_ctx;
        let chained = crate::fire_named_event_with_sequences(
            name,
            &ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            ctx,
            None,
        );
        assert!(chained.is_empty(), "{name} chains nothing");
    }
    app.dispatch_system_commands();
}

/// Every `visibleWhen` predicate in `tree` that names `slot`, as JSON text.
fn predicates_naming(tree: &serde_json::Value, slot: &str, out: &mut Vec<String>) {
    match tree {
        serde_json::Value::Object(map) => {
            if let Some(predicate) = map.get("visibleWhen") {
                let text = predicate.to_string();
                if text.contains(slot) {
                    out.push(text);
                }
            }
            map.values()
                .for_each(|value| predicates_naming(value, slot, out));
        }
        serde_json::Value::Array(items) => {
            items
                .iter()
                .for_each(|item| predicates_naming(item, slot, out));
        }
        _ => {}
    }
}

// AC U8: the dev title menu, where an ended client lands, shows a notice for
// every end-cause value the engine publishes, and so does the submenu each
// cause concerns: Host for a failed hosting, Join for every other.
#[test]
fn the_dev_frontend_shows_a_notice_for_every_end_cause() {
    let mut view = entering_engine(spawn_points(3));
    let trees = install_dev_frontend(&mut view.app);
    let shows = |menu: &str, cause: EndCause| {
        let (_, tree) = trees
            .iter()
            .find(|(name, _)| name == menu)
            .unwrap_or_else(|| panic!("the dev frontend registers `{menu}`"));
        let mut predicates = Vec::new();
        predicates_naming(tree, "session.endCause", &mut predicates);
        predicates
            .iter()
            .any(|predicate| predicate.contains(&format!("\"{}\"", cause.as_str())))
    };
    for cause in EndCause::ALL {
        assert!(
            shows("frontend.menuTree", cause),
            "the title menu shows no notice for `{}`",
            cause.as_str()
        );
        let submenu = if cause == EndCause::HostFailed {
            "frontend.host"
        } else {
            "frontend.join"
        };
        assert!(
            shows(submenu, cause),
            "`{submenu}` shows no notice for `{}`",
            cause.as_str()
        );
    }
}

/// One drawn layer: its tree, its texts, and its enabled focus stops in export
/// order, each with what pressing it does and where it sits.
struct DrawnLayer {
    name: String,
    texts: Vec<String>,
    stops: Vec<(String, String, [f32; 4])>,
}

/// Lay out every layer the UI pass composes this frame, bottom to top, over
/// the session's live slots and cells.
fn draw_layers(app: &mut App) -> Vec<DrawnLayer> {
    use postretro_ui::theme::UiTheme;
    use postretro_ui::tree::{ImageSizes, NodeInteraction, UiTree};
    const DEVICE: [u32; 2] = [1280, 720];
    let session = app.session.as_mut().unwrap();
    let snapshot = App::build_ui_read_snapshot(
        &session.modal_stack,
        &mut session.presentation_cells,
        &session.scripting.script_ctx.slot_table.borrow(),
        session.net_endpoint.as_ref(),
        0.0,
        session.ui_input_mode,
        None,
        false,
        None,
        None,
    );
    snapshot
        .trees
        .iter()
        .map(|tree| {
            let mut ui = UiTree::from_descriptor(&tree.descriptor, &UiTheme::engine_default());
            let drawn = ui.build_draw_data_retained(
                DEVICE,
                &mut session.font_system,
                &ImageSizes::new(),
                &snapshot.slot_values,
                &snapshot.cell_values,
                0.0,
            );
            let rects = ui.export_focus_rects(
                &tree.descriptor,
                DEVICE,
                &snapshot.slot_values,
                &snapshot.cell_values,
            );
            DrawnLayer {
                name: tree.name.clone(),
                texts: drawn.texts.into_iter().map(|text| text.content).collect(),
                stops: rects
                    .rects
                    .into_iter()
                    .filter(|rect| !rect.disabled)
                    .map(|rect| {
                        let on_press = match rect.interaction {
                            Some(NodeInteraction::Button { on_press, .. }) => on_press,
                            _ => String::new(),
                        };
                        (rect.id, on_press, rect.rect)
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Lay out the top modal tree, as the UI pass does: its drawn texts, and its
/// focus stops in export order.
fn draw_top(app: &mut App) -> (Vec<String>, Vec<String>) {
    let top = draw_layers(app).pop().expect("a modal is up");
    (
        top.texts,
        top.stops.into_iter().map(|(id, _, _)| id).collect(),
    )
}

fn has(texts: &[String], text: &str) -> bool {
    texts.iter().any(|drawn| drawn == text)
}

fn enum_slot(app: &mut App, slot: &str, value: &str) {
    app.write_session_slot(slot, SlotValue::Enum(value.to_string()));
}

// The dev keyboard says which field it edits, shows the entry as it is typed,
// masks a password, offers the address pad only for an address, and keeps the
// key ids the engine shortcuts press.
#[test]
fn the_dev_keyboard_titles_its_field_and_masks_a_password() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    write_text(&mut view.app, "session.joinAddress", "10.0.0.");

    press(&mut view.app, "frontend.editJoinAddress");
    press(&mut view.app, "kbAppend_2");
    let (texts, stops) = draw_top(&mut view.app);
    assert!(has(&texts, "HOST ADDRESS"), "{texts:?}");
    assert!(
        has(&texts, "10.0.0.2"),
        "the entry shows as typed: {texts:?}"
    );
    for id in ["pad_1", "pad_dot", "pad_colon", "key_backspace", "key_done"] {
        assert!(stops.iter().any(|stop| stop == id), "no `{id}`: {stops:?}");
    }
    assert_eq!(stops.first().map(String::as_str), Some("pad_1"));
    assert!(
        !stops
            .iter()
            .any(|stop| stop == "key_q" || stop == "key_space")
    );
    view.app.cancel_text_entry();

    press(&mut view.app, "frontend.editPassword");
    for key in ["kbAppend_h", "kbAppend_i"] {
        press(&mut view.app, key);
    }
    let (texts, stops) = draw_top(&mut view.app);
    assert!(has(&texts, "PASSWORD"), "{texts:?}");
    assert!(has(&texts, "**"), "the entry shows masked: {texts:?}");
    assert!(
        !texts.iter().any(|text| text.contains("hi")),
        "the password never draws: {texts:?}"
    );
    for id in ["key_q", "key_dot", "key_space", "key_backspace", "key_done"] {
        assert!(stops.iter().any(|stop| stop == id), "no `{id}`: {stops:?}");
    }
    assert!(!stops.iter().any(|stop| stop == "pad_1"));
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String("hi".to_string()))
    );
}

// Each session menu shows one state at a time. When the focused button hides
// with its state, the first stop left is the new state's own button, so the
// initial-focus fallback lands there.
#[test]
fn the_dev_session_menus_show_one_state_at_a_time() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    let app = &mut view.app;

    press(app, "frontend.openJoin");
    let (_, stops) = draw_top(app);
    assert_eq!(
        stops,
        [
            "joinAddress",
            "joinPassword",
            "joinStart",
            "frontend.join.back"
        ]
    );
    enum_slot(app, "session.connection", "connecting");
    let (texts, stops) = draw_top(app);
    assert_eq!(stops, ["joinCancel", "frontend.join.back"]);
    assert!(
        texts.iter().any(|text| text.starts_with("CONNECTING TO")),
        "{texts:?}"
    );
    enum_slot(app, "session.connection", "waiting");
    enum_slot(app, "session.waitReason", "host");
    let (texts, stops) = draw_top(app);
    assert_eq!(stops, ["joinLeave", "frontend.join.back"]);
    assert!(
        has(&texts, "Waiting for the host to start a level."),
        "{texts:?}"
    );
    enum_slot(app, "session.connection", "offline");
    enum_slot(app, "session.waitReason", "");
    enum_slot(app, "session.endCause", "refusedPassword");
    let (texts, _) = draw_top(app);
    assert!(has(&texts, "PASSWORD REFUSED"), "{texts:?}");
    app.session.as_mut().unwrap().modal_stack.pop();

    press(app, "frontend.openHost");
    let (texts, stops) = draw_top(app);
    assert_eq!(stops, ["hostPassword", "hostStart", "frontend.host.back"]);
    assert!(has(&texts, "None: anyone can join"), "{texts:?}");
    assert!(
        !has(&texts, "PASSWORD REFUSED"),
        "a join's notice: {texts:?}"
    );
    app.write_session_slot("session.hosting", SlotValue::Boolean(true));
    write_text(app, "session.hostAddress", "10.0.0.7:27015");
    let (texts, stops) = draw_top(app);
    assert_eq!(stops, ["hostPlay", "hostStop", "frontend.host.back"]);
    assert!(has(&texts, "HOSTING ON 10.0.0.7:27015"), "{texts:?}");
    app.session.as_mut().unwrap().modal_stack.pop();

    // The title leads with the notice and its answer while there is one.
    app.write_session_slot("session.hosting", SlotValue::Boolean(false));
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named("frontend.menuTree", None);
    let (_, stops) = draw_top(app);
    assert_eq!(
        stops.first().map(String::as_str),
        Some("titleNotice.refusedPassword")
    );
    enum_slot(app, "session.endCause", "");
    let (_, stops) = draw_top(app);
    assert_eq!(
        stops,
        [
            "frontendPlay",
            "frontendHost",
            "frontendJoin",
            "frontendOptions",
            "frontendExit"
        ]
    );
}

/// The press that closes a dev frontend layer: BACK or CANCEL, or on the
/// title root, which nothing lies beneath, EXIT.
fn closes_its_layer(on_press: &str) -> bool {
    on_press == postretro_ui::actions::CLOSE_DIALOG_ACTION || on_press == "dev.askExit"
}

/// Every screen of the dev frontend flow, as the player reaches it: one
/// layer draws, so only its controls show and take focus; it has exactly one
/// way back, and nothing else reads as one; and that way back sits in the
/// footer, below every other control.
fn assert_one_layer_with_one_way_back(app: &mut App, screen: &str) {
    let layers = draw_layers(app);
    let names: Vec<&str> = layers.iter().map(|layer| layer.name.as_str()).collect();
    assert_eq!(layers.len(), 1, "{screen}: layers drawn {names:?}");
    let layer = &layers[0];
    let backs: Vec<_> = layer
        .stops
        .iter()
        .filter(|(_, on_press, _)| closes_its_layer(on_press))
        .collect();
    assert_eq!(
        backs.len(),
        1,
        "{screen}: one way back, found {backs:?} in {:?}",
        layer.stops
    );
    let back_labels = layer
        .texts
        .iter()
        .filter(|text| matches!(text.as_str(), "BACK" | "CANCEL"))
        .count();
    assert!(
        back_labels <= 1,
        "{screen}: only the way back reads as one: {:?}",
        layer.texts
    );
    let (back, _, back_rect) = backs[0];
    for (id, _, rect) in &layer.stops {
        assert!(
            back_rect[1] >= rect[1],
            "{screen}: `{back}` sits below every control, but `{id}` is lower"
        );
    }
}

// The engine's own panels opened from the dev frontend's Options own the
// screen as the mod's layers do: Options, and its BACK, stop drawing under
// them, and show again once the panel closes.
#[test]
fn engine_panels_over_the_dev_options_draw_alone() {
    use postretro_ui::demo::{
        ACCESSIBILITY_PANEL_NAME, CONTROLS_PANEL_NAME, DISPLAY_MODE_CONFIRM_NAME,
    };
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    let app = &mut view.app;
    // The shipped engine panels, registered as boot registers them.
    {
        let core = postretro_ui::core_root::CoreRoot::at(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core"),
        );
        let registry = app.session.as_mut().unwrap().modal_stack.registry_mut();
        for (name, file) in [
            (CONTROLS_PANEL_NAME, "controlsPanel.json"),
            (ACCESSIBILITY_PANEL_NAME, "accessibilityPanel.json"),
            (DISPLAY_MODE_CONFIRM_NAME, "displayModeConfirm.json"),
        ] {
            postretro_ui::tree_asset::register_tree_from_disk(registry, &core, name, file, false);
        }
    }
    let open_options = |app: &mut App| {
        let stack = &mut app.session.as_mut().unwrap().modal_stack;
        stack.clear_pushed();
        stack.push_named("frontend.menuTree", None);
        press(app, "frontend.openOptions");
    };
    let layer_names = |app: &mut App| {
        draw_layers(app)
            .into_iter()
            .map(|layer| layer.name)
            .collect::<Vec<_>>()
    };
    for (panel, open) in [
        (
            CONTROLS_PANEL_NAME,
            (|app: &mut App| app.open_controls_panel()) as fn(&mut App),
        ),
        (ACCESSIBILITY_PANEL_NAME, |app: &mut App| {
            app.open_accessibility_panel()
        }),
        (DISPLAY_MODE_CONFIRM_NAME, |app: &mut App| {
            app.session
                .as_mut()
                .unwrap()
                .modal_stack
                .push_named(DISPLAY_MODE_CONFIRM_NAME, None)
        }),
    ] {
        open_options(app);
        let options = layer_names(app);
        assert_eq!(options.len(), 1, "fixture: Options draws alone");
        open(app);
        assert_eq!(
            app.session.as_ref().unwrap().modal_stack.active_name(),
            Some(panel),
            "fixture: {panel} opened"
        );
        assert_eq!(
            layer_names(app),
            vec![panel.to_string()],
            "{panel} draws alone"
        );
        app.session.as_mut().unwrap().modal_stack.pop();
        assert_eq!(
            layer_names(app),
            options,
            "{panel} closed: Options shows again"
        );
    }
}

// The rules the frontend's visual grammar rests on, over every screen and
// session state: title, level select, options, the exit dialog, Host and Join
// in each state, and the keyboard over each field.
#[test]
fn every_dev_frontend_screen_is_one_layer_with_one_way_back() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    let app = &mut view.app;
    let reset = |app: &mut App| {
        let stack = &mut app.session.as_mut().unwrap().modal_stack;
        stack.clear_pushed();
        stack.push_named("frontend.menuTree", None);
    };

    reset(app);
    assert_one_layer_with_one_way_back(app, "title");
    for (open, screen) in [
        ("frontend.openPlay", "level select"),
        ("frontend.openOptions", "options"),
        ("dev.askExit", "exit dialog"),
        ("dev.askQuit", "quit dialog"),
        ("frontend.openHost", "host"),
        ("frontend.editHostPassword", "host password keyboard"),
    ] {
        press(app, open);
        assert_one_layer_with_one_way_back(app, screen);
    }
    app.cancel_text_entry();
    app.write_session_slot("session.hosting", SlotValue::Boolean(true));
    write_text(app, "session.hostAddress", "10.0.0.7:27015");
    assert_one_layer_with_one_way_back(app, "hosting");
    press(app, "frontend.openPlay");
    assert_one_layer_with_one_way_back(app, "level select over hosting");
    reset(app);
    assert_one_layer_with_one_way_back(app, "title while hosting");
    app.write_session_slot("session.hosting", SlotValue::Boolean(false));

    press(app, "frontend.openJoin");
    assert_one_layer_with_one_way_back(app, "join");
    for (edit, screen) in [
        ("frontend.editJoinAddress", "address keyboard"),
        ("frontend.editPassword", "password keyboard"),
    ] {
        press(app, edit);
        assert_one_layer_with_one_way_back(app, screen);
        app.cancel_text_entry();
    }
    for cause in EndCause::ALL {
        enum_slot(app, "session.endCause", cause.as_str());
        assert_one_layer_with_one_way_back(app, cause.as_str());
    }
    enum_slot(app, "session.endCause", "");
    enum_slot(app, "session.connection", "connecting");
    assert_one_layer_with_one_way_back(app, "connecting");
    enum_slot(app, "session.connection", "waiting");
    for reason in ["host", "modDiffers", "levelDiffers"] {
        enum_slot(app, "session.waitReason", reason);
        assert_one_layer_with_one_way_back(app, reason);
    }
    reset(app);
    assert_one_layer_with_one_way_back(app, "title while waiting");
    enum_slot(app, "session.connection", "offline");
    enum_slot(app, "session.waitReason", "");
    for cause in EndCause::ALL {
        enum_slot(app, "session.endCause", cause.as_str());
        assert_one_layer_with_one_way_back(app, &format!("title, {}", cause.as_str()));
    }
}

// The title's answer to a refused password opens the Join menu with the
// password keyboard over it; committing lands back on Join.
#[test]
fn a_title_notice_reopens_the_field_to_fix() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    press(&mut view.app, "frontend.retypePassword");
    press(&mut view.app, "kbAppend_p");
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String("p".to_string()))
    );
    assert_eq!(
        view.app.session.as_ref().unwrap().modal_stack.active_name(),
        Some("frontend.join")
    );
}

// The Join menu's fields: the keyboard opens seeded with the field's text,
// commit copies the entry into the field, cancel leaves it, and both empty
// `ui.textEntry`. The password never stays in `ui.textEntry`.
#[test]
fn a_menu_text_field_commits_into_its_own_slot() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    write_text(&mut view.app, "session.joinAddress", "10.0.0.");
    let text_entry = |app: &App| slot(app, "ui.textEntry");
    let typed = |app: &mut App, text: &str| {
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .system_commands
            .edit_text(
                "ui.textEntry".to_string(),
                postretro_entities::reactions::system_commands::TextEditCommand::Append(
                    text.to_string(),
                ),
            );
    };

    press(&mut view.app, "frontend.editJoinAddress");
    assert_eq!(
        text_entry(&view.app),
        Some(SlotValue::String("10.0.0.".to_string())),
        "the keyboard opens with the field's text"
    );
    // The on-screen keys the dev frontend registers type it.
    for key in ["kbAppend_2", "kbAppend_colon", "kbAppend_3"] {
        press(&mut view.app, key);
    }
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinAddress"),
        Some(SlotValue::String("10.0.0.2:3".to_string()))
    );
    assert_eq!(
        text_entry(&view.app),
        Some(SlotValue::String(String::new()))
    );

    press(&mut view.app, "frontend.editPassword");
    typed(&mut view.app, "hunter2");
    view.app.cancel_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String(String::new())),
        "cancel leaves the field as it was"
    );
    assert_eq!(
        text_entry(&view.app),
        Some(SlotValue::String(String::new()))
    );
    press(&mut view.app, "frontend.editPassword");
    typed(&mut view.app, "hunter2");
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String("hunter2".to_string()))
    );
    assert_eq!(
        text_entry(&view.app),
        Some(SlotValue::String(String::new()))
    );
}

/// Type `text` on the dev keyboard's on-screen keys, then commit.
fn type_on_keys(app: &mut App, text: &str) {
    for ch in text.chars() {
        let key = match ch {
            '.' => "kbAppend_dot".to_string(),
            ':' => "kbAppend_colon".to_string(),
            other => format!("kbAppend_{other}"),
        };
        press(app, &key);
    }
    app.commit_text_entry();
}

// Both sides in-app, every field typed on the dev keyboard: the host's
// password field protects its run, the joiner's admits it, and neither
// side's other password slot is touched.
#[test]
fn a_menu_host_and_a_menu_join_typed_on_the_keyboard_meet() {
    let points = spawn_points(3);
    let mut host = entering_engine(points.clone());
    let _ = install_dev_frontend(&mut host.app);
    press(&mut host.app, "frontend.editHostPassword");
    type_on_keys(&mut host.app, "pw");
    assert_eq!(
        slot(&host.app, "session.hostPassword"),
        Some(SlotValue::String("pw".to_string()))
    );
    assert_eq!(
        slot(&host.app, "session.joinPassword"),
        Some(SlotValue::String(String::new())),
        "the host field types into the host's slot only"
    );
    press(&mut host.app, "frontend.hostSession");
    assert_eq!(host.app.session_role(), SessionRole::Host);
    assert_eq!(
        slot(&host.app, "session.passwordProtected"),
        Some(SlotValue::Boolean(true))
    );
    let mut relay = Relay::adopt(host.app);
    let address = relay.address().to_string();

    let join = |password: &str| {
        let mut view = entering_engine(points.clone());
        let _ = install_dev_frontend(&mut view.app);
        write_text(&mut view.app, "session.joinAddress", "");
        press(&mut view.app, "frontend.editJoinAddress");
        type_on_keys(&mut view.app, &address);
        press(&mut view.app, "frontend.editPassword");
        type_on_keys(&mut view.app, password);
        assert_eq!(
            slot(&view.app, "session.hostPassword"),
            Some(SlotValue::String(String::new())),
            "the join field types into the join's slot only"
        );
        press(&mut view.app, "frontend.joinSession");
        assert_eq!(view.app.session_role(), SessionRole::Client);
        view
    };
    let mut wrong = join("pq");
    assert!(!admitted_or_refused(&mut relay, &mut wrong));
    assert_eq!(end_cause(&wrong.app), "refusedPassword");
    let mut right = join("pw");
    assert!(admitted_or_refused(&mut relay, &mut right));

    // Ending the run clears the protected flag with the hosting.
    relay.app.end_session();
    close_out(&mut relay.app);
    assert_eq!(
        slot(&relay.app, "session.passwordProtected"),
        Some(SlotValue::Boolean(false))
    );
}

// Regression: with persistence unavailable a join skipped its rebase, so a
// leave reset the world to its declared defaults.
#[test]
fn join_with_persistence_off_still_restores_the_world_at_join() {
    let dir = tempfile::tempdir().unwrap();
    let relay = host_in_level(&spawn_points(3));
    let mut view = single_player_at_menu(spawn_points(3));
    with_saves(&mut view, dir.path());
    {
        let session = view.app.session.as_mut().unwrap();
        session.data_dir = None;
        assert!(session.state_store_lifecycle.disable_persistence());
    }
    write_world(&view.app, 42.0);

    assert!(view.app.join_from_frontend(relay.address()));
    write_world(&view.app, 99.0);
    assert!(view.app.leave_session(LeaveCause::Own));

    assert_eq!(slot(&view.app, WORLD_SLOT), Some(SlotValue::Number(42.0)));
}

// A field whose keyboard closes some other way leaves nothing typed in
// `ui.textEntry`, and a later keyboard opened without a target never copies
// into the old field.
#[test]
fn a_field_closed_another_way_is_dropped_with_its_text() {
    let mut view = entering_engine(spawn_points(3));
    let _ = install_dev_frontend(&mut view.app);
    press(&mut view.app, "frontend.editPassword");
    press(&mut view.app, "kbAppend_h");
    view.app.session.as_mut().unwrap().modal_stack.pop();
    run(&mut view.app, []);
    assert_eq!(
        slot(&view.app, "ui.textEntry"),
        Some(SlotValue::String(String::new())),
        "the typed text left with its keyboard"
    );

    run(
        &mut view.app,
        [SystemReactionCommand::PushTree {
            tree: postretro_ui::keyboard_asset::KEYBOARD_TREE_NAME.to_string(),
            on_commit: None,
            commit_target: None,
        }],
    );
    press(&mut view.app, "kbAppend_x");
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String(String::new())),
        "an untargeted entry never reaches the old field"
    );
}

// Admission consumes the password; an open PASSWORD field cannot write the
// consumed value, or anything typed after, back.
#[test]
fn an_open_password_field_cannot_restore_a_consumed_password() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let mut view = joining_with(&relay, &points, "p");
    let _ = install_dev_frontend(&mut view.app);
    press(&mut view.app, "frontend.editPassword");
    drain(&mut view, vec![holding(HoldingCause::HostLevelAbsent)]);
    press(&mut view.app, "kbAppend_x");
    view.app.commit_text_entry();
    assert_eq!(
        slot(&view.app, "session.joinPassword"),
        Some(SlotValue::String(String::new()))
    );
    assert_eq!(
        slot(&view.app, "ui.textEntry"),
        Some(SlotValue::String(String::new()))
    );
}

// Regression: a `leaveSession(); joinSession()` reaction whose frame also
// brought the host's end was dropped with the unloading level's queue.
#[test]
fn a_join_queued_in_the_frame_the_host_ends_survives_the_unload() {
    let points = spawn_points(3);
    let mut first = host_in_level(&points);
    let mut second = host_in_level(&points);
    let mut view = client_in_level(&mut first, &points, &second.address().to_string());
    let ctx = script_ctx_of(&view.app);
    ctx.system_commands.push(leave_verb());
    ctx.system_commands.push(join_verb());

    // The host's end arrives in the same frame; the frame stops before its
    // command drain, and the next frame drains the unload first.
    drain(&mut view, vec![closing(ClosingCause::SessionEnded)]);
    assert_eq!(view.app.session_role(), SessionRole::SinglePlayer);
    view.app.drain_level_requests();
    view.app.dispatch_system_commands();

    assert_eq!(view.app.session_role(), SessionRole::Client, "the join ran");
    let _ = follow_into_level(&mut second, &mut view);
}

// A press kept through an unload only while the frontend menu is up: an
// unload that leads to a played level drops it with the level's queue.
#[test]
fn an_unload_away_from_the_frontend_drops_a_queued_press() {
    let mut view = single_player_at_menu(spawn_points(3));
    let ctx = script_ctx_of(&view.app);
    ctx.system_commands.push(join_verb());
    view.app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    assert!(!view.app.frontend_menu_is_present());
    view.app.unload_level();
    assert!(
        ctx.system_commands.take_ordered().is_empty(),
        "the press left with the level"
    );

    // With the menu up, the same unload keeps it.
    let mut view = single_player_at_menu(spawn_points(3));
    let ctx = script_ctx_of(&view.app);
    assert!(view.app.frontend_menu_is_present());
    ctx.system_commands.push(join_verb());
    view.app.unload_level();
    assert_eq!(
        ctx.system_commands.take_ordered().len(),
        1,
        "the press stays"
    );
}

// Drift guard: the catalog's `session.endCause` values are the engine's end
// causes, plus the empty value, in order.
#[test]
fn end_cause_catalog_values_match_the_engine_vocabulary() {
    let mut expected = vec![""];
    expected.extend(EndCause::ALL.iter().map(|cause| match cause {
        EndCause::SessionEnded
        | EndCause::HostLost
        | EndCause::ConnectFailed
        | EndCause::RefusedMod
        | EndCause::RefusedPassword
        | EndCause::BadAddress
        | EndCause::HostFailed => cause.as_str(),
    }));
    assert_eq!(
        postretro_entities::engine_state_catalog::SESSION_END_CAUSE_VALUES,
        expected.as_slice()
    );
}
