//! The prep stage's shared state: the staged parse's hand-outs, each task's
//! product, the rules that spawn a task once its inputs exist and settle the
//! load's one outcome, and, for a load that builds a renderer set, the hand-off
//! of each GPU-bound product to the set's builder as soon as it exists.
//! See: context/plans/done/level-load-pipeline/index.md (Staged pipeline) · research.md §9 (P10, P18, P19)

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use postretro_audio::DecodedLevelSounds;
use postretro_level_loader::{
    CompleteHandOuts, EntitiesHandOut, FinalHandOut, GeometryHandOut, GpuLightingPayloads, HandOut,
    HandOutStage, LevelLoadProgress, LevelWorld, LoadCancel, LoadPhase, LoadProgress, PrlLoadError,
    ShManifestHandOut, SpatialHandOut, StagedParseOutcome,
};
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::lightmap_family::LightmapFamilyPlan;
use postretro_render_cpu::sh_residency::ShResidencyPlan;

use super::model_cache::{self, ModelPart};
use super::model_key::ModelInstallParams;
use super::products::ModelUpload;
use super::tasks::{self, TexturePrep};
use super::upload::UploadSlot;
use super::{
    LevelLoadFailure, LevelPrepInputs, LoadStage, PrepKind, PrepTimings, PreparedGpuLevel,
    PreparedLevel, PreparedModel, PreparedModels, PreparedPresentation,
};
use crate::collision::CollisionWorld;
use crate::load_pool::{CancelToken, GroupSpawner, Priority, TaskError, TaskHandle};
use crate::render::LevelGeometryParts;
use crate::session::level_streaming::StreamingLevelData;
use crate::startup::render_profile::renderer_surface_depth_quality;

/// What a finished prep task hands back.
enum Product {
    Textures(TexturePrep),
    ShResidency(Box<ShResidencyPlan>),
    RangeChecked,
    /// The assembled level and the GPU-only lighting payloads taken out of it.
    Assembled(Box<LevelWorld>, GpuLightingPayloads),
    Collision(Box<CollisionWorld>),
    Streaming(StreamingLevelData),
    LightmapFamily(LightmapFamilyPlan),
    Fluid(Option<PreparedFluidSurfaces>),
    Sounds(DecodedLevelSounds),
    ModelsPredicted(Vec<String>),
    /// One predicted model, at its index in the prediction.
    Model(usize, Box<PreparedModel>),
}

/// One load's prep, shared by the parse task, every prep task and the main
/// thread's [`super::LevelPrep`].
pub(super) struct PrepShared {
    pub(super) inputs: LevelPrepInputs,
    spawner: GroupSpawner,
    parse_progress: Arc<LoadProgress>,
    prep_progress: Arc<LoadProgress>,
    /// The load's set builder, when it builds one, and the upload phase's
    /// progress.
    pub(super) upload: UploadSlot,
    /// Set by the load's outcome and by cancel. No task spawns or starts
    /// after it, and the parse stops at its next stage boundary.
    stopped: AtomicBool,
    state: Mutex<PrepState>,
}

