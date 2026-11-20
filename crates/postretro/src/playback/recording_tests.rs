// Camera-route recording: the pre-view-feel view each presented world frame
// holds, resampling to 60 Hz by time and never across a pawn's jump, a
// frame-locked replay bit-equal to the resampled file, the first gameplay
// level's scope, and the one write on every exit route, refused when
// playback could not read it. The frame loop needs an event loop, so App-level tests
// drive the hooks in its order and source-order tests pin the call sites.
// See: context/lib/rendering_pipeline.md §12 · context/lib/boot_sequence.md §1, §4, §5

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_camera_route::{
    DEFAULT_HFOV_DEGREES, PoseFile, RouteOrigin, RoutePose, parse_pose_file,
};
use postretro_entities::{EntityId, Transform};
use postretro_foundation::{
    BobParams, MovementStateKind, PlayerMovementComponent, TiltParams, ViewFeelParams,
};
use postretro_test_log_capture::LogCapture;
use tempfile::TempDir;

use super::recording::{RecordState, Sample, resample};
use crate::App;
use crate::camera::{HFOV, RenderCamera};
use crate::frame_eye::{self, FrameEyeInputs, ViewFeelDriver, ViewFeelTracking};
use crate::playback::Playback;
use crate::startup::lifecycle::tests::{level_world, test_app};
use crate::startup::{LevelRequest, LevelSource};
use crate::view_feel::ViewFeelState;

const ASPECT: f32 = 16.0 / 10.0;
const FIRST_LEVEL: &str = "maps/first.prl";

