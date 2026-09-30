// Per-frame eye assembly: view feel is evaluated once, into the one
// `RenderCamera` that render and the audio listener both read.
// See: context/lib/rendering_pipeline.md §11 · context/lib/audio.md §3

use glam::Vec3;
use postretro_entities::EntityId;
use postretro_foundation::{MovementStateKind, ViewFeelParams};

use crate::camera::RenderCamera;
use crate::view_feel::{self, TimedMovementEdge, ViewFeelState};

/// The pawn driving view feel this frame, with the movement facts the
/// evaluator reads. Absent when no followed pawn carries `view_feel`.
pub(crate) struct ViewFeelDriver {
    pub(crate) pawn: EntityId,
    pub(crate) params: ViewFeelParams,
    pub(crate) velocity: Vec3,
    pub(crate) is_grounded: bool,
    pub(crate) movement_state: MovementStateKind,
    /// Baseline eye clearance from the pawn's feet, before render offsets.
    pub(crate) eye_height_above_feet: f32,
}

/// Everything one frame's eye depends on.
pub(crate) struct FrameEyeInputs<'a> {
    /// Interpolated (presented) eye before view-feel offsets.
    pub(crate) presented_eye: Vec3,
    pub(crate) aspect: f32,
    /// Carry-yaw-adjusted render yaw.
    pub(crate) render_yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) driver: Option<ViewFeelDriver>,
    pub(crate) movement_edges: &'a [TimedMovementEdge],
    pub(crate) frame_dt: f32,
    /// Accessibility scale, owned and clamped by the options module.
    pub(crate) view_feel_scale: f32,
}

/// The render-rate view-feel integrator and the driver it was last synced to.
pub(crate) struct ViewFeelTracking<'a> {
    pub(crate) state: &'a mut ViewFeelState,
    pub(crate) followed_pawn: &'a mut Option<EntityId>,
    pub(crate) descriptor: &'a mut Option<ViewFeelParams>,
}

/// One frame's evaluated eye, plus the view-feel offsets the first-person
/// viewmodel re-applies in camera space.
pub(crate) struct FrameEye {
    pub(crate) camera: RenderCamera,
    /// Yaw-derived, Y-free unit right vector the offsets were mapped onto.
    pub(crate) camera_right: Vec3,
    pub(crate) eye_offset: Vec3,
    pub(crate) roll: f32,
    pub(crate) yaw_offset: f32,
    pub(crate) pitch_offset: f32,
}

/// Eye clearance in the same presented space as the interpolated camera.
/// The latest tick eye already includes client correction, so only its delta
/// from the rendered eye is needed when capsule resize precedes interpolation.
pub(crate) fn presented_eye_clearance(
    presented_eye_y: f32,
    latest_tick_eye_y: f32,
    latest_tick_clearance: f32,
) -> f32 {
    latest_tick_clearance + (presented_eye_y - latest_tick_eye_y)
}

