//! Level seam level changes at the App: the arrival harvest at unload, what
//! clears an arrival, and the frame-end pick among a frame's activations and
//! script level verbs.
//! See: context/lib/entity_model.md §7e · context/lib/boot_sequence.md §4 ·
//! context/plans/in-progress/E25--landmark-level-change/research.md §6

use super::lifecycle_test_fixtures::script_ctx;
use super::seam_test_fixtures::*;
use super::*;
use crate::startup::LevelRequest;
use postretro_combat_model::CarriedState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::reactions::system_commands::{FireOrigin, MachineSource};
use postretro_entities::{AmmoReserve, SeamComponent, SystemReactionCommand};
use postretro_foundation::Seat;
use postretro_physics::movement::seam_arrival::{Landmark, LandmarkRelative};
use postretro_test_log_capture::LogCapture;

struct NoExit;

impl crate::startup::LoopExit for NoExit {
    fn exit(&self) {
        panic!("a runtime load failure never exits the loop");
    }
}

/// A host standing inside the source door, turned and moving.
const INSIDE: Vec3 = Vec3::new(21.0, 1.2, 1.0);
const OUTSIDE: Vec3 = Vec3::new(-10.0, 1.2, -10.0);

/// The source level installed and revealed: the host at its start.
fn in_source(seams: Vec<postretro_level_format::seams::SeamRecord>) -> App {
    let mut app = seam_app();
    install(&mut app, SOURCE, seam_world(seams, &spawn_points(2)));
    reveal(&mut app);
    app
}

fn seam_requests(app: &App) -> Vec<LevelRequest> {
    app.level_requests.iter().cloned().collect()
}

fn seam_request(target: &str, seam: &str) -> LevelRequest {
    LevelRequest::Seam {
        catalog_id: target.into(),
        seam: seam.into(),
    }
}

/// A second movement pawn, seated nowhere, standing at `position`.
fn bystander(app: &mut App, position: Vec3) -> postretro_entities::EntityId {
    spawn_seat_pawn(app, Seat(9), position, 0.0)
}

// Arr 2 (host), Arr 6 and Carry 2's consumption: the harvest reads the host's
// camera view and its pawn's motion before any teardown, and its first spawn in
// the destination takes them through the destination landmark; Settling and
// the reveal present the arrival eye, yaw and pitch.
#[test]
fn seam_crossing_arrives_host_at_mapped_pose_view_and_motion() {
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.3, Vec3::new(2.0, 0.0, -1.0));
    app.camera.yaw = 0.4;
    app.camera.pitch = -0.2;
    let expected = LandmarkRelative::capture(
        &movement(&app, host),
        INSIDE,
        0.4,
        -0.2,
        Landmark {
            origin: SOURCE_LANDMARK,
            yaw: 0.0,
        },
    );

    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, DOOR)]);
    app.drain_level_requests();
    let crossing = app
        .seam_crossing()
        .expect("the unload harvested the crossing");
    assert_eq!(crossing.arrivals.get(&Seat(0)), Some(&expected));

    install(
        &mut app,
        DESTINATION,
        seam_world(vec![destination_door()], &spawn_points(2)),
    );
    let host = local_pawn(&app);
    let body = destination_body(&expected);
    assert!(
        transform(&app, host).position.distance(body) < 1e-4,
        "the host arrives landmark-relative, vertical included"
    );
    let arrived = movement(&app, host);
    let rotated = glam::Quat::from_rotation_y(DESTINATION_YAW) * Vec3::new(2.0, 0.0, -1.0);
    assert!(
        arrived.velocity.distance(rotated) < 1e-4,
        "velocity turns by the landmark yaw: {:?}",
        arrived.velocity
    );
    let yaw = postretro_foundation::wrap_teleport_yaw(0.4 + DESTINATION_YAW);
    for stage in ["install", "reveal"] {
        let (eye, camera_yaw, pitch) = camera_pose(&app);
        assert!(
            eye.distance(player_eye(body)) < 1e-3,
            "{stage}: camera at the arrival eye {eye:?}"
        );
        assert!((camera_yaw - yaw).abs() < 1e-5, "{stage}: arrival yaw");
        assert!((pitch + 0.2).abs() < 1e-6, "{stage}: arrival pitch");
        if stage == "install" {
            reveal(&mut app);
        }
    }
    assert!(
        app.seam_crossing().is_none(),
        "the first spawn consumed the crossing"
    );
    assert!(app.seam_stage.is_latched(host, 0), "the arrival latches");
    assert_eq!(app.seam_stage.take_arrivals(), [(0, host)]);
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .seat_table
            .as_ref()
            .unwrap()
            .placement_for_seat(Seat(0)),
        None,
        "the arrival holds no player start"
    );
}

