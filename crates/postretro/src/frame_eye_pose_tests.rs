// Tests: `viewFeel.pose` composed into the frame's eye — the eye-drop floor,
// camera-only angles, clamps, refresh — and the frame's driver.

use super::*;
use postretro_foundation::{
    ImpulseChannels, ImpulseParams, ImpulseStateParams, ImpulseStates, LedgeFocus, PoseChannels,
    PoseParams, PoseRow, VaultPoseRow,
};

const YAW: f32 = 0.3;
const PATH_S: f32 = 0.42;
const DT: f32 = 1.0 / 60.0;

fn no_view_feel() -> ViewFeelParams {
    ViewFeelParams {
        bob: None,
        tilt: None,
        sway: None,
        impulse: None,
        pose: None,
    }
}

fn pose(slide: Option<PoseRow>, vault: Option<VaultPoseRow>) -> ViewFeelParams {
    ViewFeelParams {
        pose: Some(PoseParams { slide, vault }),
        ..no_view_feel()
    }
}

fn channels(pitch: f32, roll: f32, eye_drop: f32, fov: f32) -> PoseChannels {
    PoseChannels {
        pitch,
        roll,
        eye_drop,
        fov,
    }
}

/// One frame's movement facts.
#[derive(Clone, Copy)]
struct Step {
    kind: MovementStateKind,
    vault: Option<VaultViewInput>,
    pitch: f32,
    frame_dt: f32,
    clearance: f32,
}

fn step(kind: MovementStateKind, frame_dt: f32) -> Step {
    Step {
        kind,
        vault: None,
        pitch: -0.2,
        frame_dt,
        clearance: 1.5,
    }
}

fn vault_step(progress: f32, landing: Option<Vec3>, frame_dt: f32) -> Step {
    Step {
        vault: Some(VaultViewInput {
            progress,
            path_s: PATH_S,
            landing,
        }),
        ..step(MovementStateKind::Vault, frame_dt)
    }
}

struct Rig {
    state: ViewFeelState,
    followed: Option<EntityId>,
    descriptor: Option<ViewFeelParams>,
}

impl Rig {
    fn new() -> Self {
        Self {
            state: ViewFeelState::default(),
            followed: None,
            descriptor: None,
        }
    }

    fn frame(&mut self, params: &ViewFeelParams, step: Step, scale: f32) -> FrameEye {
        self.frame_with_edges(params, step, scale, &[])
    }

    fn frame_with_edges(
        &mut self,
        params: &ViewFeelParams,
        step: Step,
        scale: f32,
        edges: &[view_feel::TimedMovementEdge],
    ) -> FrameEye {
        assemble_frame_eye(
            FrameEyeInputs {
                presented_eye: Vec3::new(1.0, 2.0, 3.0),
                aspect: 16.0 / 9.0,
                render_yaw: YAW,
                pitch: step.pitch,
                driver: Some(ViewFeelDriver {
                    pawn: EntityId::from_raw(1),
                    params: params.clone(),
                    velocity: Vec3::new(6.0, 0.0, 0.0),
                    is_grounded: true,
                    movement_state: step.kind,
                    vault: step.vault,
                    eye_height_above_feet: step.clearance,
                }),
                movement_edges: edges,
                frame_dt: step.frame_dt,
                view_feel_scale: scale,
            },
            ViewFeelTracking {
                state: &mut self.state,
                followed_pawn: &mut self.followed,
                descriptor: &mut self.descriptor,
            },
        )
    }
}

/// Everything a frame's eye hands render, the listener and the viewmodel.
fn presented(eye: &FrameEye) -> Vec<f32> {
    let mut values = eye.camera.view_projection.to_cols_array().to_vec();
    values.extend(eye.camera.eye_position.to_array());
    values.extend(eye.eye_offset.to_array());
    values.extend([eye.roll, eye.pitch_offset, eye.yaw_offset]);
    values
}