/// Evaluate view feel once and assemble this frame's eye. The integrator
/// advances by `frame_dt` exactly once per call, so a frame must call this
/// once and share the result.
///
/// When a pawn drives view feel, its output folds into the look angles, roll,
/// and eye offset. Otherwise the pass-through path (`roll = 0`, no offsets)
/// keeps the matrix bit-identical to the no-view-feel render. The evaluator
/// never sees the camera basis: its two velocity-space inputs are derived here
/// from the pawn velocity and the camera right vector, taken from the same
/// carry-yaw-adjusted yaw that enters `RenderCamera`, so view feel and the view
/// matrix never disagree during a sub-tick turntable rotation.
pub(crate) fn assemble_frame_eye(
    inputs: FrameEyeInputs<'_>,
    tracking: ViewFeelTracking<'_>,
) -> FrameEye {
    let camera_right = crate::camera_right_for_yaw(inputs.render_yaw);
    let (fov_offset, roll, yaw_offset, pitch_offset, eye_offset) = match inputs.driver {
        Some(driver) => {
            crate::sync_view_feel_driver(
                tracking.state,
                tracking.followed_pawn,
                tracking.descriptor,
                Some((driver.pawn, &driver.params)),
            );
            let (horizontal_speed, lateral_velocity) =
                view_feel::view_feel_inputs(driver.velocity, camera_right);
            let output = view_feel::evaluate_with_edges(
                &driver.params,
                horizontal_speed,
                lateral_velocity,
                driver.is_grounded,
                inputs.movement_edges,
                tracking.state,
                // The evaluator leaves the integrator untouched at
                // `frame_dt == 0`, so a zero-length frame passes straight through.
                inputs.frame_dt,
                inputs.view_feel_scale,
            );
            let (roll, yaw, pitch, mut eye) =
                view_feel::map_output_to_camera(&output, camera_right);
            let slide = view_feel::slide::evaluate(
                driver.params.slide.as_ref(),
                driver.movement_state,
                tracking.state,
                inputs.frame_dt,
                inputs.view_feel_scale,
            );
            // Bound the added dip after bob. Do not lift an already low baseline eye.
            let available_drop = (driver.eye_height_above_feet + eye.y
                - view_feel::slide::MIN_EYE_CLEARANCE)
                .max(0.0);
            eye.y -= slide.eye_drop.min(available_drop);
            (
                output.impulse_fov + slide.fov_increase,
                roll,
                yaw,
                pitch,
                eye,
            )
        }
        None => {
            crate::sync_view_feel_driver(
                tracking.state,
                tracking.followed_pawn,
                tracking.descriptor,
                None,
            );
            (0.0, 0.0, 0.0, 0.0, Vec3::ZERO)
        }
    };

    FrameEye {
        camera: RenderCamera::new(
            inputs.presented_eye,
            inputs.aspect,
            inputs.render_yaw + yaw_offset,
            inputs.pitch + pitch_offset,
            roll,
            eye_offset,
            fov_offset,
        ),
        camera_right,
        eye_offset,
        roll,
        yaw_offset,
        pitch_offset,
    }
}

