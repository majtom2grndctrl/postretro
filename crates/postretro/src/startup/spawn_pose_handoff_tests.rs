//! Where the host first names an installed level to a seat, it resolves the
//! seat's spawn pose and sends its hint after Relevel; promotion spawns the
//! pawn at that pose. A host frontend backdrop names no level.
//! See: context/lib/networking.md §Slot lifecycle

use super::lifecycle_test_fixtures::{
    commit_level_bundle_source, connect_test_app_as_client, level_world, script_ctx, test_app,
};
use super::spawn_pose_handoff_fixtures::{
    LEVEL, Naming, OfferOne, Relay, hint_pose_of, pose, spawn_points, standing_on,
};
use super::*;
use crate::spawn_pose::{SpawnPose, UnbackedSpawn};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_foundation::Seat;
use postretro_net::wire::SpawnPoseHint;
use postretro_scripting_core::runtime::{Frontend, MenuCamera};
use postretro_sim::scripting::map_entity::MapEntity;

fn relevel_then_hint(naming: &[Naming]) -> (u64, SpawnPoseHint) {
    match naming {
        [Naming::Relevel(life), Naming::Hint(hint)] => (*life, hint.clone()),
        other => panic!("expected Relevel then its hint, got {other:?}"),
    }
}

fn position(naming: &[Naming], wanted: impl Fn(&Naming) -> bool) -> usize {
    naming
        .iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("arrival missing from {naming:?}"))
}

fn is_hint(naming: &Naming) -> bool {
    matches!(naming, Naming::Hint(_))
}

fn is_marker(naming: &Naming) -> bool {
    matches!(naming, Naming::Marker)
}

/// The hint names start `index` of `points`: its standing eye, yaw and pitch.
fn assert_hint_names(hint: &SpawnPoseHint, points: &[MapEntity], index: usize) {
    let (eye, yaw, pitch) = hint_pose_of(points, index);
    assert!(
        Vec3::from_array(hint.eye).distance(Vec3::from_array(eye)) < 1e-5,
        "hint eye {:?} is start {index}'s eye {eye:?}",
        hint.eye
    );
    assert_eq!(
        (hint.yaw, hint.pitch),
        (yaw, pitch),
        "start {index}'s facing"
    );
}

fn scout_pose() -> UnbackedSpawn {
    UnbackedSpawn {
        pose: SpawnPose {
            eye: Vec3::new(20.0, 3.0, -9.0),
            yaw: 0.8,
            pitch: 0.25,
        },
        entity_class: "scout".into(),
        single_use: false,
    }
}

/// The two clients ordered by seat: the lower resolves first.
fn by_seat(relay: &Relay, a: u64, b: u64) -> (u64, u64) {
    if relay.seat(a) < relay.seat(b) {
        (a, b)
    } else {
        (b, a)
    }
}

// R1, R14, O8.
#[test]
fn host_install_sends_relevel_then_hint_and_promotion_spawns_at_the_hinted_pose() {
    let mut relay = Relay::admitted(&[41]);
    let points = spawn_points(3);
    let (host_pawn, host_placement) = relay.install(points.clone()).unwrap();

    let hint = relay.await_hint(41);
    let (life, _) = relevel_then_hint(&relay.naming(41));
    assert!(
        !relay.participating(41) && !relay.client(41).is_participating(),
        "the hint precedes promotion"
    );
    assert_eq!(hint.level_identity, LEVEL);
    assert_eq!(hint.level_life, Some(life), "the life Revealed declares");
    assert_hint_names(&hint, &points, 1);
    assert!(
        hint.yaw != 0.0 && hint.pitch != 0.0,
        "fixture faces off-axis"
    );

    relay.reveal(41);
    relay.host_reveal();
    let pawn = relay.await_pawn(41);

    relay.assert_pawn_at_hint(pawn, &hint);
    // R14: the host keeps its install spawn and no joiner is given it.
    assert_eq!(host_placement, 0);
    assert_eq!(relay.placement(41), Some(1));
    assert_eq!(relay.seats().placement_for_seat(Seat(0)), Some(0));
    assert_eq!(
        script_ctx(&relay.app).registry.borrow().local_player_pawn(),
        Some(host_pawn),
        "the host's pawn is still its own"
    );
    assert!(
        relay
            .transform(host_pawn)
            .position
            .distance(standing_on(&points, 0))
            < 1e-5
    );
}

