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
use postretro_physics::movement::{MovementInput, tick};

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
        position = tick(
            &mut component,
            &input,
            &world,
            EnvironmentView::level_only(-20.32),
            1.0 / 60.0,
            position,
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
        duration_ms: 320.0,
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
