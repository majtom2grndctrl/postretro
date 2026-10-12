// Environment volumes through the real level install: built before the parity
// digest and the first tick, replaced whole on the next level, and resolved
// against the script gravity register the way every tick call site builds it.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use std::path::PathBuf;

use glam::Vec3;
use postretro_collision::{BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment};
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::environment_volumes::{
    EnvironmentBrush, EnvironmentPlane, EnvironmentVolumeRecord, EnvironmentVolumesSection,
};
use postretro_level_loader::LevelWorld;

use super::tests::{level_world, script_ctx, test_app};
use crate::App;

/// Volume gravity inside each fixture's gravity volume.
const VOLUME_GRAVITY: f32 = -2.0;
const LEVEL_GRAVITY: f32 = -9.81;

fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = [0.0; 3];
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal,
            distance: max[axis],
        });
        normal[axis] = -1.0;
        planes.push(EnvironmentPlane {
            normal,
            distance: -min[axis],
        });
    }
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// A level whose header gravity is `level_gravity`, with one gravity volume
/// spanning `min..min + 4` on every axis.
fn level_with_gravity_volume(name: &str, level_gravity: f32, min: Vec3) -> LevelWorld {
    let mut world = level_world(name, 1);
    world.environment_volumes = EnvironmentVolumesSection {
        level_gravity,
        volumes: vec![EnvironmentVolumeRecord {
            source_entity: 1,
            priority: 0,
            gravity: Some(VOLUME_GRAVITY),
            push: None,
            fluid: String::new(),
            brushes: vec![box_brush(min, min + Vec3::splat(4.0))],
            surface_faces: Vec::new(),
        }],
    };
    world
}

/// Resolve at `position` exactly as a tick call site does: the view is built
/// from the script gravity register and the installed volumes.
fn resolve_at(app: &App, position: Vec3) -> ResolvedEnvironment {
    let view = EnvironmentView::new(script_ctx(app).gravity.get(), &app.environment_volumes);
    resolve_environment(
        view,
        &BodyProbe {
            position,
            half_height: 0.5,
            radius: 0.25,
            eye_height: 0.6,
        },
    )
}

/// Run a Luau data script through the session's script runtime, the route the
/// level install's data-script stage takes.
fn run_luau_data_script(app: &App, source: &str) {
    let section = DataScriptSection {
        compiled_bytes: source.as_bytes().to_vec(),
        source_path: "environment-gravity-fixture.luau".to_string(),
    };
    let content_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let _ = app
        .session
        .as_ref()
        .expect("test app session installed")
        .scripting
        .script_runtime
        .run_data_script(&section, &content_root);
}

#[test]
fn level_install_builds_environment_volumes_before_parity_digest_and_any_tick() {
    let source = include_str!("lifecycle.rs")
        .split("#[cfg(test)]\npub(crate) mod tests")
        .next()
        .unwrap();
    let install = source
        .split("fn install_level_payload(")
        .nth(1)
        .unwrap()
        .split("\n    fn ")
        .next()
        .unwrap();
    let collision = install
        .find("crate::collision::CollisionWorld::from_level(")
        .expect("install builds static collision");
    let volumes = install
        .find("self.environment_volumes =")
        .expect("install builds the environment volumes");
    let digest = install
        .find("level_content_digest(")
        .expect("install computes the parity digest");
    let parity = install
        .find("endpoint.set_level_parity(")
        .expect("install publishes level parity");
    let renderer_less_return = install
        .find("return Ok(());")
        .expect("install has a renderer-less return");
    assert!(
        collision < volumes,
        "a rejected collision mesh fails the load before the volumes change"
    );
    for (later, what) in [
        (digest, "the parity digest"),
        (parity, "publishing level parity"),
        (renderer_less_return, "the renderer-less return"),
    ] {
        assert!(
            volumes < later,
            "the volumes must be installed before {what}"
        );
    }

    // Install itself never ticks: once it returns, the first tick's view
    // already resolves this level's volume.
    let mut app = test_app();
    assert!(app.environment_volumes.is_empty());
    app.install_level_payload(
        level_with_gravity_volume("a", LEVEL_GRAVITY, Vec3::ZERO),
        PathBuf::from("baked"),
    )
    .expect("fixture installs");
    assert!(
        app.renderer.is_none(),
        "the renderer-less route installs too"
    );
    assert_eq!(app.environment_volumes.len(), 1);
    assert_eq!(resolve_at(&app, Vec3::splat(2.0)).gravity, VOLUME_GRAVITY);
}