// Arr 2 (client), Arr 9 / P8 and Coop 4: a client's view is what its
// host-side pawn faces, its latest resolved facing turned into the pawn's
// teleport frame though its commands still carry the pre-teleport stamp; a
// client with no resolved command takes its pawn's yaw at pitch 0. Both are
// read before the unload resets the command queues.
#[test]
fn seam_harvest_reads_client_facing_in_its_teleport_frame_before_unload() {
    let mut app = in_source(vec![source_door()]);
    let endpoint =
        crate::netcode::NetEndpoint::from_role(&crate::netcode::NetRole::Host { port: 0 }, None)
            .unwrap()
            .unwrap();
    {
        let session = app.session.as_mut().unwrap();
        session.net_endpoint = Some(endpoint);
        let mut seats = crate::netcode::SeatTable::new().unwrap();
        assert_eq!(
            seats.admit_or_reclaim(7, None, false).unwrap().seat,
            Seat(1)
        );
        assert_eq!(
            seats.admit_or_reclaim(8, None, false).unwrap().seat,
            Seat(2)
        );
        session.seat_table = Some(seats);
    }
    let turned = spawn_seat_pawn(&mut app, Seat(1), Vec3::new(19.0, 1.2, -1.0), 0.9);
    let silent = spawn_seat_pawn(&mut app, Seat(2), Vec3::new(22.0, 1.2, 2.0), -0.6);
    place(
        &app,
        turned,
        Vec3::new(19.0, 1.2, -1.0),
        0.9,
        Vec3::new(0.0, 0.0, 3.0),
    );
    // The pawn teleported after the commands below were sampled.
    {
        let ctx = script_ctx(&app);
        let mut registry = ctx.registry.borrow_mut();
        let mut moved = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(turned)
            .unwrap()
            .clone();
        moved.teleport.accumulated_yaw = 0.5;
        registry.set_component(turned, moved).unwrap();
    }
    {
        let Some(crate::netcode::NetEndpoint::Host { command_queues, .. }) =
            app.session.as_mut().unwrap().net_endpoint.as_mut()
        else {
            unreachable!()
        };
        for tick in 1..=3 {
            assert!(command_queues.ingest_for_test(7, &client_command(tick, 1.0, 0.25, 0.2)));
        }
        let mut owners = crate::netcode::MovementOwners::default();
        owners.set_for_test(turned, 7);
        assert_eq!(
            crate::netcode::host_resolve_remote_commands(&owners, command_queues).len(),
            1,
            "fixture: client 7's first command resolves"
        );
    }

    app.unload_level_crossing(Some(DOOR));

    let crossing = app.seam_crossing().expect("a seam unload harvests");
    let turned_arrival = &crossing.arrivals[&Seat(1)];
    assert!(
        (turned_arrival.view_yaw - 1.3).abs() < 1e-5,
        "facing 1.0 sampled at stamp 0.2, pawn now at 0.5: {}",
        turned_arrival.view_yaw
    );
    assert!((turned_arrival.view_pitch - 0.25).abs() < 1e-6);
    assert!(
        turned_arrival.velocity.distance(Vec3::new(0.0, 0.0, 3.0)) < 1e-5,
        "the client's own velocity, not a reset"
    );
    let silent_arrival = &crossing.arrivals[&Seat(2)];
    assert!((silent_arrival.view_yaw + 0.6).abs() < 1e-5);
    assert_eq!(silent_arrival.view_pitch, 0.0);
    let _ = silent;
}

