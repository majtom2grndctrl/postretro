// Environment volumes through the real level install: built before the parity
// digest and the first tick, replaced whole on the next level, and resolved
// against the script gravity register the way every tick call site builds it.
// See: context/lib/boot_sequence.md §3 · context/lib/entity_model.md §7 (Environment Volumes)

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
        .find("self.install_environment_volumes(")
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
        fog_distance: None,
        fog_color: None,
        wave_period: None,
        wave_amplitude: 8.0,
        wave_length: [64.0, 80.0],
        surface_reflectivity: 1.0,
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

// An undeclared fluid name loads dry and warns, naming the entity and the fluid.
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

// Host side: the real staged reload route declares the missing fluid.
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

    commit_staged_manifest(&mut app, mod_root.path());

    assert_eq!(resolve_at(&app, SUBMERGED).fluid, Some(1));
}

/// Queue a staged manifest build of the start script under `mod_root` and poll
/// until it commits, the route a host's manifest reload takes.
fn commit_staged_manifest(app: &mut App, mod_root: &std::path::Path) {
    let generation = app
        .session
        .as_mut()
        .expect("test app session installed")
        .scripting
        .script_runtime
        .enqueue_staged_manifest_build(mod_root)
        .expect("staged build queued")
        .expect("debug builds stage manifest reloads");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while script_runtime_committed(app) != Some(generation) {
        assert!(
            std::time::Instant::now() < deadline,
            "the staged manifest never committed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_staged_manifest_results();
    }
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
    deliver_host_tuning(app, fluids, None);
}

/// Like [`deliver_host_fluids`], with the host's player movement descriptor
/// riding the same payload.
fn deliver_host_tuning(
    app: &mut App,
    fluids: &[(&str, f32)],
    movement: Option<postretro_foundation::PlayerMovementDescriptor>,
) {
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
        movement,
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

// Through the App: a connected client's volumes resolve dry until the host's
// table arrives with tuning, then only against that table, never the client's
// own manifest; a table without the fluid, or a demotion, makes the volume dry
// again.
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

    // A host table without water leaves the volume dry on this client,
    // though its own manifest declares water.
    deliver_host_fluids(&mut app, &[("lava", 9.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (None, 0.0));

    // The host's reload adds water back; the volume follows.
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

const UNDECLARED_FLUID_WARNING: &str = "names undeclared fluid";

// A connected client never resolves names against its own registry, so it
// never warns about one: before the host's table installs (and after a
// demotion drops it) it builds its volumes with every fluid dry, so gravity
// and push still hold, and once the table arrives the fluids resolve, even
// from an empty table.
#[test]
fn a_connected_clients_install_and_demotion_log_no_undeclared_fluid_warning() {
    const PUSH: [f32; 3] = [0.0, 0.0, 3.0];
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut app = test_app();
    // The client's own manifest declares water but lacks lava; only the
    // host's table decides either.
    declare_fluids(&app, &["water"]);
    connect_as_client(&mut app);
    let mut world = level_with_fluid_volume(7, "lava");
    for (source_entity, gravity, push, fluid, min) in [
        (8, Some(VOLUME_GRAVITY), None, "", 10.0),
        (9, None, Some(PUSH), "", 20.0),
        (10, None, None, "water", 30.0),
    ] {
        world
            .environment_volumes
            .volumes
            .push(EnvironmentVolumeRecord {
                source_entity,
                priority: 0,
                gravity,
                push,
                fluid: fluid.to_string(),
                brushes: vec![box_brush(Vec3::splat(min), Vec3::splat(min + 4.0))],
                surface_faces: Vec::new(),
            });
    }
    app.install_level_payload(world, PathBuf::from("baked"))
        .expect("fixture installs");
    let assert_awaiting = |app: &App, when: &str| {
        assert_eq!(
            app.environment_volumes.len(),
            4,
            "{when}: every volume builds"
        );
        assert_eq!(
            resolve_at(app, SUBMERGED).fluid,
            None,
            "{when}: lava is dry"
        );
        assert_eq!(
            resolve_at(app, Vec3::splat(32.0)).fluid,
            None,
            "{when}: water is dry though the client's own manifest declares it"
        );
        assert_eq!(
            resolve_at(app, Vec3::splat(12.0)).gravity,
            VOLUME_GRAVITY,
            "{when}"
        );
        assert_eq!(
            resolve_at(app, Vec3::splat(22.0)).push,
            Vec3::from(PUSH),
            "{when}"
        );
    };
    assert_awaiting(&app, "before the host's table");

    // The host's table carries no fluid at all: still the source the client
    // was waiting for; the gravity volume applies under it.
    deliver_host_fluids(&mut app, &[]);
    assert_eq!(app.environment_volumes.len(), 4);
    let host = client_resolve_at(&app, Vec3::splat(12.0)).expect("host tuning installed");
    assert_eq!(host.gravity, VOLUME_GRAVITY);
    assert_eq!(client_resolve_at(&app, SUBMERGED).unwrap().fluid, None);

    deliver_host_fluids(&mut app, &[("lava", 9.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!((resolved.fluid, resolved.viscosity), (Some(0), 9.0));

    crate::client_drain_control(
        &mut app,
        vec![postretro_net::wire::ServerControlMessage::Divergence(
            postretro_net::wire::DivergenceReason::Holding(
                postretro_net::wire::HoldingCause::HostLevelAbsent,
            ),
        )],
    );
    assert_awaiting(&app, "after a demotion");

    capture.assert_not_logged(log::Level::Warn, UNDECLARED_FLUID_WARNING);
}

// A fluid-table reorder moves fluid indices, so a pawn swimming through the
// reload is remapped by name and its exit edge plays the fluid it was in, not
// the one that took its old index.
#[test]
fn a_host_reload_that_reorders_fluids_mid_swim_keeps_the_swimmers_fluid() {
    use postretro_foundation::{
        FluidDescriptor, FluidRegistry, MovementState, PlayerMovementComponent,
    };
    let sounding = |name: &str, exit: &str| FluidDescriptor {
        exit_sound: Some(exit.to_string()),
        ..fluid(name)
    };
    let declare = |app: &App, fluids: Vec<FluidDescriptor>| {
        script_ctx(app).data_registry.borrow_mut().replace_fluids(
            FluidRegistry::from_declarations("test", fluids.into_iter().map(Ok)),
        );
    };
    let mut app = test_app();
    declare(
        &app,
        vec![
            sounding("water", "sfx/water_out"),
            sounding("sludge", "sfx/sludge_out"),
        ],
    );
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    spawn_local_pawn(&app, "swimmer", Some(swim_block()));
    let ctx = script_ctx(&app);
    let pawn = ctx
        .registry
        .borrow()
        .local_player_pawn()
        .expect("local pawn");
    let swim_state = |app: &App| {
        script_ctx(app)
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("movement")
            .movement_state
    };
    {
        let mut registry = ctx.registry.borrow_mut();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("movement")
            .clone();
        movement.movement_state = MovementState::Swim { fluid: 0 };
        registry.set_component(pawn, movement).expect("movement");
    }

    // The reload declares the same fluids in the other order.
    declare(
        &app,
        vec![
            sounding("sludge", "sfx/sludge_out"),
            sounding("water", "sfx/water_out"),
        ],
    );
    app.refresh_environment_fluids();
    assert_eq!(swim_state(&app), MovementState::Swim { fluid: 1 });
    assert_eq!(resolve_at(&app, SUBMERGED).fluid, Some(1));

    // Rise out of the pool: the exit edge plays water's exit sound.
    ctx.registry
        .borrow_mut()
        .set_component(
            pawn,
            postretro_entities::Transform {
                position: Vec3::new(2.0, 20.0, 2.0),
                ..Default::default()
            },
        )
        .expect("transform");
    let view = EnvironmentView::new(
        LEVEL_GRAVITY,
        &app.environment_volumes,
        app.environment_fluids.viscosities(),
    );
    let idle = postretro_physics::movement::MovementInput {
        wish_dir: glam::Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        facing_pitch: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };
    let mut registry = ctx.registry.borrow_mut();
    let ticks = postretro_sim::sim::run_host_movement_tick(
        &mut registry,
        &app.collision_world,
        view,
        &[(pawn, idle)],
        1.0 / 60.0,
    );
    let mut emissions = Vec::new();
    for (id, events) in ticks {
        let emitter = postretro_sim::emission::entity_emitter(&registry, id);
        postretro_sim::emission::append_movement_emissions(&events, &emitter, &mut emissions);
    }
    assert!(!matches!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .movement_state,
        MovementState::Swim { .. }
    ));
    let data = ctx.data_registry.borrow();
    let mut movers = crate::runtime_movers::KinematicMoverRenderCollector::new();
    let mut scene = crate::sound_events::AnchorScene {
        registry: &registry,
        world: None,
        movers: &mut movers,
    };
    let fluids = crate::sound_events::LocalFluids {
        active: &app.environment_fluids,
        local: &data.fluids,
    };
    let played: Vec<String> = emissions
        .iter()
        .filter_map(|emission| crate::sound_events::movement_sound(emission, fluids, &mut scene))
        .map(|request| request.sound)
        .collect();
    assert_eq!(played, ["sfx/water_out"]);
}

/// This machine's presentation of installed volume `volume`, read through the
/// shared lookup every per-frame consumer uses.
fn volume_presentation<T>(
    app: &App,
    volume: usize,
    read: impl FnOnce(&postretro_foundation::FluidDescriptor) -> T,
) -> Option<T> {
    let ctx = script_ctx(app);
    let registry = ctx.data_registry.borrow();
    crate::fluid_presentation::local_fluid_presentation(
        &app.environment_volumes,
        &app.environment_fluids,
        &registry.fluids,
        volume,
    )
    .map(read)
}

/// This machine's presentation of active-table fluid `fluid`: the local
/// declaration of the name the active table gives it.
fn fluid_presentation<T>(
    app: &App,
    fluid: u16,
    read: impl FnOnce(&postretro_foundation::FluidDescriptor) -> T,
) -> Option<T> {
    let ctx = script_ctx(app);
    let registry = ctx.data_registry.borrow();
    app.environment_fluids
        .local_presentation(fluid, &registry.fluids)
        .map(read)
}

// Through the App: the presentation lookup maps the active table's index to
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
        volume_presentation(&app, 0, |_| ()).is_none(),
        "dry: no table yet"
    );

    deliver_host_fluids(&mut app, &[("sludge", 6.0), ("water", 2.0)]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!(
        (resolved.fluid, resolved.viscosity),
        (Some(1), 2.0),
        "movement takes the host's water"
    );
    assert_eq!(
        fluid_presentation(&app, 1, |fluid| fluid.view_tint),
        Some(CLIENT_TINT),
        "presentation is the client's own"
    );
    assert_eq!(
        volume_presentation(&app, 0, |fluid| fluid.view_tint),
        Some(CLIENT_TINT)
    );
    assert!(
        fluid_presentation(&app, 0, |_| ()).is_none(),
        "no local sludge: that fluid shows nothing"
    );

    // A client whose manifest lacks water still resolves it, and shows nothing.
    declare_fluids(&app, &["sludge"]);
    let resolved = client_resolve_at(&app, SUBMERGED).expect("host tuning installed");
    assert_eq!(resolved.fluid, Some(1));
    assert!(fluid_presentation(&app, 1, |_| ()).is_none());
    assert!(volume_presentation(&app, 0, |_| ()).is_none());
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
        volume_presentation(&app, 0, |fluid| fluid.name.clone()),
        Some("water".to_string())
    );
}

/// Spawn the local player pawn with `swim` on its movement descriptor, as the
/// boot spawn from a `player_spawn` would, under descriptor `canonical_name`.
fn spawn_local_pawn(
    app: &App,
    canonical_name: &str,
    swim: Option<postretro_foundation::SwimParams>,
) {
    use postretro_entities::Transform;
    use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
    let mut descriptor = crate::tests::minimal_player_descriptor();
    descriptor.swim = swim;
    let ctx = script_ctx(app);
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            pawn,
            postretro_foundation::PlayerMovementComponent::from_descriptor(&descriptor),
        )
        .unwrap();
    registry
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: canonical_name.to_string(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::PlayerSpawn,
            },
        )
        .unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
}

fn swim_block() -> postretro_foundation::SwimParams {
    postretro_foundation::SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.8,
        enter_depth: 0.5,
        exit_depth: 0.35,
        fluid_jump_velocity: Some(6.5),
    }
}

const WADE_ONLY_WARNING: &str = "has no movement.swim block; this level has fluid volumes";

// A level with fluid volumes, loaded for a player whose descriptor has
// no swim block, warns once naming the descriptor.
#[test]
fn a_fluid_level_for_a_player_without_a_swim_block_warns_once_naming_the_descriptor() {
    // The warning runs once per install, right after the boot pawn spawns.
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
    let spawn = install
        .find("install_world_cpu(")
        .expect("install spawns the boot pawn");
    let warn = install
        .find("self.warn_if_local_player_cannot_swim();")
        .expect("install warns about a wade-only player");
    assert!(spawn < warn, "the warning reads the spawned pawn");
    assert_eq!(
        install.matches("warn_if_local_player_cannot_swim").count(),
        1,
        "once per install"
    );

    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    spawn_local_pawn(&app, "wader", None);

    let capture = postretro_test_log_capture::LogCapture::start();
    app.warn_if_local_player_cannot_swim();
    capture.assert_logged_once(
        log::Level::Warn,
        "[Movement] player descriptor 'wader' has no movement.swim block; this level has fluid volumes, so the player wades through them and cannot swim",
    );
}

#[test]
fn a_swimming_player_or_a_dry_level_never_warns_about_the_swim_block() {
    // A swimmer in a fluid level.
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "water"), PathBuf::from("baked"))
        .expect("fixture installs");
    spawn_local_pawn(&app, "swimmer", Some(swim_block()));
    {
        let capture = postretro_test_log_capture::LogCapture::start();
        app.warn_if_local_player_cannot_swim();
        capture.assert_not_logged(log::Level::Warn, WADE_ONLY_WARNING);
    }

    // A wader in a level whose only fluid volume loads dry.
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_fluid_volume(7, "lava"), PathBuf::from("baked"))
        .expect("fixture installs");
    spawn_local_pawn(&app, "wader", None);
    let capture = postretro_test_log_capture::LogCapture::start();
    app.warn_if_local_player_cannot_swim();
    capture.assert_not_logged(log::Level::Warn, WADE_ONLY_WARNING);
}

