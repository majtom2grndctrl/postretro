//! Level install tests: §3 order, rejection before mutation, tags, spawners,
//! carried lights, trigger pools and client suppression.
//! See: context/lib/boot_sequence.md §3

use super::lifecycle_install_world::resolve_carried_light_bindings_after_mover_spawn;
use super::lifecycle_spawners::SpawnerInstallDiagnostics;
use super::lifecycle_test_fixtures::*;
use super::*;
use crate::startup::LevelRequest;
use std::collections::BTreeMap;

use postretro_entities::Transform;
use postretro_entities::{EntityId, TriggerPoolArm, TriggerPoolDescriptor, TriggerVolumeComponent};

use postretro_level_format::trigger_volumes::TriggerVolumeRecord;

/// Connected-client trigger-pool install result exposed only to cross-subsystem
/// tests. The registry is the one populated by [`install_world_cpu`].
pub(crate) struct ConnectedClientTriggerPoolInstallFixture {
    pub(crate) registry: postretro_entities::EntityRegistry,
    pub(crate) trap: postretro_entities::EntityId,
    pub(crate) report: TriggerPoolInstallReport,
}

#[test]
fn spawner_install_resolves_only_ai_descriptors_and_replaces_prior_level_data() {
    use crate::scripting::builtins::data_archetype_test_fixtures::{
        behavior_enemy_descriptor, mesh_descriptor,
    };
    use crate::scripting::builtins::entity_spawner;
    use crate::scripting::map_entity::MapEntity;
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut registry = postretro_entities::EntityRegistry::new();
    let map_entity = |archetype: &str| MapEntity {
        map_ordinal: None,
        classname: entity_spawner::CLASSNAME.to_string(),
        origin: Vec3::ZERO,
        angles: Vec3::ZERO,
        key_values: [
            ("archetype".to_string(), archetype.to_string()),
            ("count".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect(),
        tags: vec![],
    };
    let enemy = entity_spawner::handle(&map_entity("cultist"), &mut registry).unwrap();
    let non_ai = entity_spawner::handle(&map_entity("crate"), &mut registry).unwrap();
    let missing = entity_spawner::handle(&map_entity("absent"), &mut registry).unwrap();
    let context = crate::spawner::SpawnContext::default();
    context.replace_level_data(std::collections::HashMap::new(), None);

    let params = postretro_foundation::NavAgentParams {
        radius: 0.4,
        height: 2.0,
        step_height: 0.45,
        max_slope_deg: 50.0,
    };
    let diagnostics = resolve_spawners_for_level(
        &mut registry,
        &[
            behavior_enemy_descriptor("cultist"),
            mesh_descriptor("crate", false),
        ],
        Some(params),
        &context,
    );

    assert!(
        registry
            .get_component::<SpawnerComponent>(enemy)
            .unwrap()
            .resolved
    );
    assert!(
        !registry
            .get_component::<SpawnerComponent>(non_ai)
            .unwrap()
            .resolved
    );
    assert!(
        !registry
            .get_component::<SpawnerComponent>(missing)
            .unwrap()
            .resolved
    );
    assert_eq!(
        diagnostics,
        SpawnerInstallDiagnostics {
            missing_archetype: 1,
            non_ai_archetype: 1,
        }
    );
    let descriptors = context.resolved_enemy_descriptors_for_test();
    assert_eq!(descriptors.len(), 1);
    assert!(descriptors.contains_key("cultist"));
    assert_eq!(context.agent_params_for_test(), Some(params));
    drop(descriptors);

    // A new level drops stale descriptor entries and warning dedup state.
    resolve_spawners_for_level(&mut registry, &[], None, &context);
    assert!(context.resolved_enemy_descriptors_for_test().is_empty());
}

#[test]
fn resolved_spawner_mesh_models_include_only_renderable_resolved_archetypes() {
    use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut registry = postretro_entities::EntityRegistry::new();
    let add_spawner =
        |registry: &mut postretro_entities::EntityRegistry, archetype: &str, resolved: bool| {
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    SpawnerComponent {
                        archetype_name: archetype.to_string(),
                        count: 1,
                        spawned_tags: Vec::new(),
                        resolved,
                    },
                )
                .unwrap();
        };
    add_spawner(&mut registry, "spawner_only", true);
    add_spawner(&mut registry, "spawner_only", true);
    add_spawner(&mut registry, "unresolved", false);
    add_spawner(&mut registry, "non_mesh", true);
    add_spawner(&mut registry, "absent", true);

    let mut spawner_only = behavior_enemy_descriptor("spawner_only");
    spawner_only.mesh.as_mut().unwrap().model = "models/spawner_only.gltf".to_string();
    spawner_only.mesh.as_mut().unwrap().attachments =
        [("hand".to_string(), "models/spawner_prop.gltf".to_string())]
            .into_iter()
            .collect();
    let mut non_mesh = behavior_enemy_descriptor("non_mesh");
    non_mesh.mesh = None;

    let models = resolved_spawner_mesh_models(&registry, &[spawner_only, non_mesh]);
    assert_eq!(
        models,
        vec![
            "models/spawner_only.gltf".to_string(),
            "models/spawner_prop.gltf".to_string(),
        ]
    );
    assert!(crate::distinct_mesh_models(&registry).is_empty());
}

#[test]
fn windowed_carrier_binding_reresolves_before_first_bridge_update_after_reload() {
    use postretro_entities::components::light::LightComponent;
    use postretro_level_loader::{KinematicGeometry, LoadedKinematicMover, LoadedMemberLight};

    fn geometry() -> KinematicGeometry {
        KinematicGeometry {
            movers: vec![LoadedKinematicMover {
                mover_id: 71,
                name: "reload_lift".to_string(),
                tags: Vec::new(),
                origin: Vec3::ZERO,
                path: "reload_lift_start".to_string(),
                speed_mps: 1.0,
                wait_ms: 0.0,
                move_mode: 0,
                start_on_spawn: false,
                vertices: Vec::new(),
                indices: Vec::new(),
                face_meta: Vec::new(),
                spin_axis: Vec3::ZERO,
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".to_string(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: None,
                open_event: None,
                close_event: None,
                blocked_event: None,
                crush_event: None,
                open_sound: None,
                close_sound: None,
                blocked_sound: None,
                crush_sound: None,
                sealed_portal_ids: Vec::new(),
                carried_lights: vec![LoadedMemberLight {
                    alpha_light_index: 0,
                    local_offset: Vec3::new(2.0, 0.0, 0.0),
                }],
            }],
            waypoints: Vec::new(),
        }
    }

    let mut carried_light = map_light("e22_carried", [2.0, 0.0, 0.0]);
    carried_light.is_dynamic = true;

    // This is the same windowed half of install_level_payload: the bridge
    // creates light entities first, mover spawn completes, then the pass
    // binds before LightBridge::update gets a frame.
    let first_geometry = geometry();
    let mut first_registry = postretro_entities::EntityRegistry::new();
    let mut first_bridge = crate::scripting_systems::light_bridge::LightBridge::new();
    first_bridge.populate_from_level(&[carried_light.clone()], &mut first_registry, 0);
    let first_mover = first_registry.spawn(Transform {
        position: Vec3::new(10.0, 0.0, 0.0),
        ..Transform::default()
    });
    resolve_carried_light_bindings_after_mover_spawn(
        &first_geometry,
        &[first_mover],
        &first_bridge,
        &mut first_registry,
    );
    let first_light = first_bridge.entity_for_map_index(0).unwrap();
    assert_eq!(
        first_registry
            .get_component::<LightComponent>(first_light)
            .unwrap()
            .carrier
            .as_ref()
            .unwrap()
            .mover_entity,
        first_mover,
        "the first windowed install binds before its first bridge frame"
    );
    assert!(
        first_bridge.update(&mut first_registry, 0.0, 0.0).is_some(),
        "the first bridge update receives the already-bound carrier"
    );

    // Level unload clears bridge + registry. Make the replacement mover
    // occupy a distinct raw id so this asserts an actual re-resolution,
    // rather than accidentally passing because an allocator reused it.
    let reloaded_geometry = geometry();
    let mut reloaded_registry = postretro_entities::EntityRegistry::new();
    let mut reloaded_bridge = crate::scripting_systems::light_bridge::LightBridge::new();
    reloaded_bridge.populate_from_level(&[carried_light], &mut reloaded_registry, 0);
    let _unrelated = reloaded_registry.spawn(Transform::default());
    let reloaded_mover = reloaded_registry.spawn(Transform {
        position: Vec3::new(30.0, 0.0, 0.0),
        ..Transform::default()
    });
    assert_ne!(first_mover, reloaded_mover, "fixture needs fresh mover ids");
    resolve_carried_light_bindings_after_mover_spawn(
        &reloaded_geometry,
        &[reloaded_mover],
        &reloaded_bridge,
        &mut reloaded_registry,
    );

    let reloaded_light = reloaded_bridge.entity_for_map_index(0).unwrap();
    assert_eq!(
        reloaded_registry
            .get_component::<LightComponent>(reloaded_light)
            .unwrap()
            .carrier
            .as_ref()
            .unwrap()
            .mover_entity,
        reloaded_mover,
        "reload must not retain the first level's raw mover entity id"
    );
    assert!(
        reloaded_bridge
            .update(&mut reloaded_registry, 0.0, 0.0)
            .is_some(),
        "the replacement carrier is also present before its first bridge update"
    );
    let reloaded_position = Vec3::from_array(
        reloaded_bridge.collect_all_as_map_lights(&reloaded_registry, 0.0)[0]
            .0
            .origin
            .map(|value| value as f32),
    );
    assert!(
        reloaded_position.distance(Vec3::new(32.0, 0.0, 0.0)) <= 1.0e-6,
        "the reloaded bridge composes the fresh mover pose, not the authored origin"
    );
}

fn pool_descriptor(tag: &str, arm: TriggerPoolArm, levels: &[&str]) -> TriggerPoolDescriptor {
    TriggerPoolDescriptor {
        tag: tag.to_string(),
        arm,
        levels: levels.iter().map(|level| (*level).to_string()).collect(),
    }
}

fn trap_pool_trigger_record(
    name: &str,
    tag: &str,
    index: usize,
    enabled_on_spawn: bool,
) -> TriggerVolumeRecord {
    let x = index as f32 * 4.0;
    TriggerVolumeRecord {
        name: name.to_string(),
        tags: vec![tag.to_string()],
        aabb_min: [x, 0.0, 0.0],
        aabb_max: [x + 1.0, 1.0, 1.0],
        activation: 0,
        target_tag: String::new(),
        command: 0,
        command_arg: String::new(),
        fire_mode: 0,
        rearm_ms: 0.0,
        enabled_on_spawn,
        on_fire: String::new(),
        on_exit: String::new(),
    }
}

fn trap_pool_fixture_world() -> postretro_level_loader::LevelWorld {
    let mut world = level_world("trap_pools", 1);
    world.trigger_volumes = (0..4)
        .map(|index| {
            trap_pool_trigger_record(&format!("closet-{index}"), "closet_trap", index, true)
        })
        .chain((0..4).map(|index| {
            trap_pool_trigger_record(&format!("ambush-{index}"), "ambush_trap", index + 4, false)
        }))
        .collect();
    world
}

struct TrapPoolFixtureInstall {
    report: TriggerPoolInstallReport,
    armed_by_tag: BTreeMap<String, Vec<EntityId>>,
}

fn install_trigger_pool_world(
    world: postretro_level_loader::LevelWorld,
    policy: TriggerPoolSeedPolicy,
    active_level_tags: &[&str],
    global_pools: Vec<TriggerPoolDescriptor>,
    suppress_host_replicated: bool,
) -> TriggerPoolWorldInstall {
    install_descriptor_world(
        world,
        policy,
        active_level_tags,
        global_pools,
        suppress_host_replicated,
        Vec::new(),
    )
}

#[test]
fn authored_map_ordinals_survive_player_partition_and_client_suppression() {
    use crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor;
    use postretro_entities::components::map_ordinal::MapOrdinal;
    use postretro_entities::{ComponentKind, ComponentValue};
    use postretro_level_format::map_entity::MapEntityRecord;
    let mut prop = mesh_descriptor("network_prop", false);
    prop.mesh.as_mut().unwrap().model =
        "models/decraniated_low_poly_retro_pixel/scene.gltf".to_string();
    let mut local_light = prop.clone();
    local_light.canonical_name = Some("local_light".into());
    local_light.mesh = None;
    local_light.light = Some(postretro_entities::LightDescriptor {
        color: [1.0; 3],
        intensity: 1.0,
        range: 8.0,
        is_dynamic: true,
    });
    for connected in [false, true] {
        let mut world = level_world("ordinal_install", 1);
        world.map_entities = ["player_spawn", "network_prop", "local_light"]
            .into_iter()
            .enumerate()
            .map(|(index, classname)| MapEntityRecord {
                classname: classname.to_string(),
                origin: [index as f32 * 3.0, 0.0, 0.0],
                tags: vec![classname.to_string()],
                ..Default::default()
            })
            .collect();
        let installed = install_descriptor_world(
            world,
            TriggerPoolSeedPolicy::ArmAll,
            &[],
            Vec::new(),
            connected,
            vec![prop.clone(), local_light.clone()],
        );
        let ordinals: Vec<_> = installed
            .registry
            .iter_with_kind(ComponentKind::MapOrdinal)
            .filter_map(|(id, value)| match value {
                ComponentValue::MapOrdinal(MapOrdinal(index)) if *index > 0 => Some((id, *index)),
                _ => None,
            })
            .collect();
        assert!(
            ordinals.iter().any(|(_, ordinal)| *ordinal == 2),
            "client-local descriptor retains PRL ordinal after partition and filtering"
        );
        assert_eq!(
            ordinals.iter().any(|(_, ordinal)| *ordinal == 1),
            !connected,
            "networked descriptor has its original ordinal only on the host"
        );
    }
}

#[test]
fn suppressed_descriptor_models_install_hit_zones_once() {
    use crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor;
    use postretro_entities::ComponentKind;
    use postretro_entities::provenance::DescriptorProvenance;
    use postretro_level_format::map_entity::MapEntityRecord;
    use postretro_scripting_core::data_descriptors::HealthDescriptor;

    let model = "models/decraniated_low_poly_retro_pixel/scene.gltf";
    let mut prop = mesh_descriptor("network_prop", false);
    prop.mesh.as_mut().unwrap().model = model.to_string();
    let mut dummy = prop.clone();
    dummy.canonical_name = Some("network_dummy".to_string());
    dummy.health = Some(HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: Default::default(),
    });
    for connected in [false, true] {
        let mut world = level_world("descriptor_install", 1);
        world.map_entities = ["network_prop", "network_dummy", "network_dummy"]
            .into_iter()
            .enumerate()
            .map(|(i, classname)| MapEntityRecord {
                classname: classname.to_string(),
                origin: [i as f32 * 3.0, 0.0, 0.0],
                ..Default::default()
            })
            .collect();

        let installed = install_descriptor_world(
            world,
            TriggerPoolSeedPolicy::ArmAll,
            &[],
            Vec::new(),
            connected,
            vec![prop.clone(), dummy.clone()],
        );
        let placements = installed.registry.iter_with_kind(ComponentKind::DescriptorProvenance)
                .filter(|(_, value)| matches!(value, postretro_entities::ComponentValue::DescriptorProvenance(DescriptorProvenance { canonical_name, .. }) if canonical_name.starts_with("network_")))
                .count();
        assert_eq!(placements, if connected { 0 } else { 3 });
        assert_eq!(
            installed.upload_models,
            [model],
            "shared suppressed models upload once"
        );
        let zones = installed
            .hit_zone_store
            .get_by_name(model)
            .expect("suppressed model retains CPU hit zones");
        assert!(zones.joint_zones.iter().any(Option::is_some));
        assert!(zones.derived_bound.is_some());
    }
}

