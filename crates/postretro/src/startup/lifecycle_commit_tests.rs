//! The one-frame commit of a level bundle against the main-thread install,
//! the oracle: equal results on the app's fixtures, no GPU creation beyond
//! its counted fallbacks, every failing check before any mutation, parity at
//! the commit, tiers from dispatch, one extent rebuild, and a forced model
//! miss loaded and logged once.
//! See: context/plans/done/level-load-pipeline/index.md (Acceptance: Off the main thread 2, Settings, Equivalence 2–3, Failure 4, Co-op, Models 1) · research.md §9 (P14, P16, P17, P23)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{Mat4, Vec3};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_loader::{HandOut, LevelWorld, ShDrainBatch};
use postretro_renderer::gpu_creation_counts;
use postretro_test_log_capture::LogCapture;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::lifecycle_test_fixtures::*;
use super::*;
use crate::level_load::test_support::{
    load_app_bundle, no_prm_root, offscreen_renderer_or_skip, retire_app_load, test_pool,
    write_textured_level,
};
use crate::level_load::{
    BundleSet, LevelBundle, LevelCommitReport, LevelLoadStages, LevelPrepInputs, LoadStage,
};
use crate::load_pool::LoadPool;
use crate::render::{ClearColor, ShSampleRegionSets};
use crate::startup::render_profile::{
    renderer_reflection_quality, renderer_spot_shadow_map_resolution,
    renderer_surface_depth_quality,
};

/// A small skinned model with hit zones, all buffers in `data:` URIs.
pub(super) const MODEL: &str = "models/pose-modifier-fixture/joint_zones.gltf";
/// A textured rifle no fixture predicts.
const RIFLE: &str = "models/ar_4/model.gltf";
/// A fog pixel scale other than the renderer's default of 4.
const FOG_SCALE: u32 = 6;

pub(super) fn dev_content_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

fn dev_prm_root() -> PathBuf {
    crate::startup::worker::derive_prm_root_dev_layout(&dev_content_root())
}

/// A test App with an offscreen renderer at the player's tiers (as full init
/// applies them) and the engine's built-in classname handlers, its content
/// rooted at `content_root`; `None`, with a note, without an adapter (and a
/// failure instead under `POSTRETRO_REQUIRE_GPU`).
pub(super) fn gpu_app(test: &str, content_root: &Path) -> Option<App> {
    let renderer = offscreen_renderer_or_skip(test)?;
    let mut app = test_app();
    app.renderer = Some(renderer);
    app.content_root = content_root.to_path_buf();
    crate::scripting::builtins::register_builtins(
        &mut app.session.as_mut().unwrap().classname_dispatch,
    );
    let options = app.session.as_ref().unwrap().player_options.clone();
    app.configure_player_shadow_quality(options.shadow_quality);
    app.apply_player_surface_depth_quality(options.surface_depth_quality);
    app.apply_player_reflection_quality(options.reflection_quality);
    // Both paths roll trigger pools; a pinned seed makes them comparable.
    app.session_boot_config = crate::startup::session::SessionBootConfig::with_pool_seed(1);
    Some(app)
}

/// One level, loadable by both paths: the file, its `.prm` root, the mod's
/// content root, and edits that stand in for level data the file lacks.
pub(super) struct CommitCase {
    pub(super) label: &'static str,
    pub(super) map: PathBuf,
    pub(super) prm_root: PathBuf,
    pub(super) content_root: PathBuf,
    /// Applied to the assembled level before prep's tasks read it, and to the
    /// oracle's parsed level: data both paths see.
    pub(super) edit_world: Option<fn(&mut LevelWorld)>,
    /// Applied to prep's copy of each hand-out (so the model prediction sees
    /// it); `edit_world` must make the oracle's level match.
    pub(super) edit_hand_out: Option<fn(&mut HandOut)>,
    /// Model handles the level installs.
    pub(super) models: &'static [&'static str],
    /// Whether the fixture draws a frame worth hashing.
    pub(super) frame: bool,
    pub(super) _keep: Box<dyn std::any::Any>,
}

impl CommitCase {
    /// The textured level whose lightmap streams: a baked wall texture,
    /// UV normalize, a collision mesh.
    pub(super) fn textured() -> Self {
        let (level, prm_root) = write_textured_level();
        Self {
            label: "textured",
            map: level.path().to_path_buf(),
            prm_root: prm_root.path().to_path_buf(),
            content_root: level.path().parent().unwrap().to_path_buf(),
            edit_world: None,
            edit_hand_out: None,
            models: &[],
            frame: true,
            _keep: Box::new((level, prm_root)),
        }
    }

    /// The streamed-SH level.
    pub(super) fn streamed_sh() -> Self {
        let (dir, map) = crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
        Self {
            label: "streamed-sh",
            map,
            prm_root: no_prm_root(),
            content_root: dir.path().to_path_buf(),
            edit_world: None,
            edit_hand_out: None,
            models: &[],
            frame: false,
            _keep: Box::new(dir),
        }
    }

    /// The textured level with a predicted `prop_mesh` naming [`MODEL`], a
    /// non-default fog pixel scale, and map lights, fog volumes and trigger
    /// volumes for the entity-id order: in the dev mod's content root, so the
    /// model resolves there.
    pub(super) fn textured_with_model() -> Self {
        let mut case = Self::textured();
        case.label = "textured+model";
        case.content_root = dev_content_root();
        case.edit_world = Some(add_model_and_entities);
        case.edit_hand_out = Some(add_model_hand_out);
        case.models = &[MODEL];
        case
    }

