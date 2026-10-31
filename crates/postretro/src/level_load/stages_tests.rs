//! The load-stage handle: prep hands each GPU-bound product to the set build
//! as soon as it exists, the load ends in one bundle whose set equals the
//! main-thread oracle's, the main thread creates nothing, one failure per
//! load, cancel drops every builder and releases a received set, and upload
//! credits its share of the bar.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Off the main thread, Equivalence, Failure, Cancellation, Progress) · research.md §9 (P10, P13, P15)

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_loader::{HandOut, HandOutStage, LevelLoadProgress, LoadPhase, ShDrainBatch};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::test_support::{
    HOLD_TIMEOUT, PrepProbe, PrepTestHooks, inputs_for, no_prm_root, offscreen_renderer_or_skip,
    test_pool, write_late_rejected_level, write_textured_level,
};
use super::upload::level_build_snapshot;
use super::*;
use crate::load_pool::LoadPool;
use crate::render::{ClearColor, LevelBuildPart, Renderer, ShSampleRegionSets};
use postretro_renderer::{PreviousLevelSet, gpu_creation_counts};

/// A set build waits for the install pipelines (the upload thread's first
/// job) on a fresh renderer, so a whole load gets longer than a prep hold.
pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// A small skinned model in the dev mod, as a `prop_mesh` names it.
const MODEL: &str = "models/pose-modifier-fixture/joint_zones.gltf";

fn dev_content_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

/// Start `inputs`' stages with a set build begun on `renderer` at the
/// inputs' tiers, planning against the renderer's limits.
pub(super) fn start_with_renderer(
    renderer: &mut Renderer,
    pool: &LoadPool,
    mut inputs: LevelPrepInputs,
) -> (LevelLoadStages, Arc<LevelLoadProgress>) {
    inputs.device_limits = renderer.install_device_limits();
    let progress = Arc::new(LevelLoadProgress::new());
    let upload = renderer.begin_level_set_build(level_build_snapshot(inputs.tiers));
    let stages = start_level_load_stages(pool.group(), inputs, Some(upload), Arc::clone(&progress));
    (stages, progress)
}

