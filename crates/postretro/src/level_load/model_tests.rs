//! Model prediction and parse on the pool: the predicted handles cover the
//! install's real sweep (host and connected client), and each prepared
//! model's parse, clip entry, hit zones and material CPU products equal what
//! the oracle sweep builds, a model that fails to parse included.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Predict models, then fall back exactly)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_model::ModelHandle;
use postretro_model::gltf_loader::LoadedModel;
use postretro_render_cpu::material_plan::{parse_blake3_key, plan_submesh_materials};
use postretro_render_cpu::texture_prep::prepare_model_diffuse_texture;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::model_prediction::predict_level_models;
use super::oracle_tests::texture_summary;
use super::test_support::{
    inputs_for, offscreen_renderer_or_skip, prediction_inputs, run_prep, test_pool,
};
use crate::scripting_systems::hit_zones::{HitZoneStore, ModelLoadWarningOwner};
use crate::startup::lifecycle::tests::{
    install_descriptor_world, level_world, script_ctx, test_app,
};

pub(crate) fn dev_content_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

/// The dev mod's registered entity descriptors, from its real mod init.
pub(super) fn dev_descriptors() -> Vec<EntityTypeDescriptor> {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = postretro_scripting_core::primitives_registry::PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx);
    crate::coop_fixture_tests::initialize_dev_mod(&mut app, &primitives).entities
}

pub(crate) fn record(
    classname: &str,
    key_values: &[(&str, &str)],
    index: usize,
) -> MapEntityRecord {
    MapEntityRecord {
        classname: classname.to_string(),
        origin: [index as f32 * 4.0, 0.0, 0.0],
        key_values: key_values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
        ..Default::default()
    }
}

/// A prop model no dev descriptor names.
const PROP_MODEL: &str = "models/decraniated_low_poly_retro_pixel/scene.gltf";

/// Placements that exercise every rule the prediction mirrors: a built-in
/// `prop_mesh` (and one with an empty model), a built-in `entity_spawner`
/// naming an AI enemy, one placement of every map-placeable mesh descriptor
/// but the movement ones, a player start, an unknown classname and a region
/// companion.
fn rule_coverage_placements(descriptors: &[EntityTypeDescriptor]) -> Vec<MapEntityRecord> {
    let mut records = vec![
        record("prop_mesh", &[("model", PROP_MODEL)], 0),
        record("prop_mesh", &[("model", "")], 1),
        record("player_spawn", &[], 2),
        record("no_such_classname", &[("model", "models/never.gltf")], 3),
        record("trigger_teleport", &[], 4),
    ];
    let enemy = descriptors
        .iter()
        .find(|descriptor| descriptor.behavior.is_some() && descriptor.mesh.is_some())
        .and_then(|descriptor| descriptor.canonical_name.clone())
        .expect("the dev mod registers an AI enemy with a mesh");
    records.push(record(
        "entity_spawner",
        &[("archetype", &enemy), ("count", "1")],
        5,
    ));
    for descriptor in descriptors {
        if descriptor.mesh.is_some()
            && descriptor.movement.is_none()
            && let Some(name) = &descriptor.canonical_name
        {
            let index = records.len();
            records.push(record(name, &[], index));
        }
    }
    records
}

/// The real sweep's model list for `map_entities`: the production
/// renderer-free install with `descriptors`, reading the list the upload
/// hook receives.
pub(super) fn real_sweep(
    map_entities: Vec<MapEntityRecord>,
    descriptors: &[EntityTypeDescriptor],
    connected_client: bool,
) -> (Vec<String>, postretro_entities::EntityRegistry) {
    let mut world = level_world("model_prediction", 1);
    world.map_entities = map_entities;
    let installed = install_descriptor_world(
        world,
        crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
        &[],
        Vec::new(),
        connected_client,
        descriptors.to_vec(),
    );
    (installed.upload_models, installed.registry)
}

fn set(handles: impl IntoIterator<Item = impl AsRef<str>>) -> BTreeSet<String> {
    handles
        .into_iter()
        .map(|handle| handle.as_ref().to_string())
        .collect()
}