/// Run the real renderer-free installation seam with a policy pinned on
/// `WorldInstallHandles`, never through argv. The synthetic records mirror
/// the authored fixture's two four-member pools while keeping this QA gate
/// free of a cold PRL bake.
fn install_trap_pool_fixture(
    policy: TriggerPoolSeedPolicy,
    active_level_tags: &[&str],
    global_pools: Vec<TriggerPoolDescriptor>,
    suppress_host_replicated: bool,
) -> TrapPoolFixtureInstall {
    let installed = install_trigger_pool_world(
        trap_pool_fixture_world(),
        policy,
        active_level_tags,
        global_pools,
        suppress_host_replicated,
    );
    let registry = &installed.registry;
    let armed_by_tag = ["closet_trap", "ambush_trap"]
        .into_iter()
        .map(|tag| {
            let mut armed: Vec<EntityId> = registry
                .query_by_component_and_tag(
                    postretro_entities::ComponentKind::TriggerVolume,
                    Some(tag),
                )
                .filter_map(|(id, _)| {
                    registry
                        .get_component::<TriggerVolumeComponent>(id)
                        .is_ok_and(|trigger| trigger.armed)
                        .then_some(id)
                })
                .collect();
            armed.sort_unstable();
            (tag.to_string(), armed)
        })
        .collect();
    TrapPoolFixtureInstall {
        report: installed.report,
        armed_by_tag,
    }
}

