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

/// Resolve at `position` exactly as a host tick call site does: the view is
/// built from the script gravity register, the installed volumes and the
/// viscosities of the table they resolved against.
fn resolve_at(app: &App, position: Vec3) -> ResolvedEnvironment {
    let view = EnvironmentView::new(
        script_ctx(app).gravity.get(),
        &app.environment_volumes,
        app.environment_fluids.viscosities(),
    );
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

fn fluid(name: &str) -> postretro_foundation::FluidDescriptor {
    postretro_foundation::FluidDescriptor {
        name: name.to_string(),
        viscosity: 2.0,
        view_tint: [0.1, 0.3, 0.4, 0.35],
        muffle: 0.8,
        surface_opacity: 0.6,
        enter_sound: None,
        exit_sound: None,
    }
}

fn declare_fluids(app: &App, names: &[&str]) {
    script_ctx(app).data_registry.borrow_mut().replace_fluids(
        postretro_foundation::FluidRegistry::from_declarations(
            "test",
            names.iter().map(|name| Ok(fluid(name))),
        ),
    );
}

/// A level with one fluid volume, map entity `source_entity`, naming `fluid`
/// and spanning `0..4` on every axis.
fn level_with_fluid_volume(source_entity: u32, fluid: &str) -> LevelWorld {
    let mut world = level_world("fluid", 1);
    world.environment_volumes = EnvironmentVolumesSection {
        level_gravity: LEVEL_GRAVITY,
        volumes: vec![EnvironmentVolumeRecord {
            source_entity,
            priority: 0,
            gravity: None,
            push: None,
            fluid: fluid.to_string(),
            brushes: vec![box_brush(Vec3::ZERO, Vec3::splat(4.0))],
            surface_faces: Vec::new(),
        }],
    };
    world
}

/// Deep inside the fixture volume: the probe capsule is fully immersed.
const SUBMERGED: Vec3 = Vec3::splat(2.0);

#[test]
fn a_declared_fluid_resolves_to_its_declaration_order_index() {
    let mut app = test_app();
    declare_fluids(&app, &["water", "sludge"]);
    app.install_level_payload(level_with_fluid_volume(3, "sludge"), PathBuf::from("baked"))
        .expect("fixture installs");

    let resolved = resolve_at(&app, SUBMERGED);
    assert!(resolved.immersion > 0.0, "the probe sits inside the volume");
    assert_eq!(resolved.fluid, Some(1));
}

// AC N12: an undeclared fluid name loads dry and warns, naming the entity and the fluid.
#[test]
fn a_fluid_volume_naming_an_undeclared_fluid_loads_dry_and_warns_naming_entity_and_fluid() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "lava"), PathBuf::from("baked"))
        .expect("an undeclared fluid never fails the load");

    let resolved = resolve_at(&app, SUBMERGED);
    assert_eq!(resolved.fluid, None, "the volume loads dry");
    assert_eq!(resolved.immersion, 0.0);
    capture.assert_logged_once(
        log::Level::Warn,
        "[Loader] fluid_volume entity 7 names undeclared fluid 'lava'; it loads dry",
    );
    capture.assert_not_logged(log::Level::Warn, "undeclared fluid 'water'");
}

// AC N9 (host half): the real staged reload route declares the missing fluid.
#[test]
fn a_host_manifest_reload_that_adds_a_fluid_makes_its_volume_no_longer_dry() {
    let mod_root = tempfile::tempdir().expect("temporary mod root");
    std::fs::write(
        mod_root.path().join("start-script.js"),
        "globalThis.__postretroModManifest = { name: 'Fluids', id: 'fluids', version: '1', \
         fluids: [{ name: 'water', viscosity: 2, viewTint: [0.1, 0.3, 0.4, 0.35], muffle: 0.8, surfaceOpacity: 0.6 }, \
                  { name: 'lava', viscosity: 9, viewTint: [1, 0.2, 0, 0.8], muffle: 0.5, surfaceOpacity: 1 }] };",
    )
    .expect("reloaded manifest written");

    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "lava"), PathBuf::from("baked"))
        .expect("fixture installs");
    assert_eq!(
        resolve_at(&app, SUBMERGED).fluid,
        None,
        "dry before the reload"
    );

    let generation = app
        .session
        .as_mut()
        .expect("test app session installed")
        .scripting
        .script_runtime
        .enqueue_staged_manifest_build(mod_root.path())
        .expect("staged build queued")
        .expect("debug builds stage manifest reloads");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while script_runtime_committed(&app) != Some(generation) {
        assert!(
            std::time::Instant::now() < deadline,
            "the staged manifest never committed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_staged_manifest_results();
    }

    assert_eq!(resolve_at(&app, SUBMERGED).fluid, Some(1));
}