fn assert_same_frames(old: &[FrameEye], new: &[FrameEye], label: &str) {
    assert_eq!(old.len(), new.len());
    for (index, (a, b)) in old.iter().zip(new).enumerate() {
        for (x, y) in presented(a).iter().zip(presented(b)) {
            assert!(
                (x - y).abs() <= 1.0e-5,
                "{label}, frame {index}: {x} vs {y}"
            );
        }
    }
}

fn horizontal_fov_deg(camera: &RenderCamera) -> f32 {
    let projection = camera.view_projection * camera.view_matrix.inverse();
    2.0 * (1.0 / projection.x_axis.x).atan().to_degrees()
}

// --- Composition --------------------------------------------------------------

// Pin P2: two rows with eye drop at once.
#[test]
fn every_rows_eye_drop_sums_under_one_floor_above_the_feet() {
    let params = pose(
        Some(PoseRow {
            channels: channels(0.0, 0.0, 0.6, 0.0),
            enter_rate: 18.0,
            exit_rate: 6.0,
        }),
        Some(VaultPoseRow {
            channels: channels(0.0, 0.0, 0.6, 0.0),
            ease_in: 0.15,
            ease_out: 0.2,
            ledge_focus: None,
        }),
    );
    let clearance = 1.0;
    let mut rig = Rig::new();
    for _ in 0..60 {
        rig.frame(
            &params,
            Step {
                clearance,
                ..step(MovementStateKind::Slide, DT)
            },
            1.0,
        );
    }
    let mut floor_bound = false;
    for index in 0..20 {
        let eye = rig.frame(
            &params,
            Step {
                clearance,
                ..vault_step(index as f32 * DT / PATH_S, None, DT)
            },
            1.0,
        );
        let feet = 2.0 - clearance;
        assert!(
            eye.camera.eye_position.y >= feet + view_feel::pose::MIN_EYE_CLEARANCE - 1.0e-5,
            "frame {index}: {}",
            eye.camera.eye_position.y
        );
        // Each row alone (0.6 m) stays clear of the 0.95 m available; only
        // their sum reaches the floor.
        floor_bound |= (eye.camera.eye_position.y - (feet + view_feel::pose::MIN_EYE_CLEARANCE))
            .abs()
            < 1.0e-4;
    }
    assert!(floor_bound, "the floor bound the summed drop");
}

#[test]
fn row_angles_turn_the_camera_only_and_eye_drop_moves_eye_listener_and_viewmodel() {
    let params = ViewFeelParams {
        sway: None,
        ..pose(
            Some(PoseRow {
                channels: channels(-4.0, 3.0, 0.2, 6.0),
                enter_rate: 18.0,
                exit_rate: 12.0,
            }),
            None,
        )
    };
    let mut rig = Rig::new();
    let mut eye = rig.frame(&params, step(MovementStateKind::Slide, 1.0), 1.0);
    for _ in 0..10 {
        eye = rig.frame(&params, step(MovementStateKind::Slide, 1.0), 1.0);
    }
    assert_eq!(
        (eye.pitch_offset, eye.roll, eye.yaw_offset),
        (0.0, 0.0, 0.0),
        "the viewmodel re-applies no row pitch or roll"
    );
    assert!(
        (eye.eye_offset.y + 0.2).abs() < 1.0e-5,
        "the viewmodel follows the drop"
    );
    assert_eq!(
        listener_for(&eye.camera, None).position,
        eye.camera.eye_position.to_array(),
        "the listener sits at the dropped eye"
    );
    let expected = RenderCamera::new(
        Vec3::new(1.0, 2.0, 3.0),
        16.0 / 9.0,
        YAW,
        -0.2 + (-4.0_f32).to_radians(),
        3.0_f32.to_radians(),
        eye.eye_offset,
        6.0,
    );
    for (a, b) in eye
        .camera
        .view_projection
        .to_cols_array()
        .iter()
        .zip(expected.view_projection.to_cols_array())
    {
        assert!((a - b).abs() < 1.0e-5);
    }
}