pub(super) fn install_connected_client_trigger_pool_fixture()
-> ConnectedClientTriggerPoolInstallFixture {
    let mut world = level_world("connected_client_trap_pool", 1);
    let mut trap = trap_pool_trigger_record("network-trap", "trap-pool", 0, false);
    trap.on_fire = "trapPools.spawn".to_string();
    world.trigger_volumes = vec![trap];
    let installed = install_trigger_pool_world(
        world,
        TriggerPoolSeedPolicy::Seeded(17),
        &[],
        vec![pool_descriptor("trap-pool", TriggerPoolArm::Count(1), &[])],
        true,
    );
    let trap = installed
        .registry
        .query_by_component_and_tag(
            postretro_entities::ComponentKind::TriggerVolume,
            Some("trap-pool"),
        )
        .map(|(id, _)| id)
        .next()
        .expect("real client install materializes the authored trap");

    ConnectedClientTriggerPoolInstallFixture {
        registry: installed.registry,
        trap,
        report: installed.report,
    }
}

// The connected-client install policy belongs to binary lifecycle
// coverage: netcode receives the resulting registry but must not depend on
// this App-side installation fixture.
#[test]
fn connected_client_trigger_pool_install_keeps_authored_trigger_unarmed() {
    let installed = install_connected_client_trigger_pool_fixture();
    assert_eq!(installed.report, TriggerPoolInstallReport::default());
    assert!(
        !installed
            .registry
            .get_component::<TriggerVolumeComponent>(installed.trap)
            .expect("client trap remains live")
            .armed,
        "the connected client does not rerun the host pool roll",
    );
}

