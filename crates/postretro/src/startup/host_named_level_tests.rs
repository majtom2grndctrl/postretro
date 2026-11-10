//! A connected client following its host's level clears every frontend menu
//! layer at that level's install and settles at the stand-in; a failed
//! host-named load puts the menu back with no backdrop load. Backdrops the
//! host has not named, and single-player, keep the menu pose.
//! See: context/lib/boot_sequence.md §1 (Presented pose) · context/lib/networking.md §Slot lifecycle

use super::spawn_pose_handoff_fixtures::{LEVEL, Relay, spawn_points};
use super::spawn_pose_stand_in_fixtures::{
    ClientView, OTHER_LEVEL, assert_pose, hint_pose, host_reinstalls, menu_pose, without_hints,
};
use super::*;
use crate::startup::{BootDestination, LevelRequest};
use postretro_net::wire::SpawnPoseHint;
use postretro_sim::scripting::map_entity::MapEntity;

const CLIENT: u64 = 41;

/// The failure landing's loop: a runtime load never exits it.
struct NoExit;

impl LoopExit for NoExit {
    fn exit(&self) {
        panic!("a runtime load failure never exits the loop");
    }
}

/// A host with one seated client and `points` installed, its Relevel and hint
/// delivered to the relay client, not yet to any client App.
fn hosted(points: &[MapEntity]) -> (Relay, SpawnPoseHint) {
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.to_vec());
    let hint = relay.await_hint(CLIENT);
    (relay, hint)
}

/// A `--connect` client at its frontend menu, as boot presents it with no
/// host level queued yet.
fn client_at_menu(points: Vec<MapEntity>) -> ClientView {
    let mut client = ClientView::new(points).with_frontend();
    client.app.present_frontend_menu();
    assert!(client.menu_is_present());
    client
}

/// Neither a backdrop nor any other load is queued or in flight.
#[track_caller]
fn assert_no_load(client: &ClientView) {
    assert!(
        client.app.level_requests.is_empty(),
        "no load is queued: {:?}",
        client.app.level_requests
    );
    assert!(client.app.level_load.is_none(), "no load is in flight");
}

/// The menu is gone, and later frames' menu-pose step leaves the camera.
#[track_caller]
fn assert_menu_gone(client: &mut ClientView) {
    assert!(!client.menu_is_present(), "the frontend menu is gone");
    assert_eq!(client.pushed_layers(), 0, "no frontend layer survives");
    let before = client.camera();
    client.app.apply_frontend_menu_camera_pose_if_present();
    assert_pose(client.camera(), before, "the menu pose no longer applies");
}

// M1: a `--connect` boot with no map argument, and one whose map argument the
// host's map replaced.
#[test]
fn connected_client_booting_to_its_menu_or_a_replaced_map_reveals_the_host_level_at_the_hint_with_the_menu_gone()
 {
    let points = spawn_points(3);
    let (mut relay, hint) = hosted(&points);
    let arrivals = relay.forward(CLIENT);
    for destination in [
        BootDestination::Frontend,
        BootDestination::BootMap(PathBuf::from("content/dev/maps/cli-boot.prl")),
    ] {
        let mut client = ClientView::new(points.clone()).with_frontend();
        // The host names its map while the splash waits.
        client.app.boot_state = BootState::Splash;
        client.drain(arrivals.clone());
        client.app.start_boot_destination(destination.clone());
        assert!(
            client.menu_is_present(),
            "{destination:?}: boot pushes the menu"
        );
        assert!(
            client
                .app
                .level_load
                .as_ref()
                .is_some_and(|load| load.host_named),
            "{destination:?}: the host's load is in flight, host-named"
        );

        client.install();
        assert_menu_gone(&mut client);
        client.assert_at(hint_pose(&hint), "the host level's stand-in");
        assert_pose(
            client.app.presented_pose(),
            hint_pose(&hint),
            "Settling streams from the hint",
        );
        client.reveal();
        client.assert_at(hint_pose(&hint), "the reveal");
    }
}

