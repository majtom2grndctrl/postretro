//! Seam checkpoints: the level-start save a seam load runs, Continue from it,
//! and the seam's spawn set, across runs, the frontend, suspend and a hosted
//! Continue.
//! See: context/lib/entity_model.md §7e · context/lib/scripting.md §5 ·
//! context/plans/in-progress/E25--landmark-level-change/research.md §6

use super::lifecycle_test_fixtures::{drop_in_flight_load, script_ctx};
use super::seam_test_fixtures::*;
use super::*;
use crate::session::saves::test_fixtures::{MOD_ID, app_with_store, boot, present_frontend};
use crate::startup::LevelRequest;
use postretro_entities::SystemReactionCommand;
use postretro_entities::reactions::system_commands::{FireOrigin, MachineSource};
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::Seat;
use postretro_physics::movement::seam_arrival::LandmarkRelative;
use postretro_scripting_core::runtime::SavesManifest;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_sim::scripting::save_files::{
    LedgerDocument, LevelRecord, SaveDir, SeamCheckpointRecord, SeamPoseRecord, read_document,
};
use postretro_test_log_capture::LogCapture;
use std::path::Path;

const SAVES: Option<SavesManifest> = Some(SavesManifest {
    character_slots: 2,
    run_slots: 2,
});

const INSIDE: Vec3 = Vec3::new(21.0, 1.2, 1.0);
const OUTSIDE: Vec3 = Vec3::new(-10.0, 1.2, -10.0);

/// The pose the host saves crossing at `INSIDE` with the camera at yaw 0.4,
/// pitch -0.2, through the source landmark at yaw 0.
fn saved_pose() -> SeamPoseRecord {
    SeamPoseRecord {
        offset: (INSIDE - SOURCE_LANDMARK).to_array(),
        view_yaw: 0.4,
        view_pitch: -0.2,
    }
}

fn origin() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}

fn verb(command: &str, index: usize, name: &str) -> SystemReactionCommand {
    let dispatch_source = "named:test".to_string();
    match command {
        "create" => SystemReactionCommand::CreateCharacter {
            index,
            dispatch_source,
            origin: origin(),
        },
        "newRun" => SystemReactionCommand::NewRun {
            index,
            map: name.into(),
            dispatch_source,
            origin: origin(),
        },
        "continue" => SystemReactionCommand::ContinueRun {
            index,
            dispatch_source,
            origin: origin(),
        },
        _ => SystemReactionCommand::Checkpoint {
            name: name.into(),
            dispatch_source,
            origin: origin(),
        },
    }
}

fn checkpoint(app: &mut App, name: &str) {
    app.handle_save_verb(verb("checkpoint", 0, name));
}

fn continue_run(app: &mut App) {
    app.handle_save_verb(verb("continue", 0, ""));
}

/// A booted mod with a `saves` block at its frontend, its catalog holding the
/// seam levels, with character Vex selected.
fn frontend(dir: &Path) -> App {
    let mut app = app_with_store(dir);
    boot(&mut app, SAVES);
    let ctx = script_ctx(&app);
    ctx.data_registry.borrow_mut().entities = descriptors();
    for id in [SOURCE, DESTINATION, THIRD] {
        ctx.data_registry
            .borrow_mut()
            .maps
            .push(postretro_foundation::ModMapEntry {
                id: id.into(),
                path: format!("maps/{id}-never-read.prl"),
                name: id.into(),
                tags: Vec::new(),
                loading_tree: Vec::new(),
            });
    }
    app.boot_state = BootState::Frontend;
    present_frontend(&mut app);
    ctx.slot_table
        .borrow_mut()
        .get_mut("ui.textEntry")
        .unwrap()
        .write_value(Some(SlotValue::String("Vex".into())));
    app.handle_save_verb(verb("create", 0, ""));
    app
}

fn source_world() -> postretro_level_loader::LevelWorld {
    seam_world(vec![source_door()], &spawn_points(2))
}