impl std::fmt::Debug for PrepShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        f.debug_struct("PrepShared")
            .field("map_path", &self.inputs.map_path)
            .field("last_hand_out", &state.last_hand_out)
            .field("finished", &state.finished)
            .field("settled", &state.settled)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct PrepState {
    sh_manifest: Option<ShManifestHandOut>,
    geometry: Option<Arc<GeometryHandOut>>,
    spatial: Option<Arc<SpatialHandOut>>,
    entities: Option<Arc<EntitiesHandOut>>,
    final_hand_out: Option<Box<FinalHandOut>>,
    last_hand_out: Option<HandOutStage>,
    /// The parse returned an accepting verdict, and with it dropped its own
    /// references to hand-outs 2–4.
    parse_accepted: bool,
    parse_time: Duration,

    textures: Option<TexturePrep>,
    /// `Some(None)` for a level without an SH manifest.
    sh_residency: Option<Option<ShResidencyPlan>>,
    range_checked: bool,
    assemble_spawned: bool,
    world: Option<Arc<LevelWorld>>,
    /// The world's lighting payloads, shared with the lightmap family plan
    /// until the geometry part takes them.
    payloads: Option<Arc<GpuLightingPayloads>>,
    world_readers_spawned: bool,
    /// The geometry part and `finish` went to the set builder.
    geometry_submitted: bool,
    finish_submitted: bool,
    collision: Option<CollisionWorld>,
    streaming: Option<StreamingLevelData>,
    lightmap_family: Option<LightmapFamilyPlan>,
    fluid: Option<Option<PreparedFluidSurfaces>>,
    /// `Some(None)` when the dispatch had no audio.
    sounds: Option<Option<DecodedLevelSounds>>,
    predicted_models: Option<Vec<String>>,
    model_parse_spawned: bool,
    /// Each predicted model's prep, at its index in the prediction, as the
    /// parse tasks finish in any order.
    models: Vec<Option<PreparedModel>>,

    started: BTreeSet<PrepKind>,
    finished: BTreeSet<PrepKind>,
    timings: Vec<(PrepKind, Duration)>,
    handles: Vec<(LoadStage, TaskHandle<()>)>,
    /// A pre-verdict task's failure, held until the parse's verdict: a
    /// rejection replaces it, an acceptance reports it.
    held_failure: Option<LevelLoadFailure>,
    outcome: Option<Result<PreparedLevel, LevelLoadFailure>>,
    /// The outcome was decided (or the load cancelled); nothing after it is
    /// stored or reported.
    settled: bool,
}

impl PrepState {
    /// Drop every hand-out and product: no prep result outlives a failure
    /// or a cancel.
    fn clear_products(&mut self) {
        self.sh_manifest = None;
        self.geometry = None;
        self.spatial = None;
        self.entities = None;
        self.final_hand_out = None;
        self.textures = None;
        self.sh_residency = None;
        self.world = None;
        self.payloads = None;
        self.collision = None;
        self.streaming = None;
        self.lightmap_family = None;
        self.fluid = None;
        self.sounds = None;
        self.predicted_models = None;
        self.models.clear();
    }

    fn take_complete_hand_outs(&mut self) -> Option<CompleteHandOuts> {
        if self.sh_manifest.is_none()
            || self.geometry.is_none()
            || self.spatial.is_none()
            || self.entities.is_none()
            || self.final_hand_out.is_none()
        {
            return None;
        }
        Some(CompleteHandOuts {
            sh_manifest: self.sh_manifest.take()?,
            geometry: self.geometry.take()?,
            spatial: self.spatial.take()?,
            entities: self.entities.take()?,
            final_hand_out: self.final_hand_out.take()?,
        })
    }

    /// Store a task's product; whether its kind is now complete (every
    /// model parse task of a load shares one kind).
    fn store(&mut self, product: Product) -> bool {
        match product {
            Product::Textures(textures) => self.textures = Some(textures),
            Product::ShResidency(plan) => self.sh_residency = Some(Some(*plan)),
            Product::RangeChecked => self.range_checked = true,
            Product::Assembled(world, payloads) => {
                self.world = Some(Arc::new(*world));
                self.payloads = Some(Arc::new(payloads));
            }
            Product::Collision(collision) => self.collision = Some(*collision),
            Product::Streaming(streaming) => self.streaming = Some(streaming),
            Product::LightmapFamily(plan) => self.lightmap_family = Some(plan),
            Product::Fluid(fluid) => self.fluid = Some(fluid),
            Product::Sounds(sounds) => self.sounds = Some(Some(sounds)),
            Product::ModelsPredicted(handles) => {
                self.models = handles.iter().map(|_| None).collect();
                self.predicted_models = Some(handles);
            }
            Product::Model(index, model) => {
                if let Some(slot) = self.models.get_mut(index) {
                    *slot = Some(*model);
                }
                return self.models.iter().all(Option::is_some);
            }
        }
        true
    }
}