fn script_runtime_committed(app: &App) -> Option<u64> {
    app.session
        .as_ref()
        .expect("test app session installed")
        .scripting
        .script_runtime
        .committed_staged_manifest_generation()
}

#[test]
fn a_reload_that_leaves_the_fluid_table_unchanged_keeps_the_installed_volumes() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    let capture = postretro_test_log_capture::LogCapture::start();
    app.refresh_environment_fluids();
    capture.assert_not_logged(log::Level::Info, "[Loader] environment volumes:");
    assert_eq!(resolve_at(&app, SUBMERGED).fluid, Some(0));
}

fn fluid_with(
    name: &str,
    viscosity: f32,
    view_tint: [f32; 4],
) -> postretro_foundation::FluidDescriptor {
    postretro_foundation::FluidDescriptor {
        viscosity,
        view_tint,
        ..fluid(name)
    }
}

/// Make this app a connected client, before any host tuning arrives, mid-load
/// of the host's level.
fn connect_as_client(app: &mut App) {
    // A connected install publishes level parity for the in-flight load's source.
    app.level_load = Some(crate::startup::InFlightLevelLoad {
        map_path: PathBuf::from("content/dev/maps/fluid.prl"),
        content_root: PathBuf::from("content/dev"),
        entry: crate::startup::LevelLoadEntry {
            catalog_id: Some("fluid".to_string()),
            path: "maps/fluid.prl".to_string(),
            name: "Fluid".to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
    });
    app.session
        .as_mut()
        .expect("test app session installed")
        .net_endpoint = Some(
        crate::netcode::NetEndpoint::from_role(
            &crate::netcode::NetRole::Connect {
                addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 1)),
            },
            None,
        )
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint"),
    );
}

/// Deliver a host tuning payload carrying `fluids` through the production
/// client Control drain, as `net_poll_and_apply` does.
fn deliver_host_fluids(app: &mut App, fluids: &[(&str, f32)]) {
    use postretro_combat_model::{
        EnvironmentTuning, FluidMovementTable, FluidMovementTuning, TuningPayload,
    };
    let fluids = FluidMovementTable::new(
        fluids
            .iter()
            .map(|&(name, viscosity)| FluidMovementTuning {
                name: name.to_string(),
                viscosity,
            })
            .collect(),
    )
    .expect("valid host table");
    let payload = TuningPayload::new(
        None,
        std::array::from_fn(|_| None),
        EnvironmentTuning {
            level_gravity: LEVEL_GRAVITY,
            fluids,
        },
    );
    crate::client_drain_control(
        app,
        vec![postretro_net::wire::ServerControlMessage::Tuning(
            serde_json::to_vec(&payload).expect("payload serializes"),
        )],
    );
}

/// Resolve at `position` exactly as client prediction does: the installed
/// volumes under the host's level gravity and fluid viscosities.
fn client_resolve_at(app: &App, position: Vec3) -> Option<ResolvedEnvironment> {
    let host = app
        .session
        .as_ref()?
        .net_endpoint
        .as_ref()?
        .client_host_environment()?;
    Some(resolve_environment(
        EnvironmentView::new(
            host.level_gravity,
            &app.environment_volumes,
            host.fluids.viscosities(),
        ),
        &BodyProbe {
            position,
            half_height: 0.5,
            radius: 0.25,
            eye_height: 0.6,
        },
    ))
}

