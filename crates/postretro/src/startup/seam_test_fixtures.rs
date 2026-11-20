//! Level seam lifecycle fixtures: seam records, a CPU host install that runs
//! segment B's world install, the post-load passes, the seam install and the
//! camera pose in production order, and seam activations through the stage.
//! See: context/lib/testing_guide.md §4 · context/lib/entity_model.md §7e

use super::lifecycle_test_fixtures::{
    catalog_map, drop_in_flight_load, level_world, script_ctx, test_app,
};
use super::lifecycle_world_handles::bind_installed_first_player;
use super::*;
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport};
use glam::Quat;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_format::seams::{
    SEAM_CLASSNAME, SEAM_SOURCE_ENTITY_KEY, SeamActivation, SeamMode, SeamRecord, SeamsSection,
};
use postretro_physics::movement::seam_arrival::{Landmark, LandmarkRelative};
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_sim::trigger_system::{AuthoritativePlayer, PlayerId};
use std::collections::HashMap;
use std::time::Instant;

pub(super) use super::spawn_pose_handoff_fixtures::{descriptors, spawn_points};

/// The level a crossing leaves, and the level it arrives in.
pub(super) const SOURCE: &str = "seam-source";
pub(super) const DESTINATION: &str = "seam-destination";
pub(super) const THIRD: &str = "seam-third";
/// The crossing seam's name, shared by both maps.
pub(super) const DOOR: &str = "door";

/// The source seam's landmark: yaw 0, so a source offset reads as authored.
pub(super) const SOURCE_LANDMARK: Vec3 = Vec3::new(20.0, 0.0, 0.0);
/// The destination seam's landmark, far from every start and turned.
pub(super) const DESTINATION_LANDMARK: Vec3 = Vec3::new(-40.0, 2.0, 30.0);
pub(super) const DESTINATION_YAW: f32 = std::f32::consts::FRAC_PI_2;

/// What a settle check reports for a level that streams nothing: settled.
const SETTLED: SettleReport = SettleReport {
    sh: ResourceSettle::NotStreamed,
    lightmap: ResourceSettle::NotStreamed,
};

pub(super) fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal: normal.to_array(),
            distance: max[axis],
        });
        planes.push(EnvironmentPlane {
            normal: (-normal).to_array(),
            distance: -min[axis],
        });
    }
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// A touch seam `name` targeting `target`: an 8 x 4 x 8 box whose floor
/// centre is its landmark, facing `yaw`.
pub(super) fn seam(name: &str, target: &str, landmark: Vec3, yaw: f32) -> SeamRecord {
    SeamRecord {
        source_entity: 0,
        name: name.into(),
        target: target.into(),
        activation: SeamActivation::Touch,
        mode: SeamMode::Classic,
        enabled_on_spawn: true,
        brushes: vec![box_brush(
            landmark - Vec3::new(4.0, 0.0, 4.0),
            landmark + Vec3::new(4.0, 4.0, 4.0),
        )],
        landmark_origin: landmark.to_array(),
        landmark_yaw: yaw,
        on_arrive: String::new(),
    }
}

/// The source map's crossing seam.
pub(super) fn source_door() -> SeamRecord {
    seam(DOOR, DESTINATION, SOURCE_LANDMARK, 0.0)
}

/// The destination map's seam of the crossing's name.
pub(super) fn destination_door() -> SeamRecord {
    seam(DOOR, SOURCE, DESTINATION_LANDMARK, DESTINATION_YAW)
}

fn record_of(point: &MapEntity) -> MapEntityRecord {
    MapEntityRecord {
        classname: point.classname.clone(),
        origin: point.origin.to_array(),
        angles: point.angles.to_array(),
        key_values: point
            .key_values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        tags: point.tags.clone(),
    }
}

/// A level holding `seams`, in map order, each with its id-29 companion, and
/// `points` as its `player_spawn`s.
pub(super) fn seam_world(
    seams: Vec<SeamRecord>,
    points: &[MapEntity],
) -> postretro_level_loader::LevelWorld {
    let mut world = level_world("seam", 1);
    world.map_entities = points.iter().map(record_of).collect();
    world.seams = SeamsSection {
        seams: seams
            .into_iter()
            .enumerate()
            .map(|(index, mut record)| {
                record.source_entity = 100 + index as u32;
                world.map_entities.push(MapEntityRecord {
                    classname: SEAM_CLASSNAME.into(),
                    origin: record.landmark_origin,
                    angles: [0.0; 3],
                    key_values: vec![(
                        SEAM_SOURCE_ENTITY_KEY.into(),
                        record.source_entity.to_string(),
                    )],
                    tags: Vec::new(),
                });
                record
            })
            .collect(),
    };
    world
}