/// A level with one `water` pool spanning `0..4` on every axis, its top face
/// drawn, under a coincident current pushing +X at `push` m/s².
fn level_with_current(push: f32) -> LevelWorld {
    use postretro_level_format::environment_volumes::{
        EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
    };
    let corner = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 4.0, z],
        uv: [x * 39.37, z * 39.37],
    };
    let mut world = level_with_fluid_volume(7, "water");
    world.environment_volumes.volumes[0].surface_faces = vec![EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            corner(0.0, 4.0),
            corner(4.0, 4.0),
            corner(4.0, 0.0),
            corner(0.0, 0.0),
        ],
    }];
    world
        .environment_volumes
        .volumes
        .push(EnvironmentVolumeRecord {
            source_entity: 8,
            priority: 0,
            gravity: None,
            push: Some([push, 0.0, 0.0]),
            fluid: String::new(),
            brushes: vec![box_brush(Vec3::ZERO, Vec3::splat(4.0))],
            surface_faces: Vec::new(),
        });
    world
}

#[track_caller]
fn assert_drift(app: &App, expected_x: f32) {
    assert_eq!(app.fluid_surface_drift.len(), 1, "one drawn face");
    let drift = app.fluid_surface_drift[0];
    assert!(
        (drift - Vec3::new(expected_x, 0.0, 0.0)).length() < 1.0e-4,
        "drift {drift}, expected {expected_x} m/s along +X"
    );
}