/// Poll `stages` to its one outcome, failing past [`LOAD_TIMEOUT`].
pub(super) fn poll_outcome(
    stages: &mut LevelLoadStages,
    mut renderer: Option<&mut Renderer>,
) -> Result<LevelBundle, LevelLoadFailure> {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    loop {
        if let Some(outcome) = stages.try_take(renderer.as_deref_mut()) {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "the load never yielded: {stages:?}, prep {:?}, upload parts {:?}",
            stages.probe().state(),
            stages.probe().upload_parts_done()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Polling `stages` for a while reports nothing more.
fn assert_reports_nothing_more(stages: &mut LevelLoadStages, renderer: &mut Renderer) {
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        assert!(stages.try_take(Some(renderer)).is_none(), "a second report");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Poll a retirement to its end, failing past [`LOAD_TIMEOUT`].
pub(super) fn retire(mut retirement: LoadRetirement) {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while !retirement.try_finish() {
        assert!(Instant::now() < deadline, "the load never retired");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn resident(bundle: &LevelBundle) -> crate::render::LevelSetId {
    match bundle.set {
        BundleSet::Resident(id) => id,
        BundleSet::PrepOnly(_) => panic!("a load with a renderer builds a set"),
    }
}

/// Apply `tiers` to `renderer` as the oracle install's renderer stage does.
fn apply_tiers(renderer: &mut Renderer, tiers: LevelTiers) {
    let snapshot = level_build_snapshot(tiers);
    renderer.set_spot_shadow_map_resolution(snapshot.spot_shadow_map_resolution);
    renderer.set_surface_depth_quality(snapshot.surface_depth_quality);
    renderer.set_reflection_quality(snapshot.reflection_quality);
}

/// The world's quad at clip-space depth 0.5, drawn through cell 0.
fn capture(renderer: &mut Renderer) -> Vec<u8> {
    let cells = VisibleCells::Culled(vec![0]);
    renderer
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
        .expect("the capture records")
}

fn frame_hash(pixels: &[u8]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Today's main-thread install of `map` through the renderer, as the oracle's
/// renderer stage and presentation run it: textures, UV normalize, geometry,
/// fluids, the fog pixel scale and cell masks, then each model.
fn install_oracle(renderer: &mut Renderer, map: &Path, prm_cache_root: &Path, models: &[&str]) {
    let mut world = postretro_level_loader::load_prl(map.to_str().unwrap()).expect("oracle parse");
    let materials = level_texture_materials(&world.texture_names);
    renderer.install_textures(
        &world.texture_names,
        &world.texture_cache_keys,
        prm_cache_root,
        &materials,
    );
    renderer.normalize_world_uvs(&mut world);
    let payloads = world.take_gpu_lighting_payloads();
    renderer
        .install_level_geometry(
            &crate::render::level_world_to_geometry(&world, &materials),
            payloads,
        )
        .expect("the oracle install accepts the level");
    renderer.install_fluid_surfaces(
        &world.environment_volumes.volumes,
        world.reflection_probes.as_ref(),
    );
    renderer.set_fog_pixel_scale(world.fog_pixel_scale);
    renderer.install_fog_cell_masks_for_level(world.fog_cell_masks.clone());
    let content_root = dev_content_root();
    for handle in models {
        let open_path = content_root.join(handle);
        let parsed = postretro_model::gltf_loader::load_model(&open_path);
        assert!(
            renderer
                .upload_parsed_skinned_model(handle, &open_path, &parsed, prm_cache_root)
                .is_some(),
            "the oracle uploads {handle}"
        );
    }
}

/// A `prop_mesh` naming [`MODEL`], added to prep's copy of hand-out 4.
fn add_model_placement(hand_out: &mut HandOut) {
    if let HandOut::Entities(entities) = hand_out {
        Arc::make_mut(entities).map_entities.push(MapEntityRecord {
            classname: "prop_mesh".to_string(),
            origin: [0.25, 0.25, 0.5],
            key_values: vec![("model".to_string(), MODEL.to_string())],
            ..Default::default()
        });
    }
}

// Off-main 5: with the parse held right after hand-out 2, texture prep
// finishes and the textures part's upload job completes on the upload thread
// before the parse reads another section, so the upload starts before the
// parse returns. A hold that times out fails.
#[test]
fn staged_parse_overlaps_prep_and_upload() {
    let Some(mut renderer) = offscreen_renderer_or_skip("staged_parse_overlaps_prep_and_upload")
    else {
        return;
    };
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    let overlapped = Arc::new(AtomicBool::new(false));
    let hold_reached = Arc::new(AtomicBool::new(false));
    {
        let overlapped = Arc::clone(&overlapped);
        let hold_reached = Arc::clone(&hold_reached);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage != HandOutStage::Geometry {
                return;
            }
            hold_reached.store(true, Ordering::Release);
            let done = probe.wait_for(|probe| {
                probe.state().finished.contains(&PrepKind::Textures)
                    && probe
                        .upload_parts_done()
                        .contains(&LevelBuildPart::Textures)
            });
            assert_eq!(probe.state().last_hand_out, Some(HandOutStage::Geometry));
            assert!(!probe.state().parse_accepted);
            overlapped.store(done, Ordering::Release);
        }));
    }
    let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
    let bundle = poll_outcome(&mut stages, Some(&mut renderer)).expect("the textured level loads");
    assert!(
        hold_reached.load(Ordering::Acquire),
        "the parse reached hand-out 2"
    );
    assert!(
        overlapped.load(Ordering::Acquire),
        "texture prep and its upload part finished while the parse was held (a timeout fails)"
    );
    let id = resident(&bundle);
    assert!(renderer.resident_level_sets().contains(&id));
    retire(stages.cancel(Some(&mut renderer)));
}

// Equiv 2, first half: on the app's fixtures, the bundle's set, published,
// has the oracle install's resource summary, and draws the same frame where
// the fixture renders one: a textured level whose lightmap streams, a
// streamed-SH level, and a level with a model.
#[test]
fn load_stages_bundle_matches_the_oracle_install() {
    let test = "load_stages_bundle_matches_the_oracle_install";
    struct Case {
        label: &'static str,
        map: PathBuf,
        prm_root: PathBuf,
        content_root: Option<PathBuf>,
        edit_hand_out: Option<fn(&mut HandOut)>,
        models: &'static [&'static str],
        frame: bool,
        _keep: Box<dyn std::any::Any>,
    }
    let (textured, textured_prm) = write_textured_level();
    let (sh_dir, streamed_sh) =
        crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
    let lightmap = crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write();
    let dev_prm = crate::startup::worker::derive_prm_root_dev_layout(&dev_content_root());
    let cases = vec![
        Case {
            label: "textured",
            map: textured.path().to_path_buf(),
            prm_root: textured_prm.path().to_path_buf(),
            content_root: None,
            edit_hand_out: None,
            models: &[],
            frame: true,
            _keep: Box::new((textured, textured_prm)),
        },
        Case {
            label: "streamed-sh",
            map: streamed_sh,
            prm_root: no_prm_root(),
            content_root: None,
            edit_hand_out: None,
            models: &[],
            frame: false,
            _keep: Box::new(sh_dir),
        },
        Case {
            label: "model",
            map: lightmap.path().to_path_buf(),
            prm_root: dev_prm,
            content_root: Some(dev_content_root()),
            edit_hand_out: Some(add_model_placement),
            models: &[MODEL],
            frame: false,
            _keep: Box::new(lightmap),
        },
    ];
    for case in cases {
        let label = case.label;
        let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
            return;
        };
        let pool = test_pool();
        let make_inputs = || {
            let mut inputs = inputs_for(&case.map, &case.prm_root);
            if let Some(content_root) = &case.content_root {
                inputs.content_root = content_root.clone();
            }
            inputs.hooks.edit_hand_out = case.edit_hand_out;
            inputs
        };
        let inputs = make_inputs();
        apply_tiers(&mut renderer, inputs.tiers);

        install_oracle(&mut renderer, &case.map, &case.prm_root, case.models);
        let oracle = renderer
            .active_level_resource_summary()
            .expect("the oracle published a set");
        let oracle_frame = case.frame.then(|| frame_hash(&capture(&mut renderer)));

        let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
        let bundle = poll_outcome(&mut stages, Some(&mut renderer))
            .unwrap_or_else(|failure| panic!("{label}: the staged load failed: {failure}"));
        let mut stages = Some(stages);
        assert_eq!(
            bundle.presentation.models.handles().collect::<Vec<_>>(),
            case.models,
            "{label}: predicted models"
        );
        renderer.publish_level_set(resident(&bundle), PreviousLevelSet::Retire);
        assert_eq!(
            renderer.active_level_resource_summary().as_deref(),
            Some(oracle.as_str()),
            "{label}: resource summaries"
        );
        if let Some(oracle_frame) = oracle_frame {
            assert_eq!(
                frame_hash(&capture(&mut renderer)),
                oracle_frame,
                "{label}: frames"
            );
        }
        if !case.models.is_empty() {
            assert!(oracle.contains(MODEL), "{label}: {oracle}");
            // The model missed the empty cache; committed, it enters it, and
            // a second load hits it and builds the same set.
            renderer.commit_model_cache(resident(&bundle));
            if let Some(stages) = stages.take() {
                retire(stages.cancel(Some(&mut renderer)));
            }
            let (mut warm, _progress) = start_with_renderer(&mut renderer, &pool, make_inputs());
            let bundle = poll_outcome(&mut warm, Some(&mut renderer))
                .unwrap_or_else(|failure| panic!("{label}: the warm load failed: {failure}"));
            assert!(
                bundle
                    .presentation
                    .models
                    .iter()
                    .all(|model| model.cache == super::ModelCacheUse::Hit),
                "{label}: the warm load hits: {:?}",
                bundle.presentation.models
            );
            renderer.publish_level_set(resident(&bundle), PreviousLevelSet::Retire);
            assert_eq!(
                renderer.active_level_resource_summary().as_deref(),
                Some(oracle.as_str()),
                "{label}: a hit's resource summary"
            );
            stages = Some(warm);
        }
        if let Some(stages) = stages {
            retire(stages.cancel(Some(&mut renderer)));
        }
    }
}

// Off-main: the main thread's half of a load (dispatch, every poll, the
// set's receipt and the bundle) creates no GPU object; the upload thread
// created the set.
#[test]
fn load_stage_main_thread_half_creates_nothing() {
    let (level, _prm_root) = write_textured_level();
    let pool = test_pool();
    let mut app = crate::startup::lifecycle::tests::test_app();
    app.renderer = offscreen_renderer_or_skip("load_stage_main_thread_half_creates_nothing");
    if app.renderer.is_none() {
        return;
    }
    let content_root = level.path().parent().unwrap().to_path_buf();
    let progress = Arc::new(LevelLoadProgress::new());
    let before = gpu_creation_counts();
    let mut stages = app
        .begin_level_load_stages(
            pool.group(),
            level.path().to_path_buf(),
            content_root,
            Arc::clone(&progress),
        )
        .expect("a renderer and a session dispatch a load");
    let bundle =
        poll_outcome(&mut stages, app.renderer.as_mut()).expect("the textured level loads");
    let id = resident(&bundle);
    let created = gpu_creation_counts().since(before);
    assert_eq!(
        created.total(),
        0,
        "dispatch, polls and receipt created on the main thread: {created:?}"
    );
    let renderer = app.renderer.as_mut().unwrap();
    assert!(renderer.resident_level_sets().contains(&id));
    assert!(renderer.release_level_set(id));
    assert_eq!(gpu_creation_counts().since(before).total(), 0);
    retire(stages.cancel(app.renderer.as_mut()));
}

// P10: a real upload-thread failure (a second textures part) ends the load
// as one failure at the upload stage. Prep stops, nothing else is reported,
// and no set stays resident.
#[test]
fn upload_failure_ends_the_load_once() {
    let Some(mut renderer) = offscreen_renderer_or_skip("upload_failure_ends_the_load_once") else {
        return;
    };
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    inputs.hooks.upload_textures_twice = true;
    let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
    let probe = stages.builder_probe().expect("the load builds a set");
    let failure = match poll_outcome(&mut stages, Some(&mut renderer)) {
        Ok(_) => panic!("a load whose build failed yielded a bundle"),
        Err(failure) => failure,
    };
    assert_eq!(failure.stage, LoadStage::Upload, "{failure}");
    assert!(failure.message.contains("textures twice"), "{failure}");
    assert_reports_nothing_more(&mut stages, &mut renderer);
    assert!(renderer.resident_level_sets().is_empty());
    retire(stages.cancel(Some(&mut renderer)));
    assert!(probe.is_released(), "every builder clone dropped");
}

// P10: prep and the upload failing together report one failure. With both
// settled before the main thread polls, prep's wins; and an upload failure
// polled while the parse's verdict is pending waits for it, so a late parse
// rejection is the one report (P18).
#[test]
fn prep_and_upload_failing_together_report_once() {
    let test = "prep_and_upload_failing_together_report_once";
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
            return;
        };
        let (level, prm_root) = write_textured_level();
        let pool = test_pool();
        let mut inputs = inputs_for(level.path(), prm_root.path());
        inputs.hooks = PrepTestHooks {
            upload_textures_twice: true,
            fail: vec![(PrepKind::Collision, "injected collision failure")],
            // The prep failure lands only once the build has failed.
            before_injected_failure: Some(Arc::new(|probe: &PrepProbe| {
                assert!(probe.wait_for(PrepProbe::upload_failed));
            })),
            ..Default::default()
        };
        let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
        let probe = stages.probe();
        assert!(
            probe.wait_for(|probe| probe.upload_failed() && probe.state().settled),
            "both stages failed before the first poll"
        );
        let failure = match poll_outcome(&mut stages, Some(&mut renderer)) {
            Ok(_) => panic!("a failed load yielded a bundle"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.stage,
            LoadStage::Prep(PrepKind::Collision),
            "{failure}"
        );
        assert_reports_nothing_more(&mut stages, &mut renderer);
        assert!(renderer.resident_level_sets().is_empty());
        retire(stages.cancel(Some(&mut renderer)));
    }
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
            return;
        };
        let level = write_late_rejected_level();
        let pool = test_pool();
        let mut inputs = inputs_for(level.path(), &no_prm_root());
        inputs.hooks.upload_textures_twice = true;
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let held_tx = Mutex::new(held_tx);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage != HandOutStage::Entities {
                return;
            }
            // Held before the verdict until the build failed and the main
            // thread polled it.
            assert!(probe.wait_for(PrepProbe::upload_failed));
            let _ = held_tx.lock().unwrap().send(());
            let _ = release_rx.lock().unwrap().recv_timeout(HOLD_TIMEOUT);
        }));
        let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
        held_rx
            .recv_timeout(HOLD_TIMEOUT)
            .expect("the parse held before its verdict with the build failed");
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            assert!(
                stages.try_take(Some(&mut renderer)).is_none(),
                "an upload failure was reported before the parse's verdict"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        release_tx.send(()).unwrap();
        let failure = match poll_outcome(&mut stages, Some(&mut renderer)) {
            Ok(_) => panic!("a rejected level yielded a bundle"),
            Err(failure) => failure,
        };
        assert_eq!(failure.stage, LoadStage::Parse, "{failure}");
        assert_reports_nothing_more(&mut stages, &mut renderer);
        retire(stages.cancel(Some(&mut renderer)));
    }
}

