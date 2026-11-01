//! The shared model cache: the content key tracks every file the parse and
//! the upload read and every install parameter, and nothing else; a
//! same-content load hits every model with no parse and no upload; a release
//! frees nothing, a commit frees what nothing references, and a failed or
//! cancelled load leaves the cache as it found it; a cancelled load's late
//! upload never reaches the next load's first lookup; a dropped renderer
//! takes its cache with it; the commit's synchronous miss loads, counts and
//! logs once; and an edited model misses while the old set keeps its entry.
//! See: context/plans/done/level-load-pipeline/index.md (Model cache, Models rows) · research.md §9 (P7, P9, P11, P29, P30)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postretro_model::gltf_loader::{load_model, read_model_sources};
use postretro_render_cpu::surface_depth::SurfaceDepthQuality as RendererSurfaceDepthQuality;
use postretro_render_cpu::texture_prep::{ModelSidecars, model_material_sidecar_paths};
use postretro_renderer::PreviousLevelSet;
use postretro_test_log_capture::LogCapture;

use super::model_key::model_content_key;
use super::model_prediction::predict_level_models;
use super::model_tests::{
    dev_content_root, dev_descriptors, real_sweep, record, write_level_with_entities,
};
use super::stages_tests::{poll_outcome, resident, retire, start_with_renderer};
use super::test_support::{
    HOLD_TIMEOUT, PrepProbe, inputs_for, offscreen_renderer_or_skip, prediction_inputs, test_pool,
    write_diffuse_prm,
};
use super::*;
use crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl;
use crate::load_pool::LoadPool;
use crate::render::{LevelBuildPart, ModelCacheCommit, ModelContentKey, Renderer};
use crate::scripting_systems::hit_zones::ModelHitZones;

/// A small skinned model with hit zones, all buffers in `data:` URIs.
const ZONES: &str = "models/pose-modifier-fixture/joint_zones.gltf";
/// A textured rifle: an external buffer and four base-color PNGs.
const RIFLE: &str = "models/ar_4/model.gltf";
/// A textured, animated enemy.
const KNIGHT: &str = "models/reference_enemy_kaykit_knight/scene.gltf";
/// A prop no dev descriptor names.
const PROP: &str = "models/decraniated_low_poly_retro_pixel/scene.gltf";

fn dev_prm_root() -> PathBuf {
    crate::startup::worker::derive_prm_root_dev_layout(&dev_content_root())
}

/// A streamed-lightmap level placing one `prop_mesh` per handle.
fn level_with_props(handles: &[&str]) -> StreamedLightmapPrl {
    write_level_with_entities(
        handles
            .iter()
            .enumerate()
            .map(|(index, handle)| record("prop_mesh", &[("model", handle)], index))
            .collect(),
    )
}

fn key_of(path: &Path, prm_root: &Path, params: ModelInstallParams) -> ModelContentKey {
    let sources = read_model_sources(path).expect("the model's files read");
    let sidecars = ModelSidecars::read(sources.material_keys(), prm_root);
    model_content_key(&sources, &sidecars, params)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, to.join(path.file_name().unwrap())).unwrap();
        }
    }
}

/// Run `edit` on `path`, check the key moved, restore the file and check it
/// came back.
fn assert_edit_misses(
    label: &str,
    path: &Path,
    edit: impl FnOnce(&mut Vec<u8>),
    key: impl Fn() -> ModelContentKey,
    base: ModelContentKey,
) {
    let original = std::fs::read(path).unwrap();
    let mut edited = original.clone();
    edit(&mut edited);
    std::fs::write(path, &edited).unwrap();
    assert_ne!(key(), base, "{label}: an edit misses");
    std::fs::write(path, &original).unwrap();
    assert_eq!(key(), base, "{label}: restored content hits again");
}

/// The prep inputs for loading `map` with its models under `content_root`,
/// no audio, at `tiers` (else the defaults).
fn load_inputs(
    map: &Path,
    content_root: &Path,
    prm_root: &Path,
    tiers: Option<LevelTiers>,
) -> LevelPrepInputs {
    let mut inputs = inputs_for(map, prm_root);
    inputs.content_root = content_root.to_path_buf();
    inputs.audio_enabled = false;
    if let Some(tiers) = tiers {
        inputs.tiers = tiers;
    }
    inputs
}