/// A single-player App whose catalog holds the three seam levels and whose
/// mod registers the `player` and `scout` classes, standing at its frontend.
pub(super) fn seam_app() -> App {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.data_registry.borrow_mut().entities = descriptors();
    ctx.data_registry.borrow_mut().replace_maps(vec![
        catalog_map(SOURCE, "maps/seam-source.prl", "Source", &[]),
        catalog_map(DESTINATION, "maps/seam-destination.prl", "Destination", &[]),
        catalog_map(THIRD, "maps/seam-third.prl", "Third", &[]),
    ]);
    let session = app.session.as_mut().unwrap();
    session.seat_table = Some(crate::netcode::SeatTable::local_only());
    app.boot_state = BootState::Frontend;
    app
}

fn load_entry(catalog_id: &str) -> InFlightLevelLoad {
    InFlightLevelLoad {
        map_path: PathBuf::from(format!("content/dev/maps/{catalog_id}.prl")),
        content_root: PathBuf::from("content/dev"),
        entry: LevelLoadEntry {
            catalog_id: Some(catalog_id.to_string()),
            path: format!("maps/{catalog_id}.prl"),
            name: catalog_id.to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
        host_named: false,
    }
}

/// Install `world` as catalog level `catalog_id` on the host or in single
/// player, through every renderer-free install stage in production order:
/// identity, segment B (the host's pawn at its start, bound to seat 0), the
/// post-load passes, the seam install, the hints, the camera pose, then
/// Settling. A load in flight stands in for its own stages: its entry names
/// the level.
pub(super) fn install(app: &mut App, catalog_id: &str, world: postretro_level_loader::LevelWorld) {
    let load = app
        .level_load
        .clone()
        .filter(|load| load.entry.catalog_id.as_deref() == Some(catalog_id))
        .unwrap_or_else(|| load_entry(catalog_id));
    if app.level_load.is_some() {
        drop_in_flight_load(app);
    }
    app.level_load = Some(load);
    app.install_level_identity(&world);
    app.collision_world =
        crate::collision::CollisionWorld::from_level(&world).expect("fixture collision is valid");
    app.level = Some(world);
    let ctx = script_ctx(app);
    let content_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let products = {
        let world = app.level.as_ref().unwrap();
        let session = app.session.as_mut().unwrap();
        let handles = WorldInstallHandles {
            world,
            script_ctx: &ctx,
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: Default::default(),
            content_root: &content_root,
            active_level_tags: &[],
            nav_graph: None,
            fog_volume_bridge: &mut session.fog_volume_bridge,
            emissive_volume_bridge: &mut session.emissive_volume_bridge,
            trigger_volume_bridge: &mut session.trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut session.modal_stack,
            progress_tracker: &mut session.progress_tracker,
            crossing_detector: &mut session.crossing_detector,
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            system_reaction_ir_bindings: &session.scripting.system_reaction_ir_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            occupancy_families: &mut session.occupancy_families,
            trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
            suppress_host_replicated: false,
            suppress_boot_pawn: false,
            local_carried_loadout: session
                .seat_table
                .as_ref()
                .and_then(|seats| seats.carried_state(Seat(0)))
                .cloned(),
            spawn_checkpoint: session.saves.spawn_checkpoint().map(str::to_owned),
            spawn_checkpoint_seam: session.saves.spawn_checkpoint_is_seam(),
        };
        let mut first_player_placement = None;
        let products = install_world_cpu(
            handles,
            &mut StartupTimings::new(),
            |_models, _clips| crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide,
            |first_player| {
                first_player_placement = first_player.map(|spawned| spawned.placement);
                if let Some(seats) = session.seat_table.as_mut() {
                    bind_installed_first_player(
                        seats,
                        &mut ctx.registry.borrow_mut(),
                        first_player,
                    );
                }
            },
        );
        crate::respawn_placements::publish(
            &ctx,
            &products.spawn_points,
            &ctx.data_registry.borrow().entities,
            session.seat_table.as_ref(),
            first_player_placement,
        );
        products
    };
    crate::runtime_teleporters::bind_loaded_teleporters(
        &mut app.collision_world.teleporters,
        &ctx.registry.borrow(),
    );
    let first_spawn = app.install_level_post_load_passes(&ctx, products);
    let first_spawn = app.install_level_seams(first_spawn);
    app.send_install_spawn_pose_hints();
    app.install_level_camera_pose(first_spawn);
    app.enter_installed_level_settling();
}

/// Drain the queued level request, then install the load it began from
/// `world` (a request whose load fails to resolve installs nothing).
pub(super) fn drain_and_install(app: &mut App, world: postretro_level_loader::LevelWorld) {
    app.drain_level_requests();
    let catalog_id = app
        .level_load
        .as_ref()
        .and_then(|load| load.entry.catalog_id.clone())
        .expect("the drain began a catalog load");
    install(app, &catalog_id, world);
}

/// A settled check and the reveal edge: Settling ends at Running.
pub(super) fn reveal(app: &mut App) {
    assert_eq!(app.boot_state, BootState::Settling);
    app.advance_settle(SETTLED, Instant::now());
    assert!(app.reveal_if_released(), "a settled level reveals");
    assert_eq!(app.boot_state, BootState::Running);
}

/// The local player's pawn.
pub(super) fn local_pawn(app: &App) -> EntityId {
    script_ctx(app)
        .registry
        .borrow()
        .local_player_pawn()
        .expect("the install spawned the local pawn")
}

pub(super) fn transform(app: &App, pawn: EntityId) -> Transform {
    *script_ctx(app)
        .registry
        .borrow()
        .get_component::<Transform>(pawn)
        .unwrap()
}

pub(super) fn movement(app: &App, pawn: EntityId) -> PlayerMovementComponent {
    script_ctx(app)
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone()
}

/// Move `pawn` to `position` facing `yaw`, moving at `velocity`.
pub(super) fn place(app: &App, pawn: EntityId, position: Vec3, yaw: f32, velocity: Vec3) {
    let ctx = script_ctx(app);
    let mut registry = ctx.registry.borrow_mut();
    let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
    transform.position = position;
    transform.rotation = Quat::from_rotation_y(yaw);
    registry.set_component(pawn, transform).unwrap();
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    movement.velocity = velocity;
    registry.set_component(pawn, movement).unwrap();
}

/// A `player`-class pawn bound to `seat` at `position`, facing `yaw`, with
/// health, as a client's participation entry leaves one.
pub(super) fn spawn_seat_pawn(app: &mut App, seat: Seat, position: Vec3, yaw: f32) -> EntityId {
    let ctx = script_ctx(app);
    let movement = PlayerMovementComponent::from_descriptor(
        descriptors()[0]
            .movement
            .as_ref()
            .expect("the player class moves"),
    );
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform {
        position,
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    });
    registry.set_component(pawn, movement).unwrap();
    registry
        .set_component(
            pawn,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                ..Default::default()
            }),
        )
        .unwrap();
    let session = app.session.as_mut().unwrap();
    session
        .seat_table
        .as_mut()
        .unwrap()
        .bind_pawn(&mut registry, seat, pawn);
    pawn
}