// N7, N9 and N11 through the App: a connected client's volumes resolve dry
// until the host's table arrives with tuning, then only against that table,
// never the client's own manifest; a table without the fluid, or a demotion,
// makes the volume dry again.
#[test]
fn a_connected_client_resolves_fluid_volumes_only_against_the_installed_host_table() {
    let mut app = test_app();
    // The client's own manifest declares water, at a different index.
    declare_fluids(&app, &["sludge", "water"]);
    connect_as_client(&mut app);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    assert!(app.active_environment_fluids().names().is_empty());
    assert_eq!(
        client_resolve_at(&app, SUBMERGED),
        None,
        "no host tuning yet"
    );
    assert_eq!(
        resolve_at(&app, SUBMERGED).fluid,
        None,
        "dry before the host's table, despite the client's own water"
    );

    deliver_host_fluids(&mut app, &[("water", 3.5)]);
    assert_eq!(app.active_environment_fluids().names(), ["water"]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (Some(0), 3.5));

    // A viscosity-only resend keeps the installed volumes.
    let capture = postretro_test_log_capture::LogCapture::start();
    deliver_host_fluids(&mut app, &[("water", 1.25)]);
    capture.assert_not_logged(log::Level::Info, "[Loader] environment volumes:");
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (Some(0), 1.25));

    // N11: a host table without water leaves the volume dry on this client,
    // though its own manifest declares water.
    deliver_host_fluids(&mut app, &[("lava", 9.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (None, 0.0));

    // N9 (client half): the host's reload adds water back; the volume follows.
    deliver_host_fluids(&mut app, &[("lava", 9.0), ("water", 2.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (Some(1), 2.0));

    // Demotion drops the host's tuning: dry until the next one installs.
    crate::client_drain_control(
        &mut app,
        vec![postretro_net::wire::ServerControlMessage::Divergence(
            postretro_net::wire::DivergenceReason::Holding(
                postretro_net::wire::HoldingCause::HostLevelAbsent,
            ),
        )],
    );
    assert!(app.active_environment_fluids().names().is_empty());
    assert_eq!(resolve_at(&app, SUBMERGED).fluid, None);
}

// N8 through the App: the presentation lookup maps the active table's index to
// a name and the LOCAL manifest's declaration of it.
#[test]
fn fluid_presentation_resolves_the_active_fluid_against_the_local_manifest() {
    const CLIENT_TINT: [f32; 4] = [0.9, 0.2, 0.5, 0.7];
    let mut app = test_app();
    script_ctx(&app).data_registry.borrow_mut().replace_fluids(
        postretro_foundation::FluidRegistry::from_declarations(
            "test",
            [Ok(fluid_with("water", 8.0, CLIENT_TINT))],
        ),
    );
    connect_as_client(&mut app);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    assert!(
        app.fluid_presentation_for_volume(0).is_none(),
        "dry: no table yet"
    );

    deliver_host_fluids(&mut app, &[("sludge", 6.0), ("water", 2.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!(
        (resolved.fluid, resolved.viscosity),
        (Some(1), 2.0),
        "movement takes the host's water"
    );
    let shown = app
        .fluid_presentation(1)
        .expect("the client declares water");
    assert_eq!(
        shown.view_tint, CLIENT_TINT,
        "presentation is the client's own"
    );
    drop(shown);
    assert_eq!(
        app.fluid_presentation_for_volume(0)
            .map(|fluid| fluid.view_tint),
        Some(CLIENT_TINT)
    );
    assert!(
        app.fluid_presentation(0).is_none(),
        "no local sludge: that fluid shows nothing"
    );

    // A client whose manifest lacks water still resolves it, and shows nothing.
    declare_fluids(&app, &["sludge"]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!(resolved.fluid, Some(1));
    assert!(app.fluid_presentation(1).is_none());
    assert!(app.fluid_presentation_for_volume(0).is_none());
}

// Offline or hosting, the active table is the machine's own registry, so the
// presentation lookup returns its own declaration.
#[test]
fn a_host_presents_its_own_fluid_declaration() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    let resolved = resolve_at(&app, SUBMERGED);
    assert_eq!((resolved.fluid, resolved.viscosity), (Some(0), 2.0));
    assert_eq!(
        app.fluid_presentation_for_volume(0)
            .map(|fluid| fluid.name.clone()),
        Some("water".to_string())
    );
}