// R2, and the Decision that a seat whose pose no placement backs holds none.
#[test]
fn unbacked_test_pose_reserves_nothing_and_its_pawn_spawns_there() {
    let mut relay = Relay::admitted(&[41, 42]);
    let (offered_client, other_client) = by_seat(&relay, 41, 42);
    let offered = relay.seat(offered_client).unwrap();
    relay
        .app
        .spawn_poses
        .set_unbacked_source(Some(Box::new(OfferOne {
            seat: offered,
            spawn: scout_pose(),
            consumed: Default::default(),
        })));
    let points = spawn_points(3);
    relay.install(points.clone());

    let offered_hint = relay.await_hint(offered_client);
    let other_hint = relay.await_hint(other_client);
    relevel_then_hint(&relay.naming(offered_client));
    assert_eq!(
        pose(&offered_hint),
        (scout_pose().pose.eye.to_array(), 0.8, 0.25)
    );
    assert_eq!(relay.placement(offered_client), None);
    assert_eq!(
        relay.placement(other_client),
        Some(1),
        "the next seat takes the lowest free start, as if the first were absent"
    );
    assert_hint_names(&other_hint, &points, 1);

    relay.reveal(offered_client);
    relay.reveal(other_client);
    relay.host_reveal();
    let offered_pawn = relay.await_pawn(offered_client);
    let other_pawn = relay.await_pawn(other_client);

    relay.assert_pawn_at_hint(offered_pawn, &offered_hint);
    assert_eq!(
        script_ctx(&relay.app)
            .registry
            .borrow()
            .get_component::<DescriptorProvenance>(offered_pawn)
            .unwrap()
            .canonical_name,
        "scout",
        "the pawn spawns as the class the source named"
    );
    relay.assert_pawn_at_hint(other_pawn, &other_hint);
    assert_eq!(relay.placement(offered_client), None);
}

// O8: production install sends the hints after Relevel, the host pawn's bind
// and the retained spawn points, as the fixture's install orders them.
#[test]
fn commit_sends_spawn_pose_hints_after_relevel_host_bind_and_spawn_points() {
    let commit = commit_level_bundle_source();
    let at = |needle: &str| {
        commit
            .find(needle)
            .unwrap_or_else(|| panic!("commit reaches `{needle}`"))
    };
    let hints = at("self.send_install_spawn_pose_hints()");
    assert!(at("endpoint.set_relevel_catalog_id(") < hints);
    assert!(at("bind_installed_first_player(") < hints);
    assert!(at("self.host_spawn_points = products.spawn_points") < hints);
    assert_eq!(
        commit
            .matches("self.send_install_spawn_pose_hints()")
            .count(),
        1
    );
}

// R9, Running host.
#[test]
fn seat_admitted_into_a_running_catalog_level_receives_relevel_then_its_hint() {
    let mut relay = Relay::host();
    let points = spawn_points(3);
    relay.install(points.clone());
    relay.host_reveal();

    relay.admit(&[41]);
    let hint = relay.await_hint(41);

    let (life, _) = relevel_then_hint(&relay.naming(41));
    assert_eq!(hint.level_life, Some(life));
    assert_hint_names(&hint, &points, 1);
    assert_eq!(relay.placement(41), Some(1), "admission reserves the start");
}

// R9, host still Settling: the world-less poll admits the seat.
#[test]
fn seat_admitted_while_the_host_settles_receives_relevel_then_its_hint() {
    let mut relay = Relay::host();
    relay.world_less = true;
    let points = spawn_points(3);
    relay.install(points.clone());

    relay.admit(&[41]);
    let hint = relay.await_hint(41);

    relevel_then_hint(&relay.naming(41));
    assert_hint_names(&hint, &points, 1);
    assert_eq!(relay.placement(41), Some(1));
}

// R9, uncatalogued level, from both host polls.
#[test]
fn seat_admitted_into_an_uncatalogued_level_receives_the_hint_alone() {
    for world_less in [false, true] {
        let mut relay = Relay::host();
        relay.world_less = world_less;
        let points = spawn_points(3);
        relay.install_from(None, points.clone());
        let identity = relay.app.published_level_identity.clone().unwrap();

        relay.admit(&[41]);
        let hint = relay.await_hint(41);

        assert_eq!(relay.naming(41), [Naming::Hint(hint.clone())]);
        assert_eq!(hint.level_identity, identity);
        assert_eq!(hint.level_life, None, "an uncatalogued level has no life");
        assert_hint_names(&hint, &points, 1);
    }
}

// R10, O3.
#[test]
fn seat_admitted_while_the_host_loads_is_hinted_at_install_as_a_later_admission_would_be() {
    let points = spawn_points(3);
    let mut loading = Relay::host();
    // The previous level's starts differ from the next one's.
    let previous: Vec<MapEntity> = spawn_points(3)
        .into_iter()
        .map(|mut start| {
            start.origin += Vec3::X * 100.0;
            start
        })
        .collect();
    loading.install(previous);
    loading.unload();
    loading.world_less = true;

    loading.admit(&[41]);
    loading.settle();
    assert_eq!(loading.naming(41), [], "no level is named while loading");
    assert_eq!(loading.placement(41), None);

    loading.install(points.clone());
    let hint = loading.await_hint(41);
    relevel_then_hint(&loading.naming(41));

    let mut later = Relay::host();
    later.install(points.clone());
    later.admit(&[41]);
    let expected = later.await_hint(41);
    assert_eq!(pose(&hint), pose(&expected));
    assert_hint_names(&hint, &points, 1);
}