#[test]
fn a_full_pose_on_every_row_and_a_kick_stay_inside_the_pitch_limit_and_fov_band() {
    let kick = ImpulseChannels {
        fov: 90.0,
        pitch: -90.0,
        roll: 0.0,
    };
    let params = ViewFeelParams {
        impulse: Some(ImpulseParams {
            tension: 1.0,
            max: ImpulseChannels {
                fov: 180.0,
                pitch: 180.0,
                roll: 180.0,
            },
            states: ImpulseStates {
                normal: None,
                dash: None,
                crouch: None,
                slide: None,
                vault: Some(ImpulseStateParams {
                    tension: None,
                    enter: Some(kick),
                    exit: None,
                }),
            },
        }),
        ..pose(
            Some(PoseRow {
                channels: channels(-90.0, 0.0, 0.0, 90.0),
                enter_rate: 240.0,
                exit_rate: 0.1,
            }),
            Some(VaultPoseRow {
                channels: channels(-90.0, 0.0, 0.0, 90.0),
                ease_in: 0.0,
                ease_out: 0.0,
                ledge_focus: Some(LedgeFocus {
                    strength: 1.0,
                    max_pitch: 89.0,
                }),
            }),
        )
    };
    let limit = 89.0_f32.to_radians();
    for (aim, fov_sign) in [(-limit, 1.0), (limit, -1.0)] {
        let params = if fov_sign < 0.0 {
            let mut narrow = params.clone();
            let pose = narrow.pose.as_mut().unwrap();
            pose.slide.as_mut().unwrap().channels.fov = -90.0;
            pose.vault.as_mut().unwrap().channels.fov = -90.0;
            narrow.impulse = None;
            narrow
        } else {
            params.clone()
        };
        let mut rig = Rig::new();
        for _ in 0..30 {
            rig.frame(
                &params,
                Step {
                    pitch: aim,
                    ..step(MovementStateKind::Slide, DT)
                },
                1.0,
            );
        }
        // The vault's entry edge kicks on the same frame.
        let entry = [view_feel::TimedMovementEdge {
            edge: crate::movement::MovementStateEdge {
                from: MovementStateKind::Slide,
                to: MovementStateKind::Vault,
            },
            age: 0.0,
        }];
        let eye = rig.frame_with_edges(
            &params,
            Step {
                pitch: aim,
                ..vault_step(0.5, Some(Vec3::new(1.5, -5.0, 3.0)), DT)
            },
            1.0,
            &entry,
        );
        let pitch = eye.camera.forward.y.asin();
        assert!(pitch.abs() <= limit + 1.0e-4, "{}", pitch.to_degrees());
        let fov = horizontal_fov_deg(&eye.camera);
        assert!(
            (crate::camera::MIN_FOV_DEG - 1.0e-2..=crate::camera::MAX_FOV_DEG + 1.0e-2)
                .contains(&fov),
            "{fov}"
        );
    }
}

// Focus never turns the view sideways.
#[test]
fn ledge_focus_never_turns_the_rendered_yaw() {
    let with_focus = |strength: f32, bearing: f32| {
        let params = pose(
            None,
            Some(VaultPoseRow {
                channels: channels(0.0, 0.0, 0.0, 0.0),
                ease_in: 0.15,
                ease_out: 0.2,
                ledge_focus: Some(LedgeFocus {
                    strength,
                    max_pitch: 60.0,
                }),
            }),
        );
        let landing = Vec3::new(1.0, 2.0, 3.0)
            + glam::Quat::from_rotation_y(bearing) * Vec3::new(0.6, -0.5, 0.0);
        let mut rig = Rig::new();
        let mut eye = None;
        for index in 0..=12 {
            eye = Some(rig.frame(
                &params,
                vault_step(index as f32 * DT / PATH_S, Some(landing), DT),
                1.0,
            ));
        }
        eye.unwrap().camera.forward
    };
    let level_yaw = |forward: Vec3| forward.x.atan2(forward.z);
    for bearing in [0.0_f32, 1.2, 2.9, -1.7] {
        let off = with_focus(0.0, bearing);
        let on = with_focus(1.0, bearing);
        assert!((level_yaw(on) - level_yaw(off)).abs() < 1.0e-5, "{bearing}");
        assert!(on.y < off.y - 0.1, "{bearing}: focus pitched the view down");
        let ahead = with_focus(1.0, 0.0);
        assert!(
            (on.y - ahead.y).abs() < 1.0e-5,
            "{bearing}: same pitch at any bearing"
        );
    }
}

