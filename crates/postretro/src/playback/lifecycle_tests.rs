// Playback's lifecycle through the App: rows, aborts, marks, the stall
// timeout, the settle outcome, life-state logging and the input gate. The
// frame loop itself needs an event loop, so these drive the hooks it calls in
// its order, and source-order tests pin those call sites.
// See: context/lib/rendering_pipeline.md §12

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_camera_route::{PoseFile, format_pose_file};
use postretro_entities::Transform;
use postretro_foundation::PlayerMovementComponent;
use tempfile::TempDir;

use super::*;
use crate::playback::route::tests::pose_file;
use crate::playback::run::{Clock, RouteRun};
use crate::playback::{ChangeKind, Playback};
use crate::session::level_streaming::DrainAdmission;
use crate::startup::lifecycle::tests::test_app;

pub(crate) struct Run {
    pub(crate) app: App,
    file: PoseFile,
    pub(crate) report: PathBuf,
    launched: Instant,
    _dir: TempDir,
}

/// A test App playing a route of `poses` poses with `hold` hold frames,
/// armed (Settling) as install leaves it.
pub(crate) fn playing(hold: u32, poses: usize, extra: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cp.txt");
    let report = dir.path().join("run.playback.json");
    let file = pose_file(100.0, poses);
    std::fs::write(&path, format_pose_file(&file)).unwrap();
    let mut args: Vec<String> = [
        "postretro",
        "--camera-route",
        &path.display().to_string(),
        "--camera-route-hold",
        &hold.to_string(),
        "--camera-route-report",
        &report.display().to_string(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    let launched = Instant::now();
    let mut app = test_app();
    app.playback = Playback::from_args(&args, launched).unwrap();
    app.playback.active.as_mut().unwrap().run.arm();
    Run {
        app,
        file,
        report,
        launched,
        _dir: dir,
    }
}

/// One Running frame through the hooks in the frame loop's order.
pub(crate) fn frame(app: &mut App, now: Instant, presented: bool, streamed: bool) {
    app.playback_begin_world_frame(now);
    if streamed {
        app.playback.note_streamed(DrainAdmission::default());
    }
    app.playback.note_presented(presented);
    app.playback_finish_frame();
}

pub(crate) fn record(run: &Run) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(&run.report).expect("record written")).unwrap()
}

