// Playback's App-facing seams: launch loading, the route's frame eye, and the
// held view Settling streams from.
// See: context/lib/rendering_pipeline.md §11, §12

use glam::Vec3;
use postretro_camera_route::format_pose_file;
use postretro_entities::EntityId;
use postretro_foundation::{BobParams, MovementStateKind, ViewFeelParams};

use super::*;
use crate::frame_eye::{FrameEye, ViewFeelDriver, ViewFeelTracking, assemble_frame_eye};
use crate::playback::route::tests::pose_file;
use crate::view_feel::ViewFeelState;

const ASPECT: f32 = 16.0 / 10.0;

fn args(list: &[&str]) -> Vec<String> {
    std::iter::once("postretro")
        .chain(list.iter().copied())
        .map(str::to_string)
        .collect()
}

fn bobbing_driver() -> ViewFeelDriver {
    ViewFeelDriver {
        pawn: EntityId::from_raw(1),
        params: ViewFeelParams {
            bob: Some(BobParams {
                vertical_frequency: 1.0,
                lateral_frequency: 0.5,
                vertical_amplitude: 0.1,
                lateral_amplitude: 0.05,
                speed_threshold: 0.5,
                grounded_only: false,
            }),
            tilt: None,
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

/// One frame's eye as the frame loop assembles it: the presented aim through
/// `frame_eye_inputs`, then one view-feel evaluation.
fn frame_eye(
    aim: crate::PresentedAimPose,
    base_hfov: f32,
    driver: Option<ViewFeelDriver>,
    state: &mut ViewFeelState,
) -> FrameEye {
    let mut followed = None;
    let mut descriptor = None;
    assemble_frame_eye(
        aim.frame_eye_inputs(ASPECT, base_hfov, driver, &[], 0.25, 1.0),
        ViewFeelTracking {
            state,
            followed_pawn: &mut followed,
            descriptor: &mut descriptor,
        },
    )
}

fn route_aim(view: RouteView) -> crate::PresentedAimPose {
    crate::PresentedAimPose {
        position: view.eye,
        yaw: view.yaw,
        pitch: view.pitch,
    }
}

fn assert_camera_bits(actual: &RenderCamera, expected: &RenderCamera, what: &str) {
    assert_eq!(
        actual.eye_position.to_array().map(f32::to_bits),
        expected.eye_position.to_array().map(f32::to_bits),
        "{what}: eye"
    );
    assert_eq!(
        actual.forward.to_array().map(f32::to_bits),
        expected.forward.to_array().map(f32::to_bits),
        "{what}: yaw/pitch forward"
    );
    assert_eq!(
        actual.view_projection.to_cols_array().map(f32::to_bits),
        expected.view_projection.to_cols_array().map(f32::to_bits),
        "{what}: view projection"
    );
}

#[test]
fn route_frame_eye_is_the_pose_at_the_file_fov_with_no_view_feel() {
    let view = RouteView {
        eye: Vec3::new(12.5, 1.75, -40.25),
        yaw: 1.2,
        pitch: -0.15,
        hfov: 90.0_f32.to_radians(),
    };
    // The followed pawn has been driving view feel: a bob and impulse state
    // live in the integrator when the route frame bypasses it.
    let mut state = ViewFeelState::default();
    let felt = frame_eye(
        route_aim(view),
        view.hfov,
        Some(bobbing_driver()),
        &mut state,
    );
    assert_ne!(felt.eye_offset, Vec3::ZERO, "view feel moves a pawn's eye");

    let route = frame_eye(route_aim(view), view.hfov, None, &mut state);
    let expected = RenderCamera::with_base_hfov(
        view.eye,
        ASPECT,
        view.yaw,
        view.pitch,
        0.0,
        Vec3::ZERO,
        0.0,
        view.hfov,
    );
    assert_camera_bits(&route.camera, &expected, "file FOV");
    assert_eq!(
        route.camera.eye_position, view.eye,
        "render eye is the pose eye"
    );
    assert_eq!((route.roll, route.eye_offset), (0.0, Vec3::ZERO));

    // At the engine's own FOV the route frame is today's camera at the pose.
    let at_hfov = frame_eye(route_aim(view), crate::camera::HFOV, None, &mut state);
    let today = RenderCamera::new(view.eye, ASPECT, view.yaw, view.pitch, 0.0, Vec3::ZERO, 0.0);
    assert_camera_bits(&at_hfov.camera, &today, "HFOV");
}

#[test]
fn settling_held_view_is_the_route_frame_camera() {
    // P5: the held view Settling streams from is the reveal frame's camera.
    let view = RouteView {
        eye: Vec3::new(-3.0, 2.0, 8.0),
        yaw: -0.7,
        pitch: 0.3,
        hfov: 110.0_f32.to_radians(),
    };
    let held = view.held_view(ASPECT);
    let route = frame_eye(
        route_aim(view),
        view.hfov,
        None,
        &mut ViewFeelState::default(),
    );
    assert_camera_bits(&held.camera, &route.camera, "held vs route frame");
}

#[test]
fn playback_from_args_is_inactive_without_a_route() {
    let playback = Playback::from_args(&args(&["map.prl"])).unwrap();
    assert!(playback.active.is_none());
    assert!(!playback.captures_gameplay());
    assert_eq!(playback.route_view(), None);
}

#[test]
fn playback_from_args_loads_the_route_and_refuses_a_bad_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cp.txt");
    std::fs::write(&path, format_pose_file(&pose_file(100.0, 3))).unwrap();
    let path_arg = path.display().to_string();
    let mut playback =
        Playback::from_args(&args(&["--camera-route", &path_arg, "map.prl"])).unwrap();
    {
        let active = playback.active.as_ref().expect("a named route is active");
        assert_eq!(active.args.hold, args::DEFAULT_HOLD_FRAMES);
        assert_eq!(active.hfov, 100.0_f32.to_radians());
        assert_eq!(active.run.phase(), Phase::AwaitingInstall);
    }
    assert!(
        !playback.captures_gameplay(),
        "nothing is suspended before the route's level installs"
    );
    let active = playback.active.as_mut().unwrap();
    active.run.arm();
    assert!(playback.captures_gameplay());

    let bad = dir.path().join("bad.txt");
    std::fs::write(&bad, format_pose_file(&pose_file(140.0, 1))).unwrap();
    let error = Playback::from_args(&args(&["--camera-route", &bad.display().to_string()]))
        .err()
        .expect("a FOV outside the band refuses");
    assert!(error.contains("140°"), "{error}");
}

#[test]
fn route_view_follows_the_world_frame_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cp.txt");
    let file = pose_file(100.0, 2);
    std::fs::write(&path, format_pose_file(&file)).unwrap();
    let mut playback = Playback::from_args(&args(&[
        "--camera-route",
        &path.display().to_string(),
        "--camera-route-hold",
        "1",
    ]))
    .unwrap();
    let active = playback.active.as_mut().unwrap();
    active.run.arm();
    let mut drawn = Vec::new();
    for _ in 0..3 {
        let active = playback.active.as_mut().unwrap();
        active.run.begin_world_frame();
        drawn.push(playback.route_view().unwrap().eye);
        let active = playback.active.as_mut().unwrap();
        active.run.finish_frame(true);
        assert_eq!(playback.route_view(), None, "no pose outside a world frame");
    }
    assert_eq!(
        drawn,
        vec![file.poses[0].eye, file.poses[0].eye, file.poses[1].eye]
    );
}