// M2, O15.
#[test]
fn submenu_over_the_frontend_menu_is_cleared_with_it_at_the_host_level_install() {
    let points = spawn_points(3);
    let (mut relay, hint) = hosted(&points);
    let mut client = client_at_menu(points);
    client
        .app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "modSettings",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    client
        .app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named("modSettings", None);
    assert_eq!(client.pushed_layers(), 2, "a submenu over the root");

    client.follow(&mut relay, CLIENT);
    client.install();
    assert_menu_gone(&mut client);
    client.assert_at(hint_pose(&hint), "the host level's stand-in");
}

// M3, O2: the backdrop load of the host's level, in flight or still queued.
#[test]
fn backdrop_load_of_the_host_level_still_in_flight_is_adopted_and_installs_as_the_host_level() {
    let points = spawn_points(3);
    let (mut relay, hint) = hosted(&points);
    let arrivals = relay.forward(CLIENT);
    for dispatched in [true, false] {
        let mut client = ClientView::new(points.clone()).with_frontend();
        client.app.populate_frontend();
        if dispatched {
            client.app.drain_level_requests();
            assert!(
                client
                    .app
                    .level_load
                    .as_ref()
                    .is_some_and(|load| !load.host_named),
                "the backdrop load is the client's own"
            );
        }

        client.drain(arrivals.clone());
        if dispatched {
            assert!(
                client.app.level_requests.is_empty(),
                "the load is not restarted"
            );
            assert!(client.app.level_load.as_ref().unwrap().host_named);
        } else {
            assert_eq!(
                client.app.level_requests,
                [LevelRequest::HostLevel(LEVEL.to_string())],
                "the queued backdrop load is adopted, not doubled"
            );
        }

        client.install();
        assert_menu_gone(&mut client);
        client.assert_at(hint_pose(&hint), "the host level's stand-in");
        client.reveal();
        client.assert_at(hint_pose(&hint), "the reveal");
    }
}

// M4, O1, O12, O13.
#[test]
fn installed_backdrop_of_the_host_level_reloads_as_the_host_level_whether_settling_or_running() {
    let points = spawn_points(3);
    let (mut relay, hint) = hosted(&points);
    let arrivals = relay.forward(CLIENT);
    for running in [false, true] {
        let mut client = ClientView::new(points.clone()).with_frontend();
        client.app.populate_frontend();
        client.install();
        if running {
            client.reveal();
        }
        assert!(client.menu_is_present(), "the backdrop keeps the menu");
        client.assert_at(menu_pose(), "the backdrop presents the menu pose");

        client.drain(arrivals.clone());
        assert_eq!(
            client.app.level_requests,
            [LevelRequest::HostLevel(LEVEL.to_string())],
            "running {running}: the backdrop reloads as the host's level"
        );
        client.install();
        assert_menu_gone(&mut client);
        client.assert_at(hint_pose(&hint), "the host level's stand-in");
        client.reveal();
        client.assert_at(hint_pose(&hint), "the reveal");
    }
}

// M5, O14.
#[test]
fn host_named_load_with_no_matching_hint_takes_the_first_spawn_stand_in_not_the_menu_pose() {
    let points = spawn_points(3);
    let (mut relay, _hint) = hosted(&points);
    let mut client = client_at_menu(points);
    client.drain(without_hints(&relay.forward(CLIENT)));

    client.install();
    assert_menu_gone(&mut client);
    client.assert_at(client.first_spawn_pose(), "the first-spawn stand-in");
    client.reveal();
    client.assert_at(client.first_spawn_pose(), "the reveal");
}

