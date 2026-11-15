//! Session player counts across a level's life in one app: written at install
//! from the seats, each tick from the capture, zeroed at unload; a frontend
//! backdrop is not a session level.
//! See: context/lib/scripting.md §5 (Session players)

use super::lifecycle_test_fixtures::{level_world, script_ctx, test_app};
use super::*;
use crate::netcode::session_players::CapturedPawns;
use postretro_entities::{SlotValue, Transform};
use postretro_foundation::Seat;
use postretro_scripting_core::runtime::{Frontend, MenuCamera};

const LEVEL: &str = "session-players";

fn counts(app: &App) -> [f32; 5] {
    let ctx = script_ctx(app);
    let table = ctx.slot_table.borrow();
    ["connected", "loading", "inPlay", "down", "reconnecting"].map(|name| {
        match table
            .get(&format!("session.playerCounts.{name}"))
            .unwrap()
            .value
        {
            Some(SlotValue::Number(value)) => value,
            ref other => panic!("count `{name}` reads {other:?}"),
        }
    })
}

fn single_player(app: &mut App) {
    app.session.as_mut().unwrap().seat_table = Some(crate::netcode::SeatTable::local_only());
}

/// The host's pawn spawns and binds to seat 0, as install's first player does.
fn spawn_host_pawn(app: &mut App) -> postretro_entities::EntityId {
    let ctx = script_ctx(app);
    let pawn = ctx.registry.borrow_mut().spawn(Transform::default());
    app.session
        .as_mut()
        .unwrap()
        .seat_table
        .as_mut()
        .unwrap()
        .bind_pawn(&mut ctx.registry.borrow_mut(), Seat(0), pawn);
    pawn
}

fn tick(app: &mut App, captured: &[postretro_entities::EntityId], counted: &[postretro_entities::EntityId]) {
    let ctx = script_ctx(app);
    let session = app.session.as_mut().unwrap();
    session.session_players.publish_tick(
        session.seat_table.as_ref(),
        &ctx.registry.borrow(),
        CapturedPawns { captured, counted },
        &mut ctx.slot_table.borrow_mut(),
    );
}

// Single player reads `inPlay` 1, or `down` with `inPlay` 0 when dead; the host
// seat reads `loading` between install and its pawn spawning (P26); unload
// zeroes every count until the next install writes them (P14, P21).
#[test]
fn single_player_counts_follow_install_tick_death_and_unload() {
    let mut app = test_app();
    single_player(&mut app);
    assert_eq!(counts(&app), [0.0; 5], "no session level yet");

    app.install_level_identity(&level_world(LEVEL, 1));
    assert!(app.session.as_ref().unwrap().session_players.session_level());
    assert_eq!(counts(&app), [1.0, 1.0, 0.0, 0.0, 0.0]);

    let pawn = spawn_host_pawn(&mut app);
    tick(&mut app, &[pawn], &[pawn]);
    assert_eq!(counts(&app), [1.0, 0.0, 1.0, 0.0, 0.0]);

    tick(&mut app, &[pawn], &[]);
    assert_eq!(counts(&app), [1.0, 0.0, 0.0, 1.0, 0.0], "dead: down, not in play");

    app.unload_level();
    assert_eq!(counts(&app), [0.0; 5]);
    assert!(!app.session.as_ref().unwrap().session_players.session_level());
    tick(&mut app, &[], &[]);
    assert_eq!(counts(&app), [0.0; 5], "no tick writes counts without a session level");
}

// A frontend backdrop is not a session level: its ticks write nothing, with or
// without a pawn on it (P14).
#[test]
fn frontend_backdrop_is_not_a_session_level() {
    let mut app = test_app();
    single_player(&mut app);
    let session = app.session.as_mut().unwrap();
    session.modal_stack.registry_mut().register(
        "mainMenu",
        postretro_ui::demo::build_frontend_menu_descriptor(),
        postretro_ui::modal_stack::ScopeTier::Mod,
        false,
    );
    session.frontend = Some(Frontend {
        menu_tree: "mainMenu".to_string(),
        background_level: Some(LEVEL.to_string()),
        camera: MenuCamera {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        },
    });
    app.present_frontend_menu();
    assert!(app.frontend_menu_is_present());

    app.install_level_identity(&level_world(LEVEL, 1));
    assert!(!app.session.as_ref().unwrap().session_players.session_level());
    let pawn = spawn_host_pawn(&mut app);
    tick(&mut app, &[pawn], &[pawn]);
    assert_eq!(counts(&app), [0.0; 5]);
    assert!(
        app.session
            .as_ref()
            .unwrap()
            .session_players
            .statuses()
            .is_empty()
    );
}

// A client holds the host's replicated counts while its level lives; leaving
// unloads it, and every count reads 0 at the frontend (P14).
#[test]
fn client_counts_read_zero_once_its_level_unloads() {
    let mut app = test_app();
    super::lifecycle_test_fixtures::connect_test_app_as_client(&mut app);
    {
        let ctx = script_ctx(&app);
        let mut table = ctx.slot_table.borrow_mut();
        // As a snapshot applies them: engine writes bypass readonly.
        for (name, value) in [("connected", 3.0), ("inPlay", 2.0), ("loading", 1.0)] {
            table
                .get_mut(&format!("session.playerCounts.{name}"))
                .unwrap()
                .write_value(Some(SlotValue::Number(value)));
        }
    }
    assert_eq!(counts(&app), [3.0, 1.0, 2.0, 0.0, 0.0]);

    app.unload_level();
    assert_eq!(counts(&app), [0.0; 5]);
}