fn client_command(
    client_tick: u32,
    facing_yaw: f32,
    aim_pitch: f32,
    stamp: f32,
) -> postretro_net::wire::InputCommand {
    use postretro_net::wire;
    wire::InputCommand {
        teleport_yaw_stamp: stamp,
        client_tick,
        movement: wire::WireMovementInput {
            assist_held: false,
            wish_dir: [0.0; 2],
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw,
            use_pressed: false,
            drop_pressed: false,
            aim_pitch,
            firing_slot: 0,
        },
        fire_button: wire::WireFireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: wire::WireFireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        activation: Default::default(),
    }
}

// Carry 3 / P1: the harvest's inside test decides who arrives, never the
// activation: the host activated the seam, then left it before the drain, and
// arrives at a player start with no warning.
#[test]
fn seam_activator_outside_at_harvest_arrives_at_player_start() {
    let capture = LogCapture::start();
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    place(&app, host, OUTSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, DOOR)]);

    app.drain_level_requests();
    assert!(
        app.seam_crossing().unwrap().arrivals.is_empty(),
        "nobody inside at the harvest"
    );
    let points = spawn_points(2);
    install(
        &mut app,
        DESTINATION,
        seam_world(vec![destination_door()], &points),
    );
    let host = local_pawn(&app);
    assert!(
        transform(&app, host)
            .position
            .distance(super::spawn_pose_handoff_fixtures::standing_on(&points, 0))
            < 1e-4,
        "the host stands on the first start"
    );
    assert!(app.seam_stage.take_arrivals().is_empty(), "no `arrive`");
    capture.assert_not_logged(log::Level::Warn, "seam `door`");
}

/// Seat 0's pawn carrying `health` and 9 shells, standing inside the door.
fn carrying_host(app: &mut App, health_now: f32) {
    let host = local_pawn(app);
    place(app, host, INSIDE, 0.0, Vec3::ZERO);
    let ctx = script_ctx(app);
    let mut registry = ctx.registry.borrow_mut();
    let mut health =
        HealthComponent::from_descriptor(&postretro_entities::data_descriptors::HealthDescriptor {
            max: 100.0,
            ..Default::default()
        });
    health.current = health_now;
    registry.set_component(host, health).unwrap();
    let mut reserve = AmmoReserve::new();
    reserve.set_exact("shells", 9);
    registry.set_component(host, reserve).unwrap();
}

fn carried(app: &App) -> Option<CarriedState> {
    app.session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .carried_state(Seat(0))
        .cloned()
}

// Carry 1 and Arr 7: a seam's unload carries each seat's state exactly as
// `loadLevel`'s does, a pawn at zero health included, which still crosses
// landmark-relative; the crossing changes where the seat arrives, never its
// life or what it carries.
#[test]
fn seam_crossing_carries_seat_state_as_load_level_does() {
    for health in [37.0, 0.0] {
        let mut by_seam = in_source(vec![source_door()]);
        carrying_host(&mut by_seam, health);
        let mut by_load = in_source(vec![source_door()]);
        carrying_host(&mut by_load, health);

        by_seam.unload_level_crossing(Some(DOOR));
        by_load.unload_level();

        assert!(
            by_seam
                .seam_crossing()
                .unwrap()
                .arrivals
                .contains_key(&Seat(0)),
            "health {health}: seat 0 arrives landmark-relative"
        );
        let carried_by_seam = carried(&by_seam).expect("the seam unload harvests seat 0");
        assert_eq!(
            Some(carried_by_seam.clone()),
            carried(&by_load),
            "health {health}"
        );
        assert_eq!(carried_by_seam.reserve.available("shells"), 9);
    }
}

