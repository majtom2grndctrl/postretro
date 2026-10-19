// The vault scripting-surface example run as mod manifests through the
// production script runtime: the `content/dev/scripts/movement-vault.{ts,luau}`
// twins must declare the same vault block, content/dev's own player must
// declare it too, and each declaration must vault a ledge in a test world.
// content/dev's manifest is TS-only, so the Luau half lives in the twin.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use postretro_collision::EnvironmentView;
use postretro_foundation::data_descriptors::{
    PlayerMovementDescriptor, VaultEntry, VaultParams, VaultTrigger,
};
use postretro_foundation::{MovementStateKind, PlayerMovementComponent};
use postretro_physics::collision::CollisionWorld;
use postretro_physics::movement::{MovementInput, tick_with_immersion_and_armed};

use super::fluids_swim_surface_tests::{dev_content, dev_manifest, fluids_named_by, run_manifest};

fn ts_twin() -> String {
    postretro_script_compiler::bundle_entry(&dev_content().join("scripts/movement-vault.ts"))
        .expect("movement-vault.ts bundles")
}

fn luau_twin() -> String {
    std::fs::read_to_string(dev_content().join("scripts/movement-vault.luau"))
        .expect("movement-vault.luau reads")
}

fn player_movement(
    manifest: &postretro_scripting_core::runtime::ModManifestResult,
) -> PlayerMovementDescriptor {
    manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some("player"))
        .expect("a player entity")
        .movement
        .clone()
        .expect("movement present")
}

/// Floor at y = 0 for x < 5, a riser facing -X at x = 5, and a top at y = 1.2.
fn ledge_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-50.0, 0.0, -50.0),
        Vec3::new(5.0, 0.0, -50.0),
        Vec3::new(5.0, 0.0, 50.0),
        Vec3::new(-50.0, 0.0, 50.0),
        Vec3::new(5.0, 0.0, -50.0),
        Vec3::new(5.0, 1.2, -50.0),
        Vec3::new(5.0, 1.2, 50.0),
        Vec3::new(5.0, 0.0, 50.0),
        Vec3::new(5.0, 1.2, -50.0),
        Vec3::new(60.0, 1.2, -50.0),
        Vec3::new(60.0, 1.2, 50.0),
        Vec3::new(5.0, 1.2, 50.0),
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

/// Whether a pawn built from `movement`, airborne at the riser and pushing
/// into it, vaults onto the top.
fn vaults_in_a_test_world(movement: &PlayerMovementDescriptor) -> bool {
    let world = ledge_world();
    let mut component = PlayerMovementComponent::from_descriptor(movement);
    component.coyote_timer_ms = 1000.0;
    let radius = movement.capsule.radius;
    let reach = movement.capsule.half_height + radius;
    let mut position = Vec3::new(5.0 - radius - 0.05, 0.5 + reach, 0.0);
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
    let mut vaulted = false;
    for _ in 0..60 {
        position = tick_with_immersion_and_armed(
            &mut component,
            &input,
            &world,
            EnvironmentView::level_only(-20.32),
            1.0 / 60.0,
            position,
            |_| false,
        )
        .0;
        vaulted |= component.movement_state.kind() == MovementStateKind::Vault;
    }
    vaulted && position.x > 5.0 && position.y > 1.2
}

// The vault example runs as a content/dev fixture in TypeScript and Luau,
// both declare the same block, and content/dev's own player declares it too.
#[test]
fn movement_vault_twins_and_the_dev_player_declare_one_vault_block_that_vaults() {
    let ts = run_manifest("start-script.js", &ts_twin()).expect("TS twin initializes");
    let luau = run_manifest("start-script.luau", &luau_twin()).expect("Luau twin initializes");
    let dev = dev_manifest();

    let expected = VaultParams {
        max_height: 1.6,
        swim_max_height: 0.9,
        duration_ms: 800.0,
        from: VaultEntry {
            air: Some(VaultTrigger::Auto),
            ground: None,
            swim: Some(VaultTrigger::Auto),
        },
        allow_fire: false,
    };
    for (label, manifest) in [
        ("TS twin", &ts),
        ("Luau twin", &luau),
        ("content/dev", &dev),
    ] {
        let movement = player_movement(manifest);
        assert_eq!(movement.vault, Some(expected.clone()), "{label}");
        assert!(vaults_in_a_test_world(&movement), "{label}'s player vaults");
    }
}

// The vault dev stations' pool loads wet: the host resolves the map's fluid by
// name against content/dev's manifest, and an undeclared name loads dry, which
// would leave the pool station nothing to swim in.
#[test]
fn vault_test_map_names_only_fluids_the_dev_manifest_declares() {
    let dev = dev_manifest();
    let named = fluids_named_by("vault-test.map");
    assert_eq!(
        named.iter().map(String::as_str).collect::<Vec<_>>(),
        ["water"]
    );
    assert!(dev.fluids.index_of("water").is_some());
}