fn phases(record: &serde_json::Value) -> Vec<String> {
    record["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["phase"].as_str().unwrap().to_string())
        .collect()
}

fn exit_error(app: &App) -> String {
    format!("{:#}", app.exit_result.as_ref().unwrap_err())
}

#[test]
fn playback_writes_hold_no_draw_and_route_rows_then_completes() {
    // Hold rows, a no-draw row for a frame that streamed without a
    // surface, nothing for one that did not stream, then each pose once.
    let mut run = playing(1, 2, &[]);
    let now = Instant::now();
    let app = &mut run.app;
    frame(app, now, true, true);
    assert!(
        app.playback.stall.is_none(),
        "the first world frame disarms"
    );
    frame(app, now, false, true);
    frame(app, now, false, false);
    frame(app, now, true, true);
    assert!(!app.playback.exit_pending);
    frame(app, now, true, true);
    assert!(app.playback.exit_pending, "the last pose ends the run");
    assert!(app.playback.active.is_none());
    assert!(app.exit_result.is_ok());

    let record = record(&run);
    assert_eq!(phases(&record), ["hold", "no_draw", "route", "route"]);
    assert_eq!(record["header"]["outcome"], "complete");
    let rows = record["rows"].as_array().unwrap();
    assert_eq!(rows[1]["pose"], 0, "the no-draw row streamed from pose 0");
    assert_eq!(
        rows.iter()
            .map(|row| row["gap"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [false, false, true, false],
        "the first row after unpresented frames spans their gap"
    );
}

#[test]
fn playback_level_request_during_the_route_aborts_and_keeps_its_rows() {
    // Rows through frame k stay, the request is left undone, and the
    // session exits non-zero.
    let mut run = playing(0, 3, &[]);
    let now = Instant::now();
    frame(&mut run.app, now, true, true);
    frame(&mut run.app, now, true, true);
    run.app
        .enqueue_level_request(crate::startup::LevelRequest::Unload);
    run.app.drain_level_requests();
    assert!(run.app.playback.exit_pending);
    assert!(exit_error(&run.app).contains("level unload requested"));
    assert_eq!(
        run.app.boot_state,
        BootState::Running,
        "the level stays while the session exits"
    );
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert_eq!(record["header"]["abort_reason"], "level unload requested");
    assert_eq!(phases(&record), ["route", "route"]);
}

#[test]
fn playback_level_request_during_settling_or_the_hold_aborts() {
    // A reload during Settling writes no rows; a change during the hold
    // keeps the hold rows written so far.
    let level = PathBuf::from("maps/route.prl");
    let mut settling = playing(3, 2, &[]);
    settling.app.active_level_source = Some(LevelSource::Path(level.clone()));
    settling
        .app
        .enqueue_level_request(LevelRequest::Load(LevelSource::Path(level.clone())));
    settling.app.drain_level_requests();
    let record_settling = record(&settling);
    assert!(
        record_settling["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .starts_with("level reload requested")
    );
    assert!(phases(&record_settling).is_empty());

    let mut hold = playing(3, 2, &[]);
    hold.app.active_level_source = Some(LevelSource::Path(level));
    frame(&mut hold.app, Instant::now(), true, true);
    hold.app
        .enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "maps/other.prl",
        ))));
    hold.app.drain_level_requests();
    let record_hold = record(&hold);
    assert!(
        record_hold["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .starts_with("level change requested")
    );
    assert_eq!(phases(&record_hold), ["hold"]);
    assert!(exit_error(&hold.app).contains("level change requested"));
}

#[test]
fn playback_level_request_on_the_last_pose_frame_leaves_the_run_complete() {
    // The frame finishes and the record is written before the next
    // redraw drains the request.
    let mut run = playing(0, 2, &[]);
    let now = Instant::now();
    frame(&mut run.app, now, true, true);
    run.app.playback_begin_world_frame(now);
    run.app
        .enqueue_level_request(crate::startup::LevelRequest::Unload);
    run.app.playback.note_streamed(DrainAdmission::default());
    run.app.playback.note_presented(true);
    run.app.playback_finish_frame();
    run.app.drain_level_requests();
    assert!(run.app.exit_result.is_ok(), "complete, not aborted");
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "complete");
    assert!(record["header"]["abort_reason"].is_null());
    assert_eq!(phases(&record), ["route", "route"]);
}

#[test]
fn playback_join_from_the_frontend_aborts_any_run_not_yet_ended() {
    let addr: std::net::SocketAddr = "127.0.0.1:27015".parse().unwrap();
    let mut idle = test_app();
    assert_eq!(idle.playback_join_abort_reason(addr), None);
    assert!(!idle.playback_abort(idle.playback_join_abort_reason(addr)));

    let mut run = playing(0, 2, &[]);
    let reason = run.app.playback_join_abort_reason(addr);
    assert!(run.app.playback_abort(reason));
    assert_eq!(
        record(&run)["header"]["abort_reason"],
        "join to host 127.0.0.1:27015 requested"
    );

    // A complete run is gone: a later join leaves it complete.
    let mut done = playing(0, 1, &[]);
    frame(&mut done.app, Instant::now(), true, true);
    assert_eq!(done.app.playback_join_abort_reason(addr), None);
    assert_eq!(record(&done)["header"]["outcome"], "complete");
}

