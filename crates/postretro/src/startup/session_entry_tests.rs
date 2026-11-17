//! In-app session entry: a single-player engine at its frontend joins or
//! hosts over a real loopback transport, built on the path boot flags take.
//! See: context/lib/networking.md §Role model (Runtime entry)

use super::spawn_pose_handoff_fixtures::{LEVEL, MOD_ID, Relay, spawn_points};
use super::spawn_pose_stand_in_fixtures::ClientView;
use super::lifecycle_test_fixtures::test_app;
use super::*;
use super::session_membership::LeaveCause;
use crate::netcode::{NetEndpoint, NetRole};
use crate::session::net_entry::enter_role;
use crate::session::role_state::{RoleBoundState, SessionRole, apply_role};
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::Seat;
use postretro_net::slots::SlotState;
use postretro_net::wire::SessionId;
use crate::startup::{BootState, LevelRequest};
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
    let hint = view.stored_hint().expect("the host hinted the client's spawn");
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
    assert!(view.app.level_load_in_flight(), "fixture: the backdrop loads");
    assert!(!view.app.level_load.as_ref().unwrap().host_named);

    assert!(view.app.join_from_frontend(relay.address()));
    assert!(!view.app.level_load_in_flight(), "the backdrop load is cancelled");
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
    let declared = view.declared_reveal().expect("the client declared its reveal");
    assert_eq!(declared.level_life, relay.server().installed_level_life());
    assert_client_role(&view);
}

// A join from anywhere but a single-player frontend changes nothing.
#[test]
fn in_app_join_off_the_frontend_is_refused_and_changes_nothing() {
    let points = spawn_points(3);
    let relay = host_in_level(&points);
    let mut view = single_player_at_menu(points);
    view.app.session.as_mut().unwrap().modal_stack.clear_pushed();
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
        if app.session.as_ref().unwrap().closing_host_transport.is_none() {
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
    assert!(!relay.participating(id), "the joiner waits at the host's frontend");
    assert!(view.app.level_requests.is_empty(), "the host names no level");

    // The host starts a level: its menu clears and the level installs.
    relay.app.session.as_mut().unwrap().modal_stack.clear_pushed();
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
    assert!(view.app.at_frontend(), "the left level unloaded under the menu");

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
    assert_eq!(relay.seat(22), Some(Seat(1)), "seat ids restart with the run");
}

// AC E8: a failed session-id mint leaves the ledger, the role and the
// endpoint as they were.
#[test]
fn failed_session_mint_at_host_entry_stays_single_player_with_its_ledger() {
    let mut view = single_player_at_menu(spawn_points(3));
    let before = session_id(&view.app);
    let hosted = view
        .app
        .host_from_frontend_with(0, |_| Err(getrandom::Error::UNSUPPORTED));
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
    fn boot(role: &NetRole) -> RoleBoundState {
        let mut app = test_app();
        let session = app.session.as_mut().unwrap();
        session.net_endpoint = enter_role(
            role,
            None,
            &mut session.scripting,
            &mut session.seat_table,
            getrandom::fill,
            crate::netcode::probe_lan_address,
        )
        .endpoint;
        observe(&app)
    }
    let relay = host_in_level(&spawn_points(3));

    let mut host = single_player_at_menu(spawn_points(3));
    assert!(host.app.host_from_frontend(0));
    assert_eq!(observe(&host.app), boot(&NetRole::Host { port: 0 }));

    let mut client = single_player_at_menu(spawn_points(3));
    assert!(client.app.join_from_frontend(relay.address()));
    assert_eq!(
        observe(&client.app),
        boot(&NetRole::Connect {
            addr: relay.address()
        })
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
    session.scripting.script_runtime.commit_store_identity_for_test(
        ledger,
        [WORLD_SLOT.to_string(), CHARACTER_SLOT.to_string()].into(),
    );
    assert!(session.state_store_lifecycle.should_restore_after_mod_init(true));
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

fn saved_file(app: &App) -> std::path::PathBuf {
    let session = app.session.as_ref().unwrap();
    crate::scripting::state_persistence::state_path(session.data_dir.as_deref(), MOD_ID).unwrap()
}

fn character_in(state: Option<&crate::scripting::state_persistence::PersistedState>) -> f64 {
    let seed = crate::scripting::state_persistence::join_seed_from_persisted_state(
        state,
        Some(PLAYER_ID),
    );
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

    let on_disk = crate::scripting::state_persistence::load_persisted_state(&saved_file(&view.app))
        .unwrap()
        .expect("the join wrote the save");
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
        assert!(seats.carried_state(Seat(0)).is_some(), "fixture: seat 0 carries");
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