// A host manifest reload that changes only a viscosity, names unchanged, still
// re-resolves drift.
#[test]
fn a_viscosity_only_host_reload_re_resolves_fluid_surface_drift() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    // push 6 ÷ water viscosity 2.
    assert_drift(&app, 3.0);

    script_ctx(&app).data_registry.borrow_mut().replace_fluids(
        postretro_foundation::FluidRegistry::from_declarations(
            "test",
            [Ok(fluid_with("water", 6.0, [0.1, 0.3, 0.4, 0.35]))],
        ),
    );
    let capture = postretro_test_log_capture::LogCapture::start();
    app.refresh_environment_fluids();
    capture.assert_not_logged(log::Level::Info, "[Loader] environment volumes:");
    assert_drift(&app, 1.0);
}

// A client draws drift only from the host's table. Before the tuning
// arrives its fluid shows nothing and drifts none; each tuning, a
// viscosity-only resend included, re-resolves drift; a demotion stills it, and
// the next tuning brings the new table's drift.
#[test]
fn a_connected_client_resolves_drift_from_each_host_tuning() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    connect_as_client(&mut app);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    assert_drift(&app, 0.0);
    let local = script_ctx(&app).data_registry.borrow().fluids.clone();
    assert!(
        crate::fluid_presentation::local_fluid_presentation(
            &app.environment_volumes,
            &app.environment_fluids,
            &local,
            0,
        )
        .is_none(),
        "no fluid surface draws before the host's table"
    );

    deliver_host_fluids(&mut app, &[("water", 2.0)]);
    assert_drift(&app, 3.0);
    deliver_host_fluids(&mut app, &[("water", 1.25)]);
    assert_drift(&app, 4.8);

    crate::client_drain_control(
        &mut app,
        vec![postretro_net::wire::ServerControlMessage::Divergence(
            postretro_net::wire::DivergenceReason::Holding(
                postretro_net::wire::HoldingCause::HostLevelAbsent,
            ),
        )],
    );
    assert_drift(&app, 0.0);
    deliver_host_fluids(&mut app, &[("water", 6.0)]);
    assert_drift(&app, 1.0);
}