// Models 1: on fixtures that install entities with models through every
// placement rule, with the dev mod's real descriptors, the prediction equals
// the real sweep's list as a set for both net roles: no miss, and nothing
// over-predicted.
#[test]
fn predicted_models_match_the_real_sweep() {
    let descriptors = dev_descriptors();
    let placements = rule_coverage_placements(&descriptors);
    for connected_client in [false, true] {
        let (real, _) = real_sweep(placements.clone(), &descriptors, connected_client);
        let predicted = predict_level_models(
            &placements,
            &prediction_inputs(&descriptors, connected_client),
        );
        assert_eq!(
            predicted.len(),
            set(&predicted).len(),
            "the prediction is deduplicated"
        );
        assert!(
            real.contains(&PROP_MODEL.to_string()),
            "connected {connected_client}: the fixture's prop reaches the sweep"
        );
        let real = set(&real);
        let predicted = set(&predicted);
        let misses: Vec<_> = real.difference(&predicted).collect();
        let over: Vec<_> = predicted.difference(&real).collect();
        assert!(
            misses.is_empty() && over.is_empty(),
            "connected {connected_client}: misses {misses:?}, over-predictions {over:?}"
        );
    }
}

// Models 1: on a connected client, a host-replicated placement never spawns
// locally, yet its model reaches the sweep through the suppressed set; the
// prediction carries it, so it misses zero times.
#[test]
fn connected_client_prediction_covers_replicated_placements() {
    use crate::scripting::builtins::descriptor_is_host_replicated;
    let descriptors = dev_descriptors();
    let replicated: Vec<&EntityTypeDescriptor> = descriptors
        .iter()
        .filter(|descriptor| {
            descriptor.mesh.is_some()
                && descriptor.movement.is_none()
                && descriptor_is_host_replicated(descriptor)
        })
        .collect();
    assert!(!replicated.is_empty(), "the dev mod has replicated meshes");
    let placements: Vec<MapEntityRecord> = replicated
        .iter()
        .enumerate()
        .map(|(index, descriptor)| {
            record(descriptor.canonical_name.as_deref().unwrap(), &[], index)
        })
        .collect();
    let (real, registry) = real_sweep(placements.clone(), &descriptors, true);
    assert!(
        registry
            .iter_with_kind(postretro_entities::ComponentKind::DescriptorProvenance)
            .next()
            .is_none(),
        "a connected client spawns none of the replicated placements"
    );
    assert!(
        !real.is_empty(),
        "the replicated placements' models reach the sweep"
    );
    let predicted = predict_level_models(&placements, &prediction_inputs(&descriptors, true));
    let predicted = set(&predicted);
    let misses: Vec<&String> = real
        .iter()
        .filter(|handle| !predicted.contains(*handle))
        .collect();
    assert!(misses.is_empty(), "client misses: {misses:?}");
    for descriptor in replicated {
        let model = &descriptor.mesh.as_ref().unwrap().model;
        if !model.is_empty() {
            assert!(real.contains(model), "{model} reaches the client sweep");
            assert!(predicted.contains(model), "{model} is predicted");
        }
    }
}

/// A streamed-lightmap fixture whose map-entity section places `records`.
pub(crate) fn write_level_with_entities(
    records: Vec<MapEntityRecord>,
) -> crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl {
    use postretro_level_format::{SectionBlob, SectionId};
    crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write_edited(|sections| {
        sections.retain(|section| section.section_id != SectionId::MapEntity as u32);
        sections.push(SectionBlob {
            section_id: SectionId::MapEntity as u32,
            version: 1,
            data: MapEntitySection { entries: records }.to_bytes(),
        });
    })
}

/// Everything the upload and the hit-zone store read of a parse, the
/// socket map in key order.
fn parse_summary(model: &LoadedModel) -> String {
    let mut sockets: Vec<String> = model
        .sockets
        .iter()
        .map(|(name, binding)| format!("{name}={binding:?}"))
        .collect();
    sockets.sort();
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{sockets:?}",
        model.mesh,
        model.skeleton,
        model.clips,
        model.submeshes,
        model.joint_zones,
        model.legs,
        model.tags
    )
}

fn hit_zone_summary(store: &HitZoneStore, handle: &str) -> Option<String> {
    let key = ModelHandle::from(handle.to_string());
    store.get(&key).map(|zones| {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
            zones.skeleton,
            zones.clips,
            zones.joint_zones,
            zones.derived_bound,
            zones.pose_stack,
            store.has_pose_modifiers(&key),
            store.has_precise_zones(&key)
        )
    })
}