// Carry 4 / P12: an arrival belongs to the load that carried its seam. A seam
// load that fails, that a queued unload cancels, or that a later load
// supersedes, leaves none: the next level places the host at a start with no
// warning, though it holds a seam of the crossing's name.
#[test]
fn seam_load_failure_cancel_or_supersede_leaves_no_arrival() {
    for ending in ["fails", "unload cancels", "load supersedes"] {
        let capture = LogCapture::start();
        let mut app = in_source(vec![source_door()]);
        let host = local_pawn(&app);
        place(&app, host, INSIDE, 0.0, Vec3::ZERO);
        request_seam(&mut app, DESTINATION, DOOR);
        app.drain_level_requests();
        assert!(
            app.seam_crossing().is_some_and(|c| !c.arrivals.is_empty()),
            "{ending}: the unload harvested an arrival"
        );
        match ending {
            "fails" => app.finish_level_failure("prl parse failed".into(), &NoExit),
            "unload cancels" => {
                app.enqueue_level_request(LevelRequest::Unload);
                app.drain_level_requests();
            }
            _ => {
                app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(THIRD.into())));
                app.drain_level_requests();
            }
        }
        assert!(
            app.seam_crossing().is_none(),
            "{ending}: no arrival survives"
        );

        let points = spawn_points(2);
        install(
            &mut app,
            DESTINATION,
            seam_world(vec![destination_door()], &points),
        );
        let host = local_pawn(&app);
        assert!(
            transform(&app, host)
                .position
                .distance(super::spawn_pose_handoff_fixtures::standing_on(&points, 0))
                < 1e-4,
            "{ending}: the host stands on its start"
        );
        capture.assert_not_logged(log::Level::Warn, "seam `door`");
    }
}

// Carry 5 / P6 (a) and (b): a suspend before the seam's load drains drops the
// queued request; a suspend while it loads clears the arrival it harvested.
#[test]
fn seam_suspend_before_drain_or_during_load_leaves_no_arrival() {
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    request_seam(&mut app, DESTINATION, DOOR);
    app.suspend_platform();
    assert!(app.level_requests.is_empty(), "the queued seam load drops");
    assert!(app.seam_crossing().is_none());

    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    request_seam(&mut app, DESTINATION, DOOR);
    app.drain_level_requests();
    assert!(app.seam_crossing().is_some());
    app.suspend_platform();
    assert!(
        app.seam_crossing().is_none(),
        "the cancelled load's arrival clears"
    );
}

fn script_verb(verb: &str) -> SystemReactionCommand {
    let dispatch_source = "named:test".to_string();
    match verb {
        "loadLevel" => SystemReactionCommand::LoadLevel {
            map: THIRD.into(),
            dispatch_source,
        },
        "restartLevel" => SystemReactionCommand::RestartLevel { dispatch_source },
        _ => SystemReactionCommand::ReturnToFrontend {
            dispatch_source,
            origin: FireOrigin::EveryMachine(MachineSource::OwnState),
        },
    }
}

// Act 4 / P3: a script `loadLevel`, `restartLevel` or `returnToFrontend` on the
// seam's activation frame, before or after the activation, is the frame's
// level change; the seam requests nothing and its unload writes no arrival.
#[test]
fn seam_activation_loses_to_a_script_level_verb_on_the_same_frame() {
    for verb in ["loadLevel", "restartLevel", "returnToFrontend"] {
        for script_first in [true, false] {
            let what = format!("{verb}, script first: {script_first}");
            let mut app = in_source(vec![source_door()]);
            let host = local_pawn(&app);
            place(&app, host, INSIDE, 0.0, Vec3::ZERO);
            let ctx = script_ctx(&app);
            if script_first {
                ctx.system_commands.push(script_verb(verb));
                app.dispatch_system_commands();
                trigger_tick(&mut app);
            } else {
                trigger_tick(&mut app);
                ctx.system_commands.push(script_verb(verb));
                app.dispatch_system_commands();
            }
            app.pick_seam_level_request();
            let requests = seam_requests(&app);
            assert!(
                !requests
                    .iter()
                    .any(|request| matches!(request, LevelRequest::Seam { .. })),
                "{what}: the seam's request drops: {requests:?}"
            );
            assert!(!requests.is_empty(), "{what}: the script request proceeds");
            app.drain_level_requests();
            app.drain_level_requests();
            assert!(
                app.seam_crossing().is_none(),
                "{what}: no arrival is written"
            );
        }
    }
}