/// `count` starts at the destination, keyed per `keys` in order (`None` is
/// the level-start set).
fn keyed_points(keys: &[Option<&str>]) -> Vec<MapEntity> {
    let mut points = spawn_points(keys.len());
    for (point, key) in points.iter_mut().zip(keys) {
        if let Some(key) = key {
            point.key_values.insert("checkpoint".into(), (*key).into());
        }
    }
    points
}

fn destination_world(points: &[MapEntity]) -> postretro_level_loader::LevelWorld {
    seam_world(vec![destination_door()], points)
}

/// A run of Vex's started at the source and revealed there.
fn run_in_source(dir: &Path) -> App {
    let mut app = frontend(dir);
    app.handle_save_verb(verb("newRun", 0, SOURCE));
    drain_and_install(&mut app, source_world());
    reveal(&mut app);
    app
}

/// The host stands at `at` with the camera at yaw 0.4, pitch -0.2, and the
/// door's load drains: the unload harvests and the level-start save runs.
fn cross(app: &mut App, at: Vec3) {
    let host = local_pawn(app);
    place(app, host, at, 0.0, Vec3::new(3.0, 0.0, 0.0));
    app.camera.yaw = 0.4;
    app.camera.pitch = -0.2;
    request_seam(app, DESTINATION, DOOR);
    app.drain_level_requests();
}

fn run_file(dir: &Path) -> Option<LedgerDocument> {
    read_document(&SaveDir::new(dir, MOD_ID).run_path(0)).unwrap()
}

fn run_level(dir: &Path) -> LevelRecord {
    run_file(dir).unwrap().level.unwrap()
}

fn seam_level(pose: Option<SeamPoseRecord>) -> LevelRecord {
    LevelRecord {
        catalog_id: DESTINATION.into(),
        checkpoint: Some(DOOR.into()),
        seam: Some(SeamCheckpointRecord { pose }),
    }
}

/// Back at the frontend: the played level unloads and the run ends.
fn quit(app: &mut App) {
    app.navigate_to_frontend();
    app.drain_level_requests();
    app.drain_level_requests();
    assert!(app.at_frontend());
}

/// Where the saved pose arrives: at rest, through the destination landmark.
fn saved_body() -> Vec3 {
    destination_body(&LandmarkRelative::at_rest(
        Vec3::from(saved_pose().offset),
        0.4,
        -0.2,
    ))
}

#[track_caller]
fn assert_host_at_saved_pose(app: &mut App, what: &str) {
    let host = local_pawn(app);
    assert!(
        transform(app, host).position.distance(saved_body()) < 1e-4,
        "{what}: the host spawns at the saved pose"
    );
    assert_eq!(movement(app, host).velocity, Vec3::ZERO, "{what}: at rest");
    let (eye, yaw, pitch) = camera_pose(app);
    assert!(eye.distance(player_eye(saved_body())) < 1e-3, "{what}: eye");
    let expected = postretro_foundation::wrap_teleport_yaw(0.4 + DESTINATION_YAW);
    assert!((yaw - expected).abs() < 1e-5, "{what}: yaw");
    assert!((pitch + 0.2).abs() < 1e-6, "{what}: pitch");
    assert!(app.seam_stage.is_latched(host, 0), "{what}: latched");
    assert_eq!(
        app.seam_stage.take_arrivals(),
        [(0, host)],
        "{what}: `arrive` fires once for it"
    );
}

#[track_caller]
fn assert_host_on_start(app: &App, points: &[MapEntity], index: usize, what: &str) {
    let host = local_pawn(app);
    let standing = super::spawn_pose_handoff_fixtures::standing_on(points, index);
    assert!(
        transform(app, host).position.distance(standing) < 1e-4,
        "{what}: the host stands on start {index}, at {:?}",
        transform(app, host).position
    );
}

fn active_keys(app: &App) -> Vec<Option<String>> {
    app.host_spawn_points
        .iter()
        .map(|spawn| spawn.key_values.get("checkpoint").cloned())
        .collect()
}