    fn edit_inputs(&self, inputs: &mut LevelPrepInputs) {
        inputs.prm_cache_root = self.prm_root.clone();
        inputs.hooks.edit_world = self.edit_world;
        inputs.hooks.edit_hand_out = self.edit_hand_out;
    }

    /// The oracle's level: the monolithic parse, edited as prep's is.
    pub(super) fn oracle_world(&self) -> LevelWorld {
        let mut world = postretro_level_loader::load_prl(self.map.to_str().unwrap())
            .unwrap_or_else(|error| panic!("{}: the oracle parse failed: {error}", self.label));
        if let Some(edit) = self.edit_world {
            edit(&mut world);
        }
        world
    }
}

fn model_record(handle: &str) -> MapEntityRecord {
    MapEntityRecord {
        classname: "prop_mesh".to_string(),
        origin: [0.25, 0.25, 0.5],
        key_values: vec![("model".to_string(), handle.to_string())],
        ..Default::default()
    }
}

fn add_model_hand_out(hand_out: &mut HandOut) {
    if let HandOut::Entities(entities) = hand_out {
        Arc::make_mut(entities)
            .map_entities
            .push(model_record(MODEL));
    }
}

/// Lights, fog volumes, triggers and the fog pixel scale; the model
/// placement only where prep's hand-out edit did not already add it.
pub(super) fn add_entities(world: &mut LevelWorld) {
    world.lights = vec![
        map_light("light", [0.0, 2.0, 0.0]),
        map_light("light", [4.0, 2.0, 0.0]),
    ];
    world.light_influences = world
        .lights
        .iter()
        .map(|light| postretro_render_data::influence::LightInfluence {
            center: Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            ),
            radius: light.falloff_range,
        })
        .collect();
    world.fog_volumes = vec![fog_record("fog", 0.0), fog_record("fog", 4.0)];
    world.trigger_volumes = vec![trigger_record("gate", 0), trigger_record("gate", 1)];
    world.fog_pixel_scale = FOG_SCALE;
}

fn add_model_and_entities(world: &mut LevelWorld) {
    add_entities(world);
    if !world
        .map_entities
        .iter()
        .any(|entity| entity.classname == "prop_mesh")
    {
        world.map_entities.push(model_record(MODEL));
    }
}

fn trigger_record(
    tag: &str,
    index: usize,
) -> postretro_level_format::trigger_volumes::TriggerVolumeRecord {
    let x = index as f32 * 4.0;
    postretro_level_format::trigger_volumes::TriggerVolumeRecord {
        name: format!("{tag}-{index}"),
        tags: vec![tag.to_string()],
        aabb_min: [x, 0.0, 0.0],
        aabb_max: [x + 1.0, 1.0, 1.0],
        activation: 0,
        target_tag: String::new(),
        command: 0,
        command_arg: String::new(),
        fire_mode: 0,
        rearm_ms: 0.0,
        enabled_on_spawn: true,
        on_fire: String::new(),
        on_exit: String::new(),
    }
}

/// Load `case` through the pipeline's stages for `app` to its bundle.
/// `edit` adjusts the prep inputs after the case's own edits.
pub(super) fn load_case(
    app: &mut App,
    pool: &LoadPool,
    case: &CommitCase,
    edit: impl FnOnce(&mut LevelPrepInputs),
) -> (LevelLoadStages, LevelBundle) {
    load_app_bundle(
        app,
        pool,
        &case.map,
        &case.content_root,
        |inputs| {
            case.edit_inputs(inputs);
            edit(inputs);
        },
        |_| {},
    )
}

/// Load `case` and commit its bundle in `app`; the commit's report.
pub(super) fn commit_case(app: &mut App, pool: &LoadPool, case: &CommitCase) -> LevelCommitReport {
    let (stages, bundle) = load_case(app, pool, case, |_| {});
    let report = app
        .commit_level_bundle(bundle)
        .unwrap_or_else(|failure| panic!("{}: the commit failed: {failure}", case.label));
    retire_app_load(stages, app);
    report
}

/// The main-thread oracle in `app`, as the worker path's
/// `finish_level_payload` runs it on a delivered level.
pub(super) fn oracle_case(app: &mut App, case: &CommitCase) {
    app.install_level_payload(case.oracle_world(), case.prm_root.clone())
        .unwrap_or_else(|error| panic!("{}: the oracle install failed: {error}", case.label));
    app.promote_level_tree_images();
    app.install_level_streaming_sessions()
        .unwrap_or_else(|error| panic!("{}: the oracle's sessions: {error:#}", case.label));
    app.enter_installed_level_settling();
}

/// Every id the registry holds of `kind`, in order.
fn ids_of(app: &App, kind: postretro_entities::ComponentKind) -> Vec<u32> {
    let mut ids: Vec<u32> = script_ctx(app)
        .registry
        .borrow()
        .iter_with_kind(kind)
        .map(|(id, _)| id.to_raw())
        .collect();
    ids.sort_unstable();
    ids
}