/// The audio listener for a frame: the rendered eye and its look direction.
/// `attached` names the pawn the listener rides, whose own sounds play
/// unpositioned.
pub(crate) fn listener_for(
    eye: &RenderCamera,
    attached: Option<u64>,
) -> postretro_audio::ListenerState {
    postretro_audio::ListenerState {
        position: eye.eye_position.to_array(),
        forward: eye.forward.to_array(),
        up: [0.0, 1.0, 0.0],
        attached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::BobParams;

    const YAW: f32 = 0.3;
    const PITCH: f32 = -0.2;
    const FRAME_DT: f32 = 0.25;
    const VELOCITY: Vec3 = Vec3::new(6.0, 0.0, 0.0);

    fn bobbing() -> ViewFeelParams {
        ViewFeelParams {
            slide: None,
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
        }
    }

    fn pawn() -> EntityId {
        EntityId::from_raw(1)
    }

    fn inputs() -> FrameEyeInputs<'static> {
        FrameEyeInputs {
            presented_eye: Vec3::new(1.0, 2.0, 3.0),
            aspect: 16.0 / 9.0,
            render_yaw: YAW,
            pitch: PITCH,
            driver: Some(ViewFeelDriver {
                pawn: pawn(),
                params: bobbing(),
                velocity: VELOCITY,
                is_grounded: true,
                movement_state: MovementStateKind::Normal,
                eye_height_above_feet: 1.0,
            }),
            movement_edges: &[],
            frame_dt: FRAME_DT,
            view_feel_scale: 1.0,
        }
    }

    fn assemble(state: &mut ViewFeelState) -> FrameEye {
        let mut followed = None;
        let mut descriptor = None;
        assemble_frame_eye(
            inputs(),
            ViewFeelTracking {
                state,
                followed_pawn: &mut followed,
                descriptor: &mut descriptor,
            },
        )
    }

    #[test]
    fn listener_is_the_rendered_eye_including_the_view_feel_offset() {
        let mut state = ViewFeelState::default();
        let eye = assemble(&mut state);
        assert_ne!(eye.eye_offset, Vec3::ZERO, "bob moves the eye this frame");

        let listener = listener_for(&eye.camera, Some(9));
        assert_eq!(listener.position, eye.camera.eye_position.to_array());
        assert_eq!(
            eye.camera.eye_position,
            Vec3::new(1.0, 2.0, 3.0) + eye.eye_offset,
            "the listener sits at the view-feel-offset eye, not the bare interpolated one",
        );
        assert_eq!(listener.forward, eye.camera.forward.to_array());
        assert_eq!(listener.attached, Some(9));
    }

    // Pin P10: the eye is evaluated once per frame, ahead of audio, and shared.
    #[test]
    fn frame_eye_advances_view_feel_exactly_once() {
        let mut state = ViewFeelState::default();
        assemble(&mut state);

        let mut expected = ViewFeelState::default();
        crate::sync_view_feel_driver(
            &mut expected,
            &mut None,
            &mut None,
            Some((pawn(), &bobbing())),
        );
        let (horizontal, lateral) =
            view_feel::view_feel_inputs(VELOCITY, crate::camera_right_for_yaw(YAW));
        let evaluate = |state: &mut ViewFeelState| {
            view_feel::evaluate_with_edges(
                &bobbing(),
                horizontal,
                lateral,
                true,
                &[],
                state,
                FRAME_DT,
                1.0,
            );
        };
        evaluate(&mut expected);
        assert_eq!(state, expected, "one assembly is one integrator step");

        let mut twice = expected;
        evaluate(&mut twice);
        assert_ne!(state, twice, "a second evaluation would be observable");
    }

    fn slide_params() -> postretro_foundation::SlideViewParams {
        postretro_foundation::SlideViewParams {
            eye_drop: 0.15,
            fov_increase: 5.0,
            enter_rate: 18.0,
            exit_rate: 12.0,
        }
    }

    #[test]
    fn slide_eye_composes_dip_fov_and_listener_then_recovers_to_crouch() {
        let mut state = ViewFeelState::default();
        let mut followed = None;
        let mut descriptor = None;
        let mut frame = |kind, scale, dt| {
            let mut input = inputs();
            let driver = input.driver.as_mut().unwrap();
            driver.params.bob = None;
            driver.params.slide = Some(slide_params());
            driver.movement_state = kind;
            input.view_feel_scale = scale;
            input.frame_dt = dt;
            assemble_frame_eye(
                input,
                ViewFeelTracking {
                    state: &mut state,
                    followed_pawn: &mut followed,
                    descriptor: &mut descriptor,
                },
            )
        };
        let sliding = frame(MovementStateKind::Slide, 1.0, 1.0);
        assert!((sliding.eye_offset.y + 0.15).abs() < 1e-5);
        let expected = RenderCamera::new(
            Vec3::new(1.0, 2.0, 3.0),
            16.0 / 9.0,
            YAW,
            PITCH,
            0.0,
            sliding.eye_offset,
            5.0,
        );
        for (a, b) in sliding
            .camera
            .view_projection
            .to_cols_array()
            .iter()
            .zip(expected.view_projection.to_cols_array())
        {
            assert!((a - b).abs() < 1e-5);
        }
        assert_eq!(
            listener_for(&sliding.camera, None).position,
            sliding.camera.eye_position.to_array()
        );
        let suppressed = frame(MovementStateKind::Slide, 0.0, 1.0 / 60.0);
        assert_eq!(suppressed.eye_offset, Vec3::ZERO);
        let recovering = frame(MovementStateKind::Crouch, 1.0, 1.0 / 60.0);
        assert!(recovering.eye_offset.y > sliding.eye_offset.y && recovering.eye_offset.y < 0.0);
        let recovered = frame(MovementStateKind::Crouch, 1.0, 2.0);
        assert!(recovered.eye_offset.y.abs() < 1e-5);
    }

    #[test]
    fn slide_eye_keeps_clearance_during_feet_planted_stand_interpolation() {
        let mut state = ViewFeelState::default();
        let mut input = inputs();
        // Feet remain at zero while the latest tick stands at 1.7 m. The
        // displayed eye still interpolates from the prior crouched pose.
        input.presented_eye.y = 0.9;
        let driver = input.driver.as_mut().unwrap();
        driver.params.bob = None;
        driver.params.slide = Some(postretro_foundation::SlideViewParams {
            eye_drop: 100.0,
            ..slide_params()
        });
        driver.eye_height_above_feet = presented_eye_clearance(0.9, 1.7, 1.7);
        driver.movement_state = MovementStateKind::Slide;
        let mut followed = None;
        let mut descriptor = None;
        let active = assemble_frame_eye(
            input,
            ViewFeelTracking {
                state: &mut state,
                followed_pawn: &mut followed,
                descriptor: &mut descriptor,
            },
        );
        assert!((active.camera.eye_position.y - view_feel::slide::MIN_EYE_CLEARANCE).abs() < 1e-5);
        let mut input = inputs();
        input.presented_eye.y = 0.9;
        let driver = input.driver.as_mut().unwrap();
        driver.params = descriptor.clone().unwrap();
        driver.eye_height_above_feet = presented_eye_clearance(0.9, 1.7, 1.7);
        driver.movement_state = MovementStateKind::Normal;
        let exiting = assemble_frame_eye(
            input,
            ViewFeelTracking {
                state: &mut state,
                followed_pawn: &mut followed,
                descriptor: &mut descriptor,
            },
        );
        assert!(exiting.camera.eye_position.y >= view_feel::slide::MIN_EYE_CLEARANCE - 1e-5);
    }

    #[test]
    fn slide_eye_driver_changes_clear_active_slide_without_old_pawn_leakage() {
        let mut state = ViewFeelState::default();
        let mut followed = None;
        let mut descriptor = None;
        let mut input = inputs();
        let driver = input.driver.as_mut().unwrap();
        driver.params.bob = None;
        driver.params.slide = Some(slide_params());
        driver.movement_state = MovementStateKind::Slide;
        input.frame_dt = 1.0;
        let active = assemble_frame_eye(
            input,
            ViewFeelTracking {
                state: &mut state,
                followed_pawn: &mut followed,
                descriptor: &mut descriptor,
            },
        );
        assert!(active.eye_offset.y < -0.1);
        for replacement in [Some(pawn()), Some(EntityId::from_raw(2)), None] {
            let mut input = inputs();
            let driver = input.driver.as_mut().unwrap();
            driver.params.bob = None;
            driver.params.slide = Some(slide_params());
            driver.movement_state = MovementStateKind::Slide;
            // Seed a fresh held slide before each lifecycle change.
            input.frame_dt = 1.0;
            assemble_frame_eye(
                input,
                ViewFeelTracking {
                    state: &mut state,
                    followed_pawn: &mut followed,
                    descriptor: &mut descriptor,
                },
            );
            let mut input = inputs();
            let driver = input.driver.as_mut().unwrap();
            driver.params.bob = None;
            driver.params.slide = Some(slide_params());
            driver.movement_state = MovementStateKind::Slide;
            match replacement {
                Some(id) => {
                    driver.pawn = id;
                    if id == pawn() {
                        driver.params.slide.as_mut().unwrap().eye_drop = 0.2;
                    }
                }
                None => input.driver = None,
            }
            input.frame_dt = 0.0;
            let changed = assemble_frame_eye(
                input,
                ViewFeelTracking {
                    state: &mut state,
                    followed_pawn: &mut followed,
                    descriptor: &mut descriptor,
                },
            );
            assert!(changed.eye_offset.y.abs() < 1e-6);
            assert_eq!(state, ViewFeelState::default());
        }
    }

    #[test]
    fn slide_eye_limits_dip_after_bob_to_pawn_feet_clearance() {
        let mut state = ViewFeelState::default();
        let mut input = inputs();
        // At 6 m/s this puts vertical bob at its -0.1 m trough, so clamping
        // before bob would leave the eye below the required clearance.
        input.frame_dt = 0.125;
        let driver = input.driver.as_mut().unwrap();
        driver.movement_state = MovementStateKind::Slide;
        driver.eye_height_above_feet = 0.2;
        driver.params.slide = Some(postretro_foundation::SlideViewParams {
            eye_drop: 100.0,
            ..slide_params()
        });
        let eye = assemble_frame_eye(
            input,
            ViewFeelTracking {
                state: &mut state,
                followed_pawn: &mut None,
                descriptor: &mut None,
            },
        );
        assert!((eye.eye_offset.y + 0.15).abs() < 1e-5);
        assert!(
            (eye.camera.eye_position.y - (2.0 - 0.2 + view_feel::slide::MIN_EYE_CLEARANCE)).abs()
                < 1e-5
        );
        assert_ne!(eye.eye_offset.x, 0.0, "lateral bob still composes");
    }
}