/// The camera's own motion: eye, yaw and pitch at `t` seconds, steady speed
/// and turn. Yaw is wrapped into (-π, π] as a camera's can be, and crosses
/// ±π early on, so resampling must turn the short way round.
fn motion(t: f64) -> (Vec3, f32, f32) {
    let t = t as f32;
    let yaw = (3.0 + 1.5 * t + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    (Vec3::new(2.0 * t, 1.5, -3.0 * t), yaw, -0.2 + 0.1 * t)
}

/// The eye's speed in `motion`, metres a second.
const SPEED: f32 = 3.605_551_3;

fn samples(start: Instant, times: impl IntoIterator<Item = f64>) -> Vec<Sample> {
    times
        .into_iter()
        .map(|t| {
            let (eye, yaw, pitch) = motion(t);
            Sample {
                at: start + Duration::from_secs_f64(t),
                eye,
                yaw,
                pitch,
                continuity: None,
            }
        })
        .collect()
}

fn angle_gap(a: f32, b: f32) -> f32 {
    ((a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs()
}

fn assert_follows_motion(poses: &[RoutePose]) {
    for (k, pose) in poses.iter().enumerate() {
        let (eye, yaw, pitch) = motion(k as f64 / 60.0);
        assert!(pose.eye.distance(eye) < 1.0e-4, "pose {k}: {pose:?}");
        assert!(angle_gap(pose.yaw, yaw) < 1.0e-4, "pose {k} yaw: {pose:?}");
        assert!((pose.pitch - pitch).abs() < 1.0e-5, "pose {k} pitch");
        assert!(pose.walk.is_none() && !pose.cut);
    }
}

#[test]
fn recording_at_30_and_60_fps_resamples_to_the_same_60_hz_poses() {
    // AC 45: one second of the same motion, recorded at either rate.
    let start = Instant::now();
    let at_30 = resample(&samples(start, (0..=30).map(|i| i as f64 / 30.0)), 60.0);
    let at_60 = resample(&samples(start, (0..=60).map(|i| i as f64 / 60.0)), 60.0);
    assert_eq!(at_30.len(), 61, "pose k at k/60 s up to the last sample");
    assert_eq!(at_60.len(), 61);
    assert_follows_motion(&at_30);
    assert_follows_motion(&at_60);
    for (a, b) in at_30.iter().zip(&at_60) {
        assert!(a.eye.distance(b.eye) < 1.0e-4);
        assert!(angle_gap(a.yaw, b.yaw) < 1.0e-4);
        assert!((a.pitch - b.pitch).abs() < 1.0e-5);
    }
}

#[test]
fn recording_across_a_frame_hitch_leaves_no_pose_jump_beyond_the_motion() {
    // AC 45: a 150 ms frame mid-recording is bridged by time, not by frame.
    let start = Instant::now();
    let times = (0..=20)
        .map(|i| i as f64 / 60.0)
        .chain((29..=60).map(|i| i as f64 / 60.0));
    let poses = resample(&samples(start, times), 60.0);
    assert_eq!(poses.len(), 61);
    assert_follows_motion(&poses);
    for pair in poses.windows(2) {
        assert!(
            pair[0].eye.distance(pair[1].eye) <= SPEED / 60.0 + 1.0e-4,
            "{pair:?}"
        );
    }
}

#[test]
fn recording_resample_keeps_a_lone_frame_and_ends_at_the_last_sample() {
    let start = Instant::now();
    let lone = resample(&samples(start, [0.0]), 60.0);
    assert_eq!(lone.len(), 1);
    let (eye, yaw, pitch) = motion(0.0);
    assert_eq!((lone[0].eye, lone[0].yaw, lone[0].pitch), (eye, yaw, pitch));
    // 25 ms of frames: poses at 0 and 1/60 s only, nothing past the last.
    assert_eq!(resample(&samples(start, [0.0, 0.01, 0.025]), 60.0).len(), 2);
}

// --- App-level hooks ---

struct Recording {
    app: App,
    path: PathBuf,
    _dir: TempDir,
}

fn launch(path: &Path) -> Vec<String> {
    [
        "postretro",
        "--record-camera-route",
        &path.display().to_string(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// A test App recording to a temp file, its first gameplay level installed.
fn recording() -> Recording {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rec.txt");
    let mut app = test_app();
    app.playback = Playback::from_args(&launch(&path), Instant::now()).unwrap();
    assert!(
        app.playback.stall.is_some(),
        "a recording arms the stall timeout"
    );
    install(&mut app, FIRST_LEVEL);
    Recording {
        app,
        path,
        _dir: dir,
    }
}

fn install(app: &mut App, level: &str) {
    app.level = Some(level_world("route", 1));
    app.map_path = Some(PathBuf::from(level));
    app.active_level_source = Some(LevelSource::Path(PathBuf::from(level)));
    app.playback_level_installed();
}

fn eye_inputs(t: f64, driver: Option<ViewFeelDriver>) -> FrameEyeInputs<'static> {
    let (presented_eye, render_yaw, pitch) = motion(t);
    FrameEyeInputs {
        presented_eye,
        aspect: ASPECT,
        base_hfov: HFOV,
        render_yaw,
        pitch,
        driver,
        movement_edges: &[],
        frame_dt: 0.25,
        view_feel_scale: 1.0,
    }
}

/// One Running frame at `t` seconds through the hooks in the frame loop's
/// order: the eye, the present, the frame's end.
fn world_frame(app: &mut App, start: Instant, t: f64, presented: bool) {
    app.playback_record_eye(start + Duration::from_secs_f64(t), &eye_inputs(t, None));
    app.playback.note_presented(presented);
    end_frame(app);
}

/// A presented Running frame whose eye is `offset` from `motion`'s: the
/// camera somewhere a jump put it.
fn world_frame_moved(app: &mut App, start: Instant, t: f64, offset: Vec3) {
    let inputs = eye_inputs(t, None);
    let inputs = FrameEyeInputs {
        presented_eye: inputs.presented_eye + offset,
        ..inputs
    };
    app.playback_record_eye(start + Duration::from_secs_f64(t), &inputs);
    app.playback.note_presented(true);
    end_frame(app);
}

/// Spawns the local pawn the camera follows.
fn follow_pawn(app: &App) -> EntityId {
    let registry = &app.session.as_ref().unwrap().scripting.script_ctx.registry;
    let pawn = registry.borrow_mut().spawn(Transform::default());
    registry
        .borrow_mut()
        .set_component(
            pawn,
            PlayerMovementComponent::from_descriptor(&crate::tests::minimal_player_descriptor()),
        )
        .unwrap();
    pawn
}

/// Changes the pawn's movement state between frames, as a tick would.
fn move_pawn(app: &App, pawn: EntityId, change: impl FnOnce(&mut PlayerMovementComponent)) {
    let mut registry = app
        .session
        .as_ref()
        .unwrap()
        .scripting
        .script_ctx
        .registry
        .borrow_mut();
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    change(&mut movement);
    registry.set_component(pawn, movement).unwrap();
}

fn teleport(movement: &mut PlayerMovementComponent) {
    movement.teleport.generation = movement.teleport.generation.wrapping_add(1);
}

fn respawn(movement: &mut PlayerMovementComponent) {
    movement.life_state_entry = movement.life_state_entry.wrapping_add(1);
}

/// Where a jump puts the camera in these tests: far from `motion`.
const AWAY: Vec3 = Vec3::new(50.0, 0.0, 0.0);

fn end_frame(app: &mut App) {
    if let Some(recording) = app.playback.recording.as_mut() {
        recording.end_frame();
    }
    app.playback_finish_frame();
}

fn written(path: &Path) -> PoseFile {
    parse_pose_file(&std::fs::read_to_string(path).expect("the recording was written")).unwrap()
}

fn state(app: &App) -> RecordState {
    app.playback.recording.as_ref().unwrap().state()
}

fn bobbing_and_tilting() -> ViewFeelDriver {
    ViewFeelDriver {
        pawn: postretro_entities::EntityId::from_raw(1),
        params: ViewFeelParams {
            bob: Some(BobParams {
                vertical_frequency: 1.0,
                lateral_frequency: 0.5,
                vertical_amplitude: 0.1,
                lateral_amplitude: 0.05,
                speed_threshold: 0.5,
                grounded_only: false,
            }),
            tilt: Some(TiltParams {
                max_angle: 8.0,
                speed_reference: 4.0,
                tension: 20.0,
                grounded_only: false,
            }),
            sway: None,
            impulse: None,
            pose: None,
        },
        velocity: Vec3::new(6.0, 0.0, 0.0),
        is_grounded: true,
        movement_state: MovementStateKind::Normal,
        vault: None,
        eye_height_above_feet: 1.0,
    }
}

#[test]
fn recording_holds_the_pre_feel_view_while_view_feel_moves_the_camera() {
    // AC 44: the frame's view-feel driver bobs and rolls its camera; the
    // recording holds the eye, yaw and pitch from before view feel, at the
    // base horizontal FOV.
    let mut run = recording();
    let start = Instant::now();
    let mut state = ViewFeelState::default();
    for frame in 0..3 {
        let t = f64::from(frame) / 60.0;
        let inputs = eye_inputs(t, Some(bobbing_and_tilting()));
        run.app
            .playback_record_eye(start + Duration::from_secs_f64(t), &inputs);
        let eye = frame_eye::assemble_frame_eye(
            inputs,
            ViewFeelTracking {
                state: &mut state,
                followed_pawn: &mut None,
                descriptor: &mut None,
            },
        );
        assert_ne!(eye.eye_offset, Vec3::ZERO, "view feel bobs this frame");
        assert_ne!(eye.roll, 0.0, "view feel rolls this frame");
        run.app.playback.note_presented(true);
        end_frame(&mut run.app);
    }
    let recorded = run.app.playback.recording.as_ref().unwrap().samples();
    for (frame, sample) in recorded.iter().enumerate() {
        let (eye, yaw, pitch) = motion(frame as f64 / 60.0);
        assert_eq!(
            (sample.eye, sample.yaw.to_bits(), sample.pitch.to_bits()),
            (eye, yaw.to_bits(), pitch.to_bits())
        );
    }
    run.app.playback_session_exits();
    let file = written(&run.path);
    assert_eq!(
        file.header.projection.hfov_degrees.to_bits(),
        DEFAULT_HFOV_DEGREES.to_bits(),
        "the default FOV writes the generator's exact default"
    );
    assert_eq!(file.header.projection.aspect, ASPECT);
    assert_eq!(file.poses[0].eye, motion(0.0).0);
}

#[test]
fn recording_skips_unpresented_and_world_less_frames_and_writes_only_at_the_end() {
    // AC 46: no pose for a frame that drew no world or a Settling frame (it
    // takes no eye), no file until the write, and a header naming the
    // recorded origin and the level's bake.
    let mut run = recording();
    let start = Instant::now();
    // Settling: frames present with no eye offered.
    run.app.playback.note_any_present();
    run.app.playback.note_presented(true);
    end_frame(&mut run.app);
    world_frame(&mut run.app, start, 0.0, false);
    assert_eq!(state(&run.app), RecordState::Waiting);
    assert!(run.app.playback.stall.is_some(), "no world frame yet");
    for frame in 1..=6 {
        world_frame(&mut run.app, start, f64::from(frame) / 60.0, frame != 4);
    }
    assert!(
        run.app.playback.stall.is_none(),
        "the first world frame disarms"
    );
    assert_eq!(
        run.app.playback.recording.as_ref().unwrap().samples().len(),
        5
    );
    assert!(!run.path.exists(), "no I/O until the one write");

    run.app.playback_session_exits();
    let file = written(&run.path);
    assert_eq!(file.header.origin, RouteOrigin::Recorded);
    assert_eq!(
        file.header.bake,
        postretro_camera_route::bake_identity(FIRST_LEVEL, run.app.level.as_ref().unwrap())
    );
    assert_eq!(file.header.param("frames"), Some("5"));
    assert!(file.header.param("source_hz").is_some());
    assert_eq!(file.header.cadence_hz, 60);
    assert_eq!(file.header.spawn, None);
    // Recorded from t = 1/60 s to 6/60 s: poses at 0 .. 5/60 s after it.
    assert_eq!(file.poses.len(), 6);
    assert!(file.poses[0].eye.distance(motion(1.0 / 60.0).0) < 1.0e-6);
    assert!(file.poses[3].eye.distance(motion(4.0 / 60.0).0) < 1.0e-4);
    assert!(exit_ok(&run.app));
}

fn exit_ok(app: &App) -> bool {
    app.exit_result.is_ok()
}

#[test]
fn recording_records_nothing_for_a_frontend_backdrop_then_starts_at_the_started_map() {
    // A recording launched into a mod with a frontend backdrop: backdrop
    // frames record nothing; Start loads the map, whose first world frame
    // starts the file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rec.txt");
    let mut app = test_app();
    app.playback = Playback::from_args(&launch(&path), Instant::now()).unwrap();
    app.backdrop_installed = true;
    let start = Instant::now();
    for frame in 0..5 {
        world_frame(&mut app, start, f64::from(frame) / 60.0, true);
    }
    assert_eq!(state(&app), RecordState::Waiting);
    assert!(app.playback.stall.is_some());

    app.backdrop_installed = false;
    install(&mut app, FIRST_LEVEL);
    for frame in 10..13 {
        world_frame(&mut app, start, f64::from(frame) / 60.0, true);
    }
    app.playback_session_exits();
    let file = written(&path);
    assert_eq!(file.header.param("frames"), Some("3"));
    assert_eq!(file.poses[0].eye, motion(10.0 / 60.0).0);
}

#[test]
fn recording_level_end_writes_the_first_levels_frames_and_nothing_after() {
    // A level change, an unload, a reload and a restart (a reload of the
    // installed source) each write the frames before the request drained;
    // nothing after it records, the next level included.
    for (name, request) in [
        (
            "change",
            LevelRequest::Load(LevelSource::Path(PathBuf::from("maps/next.prl"))),
        ),
        ("unload", LevelRequest::Unload),
        (
            "reload",
            LevelRequest::Load(LevelSource::Path(PathBuf::from(FIRST_LEVEL))),
        ),
    ] {
        let mut run = recording();
        let start = Instant::now();
        for frame in 0..4 {
            world_frame(&mut run.app, start, f64::from(frame) / 60.0, true);
        }
        run.app.enqueue_level_request(request);
        assert!(
            !run.path.exists(),
            "{name}: written when the request drains"
        );
        run.app.drain_level_requests();
        assert_eq!(state(&run.app), RecordState::Ended, "{name}");
        let bytes = std::fs::read(&run.path).unwrap();
        let file = written(&run.path);
        assert_eq!(file.header.param("frames"), Some("4"), "{name}");
        assert_eq!(file.poses.len(), 4, "{name}");

        // The next level records nothing, and exit writes nothing more.
        install(&mut run.app, "maps/next.prl");
        for frame in 10..14 {
            world_frame(&mut run.app, start, f64::from(frame) / 60.0, true);
        }
        run.app.playback_session_exits();
        assert_eq!(std::fs::read(&run.path).unwrap(), bytes, "{name}");
    }
}

#[test]
fn recording_with_no_world_frame_writes_no_file_and_warns_once() {
    let capture = LogCapture::start();
    let mut run = recording();
    world_frame(&mut run.app, Instant::now(), 0.0, false);
    run.app.playback_session_exits();
    run.app.playback_session_exits();
    assert!(!run.path.exists());
    capture.assert_logged_once(
        log::Level::Warn,
        "the session ended before its first world frame; no camera route recorded",
    );
}

#[test]
fn recording_is_written_once_with_the_same_bytes_whichever_exit_route_ended_it() {
    // AC 48: window close, the quit chord, a menu quit and a script quit all
    // reach `exiting`, whose hook writes the file once, the same whichever
    // route ended the session.
    let start = Instant::now();
    let mut files = Vec::new();
    for route in ["window close", "quit chord", "menu quit", "script quit"] {
        let mut run = recording();
        for frame in 0..8 {
            world_frame(&mut run.app, start, f64::from(frame) / 45.0, true);
        }
        // A menu or script quit sets the exit-to-desktop flag the frame acts
        // on; a close or the chord exits from the event directly.
        run.app.pending_exit_to_desktop = matches!(route, "menu quit" | "script quit");
        run.app.playback_session_exits();
        let bytes = std::fs::read(&run.path).unwrap();
        std::fs::remove_file(&run.path).unwrap();
        run.app.playback_session_exits();
        assert!(!run.path.exists(), "{route}: written once");
        files.push(bytes);
    }
    assert!(files.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn recorded_route_replays_frame_locked_bit_equal_with_no_roll_at_its_fov() {
    // AC 44, 45: played frame-locked, route frame k draws pose k of the
    // resampled file bit-equal, with no roll or FOV offset. A teleport
    // mid-recording cuts a pose, which round-trips and replays the same.
    let mut run = recording();
    let pawn = follow_pawn(&run.app);
    let start = Instant::now();
    for frame in 0..12 {
        if frame == 7 {
            move_pawn(&run.app, pawn, teleport);
        }
        let offset = if frame >= 7 { AWAY } else { Vec3::ZERO };
        world_frame_moved(&mut run.app, start, f64::from(frame) / 40.0, offset);
    }
    let expected = resample(run.app.playback.recording.as_ref().unwrap().samples(), 60.0);
    assert_eq!(expected.iter().filter(|pose| pose.cut).count(), 1);
    run.app.playback_session_exits();
    let file = written(&run.path);
    assert_eq!(file.poses, expected, "the write round-trips bit-equal");

    let report = run.path.with_extension("playback.json");
    let args: Vec<String> = [
        "postretro",
        "--camera-route",
        &run.path.display().to_string(),
        "--camera-route-hold",
        "0",
        "--camera-route-report",
        &report.display().to_string(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let mut app = test_app();
    app.playback = Playback::from_args(&args, Instant::now()).unwrap();
    install(&mut app, FIRST_LEVEL);
    assert!(
        app.playback.captures_gameplay(),
        "the recording's bake matches"
    );
    let hfov = file.header.projection.hfov_degrees.to_radians();
    for (k, pose) in expected.iter().enumerate() {
        app.playback_begin_world_frame(start);
        let view = app.playback.route_view().expect("a route frame");
        assert_eq!(
            (view.eye, view.yaw.to_bits(), view.pitch.to_bits()),
            (pose.eye, pose.yaw.to_bits(), pose.pitch.to_bits()),
            "route frame {k}"
        );
        assert_eq!(view.hfov, hfov);
        let eye = frame_eye::assemble_frame_eye(
            FrameEyeInputs {
                presented_eye: view.eye,
                aspect: ASPECT,
                base_hfov: view.hfov,
                render_yaw: view.yaw,
                pitch: view.pitch,
                driver: None,
                movement_edges: &[],
                frame_dt: 1.0 / 60.0,
                view_feel_scale: 1.0,
            },
            ViewFeelTracking {
                state: &mut ViewFeelState::default(),
                followed_pawn: &mut None,
                descriptor: &mut None,
            },
        );
        assert_eq!(eye.roll, 0.0);
        let unfelt = RenderCamera::with_base_hfov(
            view.eye,
            ASPECT,
            view.yaw,
            view.pitch,
            0.0,
            Vec3::ZERO,
            0.0,
            hfov,
        );
        assert_eq!(eye.camera.view_projection, unfelt.view_projection);
        app.playback.note_presented(true);
        app.playback_finish_frame();
    }
    assert!(app.playback.exit_pending, "the last pose ends the run");
    assert!(app.exit_result.is_ok());
}

#[test]
fn recording_never_blends_across_a_teleport_or_respawn_and_cuts_at_the_arrival() {
    // A teleport at 0.125 s moves the camera to AWAY; a respawn at 0.225 s
    // puts it back. Frames at 40 Hz, poses at 60 Hz: a pose between a jump's
    // two frames holds the frame before it, and the first pose past each
    // jump is cut. No pose lies between the two places.
    let mut run = recording();
    let pawn = follow_pawn(&run.app);
    let start = Instant::now();
    for frame in 0..16 {
        match frame {
            5 => move_pawn(&run.app, pawn, teleport),
            9 => move_pawn(&run.app, pawn, respawn),
            _ => {}
        }
        let offset = if (5..9).contains(&frame) {
            AWAY
        } else {
            Vec3::ZERO
        };
        world_frame_moved(&mut run.app, start, f64::from(frame) / 40.0, offset);
    }
    let recorded = run
        .app
        .playback
        .recording
        .as_ref()
        .unwrap()
        .samples()
        .to_vec();
    run.app.playback_session_exits();
    let file = written(&run.path);
    assert_eq!(file.poses.len(), 23);

    // Frames 4 (0.1 s) and 5 (0.125 s) bracket the teleport, 8 (0.2 s) and
    // 9 (0.225 s) the respawn.
    let cuts: Vec<usize> = (0..file.poses.len())
        .filter(|&k| file.poses[k].cut)
        .collect();
    assert_eq!(cuts, [8, 14], "the first pose past each jump");
    for (k, frame) in [(7, 4), (13, 8)] {
        let (pose, sample) = (&file.poses[k], &recorded[frame]);
        assert_eq!(
            (pose.eye, pose.yaw.to_bits(), pose.pitch.to_bits()),
            (sample.eye, sample.yaw.to_bits(), sample.pitch.to_bits()),
            "pose {k} holds frame {frame}"
        );
    }
    for (k, pose) in file.poses.iter().enumerate() {
        let place = if (8..14).contains(&k) {
            AWAY
        } else {
            Vec3::ZERO
        };
        let (eye, _, _) = motion(k as f64 / 60.0);
        assert!(
            pose.eye.distance(eye + place) <= SPEED / 40.0 + 1.0e-4,
            "pose {k} is at one place or the other: {pose:?}"
        );
    }
}

#[test]
fn a_recording_playback_would_refuse_writes_no_file_and_fails_the_exit() {
    // No gameplay level's bake named the recording, or the level had no
    // path: the file would not parse, so none is written.
    let capture = LogCapture::start();
    for case in ["no level", "no level path"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.txt");
        let mut app = test_app();
        app.playback = Playback::from_args(&launch(&path), Instant::now()).unwrap();
        if case == "no level path" {
            app.level = Some(level_world("route", 1));
            app.map_path = None;
            app.playback_level_installed();
        }
        world_frame(&mut app, Instant::now(), 0.0, true);
        assert_eq!(state(&app), RecordState::Recording, "{case}");
        app.playback_session_exits();
        assert!(!path.exists(), "{case}: no file playback would refuse");
        assert!(app.exit_result.is_err(), "{case}: the exit fails");
    }
    capture.assert_logged(log::Level::Error, "would not parse");
}

#[test]
fn a_failed_recording_write_fails_the_exit_and_keeps_an_earlier_error() {
    for earlier in [None, Some("an earlier failure")] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let path = out.join("rec.txt");
        let mut app = test_app();
        app.playback = Playback::from_args(&launch(&path), Instant::now()).unwrap();
        install(&mut app, FIRST_LEVEL);
        world_frame(&mut app, Instant::now(), 0.0, true);
        // The directory goes away before the write.
        std::fs::remove_dir(&out).unwrap();
        if let Some(earlier) = earlier {
            app.exit_result = Err(anyhow::anyhow!(earlier));
        }
        app.playback_session_exits();
        assert!(!path.exists());
        let error = app.exit_result.as_ref().unwrap_err().to_string();
        match earlier {
            Some(earlier) => assert_eq!(error, earlier),
            None => assert!(error.contains("writing the camera route"), "{error}"),
        }
    }
}

#[test]
fn playback_closed_by_the_person_writes_its_record_aborted_and_exits_non_zero() {
    let mut run = crate::playback::lifecycle::tests::playing(1, 5, &[]);
    let now = Instant::now();
    for _ in 0..3 {
        crate::playback::lifecycle::tests::frame(&mut run.app, now, true, true);
    }
    run.app.playback_session_exits();
    assert!(run.app.playback.active.is_none());
    let exit = run.app.exit_result.as_ref().expect_err("a close before the route ends is an aborted run");
    assert!(exit.to_string().contains("window closed or quit before the route ended"));
    let record = crate::playback::lifecycle::tests::record(&run);
    assert_eq!(record["header"]["outcome"], "aborted");
    assert!(
        record["header"]["abort_reason"]
            .as_str()
            .unwrap()
            .contains("window closed")
    );
    assert_eq!(record["rows"].as_array().unwrap().len(), 3);
}

#[test]
fn inactive_recording_hooks_allocate_nothing() {
    // AC 30: with no recording, a frame allocates nothing for it.
    let mut app = test_app();
    let start = Instant::now();
    let inputs = eye_inputs(0.0, None);
    let control = crate::alloc_probe::AllocSnapshot::arm();
    std::hint::black_box(Box::new(7_u64));
    assert!(
        control.allocs_since() >= 1,
        "probe must count in this binary"
    );

    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for _ in 0..240 {
        app.playback_record_eye(start, &inputs);
        app.playback.note_presented(true);
        end_frame(&mut app);
        app.recording_level_ends();
    }
    assert_eq!(probe.allocs_since(), 0);
    assert!(app.playback.recording.is_none());
}

#[test]
fn a_recording_frame_within_capacity_allocates_nothing() {
    let mut run = recording();
    let start = Instant::now();
    world_frame(&mut run.app, start, 0.0, true);
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for frame in 1..240 {
        world_frame(&mut run.app, start, f64::from(frame) / 60.0, true);
    }
    assert_eq!(probe.allocs_since(), 0);
}

// --- Source order ---

const FRAME_LOOP: &str = include_str!("../frame_loop/mod.rs");
const MAIN: &str = include_str!("../main.rs");
const BOOT_STATE: &str = include_str!("../startup/lifecycle_boot_state.rs");
const SETTLING: &str = include_str!("../startup/settling.rs");
const SESSION_ENTRY: &str = include_str!("../startup/session_entry.rs");

fn at(haystack: &str, needle: &str, from: usize) -> usize {
    from + haystack[from..]
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found after {from}"))
}

#[test]
fn frame_loop_records_the_eye_inputs_before_view_feel_and_after_the_aim() {
    let redraw = at(FRAME_LOOP, "pub(crate) fn redraw(", 0);
    let aim = at(FRAME_LOOP, "app.render_aim_pose(presented_aim)", redraw);
    let inputs = at(
        FRAME_LOOP,
        "let eye_inputs = presented_aim.frame_eye_inputs(",
        aim,
    );
    let record = at(
        FRAME_LOOP,
        "app.playback_record_eye(now, &eye_inputs);",
        inputs,
    );
    let eye = at(FRAME_LOOP, "frame_eye::assemble_frame_eye(", record);
    assert!(
        FRAME_LOOP[eye..]
            .trim_start_matches("frame_eye::assemble_frame_eye(")
            .trim_start()
            .starts_with("eye_inputs,")
    );
    let presented = at(FRAME_LOOP, "app.playback.note_presented(presented);", eye);
    let end = at(FRAME_LOOP, "app.playback_end_frame(event_loop);", presented);
    assert!(aim < inputs && record < eye && eye < presented && presented < end);
    assert!(
        !SETTLING.contains("playback_record_eye"),
        "a Settling frame offers no eye"
    );
}

#[test]
fn level_requests_end_a_recording_before_the_level_unloads() {
    let drain = at(BOOT_STATE, "pub(crate) fn drain_level_requests(", 0);
    let load_end = at(BOOT_STATE, "self.recording_level_ends();", drain);
    let load_unload = at(BOOT_STATE, "self.unload_level();", load_end);
    let begin = at(BOOT_STATE, "self.begin_level_load(load);", load_unload);
    let none_end = at(BOOT_STATE, "self.recording_level_ends();", begin);
    let frontend = at(
        BOOT_STATE,
        "self.abandon_level_load_for_frontend();",
        none_end,
    );
    let none_unload = at(BOOT_STATE, "self.unload_level();", frontend);
    assert!(load_end < load_unload && none_end < frontend && frontend < none_unload);
}

#[test]
fn a_frontend_join_ends_a_recording_after_the_playback_abort_check() {
    // A join leaves for the host's level: a recording of the first gameplay
    // level is written before it, unless a playing route refuses the join.
    let join = at(SESSION_ENTRY, "pub(crate) fn join_from_frontend_with(", 0);
    let abort = at(SESSION_ENTRY, "self.playback_join_abort_reason(addr)", join);
    let end = at(SESSION_ENTRY, "self.recording_level_ends();", abort);
    let connect = at(SESSION_ENTRY, "self.enter_net_role(", join);
    assert!(abort < end && end < connect);
}

#[test]
fn every_exit_route_reaches_the_exiting_hook_before_teardown() {
    let exiting = at(MAIN, "fn exiting(", 0);
    let hook = at(MAIN, "self.playback_session_exits();", exiting);
    let teardown = at(MAIN, "self.exit_saves_and_teardown();", exiting);
    assert!(hook < teardown);
    for (route, from) in [
        ("window close", "WindowEvent::CloseRequested =>"),
        ("quit chord", "input::escape_is_dev_quit_chord("),
        ("menu or script quit", "if self.pending_exit_to_desktop {"),
    ] {
        let start = at(MAIN, from, 0);
        let exit = at(MAIN, "event_loop.exit();", start);
        assert!(exit - start < 1_500, "{route} exits the event loop");
    }
}