// Ckpt 1 / P22 and Ckpt 12: a single-player crossing in a run saves the run at
// the destination with the seam's name and the pose in the landmark's frame.
// Continue spawns the player there at rest, latched, presenting the saved eye,
// yaw and pitch through Settling, and fires `arrive` once; a second Continue
// arrives there again.
#[test]
fn seam_crossing_saves_its_pose_and_continue_arrives_there_twice() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    assert_eq!(run_level(dir.path()), seam_level(Some(saved_pose())));
    install(&mut app, DESTINATION, destination_world(&spawn_points(2)));

    for round in ["first Continue", "second Continue"] {
        quit(&mut app);
        continue_run(&mut app);
        drain_and_install(&mut app, destination_world(&spawn_points(2)));
        assert_host_at_saved_pose(&mut app, round);
        reveal(&mut app);
        let (eye, ..) = camera_pose(&app);
        assert!(
            eye.distance(player_eye(saved_body())) < 1e-3,
            "{round}: reveal"
        );
        assert_eq!(
            run_level(dir.path()),
            seam_level(Some(saved_pose())),
            "{round}: Continue's level start keeps the seam checkpoint"
        );
    }
}

// Ckpt 2: Continue from a seam checkpoint whose pose is blocked, or into a
// level without that seam, places the player at a start with one warning.
#[test]
fn seam_continue_blocked_or_missing_seam_places_host_at_start_with_one_warning() {
    for case in ["missing", "blocked"] {
        let capture = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let mut app = run_in_source(dir.path());
        cross(&mut app, INSIDE);
        install(&mut app, DESTINATION, destination_world(&spawn_points(2)));
        quit(&mut app);
        continue_run(&mut app);
        let points = spawn_points(2);
        let world = match case {
            "missing" => seam_world(
                vec![seam("hatch", SOURCE, DESTINATION_LANDMARK, 0.0)],
                &points,
            ),
            _ => wall_at(destination_world(&points), saved_body()),
        };
        drain_and_install(&mut app, world);
        assert_host_on_start(&app, &points, 0, case);
        let warning = match case {
            "missing" => "seam `door`: this level has no seam of that name",
            _ => "seam `door` arrival blocked",
        };
        capture.assert_logged_once(log::Level::Warn, warning);
        assert!(
            app.seam_stage.take_arrivals().is_empty(),
            "{case}: no `arrive`"
        );
    }
}