// Regression: a quit to the menu on the same frame as a seam activation lost
// to the seam, whose target then installed as the frontend backdrop.
#[test]
fn seam_activation_loses_to_a_same_frame_quit_to_menu() {
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    app.quit_to_menu();
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    let requests = seam_requests(&app);
    assert!(
        !requests
            .iter()
            .any(|request| matches!(request, LevelRequest::Seam { .. })),
        "the seam's request drops: {requests:?}"
    );
}

// A script level verb beats only its own frame's seams: one that never
// reached a pick is cleared at the next redraw boundary.
#[test]
fn seam_script_request_note_clears_at_the_next_boundary() {
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    app.note_script_level_request();
    app.drain_level_requests();
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert!(
        seam_requests(&app)
            .iter()
            .any(|request| matches!(request, LevelRequest::Seam { .. })),
        "the stale note no longer drops this frame's seam"
    );
}

/// The source level with seams `first` (target `first_target`) and `second`
/// (target `THIRD`) side by side, the host in neither.
fn two_seams(first_target: &str) -> App {
    let app = in_source(vec![
        seam("first", first_target, SOURCE_LANDMARK, 0.0),
        seam("second", THIRD, Vec3::new(40.0, 0.0, 0.0), 0.0),
    ]);
    let host = local_pawn(&app);
    place(&app, host, OUTSIDE, 0.0, Vec3::ZERO);
    app
}

const IN_FIRST: Vec3 = Vec3::new(20.0, 1.2, 0.0);
const IN_SECOND: Vec3 = Vec3::new(40.0, 1.2, 0.0);

// Act 6 / P11 and P15: of seams activating on one frame, the first in map
// order wins, whichever activated first in the frame.
#[test]
fn seam_pick_takes_first_in_map_order_whatever_the_activation_order() {
    // P11: both on one tick.
    let mut app = two_seams(DESTINATION);
    let host = local_pawn(&app);
    place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
    bystander(&mut app, IN_SECOND);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, "first")]);

    // P15: the second in map order activates on an earlier tick of the frame.
    let mut app = two_seams(DESTINATION);
    let host = local_pawn(&app);
    let other = bystander(&mut app, IN_SECOND);
    trigger_tick(&mut app);
    place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    let _ = other;
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, "first")]);
}

// Act 7 / P21: a seam that activated still loads when it is disarmed or
// emptied later in the frame, and still wins the pick among several.
#[test]
fn seam_activated_then_disarmed_or_emptied_still_loads() {
    let mut app = two_seams(DESTINATION);
    let host = local_pawn(&app);
    place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    // Later in the frame: the first seam is disarmed and emptied, and the
    // second activates.
    {
        let ctx = script_ctx(&app);
        let mut registry = ctx.registry.borrow_mut();
        let entity = app.seam_stage.entity(0).unwrap();
        let mut component = registry
            .get_component::<SeamComponent>(entity)
            .unwrap()
            .clone();
        component.armed = false;
        registry.set_component(entity, component).unwrap();
    }
    place(&app, host, IN_SECOND, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, "first")]);
}