// Tuning before the level install and after it draw identical drift.
#[test]
fn a_client_tuned_before_its_level_install_draws_the_same_drift_as_one_tuned_after() {
    let mut tuned_first = test_app();
    declare_fluids(&tuned_first, &["water"]);
    connect_as_client(&mut tuned_first);
    deliver_host_fluids(&mut tuned_first, &[("water", 2.0)]);
    tuned_first
        .install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");

    let mut tuned_after = test_app();
    declare_fluids(&tuned_after, &["water"]);
    connect_as_client(&mut tuned_after);
    tuned_after
        .install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    deliver_host_fluids(&mut tuned_after, &[("water", 2.0)]);

    assert_drift(&tuned_first, 3.0);
    assert_eq!(
        tuned_first.fluid_surface_drift,
        tuned_after.fluid_surface_drift
    );
}

// A connected client caps drift at the terminal velocity of the movement
// descriptor on the host's tuning payload, the one its own prediction reads.
#[test]
fn a_connected_clients_drift_caps_at_the_host_tuning_terminal_velocity() {
    let mut app = test_app();
    connect_as_client(&mut app);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");

    let mut movement = crate::tests::minimal_player_descriptor();
    movement.fall.terminal_velocity = 1.5;
    deliver_host_tuning(&mut app, &[("water", 2.0)], Some(movement));
    // push 6 ÷ water viscosity 2 would settle at 3; the host's cap holds it.
    assert_drift(&app, 1.5);

    // A payload without a movement descriptor leaves drift uncapped.
    deliver_host_tuning(&mut app, &[("water", 2.0)], None);
    assert_drift(&app, 3.0);
}