// Cancel drops every builder clone (the slot prep's tasks submit through and
// the handle's) and reports nothing: mid-parse, with a part already queued;
// and after the set arrived, which it releases.
#[test]
fn cancelled_load_stages_release_the_set_and_drop_every_builder() {
    let test = "cancelled_load_stages_release_the_set_and_drop_every_builder";
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
            return;
        };
        let (level, prm_root) = write_textured_level();
        let pool = test_pool();
        let mut inputs = inputs_for(level.path(), prm_root.path());
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let held_tx = Mutex::new(held_tx);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage != HandOutStage::Geometry {
                return;
            }
            assert!(probe.wait_for(|probe| {
                probe
                    .upload_parts_done()
                    .contains(&LevelBuildPart::Textures)
            }));
            let _ = held_tx.lock().unwrap().send(());
            let _ = release_rx.lock().unwrap().recv_timeout(HOLD_TIMEOUT);
        }));
        let (stages, progress) = start_with_renderer(&mut renderer, &pool, inputs);
        let probe = stages.builder_probe().expect("the load builds a set");
        held_rx
            .recv_timeout(HOLD_TIMEOUT)
            .expect("the parse held with the textures part done");
        let retirement = stages.cancel(Some(&mut renderer));
        release_tx.send(()).unwrap();
        retire(retirement);
        assert!(probe.is_released(), "every builder clone dropped");
        assert!(renderer.resident_level_sets().is_empty());
        assert!(!progress.phase(LoadPhase::Upload).is_finished());
    }
    {
        let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
            return;
        };
        let (level, prm_root) = write_textured_level();
        let pool = test_pool();
        let inputs = inputs_for(level.path(), prm_root.path());
        let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
        let probe = stages.builder_probe().expect("the load builds a set");
        stages.withhold_prep(true);
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let id = loop {
            assert!(
                stages.try_take(Some(&mut renderer)).is_none(),
                "a load reported with prep withheld"
            );
            if let Some(id) = stages.received_set() {
                break id;
            }
            assert!(Instant::now() < deadline, "the set never arrived");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(renderer.resident_level_sets(), [id]);
        let before = gpu_creation_counts();
        let retirement = stages.cancel(Some(&mut renderer));
        assert!(
            renderer.resident_level_sets().is_empty(),
            "cancel released the received set"
        );
        assert_eq!(gpu_creation_counts().since(before).total(), 0);
        retire(retirement);
        assert!(probe.is_released(), "every builder clone dropped");
    }
}