// Every prepared model equals the oracle sweep's: the parse, the clip entry
// (read back from the renderer's cache after the oracle's upload when an
// adapter exists), the hit-zone entry, and the material plan and diffuse CPU
// products the renderer's upload builds. A model that fails to parse carries
// the parse error, the empty clip entry with zero bounds, and no hit zones,
// as the oracle leaves it; it fails nothing. Results keep predicted order
// though the first model finishes last.
#[test]
fn prepared_models_match_the_oracle_sweep() {
    let mut renderer = offscreen_renderer_or_skip("prepared_models_match_the_oracle_sweep");
    let handles = [
        PROP_MODEL,
        "models/does_not_exist/scene.gltf",
        "models/ar_4/model.gltf",
        "models/pose-modifier-fixture/joint_zones.gltf",
        "models/reference_enemy_kaykit_knight/scene.gltf",
    ];
    let level = write_level_with_entities(
        handles
            .iter()
            .enumerate()
            .map(|(index, handle)| record("prop_mesh", &[("model", handle)], index))
            .collect(),
    );
    let content_root = dev_content_root();
    let prm_root = crate::startup::worker::derive_prm_root_dev_layout(&content_root);
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), &prm_root);
    inputs.content_root = content_root.clone();
    inputs.audio_enabled = false;
    let parse_starts = Arc::new(Mutex::new(Vec::new()));
    {
        let parse_starts = Arc::clone(&parse_starts);
        inputs.hooks.before_model_parse = Some(Arc::new(move |index| {
            parse_starts.lock().unwrap().push(index);
            if index == 0 {
                // The first model finishes after the others.
                std::thread::sleep(Duration::from_millis(300));
            }
        }));
    }
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared = outcome.unwrap_or_else(|failure| panic!("prep failed: {failure}"));
    let models = prepared.presentation.models;
    assert_eq!(
        models.handles().collect::<Vec<_>>(),
        handles,
        "prepared models keep the predicted order"
    );
    assert_eq!(parse_starts.lock().unwrap().len(), handles.len());
    assert_eq!(
        models.get(handles[2]).map(|model| model.handle.as_str()),
        Some(handles[2]),
        "prepared models are keyed by handle"
    );
    let real_sweep: Vec<String> = handles[1..]
        .iter()
        .map(|handle| handle.to_string())
        .chain(["models/registry_only.gltf".to_string()])
        .collect();
    assert_eq!(
        models.misses(&real_sweep),
        ["models/registry_only.gltf"],
        "a handle only the real sweep holds is a miss"
    );
    assert_eq!(models.over_predictions(&real_sweep), [PROP_MODEL]);
    let has_zone_bearing_model = models
        .iter()
        .filter_map(|model| model.parsed.as_ref().ok())
        .any(|parsed| parsed.hit_zones.derived_bound.is_some());
    let has_animated_model = models
        .iter()
        .any(|model| !model.clip_entry.clips.is_empty());

    let oracle =
        crate::startup::lifecycle::parse_sweep_models(&handles.map(str::to_string), &content_root);
    let mut failed = 0;
    for (model, oracle) in models.into_models().into_iter().zip(oracle) {
        assert_eq!(model.handle, oracle.handle);
        assert_eq!(model.open_path, oracle.open_path);

        match (&model.parsed, &oracle.result) {
            (Ok(parsed), Ok(loaded)) => {
                let upload = parsed
                    .upload
                    .as_ref()
                    .expect("a load that builds no set keeps each model's upload inputs");
                assert_eq!(
                    parse_summary(&upload.model),
                    parse_summary(loaded),
                    "{}: parse",
                    model.handle
                );
                let plan = plan_submesh_materials(&loaded.submeshes);
                assert_eq!(upload.materials.plan, plan, "{}: plan", model.handle);
                let diffuse: Vec<_> = plan
                    .distinct_keys
                    .iter()
                    .map(|key| prepare_model_diffuse_texture(key, parse_blake3_key(key), &prm_root))
                    .collect();
                assert_eq!(
                    texture_summary(&upload.materials.diffuse),
                    texture_summary(&diffuse),
                    "{}: diffuse",
                    model.handle
                );
            }
            (Err(prepared_error), Err(oracle_error)) => {
                assert_eq!(prepared_error.to_string(), oracle_error.to_string());
                assert_eq!(
                    model.clip_entry,
                    crate::startup::lifecycle::ModelClipEntry::default()
                );
                failed += 1;
            }
            _ => panic!(
                "{}: prep and the oracle disagree on the parse",
                model.handle
            ),
        }

        // Clip entry: the renderer's cache after the oracle's own upload,
        // which is what the oracle sweep's tables used to read back.
        let (clips, bounds) = match renderer.as_mut() {
            Some(renderer) => {
                renderer.upload_parsed_skinned_model(
                    &oracle.handle,
                    &oracle.open_path,
                    &oracle.result,
                    &prm_root,
                );
                (
                    renderer.skinned_model_clip_metadata(&oracle.handle),
                    renderer.skinned_model_local_bounds(&oracle.handle),
                )
            }
            None => {
                let entry = crate::startup::lifecycle::model_clip_entry(&oracle.result);
                (entry.clips, entry.bounds)
            }
        };
        assert_eq!(model.clip_entry.clips, clips, "{}: clips", model.handle);
        assert_eq!(model.clip_entry.bounds, bounds, "{}: bounds", model.handle);

        // Hit zones: the oracle store consumes its parse; prep's prebuilt
        // entry installs through the cheap half.
        let mut oracle_store = HitZoneStore::new();
        let mut prepared_store = HitZoneStore::new();
        prepared_store.insert_built(
            &model.handle,
            &model.open_path,
            model.parsed.map(|parsed| parsed.hit_zones),
            ModelLoadWarningOwner::GameSide,
        );
        oracle_store.insert_loaded(
            &oracle.handle,
            &oracle.open_path,
            oracle.result,
            ModelLoadWarningOwner::GameSide,
        );
        assert_eq!(
            hit_zone_summary(&prepared_store, &model.handle),
            hit_zone_summary(&oracle_store, &model.handle),
            "{}: hit zones",
            model.handle
        );
    }
    assert_eq!(failed, 1, "the missing model fails to parse on both paths");
    assert!(has_zone_bearing_model, "a zone-bearing model is covered");
    assert!(has_animated_model, "an animated model is covered");
}