// Host and offline cap drift at the local pawn's live movement component, the
// value movement caps against, not at a descriptor looked up by class name.
#[test]
fn a_hosts_drift_caps_at_the_local_pawns_live_terminal_velocity() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    // No pawn and no registered descriptor: no cap.
    assert_drift(&app, 3.0);

    spawn_local_pawn(&app, "swimmer", None);
    let ctx = script_ctx(&app);
    {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.local_player_pawn().expect("local pawn");
        let mut movement = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            .expect("movement")
            .clone();
        movement.fall.terminal_velocity = 1.5;
        registry.set_component(pawn, movement).expect("movement");
    }
    // No descriptor named `swimmer` is registered, so only the pawn's own
    // component can supply this cap.
    app.resolve_fluid_surface_drift();
    assert_drift(&app, 1.5);
}

/// A start-script manifest declaring `water` (viscosity 2) and a `player`
/// descriptor whose movement caps falling at `terminal_velocity`.
fn manifest_with_player_terminal_velocity(terminal_velocity: f32) -> String {
    format!(
        "globalThis.__postretroModManifest = {{ name: 'Fluids', id: 'fluids', version: '1', \
         fluids: [{{ name: 'water', viscosity: 2, viewTint: [0.1, 0.3, 0.4, 0.35], muffle: 0.8, surfaceOpacity: 0.6 }}], \
         entities: [{{ canonicalName: 'player', components: {{ movement: {{ \
         capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }}, \
         ground: {{ speed: {{ walk: 7, run: 11, crouch: 3 }}, accel: 10, stepHeight: 0.3, maxSlope: 45 }}, \
         air: {{ forwardSteer: 0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0 }}, \
         fall: {{ terminalVelocity: {terminal_velocity} }} }} }} }}] }};"
    )
}