#[test]
fn installing_a_second_level_leaves_none_of_the_first_levels_volumes_resolvable() {
    let mut app = test_app();
    let a_inside = Vec3::splat(2.0);
    let b_inside = Vec3::new(22.0, 2.0, 2.0);
    app.install_level_payload(
        level_with_gravity_volume("a", LEVEL_GRAVITY, Vec3::ZERO),
        PathBuf::from("baked"),
    )
    .expect("level A installs");
    assert_eq!(resolve_at(&app, a_inside).gravity, VOLUME_GRAVITY);

    app.unload_level();
    assert!(
        app.environment_volumes.is_empty(),
        "no volume resolves between levels"
    );

    app.install_level_payload(
        level_with_gravity_volume("b", -5.0, Vec3::new(20.0, 0.0, 0.0)),
        PathBuf::from("baked"),
    )
    .expect("level B installs");
    assert_eq!(app.environment_volumes.len(), 1);
    assert_eq!(
        resolve_at(&app, a_inside).gravity,
        -5.0,
        "level A's volume is gone: its point resolves level B's default"
    );
    assert_eq!(resolve_at(&app, b_inside).gravity, VOLUME_GRAVITY);
    assert_eq!(
        app.environment_volumes.volume(0).unwrap().brushes[0].aabb_min,
        Vec3::new(20.0, 0.0, 0.0)
    );
}

#[test]
fn level_install_seeds_script_gravity_from_the_environment_header() {
    // A map with only `initialGravity` (here upward) and no volumes behaves as
    // before: the header value seeds the gravity register every body resolves.
    let mut app = test_app();
    let mut world = level_world("initial-gravity-only", 1);
    world.environment_volumes = EnvironmentVolumesSection {
        level_gravity: 4.0,
        volumes: Vec::new(),
    };
    app.install_level_payload(world, PathBuf::from("baked"))
        .expect("fixture installs");

    assert_eq!(script_ctx(&app).gravity.get(), 4.0);
    assert!(app.environment_volumes.is_empty());
    let resolved = resolve_at(&app, Vec3::splat(2.0));
    assert_eq!(resolved.gravity, 4.0);
    assert_eq!(resolved.push, Vec3::ZERO);
}

#[test]
fn set_gravity_changes_gravity_outside_volumes_and_never_inside_a_gravity_volume() {
    let mut app = test_app();
    app.install_level_payload(
        level_with_gravity_volume("set-gravity", LEVEL_GRAVITY, Vec3::ZERO),
        PathBuf::from("baked"),
    )
    .expect("fixture installs");
    let inside = Vec3::splat(2.0);
    let outside = Vec3::new(10.0, 2.0, 2.0);
    assert_eq!(resolve_at(&app, outside).gravity, LEVEL_GRAVITY);

    run_luau_data_script(
        &app,
        "function setupLevel(_ctx)\n  setGravity(-4)\n  return {}\nend\n",
    );

    assert_eq!(script_ctx(&app).gravity.get(), -4.0);
    assert_eq!(resolve_at(&app, outside).gravity, -4.0);
    assert_eq!(
        resolve_at(&app, inside).gravity,
        VOLUME_GRAVITY,
        "volume gravity is absolute; setGravity never reaches inside"
    );
}

#[test]
fn two_set_gravity_calls_in_one_turn_leave_the_later_value_and_the_volume_holds() {
    let mut app = test_app();
    app.install_level_payload(
        level_with_gravity_volume("set-gravity-twice", LEVEL_GRAVITY, Vec3::ZERO),
        PathBuf::from("baked"),
    )
    .expect("fixture installs");
    let inside = Vec3::splat(2.0);
    let outside = Vec3::new(10.0, 2.0, 2.0);

    run_luau_data_script(
        &app,
        "function setupLevel(_ctx)\n  setGravity(-3)\n  setGravity(1.5)\n  return {}\nend\n",
    );

    // The next tick builds its view once from the register: the later write.
    assert_eq!(script_ctx(&app).gravity.get(), 1.5);
    assert_eq!(resolve_at(&app, outside).gravity, 1.5);
    assert_eq!(resolve_at(&app, inside).gravity, VOLUME_GRAVITY);
}