/// Install `world` with the dev mod initialized, as the session would: the
/// real data script runs, the archetype sweep reads the mod's descriptors.
/// Returns the prediction from the dispatch snapshot taken before install,
/// and the real sweep's list.
fn predict_and_sweep_dev_map(
    world: &postretro_level_loader::LevelWorld,
    connected_client: bool,
) -> (Vec<String>, Vec<String>) {
    use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = postretro_scripting_core::primitives_registry::PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    crate::coop_fixture_tests::initialize_dev_mod(&mut app, &primitives);
    let predicted = {
        let session = app.session.as_ref().unwrap();
        let inputs = super::model_prediction::ModelPredictionInputs::capture(
            &ctx.data_registry.borrow().entities,
            &session.classname_dispatch,
            connected_client,
        );
        predict_level_models(&world.map_entities, &inputs)
    };
    let content_root = dev_content_root();
    let mut real = Vec::new();
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
        trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
        suppress_host_replicated: connected_client,
        suppress_boot_pawn: connected_client,
        local_carried_loadout: None,
    };
    let _ = install_world_cpu(
        handles,
        &mut crate::startup::StartupTimings::new(),
        |models, _clip_tables| {
            real.extend(models.iter().map(|model| model.handle.clone()));
            ModelLoadWarningOwner::GameSide
        },
        |_first_player| {},
    );
    (predicted, real)
}

// Models 1, on demand: every compiled dev-mod map, host and connected
// client, against the real install's sweep with the dev mod initialized.
// Zero misses on every current map; over-predictions are reported. A map
// the current loader refuses (a stale bake) is skipped with its reason.
#[test]
#[ignore = "requires the compiled dev-mod maps; run on demand and record the table"]
fn dev_mod_maps_predict_every_model() {
    let content_root = dev_content_root();
    let mut maps: Vec<_> = std::fs::read_dir(content_root.join("maps"))
        .expect("content/dev/maps exists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    maps.sort();
    assert!(!maps.is_empty(), "compile the dev-mod maps first");
    let mut rows = Vec::new();
    let mut missed = Vec::new();
    for map in maps {
        let label = map.file_stem().unwrap().to_string_lossy().into_owned();
        let world = match postretro_level_loader::load_prl(map.to_str().unwrap()) {
            Ok(world) => world,
            Err(error) => {
                eprintln!("[ModelPrediction] {label}: skipped, stale bake: {error}");
                continue;
            }
        };
        for connected_client in [false, true] {
            let (predicted, real) = predict_and_sweep_dev_map(&world, connected_client);
            let (predicted_set, real_set) = (set(&predicted), set(&real));
            let misses: Vec<&String> = real_set.difference(&predicted_set).collect();
            let over: Vec<&String> = predicted_set.difference(&real_set).collect();
            let role = if connected_client { "client" } else { "host" };
            let row = format!(
                "| {label} | {role} | {} | {} | {} | {} |",
                real.len(),
                predicted.len(),
                misses.len(),
                over.len()
            );
            eprintln!("[ModelPrediction] {row} misses {misses:?} over {over:?}");
            rows.push(row);
            if !misses.is_empty() {
                missed.push(format!("{label} ({role}): {misses:?}"));
            }
        }
    }
    eprintln!("[ModelPrediction] | map | role | sweep | predicted | misses | over |");
    for row in &rows {
        eprintln!("[ModelPrediction] {row}");
    }
    assert!(!rows.is_empty(), "no current dev-mod map loaded");
    assert!(missed.is_empty(), "prediction misses: {missed:?}");
}