// A host manifest reload that changes only the player's terminal velocity
// re-resolves drift, through the production route: the commit refreshes the
// live pawn's movement component from the new descriptor, then the fluid refresh
// reads that component's cap.
#[test]
fn a_host_manifest_reload_that_changes_the_player_terminal_velocity_re_resolves_drift() {
    use postretro_entities::provenance::{DescriptorComponentKind, DescriptorProvenance};
    let mod_root = tempfile::tempdir().expect("temporary mod root");
    let start_script = mod_root.path().join("start-script.js");
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    // push 6 ÷ water viscosity 2, no cap yet.
    assert_drift(&app, 3.0);

    // The first reload registers the player descriptor; a refresh needs a prior
    // descriptor snapshot to diff against. Its cap sits above the drift.
    std::fs::write(&start_script, manifest_with_player_terminal_velocity(40.0))
        .expect("first manifest written");
    commit_staged_manifest(&mut app, mod_root.path());
    assert_drift(&app, 3.0);

    // The local pawn spawns as the descriptor's, owning its movement, high
    // enough above the floor that a refreshed capsule cannot embed it.
    spawn_local_pawn(&app, "player", None);
    {
        let ctx = script_ctx(&app);
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.local_player_pawn().expect("local pawn");
        registry
            .set_component(
                pawn,
                postretro_entities::Transform {
                    position: Vec3::new(2.0, 6.0, 2.0),
                    ..Default::default()
                },
            )
            .expect("transform");
        let mut provenance = registry
            .get_component::<DescriptorProvenance>(pawn)
            .expect("provenance")
            .clone();
        provenance
            .owned_components
            .insert(DescriptorComponentKind::Movement);
        registry
            .set_component(pawn, provenance)
            .expect("provenance");
    }

    std::fs::write(&start_script, manifest_with_player_terminal_velocity(1.5))
        .expect("reloaded manifest written");
    commit_staged_manifest(&mut app, mod_root.path());

    let ctx = script_ctx(&app);
    let live_cap = {
        let registry = ctx.registry.borrow();
        let pawn = registry.local_player_pawn().expect("local pawn");
        registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            .expect("movement")
            .fall
            .terminal_velocity
    };
    assert_eq!(live_cap, 1.5, "the reload refreshed the pawn's movement");
    // push 6 ÷ water viscosity 2 would settle at 3; the reloaded cap holds it.
    assert_drift(&app, 1.5);
}