/// The staged parse's cancel: the load's group token, or the prep having
/// stopped on its outcome.
struct PrepCancel<'a> {
    token: &'a CancelToken,
    stopped: &'a AtomicBool,
}

impl LoadCancel for PrepCancel<'_> {
    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled() || self.stopped.load(Ordering::Acquire)
    }
}

impl PrepShared {
    pub(super) fn new(
        mut inputs: LevelPrepInputs,
        spawner: GroupSpawner,
        progress: &LevelLoadProgress,
    ) -> Arc<Self> {
        super::enter_prep_progress(progress, PrepKind::ALL.len());
        // The inputs' builder moves into the slot: tasks reach it only there.
        let upload = UploadSlot::new(
            inputs.upload.take(),
            Arc::clone(progress.phase(LoadPhase::Upload)),
            #[cfg(test)]
            inputs.hooks.after_upload_part.clone(),
        );
        Arc::new(Self {
            inputs,
            spawner,
            parse_progress: Arc::clone(progress.phase(LoadPhase::Parse)),
            prep_progress: Arc::clone(progress.phase(LoadPhase::Prep)),
            upload,
            stopped: AtomicBool::new(false),
            state: Mutex::new(PrepState::default()),
        })
    }

    /// Every lock here guards plain data no panic leaves half-written, and
    /// task bodies run outside it, so a poisoned guard is still sound.
    fn lock(&self) -> MutexGuard<'_, PrepState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    /// Queue the staged parse as the load's first task.
    pub(super) fn start_parse(self: &Arc<Self>) {
        if !self.inputs.audio_enabled {
            // No audio: nothing to decode, credited at once. Before the parse
            // is queued: only a task's `advance` completes the prep, so a
            // credit landing after the last task's completion check would
            // leave the prep waiting forever.
            let mut state = self.lock();
            state.sounds = Some(None);
            self.finish_kind(&mut state, PrepKind::Sounds);
        }
        let shared = Arc::clone(self);
        let handle = self.spawner.spawn(Priority::Load, move |token| {
            shared.run_parse(token);
        });
        #[cfg(test)]
        self.inputs
            .hooks
            .after_parse_queued(&super::test_support::PrepProbe::new(Arc::clone(self)));
        self.lock().handles.push((LoadStage::Parse, handle));
    }

    fn run_parse(self: &Arc<Self>, token: &CancelToken) {
        if token.is_cancelled() || self.stopped() {
            return;
        }
        let cancel = PrepCancel {
            token,
            stopped: &self.stopped,
        };
        let started = Instant::now();
        let path = self.inputs.map_path.to_string_lossy().into_owned();
        // Returns only after the parse dropped its own hand-out references,
        // so assembly after the verdict takes the data without a copy.
        let result = postretro_level_loader::load_prl_staged(
            &path,
            &self.parse_progress,
            &cancel,
            |hand_out| self.accept_hand_out(hand_out),
        );
        match result {
            Ok(StagedParseOutcome::Complete) => self.parse_accepted(started.elapsed()),
            // The load was cancelled or already failed: report nothing.
            Ok(StagedParseOutcome::Cancelled) => {}
            Err(error) => self.parse_rejected(error),
        }
    }

    /// The parse's sink: store the hand-out and spawn every task it
    /// completes the inputs of.
    fn accept_hand_out(self: &Arc<Self>, hand_out: HandOut) {
        #[cfg(test)]
        let hand_out = self.inputs.hooks.edit_hand_out(hand_out);
        let stage = hand_out.stage();
        {
            let mut state = self.lock();
            if !state.settled {
                state.last_hand_out = Some(stage);
                self.store_hand_out(&mut state, hand_out);
                self.advance(&mut state);
            }
        }
        #[cfg(test)]
        self.inputs.hooks.after_hand_out(
            stage,
            &super::test_support::PrepProbe::new(Arc::clone(self)),
        );
    }