// Pin P9: a descriptor refresh mid-pose snaps it to neutral.
#[test]
fn a_descriptor_refresh_snaps_every_row_to_neutral_on_the_first_frame() {
    let params = pose(
        Some(PoseRow {
            channels: channels(-3.0, 0.0, 0.2, 5.0),
            enter_rate: 18.0,
            exit_rate: 12.0,
        }),
        Some(VaultPoseRow {
            channels: channels(-6.0, 2.0, 0.0, -3.0),
            ease_in: 0.15,
            ease_out: 0.2,
            ledge_focus: Some(LedgeFocus {
                strength: 0.7,
                max_pitch: 30.0,
            }),
        }),
    );
    let mut refreshed = params.clone();
    refreshed
        .pose
        .as_mut()
        .unwrap()
        .vault
        .as_mut()
        .unwrap()
        .ease_out = 0.25;
    for (label, steps) in [
        ("mid-slide", vec![step(MovementStateKind::Slide, DT); 60]),
        (
            "mid-vault",
            (0..15)
                .map(|i| vault_step(i as f32 * DT / PATH_S, Some(Vec3::new(1.6, 1.4, 3.0)), DT))
                .collect(),
        ),
        (
            "mid-release",
            (0..15)
                .map(|i| vault_step(i as f32 * DT / PATH_S, Some(Vec3::new(1.6, 1.4, 3.0)), DT))
                .chain([step(MovementStateKind::Normal, DT)])
                .collect(),
        ),
    ] {
        let mut rig = Rig::new();
        let mut held = None;
        for s in &steps {
            held = Some(rig.frame(&params, *s, 1.0));
        }
        let held = held.unwrap();
        assert!(
            held.eye_offset.y < -0.01 || horizontal_fov_deg(&held.camera) < 99.0,
            "{label}"
        );
        // Hot reload drops the movement state to `Normal` in the same commit.
        let first = rig.frame(&refreshed, step(MovementStateKind::Normal, DT), 1.0);
        let neutral = Rig::new().frame(&no_view_feel(), step(MovementStateKind::Normal, DT), 1.0);
        assert_same_frames(&[neutral], &[first], label);
    }
}

// --- The frame's driver ---------------------------------------------------------

mod driver {
    use super::*;
    use glam::{Quat, Vec2};
    use postretro_entities::Transform;
    use postretro_entities::components::kinematic_mover::{
        KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
    };
    use postretro_foundation::{MovementState, VaultEntry, VaultParams, VaultTrigger};
    use postretro_physics::collision::CollisionWorld;
    use postretro_physics::movement::{MovementInput, tick_with_immersion_and_armed};

    fn focused_view_feel() -> ViewFeelParams {
        pose(
            Some(PoseRow {
                channels: channels(-3.0, 1.0, 0.1, 5.0),
                enter_rate: 18.0,
                exit_rate: 12.0,
            }),
            Some(VaultPoseRow {
                channels: channels(0.0, 4.0, 0.0, -10.0),
                ease_in: 0.15,
                ease_out: 0.2,
                ledge_focus: Some(LedgeFocus {
                    strength: 0.7,
                    max_pitch: 60.0,
                }),
            }),
        )
    }

    fn vaulting_descriptor() -> postretro_foundation::PlayerMovementDescriptor {
        let mut descriptor = crate::tests::minimal_player_descriptor();
        descriptor.vault = Some(VaultParams {
            max_height: 1.6,
            swim_max_height: 0.9,
            duration_ms: 420.0,
            from: VaultEntry {
                air: Some(VaultTrigger::Auto),
                ground: Some(VaultTrigger::Auto),
                swim: None,
            },
            allow_fire: true,
        });
        descriptor.view_feel = Some(focused_view_feel());
        descriptor
    }