/// The highest light id and the lowest fog id `app`'s install assigned.
pub(super) fn light_and_fog_ids(app: &App) -> (u32, u32) {
    use postretro_entities::ComponentKind;
    (
        *ids_of(app, ComponentKind::Light)
            .iter()
            .max()
            .expect("light entities were populated"),
        *ids_of(app, ComponentKind::FogVolume)
            .iter()
            .min()
            .expect("fog entities were populated"),
    )
}

/// Everything the two paths must agree on, as text: the renderer's resource
/// summary, the entity ids by kind, each model's clip table, bounds and hit
/// zones, collision, streaming sessions, and the installed level and pose.
fn app_summary(app: &App, models: &[&str]) -> Vec<(String, String)> {
    use postretro_entities::ComponentKind;
    let session = app.session.as_ref().unwrap();
    let mut rows = vec![(
        "resources".to_string(),
        format!(
            "{:?}",
            app.renderer
                .as_ref()
                .and_then(|renderer| renderer.active_level_resource_summary())
        ),
    )];
    for kind in [
        ComponentKind::Transform,
        ComponentKind::Light,
        ComponentKind::FogVolume,
        ComponentKind::TriggerVolume,
        ComponentKind::KinematicMover,
        ComponentKind::Mesh,
    ] {
        rows.push((format!("ids {kind:?}"), format!("{:?}", ids_of(app, kind))));
    }
    for handle in models {
        let key = postretro_model::ModelHandle::from(handle.to_string());
        rows.push((
            format!("clips {handle}"),
            // The table itself compares by value in `assert_same_clip_tables`:
            // its name map is a `HashMap`, whose debug order varies.
            format!(
                "{:?} {:?}",
                session.mesh_clip_tables.get(&key).is_some(),
                session.mesh_clip_tables.model_bounds(&key)
            ),
        ));
        let zones = session.hit_zone_store.get(&key).map(|zones| {
            format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}",
                zones.skeleton,
                zones.clips,
                zones.joint_zones,
                zones.derived_bound,
                zones.pose_stack
            )
        });
        rows.push((format!("hit zones {handle}"), format!("{zones:?}")));
    }
    rows.push((
        "collision".to_string(),
        format!(
            "{} {}",
            app.collision_world.triangle_count(),
            app.collision_world.vertex_count()
        ),
    ));
    rows.push((
        "streaming".to_string(),
        format!(
            "sh={} lightmap={} cell_demand={}",
            session.sh_streaming.is_some(),
            session.level_streaming.lightmap().is_some(),
            session.level_streaming.cell_demand().is_some()
        ),
    ));
    let level = app.level.as_ref().expect("the level is installed");
    rows.push((
        "level".to_string(),
        format!(
            "{:?} {:?} {}",
            level.vertices, level.gpu_lighting_payloads, level.fog_pixel_scale
        ),
    ));
    rows.push((
        "pose".to_string(),
        format!(
            "{:?} {} {} {:?} {:?}",
            app.camera.position,
            app.camera.yaw,
            app.camera.pitch,
            app.boot_state,
            app.trigger_pool_report
        ),
    ));
    rows
}

/// Each model's clip table, compared by value.
fn assert_same_clip_tables(label: &str, committed: &App, oracle: &App, models: &[&str]) {
    fn tables(app: &App) -> &crate::scripting_systems::mesh_anim::MeshClipTables {
        &app.session.as_ref().unwrap().mesh_clip_tables
    }
    for handle in models {
        let key = postretro_model::ModelHandle::from(handle.to_string());
        assert!(
            tables(committed).get(&key) == tables(oracle).get(&key),
            "{label}: clip table {handle}"
        );
    }
}

/// The fixture quad at clip-space depth 0.5, drawn through cell 0.
fn frame_hash(app: &mut App) -> u64 {
    let cells = VisibleCells::Culled(vec![0]);
    let pixels = app
        .renderer
        .as_mut()
        .unwrap()
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            Some(0),
            Mat4::IDENTITY,
            Vec3::new(0.0, 0.0, 1.0),
            &[],
            &[],
            1.0,
            ClearColor {
                r: 0.05,
                g: 0.1,
                b: 0.2,
                a: 1.0,
            },
            true,
            ShDrainBatch::default(),
        )
        .expect("the capture drains")
        .frame
        .expect("the capture records");
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Commit `case` in one fresh App and install it through the oracle in
/// another, then compare what both installed; the commit's report.
fn assert_commit_matches_oracle(test: &str, case: &CommitCase) -> Option<LevelCommitReport> {
    let pool = test_pool();
    let mut committed = gpu_app(test, &case.content_root)?;
    let (stages, mut bundle) = load_case(&mut committed, &pool, case, |inputs| {
        inputs.audio_enabled = true;
    });
    // Sound registry contents: the pool's decode registers what the
    // oracle's decode of the same content root registers. Neither test App
    // has an audio device, so the registries themselves stay empty.
    let sounds = bundle
        .presentation
        .sounds
        .take()
        .expect("a dispatch with audio decodes sounds");
    assert_eq!(
        sounds.registry_summary(),
        postretro_audio::decode_level_sounds(&case.content_root).registry_summary(),
        "{}: sound registry",
        case.label
    );
    assert_eq!(
        bundle.presentation.models.handles().collect::<Vec<_>>(),
        case.models,
        "{}: predicted models",
        case.label
    );
    let report = committed
        .commit_level_bundle(bundle)
        .unwrap_or_else(|failure| panic!("{}: the commit failed: {failure}", case.label));
    retire_app_load(stages, &mut committed);

    let mut oracle = gpu_app(test, &case.content_root)?;
    oracle_case(&mut oracle, case);

    let committed_rows = app_summary(&committed, case.models);
    let oracle_rows = app_summary(&oracle, case.models);
    for ((what, committed), (_, oracle)) in committed_rows.iter().zip(&oracle_rows) {
        assert_eq!(committed, oracle, "{}: {what}", case.label);
    }
    assert_same_clip_tables(case.label, &committed, &oracle, case.models);
    if case.frame {
        assert_eq!(
            frame_hash(&mut committed),
            frame_hash(&mut oracle),
            "{}: frames",
            case.label
        );
    }
    Some(report)
}