#[test]
fn install_range_rejection_precedes_all_level_state_mutations() {
    let mut app = test_app();
    app.level = Some(level_world("old", 1));
    app.active_level_tags = vec!["old-level".to_string()];
    app.script_time = 17.0;
    app.nav_graph = Some(crate::nav::NavGraph::from_section(
        &postretro_level_format::navmesh::NavMeshSection {
            version: postretro_level_format::navmesh::NAVMESH_VERSION,
            origin: [0.0; 3],
            cell_size: 1.0,
            dim_x: 8,
            dim_z: 8,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![],
            portals: vec![],
        },
    ));
    let before_slots: BTreeMap<_, _> = slot_snapshot(&app)
        .into_iter()
        .map(|(key, record)| (key, record.value))
        .collect();
    let mut rejected = level_world("rejected", 2);
    rejected
        .bvh
        .leaves
        .push(postretro_render_data::geometry::BvhLeaf {
            aabb_min: [0.0; 3],
            aabb_max: [1.0; 3],
            material_bucket_id: 0,
            index_offset: 6,
            index_count: 3,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        });
    let error = app
        .install_level_payload(rejected, PathBuf::from("baked"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "BVH leaf 0 index range [6..9) exceeds Geometry index count 6"
    );
    assert_eq!(app.level.as_ref().unwrap().indices.len(), 3);
    assert_eq!(app.active_level_tags, ["old-level"]);
    assert!((app.script_time - 17.0).abs() < f64::EPSILON);
    assert_eq!(app.nav_graph.as_ref().unwrap().grid().dim_x, 8);
    let after_slots: BTreeMap<_, _> = slot_snapshot(&app)
        .into_iter()
        .map(|(key, record)| (key, record.value))
        .collect();
    assert_eq!(after_slots, before_slots);
    assert!(app.renderer.is_none());

    // The commit path: the range and collision checks ran in prep, so what
    // can still reject a bundle is a commit check (here, its set no longer
    // resident), which also runs before any mutation.
    let case = super::lifecycle_commit_tests::CommitCase::textured();
    let Some(mut app) = super::lifecycle_commit_tests::gpu_app(
        "install_range_rejection_precedes_all_level_state_mutations",
        &case.content_root,
    ) else {
        return;
    };
    app.level = Some(level_world("old", 1));
    app.active_level_tags = vec!["old-level".to_string()];
    app.script_time = 17.0;
    app.nav_graph = Some(crate::nav::NavGraph::from_section(
        &postretro_level_format::navmesh::NavMeshSection {
            version: postretro_level_format::navmesh::NAVMESH_VERSION,
            origin: [0.0; 3],
            cell_size: 1.0,
            dim_x: 8,
            dim_z: 8,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![],
            portals: vec![],
        },
    ));
    let before_slots: BTreeMap<_, _> = slot_snapshot(&app)
        .into_iter()
        .map(|(key, record)| (key, record.value))
        .collect();
    let collision = app.collision_world.triangle_count();
    let pool = crate::level_load::test_support::test_pool();
    let (stages, bundle) = super::lifecycle_commit_tests::load_case(&mut app, &pool, &case, |_| {});
    let crate::level_load::BundleSet::Resident(set) = bundle.set else {
        panic!("a load with a renderer builds a set");
    };
    assert!(app.renderer.as_mut().unwrap().release_level_set(set));
    let failure = app
        .commit_level_bundle(bundle)
        .expect_err("a bundle whose set is gone is refused");
    assert_eq!(
        failure.stage,
        crate::level_load::LoadStage::Commit,
        "{failure}"
    );
    assert_eq!(app.level.as_ref().unwrap().indices.len(), 3);
    assert_eq!(app.active_level_tags, ["old-level"]);
    assert!((app.script_time - 17.0).abs() < f64::EPSILON);
    assert_eq!(app.nav_graph.as_ref().unwrap().grid().dim_x, 8);
    assert_eq!(app.collision_world.triangle_count(), collision);
    assert!(app.environment_volumes.is_empty());
    assert!(app.renderer.as_ref().unwrap().active_level_set().is_none());
    let after_slots: BTreeMap<_, _> = slot_snapshot(&app)
        .into_iter()
        .map(|(key, record)| (key, record.value))
        .collect();
    assert_eq!(after_slots, before_slots);
    crate::level_load::test_support::retire_app_load(stages, &mut app);
}

#[test]
fn install_range_errors_use_the_existing_boot_and_runtime_failure_routes() {
    let finish =
        method_source(include_str!("lifecycle_finish.rs"), "finish_level_payload").unwrap();
    let rejection = finish
        .split("if let Err(err) = self.install_level_payload(")
        .nth(1)
        .unwrap()
        .split("// The level's streaming sessions exist")
        .next()
        .unwrap();
    assert!(rejection.contains("self.finish_level_failure(err.to_string(), event_loop);"));
    assert!(rejection.contains("return;"));
    let failure = method_source(include_str!("lifecycle.rs"), "finish_level_failure").unwrap();
    let boot = failure
        .split("if was_boot_load {")
        .nth(1)
        .unwrap()
        .split("return;")
        .next()
        .unwrap();
    assert!(boot.contains("self.exit_result = Err("));
    assert!(boot.contains("event_loop.exit();"));
    assert!(failure.contains("self.boot_state = BootState::Frontend;"));
    // The sequencer with every stage it calls spliced in at its call.
    let install = install_level_payload_source();
    let check = install
        .find("render::validate_level_geometry_ranges(")
        .unwrap();
    let collision = install
        .find("crate::collision::CollisionWorld::from_level(")
        .unwrap();
    for mutation in [
        "self.view_feel_state =",
        "endpoint.set_level_parity(",
        "self.nav_graph =",
        "renderer.install_textures(",
        "renderer.install_level_geometry(",
        "self.collision_world =",
    ] {
        let at = install.find(mutation).unwrap();
        assert!(check < at, "range check must precede {mutation}");
        assert!(
            collision < at,
            "static collision build must precede {mutation}"
        );
    }
    // The commit, spliced the same way: its checks (the range and collision
    // checks ran in prep) precede every mutation, and a failed check returns
    // before any.
    let commit = commit_level_bundle_source();
    let check = commit.find("self.check_level_commit(").unwrap();
    let failed = commit.find("return Err(failure);").unwrap();
    assert!(check < failed);
    for mutation in [
        "self.view_feel_state =",
        "endpoint.set_level_parity(",
        "self.nav_graph =",
        "renderer.publish_level_set(",
        "self.collision_world =",
    ] {
        let at = commit.find(mutation).unwrap();
        assert!(failed < at, "the commit's checks must precede {mutation}");
    }
    let capture = include_str!("../capture/prepared.rs");
    let check = capture
        .find("crate::render::validate_level_geometry_ranges(")
        .unwrap();
    for mutation in [
        "Renderer::new_offscreen(",
        "renderer.install_textures(",
        "renderer.install_level_geometry(",
    ] {
        assert!(
            check < capture.find(mutation).unwrap(),
            "capture check must precede {mutation}"
        );
    }
}

// Pin: no-upload-install. Payloads leave the world only into an upload;
// an install with no renderer uploads nothing, so the world keeps them.
#[test]
fn install_without_renderer_keeps_gpu_lighting_payloads_in_the_world() {
    use postretro_level_format::lightmap::{
        IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapBlockPayload, LightmapMode,
        LightmapSection,
    };
    use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

    let mut app = test_app();
    assert!(app.renderer.is_none());
    let mut world = level_world("payloads", 1);
    let lightmap = LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: LightmapMode::Shadowed,
        blocks: vec![LightmapBlock {
            cell_id: 0,
            width: 4,
            height: 4,
            irradiance: vec![1; 128],
            direction: vec![2; 8],
        }],
    };
    world.lightmap = Some(lightmap.index());
    world.shadowmask_atlas = Some(
        ShadowmaskAtlasSection {
            owners: Vec::new(),
            blocks: vec![[vec![3; 16], vec![4; 16]]],
        }
        .index(),
    );
    let payloads = postretro_level_loader::GpuLightingPayloads {
        blocks: vec![LightmapBlockPayload {
            irradiance: vec![1; 128],
            direction: vec![2; 8],
            shadowmask: Some([vec![3; 16], vec![4; 16]]),
        }],
    };
    world.gpu_lighting_payloads = postretro_level_loader::GpuLightingPayloads {
        blocks: payloads.blocks.clone(),
    };

    app.install_level_payload(world, PathBuf::from("baked"))
        .unwrap();

    let installed = app
        .level
        .as_ref()
        .expect("the world is stashed without a renderer");
    assert_eq!(installed.gpu_lighting_payloads, payloads);
    assert!(installed.lightmap.is_some() && installed.shadowmask_atlas.is_some());
}