    fn pawn(
        registry: &mut EntityRegistry,
        position: Vec3,
        state: MovementState,
    ) -> (EntityId, PlayerMovementComponent) {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        let mut component = PlayerMovementComponent::from_descriptor(&vaulting_descriptor());
        component.movement_state = state;
        registry.set_component(id, component.clone()).unwrap();
        (id, component)
    }

    fn vault_state(elapsed_ms: f32, target: Vec3, mover: Option<u32>) -> MovementState {
        MovementState::Vault {
            elapsed_ms,
            target,
            start_offset: Vec3::new(-0.6, -0.8, 0.0),
            mover,
            entry_speed: 0.0,
        }
    }

    fn assemble(driver: Option<ViewFeelDriver>, eye: Vec3, pitch: f32, rig: &mut Rig) -> FrameEye {
        assemble_frame_eye(
            FrameEyeInputs {
                presented_eye: eye,
                aspect: 16.0 / 9.0,
                render_yaw: YAW,
                pitch,
                driver,
                movement_edges: &[],
                frame_dt: DT,
                view_feel_scale: 1.0,
            },
            ViewFeelTracking {
                state: &mut rig.state,
                followed_pawn: &mut rig.followed,
                descriptor: &mut rig.descriptor,
            },
        )
    }

    #[test]
    fn a_remote_pawn_vaulting_or_sliding_adds_no_pose_to_the_local_camera() {
        for remote_state in [
            vault_state(200.0, Vec3::new(0.6, 1.0, 0.0), None),
            MovementState::Sliding {
                elapsed_ms: 100.0,
                boost: Vec3::ZERO,
                eye_current: 0.0,
            },
        ] {
            let mut registry = EntityRegistry::new();
            let (local, _) = pawn(&mut registry, Vec3::ZERO, MovementState::Normal);
            registry.mark_local_player_pawn(local).unwrap();
            pawn(&mut registry, Vec3::new(2.0, 0.0, 0.0), remote_state);

            let mut rig = Rig::new();
            let mut eye = None;
            for _ in 0..30 {
                let driver = view_feel_driver(&registry, 1.3, 1.3, DT, 1.0);
                let driver_ref = driver.as_ref().unwrap();
                assert_eq!(driver_ref.pawn, local);
                assert_eq!(driver_ref.movement_state, MovementStateKind::Normal);
                assert!(driver_ref.vault.is_none());
                eye = Some(assemble(driver, Vec3::new(0.0, 1.3, 0.0), 0.0, &mut rig));
            }
            let eye = eye.unwrap();
            assert_eq!(eye.eye_offset, Vec3::ZERO);
            assert!(
                (eye.camera.forward - Vec3::new(-YAW.sin(), 0.0, -YAW.cos())).length() < 1.0e-5
            );
        }
    }

    /// Floor at y = 0 for x < 5, a riser facing -X at x = 5, and a top at
    /// `top_y`.
    fn ledge_world(top_y: f32) -> CollisionWorld {
        let points = vec![
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(5.0, 0.0, -50.0),
            Vec3::new(5.0, 0.0, 50.0),
            Vec3::new(-50.0, 0.0, 50.0),
            Vec3::new(5.0, 0.0, -50.0),
            Vec3::new(5.0, top_y, -50.0),
            Vec3::new(5.0, top_y, 50.0),
            Vec3::new(5.0, 0.0, 50.0),
            Vec3::new(5.0, top_y, -50.0),
            Vec3::new(60.0, top_y, -50.0),
            Vec3::new(60.0, top_y, 50.0),
            Vec3::new(5.0, top_y, 50.0),
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 6, 5],
            [4, 7, 6],
            [8, 10, 9],
            [8, 11, 10],
        ];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    /// Walk a pawn into a ledge of `top_y` until it vaults, then return the
    /// registry holding it a few ticks into the climb, and its centre.
    fn mid_vault(top_y: f32) -> (EntityRegistry, Vec3) {
        let world = ledge_world(top_y);
        let descriptor = vaulting_descriptor();
        let mut component = PlayerMovementComponent::from_descriptor(&descriptor);
        let reach = descriptor.capsule.half_height + descriptor.capsule.radius;
        let mut position = Vec3::new(5.0 - descriptor.capsule.radius - 0.05, reach, 0.0);
        let input = MovementInput {
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        };
        let mut vault_ticks = 0;
        for _ in 0..120 {
            position = tick_with_immersion_and_armed(
                &mut component,
                &input,
                &world,
                postretro_collision::EnvironmentView::level_only(-20.32),
                DT,
                position,
                |_| false,
            )
            .0;
            if component.movement_state.kind() == MovementStateKind::Vault {
                vault_ticks += 1;
                if vault_ticks == 8 {
                    break;
                }
            }
        }
        assert_eq!(vault_ticks, 8, "the pawn vaults the {top_y} m ledge");
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry.set_component(id, component).unwrap();
        registry.mark_local_player_pawn(id).unwrap();
        (registry, position)
    }