// Equivalence 2: on the app's fixtures (a textured level whose lightmap
// streams, a streamed-SH level, and the textured level with a model, map
// lights, fog and trigger volumes and a non-default fog scale), the load's
// stages to a bundle and its commit install what `install_level_payload`
// installs in a fresh App: the renderer's resource summary, entity-id order,
// clip tables and hit zones, sound registry, collision, streaming sessions,
// and the frame the textured fixture draws.
#[test]
fn pipeline_install_matches_main_thread_oracle() {
    let test = "pipeline_install_matches_main_thread_oracle";
    for case in [
        CommitCase::textured(),
        CommitCase::streamed_sh(),
        CommitCase::textured_with_model(),
    ] {
        let Some(report) = assert_commit_matches_oracle(test, &case) else {
            return;
        };
        assert_eq!(report.model_misses, 0, "{}: {report:?}", case.label);
        assert_eq!(
            report.uncounted_creations().total(),
            0,
            "{}: {report:?}",
            case.label
        );
    }
}

// Equivalence 2 on every compiled dev-mod map, with the dev mod's real
// registry and baked textures. A map the current loader refuses (a stale
// bake) is skipped with its reason.
#[test]
#[ignore = "requires the compiled dev-mod maps and their baked .prm sidecars"]
fn pipeline_install_matches_main_thread_oracle_on_dev_maps() {
    let test = "pipeline_install_matches_main_thread_oracle_on_dev_maps";
    let content_root = dev_content_root();
    let prm_root = dev_prm_root();
    let mut maps: Vec<_> = std::fs::read_dir(content_root.join("maps"))
        .expect("content/dev/maps exists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "prl"))
        .collect();
    maps.sort();
    let init = |app: &mut App| {
        let ctx = script_ctx(app);
        let mut primitives =
            postretro_scripting_core::primitives_registry::PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx);
        // The dev mod's init registers the built-in classname handlers itself.
        app.session.as_mut().unwrap().classname_dispatch =
            crate::scripting::builtins::ClassnameDispatch::new();
        crate::coop_fixture_tests::initialize_dev_mod(app, &primitives);
    };
    let mut checked = 0;
    for map in maps {
        let label = map.file_stem().unwrap().to_string_lossy().into_owned();
        if let Err(error) = postretro_level_loader::load_prl(map.to_str().unwrap()) {
            eprintln!("[CommitOracle] {label}: skipped, stale bake: {error}");
            continue;
        }
        let pool = test_pool();
        let case = CommitCase {
            label: "dev map",
            map: map.clone(),
            prm_root: prm_root.clone(),
            content_root: content_root.clone(),
            edit_world: None,
            edit_hand_out: None,
            models: &[],
            frame: false,
            _keep: Box::new(()),
        };
        let Some(mut committed) = gpu_app(test, &content_root) else {
            return;
        };
        init(&mut committed);
        let (stages, bundle) = load_case(&mut committed, &pool, &case, |_| {});
        let models: Vec<String> = bundle
            .presentation
            .models
            .handles()
            .map(String::from)
            .collect();
        let report = committed
            .commit_level_bundle(bundle)
            .unwrap_or_else(|failure| panic!("{label}: the commit failed: {failure}"));
        retire_app_load(stages, &mut committed);
        let Some(mut oracle) = gpu_app(test, &content_root) else {
            return;
        };
        init(&mut oracle);
        oracle_case(&mut oracle, &case);
        let handles: Vec<&str> = models.iter().map(String::as_str).collect();
        let committed_rows = app_summary(&committed, &handles);
        let oracle_rows = app_summary(&oracle, &handles);
        for ((what, committed), (_, oracle)) in committed_rows.iter().zip(&oracle_rows) {
            assert_eq!(committed, oracle, "{label}: {what}");
        }
        assert_same_clip_tables(&label, &committed, &oracle, &handles);
        assert_eq!(
            report.uncounted_creations().total(),
            0,
            "{label}: {report:?}"
        );
        eprintln!(
            "[CommitOracle] {label}: matches the oracle ({} models, {} missed, sprites created {:?})",
            models.len(),
            report.model_misses,
            report.sprite_created
        );
        checked += 1;
    }
    assert!(checked > 0, "no current dev-mod map loaded");
    eprintln!("[CommitOracle] {checked} map(s) matched");
}

