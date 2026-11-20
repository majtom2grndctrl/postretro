//! The host's seam arrival at install: the mapped pose, its fallbacks, the
//! latches and `arrive` it takes, a self-targeting seam, and the class it
//! spawns as.
//! See: context/lib/entity_model.md §7e · context/lib/boot_sequence.md §4 ·
//! context/plans/in-progress/E25--landmark-level-change/research.md §6

use super::seam_test_fixtures::*;
use super::spawn_pose_handoff_fixtures::standing_on;
use super::*;
use crate::startup::LevelRequest;
use postretro_level_format::seams::SeamRecord;
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_physics::movement::seam_arrival::{Landmark, LandmarkRelative};
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_test_log_capture::LogCapture;

const INSIDE: Vec3 = Vec3::new(21.0, 1.2, 1.0);

/// The host crossed the source door at `INSIDE`, turned and moving: the
/// destination load is in flight, the crossing harvested. Returns the motion
/// it carries.
fn crossing(seams: Vec<SeamRecord>) -> (App, LandmarkRelative) {
    let mut app = seam_app();
    install(&mut app, SOURCE, seam_world(seams, &spawn_points(2)));
    reveal(&mut app);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.3, Vec3::new(2.0, 0.0, -1.0));
    app.camera.yaw = 0.4;
    app.camera.pitch = -0.2;
    let motion = LandmarkRelative::capture(
        &movement(&app, host),
        INSIDE,
        0.4,
        -0.2,
        Landmark {
            origin: SOURCE_LANDMARK,
            yaw: 0.0,
        },
    );
    let target = app.seam_stage.record(0).unwrap().target.clone();
    request_seam(&mut app, &target, DOOR);
    app.drain_level_requests();
    (app, motion)
}

#[track_caller]
fn assert_host_on_start(app: &App, points: &[MapEntity], what: &str) {
    let host = local_pawn(app);
    assert!(
        transform(app, host)
            .position
            .distance(standing_on(points, 0))
            < 1e-4,
        "{what}: the host stands on its start"
    );
    let (eye, ..) = camera_pose(app);
    assert!(
        eye.distance(player_eye(standing_on(points, 0))) < 1e-3,
        "{what}: and presents it"
    );
}