// Ckpt 3 / P22: a seam load that fails after its level-start save leaves the
// run at the seam checkpoint, and Continue tries the saved pose.
#[test]
fn seam_load_failing_after_its_save_keeps_the_seam_checkpoint() {
    struct NoExit;
    impl crate::startup::LoopExit for NoExit {
        fn exit(&self) {
            panic!("a runtime load failure never exits the loop");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    app.finish_level_failure("prl parse failed".into(), &NoExit);
    assert_eq!(run_level(dir.path()), seam_level(Some(saved_pose())));

    present_frontend(&mut app);
    continue_run(&mut app);
    drain_and_install(&mut app, destination_world(&spawn_points(2)));
    assert_host_at_saved_pose(&mut app, "Continue after the failed load");
}

// Ckpt 4 and P26 (b): a host outside the seam while another seat crossed saves
// a seam-marked checkpoint with no pose: that seat's crossing writes none.
// Continue places the host in the seam's spawn set, silently when none is
// keyed, and fires no `arrive`.
#[test]
fn seam_host_outside_saves_a_seam_mark_without_pose_and_continue_uses_the_set() {
    for keyed in [true, false] {
        let capture = LogCapture::start();
        let dir = tempfile::tempdir().unwrap();
        let mut app = run_in_source(dir.path());
        let seat = app
            .session
            .as_mut()
            .unwrap()
            .seat_table
            .as_mut()
            .unwrap()
            .admit_or_reclaim(7, None, false)
            .unwrap()
            .seat;
        spawn_seat_pawn(&mut app, seat, Vec3::new(19.0, 1.2, -1.0), 0.2);
        cross(&mut app, OUTSIDE);
        assert!(
            app.seam_crossing()
                .is_some_and(|crossing| crossing.arrivals.contains_key(&seat)),
            "fixture: the other seat crossed"
        );
        assert_eq!(run_level(dir.path()), seam_level(None), "keyed: {keyed}");

        let points = if keyed {
            keyed_points(&[None, Some(DOOR), Some(DOOR)])
        } else {
            keyed_points(&[None, None])
        };
        install(&mut app, DESTINATION, destination_world(&points));
        quit(&mut app);
        continue_run(&mut app);
        drain_and_install(&mut app, destination_world(&points));
        assert_host_on_start(&app, &points, usize::from(keyed), "Continue");
        assert!(app.seam_stage.take_arrivals().is_empty());
        capture.assert_not_logged(log::Level::Warn, "names no player_spawn");
        capture.assert_not_logged(log::Level::Warn, "seam `door`");
    }
}

// Ckpt 5 / P23: after a crossing the seam's keyed spawns are the active set for
// respawn and late joiners; a later `checkpoint(name)` replaces it and the next
// level start clears it.
#[test]
fn seam_spawn_set_serves_until_a_later_checkpoint_or_level_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    let points = keyed_points(&[None, Some(DOOR), Some(DOOR), Some("bridge")]);
    install(&mut app, DESTINATION, destination_world(&points));
    assert_eq!(active_keys(&app), [Some(DOOR.into()), Some(DOOR.into())]);
    let published = script_ctx(&app).respawn_placements.borrow().map_order.len();
    assert_eq!(published, 2, "respawn chooses within the seam's set");
    reveal(&mut app);

    checkpoint(&mut app, "bridge");
    assert_eq!(active_keys(&app), [Some("bridge".into())]);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(THIRD.into())));
    drain_and_install(&mut app, destination_world(&points));
    assert_eq!(active_keys(&app), [None]);
    assert_eq!(app.session.as_ref().unwrap().saves.spawn_checkpoint(), None);
}

// P25: a crossing outside a run, and one whose store cannot save, still makes
// the seam's set the active set, silently when none is keyed, and writes no
// run file.
#[test]
fn seam_crossing_without_a_run_still_selects_the_seam_set() {
    for keyed in [true, false] {
        let capture = LogCapture::start();
        let points = if keyed {
            keyed_points(&[None, Some(DOOR)])
        } else {
            keyed_points(&[None, None])
        };
        // No run: a level loaded outside one.
        let dir = tempfile::tempdir().unwrap();
        let mut app = frontend(dir.path());
        app.session.as_mut().unwrap().modal_stack.clear_pushed();
        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(SOURCE.into())));
        drain_and_install(&mut app, source_world());
        reveal(&mut app);
        cross(&mut app, INSIDE);
        install(&mut app, DESTINATION, destination_world(&points));
        assert!(run_file(dir.path()).is_none(), "no run file is written");
        let expected = if keyed {
            vec![Some(DOOR.to_string())]
        } else {
            vec![None, None]
        };
        assert_eq!(active_keys(&app), expected, "no run, keyed: {keyed}");

        // No store at all.
        let mut app = seam_app();
        install(&mut app, SOURCE, source_world());
        reveal(&mut app);
        cross(&mut app, INSIDE);
        install(&mut app, DESTINATION, destination_world(&points));
        assert_eq!(active_keys(&app), expected, "no store, keyed: {keyed}");
        assert!(
            app.session
                .as_ref()
                .unwrap()
                .saves
                .spawn_checkpoint_is_seam(),
            "the checkpoint is the seam's"
        );
        capture.assert_not_logged(log::Level::Warn, "names no player_spawn");
    }
}

// P26 (a): the silent fallback lasts as long as the seam's set; a later
// `checkpoint(name)` that keys no spawn warns as it does today.
#[test]
fn seam_silent_fallback_ends_at_a_later_unknown_checkpoint() {
    let capture = LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    install(&mut app, DESTINATION, destination_world(&spawn_points(2)));
    reveal(&mut app);
    capture.assert_not_logged(log::Level::Warn, "names no player_spawn");
    checkpoint(&mut app, "typo");
    capture.assert_logged_once(log::Level::Warn, "checkpoint `typo` names no player_spawn");
}

