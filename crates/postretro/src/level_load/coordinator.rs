//! The prep stage's shared state: the staged parse's hand-outs, each task's
//! product, and the rules that spawn a task once its inputs exist and settle
//! the load's one outcome.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Staged pipeline) · research.md §9 (P10, P18, P19)

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use postretro_level_loader::{
    CompleteHandOuts, EntitiesHandOut, FinalHandOut, GeometryHandOut, HandOut, HandOutStage,
    LevelLoadProgress, LevelWorld, LoadCancel, LoadPhase, LoadProgress, PrlLoadError,
    ShManifestHandOut, SpatialHandOut, StagedParseOutcome,
};
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::lightmap_family::LightmapFamilyPlan;
use postretro_render_cpu::sh_residency::ShResidencyPlan;

use super::tasks::{self, TexturePrep};
use super::{
    LevelLoadFailure, LevelPrepInputs, LoadStage, PrepKind, PrepTimings, PreparedGpuLevel,
    PreparedLevel, PreparedPresentation,
};
use crate::collision::CollisionWorld;
use crate::load_pool::{CancelToken, GroupSpawner, Priority, TaskError, TaskHandle};
use crate::session::level_streaming::StreamingLevelData;

/// What a finished prep task hands back.
enum Product {
    Textures(TexturePrep),
    ShResidency(Box<ShResidencyPlan>),
    RangeChecked,
    Assembled(Box<LevelWorld>),
    Collision(Box<CollisionWorld>),
    Streaming(StreamingLevelData),
    LightmapFamily(LightmapFamilyPlan),
    Fluid(Option<PreparedFluidSurfaces>),
}

/// One load's prep, shared by the parse task, every prep task and the main
/// thread's [`super::LevelPrep`].
pub(super) struct PrepShared {
    pub(super) inputs: LevelPrepInputs,
    spawner: GroupSpawner,
    parse_progress: Arc<LoadProgress>,
    prep_progress: Arc<LoadProgress>,
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
    world_readers_spawned: bool,
    collision: Option<CollisionWorld>,
    streaming: Option<StreamingLevelData>,
    lightmap_family: Option<LightmapFamilyPlan>,
    fluid: Option<Option<PreparedFluidSurfaces>>,

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
        self.collision = None;
        self.streaming = None;
        self.lightmap_family = None;
        self.fluid = None;
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