#[test]
fn playback_join_while_awaiting_the_level_aborts_the_run() {
    // A launch with a route and no map waits on the frontend: a join there
    // would install the host's level as the route's.
    let addr: std::net::SocketAddr = "127.0.0.1:27015".parse().unwrap();
    let mut run = playing(0, 2, &[]);
    run.app.playback.active.as_mut().unwrap().run = RouteRun::new(0, 2, Clock::FrameLocked);
    assert!(!run.app.playback.captures_gameplay());
    let reason = run.app.playback_join_abort_reason(addr);
    assert!(reason.is_some(), "not yet ended is enough");
    assert!(run.app.playback_abort(reason), "the join is left undone");
    assert!(run.app.playback.exit_pending && run.app.playback.active.is_none());
    assert!(exit_error(&run.app).contains("join to host 127.0.0.1:27015 requested"));
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert_eq!(
        record["header"]["abort_reason"],
        "join to host 127.0.0.1:27015 requested"
    );
    assert!(phases(&record).is_empty());
}

#[test]
fn playback_level_installed_as_a_connected_client_aborts_instead_of_arming() {
    let mut run = playing(0, 2, &[]);
    run.app.playback.active.as_mut().unwrap().run = RouteRun::new(0, 2, Clock::FrameLocked);
    run.app.session.as_mut().unwrap().net_endpoint = Some(
        crate::netcode::NetEndpoint::from_role(
            &crate::netcode::NetRole::Connect {
                addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 1)),
            },
            None,
        )
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint"),
    );
    run.app.map_path = Some(PathBuf::from("maps/host.prl"));
    run.app.playback_level_installed();
    assert!(
        run.app.playback.active.is_none() && run.app.playback.exit_pending,
        "never armed on a host's level"
    );
    assert!(exit_error(&run.app).contains("connected client"));
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert!(
        record["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .contains("connected client")
    );
}

#[test]
fn playback_matching_level_drops_an_earlier_levels_refusal() {
    // Level A mismatches and holds a refusal; level B then matches and
    // arms. B's first Settling frame holds the route, never A's refusal.
    let mut run = playing(0, 2, &[]);
    let active = run.app.playback.active.as_mut().unwrap();
    active.run = RouteRun::new(0, 2, Clock::FrameLocked);
    let rebaked = "id51=0000000000000000 walk_epoch=4 size=1 mtime=1 prl=maps/a.prl";
    assert!(!active.level_installed("maps/a.prl", rebaked.to_string()));
    assert!(
        active
            .refusal
            .as_deref()
            .is_some_and(|refusal| refusal.contains("maps/a.prl"))
    );
    let matching = "id51=4b7255ff5bdf714c walk_epoch=4 size=1 mtime=1 prl=maps/b.prl";
    assert!(active.level_installed("maps/b.prl", matching.to_string()));
    assert!(active.refusal.is_none());
    assert_eq!(active.run.phase(), crate::playback::run::Phase::Settling);

    let fallback = run.app.presented_pose().held_view(run.app.camera.aspect());
    let SettlingView::Hold(view) = run.app.playback_settling_view_for(fallback) else {
        panic!("level B matched: Settling holds the first pose");
    };
    assert_eq!(view.camera.eye_position, run.file.poses[0].eye);
    assert!(run.app.playback.active.is_some());
}

#[test]
fn playback_closed_mid_route_writes_its_record_aborted_and_exits_non_zero() {
    let mut run = playing(1, 5, &[]);
    let now = Instant::now();
    for _ in 0..3 {
        frame(&mut run.app, now, true, true);
    }
    run.app.playback_closed_on_exit();
    assert!(run.app.playback.active.is_none());
    assert!(
        exit_error(&run.app).contains("window closed or quit before the route ended"),
        "an agent keys on the exit code"
    );
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert_eq!(
        record["header"]["abort_reason"],
        "window closed or quit before the route ended"
    );
    assert_eq!(phases(&record).len(), 3);
}

#[test]
fn playback_error_exit_mid_route_records_that_error_as_the_reason() {
    let mut run = playing(0, 5, &[]);
    frame(&mut run.app, Instant::now(), true, true);
    run.app.exit_result = Err(anyhow::anyhow!("surface lost: device removed"));
    run.app.playback_closed_on_exit();
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert_eq!(
        record["header"]["abort_reason"], "surface lost: device removed",
        "the cause, never a window close"
    );
    assert_eq!(
        exit_error(&run.app),
        "surface lost: device removed",
        "the session's own error stays the exit"
    );
}