/// Load to a bundle on `renderer`.
fn load(
    renderer: &mut Renderer,
    pool: &LoadPool,
    inputs: LevelPrepInputs,
) -> (LevelLoadStages, LevelBundle) {
    let (mut stages, _progress) = start_with_renderer(renderer, pool, inputs);
    let bundle = poll_outcome(&mut stages, Some(renderer))
        .unwrap_or_else(|failure| panic!("the load failed: {failure}"));
    (stages, bundle)
}

/// The commit's renderer half, as task 11 will run it: publish the bundle's
/// set, retiring the active one, then the cache step.
fn commit(renderer: &mut Renderer, bundle: &LevelBundle) -> ModelCacheCommit {
    let id = resident(bundle);
    renderer.publish_level_set(id, PreviousLevelSet::Retire);
    renderer
        .commit_model_cache(id)
        .expect("the committed set is known")
}

fn cache_uses(bundle: &LevelBundle) -> Vec<(String, ModelCacheUse)> {
    bundle
        .presentation
        .models
        .iter()
        .map(|model| (model.handle.clone(), model.cache))
        .collect()
}

fn all(handles: &[&str], cache: ModelCacheUse) -> Vec<(String, ModelCacheUse)> {
    handles
        .iter()
        .map(|handle| (handle.to_string(), cache))
        .collect()
}

fn count_parts(stages: &LevelLoadStages, part: LevelBuildPart) -> usize {
    stages
        .probe()
        .upload_parts_done()
        .into_iter()
        .filter(|done| *done == part)
        .count()
}

/// What the commit reads of a prepared model: its clip entry and hit zones.
fn presentation_summary(model: &PreparedModel) -> String {
    let zones = model.parsed.as_ref().ok().map(|parsed| {
        let ModelHitZones {
            skeleton,
            clips,
            joint_zones,
            derived_bound,
            pose_stack,
            ..
        } = &parsed.hit_zones;
        format!("{skeleton:?}|{clips:?}|{joint_zones:?}|{derived_bound:?}|{pose_stack:?}")
    });
    format!("{:?}|{zones:?}", model.clip_entry)
}

fn key_set(renderer: &Renderer) -> Vec<ModelContentKey> {
    renderer.model_cache_keys()
}