// Arr 4: a destination with no seam of the crossing's name, and an arrival
// blocked at spawn, each place the host at a player start with one warning
// and fire no `arrive`.
#[test]
fn seam_arrival_missing_seam_or_blocked_places_host_at_start_with_one_warning() {
    for case in ["missing", "blocked"] {
        let capture = LogCapture::start();
        let (mut app, motion) = crossing(vec![source_door()]);
        let points = spawn_points(2);
        let world = match case {
            "missing" => seam_world(
                vec![seam("hatch", SOURCE, DESTINATION_LANDMARK, 0.0)],
                &points,
            ),
            _ => wall_at(
                seam_world(vec![destination_door()], &points),
                destination_body(&motion),
            ),
        };
        install(&mut app, DESTINATION, world);
        assert_host_on_start(&app, &points, case);
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

// Arr 8: a disarmed destination seam places the crossing pawn landmark-
// relative, latches it and queues `arrive`, exactly as an armed one does.
#[test]
fn seam_disarmed_destination_still_places_latches_and_queues_arrive() {
    let (mut app, motion) = crossing(vec![source_door()]);
    let mut door = destination_door();
    door.enabled_on_spawn = false;
    install(
        &mut app,
        DESTINATION,
        seam_world(vec![door], &spawn_points(2)),
    );
    let host = local_pawn(&app);
    assert!(
        !app.seam_stage.armed(&script_ctx_registry(&app), 0),
        "fixture"
    );
    assert!(
        transform(&app, host)
            .position
            .distance(destination_body(&motion))
            < 1e-4
    );
    assert!(app.seam_stage.is_latched(host, 0));
    assert_eq!(app.seam_stage.take_arrivals(), [(0, host)]);
}

fn script_ctx_registry(app: &App) -> std::cell::Ref<'_, postretro_entities::EntityRegistry> {
    let session = app.session.as_ref().unwrap();
    session.scripting.script_ctx.registry.borrow()
}

// Arr 5: an arriving pawn overlapping an armed teleporter in the next map is
// latched against it, so it does not teleport.
#[test]
fn seam_arrival_latches_against_an_armed_teleporter() {
    let (mut app, motion) = crossing(vec![source_door()]);
    let body = destination_body(&motion);
    let mut world = seam_world(vec![destination_door()], &spawn_points(2));
    world.teleporters = TeleportersSection {
        teleporters: vec![TeleporterRecord {
            source_entity: 300,
            mode: TeleportMode::Classic,
            enabled_on_spawn: true,
            brushes: vec![box_brush(body - Vec3::splat(2.0), body + Vec3::splat(2.0))],
            destination_origin: [200.0, 0.0, 200.0],
            destination_yaw: 0.0,
            landmark_origin: [0.0; 3],
            landmark_yaw: 0.0,
            arrival_sound: String::new(),
            on_teleport: String::new(),
        }],
    };
    install(&mut app, DESTINATION, world);
    let host = local_pawn(&app);
    assert!(transform(&app, host).position.distance(body) < 1e-4);
    assert_eq!(
        movement(&app, host).teleport.arrival_latches,
        [0],
        "latched against the teleporter its body point stands in"
    );
}

// Latch 7 / P10: a seam targeting its own map reloads it once, and each
// occupant spawns where it stood, facing and moving as it was, latched.
#[test]
fn seam_targeting_its_own_map_reinstalls_the_host_where_it_stood() {
    let own = seam(DOOR, SOURCE, SOURCE_LANDMARK, 0.0);
    let (mut app, _) = crossing(vec![own.clone()]);
    assert!(app.level_requests.is_empty(), "one load");
    install(&mut app, SOURCE, seam_world(vec![own], &spawn_points(2)));
    let host = local_pawn(&app);
    assert!(transform(&app, host).position.distance(INSIDE) < 1e-4);
    assert!(
        movement(&app, host)
            .velocity
            .distance(Vec3::new(2.0, 0.0, -1.0))
            < 1e-4
    );
    let (_, yaw, pitch) = camera_pose(&app);
    assert!((yaw - 0.4).abs() < 1e-5 && (pitch + 0.2).abs() < 1e-6);
    assert!(app.seam_stage.is_latched(host, 0));
    assert!(app.level_requests.is_empty(), "no second load");
}

// Comp 2: the arrival spawns as the class of the destination start the seat
// would otherwise take.
#[test]
fn seam_arrival_spawns_as_the_class_of_the_start_it_replaces() {
    let (mut app, motion) = crossing(vec![source_door()]);
    let mut points = spawn_points(2);
    points[0]
        .key_values
        .insert("entity_class".into(), "scout".into());
    install(
        &mut app,
        DESTINATION,
        seam_world(vec![destination_door()], &points),
    );
    let host = local_pawn(&app);
    assert!(
        (movement(&app, host).capsule.radius - 0.3).abs() < 1e-6,
        "a scout"
    );
    assert!(
        transform(&app, host)
            .position
            .distance(destination_body(&motion))
            < 1e-4
    );
}

// Carry 2 / P2: the first spawn consumes the arrival; the level after a
// non-seam change places the host at its start with no warning, though it
// holds a seam of the crossing's name. No respawn path reads an arrival: the
// install took it.
#[test]
fn seam_arrival_consumed_then_next_level_starts_silently() {
    let capture = LogCapture::start();
    let (mut app, motion) = crossing(vec![source_door()]);
    install(
        &mut app,
        DESTINATION,
        seam_world(vec![destination_door()], &spawn_points(2)),
    );
    let host = local_pawn(&app);
    assert!(
        transform(&app, host)
            .position
            .distance(destination_body(&motion))
            < 1e-4
    );
    assert!(app.seam_crossing().is_none(), "consumed by the first spawn");
    reveal(&mut app);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(THIRD.into())));
    let points = spawn_points(2);
    drain_and_install(&mut app, seam_world(vec![destination_door()], &points));
    assert_host_on_start(&app, &points, "the next level");
    assert!(app.seam_stage.take_arrivals().is_empty());
    capture.assert_not_logged(log::Level::Warn, "seam `door`");
}