    fn store_hand_out(self: &Arc<Self>, state: &mut PrepState, hand_out: HandOut) {
        match hand_out {
            HandOut::ShManifest(sh_manifest) => {
                if sh_manifest.manifest.is_none() {
                    // An empty kind: nothing to plan, credited at once.
                    state.sh_residency = Some(None);
                    self.finish_kind(state, PrepKind::ShResidency);
                }
                state.sh_manifest = Some(sh_manifest);
            }
            HandOut::Geometry(geometry) => {
                state.geometry = Some(Arc::clone(&geometry));
                self.spawn(state, PrepKind::Textures, move |shared| {
                    let mut prep = tasks::prep_textures(&geometry, &shared.inputs.prm_cache_root);
                    // The upload starts here, before the parse returns: the
                    // prepared sidecars go to the build from this thread.
                    if shared.upload.is_active()
                        && let Some(textures) = prep.textures.take()
                    {
                        shared.upload.textures(textures, &prep.materials);
                        #[cfg(test)]
                        if shared.inputs.hooks.upload_textures_twice {
                            shared.upload.duplicate_textures();
                        }
                        #[cfg(test)]
                        if let Some(fault) = shared.inputs.hooks.upload_fault {
                            shared.upload.fault(fault);
                        }
                    }
                    Ok(Product::Textures(prep))
                });
            }
            HandOut::Spatial(spatial) => {
                state.spatial = Some(Arc::clone(&spatial));
                if let Some(geometry) = state.geometry.clone() {
                    let spatial = Arc::clone(&spatial);
                    self.spawn(state, PrepKind::RangeCheck, move |_| {
                        tasks::check_geometry_ranges(&geometry, &spatial)
                            .map(|()| Product::RangeChecked)
                    });
                }
                let metadata = state
                    .sh_manifest
                    .as_ref()
                    .and_then(|sh_manifest| sh_manifest.manifest.clone());
                if let Some(metadata) = metadata {
                    self.spawn(state, PrepKind::ShResidency, move |_| {
                        tasks::plan_streamed_sh(&metadata, &spatial)
                            .map(|plan| Product::ShResidency(Box::new(plan)))
                    });
                }
            }
            HandOut::Entities(entities) => {
                // The prediction owns a copy of the map entities, so the
                // state stays the hand-out's only holder for assembly.
                let map_entities = entities.map_entities.clone();
                state.entities = Some(entities);
                self.spawn(state, PrepKind::ModelPrediction, move |shared| {
                    let handles = tasks::predict_models(&map_entities, &shared.inputs.models);
                    // The upload's part count is known now, before any model
                    // part goes out.
                    shared.upload.plan(handles.len());
                    Ok(Product::ModelsPredicted(handles))
                });
                if self.inputs.audio_enabled {
                    self.spawn(state, PrepKind::Sounds, |shared| {
                        #[cfg(test)]
                        let sounds = tasks::decode_sounds_reordered(
                            &shared.inputs.content_root,
                            shared.inputs.hooks.sound_decode_order,
                        );
                        #[cfg(not(test))]
                        let sounds = tasks::decode_sounds(&shared.inputs.content_root);
                        Ok(Product::Sounds(sounds))
                    });
                }
            }
            HandOut::Final(final_hand_out) => state.final_hand_out = Some(final_hand_out),
        }
    }

    /// Queue `work` as `kind`'s task. It never starts once the prep stopped;
    /// its product or failure comes back through [`Self::finish`]. `work`'s
    /// captures (hand-out and level clones) drop when it returns, before the
    /// product is stored.
    fn spawn<F>(self: &Arc<Self>, state: &mut PrepState, kind: PrepKind, work: F)
    where
        F: FnOnce(&PrepShared) -> Result<Product, String> + Send + 'static,
    {
        if self.stopped() || state.settled || state.held_failure.is_some() {
            return;
        }
        let shared = Arc::clone(self);
        let handle =
            self.spawner.spawn(Priority::Load, move |token| {
                if token.is_cancelled() || shared.stopped() {
                    return;
                }
                shared.lock().started.insert(kind);
                #[cfg(test)]
                if let Some(message) = shared.inputs.hooks.inject(kind) {
                    shared.inputs.hooks.before_injected_failure(
                        &super::test_support::PrepProbe::new(Arc::clone(&shared)),
                    );
                    shared.finish(kind, Duration::ZERO, Err(message));
                    return;
                }
                #[cfg(test)]
                shared.inputs.hooks.on_task_run(kind);
                let started = Instant::now();
                let result = work(&shared);
                shared.finish(kind, started.elapsed(), result);
            });
        state.handles.push((LoadStage::Prep(kind), handle));
    }