/// Poll `stages` with `renderer` until `done` holds of its probe, failing
/// past the hold timeout.
fn poll_until(
    stages: &mut LevelLoadStages,
    renderer: &mut Renderer,
    done: impl Fn(&PrepProbe) -> bool,
) {
    let deadline = Instant::now() + HOLD_TIMEOUT;
    while !done(&stages.probe()) {
        assert!(
            stages.try_take(Some(renderer)).is_none(),
            "the load ended early"
        );
        assert!(Instant::now() < deadline, "the load never got there");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A hold the test releases: `wait` blocks until `release`, up to the hold
/// timeout.
#[derive(Clone, Default)]
struct Hold {
    reached: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

impl Hold {
    fn wait(&self) {
        self.reached.store(true, Ordering::Release);
        let deadline = Instant::now() + HOLD_TIMEOUT;
        while !self.released.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reached(&self) -> bool {
        self.reached.load(Ordering::Acquire)
    }

    fn release(&self) {
        self.released.store(true, Ordering::Release);
    }
}

// Models 3, key half: identical content under another directory and handle
// is the same key (no path enters it); editing the `.gltf`, its external
// buffer, a base-color PNG, or a referenced `.prm` sidecar each misses, and
// so does a missing sidecar; a file the parse never reads (the licence)
// changes nothing; an install parameter change misses; and the Surface Depth
// tier, which shapes no model material, leaves the key alone.
#[test]
fn model_cache_key_tracks_file_content() {
    let scratch = tempfile::tempdir().unwrap();
    let source = dev_content_root().join("models/ar_4");
    let a = scratch.path().join("mod_a/models/ar_4");
    let b = scratch.path().join("elsewhere/props/rifle");
    copy_dir(&source, &a);
    copy_dir(&source, &b);
    let prm_root = scratch.path().join("prm");
    std::fs::create_dir_all(&prm_root).unwrap();
    let sources = read_model_sources(&a.join("model.gltf")).expect("the rifle reads");
    assert!(
        sources.buffers().count() >= 1,
        "the rifle has an external buffer"
    );
    let sidecars = model_material_sidecar_paths(sources.material_keys(), &prm_root);
    assert!(!sidecars.is_empty(), "the rifle's materials read sidecars");
    for (key, _) in &sidecars {
        write_diffuse_prm(&prm_root, *key, (8, 8));
    }

    let params = ModelInstallParams::default();
    let base = key_of(&a.join("model.gltf"), &prm_root, params);
    assert_eq!(base.version(), MODEL_CONTENT_KEY_VERSION);
    let gltf = b.join("model.gltf");
    let key = || key_of(&gltf, &prm_root, params);
    assert_eq!(
        key(),
        base,
        "identical content at another path is the same model"
    );

    assert_edit_misses("gltf", &gltf, |bytes| bytes.push(b'\n'), key, base);
    assert_edit_misses(
        "buffer",
        &b.join("model.bin"),
        |bytes| {
            let last = bytes.len() - 1;
            bytes[last] ^= 0xFF;
        },
        key,
        base,
    );
    let png = std::fs::read_dir(&b)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "png"))
        .expect("the rifle ships a base-color PNG");
    assert_edit_misses("png", &png, |bytes| bytes.push(0), key, base);
    let sidecar = &sidecars[0].1;
    assert_edit_misses("prm", sidecar, |bytes| bytes.push(0), key, base);
    let saved = std::fs::read(sidecar).unwrap();
    std::fs::remove_file(sidecar).unwrap();
    assert_ne!(key(), base, "a missing sidecar misses");
    std::fs::write(sidecar, saved).unwrap();
    assert_eq!(key(), base);
    // The parse never reads it, so the key does not either.
    std::fs::write(b.join("license.txt"), b"edited").unwrap();
    assert_eq!(key(), base, "a file the parse never reads is not keyed");

    let with = |surface_depth| {
        key_of(
            &gltf,
            &prm_root,
            ModelInstallParams::with_surface_depth(surface_depth),
        )
    };
    let on = with(Some(RendererSurfaceDepthQuality::On));
    let off = with(Some(RendererSurfaceDepthQuality::Off));
    assert_ne!(on, base, "an install parameter change misses");
    assert_ne!(on, off, "each parameter value is its own key");
    // Models consume no surface map, so the tier is not a model parameter.
    for tier in [
        RendererSurfaceDepthQuality::Off,
        RendererSurfaceDepthQuality::On,
    ] {
        assert_eq!(ModelInstallParams::at_tier(tier), params);
        assert_eq!(
            key_of(&gltf, &prm_root, ModelInstallParams::at_tier(tier)),
            base
        );
    }
}

// Models 3: a second load of the same map hits every model: no parse, no
// model part uploads, no GPU creation for models, and each set draws the
// first load's entry. The commit reads the same clip entries and hit zones
// from a hit as from the parse, and the published set's resource summary is
// the miss's. A load at another Surface Depth tier still hits every model.
#[test]
fn same_content_change_hits_every_model() {
    let Some(mut renderer) = offscreen_renderer_or_skip("same_content_change_hits_every_model")
    else {
        return;
    };
    let handles = [ZONES, RIFLE, KNIGHT];
    let level = level_with_props(&handles);
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let pool = test_pool();

    let (first_stages, first) = load(
        &mut renderer,
        &pool,
        load_inputs(level.path(), &content, &prm, None),
    );
    assert_eq!(cache_uses(&first), all(&handles, ModelCacheUse::Miss));
    assert_eq!(count_parts(&first_stages, LevelBuildPart::Model), 3);
    assert!(
        first_stages
            .model_upload_creations()
            .expect("the set arrived")
            .total()
            > 0
    );
    let first_id = resident(&first);
    let committed = commit(&mut renderer, &first);
    assert_eq!(committed.inserted, 3, "{committed:?}");
    let miss_summary = renderer.active_level_resource_summary().unwrap();
    retire(first_stages.cancel(Some(&mut renderer)));

    let (second_stages, second) = load(
        &mut renderer,
        &pool,
        load_inputs(level.path(), &content, &prm, None),
    );
    assert_eq!(
        cache_uses(&second),
        all(&handles, ModelCacheUse::Hit),
        "every model hits: none parses"
    );
    assert_eq!(count_parts(&second_stages, LevelBuildPart::Model), 0);
    assert_eq!(count_parts(&second_stages, LevelBuildPart::CachedModel), 3);
    assert_eq!(
        second_stages
            .model_upload_creations()
            .expect("the set arrived")
            .total(),
        0,
        "no model upload created anything"
    );
    let second_id = resident(&second);
    for handle in handles {
        assert_eq!(
            renderer.level_set_model_identity(second_id, handle),
            renderer.level_set_model_identity(first_id, handle),
            "{handle}: the second set draws the first load's entry"
        );
        assert_eq!(
            presentation_summary(second.presentation.models.get(handle).unwrap()),
            presentation_summary(first.presentation.models.get(handle).unwrap()),
            "{handle}: the commit reads a hit as a parse"
        );
    }
    let committed = commit(&mut renderer, &second);
    assert_eq!(
        committed,
        ModelCacheCommit {
            inserted: 0,
            freed: 0,
            retained: 3
        }
    );
    assert_eq!(
        renderer.active_level_resource_summary().as_deref(),
        Some(miss_summary.as_str()),
        "a hit builds the miss's set"
    );
    retire(second_stages.cancel(Some(&mut renderer)));

    // Surface Depth shapes no model material: a load at the other tier hits.
    let mut tiers = load_inputs(level.path(), &content, &prm, None).tiers;
    tiers.surface_depth_quality = match tiers.surface_depth_quality {
        crate::options::SurfaceDepthQuality::Off => crate::options::SurfaceDepthQuality::On,
        _ => crate::options::SurfaceDepthQuality::Off,
    };
    let (third_stages, third) = load(
        &mut renderer,
        &pool,
        load_inputs(level.path(), &content, &prm, Some(tiers)),
    );
    assert_eq!(cache_uses(&third), all(&handles, ModelCacheUse::Hit));
    retire(third_stages.cancel(Some(&mut renderer)));
}

// Models 4 and Beside 2's cache half (P11, P30): a release frees no entry; a
// commit enters its set's misses and frees every entry no resident or active
// set references, keeping one a resident set still draws; a failed and a
// cancelled load, each after a model part uploaded, leave the cache exactly
// as found; and the next commit still frees what nothing references.
#[test]
fn model_cache_release_and_commit_lifetimes() {
    let Some(mut renderer) = offscreen_renderer_or_skip("model_cache_release_and_commit_lifetimes")
    else {
        return;
    };
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let key = |handle: &str| key_of(&content.join(handle), &prm, ModelInstallParams::default());
    let sorted = |handles: &[&str]| {
        let mut keys: Vec<ModelContentKey> = handles.iter().map(|handle| key(handle)).collect();
        keys.sort();
        keys
    };
    let pool = test_pool();

    // A: the running level.
    let map_a = level_with_props(&[ZONES, RIFLE]);
    let (stages, a) = load(
        &mut renderer,
        &pool,
        load_inputs(map_a.path(), &content, &prm, None),
    );
    let a_id = resident(&a);
    commit(&mut renderer, &a);
    retire(stages.cancel(Some(&mut renderer)));
    assert_eq!(key_set(&renderer), sorted(&[ZONES, RIFLE]));

    // B, beside A: its miss is not visible before it commits.
    let map_b = level_with_props(&[RIFLE, KNIGHT]);
    let (stages, b) = load(
        &mut renderer,
        &pool,
        load_inputs(map_b.path(), &content, &prm, None),
    );
    assert_eq!(
        cache_uses(&b),
        vec![
            (RIFLE.to_string(), ModelCacheUse::Hit),
            (KNIGHT.to_string(), ModelCacheUse::Miss)
        ]
    );
    assert_eq!(key_set(&renderer), sorted(&[ZONES, RIFLE]));
    retire(stages.cancel(Some(&mut renderer)));

    // Releasing A beside the ready bundle frees nothing.
    assert!(renderer.release_level_set(a_id));
    assert_eq!(
        key_set(&renderer),
        sorted(&[ZONES, RIFLE]),
        "a release frees no entry"
    );

    // B's commit enters its miss and frees only what A alone drew.
    let committed = commit(&mut renderer, &b);
    assert_eq!(
        committed,
        ModelCacheCommit {
            inserted: 1,
            freed: 1,
            retained: 2
        }
    );
    assert_eq!(key_set(&renderer), sorted(&[RIFLE, KNIGHT]));
    let b_id = resident(&b);

    // D commits with B kept resident: B's entries survive the sweep.
    let map_d = level_with_props(&[ZONES]);
    let (stages, d) = load(
        &mut renderer,
        &pool,
        load_inputs(map_d.path(), &content, &prm, None),
    );
    let d_id = resident(&d);
    renderer.publish_level_set(d_id, PreviousLevelSet::KeepResident);
    let committed = renderer.commit_model_cache(d_id).unwrap();
    assert_eq!(
        committed,
        ModelCacheCommit {
            inserted: 1,
            freed: 0,
            retained: 3
        },
        "entries a resident set draws survive"
    );
    retire(stages.cancel(Some(&mut renderer)));
    assert!(renderer.release_level_set(b_id));
    let before_failures = key_set(&renderer);

    // A load failing after its miss uploaded leaves the cache as found.
    let map_f = level_with_props(&[PROP]);
    let mut inputs = load_inputs(map_f.path(), &content, &prm, None);
    inputs.hooks.fail = vec![(PrepKind::Fluid, "injected after a model uploaded")];
    inputs.hooks.before_injected_failure = Some(Arc::new(|probe: &PrepProbe| {
        assert!(
            probe.wait_for(|probe| probe.upload_parts_done().contains(&LevelBuildPart::Model)),
            "the miss uploaded before the failure"
        );
    }));
    let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
    let failure = poll_outcome(&mut stages, Some(&mut renderer)).expect_err("the load fails");
    assert!(failure.message.contains("injected"), "{failure}");
    retire(stages.cancel(Some(&mut renderer)));
    assert_eq!(key_set(&renderer), before_failures, "a failed load");

    // A load cancelled while its miss's part reports leaves it as found.
    let hold = Hold::default();
    let mut inputs = load_inputs(map_f.path(), &content, &prm, None);
    {
        let hold = hold.clone();
        inputs.hooks.after_upload_part = Some(Arc::new(move |part| {
            if part == LevelBuildPart::Model {
                hold.wait();
            }
        }));
    }
    let (mut stages, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
    let deadline = Instant::now() + HOLD_TIMEOUT;
    while !hold.reached() {
        assert!(stages.try_take(Some(&mut renderer)).is_none());
        assert!(Instant::now() < deadline, "the miss never uploaded");
        std::thread::sleep(Duration::from_millis(1));
    }
    let retirement = stages.cancel(Some(&mut renderer));
    hold.release();
    retire(retirement);
    assert_eq!(key_set(&renderer), before_failures, "a cancelled load");

    // The next commit still frees what nothing references: only D draws now.
    let (stages, e) = load(
        &mut renderer,
        &pool,
        load_inputs(map_d.path(), &content, &prm, None),
    );
    let committed = commit(&mut renderer, &e);
    assert_eq!(
        committed,
        ModelCacheCommit {
            inserted: 0,
            freed: 2,
            retained: 1
        }
    );
    assert_eq!(key_set(&renderer), sorted(&[ZONES]));
    retire(stages.cancel(Some(&mut renderer)));
}

// P29: load A misses a model whose part is queued on the upload thread
// behind a held part when A is cancelled, so the upload runs out after the
// cancel. On the frame A joins, the next load dispatches: its first lookup
// finds the cache as it was before A (the model misses again), and its set
// never draws anything A built.
#[test]
fn cancelled_late_insert_rolls_back_before_next_lookup() {
    let Some(mut renderer) =
        offscreen_renderer_or_skip("cancelled_late_insert_rolls_back_before_next_lookup")
    else {
        return;
    };
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let pool = test_pool();
    let warm = level_with_props(&[ZONES]);
    let (stages, warm_bundle) = load(
        &mut renderer,
        &pool,
        load_inputs(warm.path(), &content, &prm, None),
    );
    commit(&mut renderer, &warm_bundle);
    retire(stages.cancel(Some(&mut renderer)));
    let before = key_set(&renderer);

    let map = level_with_props(&[ZONES, RIFLE]);
    let hold = Hold::default();
    let mut inputs = load_inputs(map.path(), &content, &prm, None);
    {
        let hold = hold.clone();
        inputs.hooks.after_upload_part = Some(Arc::new(move |part| {
            if part == LevelBuildPart::Textures {
                hold.wait();
            }
        }));
    }
    let (mut a, _progress) = start_with_renderer(&mut renderer, &pool, inputs);
    // Both model parts are queued behind the held textures part.
    poll_until(&mut a, &mut renderer, |probe| {
        hold.reached() && probe.state().finished.contains(&PrepKind::ModelParse)
    });
    let probe = a.probe();
    let mut retirement = a.cancel(Some(&mut renderer));
    hold.release();
    let deadline = Instant::now() + HOLD_TIMEOUT;
    while !retirement.try_finish() {
        assert_eq!(key_set(&renderer), before, "nothing A uploads is entered");
        assert!(Instant::now() < deadline, "A never joined");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !probe
            .upload_parts_done()
            .iter()
            .any(|part| matches!(part, LevelBuildPart::Model | LevelBuildPart::CachedModel)),
        "the queued model parts ran out after the cancel and built nothing: {:?}",
        probe.upload_parts_done()
    );

    // The frame A joins: the next load's first lookup sees the cache as it
    // was before A.
    assert_eq!(key_set(&renderer), before);
    let (b_stages, b) = load(
        &mut renderer,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    assert_eq!(
        cache_uses(&b),
        vec![
            (ZONES.to_string(), ModelCacheUse::Hit),
            (RIFLE.to_string(), ModelCacheUse::Miss)
        ]
    );
    assert_eq!(count_parts(&b_stages, LevelBuildPart::Model), 1);
    let committed = commit(&mut renderer, &b);
    assert_eq!(committed.inserted, 1);
    retire(b_stages.cancel(Some(&mut renderer)));
}

// Models 6 (P7), as far as the renderer models a suspend (drop it, build
// another): the dropped renderer's cache goes with it, so its handle finds
// nothing and the resumed renderer starts empty; a ready, uncommitted bundle
// drops with the renderer that holds its set; a set built against the old
// renderer and delivered after the drop never reaches the new one (the
// load fails as `StaleRenderer`); and the first load after resume re-uploads
// every model and builds the same set as before.
#[test]
fn suspend_clears_model_cache_and_drops_ready_bundle() {
    let test = "suspend_clears_model_cache_and_drops_ready_bundle";
    let Some(mut old) = offscreen_renderer_or_skip(test) else {
        return;
    };
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let handles = [ZONES, RIFLE];
    let map = level_with_props(&handles);
    let pool = test_pool();
    let (stages, warm) = load(
        &mut old,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    commit(&mut old, &warm);
    let before_suspend = old.active_level_resource_summary().unwrap();
    retire(stages.cancel(Some(&mut old)));
    let handle = old
        .model_cache_handle()
        .expect("a full renderer has a cache");
    let zones_key = key_of(&content.join(ZONES), &prm, ModelInstallParams::default());
    assert!(handle.lookup(&zones_key).is_some());

    // A ready bundle, uncommitted at suspend.
    let (ready_stages, ready) = load(
        &mut old,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    assert_eq!(cache_uses(&ready), all(&handles, ModelCacheUse::Hit));
    // A load whose set is delivered, but not received, before the suspend.
    let (mut late, _progress) = start_with_renderer(
        &mut old,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    assert!(
        late.probe()
            .wait_for(|probe| probe.upload_parts_done().contains(&LevelBuildPart::Finish)),
        "the late set was delivered"
    );
    // The suspend drops the renderer, the cache and every resident set.
    retire(ready_stages.cancel(None));
    drop(ready);
    drop(old);
    assert!(
        handle.lookup(&zones_key).is_none(),
        "the dropped renderer's cache is gone"
    );

    let Some(mut resumed) = offscreen_renderer_or_skip(test) else {
        return;
    };
    assert!(resumed.model_cache_keys().is_empty());
    let deadline = Instant::now() + Duration::from_secs(120);
    let failure = loop {
        if let Some(outcome) = late.try_take(Some(&mut resumed)) {
            break outcome.expect_err("a set from the old renderer never reaches the new one");
        }
        assert!(Instant::now() < deadline, "the late load never reported");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        failure.message.contains("renderer that is gone"),
        "{failure}"
    );
    assert!(resumed.resident_level_sets().is_empty());
    retire(late.cancel(Some(&mut resumed)));

    let (stages, first) = load(
        &mut resumed,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    assert_eq!(
        cache_uses(&first),
        all(&handles, ModelCacheUse::Miss),
        "the first load after resume re-uploads every model"
    );
    assert!(stages.model_upload_creations().unwrap().total() > 0);
    commit(&mut resumed, &first);
    assert_eq!(
        resumed.active_level_resource_summary().as_deref(),
        Some(before_suspend.as_str())
    );
    retire(stages.cancel(Some(&mut resumed)));
}

// Models 1, forced miss: a model only the commit's real sweep holds takes the
// synchronous fallback. It loads into the active set, enters the cache,
// counts once and logs once per load however often the commit asks; its
// products are a parse's; and asking again hits the entry it entered.
#[test]
fn forced_model_miss_loads_and_logs_once() {
    let Some(mut renderer) = offscreen_renderer_or_skip("forced_model_miss_loads_and_logs_once")
    else {
        return;
    };
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let pool = test_pool();
    let map = level_with_props(&[ZONES]);
    let (stages, bundle) = load(
        &mut renderer,
        &pool,
        load_inputs(map.path(), &content, &prm, None),
    );
    commit(&mut renderer, &bundle);
    retire(stages.cancel(Some(&mut renderer)));
    let active = renderer.active_level_set().unwrap();
    let real_sweep = vec![ZONES.to_string(), RIFLE.to_string()];
    let misses: Vec<&str> = bundle.presentation.models.misses(&real_sweep);
    assert_eq!(misses, [RIFLE]);

    let capture = LogCapture::start();
    let mut fallback = ModelMissFallback::default();
    let params = ModelInstallParams::default();
    let loaded = fallback.load(&mut renderer, RIFLE, &content, &prm, params);
    assert_eq!(loaded.cache, ModelCacheUse::Miss);
    let oracle = load_model(&content.join(RIFLE));
    assert_eq!(
        loaded.clip_entry,
        crate::startup::lifecycle::model_clip_entry(&oracle)
    );
    assert!(loaded.parsed.is_ok());
    let identity = renderer.level_set_model_identity(active, RIFLE);
    assert!(identity.is_some(), "the miss loads into the active set");
    assert!(
        renderer
            .active_level_resource_summary()
            .unwrap()
            .contains(RIFLE)
    );
    let rifle_key = key_of(&content.join(RIFLE), &prm, params);
    assert!(
        renderer.model_cache_keys().contains(&rifle_key),
        "entered at the commit"
    );

    let again = fallback.load(&mut renderer, RIFLE, &content, &prm, params);
    assert_eq!(again.cache, ModelCacheUse::Hit);
    assert_eq!(renderer.level_set_model_identity(active, RIFLE), identity);
    assert_eq!(fallback.misses(), 1, "counted once per load");
    capture.assert_logged_once(
        log::Level::Warn,
        "model prediction missed 'models/ar_4/model.gltf'",
    );
}

// Models 2 (P9), as far as tests reach it: the staged-manifest reload commit
// is not drivable from here, so its effect is applied directly. Prediction
// half: with a descriptor's mesh block changed, as a reload's whole-registry
// swap leaves it, a load predicts the new model with zero misses against
// the real sweep, and the new model misses the cache while every unchanged
// model hits. File half: a model edited on disk keys anew and misses, while
// the old level's set keeps drawing the old entry until its release, whose
// next commit frees it.
#[test]
fn hot_reload_mesh_block_then_load_predicts_new_model() {
    let test = "hot_reload_mesh_block_then_load_predicts_new_model";
    let Some(mut renderer) = offscreen_renderer_or_skip(test) else {
        return;
    };
    let (content, prm) = (dev_content_root(), dev_prm_root());
    let pool = test_pool();

    // Prediction half.
    let before = dev_descriptors();
    let edited = before
        .iter()
        .position(|descriptor| {
            descriptor.mesh.is_some()
                && descriptor.movement.is_none()
                && descriptor.behavior.is_none()
                && descriptor.canonical_name.is_some()
        })
        .expect("the dev mod has a placeable mesh descriptor");
    let classname = before[edited].canonical_name.clone().unwrap();
    let old_model = before[edited].mesh.as_ref().unwrap().model.clone();
    assert_ne!(old_model, PROP);
    let mut after = before.clone();
    after[edited].mesh.as_mut().unwrap().model = PROP.to_string();
    let placements = vec![record(&classname, &[], 0)];
    for (label, descriptors) in [("before", &before), ("after", &after)] {
        let predicted = predict_level_models(&placements, &prediction_inputs(descriptors, false));
        let (real, _) = real_sweep(placements.clone(), descriptors, false);
        let predicted = PreparedModels::new(
            predicted
                .iter()
                .map(|handle| PreparedModel {
                    handle: handle.clone(),
                    open_path: PathBuf::new(),
                    clip_entry: Default::default(),
                    parsed: Err(postretro_model::gltf_loader::ModelLoadError::NoMesh(
                        handle.clone(),
                    )),
                    cache: ModelCacheUse::Unkeyed,
                })
                .collect(),
        );
        assert!(
            predicted.misses(&real).is_empty(),
            "{label}: zero misses against {real:?}"
        );
    }
    let level = write_level_with_entities(placements);
    let mut inputs = load_inputs(level.path(), &content, &prm, None);
    inputs.models = prediction_inputs(&before, false);
    let (stages, first) = load(&mut renderer, &pool, inputs);
    assert!(first.presentation.models.get(&old_model).is_some());
    commit(&mut renderer, &first);
    retire(stages.cancel(Some(&mut renderer)));
    let mut inputs = load_inputs(level.path(), &content, &prm, None);
    inputs.models = prediction_inputs(&after, false);
    let (stages, second) = load(&mut renderer, &pool, inputs);
    let first_uses = cache_uses(&first);
    for (handle, cache) in cache_uses(&second) {
        let expected = match first_uses.iter().find(|(first, _)| *first == handle) {
            // Only the reload added it.
            None => ModelCacheUse::Miss,
            Some((_, ModelCacheUse::Unkeyed)) => ModelCacheUse::Unkeyed,
            Some(_) => ModelCacheUse::Hit,
        };
        assert_eq!(cache, expected, "{handle}");
    }
    assert!(
        cache_uses(&second).contains(&(PROP.to_string(), ModelCacheUse::Miss)),
        "the reloaded mesh block's model misses"
    );
    assert!(second.presentation.models.get(PROP).is_some());
    commit(&mut renderer, &second);
    retire(stages.cancel(Some(&mut renderer)));

    // File half, on a copy of the rifle.
    let scratch = tempfile::tempdir().unwrap();
    copy_dir(
        &content.join("models/ar_4"),
        &scratch.path().join("models/ar_4"),
    );
    let map = level_with_props(&[RIFLE]);
    let (stages, old) = load(
        &mut renderer,
        &pool,
        load_inputs(map.path(), scratch.path(), &prm, None),
    );
    let old_id = resident(&old);
    commit(&mut renderer, &old);
    retire(stages.cancel(Some(&mut renderer)));
    let old_entry = renderer.level_set_model_identity(old_id, RIFLE);
    assert!(old_entry.is_some());
    let gltf = scratch.path().join(RIFLE);
    let mut bytes = std::fs::read(&gltf).unwrap();
    bytes.push(b'\n');
    std::fs::write(&gltf, bytes).unwrap();

    let (stages, new) = load(
        &mut renderer,
        &pool,
        load_inputs(map.path(), scratch.path(), &prm, None),
    );
    assert_eq!(cache_uses(&new), all(&[RIFLE], ModelCacheUse::Miss));
    let new_id = resident(&new);
    assert_eq!(
        renderer.level_set_model_identity(old_id, RIFLE),
        old_entry,
        "the running level keeps its entry"
    );
    assert_ne!(renderer.level_set_model_identity(new_id, RIFLE), old_entry);
    let committed = commit(&mut renderer, &new);
    assert_eq!((committed.inserted, committed.freed), (1, 1));
    retire(stages.cancel(Some(&mut renderer)));
}