    fn store(&mut self, product: Product) {
        match product {
            Product::Textures(textures) => self.textures = Some(textures),
            Product::ShResidency(plan) => self.sh_residency = Some(Some(*plan)),
            Product::RangeChecked => self.range_checked = true,
            Product::Assembled(world) => self.world = Some(Arc::new(*world)),
            Product::Collision(collision) => self.collision = Some(*collision),
            Product::Streaming(streaming) => self.streaming = Some(streaming),
            Product::LightmapFamily(plan) => self.lightmap_family = Some(plan),
            Product::Fluid(fluid) => self.fluid = Some(fluid),
        }
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
        inputs: LevelPrepInputs,
        spawner: GroupSpawner,
        progress: &LevelLoadProgress,
    ) -> Arc<Self> {
        super::enter_prep_progress(progress, PrepKind::ALL.len());
        Arc::new(Self {
            inputs,
            spawner,
            parse_progress: Arc::clone(progress.phase(LoadPhase::Parse)),
            prep_progress: Arc::clone(progress.phase(LoadPhase::Prep)),
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
        let shared = Arc::clone(self);
        let handle = self.spawner.spawn(Priority::Load, move |token| {
            shared.run_parse(token);
        });
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
                    self.credit(state, PrepKind::ShResidency, None);
                }
                state.sh_manifest = Some(sh_manifest);
            }
            HandOut::Geometry(geometry) => {
                state.geometry = Some(Arc::clone(&geometry));
                self.spawn(state, PrepKind::Textures, move |shared| {
                    Ok(Product::Textures(tasks::prep_textures(
                        &geometry,
                        &shared.inputs.prm_cache_root,
                    )))
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
            HandOut::Entities(entities) => state.entities = Some(entities),
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
        let handle = self.spawner.spawn(Priority::Load, move |token| {
            if token.is_cancelled() || shared.stopped() {
                return;
            }
            shared.lock().started.insert(kind);
            #[cfg(test)]
            if let Some(message) = shared.inputs.hooks.inject(kind) {
                shared.finish(kind, Duration::ZERO, Err(message));
                return;
            }
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
                state.store(product);
                self.credit(&mut state, kind, Some(elapsed));
                self.advance(&mut state);
            }
        }
    }

    fn credit(&self, state: &mut PrepState, kind: PrepKind, elapsed: Option<Duration>) {
        if state.finished.insert(kind) {
            if let Some(elapsed) = elapsed {
                state.timings.push((kind, elapsed));
            }
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
                // The worker's accepted-parse warning, off the state lock.
                crate::startup::data_script_staleness::report_stale_data_script(
                    &shared.inputs.map_path,
                    world.data_script.as_ref(),
                );
                #[cfg(test)]
                shared.inputs.hooks.edit_world(&mut world);
                Ok(Product::Assembled(Box::new(world)))
            });
        }
        if !state.world_readers_spawned
            && let Some(world) = state.world.clone()
            && let Some(dims) = state
                .textures
                .as_ref()
                .map(|textures| textures.dims.clone())
        {
            state.world_readers_spawned = true;
            self.spawn_world_readers(state, &world, dims);
        }
        if state.finished.len() == PrepKind::ALL.len() {
            self.complete(state);
        }
    }

    /// The tasks that read the assembled level: after the verdict, so on
    /// final data, the manifest's cluster topology installed (pin P19).
    fn spawn_world_readers(
        self: &Arc<Self>,
        state: &mut PrepState,
        world: &Arc<LevelWorld>,
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
                &shared.inputs.device_limits,
                shared.inputs.lightmap_pool_cap_layers,
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

    /// Every kind finished: hand the products on as one prepared level.
    fn complete(&self, state: &mut PrepState) {
        // Each reader dropped its clone before its product was stored, so
        // the state holds the only reference.
        let world = match state.world.take().map(Arc::try_unwrap) {
            Some(Ok(world)) => world,
            _ => {
                let failure = LevelLoadFailure::prep(
                    PrepKind::Assemble,
                    "the assembled level is still shared",
                );
                self.settle(state, Err(failure));
                return;
            }
        };
        let (
            Some(textures),
            Some(sh_residency),
            Some(collision),
            Some(streaming),
            Some(lightmap_family),
            Some(fluid),
        ) = (
            state.textures.take(),
            state.sh_residency.take(),
            state.collision.take(),
            state.streaming.take(),
            state.lightmap_family.take(),
            state.fluid.take(),
        )
        else {
            let failure =
                LevelLoadFailure::prep(PrepKind::Assemble, "a finished prep task left no product");
            self.settle(state, Err(failure));
            return;
        };
        let prepared = PreparedLevel {
            world,
            gpu: PreparedGpuLevel {
                textures: textures.textures,
                texture_materials: textures.materials,
                diffuse_dims: textures.dims,
                sh_residency,
                lightmap_family,
                fluid,
                device_limits: self.inputs.device_limits,
                tiers: self.inputs.tiers,
                prm_cache_root: self.inputs.prm_cache_root.clone(),
            },
            collision,
            streaming,
            presentation: PreparedPresentation::default(),
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
    fn fail(&self, state: &mut PrepState, failure: LevelLoadFailure) {
        if state.settled {
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

    pub(super) fn take_outcome(&self) -> Option<Result<PreparedLevel, LevelLoadFailure>> {
        self.lock().outcome.take()
    }

    /// The load was cancelled: stop, drop everything, report nothing.
    pub(super) fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut state = self.lock();
        state.settled = true;
        state.outcome = None;
        state.held_failure = None;
        state.clear_products();
    }

    #[cfg(test)]
    pub(super) fn probe_state(&self) -> super::test_support::ProbeState {
        let state = self.lock();
        super::test_support::ProbeState {
            started: state.started.clone(),
            finished: state.finished.clone(),
            last_hand_out: state.last_hand_out,
            parse_accepted: state.parse_accepted,
            settled: state.settled,
        }
    }
}