// R11, catalog and uncatalogued hosts.
#[test]
fn hint_precedes_the_participation_marker_for_a_client_that_reveals_after_admission() {
    for catalog_id in [Some(LEVEL), None] {
        let mut relay = Relay::host();
        relay.install_from(catalog_id, spawn_points(3));
        relay.host_reveal();
        relay.connect_as(41, 41);
        relay.pump_until(|relay| relay.seat(41).is_some());

        // Revealed at once, without waiting for anything the host sent.
        relay.reveal(41);
        let pawn = relay.await_pawn(41);
        relay.pump_until(|relay| relay.naming(41).iter().any(is_marker));

        let naming = relay.naming(41);
        assert!(
            position(&naming, is_hint) < position(&naming, is_marker),
            "{catalog_id:?}: the hint precedes the marker: {naming:?}"
        );
        assert_eq!(
            matches!(naming[0], Naming::Relevel(_)),
            catalog_id.is_some(),
            "{catalog_id:?}: Relevel leads on a catalog level only: {naming:?}"
        );
        relay.assert_pawn_at_hint(pawn, &relay.hints(41)[0]);
    }
}

// R12, O11: the host half.
#[test]
fn client_revealed_before_connecting_is_promoted_in_its_admission_batch_and_hinted_after_the_marker()
 {
    let mut relay = Relay::host();
    let points = spawn_points(3);
    relay.install_from(None, points.clone());
    relay.host_reveal();
    let identity = relay.app.published_level_identity.clone().unwrap();

    relay.connect_as(41, 41);
    relay.reveal_as(41, &identity, None);
    let pawn = relay.await_pawn(41);
    let hint = relay.await_hint(41);
    relay.pump_until(|relay| relay.naming(41).iter().any(is_marker));

    let naming = relay.naming(41);
    assert!(
        position(&naming, is_marker) < position(&naming, is_hint),
        "the marker went out inside the admission poll: {naming:?}"
    );
    assert_eq!(relay.hints(41).len(), 1);
    assert_hint_names(&hint, &points, 1);
    relay.assert_pawn_at_hint(pawn, &hint);
}

// R13. A seat an unbacked source would place gets no hint either: its pawn
// spawns by the default accept, never at that pose.
#[test]
fn level_without_player_spawn_sends_no_hint_and_its_joiners_spawn_as_today() {
    let mut relay = Relay::admitted(&[41]);
    let offered = relay.seat(41).unwrap();
    relay
        .app
        .spawn_poses
        .set_unbacked_source(Some(Box::new(OfferOne {
            seat: offered,
            spawn: scout_pose(),
            consumed: Default::default(),
        })));
    relay.install(Vec::new());
    relay.host_reveal();
    relay.admit(&[42]);

    relay.reveal(41);
    relay.reveal(42);
    relay.await_pawn(41);
    relay.await_pawn(42);
    relay.settle();

    assert!(matches!(relay.naming(41)[0], Naming::Relevel(_)));
    assert!(relay.hints(41).is_empty());
    assert!(relay.hints(42).is_empty());
}

fn present_frontend_menu(app: &mut App) {
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "mainMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session.as_mut().unwrap().frontend = Some(Frontend {
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
}

// M8, O19.
#[test]
fn host_frontend_backdrop_names_no_level_and_promotes_no_one_until_the_host_starts_one() {
    let mut relay = Relay::admitted(&[41]);
    present_frontend_menu(&mut relay.app);
    let points = spawn_points(3);

    relay.install(points.clone());
    relay.host_reveal();
    relay.reveal_as(41, LEVEL, None);
    relay.settle();

    assert_eq!(relay.app.published_level_identity, None);
    assert_eq!(relay.server().installed_level_life(), None);
    assert_eq!(relay.server().revealed_level(), None);
    assert_eq!(relay.naming(41), [], "no Relevel and no hint");
    assert!(!relay.participating(41), "the backdrop promotes no one");
    assert_eq!(relay.placement(41), None);

    // The host starts a level: its verb clears the menu, the backdrop unloads.
    relay
        .app
        .session
        .as_mut()
        .unwrap()
        .modal_stack
        .clear_pushed();
    relay.unload();
    relay.install(points.clone());

    let hint = relay.await_hint(41);
    relevel_then_hint(&relay.naming(41));
    assert_hint_names(&hint, &points, 1);
}

// M8 bounds: only a host's backdrop stops naming its level.
#[test]
fn a_connected_clients_backdrop_still_publishes_its_level_identity() {
    let mut app = test_app();
    connect_test_app_as_client(&mut app);
    present_frontend_menu(&mut app);
    app.level_load = Some(crate::startup::InFlightLevelLoad {
        map_path: PathBuf::from("content/dev/maps/spawn-arena.prl"),
        content_root: PathBuf::from("content/dev"),
        entry: crate::startup::LevelLoadEntry {
            catalog_id: Some(LEVEL.to_string()),
            path: "maps/spawn-arena.prl".to_string(),
            name: "Spawn arena".to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
        host_named: false,
    });

    app.install_level_identity(&level_world(LEVEL, 1));

    assert_eq!(app.published_level_identity.as_deref(), Some(LEVEL));
}