// M6, O18: the first host-named load fails, and it is the backdrop's level too.
#[test]
fn first_host_named_load_that_fails_returns_to_the_menu_with_no_backdrop_load() {
    let points = spawn_points(3);
    let (mut relay, _hint) = hosted(&points);
    let mut client = ClientView::new(points).with_frontend();
    client.app.boot_state = BootState::Splash;
    client.follow(&mut relay, CLIENT);
    client.app.start_boot_destination(BootDestination::Frontend);
    assert!(client.app.level_load.as_ref().unwrap().host_named);

    client
        .app
        .finish_level_failure("prl parse failed".to_string(), &NoExit);
    assert_eq!(client.app.boot_state, BootState::Frontend);
    assert!(client.menu_is_present());
    client.assert_at(menu_pose(), "the menu pose");
    assert_no_load(&client);
}

// M6, O9: a later host-named load fails after an install cleared the menu.
#[test]
fn later_host_named_load_that_fails_puts_the_cleared_menu_back_with_no_backdrop_load() {
    let points = spawn_points(3);
    let (mut relay, _hint) = hosted(&points);
    let mut client = client_at_menu(points.clone());
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();
    assert!(!client.menu_is_present());

    let moved = host_reinstalls(&mut relay, CLIENT, OTHER_LEVEL, &points);
    client.drain(moved);
    client.app.drain_level_requests();
    assert_eq!(client.app.boot_state, BootState::Loading);
    client
        .app
        .finish_level_failure("prl parse failed".to_string(), &NoExit);

    assert_eq!(client.app.boot_state, BootState::Frontend);
    assert!(client.menu_is_present(), "the menu is back");
    client.assert_at(menu_pose(), "the menu pose");
    assert_no_load(&client);
}

// M6, O16: a relevel naming a catalog id this client lacks.
#[test]
fn relevel_naming_an_unknown_id_unloads_the_followed_level_to_the_menu_with_no_backdrop_load() {
    let points = spawn_points(3);
    let (mut relay, _hint) = hosted(&points);
    let mut client = client_at_menu(points);
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();
    assert!(!client.menu_is_present());

    client
        .app
        .follow_relevel_catalog("missing-map".to_string(), 99);
    assert!(client.menu_is_present(), "the menu is presented at once");
    client.app.drain_level_requests();

    assert_eq!(client.app.boot_state, BootState::Frontend);
    assert!(
        !client.app.has_installed_level(),
        "the stale level unloaded"
    );
    assert!(client.menu_is_present());
    assert_no_load(&client);
}

// M7, O17.
#[test]
fn backdrop_the_client_loads_of_its_own_accord_keeps_the_menu_even_when_it_is_the_host_level() {
    let points = spawn_points(3);
    let (mut relay, hint) = hosted(&points);
    let mut client = client_at_menu(points);
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();
    assert!(!client.menu_is_present());

    // The player quits to the menu; its backdrop is the host's level, and the
    // last relevel and the stored hint still name it.
    client.app.return_to_frontend();
    client.install();
    assert_eq!(client.app.published_level_identity.as_deref(), Some(LEVEL));
    assert_eq!(client.stored_hint(), Some(hint));
    assert!(client.menu_is_present(), "the backdrop keeps the menu");
    assert_pose(
        client.app.presented_pose(),
        menu_pose(),
        "Settling streams from the menu pose",
    );
    client.reveal();
    assert!(client.menu_is_present());
    client.assert_at(menu_pose(), "the backdrop's reveal");
}

// M9: an un-named backdrop on a connected client, and a single-player
// install with the menu pushed.
#[test]
fn backdrop_the_host_has_not_named_and_single_player_menu_install_present_the_menu_pose() {
    for single_player in [false, true] {
        let points = spawn_points(3);
        let mut app = if single_player {
            ClientView::single_player(points)
        } else {
            ClientView::new(points)
        }
        .with_frontend();
        app.app.populate_frontend();
        app.install();
        assert!(app.menu_is_present(), "single player {single_player}");
        assert_pose(
            app.app.presented_pose(),
            menu_pose(),
            "Settling streams from the menu pose",
        );
        app.reveal();
        app.assert_at(menu_pose(), "the reveal presents the menu pose");
    }
}