// Regression: a failed record write replaced the engine error a `Failed`
// ending carried, so the exit named the write, not the cause.
#[test]
fn playback_engine_error_stays_the_exit_when_the_record_write_also_fails() {
    let mut run = playing(0, 5, &[]);
    frame(&mut run.app, Instant::now(), true, true);
    let unwritable = run.report.with_file_name("missing").join("run.json");
    run.app.playback.active.as_mut().unwrap().args.report = unwritable.clone();
    run.app.exit_result = Err(anyhow::anyhow!("surface lost: device removed"));
    run.app.playback_closed_on_exit();
    assert!(!unwritable.exists(), "the write failed");
    assert_eq!(
        exit_error(&run.app),
        "surface lost: device removed",
        "the engine error stays the exit, never the write's"
    );
}

// Regression: an abort in the same iteration as an earlier error (a stall
// after a refusal, say) replaced that error as the exit.
#[test]
fn playback_abort_keeps_an_earlier_exit_error() {
    let mut run = playing(0, 5, &[]);
    run.app.exit_result = Err(anyhow::anyhow!("settling failed: device removed"));
    let reason = Some("level unload requested".to_string());
    assert!(run.app.playback_abort(reason));
    assert_eq!(
        record(&run)["header"]["abort_reason"],
        "level unload requested"
    );
    assert_eq!(exit_error(&run.app), "settling failed: device removed");
}

#[test]
fn playback_completed_run_exits_zero_through_the_exit_hook() {
    let mut run = playing(0, 1, &[]);
    frame(&mut run.app, Instant::now(), true, true);
    run.app.playback_closed_on_exit();
    assert!(run.app.exit_result.is_ok());
    assert_eq!(record(&run)["header"]["outcome"], "complete");
}