// content/dev's player authors `viewFeel.pose`, and holds a camera pose
// through the climb instead of the slow vault impulse kick it used before.
#[test]
fn the_dev_player_authors_a_pose_and_no_vault_kick() {
    use postretro_foundation::{LedgeFocus, PoseChannels, PoseParams, PoseRow, VaultPoseRow};
    let view_feel = player_movement(&dev_manifest())
        .view_feel
        .expect("the dev player has view feel");
    assert_eq!(
        view_feel.pose,
        Some(PoseParams {
            slide: Some(PoseRow {
                channels: PoseChannels {
                    pitch: 0.0,
                    roll: 0.0,
                    eye_drop: 0.15,
                    fov: 5.0,
                },
                enter_rate: 18.0,
                exit_rate: 12.0,
            }),
            vault: Some(VaultPoseRow {
                channels: PoseChannels {
                    pitch: 0.0,
                    roll: 10.0,
                    eye_drop: 0.0,
                    fov: -30.0,
                },
                ease_in: 0.33,
                ease_out: 0.33,
                ledge_focus: Some(LedgeFocus {
                    strength: 0.7,
                    max_pitch: 30.0,
                }),
            }),
        })
    );
    assert!(
        view_feel
            .impulse
            .expect("the dev player has impulses")
            .states
            .vault
            .is_none(),
        "no vault impulse row"
    );
}

/// Floor at y = 0 for x < 5, a riser facing -X at x = 5, and a top at `top_y`.
fn graded_ledge_world(top_y: f32) -> CollisionWorld {
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

/// The deepest ledge focus, degrees, the dev player's `pose.vault` presents
/// over a jump-vault onto a `top_y` ledge, aiming level at entry: the rendered
/// pitch with focus less the same frames without it.
fn peak_dev_focus(movement: &PlayerMovementDescriptor, top_y: f32) -> f32 {
    use crate::frame_eye::{
        FrameEyeInputs, ViewFeelTracking, assemble_frame_eye, view_feel_driver,
    };
    use postretro_entities::{EntityRegistry, Transform};

    const DT: f32 = 1.0 / 60.0;
    let world = graded_ledge_world(top_y);
    let mut unfocused_movement = movement.clone();
    if let Some(row) = unfocused_movement
        .view_feel
        .as_mut()
        .and_then(|view| view.pose.as_mut())
        .and_then(|pose| pose.vault.as_mut())
    {
        row.ledge_focus = None;
    }
    let mut component = PlayerMovementComponent::from_descriptor(movement);
    let reach = movement.capsule.half_height + movement.capsule.radius;
    let mut position = Vec3::new(5.0 - movement.capsule.radius - 0.3, reach, 0.0);
    let mut tracked: [(crate::view_feel::ViewFeelState, Option<_>, Option<_>); 2] =
        Default::default();
    let mut deepest = 0.0_f32;
    let mut vaulted = false;
    for tick in 0..240 {
        let input = MovementInput {
            // `x` is right; at yaw 0 right is world +X, toward the riser.
            wish_dir: Vec2::new(1.0, 0.0),
            jump_pressed: tick < 3,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        };
        position = tick_with_immersion_and_armed(
            &mut component,
            &input,
            &world,
            EnvironmentView::level_only(-20.32),
            DT,
            position,
            |_| false,
        )
        .0;
        vaulted |= component.movement_state.kind() == MovementStateKind::Vault;
        let eye = position + Vec3::Y * movement.capsule.eye_height;
        let mut pitches = [0.0_f32; 2];
        for (index, view_movement) in [movement, &unfocused_movement].into_iter().enumerate() {
            let mut presented = component.clone();
            presented.view_feel = view_movement.view_feel.clone();
            let mut registry = EntityRegistry::new();
            let pawn = registry.spawn(Transform {
                position,
                ..Transform::default()
            });
            registry.set_component(pawn, presented).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            let (state, followed, descriptor) = &mut tracked[index];
            let frame = assemble_frame_eye(
                FrameEyeInputs {
                    presented_eye: eye,
                    aspect: 16.0 / 9.0,
                    render_yaw: 0.0,
                    pitch: 0.0,
                    driver: view_feel_driver(&registry, eye.y, eye.y, DT, 1.0),
                    movement_edges: &[],
                    frame_dt: DT,
                    view_feel_scale: 1.0,
                },
                ViewFeelTracking {
                    state,
                    followed_pawn: followed,
                    descriptor,
                },
            );
            pitches[index] = frame.camera.forward.y.asin().to_degrees();
        }
        deepest = deepest.min(pitches[0] - pitches[1]);
    }
    assert!(vaulted, "the dev player vaults the {top_y} m ledge");
    deepest
}

// The dev player's ledge focus reads the ledge: the vault-test graded ledges
// settle the view differently.
#[test]
fn the_dev_players_ledge_focus_differs_across_the_graded_ledges() {
    let movement = player_movement(&dev_manifest());
    assert!(
        movement
            .view_feel
            .as_ref()
            .and_then(|view| view.pose.as_ref())
            .and_then(|pose| pose.vault.as_ref())
            .and_then(|row| row.ledge_focus)
            .is_some(),
        "the dev player authors ledge focus"
    );
    let peaks: Vec<f32> = [0.6, 1.0, 1.5, 2.2]
        .into_iter()
        .map(|top_y| peak_dev_focus(&movement, top_y))
        .collect();
    let mut distinct: Vec<f32> = Vec::new();
    for peak in &peaks {
        assert!(*peak < -0.5, "every ledge focuses: {peaks:?}");
        if distinct.iter().all(|seen| (seen - peak).abs() > 0.5) {
            distinct.push(*peak);
        }
    }
    assert!(distinct.len() >= 3, "{peaks:?}");
}