#[test]
fn catalog_level_install_retains_active_tags_from_in_flight_load() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.content_root = PathBuf::from("content/mod");
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map(
            "e1m1",
            "maps/e1m1.prl",
            "Entryway",
            &["campaign", "intro"],
        )]);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())));
    app.drain_level_requests();
    app.retain_active_level_tags_for_install();

    assert_eq!(app.active_level_tags, ["campaign", "intro"]);
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Catalog("e1m1".to_string()))
    );

    drop_in_flight_load(&mut app);
}

#[test]
fn raw_path_level_install_retains_empty_active_tags() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.content_root = PathBuf::from("content/dev");

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
        "content/dev/maps/raw-dev-map.prl",
    ))));
    app.drain_level_requests();
    app.retain_active_level_tags_for_install();

    assert!(app.active_level_tags.is_empty());
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Path(PathBuf::from(
            "content/dev/maps/raw-dev-map.prl"
        )))
    );

    drop_in_flight_load(&mut app);
}

// The install path keys host-authoritative placement suppression off
// `is_connected_client()`. Prove the role gate resolves correctly for each role:
// only the connected client suppresses map placements.
#[test]
fn host_replicated_placement_suppression_gate_is_connected_client_only() {
    use crate::netcode::{NetEndpoint, NetRole};

    // Single-player: net inert, no suppression.
    let mut app = test_app();
    app.session.as_mut().unwrap().net_endpoint = None;
    assert!(
        !app.is_connected_client(),
        "single-player must keep map-placed AI enemies (no suppression)"
    );

    // Listen host: authoritative, keeps every placement and replicates them.
    app.session.as_mut().unwrap().net_endpoint = Some(
        NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint"),
    );
    assert!(
        !app.is_connected_client(),
        "listen host must keep map-placed AI enemies (it owns + replicates them)"
    );

    // Connected client: the only role that suppresses the local spawn.
    connect_test_app_as_client(&mut app);
    assert!(
        app.is_connected_client(),
        "connected client must suppress local authoritative AI-enemy spawns"
    );
}