// Act 5 and P16: an unknown target warns once at activation, requests no load
// and leaves the level running; first in map order with an unknown target
// beside a valid seam, the valid seam's target loads.
#[test]
fn seam_unknown_target_warns_once_and_a_valid_seam_still_loads() {
    let capture = LogCapture::start();
    let mut app = two_seams("nowhere");
    let host = local_pawn(&app);
    place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
    for _ in 0..2 {
        // The pawn leaves and re-enters: the seam activates again.
        place(&app, host, OUTSIDE, 0.0, Vec3::ZERO);
        trigger_tick(&mut app);
        place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
        trigger_tick(&mut app);
        app.pick_seam_level_request();
    }
    assert!(
        app.level_requests.is_empty(),
        "an unknown target requests nothing"
    );
    assert_eq!(
        app.boot_state,
        BootState::Running,
        "the level keeps running"
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "seam `first` targets unknown map id `nowhere`",
    );

    // P16: the first in map order is unknown, the second valid.
    bystander(&mut app, IN_SECOND);
    place(&app, host, OUTSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    place(&app, host, IN_FIRST, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(THIRD, "second")]);
}

/// Register the frontend menu and present it, as boot does.
fn present_frontend(app: &mut App) {
    crate::session::saves::test_fixtures::present_frontend(app);
}

// Act 1 (backdrop) and P30: a frontend backdrop's seams never activate, even
// after a mod pops the frontend root over it, and its unload writes no
// arrival.
#[test]
fn seam_in_a_frontend_backdrop_never_activates_and_its_unload_writes_no_arrival() {
    let mut app = seam_app();
    present_frontend(&mut app);
    install(
        &mut app,
        SOURCE,
        seam_world(vec![source_door()], &spawn_points(2)),
    );
    assert!(app.backdrop_installed, "fixture: installed under the menu");
    assert!(app.seam_stage.is_inert());
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert!(app.level_requests.is_empty());

    // P30: the gate is how the level loaded, not whether the menu shows.
    app.session.as_mut().unwrap().modal_stack.clear_pushed();
    assert!(!app.frontend_menu_is_present());
    place(&app, host, OUTSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert!(
        app.level_requests.is_empty(),
        "a popped backdrop stays inert"
    );

    app.unload_level_crossing(Some(DOOR));
    assert!(
        app.seam_crossing().is_none(),
        "a backdrop's unload writes none"
    );
}

// Act 3 / P31: a seam clears no pushed tree. Under a pushed pause menu the
// destination installs as a played level and the host arrives; under the
// pushed frontend root it installs as a backdrop, its seams inert.
#[test]
fn seam_activation_clears_no_pushed_tree() {
    // Pause menu.
    let mut app = in_source(vec![source_door()]);
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
    let pushed = app.session.as_ref().unwrap().modal_stack.len();
    assert!(pushed > 0, "fixture: the pause menu is pushed");
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(app.session.as_ref().unwrap().modal_stack.len(), pushed);
    drain_and_install(
        &mut app,
        seam_world(vec![destination_door()], &spawn_points(2)),
    );
    assert!(!app.backdrop_installed, "a played level");
    assert!(!app.seam_stage.is_inert());
    assert_eq!(
        app.seam_stage.take_arrivals().len(),
        1,
        "the host arrived through the seam"
    );

    // Frontend root, pushed over the played source level.
    let mut app = in_source(vec![source_door()]);
    let host = local_pawn(&app);
    place(&app, host, INSIDE, 0.0, Vec3::ZERO);
    present_frontend(&mut app);
    trigger_tick(&mut app);
    app.pick_seam_level_request();
    assert_eq!(seam_requests(&app), [seam_request(DESTINATION, DOOR)]);
    drain_and_install(
        &mut app,
        seam_world(vec![destination_door()], &spawn_points(2)),
    );
    assert!(app.frontend_menu_is_present(), "the frontend root stays");
    assert!(app.backdrop_installed, "the destination is a backdrop");
    assert!(app.seam_stage.is_inert(), "its seams are inert");
    assert!(app.seam_stage.take_arrivals().is_empty());
}