/// The installed seam stage runs one trigger tick for every movement pawn,
/// as the host's tick does: a pawn inside an armed touch seam activates it.
pub(super) fn trigger_tick(app: &mut App) {
    let ctx = script_ctx(app);
    let registry = ctx.registry.borrow();
    let players: Vec<AuthoritativePlayer> = registry
        .query_by_component_and_tag(postretro_entities::ComponentKind::PlayerMovement, None)
        .map(|(pawn, _)| AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        })
        .collect();
    let capabilities =
        postretro_sim::sim::player_life::TriggerStageCapabilities::capture(&registry, &players);
    let _ = app
        .seam_stage
        .run_tick(&registry, &players, &capabilities, &HashMap::new());
}

/// Where `motion` arrives through the destination door's landmark.
pub(super) fn destination_body(motion: &LandmarkRelative) -> Vec3 {
    motion.body_point(Landmark {
        origin: DESTINATION_LANDMARK,
        yaw: DESTINATION_YAW,
    })
}

/// The camera's eye, yaw and pitch.
pub(super) fn camera_pose(app: &App) -> (Vec3, f32, f32) {
    (app.camera.position, app.camera.yaw, app.camera.pitch)
}

/// The eye of a `player` pawn whose body stands at `body`.
pub(super) fn player_eye(body: Vec3) -> Vec3 {
    body + Vec3::Y
        * descriptors()[0]
            .movement
            .as_ref()
            .unwrap()
            .capsule
            .eye_height
}

/// Queue the request an activated seam `seam` targeting `target` makes, as
/// the frame-end pick does.
pub(super) fn request_seam(app: &mut App, target: &str, seam: &str) {
    app.enqueue_level_request(crate::startup::LevelRequest::Seam {
        catalog_id: target.into(),
        seam: seam.into(),
    });
}

/// `world` with a wall standing through `point`: a capsule there does not fit.
pub(super) fn wall_at(
    mut world: postretro_level_loader::LevelWorld,
    point: Vec3,
) -> postretro_level_loader::LevelWorld {
    let base = world.vertices.len() as u32;
    for corner in [
        point + Vec3::new(-3.0, -3.0, 0.0),
        point + Vec3::new(3.0, -3.0, 0.0),
        point + Vec3::new(0.0, 4.0, 0.0),
    ] {
        world
            .vertices
            .push(postretro_render_data::geometry::WorldVertex {
                position: corner.to_array(),
                base_uv: [0.0, 0.0],
                normal_oct: [0, 0],
                tangent_packed: [0, 0],
                lightmap_uv: [0, 0],
                lightmap_block: 0,
                animated_block: 0,
            });
    }
    world.indices.extend([base, base + 1, base + 2]);
    world
}