#[test]
fn windowed_install_assigns_light_entity_ids_before_fog_entity_ids() {
    // Drift guard for segment B's fog-after-preexisting-lights behavior: given
    // light entities already registered (as the windowed caller does BETWEEN
    // segment A and segment B), segment B's fog-volume populate must land its
    // ids after them — both bridges key dirty tracking on `EntityId`. This test
    // reconstructs that sequence directly rather than driving
    // `install_level_payload`, so it does NOT guard the real caller's call
    // order — a future reorder there would not fail this test.
    let mut app = test_app();
    let ctx = script_ctx(&app);

    let mut world = level_world(FIXTURE_MAP_A, 1);
    world.fog_volumes = vec![fog_record("fog", 0.0), fog_record("fog", 4.0)];

    // Windowed step run between segments A and B: light entities take registry
    // ids first.
    let lights = vec![
        map_light("light", [0.0, 2.0, 0.0]),
        map_light("light", [4.0, 2.0, 0.0]),
    ];
    {
        let session = app.session.as_mut().expect("test app session installed");
        session
            .light_bridge
            .populate_from_level(&lights, &mut ctx.registry.borrow_mut(), 0);
    }

    // Segment B creates the fog-volume entities first thing, after the light
    // populate above — mirroring the windowed call order, which commits
    // the static collision world before segment B.
    app.collision_world = crate::collision::CollisionWorld::from_level(&world)
        .expect("fixture collision mesh is valid");
    let mut timings = StartupTimings::new();
    {
        let session = app.session.as_mut().expect("test app session installed");
        let handles = WorldInstallHandles {
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: Default::default(),
            world: &world,
            script_ctx: &ctx,
            content_root: std::path::Path::new("content/dev"),
            active_level_tags: &[],
            nav_graph: None,
            fog_volume_bridge: &mut session.fog_volume_bridge,
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
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            trigger_pool_policy: TriggerPoolSeedPolicy::ArmAll,
            suppress_host_replicated: false,
            suppress_boot_pawn: false,
            local_carried_loadout: None,
        };
        // No-op mesh hook: headless-shaped, no renderer to upload models.
        let _ = install_world_cpu(
            handles,
            &mut timings,
            |_models, _clip_tables| {
                crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
            },
            |_first_player| {},
        );
    }

    // Fresh registry (no despawns): `to_raw()` low bits are the allocation
    // index, so id order is creation order.
    let registry = ctx.registry.borrow();
    let max_light_id = registry
        .iter_with_kind(postretro_entities::ComponentKind::Light)
        .map(|(id, _)| id.to_raw())
        .max()
        .expect("light entities were populated");
    let min_fog_id = registry
        .iter_with_kind(postretro_entities::ComponentKind::FogVolume)
        .map(|(id, _)| id.to_raw())
        .min()
        .expect("fog entities were populated");
    assert!(
        max_light_id < min_fog_id,
        "light entity ids (max {max_light_id}) must precede fog entity ids (min {min_fog_id})",
    );
    drop(registry);

    // The real callers' call order, in source: on both the main-thread
    // install and the commit, the light bridge populates before segment B
    // (whose installer creates the fog entities first thing).
    for (path, source) in [
        ("install_level_payload", install_level_payload_source()),
        ("commit_level_bundle", commit_level_bundle_source()),
    ] {
        let lights = source
            .find("self.install_level_light_bridge(")
            .unwrap_or_else(|| panic!("{path} populates the light bridge"));
        let fog = source
            .find("install_world_cpu_with_models(")
            .unwrap_or_else(|| panic!("{path} runs segment B"));
        assert!(lights < fog, "{path}: light ids before fog ids");
    }

    // And driven for real, a level with map lights and fog volumes through
    // the main-thread install and through the commit.
    let mut case = super::lifecycle_commit_tests::CommitCase::textured();
    case.edit_world = Some(super::lifecycle_commit_tests::add_entities);
    let test = "windowed_install_assigns_light_entity_ids_before_fog_entity_ids";
    let Some(mut oracle) = super::lifecycle_commit_tests::gpu_app(test, &case.content_root) else {
        return;
    };
    super::lifecycle_commit_tests::oracle_case(&mut oracle, &case);
    let pool = crate::level_load::test_support::test_pool();
    let mut committed = super::lifecycle_commit_tests::gpu_app(test, &case.content_root)
        .expect("an adapter was found once");
    super::lifecycle_commit_tests::commit_case(&mut committed, &pool, &case);
    for (path, app) in [("oracle", &oracle), ("commit", &committed)] {
        let (max_light_id, min_fog_id) = super::lifecycle_commit_tests::light_and_fog_ids(app);
        assert!(
            max_light_id < min_fog_id,
            "{path}: light entity ids (max {max_light_id}) must precede fog entity ids (min {min_fog_id})",
        );
    }
}