    #[test]
    fn a_frame_from_a_vaulting_pawns_live_state_focuses_on_its_own_landing_spot() {
        let mut focus_by_height = Vec::new();
        for top_y in [0.6, 1.0] {
            let (mut registry, center) = mid_vault(top_y);
            let eye_y = center.y + 0.5;
            let driver = view_feel_driver(&registry, eye_y, eye_y, DT, 1.0).unwrap();
            let landing = driver.vault.unwrap().landing.unwrap();
            assert!((landing.y - top_y).abs() < 0.02, "{landing}");
            let mut rig = Rig::new();
            let with = assemble(
                Some(driver),
                Vec3::new(center.x, eye_y, center.z),
                0.0,
                &mut rig,
            );
            let id = registry.local_player_movement_pawn().unwrap();
            let mut component = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            component
                .view_feel
                .as_mut()
                .unwrap()
                .pose
                .as_mut()
                .unwrap()
                .vault
                .as_mut()
                .unwrap()
                .ledge_focus = None;
            registry.set_component(id, component).unwrap();
            let driver = view_feel_driver(&registry, eye_y, eye_y, DT, 1.0).unwrap();
            let mut rig = Rig::new();
            let without = assemble(
                Some(driver),
                Vec3::new(center.x, eye_y, center.z),
                0.0,
                &mut rig,
            );
            let focus =
                (with.camera.forward.y.asin() - without.camera.forward.y.asin()).to_degrees();
            assert!(focus < -0.5, "{top_y} m: {focus}");
            focus_by_height.push(focus);
        }
        assert!(
            (focus_by_height[0] - focus_by_height[1]).abs() > 0.5,
            "two ledge heights at one aim focus differently: {focus_by_height:?}"
        );
    }

