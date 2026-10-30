//! Authored co-op fixture scripts through real mod-init and CPU installation.
//! This uses the map's entity names/tags without baking geometry; the fresh PRL
//! and visual source-producer checks remain separate P15 proofs.
use crate::startup::lifecycle::tests::{install_capture_world, level_world, script_ctx, test_app};
use postretro_entities::{
    NamedReaction, ReactionDescriptor, ScriptCtx, ScriptError, SequenceTarget,
};
use postretro_level_format::{
    data_script::DataScriptSection, fog_volumes::FogVolumeRecord, map_entity::MapEntityRecord,
    trigger_volumes::TriggerVolumeRecord,
};
use postretro_level_loader::{
    FalloffModel, LightType, LoadedKinematicMover, LoadedKinematicWaypoint, MapLight, ShadowType,
};
use postretro_scripting_core::{
    primitive_adapters::StoreSchemaJson,
    primitives_registry::{ContextScope, PrimitiveRegistry},
    runtime::{ScriptRuntime, ScriptRuntimeConfig},
};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
};

fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

// Read only top-level entity KVPs from the actual authored map. Brush planes
// are deliberately ignored: this harness does not pretend to prove compilation.
fn authored_entities() -> Vec<BTreeMap<String, String>> {
    let source =
        std::fs::read_to_string(content().join("maps/coop-presentation-test.map")).unwrap();
    let mut entities = Vec::new();
    let mut entity = BTreeMap::new();
    let mut depth = 0;
    for line in source.lines().map(str::trim) {
        if line == "{" {
            depth += 1;
        } else if line == "}" {
            depth -= 1;
            if depth == 0 {
                entities.push(std::mem::take(&mut entity));
            }
        } else if depth == 1 && line.starts_with('"') {
            let fields: Vec<_> = line.split('"').collect();
            assert_eq!(fields.len(), 5, "fixture KVP must be simple: {line}");
            entity.insert(fields[1].into(), fields[3].into());
        }
    }
    assert_eq!(depth, 0);
    entities
}
fn tags(entity: &BTreeMap<String, String>) -> Vec<String> {
    entity
        .get("_tags")
        .map(|s| s.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}
fn fixture_world() -> postretro_level_loader::LevelWorld {
    let mut world = level_world("coop-presentation-test", 1);
    for entity in authored_entities() {
        let classname = entity.get("classname").unwrap().as_str();
        let entity_tags = tags(&entity);
        match classname {
            "worldspawn" => {}
            "light" => world.lights.push(MapLight {
                origin: [world.lights.len() as f64 * 4.0, 3.0, 0.0],
                light_type: LightType::Point,
                intensity: 1.0,
                color: [1.0; 3],
                falloff_model: FalloffModel::InverseSquared,
                falloff_range: 10.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0; 3],
                is_dynamic: true,
                casts_entity_shadows: false,
                animated_slot: None,
                tags: entity_tags,
                cell_index: 0,
                shadow_type: ShadowType::StaticLightMap,
            }),
            "fog_volume" => world.fog_volumes.push(FogVolumeRecord {
                min: [-1.0; 3],
                max: [1.0; 3],
                center: [0.0; 3],
                inv_half_ext: [1.0; 3],
                half_diag: 3.0_f32.sqrt(),
                shape_mode: 0.0,
                plane_count: 0,
                planes: vec![],
                density: 0.05,
                edge_softness: 0.0,
                glow: 0.05,
                radial_falloff: 1.0,
                tint: [1.0; 3],
                saturation: 1.0,
                min_brightness: 0.0,
                light_range: 1.0,
                anisotropy: 0.0,
                ambient_scatter: 1.0,
                tags: entity_tags,
            }),
            "trigger_volume" => world.trigger_volumes.push(TriggerVolumeRecord {
                name: entity.get("name").cloned().unwrap_or_default(),
                tags: entity_tags,
                aabb_min: [0.0; 3],
                aabb_max: [1.0; 3],
                activation: 0,
                target_tag: String::new(),
                command: 0,
                command_arg: String::new(),
                fire_mode: 1,
                rearm_ms: 0.0,
                enabled_on_spawn: true,
                on_fire: String::new(),
                on_exit: String::new(),
            }),
            "kinematic_mover" => world.kinematic_geometry.movers.push(LoadedKinematicMover {
                mover_id: 0,
                name: entity["name"].clone(),
                tags: entity_tags,
                origin: glam::Vec3::ZERO,
                path: entity["path"].clone(),
                speed_mps: 3.0,
                wait_ms: 0.0,
                move_mode: 0,
                start_on_spawn: false,
                vertices: vec![],
                indices: vec![],
                face_meta: vec![],
                spin_axis: glam::Vec3::ZERO,
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".into(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: Some(0.0),
                open_event: Some(entity["open_event"].clone()),
                close_event: None,
                blocked_event: None,
                crush_event: None,
                sealed_portal_ids: vec![],
                carried_lights: vec![],
                open_sound: None,
                close_sound: None,
                blocked_sound: None,
                crush_sound: None,
            }),
            "kinematic_waypoint" => {
                world
                    .kinematic_geometry
                    .waypoints
                    .push(LoadedKinematicWaypoint {
                        name: entity["name"].clone(),
                        next: entity.get("next").cloned().unwrap_or_default(),
                        origin: glam::Vec3::new(
                            0.0,
                            world.kinematic_geometry.waypoints.len() as f32 * 4.0,
                            0.0,
                        ),
                    })
            }
            _ => world.map_entities.push(MapEntityRecord {
                classname: classname.into(),
                tags: entity_tags,
                origin: [world.map_entities.len() as f32 * 4.0, 0.0, 0.0],
                angles: [0.0; 3],
                key_values: entity
                    .iter()
                    .filter(|(k, _)| {
                        !matches!(k.as_str(), "classname" | "origin" | "_tags" | "angles")
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }),
        }
    }
    assert_eq!(world.lights.len(), 3);
    assert_eq!(world.fog_volumes.len(), 1);
    assert_eq!(world.trigger_volumes.len(), 3);
    assert_eq!(world.kinematic_geometry.movers.len(), 1);
    assert_eq!(world.kinematic_geometry.waypoints.len(), 2);
    assert_eq!(
        world
            .map_entities
            .iter()
            .filter(|e| e.classname == "coop_target_dummy")
            .count(),
        10
    );
    world
}

fn registries(app: &mut crate::App, ctx: &ScriptCtx) {
    use postretro_sim::scripting::reactions::registry::*;
    let session = app.session.as_mut().unwrap();
    crate::scripting::builtins::register_builtins(&mut session.classname_dispatch);
    let scripting = &mut session.scripting;
    crate::scripting::primitives::light::register_light_reaction_primitives(
        &mut scripting.reaction_registry,
    );
    crate::scripting::primitives::light::register_sequenced_light_primitives(
        &mut scripting.sequence_registry,
        ctx.clone(),
    );
    register_emitter_reaction_primitives(&mut scripting.reaction_registry);
    register_fog_reaction_primitives(&mut scripting.reaction_registry);
    register_npc_state_reaction_primitives(&mut scripting.reaction_registry);
    register_grant_reactions(&mut scripting.reaction_registry);
    register_mover_reaction_primitives(
        &mut scripting.reaction_registry,
        Default::default(),
        Default::default(),
    );
    register_spawner_reaction_primitives(
        &mut scripting.reaction_registry,
        scripting.spawn_context.clone(),
    );
    register_trigger_reaction_primitives(&mut scripting.reaction_registry, Default::default());
    register_sequenced_fog_primitives(&mut scripting.sequence_registry, ctx.clone());
    register_sequenced_mover_primitives(
        &mut scripting.sequence_registry,
        ctx.clone(),
        Default::default(),
        Default::default(),
    );
    postretro_sim::scripting::reactions::system_commands::register_system_reaction_primitives(
        &mut scripting.system_registry,
    );
    crate::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
        &mut scripting.sequence_registry,
        scripting.scheduler.clone(),
    );
    scripting
        .scheduler
        .set_role(crate::scripting_systems::reaction_scheduler::SchedulerRole::Host);
}

/// Real dev mod initialization and production registrars for CPU install tests.
/// Callers may add observation primitives before building the runtime.
pub(crate) fn initialize_dev_mod(
    app: &mut crate::App,
    primitives: &PrimitiveRegistry,
) -> postretro_scripting_core::runtime::ModManifestResult {
    let ctx = script_ctx(app);
    let mut runtime = ScriptRuntime::new(
        primitives,
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        &ctx,
    )
    .unwrap();
    let mod_root = tempfile::tempdir().unwrap();
    let bundled = postretro_script_compiler::bundle_entry(&content().join("start-script.ts"))
        .expect("production dev registry bundles");
    std::fs::write(mod_root.path().join("start-script.js"), bundled).unwrap();
    runtime
        .run_mod_init(mod_root.path())
        .expect("production dev registry initializes");
    let mut manifest = runtime.mod_manifest().unwrap().clone();
    manifest.resolve_entity_faction_indices().unwrap();
    {
        let mut data = ctx.data_registry.borrow_mut();
        data.replace_entity_types(manifest.entities.clone());
        data.replace_global_reactions(manifest.reactions.clone());
        data.replace_global_crossings(manifest.crossings.clone());
        data.replace_global_player_events(manifest.player_events.clone());
    }
    app.session
        .as_mut()
        .unwrap()
        .scripting
        .impact_policy_runtime
        .replace_global_events(manifest.events.clone());
    app.session.as_mut().unwrap().scripting.script_runtime = runtime;
    registries(app, &ctx);
    manifest
}

fn install(luau: bool, capture_return: bool) -> (crate::App, Value) {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let captured = Rc::new(RefCell::new(None));
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    primitives
        .register("__coopFixtureCapture", {
            let captured = captured.clone();
            move |value: StoreSchemaJson| -> Result<(), ScriptError> {
                *captured.borrow_mut() = Some(value.0);
                Ok(())
            }
        })
        .scope(ContextScope::Both)
        .doc("test setup return capture")
        .param("value", "unknown")
        .finish();
    let manifest = initialize_dev_mod(&mut app, &primitives);
    let script = if luau {
        "coop-presentation-test.luau"
    } else {
        "coop-presentation-test.ts"
    };
    let path = content().join("scripts").join(script);
    let mut source = if luau {
        std::fs::read_to_string(&path).unwrap()
    } else {
        postretro_script_compiler::bundle_entry(&path).expect("production coop setup bundles")
    };
    if capture_return {
        source.push_str(if luau {
        "\ndo local original=setupLevel setupLevel=function(ctx) local result=original(ctx) __coopFixtureCapture(result) return result end end\n"
    } else {
        "\n;(function(){const original=globalThis.setupLevel;globalThis.setupLevel=function(ctx){const result=original(ctx);__coopFixtureCapture(result);return result;};})();\n"
    });
    }
    let mut world = fixture_world();
    world.data_script = Some(DataScriptSection {
        compiled_bytes: source.into_bytes(),
        source_path: path.to_string_lossy().into_owned(),
    });
    let logs = postretro_test_log_capture::LogCapture::start();
    install_capture_world(&mut app, &world);
    let raw = if capture_return {
        captured.borrow_mut().take().unwrap_or_else(|| {
            panic!(
                "{script}: actual authored setupLevel did not return; installer logs: {:#?}",
                logs.records()
            )
        })
    } else {
        // Prove the shipping bundle installs before any capture wrapper is added.
        let count = ctx
            .data_registry
            .borrow()
            .reactions
            .iter()
            .filter(|r| r.name.starts_with("coop."))
            .count();
        assert_eq!(
            count,
            21,
            "{script}: unwrapped shipping setup failed: {:#?}",
            logs.records()
        );
        Value::Null
    };
    assert_fixture_descriptors(&manifest);
    (app, raw)
}

/// The shipped player-events bodies (`player-events.{ts,luau}`) as an
/// installed, validated registry must hold them: each tracer array is one
/// sequence of every authored step, in order. A Pass A drop keeps the name and
/// empties the body to `Sequence(vec![])`, so a name check alone passes it.
pub(crate) fn assert_merged_player_event_bodies(reactions: &[NamedReaction], label: &str) {
    use SequenceTarget::{EventPlayer, System};
    let expected = [
        (
            "leveling.levelUp",
            vec![
                (EventPlayer, "addSlot"),
                (System, "setState"),
                (System, "playSound"),
                (System, "flashScreen"),
                (System, "screenShake"),
            ],
        ),
        (
            "heat.scald",
            vec![(EventPlayer, "applyDamage"), (System, "playSound")],
        ),
    ];
    for (name, steps) in expected {
        let bodies: Vec<_> = reactions.iter().filter(|r| r.name == name).collect();
        assert_eq!(bodies.len(), 1, "{label}: `{name}` installs one body");
        let ReactionDescriptor::Sequence(installed) = &bodies[0].descriptor else {
            panic!(
                "{label}: `{name}` installs as a sequence, not {:?}",
                bodies[0].descriptor
            );
        };
        let installed: Vec<_> = installed
            .iter()
            .map(|step| (step.id.clone(), step.primitive.as_str()))
            .collect();
        assert_eq!(
            installed, steps,
            "{label}: `{name}` survives validation with every step, in authored order"
        );
    }
}

fn assert_fixture_descriptors(manifest: &postretro_scripting_core::runtime::ModManifestResult) {
    let descriptor = |name: &str| {
        manifest
            .entities
            .iter()
            .find(|d| d.canonical_name.as_deref() == Some(name))
            .unwrap()
    };
    assert!(descriptor("coop_light_only").light.is_some());
    assert!(descriptor("coop_light_only").mesh.is_none());
    assert!(descriptor("coop_lit_prop").light.is_some());
    assert!(descriptor("coop_lit_prop").mesh.is_some());
    assert!(descriptor("coop_emitter_only").emitter.is_some());
    assert!(descriptor("coop_invisible_target").health.is_some());
    assert!(descriptor("coop_invisible_target").mesh.is_none());
    let retained = descriptor("coop_health_weapon");
    assert!(retained.health.is_some() && retained.weapon.is_some() && retained.touchable.is_some());
    let heat = serde_json::to_value(descriptor("coop_heat_test").weapon.as_ref().unwrap()).unwrap();
    assert_eq!(heat["resource"]["kind"], "heat");
    assert!((heat["resource"]["overheatAt"].as_f64().unwrap() - 100.0).abs() < 1e-6);
    let override_event = manifest
        .events
        .iter()
        .find(|e| e.filter_tag.as_deref() == Some("coop_dummy"))
        .unwrap();
    assert!(override_event.is_override);
    let base = manifest
        .events
        .iter()
        .position(|e| e.id == override_event.id && !e.is_override)
        .unwrap();
    let specialized = manifest
        .events
        .iter()
        .position(|e| e == override_event)
        .unwrap();
    assert!(
        base < specialized,
        "the actual registry includes the base before its refinement"
    );
    let effects = serde_json::to_string(&override_event.policy).unwrap();
    assert!(effects.contains("death") && effects.contains("1800") && effects.contains("despawn"));
}

fn assert_fixture_targets_hittable(app: &crate::App) {
    use glam::Vec3;
    use postretro_entities::{
        ComponentKind, Transform,
        components::{mesh::MeshComponent, weapon::WeaponComponent},
    };
    use postretro_sim::weapon::{FireButtonState, resolve_test_client_shot};

    let ctx = script_ctx(app);
    let data = ctx.data_registry.borrow();
    let pistol = data
        .entities
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("reference_pistol"))
        .unwrap()
        .weapon
        .as_ref()
        .unwrap();
    let registry = ctx.registry.borrow();
    let store = &app.session.as_ref().unwrap().hit_zone_store;
    let collision = crate::collision::CollisionWorld::new();
    for (tag, count) in [("coop_dummy", 10), ("coop_invisible_target", 1)] {
        let targets: Vec<_> = registry
            .query_by_component_and_tag(ComponentKind::Health, Some(tag))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(targets.len(), count, "installed targets for {tag}");
        for target in targets {
            if tag == "coop_dummy" {
                let mesh = registry.get_component::<MeshComponent>(target).unwrap();
                assert_eq!(
                    mesh.model,
                    "models/reference_enemy_kaykit_knight/scene.gltf"
                );
                let model = store
                    .get_by_name(&mesh.model)
                    .expect("the actual knight model must load during CPU installation");
                assert!(model.joint_zones.iter().all(Option::is_none));
                assert!(model.derived_bound.is_none());
            }
            // Regression: the unzoned knight had no authored hitbox, so pistol torso rays missed.
            let torso = registry
                .get_component::<Transform>(target)
                .unwrap()
                .position
                + Vec3::new(0.0, 0.9, 0.0);
            let mut weapon = WeaponComponent::from_descriptor(pistol);
            let resolution = resolve_test_client_shot(
                None,
                &mut weapon,
                "reference_pistol",
                0,
                FireButtonState {
                    pressed: true,
                    active: true,
                },
                torso + Vec3::new(0.0, 0.0, 2.0),
                Vec3::NEG_Z,
                &pistol.placement.clone().unwrap_or_default(),
                pistol.muzzle_offset.map(Vec3::from),
                1,
                &[0.0],
                &[],
                &collision,
                &registry,
                store,
                0.0,
                0.0,
            )
            .expect("the installed dev pistol must fire");
            assert_eq!(resolution.hits.len(), 1, "pistol torso hit on {tag}");
            let hit = &resolution.hits[0];
            assert_eq!(
                hit.target, target,
                "pistol ray must select the installed {tag}"
            );
            assert!(
                hit.zone.is_none(),
                "the authored body AABB supplies this hit"
            );
            assert!((hit.point.y - torso.y).abs() < 1e-5);
        }
    }
}