// P27: a later `checkpoint(name)` (another name, or the seam's own), or a
// `restartLevel`, outlives the seam pose: Continue then spawns in that set or
// the level-start set, never at the pose.
#[test]
fn seam_pose_is_outlived_by_a_later_checkpoint_or_restart() {
    let points = keyed_points(&[None, Some(DOOR), Some("bridge")]);
    for later in ["bridge", DOOR, "restartLevel"] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = run_in_source(dir.path());
        cross(&mut app, INSIDE);
        install(&mut app, DESTINATION, destination_world(&points));
        reveal(&mut app);
        if later == "restartLevel" {
            script_ctx(&app)
                .system_commands
                .push(SystemReactionCommand::RestartLevel {
                    dispatch_source: "named:test".into(),
                });
            app.dispatch_system_commands();
            drain_and_install(&mut app, destination_world(&points));
        } else {
            checkpoint(&mut app, later);
        }
        let level = run_level(dir.path());
        assert_eq!(level.seam, None, "{later}: the seam mark and pose are gone");

        quit(&mut app);
        continue_run(&mut app);
        drain_and_install(&mut app, destination_world(&points));
        let start = match later {
            "bridge" => 2,
            "restartLevel" => 0,
            _ => 1,
        };
        assert_host_on_start(&app, &points, start, later);
        assert!(app.seam_stage.take_arrivals().is_empty(), "{later}");
    }
}

// Ckpt 7 / P24: Continue over an installed frontend backdrop spawns the player
// at the saved pose: the backdrop's unload clears no pose Continue set.
#[test]
fn seam_continue_over_a_backdrop_keeps_the_saved_pose() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    install(&mut app, DESTINATION, destination_world(&spawn_points(2)));
    quit(&mut app);
    install(&mut app, SOURCE, source_world());
    assert!(app.backdrop_installed, "fixture: a backdrop under the menu");

    continue_run(&mut app);
    drain_and_install(&mut app, destination_world(&spawn_points(2)));
    assert_host_at_saved_pose(&mut app, "Continue over a backdrop");
}

// Ckpt 11 / P28: a suspend before the seam's load drains leaves the run at the
// source; one while the destination loads leaves it at the seam, and Continue
// after the resume arrives at the saved pose.
#[test]
fn seam_suspend_then_continue_resumes_where_the_run_saved() {
    // (a) Before the drain.
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    request_seam(&mut app, DESTINATION, DOOR);
    app.suspend_platform();
    assert_eq!(run_level(dir.path()).catalog_id, SOURCE);
    app.boot_state = BootState::Frontend;
    present_frontend(&mut app);
    continue_run(&mut app);
    assert_eq!(
        app.level_requests.back(),
        Some(&LevelRequest::Load(LevelSource::Catalog(SOURCE.into())))
    );

    // (b) While the destination loads.
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    assert!(app.level_load_in_flight());
    app.suspend_platform();
    drop_in_flight_load(&mut app);
    assert_eq!(run_level(dir.path()), seam_level(Some(saved_pose())));
    app.boot_state = BootState::Frontend;
    present_frontend(&mut app);
    continue_run(&mut app);
    drain_and_install(&mut app, destination_world(&spawn_points(2)));
    assert_host_at_saved_pose(&mut app, "Continue after a suspend");
}