    fn finish(
        self: &Arc<Self>,
        kind: PrepKind,
        elapsed: Duration,
        result: Result<Product, String>,
    ) {
        let mut state = self.lock();
        if state.settled {
            return;
        }
        match result {
            Err(message) => self.fail(&mut state, LevelLoadFailure::prep(kind, message)),
            Ok(product) => {
                state.timings.push((kind, elapsed));
                if state.store(product) {
                    self.finish_kind(&mut state, kind);
                }
                self.advance(&mut state);
            }
        }
    }

    /// Mark `kind` finished, once, and credit it to prep's progress.
    fn finish_kind(&self, state: &mut PrepState, kind: PrepKind) {
        if state.finished.insert(kind) {
            self.prep_progress.advance(1);
        }
    }

    /// Spawn every task whose inputs now exist, and settle the load once
    /// every kind finished.
    fn advance(self: &Arc<Self>, state: &mut PrepState) {
        if self.stopped() || state.settled || state.held_failure.is_some() {
            return;
        }
        // Assembly waits for the verdict and for every pre-verdict task, so
        // no hand-out reference but the state's remains: `assemble` then
        // moves the data out instead of copying it.
        if !state.assemble_spawned
            && state.parse_accepted
            && state.range_checked
            && state.sh_residency.is_some()
            && let Some(dims) = state
                .textures
                .as_ref()
                .map(|textures| textures.dims.clone())
            && let Some(hand_outs) = state.take_complete_hand_outs()
        {
            state.assemble_spawned = true;
            self.spawn(state, PrepKind::Assemble, move |shared| {
                #[cfg_attr(not(test), allow(unused_mut))]
                let mut world = tasks::assemble_level(hand_outs, &dims);
                // The accepted parse's stale-data-script warning, off the state lock.
                crate::startup::data_script_staleness::report_stale_data_script(
                    &shared.inputs.map_path,
                    world.data_script.as_ref(),
                );
                #[cfg(test)]
                shared.inputs.hooks.edit_world(&mut world);
                // Out before the level is shared: the geometry part owns them.
                let payloads = world.take_gpu_lighting_payloads();
                Ok(Product::Assembled(Box::new(world), payloads))
            });
        }
        if !state.model_parse_spawned
            && let Some(handles) = state.predicted_models.clone()
        {
            state.model_parse_spawned = true;
            self.spawn_model_parses(state, handles);
        }
        if !state.world_readers_spawned
            && let Some(world) = state.world.clone()
            && let Some(payloads) = state.payloads.clone()
            && let Some(dims) = state
                .textures
                .as_ref()
                .map(|textures| textures.dims.clone())
        {
            state.world_readers_spawned = true;
            self.spawn_world_readers(state, &world, payloads, dims);
        }
        if self.upload.is_active() {
            self.submit_geometry(state);
            self.submit_finish(state);
            if state.settled {
                return;
            }
        }
        if state.finished.len() == PrepKind::ALL.len() {
            self.complete(state);
        }
    }