// Off the main thread 2: on a fixture with a baked texture, a model and a
// non-default fog pixel scale, the commit frame creates no GPU object on
// the main thread beyond its counted fallbacks: no model missed, no extent
// or reflection tier moved, so only sprite registration (a named exception)
// may create anything. The commit parses nothing a prediction covered.
#[test]
fn pipeline_commit_creates_no_gpu_objects() {
    let test = "pipeline_commit_creates_no_gpu_objects";
    let case = CommitCase::textured_with_model();
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    let (stages, bundle) = load_case(&mut app, &pool, &case, |_| {});
    let before = gpu_creation_counts();
    let report = app
        .commit_level_bundle(bundle)
        .expect("the fixture commits");
    let created = gpu_creation_counts().since(before);
    retire_app_load(stages, &mut app);

    assert_eq!(
        report.created, created,
        "the report counts the whole commit"
    );
    assert_eq!(report.model_misses, 0, "{report:?}");
    assert_eq!(report.model_miss_created.total(), 0, "{report:?}");
    assert_eq!(report.extent_rebuilds, 0, "{report:?}");
    assert_eq!(report.reflection_tier_rebuilds, 0, "{report:?}");
    assert_eq!(report.publish_created.total(), 0, "{report:?}");
    assert_eq!(
        report.uncounted_creations().total(),
        0,
        "the commit created outside its counted fallbacks: {report:?}"
    );
    assert_eq!(
        created.total(),
        report.sprite_created.total(),
        "only sprite registration creates: {report:?}"
    );
    let summary = app
        .renderer
        .as_ref()
        .unwrap()
        .active_level_resource_summary()
        .expect("the commit published the set");
    assert!(summary.contains(MODEL), "{summary}");
    assert_eq!(
        app.level.as_ref().unwrap().fog_pixel_scale,
        FOG_SCALE,
        "the non-default fog scale reached the set's build"
    );
}

// The commit's count source reads nonzero across the main-thread oracle's
// install of the same fixture: a source that counts nothing fails here.
#[test]
fn creation_counter_counts_oracle_install() {
    let test = "creation_counter_counts_oracle_install";
    let case = CommitCase::textured_with_model();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    let before = gpu_creation_counts();
    oracle_case(&mut app, &case);
    let created = gpu_creation_counts().since(before);
    assert!(created.textures > 0, "{created:?}");
    assert!(created.buffers > 0, "{created:?}");
    assert!(created.bind_groups > 0, "{created:?}");
}

/// What a failed commit must leave untouched, read from `app`.
fn untouched_state(app: &App) -> String {
    let renderer = app.renderer.as_ref().unwrap();
    format!(
        "level={} tags={:?} source={:?} script_time={} nav={} identity={:?} collision={} \
         volumes={} active_set={:?} load={:?} boot={:?} lights={:?} fog={:?}",
        app.level.is_some(),
        app.active_level_tags,
        app.active_level_source,
        app.script_time,
        app.nav_graph.is_some(),
        app.published_level_identity,
        app.collision_world.triangle_count(),
        app.environment_volumes.len(),
        renderer.active_level_set(),
        app.level_load,
        app.boot_state,
        ids_of(app, postretro_entities::ComponentKind::Light),
        ids_of(app, postretro_entities::ComponentKind::FogVolume),
    )
}

/// A host endpoint and an in-flight catalog load, as a hosted level change
/// has when its bundle commits.
fn host_level_change(app: &mut App, map: &Path) {
    use crate::netcode::{NetEndpoint, NetRole};
    app.session.as_mut().unwrap().net_endpoint = Some(
        NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint"),
    );
    app.level_load = Some(crate::startup::InFlightLevelLoad {
        map_path: map.to_path_buf(),
        content_root: app.content_root.clone(),
        entry: crate::startup::LevelLoadEntry {
            catalog_id: Some("e2m1".to_string()),
            path: "maps/e2m1.prl".to_string(),
            name: "Next".to_string(),
            tags: vec!["next".to_string()],
            loading_tree: Vec::new(),
        },
    });
}