// Progress 1 for upload: the phase plans its parts once the model
// prediction lands, credits each part the upload thread finishes (the
// textures part while the parse is still held before its verdict), and is
// whole when the set arrives; the load stays below 1.0 for settling.
#[test]
fn load_progress_rises_through_upload() {
    let Some(mut renderer) = offscreen_renderer_or_skip("load_progress_rises_through_upload")
    else {
        return;
    };
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    let progress = Arc::new(LevelLoadProgress::new());
    let at_hold = Arc::new(Mutex::new(None));
    {
        let progress = Arc::clone(&progress);
        let at_hold = Arc::clone(&at_hold);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage != HandOutStage::Final {
                return;
            }
            if probe.wait_for(|probe| {
                probe.state().finished.contains(&PrepKind::ModelPrediction)
                    && probe
                        .upload_parts_done()
                        .contains(&LevelBuildPart::Textures)
            }) {
                *at_hold.lock().unwrap() = Some((
                    progress.phase(LoadPhase::Upload).counts(),
                    progress.fraction(),
                ));
            }
        }));
    }
    inputs.device_limits = renderer.install_device_limits();
    let upload = renderer.begin_level_set_build(level_build_snapshot(inputs.tiers));
    let mut stages =
        start_level_load_stages(pool.group(), inputs, Some(upload), Arc::clone(&progress));
    let mut samples = vec![progress.fraction()];
    let deadline = Instant::now() + LOAD_TIMEOUT;
    let bundle = loop {
        if let Some(outcome) = stages.try_take(Some(&mut renderer)) {
            break outcome.expect("the textured level loads");
        }
        samples.push(progress.fraction());
        assert!(Instant::now() < deadline, "the load never yielded");
        std::thread::sleep(Duration::from_millis(1));
    };
    samples.push(progress.fraction());
    assert!(
        samples.windows(2).all(|pair| pair[1] >= pair[0]),
        "the load's progress never decreases: {samples:?}"
    );
    let ((done, planned), held_fraction) = at_hold
        .lock()
        .unwrap()
        .expect("the hold before the verdict sampled progress");
    // Textures, geometry and `finish`; no model predicted.
    assert_eq!(planned, 3);
    assert_eq!(done, 1, "the textures part, before the verdict");
    assert!(held_fraction < 1.0);
    let upload = progress.phase(LoadPhase::Upload);
    assert!(upload.is_finished());
    assert_eq!(upload.counts(), (3, 3));
    let expected = (postretro_level_loader::LOAD_PHASE_WEIGHT_PARSE
        + postretro_level_loader::LOAD_PHASE_WEIGHT_PREP
        + postretro_level_loader::LOAD_PHASE_WEIGHT_UPLOAD) as f32;
    assert!(
        (progress.fraction() - expected).abs() < 1e-5,
        "parse, prep and upload whole, settling not: {}",
        progress.fraction()
    );
    assert!(progress.fraction() < 1.0);
    renderer.release_level_set(resident(&bundle));
    retire(stages.cancel(Some(&mut renderer)));
}