    /// The geometry part, as soon as its inputs exist: the assembled level
    /// (UVs normalized), its lighting payloads, the material table, the SH
    /// plan, the lightmap family plan and the fluid pack. Submitted from
    /// whichever pool thread stored the last of them. The planar candidate
    /// is the app's verdict over its fluid tables (session data, not level
    /// data), so the build keys the planar targets by the install rule and a
    /// different verdict re-keys them on main when the commit publishes the
    /// set. The part therefore carries `planar_candidate: None`: the verdict
    /// is not a prep input, and the commit syncs it from the app's tables
    /// just after the publish (`sync_planar_candidate`).
    fn submit_geometry(&self, state: &mut PrepState) {
        if state.geometry_submitted
            || state.lightmap_family.is_none()
            || state.fluid.is_none()
            || state.sh_residency.is_none()
        {
            return;
        }
        let (Some(world), Some(textures)) = (state.world.clone(), state.textures.as_ref()) else {
            return;
        };
        // The lightmap plan dropped its share before its product was stored.
        let payloads = match state.payloads.take().map(Arc::try_unwrap) {
            Some(Ok(payloads)) => payloads,
            _ => {
                let failure = LevelLoadFailure::prep(
                    PrepKind::LightmapFamily,
                    "the level's lighting payloads are still shared",
                );
                self.settle(state, Err(failure));
                return;
            }
        };
        let (Some(sh_residency), Some(lightmap_family), Some(fluid)) = (
            state.sh_residency.take(),
            state.lightmap_family.take(),
            state.fluid.take(),
        ) else {
            return;
        };
        state.geometry_submitted = true;
        self.upload.geometry(LevelGeometryParts {
            world,
            texture_materials: textures.materials.clone(),
            gpu_lighting_payloads: payloads,
            sh_residency,
            lightmap_family,
            fluid,
            planar_candidate: None,
        });
    }

    /// `finish`, once every part that will go out has: textures and every
    /// model (each submitted from its own task before its product was
    /// stored) and geometry. Never after a failure: a settled load stopped
    /// the slot.
    fn submit_finish(&self, state: &mut PrepState) {
        if state.finish_submitted
            || !state.geometry_submitted
            || state.textures.is_none()
            || state.predicted_models.is_none()
            || !state.models.iter().all(Option::is_some)
        {
            return;
        }
        state.finish_submitted = true;
        self.upload.finish();
    }

    /// The tasks that read the assembled level: after the verdict, so on
    /// final data, the manifest's cluster topology installed (pin P19).
    fn spawn_world_readers(
        self: &Arc<Self>,
        state: &mut PrepState,
        world: &Arc<LevelWorld>,
        payloads: Arc<GpuLightingPayloads>,
        dims: Vec<(u32, u32)>,
    ) {
        let level = Arc::clone(world);
        self.spawn(state, PrepKind::Collision, move |_| {
            tasks::build_collision(&level).map(|collision| Product::Collision(Box::new(collision)))
        });
        let level = Arc::clone(world);
        self.spawn(state, PrepKind::StreamingData, move |shared| {
            tasks::decode_streaming(&level, &shared.inputs.sh_streaming_mode)
                .map(Product::Streaming)
        });
        let level = Arc::clone(world);
        self.spawn(state, PrepKind::LightmapFamily, move |shared| {
            Ok(Product::LightmapFamily(tasks::plan_lightmap(
                &level,
                &payloads,
                &shared.inputs.device_limits,
            )))
        });
        let level = Arc::clone(world);
        self.spawn(state, PrepKind::Fluid, move |shared| {
            Ok(Product::Fluid(tasks::prep_fluid(
                &level,
                &dims,
                &shared.inputs.device_limits,
            )))
        });
    }