// Failure 4: the streaming sessions' checks run at the commit's top, before
// any mutation, against the bundle's unpublished set. A streamed-SH bundle
// whose set holds no residency snapshot, and one whose topology the SH
// controller rejects, each end as one `LevelLoadFailure` at the commit
// checks: nothing installed, no parity published, the set released, and
// the renderer's active set unchanged.
#[test]
fn streaming_session_checks_fail_before_commit() {
    let test = "streaming_session_checks_fail_before_commit";
    let pool = test_pool();
    let streamed = CommitCase::streamed_sh();
    let textured = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &streamed.content_root) else {
        return;
    };
    host_level_change(&mut app, &streamed.map);
    app.script_time = 17.0;

    // No residency snapshot: the streamed level's data over a set built for
    // a level that streams no SH.
    let (plain_stages, plain) = load_case(&mut app, &pool, &textured, |_| {});
    let (stages, mut bundle) = load_case(&mut app, &pool, &streamed, |_| {});
    let BundleSet::Resident(own_set) = bundle.set else {
        panic!("a load with a renderer builds a set");
    };
    let BundleSet::Resident(plain_set) = plain.set else {
        panic!("a load with a renderer builds a set");
    };
    bundle.set = BundleSet::Resident(plain_set);
    let before = untouched_state(&app);
    let created = gpu_creation_counts();
    let failure = app
        .commit_level_bundle(bundle)
        .expect_err("a set without a snapshot fails the commit");
    assert_eq!(failure.stage, LoadStage::Commit, "{failure}");
    assert!(
        failure.message.contains("no residency snapshot"),
        "{failure}"
    );
    assert_eq!(untouched_state(&app), before, "nothing mutated");
    assert!(
        app.published_level_identity.is_none(),
        "no parity published"
    );
    assert_eq!(gpu_creation_counts().since(created).total(), 0);
    let renderer = app.renderer.as_mut().unwrap();
    assert!(
        !renderer.resident_level_sets().contains(&plain_set),
        "the failed commit released its set"
    );
    assert!(renderer.release_level_set(own_set));
    retire_app_load(stages, &mut app);
    retire_app_load(plain_stages, &mut app);

    // The controller rejects the level's topology.
    let (stages, mut bundle) = load_case(&mut app, &pool, &streamed, |_| {});
    let BundleSet::Resident(set) = bundle.set else {
        panic!("a load with a renderer builds a set");
    };
    bundle
        .streaming
        .sh
        .as_mut()
        .expect("the fixture streams SH")
        .topology
        .break_for_tests();
    let before = untouched_state(&app);
    let failure = app
        .commit_level_bundle(bundle)
        .expect_err("a rejected topology fails the commit");
    assert_eq!(failure.stage, LoadStage::Commit, "{failure}");
    assert!(failure.message.contains("topology"), "{failure}");
    assert_eq!(untouched_state(&app), before, "nothing mutated");
    assert!(
        app.published_level_identity.is_none(),
        "no parity published"
    );
    assert!(
        !app.renderer
            .as_ref()
            .unwrap()
            .resident_level_sets()
            .contains(&set),
        "the failed commit released its set"
    );
    retire_app_load(stages, &mut app);

    // The same level with nothing broken commits, and publishes parity.
    let (stages, bundle) = load_case(&mut app, &pool, &streamed, |_| {});
    app.commit_level_bundle(bundle)
        .expect("the unbroken level commits");
    retire_app_load(stages, &mut app);
    assert!(app.published_level_identity.is_some());
    assert!(app.session.as_ref().unwrap().sh_streaming.is_some());
}

// Co-op, P14: on a host level change, parity, the relevel id and the join
// seed stay unset on every frame of the bundle phase (prep ran the range and
// collision checks there) and are set by the commit, which also installs
// the bundle's collision.
#[test]
fn host_level_change_publishes_parity_at_commit() {
    let test = "host_level_change_publishes_parity_at_commit";
    let case = CommitCase::textured();
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    host_level_change(&mut app, &case.map);
    let empty_collision = app.collision_world.triangle_count();
    let mut polls = 0;
    let (stages, bundle) = load_app_bundle(
        &mut app,
        &pool,
        &case.map,
        &case.content_root,
        |inputs| case.edit_inputs(inputs),
        |app| {
            polls += 1;
            assert!(
                app.published_level_identity.is_none(),
                "parity unset during the bundle phase"
            );
            assert_eq!(app.collision_world.triangle_count(), empty_collision);
        },
    );
    assert!(polls > 0);
    assert!(
        app.published_level_identity.is_none(),
        "unset when the bundle is ready"
    );
    let collision = (
        bundle.collision.triangle_count(),
        bundle.collision.vertex_count(),
    );
    assert!(collision.0 > 0, "prep built the collision mesh");
    app.commit_level_bundle(bundle)
        .expect("the fixture commits");
    retire_app_load(stages, &mut app);
    assert_eq!(
        app.published_level_identity.as_deref(),
        Some(level_identity(&LevelSource::Catalog("e2m1".to_string()), &app.content_root).as_str()),
        "parity published at the commit"
    );
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Catalog("e2m1".to_string())),
        "the relevel id's source"
    );
    assert_eq!(
        (
            app.collision_world.triangle_count(),
            app.collision_world.vertex_count()
        ),
        collision,
        "the bundle's collision installed"
    );
    // Parity publishes after the commit's checks, and the collision installs
    // in segment B, where the oracle installs it.
    let commit = commit_level_bundle_source();
    let checks = commit.find("self.check_level_commit(").unwrap();
    let parity = commit.find("endpoint.set_level_parity(").unwrap();
    let relevel = commit.find("endpoint.set_relevel_catalog_id(").unwrap();
    let seed = commit.find("endpoint.set_join_seed(").unwrap();
    let collision_install = commit
        .find("self.collision_world = static_collision;")
        .unwrap();
    for (at, what) in [
        (parity, "parity"),
        (relevel, "relevel id"),
        (seed, "join seed"),
    ] {
        assert!(checks < at, "{what} publishes after the commit's checks");
    }
    assert!(parity < collision_install);
}

/// The renderer tiers `tiers`' options map to.
fn renderer_tiers(
    shadow: crate::options::ShadowQuality,
    surface_depth: crate::options::SurfaceDepthQuality,
    reflection: crate::options::ReflectionQuality,
) -> postretro_renderer::LevelBuildSnapshotInputs {
    postretro_renderer::LevelBuildSnapshotInputs {
        spot_shadow_map_resolution: renderer_spot_shadow_map_resolution(shadow),
        surface_depth_quality: renderer_surface_depth_quality(surface_depth),
        reflection_quality: renderer_reflection_quality(reflection),
    }
}

/// The shadow tier's resolution the active set's summary reports.
fn active_shadow_resolution(app: &App) -> u32 {
    let summary = app
        .renderer
        .as_ref()
        .unwrap()
        .active_level_resource_summary()
        .expect("a set is active");
    let shadow = summary
        .split("shadow=(")
        .nth(1)
        .expect("the summary names the shadow tier");
    shadow[..shadow.find(',').unwrap()].parse().unwrap()
}