// Unloading a level leaves no drift behind for the next frames.
#[test]
fn unloading_a_level_clears_its_fluid_surface_drift() {
    let mut app = test_app();
    declare_fluids(&app, &["water"]);
    app.install_level_payload(level_with_current(6.0), PathBuf::from("baked"))
        .expect("fixture installs");
    assert_drift(&app, 3.0);

    app.unload_level();
    assert!(app.fluid_surface_drift.is_empty());
}

// --- Reflectivity through the staged reload (pins P12, P24) -----------------

/// A start script declaring `water` at the given opacity and reflectivity.
fn reflecting_water_script(surface_opacity: &str, surface_reflectivity: &str) -> String {
    format!(
        "globalThis.__postretroModManifest = {{ name: 'Fluids', id: 'fluids', version: '1', \
         fluids: [{{ name: 'water', viscosity: 2, viewTint: [0.1, 0.3, 0.4, 0.35], muffle: 0.8, \
         surfaceOpacity: {surface_opacity}, surfaceReflectivity: {surface_reflectivity} }}] }};"
    )
}

/// [`level_with_fluid_volume`] naming `water`, its top face at y 4 bound to
/// one reflection probe.
fn level_with_reflecting_pool() -> LevelWorld {
    use postretro_level_format::environment_volumes::{
        EnvironmentSurfaceFace, EnvironmentSurfaceVertex,
    };
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
    use postretro_level_format::reflection_probes::{
        REFLECTION_PROBE_LIGHTS_PER_PROBE, ReflectionProbeBinding, ReflectionProbeLightShare,
        ReflectionProbeRecord, ReflectionProbesSection,
    };
    let mut world = level_with_fluid_volume(7, "water");
    let vertex = |x: f32, z: f32| EnvironmentSurfaceVertex {
        position: [x, 4.0, z],
        uv: [0.0, 0.0],
    };
    world.environment_volumes.volumes[0].surface_faces = vec![EnvironmentSurfaceFace {
        texture_index: 0,
        vertices: vec![
            vertex(0.0, 0.0),
            vertex(0.0, 4.0),
            vertex(4.0, 4.0),
            vertex(4.0, 0.0),
        ],
    }];
    world.reflection_probes = Some(ReflectionProbesSection {
        tile_dimension: 4,
        tile_border: 1,
        atlas_format: IRRADIANCE_FORMAT_BC6H,
        probes: vec![ReflectionProbeRecord {
            source_entity: 9,
            box_min: [0.0, 0.0, 0.0],
            box_max: [4.0, 8.0, 4.0],
            capture_point: [2.0, 4.002, 2.0],
            layer: 0,
            lights: [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE],
        }],
        bindings: vec![ReflectionProbeBinding {
            volume: 0,
            face: 0,
            probe: 0,
        }],
        tiles: vec![0; 16],
    });
    world
}

/// This frame's fluid draws, as the app collects them for the renderer.
fn collect_draws(app: &mut App) -> Vec<postretro_render_cpu::fluid_surface::FluidSurfaceDraw> {
    let local = script_ctx(app).data_registry.borrow().fluids.clone();
    crate::fluid_presentation::collect_fluid_surface_draws(
        &mut app.fluid_surface_render,
        app.level.as_ref().expect("installed"),
        &postretro_visibility::VisibleCells::DrawAll,
        Vec3::new(2.0, 6.0, 2.0),
        &app.environment_volumes,
        &app.environment_fluids,
        &local,
    )
    .to_vec()
}

/// Queue a staged build of the start script under `mod_root` and poll until
/// it fails, the route a broken reload takes: it never commits.
fn fail_staged_manifest(app: &mut App, mod_root: &std::path::Path) {
    let capture = postretro_test_log_capture::LogCapture::start();
    let generation = app
        .session
        .as_mut()
        .expect("test app session installed")
        .scripting
        .script_runtime
        .enqueue_staged_manifest_build(mod_root)
        .expect("staged build queued")
        .expect("debug builds stage manifest reloads");
    let failed = format!("[Scripting] staged mod-init generation {generation} failed");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !capture
        .records()
        .iter()
        .any(|record| record.message.contains(&failed))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the staged manifest never failed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_staged_manifest_results();
    }
}