#[test]
fn playback_changes_mark_every_row_from_the_first_they_touch() {
    // A change before the first row marks nothing; one mid-route marks
    // every later row; a hidden-window gap marks only the row spanning it.
    let mut run = playing(1, 4, &[]);
    let now = Instant::now();
    let app = &mut run.app;
    app.playback.note_change(ChangeKind::Extents);
    frame(app, now, true, true);
    app.playback.note_change(ChangeKind::Vsync);
    frame(app, now, true, true);
    app.playback.note_occluded(true);
    app.playback.note_occluded(false);
    app.playback.note_change(ChangeKind::ScriptReload);
    frame(app, now, true, true);
    app.playback.note_change(ChangeKind::WindowMode);
    app.playback.note_change(ChangeKind::DrainCap);
    frame(app, now, true, true);
    frame(app, now, true, true);
    assert!(
        app.exit_result.is_ok(),
        "marked rows leave the run to its end"
    );

    let record = record(&run);
    let rows = record["rows"].as_array().unwrap();
    let changes: Vec<serde_json::Value> = rows.iter().map(|row| row["changes"].clone()).collect();
    assert_eq!(
        changes,
        [
            serde_json::json!([]),
            serde_json::json!(["vsync"]),
            serde_json::json!(["vsync", "script_reload"]),
            serde_json::json!(["vsync", "window_mode", "drain_cap", "script_reload"]),
            serde_json::json!(["vsync", "window_mode", "drain_cap", "script_reload"]),
        ]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["gap"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [false, false, true, false, false]
    );
}

#[test]
fn playback_stall_timeout_names_a_stalled_window_and_writes_no_rows() {
    // Nothing presented by the deadline.
    let mut run = playing(0, 2, &["--camera-route-stall-timeout", "3"]);
    run.app.boot_state = BootState::Splash;
    run.app
        .playback_check_stall(run.launched + Duration::from_millis(2_900));
    assert!(!run.app.playback.exit_pending, "before the deadline");
    run.app
        .playback_check_stall(run.launched + Duration::from_secs(3));
    assert!(run.app.playback.exit_pending);
    assert!(exit_error(&run.app).contains("stalled window"));
    let record = record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert!(
        record["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .starts_with("stalled window")
    );
    assert!(phases(&record).is_empty());
}

#[test]
fn playback_stall_timeout_names_the_screen_a_presenting_run_waited_on() {
    let mut run = playing(0, 2, &["--camera-route-stall-timeout=3"]);
    // Not yet installed: the first-launch panel holds boot.
    run.app.playback.active.as_mut().unwrap().run = RouteRun::new(0, 2, Clock::FrameLocked);
    run.app.boot_state = BootState::FirstLaunchHold;
    run.app.playback.note_any_present();
    run.app
        .playback_check_stall(run.launched + Duration::from_secs(4));
    let error = exit_error(&run.app);
    assert!(
        error.contains("first-launch accessibility panel") && !error.contains("stalled window"),
        "{error}"
    );
    assert!(
        record(&run)["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .contains("first-launch accessibility panel")
    );
}

#[test]
fn playback_first_world_frame_disarms_the_stall_timeout() {
    let mut run = playing(2, 2, &["--camera-route-stall-timeout", "3"]);
    frame(&mut run.app, Instant::now(), true, true);
    run.app
        .playback_check_stall(run.launched + Duration::from_secs(60));
    assert!(!run.app.playback.exit_pending);
    assert!(run.app.exit_result.is_ok());
}

#[test]
fn playback_records_how_settling_ended() {
    let mut run = playing(0, 1, &[]);
    run.app.playback_level_revealed(true);
    let active = run.app.playback.active.as_ref().unwrap();
    assert_eq!(active.record.header_json()["settle"], "timed_out");

    let mut settled = playing(0, 1, &[]);
    settled.app.playback_level_revealed(false);
    frame(&mut settled.app, Instant::now(), true, true);
    assert_eq!(record(&settled)["header"]["settle"], "settled");
}

#[test]
fn playback_logs_a_pawn_life_state_change_on_its_frame_and_continues() {
    // The frame that sees the change logs it; the route goes on.
    let mut run = playing(0, 3, &[]);
    let ctx = run
        .app
        .session
        .as_ref()
        .unwrap()
        .scripting
        .script_ctx
        .clone();
    let pawn = ctx.registry.borrow_mut().spawn(Transform::default());
    ctx.registry
        .borrow_mut()
        .set_component(
            pawn,
            PlayerMovementComponent::from_descriptor(&crate::tests::minimal_player_descriptor()),
        )
        .unwrap();
    let now = Instant::now();
    frame(&mut run.app, now, true, true);
    {
        let mut registry = ctx.registry.borrow_mut();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        movement.life_state = movement.life_state.wrapping_add(1);
        movement.life_state_entry = movement.life_state_entry.wrapping_add(1);
        registry.set_component(pawn, movement).unwrap();
    }
    frame(&mut run.app, now, true, true);
    assert!(
        run.app.playback.active.is_some(),
        "the route continues past a death"
    );
    frame(&mut run.app, now, true, true);
    let record = record(&run);
    let life = record["life_states"].as_array().unwrap();
    assert_eq!(life.len(), 1, "the baseline frame logs nothing");
    assert_eq!(life[0]["frame"], 1);
    assert_eq!(life[0]["pawn"], pawn.to_raw());
    assert_eq!(phases(&record), ["route", "route", "route"]);
}

#[test]
fn playback_settling_view_is_the_first_pose_with_or_without_a_pawn() {
    // The held view ignores the presented (spawn) pose either way.
    let mut run = playing(0, 2, &[]);
    let route = run
        .app
        .playback
        .active
        .as_ref()
        .unwrap()
        .view(0, 0.0)
        .unwrap();
    let expected = route.held_view(run.app.camera.aspect()).camera;
    for with_pawn in [false, true] {
        if with_pawn {
            let ctx = run
                .app
                .session
                .as_ref()
                .unwrap()
                .scripting
                .script_ctx
                .clone();
            let pawn = ctx.registry.borrow_mut().spawn(Transform::default());
            ctx.registry
                .borrow_mut()
                .set_component(
                    pawn,
                    PlayerMovementComponent::from_descriptor(
                        &crate::tests::minimal_player_descriptor(),
                    ),
                )
                .unwrap();
        }
        let fallback = run.app.presented_pose().held_view(run.app.camera.aspect());
        let SettlingView::Hold(view) = run.app.playback_settling_view_for(fallback) else {
            panic!("an armed route holds its first pose");
        };
        assert_eq!(view.camera.eye_position, run.file.poses[0].eye);
        assert_eq!(
            view.camera
                .view_projection
                .to_cols_array()
                .map(f32::to_bits),
            expected.view_projection.to_cols_array().map(f32::to_bits),
            "pawn {with_pawn}"
        );
    }

    let mut refused = playing(0, 1, &[]);
    refused.app.playback.active.as_mut().unwrap().refusal = Some("bake mismatch".into());
    let fallback = refused.app.presented_pose().held_view(1.0);
    assert!(matches!(
        refused.app.playback_settling_view_for(fallback),
        SettlingView::Refused(reason) if reason == "bake mismatch"
    ));
    assert!(refused.app.playback.active.is_none());
}

#[test]
fn playback_route_frame_aim_ignores_the_pawn_aim_and_mover_carry() {
    // A pawn riding a mover carries yaw into the gameplay aim; a route frame
    // draws its pose regardless, and the pawn's own aim is left alone.
    let mut run = playing(0, 2, &[]);
    let camera_before = run.app.camera.position;
    let carried = crate::PresentedAimPose {
        position: Vec3::new(40.0, 3.0, -7.0),
        yaw: 2.9,
        pitch: 0.4,
    };
    assert_eq!(
        run.app.render_aim_pose(carried).yaw,
        carried.yaw,
        "Settling draws no world frame; the aim passes through"
    );
    run.app.playback_begin_world_frame(Instant::now());
    let aim = run.app.render_aim_pose(carried);
    let pose = &run.file.poses[0];
    assert_eq!(
        (aim.position, aim.yaw.to_bits(), aim.pitch.to_bits()),
        (pose.eye, pose.yaw.to_bits(), pose.pitch.to_bits())
    );
    assert_eq!(
        run.app.camera.position, camera_before,
        "the pawn's camera stays"
    );
}

#[test]
fn playback_suspends_gameplay_input_from_settling_through_the_route() {
    // A movement key held from Settling on reaches the sim as neutral
    // input on every frame; the sim still ticks.
    let mut run = playing(1, 2, &[]);
    let mut keyboard = crate::input::InputSystem::new(crate::input::default_bindings());
    keyboard.handle_keyboard_event(winit::keyboard::KeyCode::KeyW, true);
    let held = keyboard.snapshot();
    assert_ne!(held.axis_value(crate::input::Action::MoveForward), 0.0);
    let mut latch = crate::input::GameplayInputLatch::new();
    let now = Instant::now();
    // Settling, hold frame 1, route frames 0 and 1.
    for step in 0..3 {
        let gate = {
            let session = run.app.session.as_ref().unwrap();
            crate::gameplay_capture_gate_for_frame(
                run.app.playback.captures_gameplay(),
                &session.modal_stack,
            )
        };
        assert!(gate, "frame {step}: playback captures gameplay");
        let snapshot = crate::gameplay_snapshot_for_capture_state(&mut latch, &held, 1, gate)
            .expect("the sim ticks under playback");
        assert_eq!(snapshot.axis_value(crate::input::Action::MoveForward), 0.0);
        frame(&mut run.app, now, true, true);
    }
    assert!(!run.app.playback.captures_gameplay(), "the run ended");
}

/// Every hook a frame calls, with playback inactive.
fn inactive_frame(app: &mut App, now: Instant) {
    app.playback_begin_world_frame(now);
    app.playback.note_streamed(DrainAdmission::default());
    app.playback.note_presented(true);
    app.playback.note_change(ChangeKind::Vsync);
    app.playback.note_occluded(false);
    std::hint::black_box(app.playback.route_view());
    std::hint::black_box(app.playback.captures_gameplay());
    app.playback_finish_frame();
}

#[test]
fn inactive_playback_hooks_allocate_nothing() {
    // AC 30: with no route, a frame allocates nothing for playback.
    let mut app = test_app();
    let now = Instant::now();
    inactive_frame(&mut app, now);

    let control = crate::alloc_probe::AllocSnapshot::arm();
    std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "probe must count in this binary"
    );

    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for _ in 0..240 {
        inactive_frame(&mut app, now);
    }
    assert_eq!(probe.allocs_since(), 0);
    assert!(app.playback.active.is_none());
}

#[test]
fn playback_rows_carry_drain_columns_and_absent_cpu_and_gpu_fields_with_timing_off() {
    // AC 26: timing off, the stage and GPU fields read null; the interval,
    // admission, drain flags and late counts are present.
    let mut run = playing(0, 1, &[]);
    let now = Instant::now();
    let app = &mut run.app;
    app.playback_begin_world_frame(now);
    app.playback.note_streamed(DrainAdmission {
        lightmap: crate::streaming::shared_drain::ResourceAdmission {
            admitted: 2,
            bytes: 4_096,
            left_behind: false,
        },
        ..DrainAdmission::default()
    });
    app.playback.note_presented(true);
    app.playback_finish_frame();
    let record = record(&run);
    let row = &record["rows"][0];
    assert!(row["cpu_ns"].is_null() && row["ticks"].is_null());
    assert!(row["gpu"].is_null());
    assert_eq!(row["drain"]["lightmap_admitted_items"], 2);
    assert_eq!(row["drain"]["lightmap_admitted_bytes"], 4_096);
    assert_eq!(row["drain"]["cold"], false);
    assert!(row["drain"].get("sh_late_cluster_frames").is_some());
    assert!(record["header"]["gpu_timing"]["rows_missing"].is_null());
}

// Source order: the frame loop and its neighbours call playback where the
// tests above assume.
const FRAME_LOOP: &str = include_str!("../frame_loop/mod.rs");
const MAIN: &str = include_str!("../main.rs");
const BOOT_STATE: &str = include_str!("../startup/lifecycle_boot_state.rs");
const SESSION_ENTRY: &str = include_str!("../startup/session_entry.rs");
const PLAYBACK_SOURCES: [&str; 13] = [
    include_str!("mod.rs"),
    include_str!("lifecycle.rs"),
    include_str!("recording.rs"),
    include_str!("recording_hooks.rs"),
    include_str!("args.rs"),
    include_str!("marks.rs"),
    include_str!("pacing.rs"),
    include_str!("record/mod.rs"),
    include_str!("record/header.rs"),
    include_str!("record/row.rs"),
    include_str!("route.rs"),
    include_str!("run.rs"),
    include_str!("sample.rs"),
];

fn at(haystack: &str, needle: &str, from: usize) -> usize {
    from + haystack[from..]
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found after {from}"))
}

#[test]
fn frame_loop_calls_playback_hooks_in_frame_order() {
    let redraw = at(FRAME_LOOP, "pub(crate) fn redraw(", 0);
    let gate = at(
        FRAME_LOOP,
        "ui_captured_gameplay_at_frame_start || playback_holds_input",
        redraw,
    );
    let snapshot = at(FRAME_LOOP, "gameplay_snapshot_for_capture_state(", gate);
    let tick = at(FRAME_LOOP, "FrameStage::FixedStep", snapshot);
    let begin = at(FRAME_LOOP, "app.playback_begin_world_frame(now);", tick);
    let aim = at(FRAME_LOOP, "app.render_aim_pose(presented_aim)", begin);
    let eye = at(FRAME_LOOP, "frame_eye::assemble_frame_eye(", aim);
    let streaming = at(FRAME_LOOP, "session.run_level_streaming_step(", eye);
    let streamed = at(
        FRAME_LOOP,
        "app.playback.note_streamed(streaming_step.admission);",
        streaming,
    );
    let render = at(FRAME_LOOP, "render_frame_indirect", streamed);
    let presented = at(
        FRAME_LOOP,
        "app.playback.note_presented(presented);",
        render,
    );
    let end = at(FRAME_LOOP, "app.playback_end_frame(event_loop);", presented);
    assert!(
        gate < snapshot && snapshot < tick && tick < begin && begin < aim && aim < eye,
        "input gate, tick, pose pick, aim, eye"
    );
    assert!(streaming < streamed && streamed < render && render < presented && presented < end);
    assert!(
        FRAME_LOOP[end..]
            .trim_start_matches("app.playback_end_frame(event_loop);")
            .trim_start()
            .starts_with('}'),
        "the end hook is the frame's last statement"
    );
    // The sim is never paused: no playback hook sits between the frame's
    // timing and the tick loop it drives.
    let timing = at(FRAME_LOOP, "app.frame_timing.begin_frame(now)", redraw);
    assert!(!FRAME_LOOP[timing..tick].contains("playback_begin"));
}

#[test]
fn playback_never_moves_the_pawn_or_touches_the_sim_clock() {
    for source in PLAYBACK_SOURCES {
        for forbidden in [
            "registry.borrow_mut",
            "set_component",
            "place_local_pawn",
            "frame_timing",
            "accumulator",
        ] {
            assert!(
                !source.contains(forbidden),
                "playback names `{forbidden}`: it may read the pawn, never move it or the sim"
            );
        }
    }
}

#[test]
fn level_requests_and_joins_reach_the_abort_hook_before_they_act() {
    let drain = at(BOOT_STATE, "pub(crate) fn drain_level_requests(", 0);
    let reason = at(
        BOOT_STATE,
        "self.playback_request_abort_reason(&request)",
        drain,
    );
    let load_abort = at(BOOT_STATE, "self.playback_abort(playback_abort)", reason);
    // A load unloads through the seam-aware crossing; Frontend unloads plainly.
    let unload = at(BOOT_STATE, "self.unload_level_crossing(", load_abort);
    let begin = at(BOOT_STATE, "self.begin_level_load(load);", load_abort);
    let none_abort = at(BOOT_STATE, "self.playback_abort(playback_abort)", begin);
    let frontend = at(
        BOOT_STATE,
        "self.abandon_level_load_for_frontend();",
        none_abort,
    );
    assert!(load_abort < unload && unload < begin && none_abort < frontend);

    let drive = at(BOOT_STATE, "fn drive_boot_state_for_redraw(", 0);
    let ended = at(BOOT_STATE, "self.playback_take_exit(event_loop)", drive);
    let drained = at(BOOT_STATE, "self.drain_level_requests();", drive);
    let exit = at(BOOT_STATE, "self.playback_take_exit(event_loop)", drained);
    let resume = at(BOOT_STATE, "self.resume_pending_entry();", drained);
    assert!(
        ended < drained,
        "a redraw after the run ended stops before any drain, unload or save"
    );
    assert!(
        exit < resume,
        "an aborted run's frame stops after the drain"
    );

    let join = at(SESSION_ENTRY, "fn join_from_frontend_with(", 0);
    let hook = at(SESSION_ENTRY, "self.playback_join_abort_reason(addr)", join);
    let cancel = at(SESSION_ENTRY, "self.cancel_backdrop_load_for_join();", join);
    assert!(hook < cancel);
}

#[test]
fn main_routes_occlusion_vsync_and_the_stall_wait_to_playback() {
    assert!(
        MAIN.contains("WindowEvent::Occluded(occluded) => self.playback.note_occluded(occluded)")
    );
    let vsync = at(MAIN, "DiagnosticAction::ToggleVsync =>", 0);
    let toggle = at(MAIN, "renderer.toggle_vsync()", vsync);
    let mark = at(MAIN, "crate::playback::ChangeKind::Vsync", toggle);
    assert!(mark - toggle < 600, "the chord marks rows where it toggles");
    let wait = at(MAIN, "fn about_to_wait(", 0);
    let hook = at(MAIN, "self.playback_about_to_wait(event_loop)", wait);
    let redraw = at(MAIN, "ws.window.request_redraw()", wait);
    assert!(hook < redraw);
}