// With no renderer, the load runs prep only: the bundle keeps prep's
// GPU-bound products, and the upload phase, with no work, is whole on entry.
// A dispatch through the App without a renderer still starts nothing.
#[test]
fn renderer_less_dispatch_yields_a_prep_only_bundle() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let progress = Arc::new(LevelLoadProgress::new());
    let mut stages = start_level_load_stages(
        pool.group(),
        inputs_for(level.path(), prm_root.path()),
        None,
        Arc::clone(&progress),
    );
    assert!(progress.phase(LoadPhase::Upload).is_finished());
    let bundle = poll_outcome(&mut stages, None).expect("the textured level prepares");
    let BundleSet::PrepOnly(gpu) = &bundle.set else {
        panic!("a renderer-less load builds no set");
    };
    assert!(matches!(
        gpu.textures.entries.as_slice(),
        [postretro_render_cpu::texture_prep::PreparedTexture::Bundle { .. }]
    ));
    assert_eq!(gpu.diffuse_dims, [(32, 16)]);
    assert_eq!(
        bundle.world.gpu_lighting_payloads,
        Default::default(),
        "assembly moved the lighting payloads out of the level"
    );
    let mut monolithic = postretro_level_loader::load_prl(level.path().to_str().unwrap()).unwrap();
    assert_eq!(
        gpu.gpu_lighting_payloads,
        monolithic.take_gpu_lighting_payloads(),
        "the prep-only products keep the level's lighting payloads"
    );
    // The commit's main-pinned products ride the bundle with the dispatch
    // tiers.
    assert_eq!(bundle.tiers, gpu.tiers);
    assert!(
        bundle.collision.triangle_count() > 0,
        "the quad's collision"
    );
    assert!(
        bundle.streaming.lightmap.is_some(),
        "the fixture streams lightmaps"
    );
    assert!(bundle.timings.parse > Duration::ZERO);
    assert!(stages.try_take(None).is_none(), "one bundle");
    retire(stages.cancel(None));

    let mut app = crate::startup::lifecycle::tests::test_app();
    app.renderer = None;
    assert!(
        app.begin_level_load_stages(
            pool.group(),
            level.path().to_path_buf(),
            level.path().parent().unwrap().to_path_buf(),
            Arc::new(LevelLoadProgress::new()),
        )
        .is_none(),
        "no renderer: no dispatch, as the capture"
    );
}