#[test]
fn coop_fixture_twins_evaluate_validate_and_cpu_install_equivalently() {
    let (ts_control, _) = install(false, false);
    let (luau_control, _) = install(true, false);
    let (ts, ts_raw) = install(false, true);
    let (luau, luau_raw) = install(true, true);
    assert_eq!(
        script_ctx(&ts_control).data_registry.borrow().reactions,
        script_ctx(&ts).data_registry.borrow().reactions,
        "capture wrapper changes TS production install"
    );
    assert_eq!(
        script_ctx(&luau_control).data_registry.borrow().reactions,
        script_ctx(&luau).data_registry.borrow().reactions,
        "capture wrapper changes Luau production install"
    );
    assert_eq!(ts_raw, luau_raw, "actual typed builder wire output differs");
    let ts_ctx = script_ctx(&ts);
    let luau_ctx = script_ctx(&luau);
    let a = ts_ctx.data_registry.borrow();
    let b = luau_ctx.data_registry.borrow();
    assert_eq!(a.reactions, b.reactions);
    assert_eq!(a.crossings, b.crossings);
    assert_eq!(a.player_events, b.player_events);
    assert_eq!(a.volume_trigger_events(), b.volume_trigger_events());
    assert_eq!(a.volume_trigger_events().len(), 3, "all authored pads bind");
    let fixture: Vec<_> = a
        .reactions
        .iter()
        .filter(|r| r.name.starts_with("coop."))
        .collect();
    assert_eq!(fixture.len(), 21);
    for name in [
        "coop.atmosphere.lights",
        "coop.atmosphere.fogFade",
        "coop.atmosphere.door",
        "coop.enemy.dead",
    ] {
        assert!(fixture.iter().any(|r| r.name == name && matches!(&r.descriptor, ReactionDescriptor::Sequence(steps) if !steps.is_empty())), "{name} must survive production validation and have actual queried targets");
    }
    assert!(a.player_events.iter().any(|e| {
        e.descriptor
            .fire
            .iter()
            .any(|name| name == "leveling.levelUp")
    }));
    let mut ts_bindings =
        postretro_sim::trigger_bindings::TriggerBindingTable::build_with_script_ctx(
            &ts_ctx.registry.borrow(),
            &a,
            &ts_ctx,
        );
    let mut luau_bindings =
        postretro_sim::trigger_bindings::TriggerBindingTable::build_with_script_ctx(
            &luau_ctx.registry.borrow(),
            &b,
            &luau_ctx,
        );
    ts_bindings.install_manifest_events(&ts_ctx.registry.borrow(), &a, &ts_ctx);
    luau_bindings.install_manifest_events(&luau_ctx.registry.borrow(), &b, &luau_ctx);
    assert_eq!(ts_bindings.bound_edges(), luau_bindings.bound_edges());
    assert_eq!(ts_bindings.bound_edges().len(), 3);
    assert_merged_player_event_bodies(&a.reactions, "coop fixture");
    let registry = ts_ctx.registry.borrow();
    for (tag, kind) in [
        (
            "coop_descriptor_light",
            postretro_entities::ComponentKind::Light,
        ),
        (
            "coop_descriptor_emitter",
            postretro_entities::ComponentKind::BillboardEmitter,
        ),
        ("coop_mesh_prop", postretro_entities::ComponentKind::Mesh),
        (
            "coop_health_weapon",
            postretro_entities::ComponentKind::Health,
        ),
        (
            "coop_invisible_target",
            postretro_entities::ComponentKind::Health,
        ),
        (
            "coop_death_enemy",
            postretro_entities::ComponentKind::Health,
        ),
    ] {
        assert!(
            registry
                .query_by_component_and_tag(kind, Some(tag))
                .next()
                .is_some(),
            "production descriptor install missing {tag}"
        );
    }
    for app in [&ts_control, &luau_control, &ts, &luau] {
        assert_fixture_targets_hittable(app);
    }
}