    /// One parse task per predicted model; their results land at their
    /// prediction index whatever order they finish in. An empty prediction
    /// credits the kind at once.
    fn spawn_model_parses(self: &Arc<Self>, state: &mut PrepState, handles: Vec<String>) {
        if handles.is_empty() {
            self.finish_kind(state, PrepKind::ModelParse);
            return;
        }
        // Every model of the load keys at the dispatch tier.
        let params = ModelInstallParams::at_tier(renderer_surface_depth_quality(
            self.inputs.tiers.surface_depth_quality,
        ));
        for (index, handle) in handles.into_iter().enumerate() {
            self.spawn(state, PrepKind::ModelParse, move |shared| {
                #[cfg(test)]
                shared.inputs.hooks.before_model_parse(index);
                // Read, key, look up; parse only on a miss.
                let (mut model, part) = model_cache::prep_model(
                    &handle,
                    &shared.inputs.content_root,
                    &shared.inputs.prm_cache_root,
                    shared.upload.model_cache(),
                    params,
                );
                // The model's part goes out from this thread; the clip entry
                // and hit zones stay for the commit. A failed parse submits
                // nothing and degrades at commit as the oracle sweep's does.
                match part {
                    Some(part) if shared.upload.is_active() => {
                        shared.upload.model(&model.handle, part);
                    }
                    // A load that builds no set keeps the upload inputs. It
                    // has no cache, so its parts are never cache hits.
                    Some(ModelPart::Upload {
                        model: loaded,
                        materials,
                        ..
                    }) => {
                        if let Ok(parsed) = model.parsed.as_mut() {
                            parsed.upload = Some(ModelUpload {
                                model: *loaded,
                                materials,
                            });
                        }
                    }
                    Some(ModelPart::Cached(_)) => {}
                    None => {
                        if shared.upload.is_active() {
                            shared.upload.forgo_model();
                        }
                    }
                }
                Ok(Product::Model(index, Box::new(model)))
            });
        }
    }

    /// Every kind finished: hand the products on as one prepared level. A
    /// load that builds a set handed its GPU-bound products to the builder
    /// already, `finish` included, and drops prep's builder here; a load
    /// that builds none keeps them.
    fn complete(&self, state: &mut PrepState) {
        let models: Option<Vec<PreparedModel>> =
            std::mem::take(&mut state.models).into_iter().collect();
        let (
            Some(world),
            Some(textures),
            Some(collision),
            Some(streaming),
            Some(sounds),
            Some(models),
        ) = (
            state.world.take(),
            state.textures.take(),
            state.collision.take(),
            state.streaming.take(),
            state.sounds.take(),
            models,
        )
        else {
            self.settle(state, Err(missing_product()));
            return;
        };
        let gpu = if self.upload.is_active() {
            if !state.finish_submitted {
                self.settle(state, Err(missing_product()));
                return;
            }
            self.upload.release();
            None
        } else {
            // No builder: production dispatches only once the renderer is
            // full-ready, so only tests reach this renderer-less path. Each
            // reader dropped its clone of the payloads before its product was
            // stored, so the state holds the only one.
            let payloads = state.payloads.take().map(Arc::try_unwrap);
            let (
                Some(prepared),
                Some(Ok(gpu_lighting_payloads)),
                Some(sh_residency),
                Some(lightmap_family),
                Some(fluid),
            ) = (
                textures.textures,
                payloads,
                state.sh_residency.take(),
                state.lightmap_family.take(),
                state.fluid.take(),
            )
            else {
                self.settle(state, Err(missing_product()));
                return;
            };
            Some(PreparedGpuLevel {
                textures: prepared,
                texture_materials: textures.materials,
                diffuse_dims: textures.dims,
                gpu_lighting_payloads,
                sh_residency,
                lightmap_family,
                fluid,
                device_limits: self.inputs.device_limits,
                tiers: self.inputs.tiers,
                prm_cache_root: self.inputs.prm_cache_root.clone(),
            })
        };
        let prepared = PreparedLevel {
            world,
            gpu,
            collision,
            streaming,
            presentation: PreparedPresentation {
                sounds,
                models: PreparedModels::new(models),
            },
            timings: PrepTimings {
                parse: state.parse_time,
                tasks: state.timings.clone(),
            },
        };
        self.prep_progress.finish();
        self.settle(state, Ok(prepared));
    }