#[test]
fn pinned_trigger_pool_install_selects_exact_counts_and_restarts_identically() {
    let pools = vec![
        pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
        pool_descriptor(
            "ambush_trap",
            TriggerPoolArm::Percentage(50.0),
            &["trap-pools"],
        ),
    ];

    let first = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::Seeded(17),
        &["trap-pools"],
        pools.clone(),
        false,
    );
    let restarted = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::Seeded(17),
        &["trap-pools"],
        pools.clone(),
        false,
    );
    let different_seed = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::Seeded(18),
        &["trap-pools"],
        pools,
        false,
    );

    assert_eq!(first.report.seed, Some(17));
    assert_eq!(
        first
            .report
            .pools
            .iter()
            .map(|pool| (pool.tag.as_str(), pool.members.len(), pool.selected.len()))
            .collect::<Vec<_>>(),
        [("closet_trap", 4, 2), ("ambush_trap", 4, 2)],
        "count and percentage pools must resolve against their actual installed members",
    );
    assert_eq!(first.armed_by_tag["closet_trap"].len(), 2);
    assert_eq!(first.armed_by_tag["ambush_trap"].len(), 2);
    assert_eq!(
        first.report, restarted.report,
        "the pinned policy reproduces the same armed identities on restart",
    );
    assert_ne!(
        first.report.pools[0].selected, different_seed.report.pools[0].selected,
        "the fixed distinct seeds deliberately select different closet members",
    );

    // The same pinned pools through a level load's commit, which installs
    // the triggers and rolls the pools with the windowed install's own
    // stages: the same counts, the same armed identities on restart and as
    // the main-thread install of the same level, a different roll for a
    // different seed.
    let Some(committed_first) = commit_trap_pools(17, pools_for_commit()) else {
        return;
    };
    let committed_restart = commit_trap_pools(17, pools_for_commit()).unwrap();
    let committed_other = commit_trap_pools(18, pools_for_commit()).unwrap();
    let oracle = oracle_trap_pools(17, pools_for_commit()).unwrap();
    assert_eq!(committed_first.seed, Some(17));
    assert_eq!(
        committed_first
            .pools
            .iter()
            .map(|pool| (pool.tag.as_str(), pool.members.len(), pool.selected.len()))
            .collect::<Vec<_>>(),
        [("closet_trap", 4, 2), ("ambush_trap", 4, 2)],
    );
    assert_eq!(
        committed_first, committed_restart,
        "the commit restarts identically"
    );
    assert_eq!(
        committed_first, oracle,
        "the commit rolls as the oracle install does"
    );
    assert_ne!(
        committed_first.pools[0].selected,
        committed_other.pools[0].selected
    );
}

fn pools_for_commit() -> Vec<TriggerPoolDescriptor> {
    vec![
        pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
        pool_descriptor(
            "ambush_trap",
            TriggerPoolArm::Percentage(50.0),
            &["trap-pools"],
        ),
    ]
}

fn add_trap_triggers(world: &mut postretro_level_loader::LevelWorld) {
    world.trigger_volumes = trap_pool_fixture_world().trigger_volumes;
}

/// A GPU App about to install the trap-pool level as the catalog map tagged
/// `trap-pools`, with `pools` global and the windowed pool seed pinned.
fn trap_pool_app(
    seed: u64,
    pools: Vec<TriggerPoolDescriptor>,
) -> Option<(App, super::lifecycle_commit_tests::CommitCase)> {
    let mut case = super::lifecycle_commit_tests::CommitCase::textured();
    case.edit_world = Some(add_trap_triggers);
    let mut app = super::lifecycle_commit_tests::gpu_app(
        "pinned_trigger_pool_install_selects_exact_counts_and_restarts_identically",
        &case.content_root,
    )?;
    app.session_boot_config = crate::startup::session::SessionBootConfig::with_pool_seed(seed);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_global_trigger_pools(pools);
    app.level_load = Some(crate::startup::InFlightLevelLoad {
        map_path: case.map.clone(),
        content_root: case.content_root.clone(),
        entry: crate::startup::LevelLoadEntry {
            catalog_id: Some("traps".to_string()),
            path: "maps/traps.prl".to_string(),
            name: "Traps".to_string(),
            tags: vec!["trap-pools".to_string()],
            loading_tree: Vec::new(),
        },
    });
    Some((app, case))
}