// Ckpt 14 / P32: a `checkpoint(name)` on the seam's activation frame, before or
// after the activation, leaves the run at the destination with the seam's
// checkpoint and pose, and the destination's set is the seam's.
#[test]
fn seam_checkpoint_on_the_activation_frame_ends_at_the_seam() {
    for checkpoint_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = run_in_source(dir.path());
        let host = local_pawn(&app);
        place(&app, host, INSIDE, 0.0, Vec3::ZERO);
        app.camera.yaw = 0.4;
        app.camera.pitch = -0.2;
        if checkpoint_first {
            checkpoint(&mut app, "bridge");
            trigger_tick(&mut app);
        } else {
            trigger_tick(&mut app);
            checkpoint(&mut app, "bridge");
        }
        app.pick_seam_level_request();
        app.drain_level_requests();
        assert_eq!(
            run_level(dir.path()),
            seam_level(Some(saved_pose())),
            "checkpoint first: {checkpoint_first}"
        );
        let points = keyed_points(&[None, Some(DOOR), Some("bridge")]);
        install(&mut app, DESTINATION, destination_world(&points));
        assert_eq!(active_keys(&app), [Some(DOOR.into())]);
    }
}

// Act 3 / P31 (save half): under the host's pushed pause menu the destination
// is a played level and its level-start save records the seam; under the
// pushed frontend root no run save is written.
#[test]
fn seam_save_under_a_pushed_tree_follows_the_menu() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    {
        let session = app.session.as_mut().unwrap();
        session.modal_stack.registry_mut().register(
            "pauseMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
        session.modal_stack.push_named("pauseMenu", None);
    }
    cross(&mut app, INSIDE);
    assert_eq!(run_level(dir.path()), seam_level(Some(saved_pose())));

    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    let before = run_level(dir.path());
    present_frontend(&mut app);
    cross(&mut app, INSIDE);
    assert_eq!(
        run_level(dir.path()),
        before,
        "no run save under the frontend root"
    );
}

// Ckpt 13 / P29: a hosted Continue from a seam checkpoint places a client
// connected at the frontend, and one joining after the host's reveal, in the
// seam's spawn set; neither takes seat 0's saved pose, and that pose reserves
// no start.
#[test]
fn seam_hosted_continue_places_clients_in_the_seam_set_never_at_the_pose() {
    let capture = LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_source(dir.path());
    cross(&mut app, INSIDE);
    install(&mut app, DESTINATION, destination_world(&spawn_points(2)));
    quit(&mut app);

    let mut relay = super::spawn_pose_handoff_fixtures::Relay::host_on(app);
    relay.admit(&[41]);
    continue_run(&mut relay.app);
    relay.app.drain_level_requests();
    drop_in_flight_load(&mut relay.app);
    let points = keyed_points(&[None, Some(DOOR), Some(DOOR)]);
    let (host, _) = relay
        .install_seam_level(DESTINATION, destination_world(&points), points.clone())
        .unwrap();
    assert!(
        relay.transform(host).position.distance(saved_body()) < 1e-4,
        "the host spawns at the saved pose"
    );
    let first = relay.await_hint(41);
    relay.reveal(41);
    relay.host_reveal();
    let first_pawn = relay.await_pawn(41);
    relay.assert_pawn_at_hint(first_pawn, &first);

    relay.admit(&[42]);
    let second = relay.await_hint(42);
    relay.reveal(42);
    let second_pawn = relay.await_pawn(42);
    relay.assert_pawn_at_hint(second_pawn, &second);

    let door_starts: Vec<Vec3> = [1, 2]
        .iter()
        .map(|&index| super::spawn_pose_handoff_fixtures::standing_on(&points, index))
        .collect();
    for pawn in [first_pawn, second_pawn] {
        let at = relay.transform(pawn).position;
        assert!(
            door_starts.iter().any(|start| start.distance(at) < 1e-4),
            "a client stands on a door start, at {at:?}"
        );
        assert!(at.distance(saved_body()) > 1.0, "never at seat 0's pose");
    }
    assert!(
        relay
            .transform(first_pawn)
            .position
            .distance(relay.transform(second_pawn).position)
            > 1.0,
        "the pose reserves no start: the clients take both door starts"
    );
    assert_eq!(
        relay.seats().placement_for_seat(Seat(0)),
        None,
        "seat 0's saved pose holds no start"
    );
    capture.assert_not_logged(log::Level::Warn, "every player_spawn placement is occupied");
}