    /// Route one failure. The first wins and stops the load; a pre-verdict
    /// task's waits for the parse's verdict, which a rejection overrides.
    /// While one is held, a later failure is dropped, so it can neither
    /// settle the load before the verdict nor displace the first; only the
    /// parse's own failure (a panic, which yields no verdict) replaces it,
    /// as a rejection does.
    fn fail(&self, state: &mut PrepState, failure: LevelLoadFailure) {
        if state.settled {
            return;
        }
        if state.held_failure.is_some() {
            if failure.stage != LoadStage::Parse {
                return;
            }
            state.held_failure = None;
            self.settle(state, Err(failure));
            return;
        }
        if let LoadStage::Prep(kind) = failure.stage
            && kind.before_verdict()
            && !state.parse_accepted
        {
            state.held_failure.get_or_insert(failure);
            return;
        }
        self.settle(state, Err(failure));
    }

    fn settle(&self, state: &mut PrepState, outcome: Result<PreparedLevel, LevelLoadFailure>) {
        self.stopped.store(true, Ordering::Release);
        if outcome.is_err() {
            state.clear_products();
            // The outcome is stored below under the same state lock, so a
            // poll that sees the build cancelled then finds this failure.
            self.upload.stop();
        }
        state.outcome = Some(outcome);
        state.settled = true;
    }

    fn parse_accepted(self: &Arc<Self>, elapsed: Duration) {
        let mut state = self.lock();
        if state.settled {
            return;
        }
        state.parse_accepted = true;
        state.parse_time = elapsed;
        if let Some(failure) = state.held_failure.take() {
            self.settle(&mut state, Err(failure));
            return;
        }
        self.advance(&mut state);
    }

    fn parse_rejected(&self, error: PrlLoadError) {
        let mut state = self.lock();
        if state.settled {
            return;
        }
        // The level was never valid: whatever a pre-verdict task made of it
        // is not the load's failure.
        state.held_failure = None;
        self.settle(&mut state, Err(LevelLoadFailure::parse(error)));
    }

    /// Main thread: a task that panicked fails the load as its stage.
    pub(super) fn poll_task_handles(&self) {
        let panics: Vec<LevelLoadFailure> = {
            let mut state = self.lock();
            let mut panics = Vec::new();
            state
                .handles
                .retain_mut(|(stage, handle)| match handle.try_take() {
                    None => true,
                    Some(Err(TaskError::Panicked(payload))) => {
                        panics.push(LevelLoadFailure::panicked(*stage, &payload));
                        false
                    }
                    Some(Ok(()) | Err(TaskError::Cancelled)) => false,
                });
            panics
        };
        for failure in panics {
            let mut state = self.lock();
            self.fail(&mut state, failure);
        }
    }

    /// The outcome, once, and whether the parse accepted the level, both
    /// read under one lock.
    pub(super) fn take_outcome_and_verdict(
        &self,
    ) -> (Option<Result<PreparedLevel, LevelLoadFailure>>, bool) {
        let mut state = self.lock();
        (state.outcome.take(), state.parse_accepted)
    }

    /// The load was cancelled: stop, cancel the build, drop everything,
    /// report nothing.
    pub(super) fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
        self.upload.stop();
        let mut state = self.lock();
        state.settled = true;
        state.outcome = None;
        state.held_failure = None;
        state.clear_products();
    }

    /// Whether the parse returned an accepting verdict: from then on, every
    /// hand-out a pre-verdict task or part read is final.
    pub(super) fn parse_verdict_accepted(&self) -> bool {
        self.lock().parse_accepted
    }

    #[cfg(test)]
    pub(super) fn probe_state(&self) -> super::test_support::ProbeState {
        let state = self.lock();
        super::test_support::ProbeState {
            started: state.started.clone(),
            finished: state.finished.clone(),
            last_hand_out: state.last_hand_out,
            parse_accepted: state.parse_accepted,
            held_failure: state.held_failure.is_some(),
            settled: state.settled,
        }
    }
}

/// A finished prep task (or a part it owed the set builder) left nothing:
/// a coordinator bug, reported as one load failure rather than a panic.
fn missing_product() -> LevelLoadFailure {
    LevelLoadFailure::prep(PrepKind::Assemble, "a finished prep task left no product")
}