fn commit_trap_pools(
    seed: u64,
    pools: Vec<TriggerPoolDescriptor>,
) -> Option<TriggerPoolInstallReport> {
    let (mut app, case) = trap_pool_app(seed, pools)?;
    let pool = crate::level_load::test_support::test_pool();
    super::lifecycle_commit_tests::commit_case(&mut app, &pool, &case);
    Some(app.trigger_pool_report.clone())
}

fn oracle_trap_pools(
    seed: u64,
    pools: Vec<TriggerPoolDescriptor>,
) -> Option<TriggerPoolInstallReport> {
    let (mut app, case) = trap_pool_app(seed, pools)?;
    super::lifecycle_commit_tests::oracle_case(&mut app, &case);
    Some(app.trigger_pool_report.clone())
}

#[test]
fn headless_arm_all_fixture_ignores_pool_counts_and_keeps_no_seed() {
    let fixture = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::ArmAll,
        &["trap-pools"],
        vec![
            pool_descriptor("closet_trap", TriggerPoolArm::Count(0), &[]),
            pool_descriptor(
                "ambush_trap",
                TriggerPoolArm::Percentage(0.0),
                &["trap-pools"],
            ),
        ],
        false,
    );

    assert_eq!(fixture.report.seed, None);
    assert_eq!(fixture.armed_by_tag["closet_trap"].len(), 4);
    assert_eq!(fixture.armed_by_tag["ambush_trap"].len(), 4);
    assert!(
        fixture
            .report
            .pools
            .iter()
            .all(|pool| pool.members == pool.selected),
        "the headless default bypass arms every installed member without rolling",
    );
}

#[test]
fn scoped_global_trigger_pool_matches_catalog_tags_but_not_direct_prl_paths() {
    let pools = vec![
        pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
        pool_descriptor(
            "ambush_trap",
            TriggerPoolArm::Percentage(50.0),
            &["trap-pools"],
        ),
    ];
    let catalog_install = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::Seeded(17),
        &["trap-pools"],
        pools.clone(),
        false,
    );
    let direct_prl_install =
        install_trap_pool_fixture(TriggerPoolSeedPolicy::Seeded(17), &[], pools, false);

    assert_eq!(catalog_install.report.pools.len(), 2);
    assert_eq!(catalog_install.armed_by_tag["ambush_trap"].len(), 2);
    assert_eq!(
        direct_prl_install
            .report
            .pools
            .iter()
            .map(|pool| pool.tag.as_str())
            .collect::<Vec<_>>(),
        ["closet_trap"],
        "an untagged direct .prl load must not match a levels-scoped mod pool",
    );
    assert!(
        direct_prl_install.armed_by_tag["ambush_trap"].is_empty(),
        "the client-authored false state remains untouched when the scoped pool is absent",
    );
}

#[test]
fn connected_client_install_skips_pool_roll_and_preserves_authored_trigger_state() {
    let fixture = install_trap_pool_fixture(
        TriggerPoolSeedPolicy::Seeded(17),
        &["trap-pools"],
        vec![
            pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
            pool_descriptor(
                "ambush_trap",
                TriggerPoolArm::Percentage(50.0),
                &["trap-pools"],
            ),
        ],
        true,
    );

    assert!(fixture.report.pools.is_empty());
    assert_eq!(fixture.report.seed, None);
    assert_eq!(
        fixture.armed_by_tag["closet_trap"].len(),
        4,
        "the client keeps authored enabled_on_spawn=true rather than running the host roll",
    );
    assert!(
        fixture.armed_by_tag["ambush_trap"].is_empty(),
        "the client keeps authored enabled_on_spawn=false rather than running the host roll",
    );
}

// A renderer refusal of the level's resources (a streamed-SH plan or first
// pool generation the device rejects) is a rejection the load fails on,
// through the route `install_range_errors_use_the_existing_boot_and_runtime_
// failure_routes` pins, never a panic. No fixture's real data makes the
// renderer refuse, so the seam injects the refusal at the geometry install.
#[test]
fn streamed_sh_install_error_rejects_the_load_instead_of_panicking() {
    let Some(renderer) = crate::level_load::test_support::offscreen_renderer_or_skip(
        "streamed_sh_install_error_rejects_the_load_instead_of_panicking",
    ) else {
        return;
    };
    let mut app = test_app();
    app.renderer = Some(renderer);
    let (_dir, path) = crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    assert!(
        world.sh_stream_manifest().is_some(),
        "the fixture streams SH"
    );
    super::lifecycle_test_fixtures::inject_geometry_install_error(
        crate::render::LevelInstallError::StreamedSh(
            postretro_render_cpu::sh_residency::ShResidencyDrainError::SlotOverflow,
        ),
    );
    let error = app
        .install_level_payload(world, PathBuf::from("baked"))
        .unwrap_err();
    assert!(
        matches!(
            error,
            super::lifecycle_install::LevelInstallRejection::RendererInstall(_)
        ),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("cannot install streamed SH level resources"),
        "{error}"
    );
    assert!(app.level.is_none(), "a refused level is never stashed");
}