    fn mover(registry: &mut EntityRegistry, from: Transform, to: Transform) -> EntityId {
        let entity = registry.spawn(from);
        let component = KinematicMoverComponent::new(
            7,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["a".to_string(), "b".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        registry.set_component(entity, component).unwrap();
        registry.set_component(entity, to).unwrap();
        entity
    }

    // Pins P8, P12, P19: a vault onto a moving, tilting mover.
    #[test]
    fn a_mover_vault_focuses_like_a_static_one_and_follows_the_mover_at_the_frames_share() {
        let local_target = Vec3::new(0.6, 1.0, 0.0);
        let from = Transform {
            position: Vec3::new(3.0, 0.5, -2.0),
            rotation: Quat::from_rotation_y(0.4),
            scale: Vec3::ONE,
        };
        let to = Transform {
            position: Vec3::new(3.5, 0.9, -2.0),
            rotation: Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.2),
            scale: Vec3::ONE,
        };
        let mut registry = EntityRegistry::new();
        mover(&mut registry, from, to);
        let (local, _) = pawn(
            &mut registry,
            Vec3::ZERO,
            vault_state(150.0, local_target, Some(7)),
        );
        registry.mark_local_player_pawn(local).unwrap();
        let eye = Vec3::new(3.2, 2.8, -2.1);

        let mut moving = Rig::new();
        let mut fixed = Rig::new();
        for (index, alpha) in [0.0_f32, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            let driver = view_feel_driver(&registry, eye.y, eye.y, DT, alpha).unwrap();
            let landing = driver.vault.unwrap().landing.unwrap();
            let presented = registry
                .interpolated_transform(
                    crate::runtime_movers::mover_entity(&registry, 7).unwrap(),
                    alpha,
                )
                .unwrap();
            let expected = presented.position + presented.rotation.normalize() * local_target;
            assert!(
                (landing - expected).length() < 1.0e-5,
                "alpha {alpha}: {landing}"
            );
            if index > 0 {
                let previous = view_feel_driver(&registry, eye.y, eye.y, DT, alpha - 0.25)
                    .unwrap()
                    .vault
                    .unwrap()
                    .landing
                    .unwrap();
                assert!((landing - previous).length() > 1.0e-3, "follows the mover");
            }
            // The same relative geometry with no mover: the same focus.
            let mut static_driver = view_feel_driver(&registry, eye.y, eye.y, DT, alpha).unwrap();
            static_driver.vault.as_mut().unwrap().landing = Some(expected);
            let a = assemble(Some(driver), eye, -0.1, &mut moving);
            let b = assemble(Some(static_driver), eye, -0.1, &mut fixed);
            assert!((a.camera.forward - b.camera.forward).length() < 1.0e-6);
        }
        // A mover with no presented pose leaves no focus, not a stale one.
        let entity = crate::runtime_movers::mover_entity(&registry, 7).unwrap();
        registry.despawn(entity).unwrap();
        let driver = view_feel_driver(&registry, eye.y, eye.y, DT, 0.5).unwrap();
        assert!(driver.vault.unwrap().landing.is_none());
    }
}

// Pins P14, P18: a descriptor refresh while the vault runs on. The clear runs
// first in the frame's assembly, then the vault joins on that same frame.
#[test]
fn a_refresh_mid_vault_rejoins_from_neutral_against_that_frames_aim() {
    let row = VaultPoseRow {
        channels: channels(0.0, 4.0, 0.0, 0.0),
        ease_in: 0.15,
        ease_out: 0.2,
        ledge_focus: Some(LedgeFocus {
            strength: 1.0,
            max_pitch: 80.0,
        }),
    };
    let params = pose(None, Some(row));
    let mut refreshed = params.clone();
    refreshed
        .pose
        .as_mut()
        .unwrap()
        .vault
        .as_mut()
        .unwrap()
        .channels
        .roll = 5.0;
    let landing = Vec3::new(1.6, 1.4, 3.0);
    let eye = Vec3::new(1.0, 2.0, 3.0);
    let line = (landing.y - eye.y)
        .atan2((landing - eye).with_y(0.0).length().max(0.5))
        .to_degrees();
    let mut rig = Rig::new();
    for index in 0..=12 {
        rig.frame(
            &params,
            Step {
                pitch: 0.0,
                ..vault_step(index as f32 * DT / PATH_S, Some(landing), DT)
            },
            1.0,
        );
    }
    let aim = 10.0_f32;
    let joined = rig.frame(
        &refreshed,
        Step {
            pitch: aim.to_radians(),
            ..vault_step(13.0 * DT / PATH_S, Some(landing), DT)
        },
        1.0,
    );
    let rendered = |frame: &FrameEye| frame.camera.forward.y.asin().to_degrees();
    assert!(
        (rendered(&joined) - aim).abs() < 0.3 * (line - aim).abs(),
        "the join frame is near neutral: {}",
        rendered(&joined)
    );
    let mut later = joined;
    for index in 14..=18 {
        later = rig.frame(
            &refreshed,
            Step {
                pitch: 0.0,
                ..vault_step(index as f32 * DT / PATH_S, Some(landing), DT)
            },
            1.0,
        );
    }
    assert!(
        (rendered(&later) - (line - aim)).abs() < 1.0e-3,
        "focus against the join frame's aim: {} vs {}",
        rendered(&later),
        line - aim
    );
}