// P17: a tier change made on the commit frame applies after the commit. The
// options bridge has written the new tiers when the commit runs, but they
// apply after it (the frame's options step), so the commit applies none:
// the set publishes at the dispatch tiers with no reflection reconcile, and
// the renderer still holds them. Then the frame's options apply Surface
// Depth and reflections through the live path, and the shadow tier waits
// for the next level, which builds at it.
#[test]
fn commit_frame_tier_change_applies_after_commit() {
    use crate::options::{ReflectionQuality, ShadowQuality, SurfaceDepthQuality};
    let test = "commit_frame_tier_change_applies_after_commit";
    let case = CommitCase::textured();
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    let dispatched = app.session.as_ref().unwrap().player_options.clone();
    let dispatch_tiers = renderer_tiers(
        dispatched.shadow_quality,
        dispatched.surface_depth_quality,
        dispatched.reflection_quality,
    );
    let (stages, bundle) = load_case(&mut app, &pool, &case, |_| {});
    let (shadow, surface_depth, reflection) = (
        ShadowQuality::Low,
        SurfaceDepthQuality::Off,
        ReflectionQuality::Medium,
    );
    assert_ne!(shadow, dispatched.shadow_quality);
    assert_ne!(surface_depth, dispatched.surface_depth_quality);
    assert_ne!(reflection, dispatched.reflection_quality);
    {
        let options = &mut app.session.as_mut().unwrap().player_options;
        options.shadow_quality = shadow;
        options.surface_depth_quality = surface_depth;
        options.reflection_quality = reflection;
    }
    let report = app
        .commit_level_bundle(bundle)
        .expect("the fixture commits");
    retire_app_load(stages, &mut app);
    assert_eq!(report.reflection_tier_rebuilds, 0, "{report:?}");
    assert_eq!(report.publish_created.total(), 0, "{report:?}");
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    assert_eq!(
        app.renderer.as_ref().unwrap().level_build_tiers(),
        dispatch_tiers,
        "the commit applied no live tier"
    );
    assert_eq!(
        active_shadow_resolution(&app),
        dispatch_tiers.spot_shadow_map_resolution
    );

    // The frame's options, after the commit.
    app.configure_player_shadow_quality(shadow);
    app.apply_player_surface_depth_quality(surface_depth);
    app.apply_player_reflection_quality(reflection);
    let live = renderer_tiers(shadow, surface_depth, reflection);
    let held = app.renderer.as_ref().unwrap().level_build_tiers();
    assert_eq!(held.surface_depth_quality, live.surface_depth_quality);
    assert_eq!(held.reflection_quality, live.reflection_quality);
    assert_ne!(
        live.spot_shadow_map_resolution,
        dispatch_tiers.spot_shadow_map_resolution
    );
    assert_eq!(
        active_shadow_resolution(&app),
        dispatch_tiers.spot_shadow_map_resolution,
        "the shadow tier waits for the next level"
    );

    // A frame flushes the live rewrite's writes; then the next level
    // dispatches at the new tiers.
    frame_hash(&mut app);
    app.unload_level();
    commit_case(&mut app, &pool, &case);
    assert_eq!(
        active_shadow_resolution(&app),
        live.spot_shadow_map_resolution
    );
}

// P16: a scene-extent change between dispatch and commit, committed by a
// Loading paint (the renderer's own extent commit), rebuilds the bundle's
// extent-bound groups and fog target once at the commit, counted; two
// changes on different frames still rebuild once; and the published set is
// bound at the extent the renderer committed last.
#[test]
fn extent_change_before_commit_rebuilds_once() {
    let test = "extent_change_before_commit_rebuilds_once";
    let case = CommitCase::textured();
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    for sizes in [&[(96u32, 80u32)][..], &[(96, 80), (128, 96)][..]] {
        app.unload_level();
        {
            let renderer = app.renderer.as_mut().unwrap();
            renderer.record_surface_size(64, 64);
            renderer.commit_extents();
        }
        let (stages, bundle) = load_case(&mut app, &pool, &case, |_| {});
        for &(width, height) in sizes {
            // A Loading paint commits the recorded size before it draws.
            let renderer = app.renderer.as_mut().unwrap();
            renderer.record_surface_size(width, height);
            renderer.commit_extents();
        }
        let report = app
            .commit_level_bundle(bundle)
            .expect("the fixture commits");
        retire_app_load(stages, &mut app);
        assert_eq!(report.extent_rebuilds, 1, "{sizes:?}: {report:?}");
        assert!(report.publish_created.total() > 0, "{report:?}");
        assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
        let scene = app.renderer.as_ref().unwrap().scene_extent();
        let &(width, height) = sizes.last().unwrap();
        assert_eq!((scene.width, scene.height), (width, height));
        let summary = app
            .renderer
            .as_ref()
            .unwrap()
            .active_level_resource_summary()
            .unwrap();
        assert!(
            summary.contains(&format!("extent=({scene:?}")),
            "the published set binds the committed extent: {summary}"
        );
    }
}