// Pin P12 for reflectivity, through the real staged reload route: the frame
// after a reload that changes only `surfaceReflectivity` draws the new value
// and re-uploads that fluid's row; an unchanged re-collect uploads nothing;
// a failed reload keeps the previous value.
#[test]
fn a_staged_reload_changing_only_reflectivity_draws_the_new_value_next_frame() {
    use postretro_render_cpu::fluid_surface::{FluidSurfaceSlots, pack_fluid_surfaces};
    let mod_root = tempfile::tempdir().expect("temporary mod root");
    let script = mod_root.path().join("start-script.js");
    std::fs::write(&script, reflecting_water_script("0.6", "1")).expect("manifest written");
    let mut app = test_app();
    commit_staged_manifest(&mut app, mod_root.path());
    let world = level_with_reflecting_pool();
    let mesh = pack_fluid_surfaces(&world.environment_volumes.volumes, &[], 1);
    app.install_level_payload(world, PathBuf::from("baked"))
        .expect("fixture installs");
    let mut slots = FluidSurfaceSlots::new(&mesh.volumes);

    let draws = collect_draws(&mut app);
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].params.reflectivity(), 1.0);
    assert_eq!(draws[0].params.opacity(), 0.6);
    slots.set_draws(&draws);
    assert!(slots.take_dirty().is_some(), "the first frame uploads");

    std::fs::write(&script, reflecting_water_script("0.6", "2.5")).expect("manifest written");
    commit_staged_manifest(&mut app, mod_root.path());
    let draws = collect_draws(&mut app);
    assert_eq!(
        draws[0].params.reflectivity(),
        2.5,
        "frame N draws the new value"
    );
    assert_eq!(draws[0].params.opacity(), 0.6, "opacity unchanged");
    slots.set_draws(&draws);
    let (range, rows) = slots.take_dirty().expect("the changed row uploads");
    assert_eq!(range, 0..1);
    assert_eq!(rows[0].reflectivity(), 2.5);
    slots.set_draws(&collect_draws(&mut app));
    assert!(
        slots.take_dirty().is_none(),
        "an unchanged re-collect uploads no row"
    );

    std::fs::write(&script, "throw new Error('broken reload');").expect("manifest written");
    fail_staged_manifest(&mut app, mod_root.path());
    let draws = collect_draws(&mut app);
    assert_eq!(
        draws[0].params.reflectivity(),
        2.5,
        "a failed reload keeps the value"
    );
    slots.set_draws(&draws);
    assert!(slots.take_dirty().is_none());
}

// Pin P24 through the staged reload route: a zero-opacity, fog-free fluid
// with a bound face leaves the draw set on the frame a reload commits
// reflectivity 1 → 0, and draws its reflection again after 0 → 1.
#[test]
fn a_staged_reload_flips_a_zero_opacity_fluids_draw_with_its_reflectivity() {
    let mod_root = tempfile::tempdir().expect("temporary mod root");
    let script = mod_root.path().join("start-script.js");
    std::fs::write(&script, reflecting_water_script("0", "1")).expect("manifest written");
    let mut app = test_app();
    commit_staged_manifest(&mut app, mod_root.path());
    app.install_level_payload(level_with_reflecting_pool(), PathBuf::from("baked"))
        .expect("fixture installs");
    let reflecting = collect_draws(&mut app);
    assert_eq!(reflecting.len(), 1, "a zero-opacity reflecting fluid draws");
    assert_eq!(reflecting[0].params.opacity(), 0.0);

    std::fs::write(&script, reflecting_water_script("0", "0")).expect("manifest written");
    commit_staged_manifest(&mut app, mod_root.path());
    assert!(
        collect_draws(&mut app).is_empty(),
        "frame N after 1 → 0 collects no draw"
    );

    std::fs::write(&script, reflecting_water_script("0", "1")).expect("manifest written");
    commit_staged_manifest(&mut app, mod_root.path());
    assert_eq!(
        collect_draws(&mut app),
        reflecting,
        "the reverse reload draws again"
    );
}