// P23: a resize recorded after the last Loading paint, not yet committed,
// is the commit frame's own: the commit reads the extent the renderer
// committed, so it counts no rebuild, and the frame's ordinary extent
// commit rebuilds the published set's extent-bound groups.
#[test]
fn resize_after_last_loading_paint_counts_no_rebuild() {
    let test = "resize_after_last_loading_paint_counts_no_rebuild";
    let case = CommitCase::textured();
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    let (stages, bundle) = load_case(&mut app, &pool, &case, |_| {});
    let built_at = app.renderer.as_ref().unwrap().scene_extent();
    app.renderer.as_mut().unwrap().record_surface_size(96, 80);
    let report = app
        .commit_level_bundle(bundle)
        .expect("the fixture commits");
    retire_app_load(stages, &mut app);
    assert_eq!(report.extent_rebuilds, 0, "{report:?}");
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    let renderer = app.renderer.as_mut().unwrap();
    assert_eq!(renderer.scene_extent(), built_at);
    let rebuilds = renderer.extent_rebuilds();
    // The commit frame's ordinary extent commit.
    renderer.commit_extents();
    let scene = renderer.scene_extent();
    assert_eq!((scene.width, scene.height), (96, 80));
    assert_eq!(
        renderer.extent_rebuilds(),
        rebuilds,
        "the resize counts none"
    );
    let summary = renderer.active_level_resource_summary().unwrap();
    assert!(
        summary.contains(&format!("extent=({scene:?}")),
        "the ordinary resize rebuilt the published set: {summary}"
    );
}

/// The forced-miss level: a placement only the assembled level gains, after
/// the model prediction read hand-out 4.
fn add_unpredicted_rifle(world: &mut LevelWorld) {
    world.map_entities.push(model_record(RIFLE));
}

// Models 1, end to end through the commit: the commit's real sweep holds a
// model the prediction lacked (a placement the level gained after the
// prediction). The commit loads it synchronously into the published set,
// counts one miss, logs it once, installs its clip table and hit zones as
// the oracle's parse would, and still loads every predicted model from its
// prepared products.
#[test]
fn forced_model_miss_loads_and_logs_once() {
    let test = "forced_model_miss_loads_and_logs_once";
    let mut case = CommitCase::textured_with_model();
    case.edit_world = Some(add_unpredicted_rifle);
    case.models = &[MODEL, RIFLE];
    let pool = test_pool();
    let Some(mut app) = gpu_app(test, &case.content_root) else {
        return;
    };
    let (stages, bundle) = load_case(&mut app, &pool, &case, |_| {});
    assert_eq!(
        bundle.presentation.models.handles().collect::<Vec<_>>(),
        [MODEL],
        "the prediction lacks the rifle"
    );
    let capture = LogCapture::start();
    let report = app
        .commit_level_bundle(bundle)
        .expect("the fixture commits");
    retire_app_load(stages, &mut app);
    assert_eq!(report.model_misses, 1, "{report:?}");
    assert!(report.model_miss_created.total() > 0, "{report:?}");
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    capture.assert_logged_once(
        log::Level::Warn,
        "model prediction missed 'models/ar_4/model.gltf'",
    );
    let summary = app
        .renderer
        .as_ref()
        .unwrap()
        .active_level_resource_summary()
        .unwrap();
    assert!(
        summary.contains(RIFLE) && summary.contains(MODEL),
        "{summary}"
    );
    let session = app.session.as_ref().unwrap();
    let rifle = postretro_model::ModelHandle::from(RIFLE.to_string());
    let oracle = postretro_model::gltf_loader::load_model(&case.content_root.join(RIFLE));
    let entry = crate::startup::lifecycle::model_clip_entry(&oracle);
    assert_eq!(
        format!("{:?}", session.mesh_clip_tables.model_bounds(&rifle)),
        format!("{:?}", entry.bounds),
        "the miss's clip table is the parse's"
    );
    assert!(
        session.hit_zone_store.get(&rifle).is_some(),
        "its hit zones"
    );
}

// Equivalence 3, the commit's source half of the install-order guards: every
// check that can fail runs before the first mutation; light entity ids are
// assigned before segment B creates the fog entities; and the commit applies
// no player tier (P17) and parses nothing a prepared product covers.
#[test]
fn commit_runs_its_checks_before_any_mutation_and_shares_the_install_stages() {
    let commit = commit_level_bundle_source();
    let checks = commit
        .find("self.check_level_commit(")
        .expect("the commit checks first");
    let failure_return = commit
        .find("return Err(failure);")
        .expect("a failed check returns");
    assert!(checks < failure_return);
    for mutation in [
        "self.view_feel_state =",
        "endpoint.set_level_parity(",
        "self.nav_graph =",
        "renderer.publish_level_set(",
        "self.collision_world =",
        "self.level = Some(",
        "self.install_environment_volumes(",
        "self.level_timings.record(",
    ] {
        let at = commit
            .find(mutation)
            .unwrap_or_else(|| panic!("the commit reaches {mutation}"));
        assert!(
            failure_return < at,
            "the commit's checks and failure return precede {mutation}"
        );
    }
    let light_bridge = commit.find("self.install_level_light_bridge(").unwrap();
    let fog = commit.find("install_world_cpu_with_models(").unwrap();
    assert!(light_bridge < fog, "light ids before fog ids");
    for forbidden in [
        "apply_player_",
        "configure_player_shadow_quality",
        "install_textures(",
        "normalize_world_uvs(",
        "install_level_geometry(",
        "install_fluid_surfaces(",
        "commit_render_extents(",
    ] {
        assert!(
            !commit.contains(forbidden),
            "the commit never calls {forbidden}"
        );
    }
}
