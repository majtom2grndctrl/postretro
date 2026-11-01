//! The upload stage's app side: the builder prep tasks hand each GPU-bound
//! product to as soon as it exists, from the pool thread that made it, and
//! the upload phase's progress in parts the upload thread finished.
//! See: context/plans/done/level-load-pipeline/index.md (Staged pipeline, Progress covers the whole load)

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use postretro_level_loader::LoadProgress;
use postretro_render_cpu::texture_prep::PreparedWorldTextures;
use postretro_render_data::material::Material;

use super::LevelTiers;
use super::model_cache::ModelPart;
#[cfg(test)]
use crate::render::LevelBuildPart;
use crate::render::{
    LevelBuildEvent, LevelBuildSnapshotInputs, LevelGeometryParts, LevelSetBuilder,
    ModelCacheHandle,
};
use crate::startup::render_profile::{
    renderer_reflection_quality, renderer_spot_shadow_map_resolution,
    renderer_surface_depth_quality,
};

/// The parts every set build submits beside its models: textures, geometry
/// and `finish`. Sprite collections stay at commit (a named, counted
/// exception), so no smoke part is planned.
const FIXED_UPLOAD_PARTS: u64 = 3;

/// The renderer tiers a load builds its set at, from the dispatch snapshot:
/// the shadow tier through the mapping `configure_player_shadow_quality`
/// applies, Surface Depth and reflections through theirs.
pub(crate) fn level_build_snapshot(tiers: LevelTiers) -> LevelBuildSnapshotInputs {
    LevelBuildSnapshotInputs {
        spot_shadow_map_resolution: renderer_spot_shadow_map_resolution(tiers.shadow_quality),
        surface_depth_quality: renderer_surface_depth_quality(tiers.surface_depth_quality),
        reflection_quality: renderer_reflection_quality(tiers.reflection_quality),
    }
}

/// A load's set builder, carried in the prep inputs from dispatch to the
/// prep that submits through it, and the instant prep submitted the build's
/// first part, which the stages read at the set's receipt for log line C.
#[derive(Clone)]
pub(crate) struct UploadBuilder {
    pub(crate) builder: LevelSetBuilder,
    pub(crate) first_part: Arc<OnceLock<Instant>>,
}

impl std::fmt::Debug for UploadBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UploadBuilder")
    }
}

/// Prep's one builder, and the upload phase's progress.
///
/// Prep submits only through [`Self::submit`], under the slot's lock, so once
/// [`Self::stop`] took the builder no part goes out. The slot holds no clone a
/// task keeps past its submit: when prep ends (completes, fails or is
/// cancelled), the slot's builder is the last one prep held.
pub(super) struct UploadSlot {
    /// Whether the load builds a set at all: fixed at dispatch. A
    /// renderer-less load runs prep only and keeps its GPU-bound products.
    active: bool,
    builder: Mutex<Option<LevelSetBuilder>>,
    /// The build's renderer's model cache, captured with the builder. A
    /// handle keeps nothing alive.
    model_cache: Option<ModelCacheHandle>,
    tracker: Arc<UploadTracker>,
    /// Set by the first submit: where line C's `upload` mark starts.
    first_part: Option<Arc<OnceLock<Instant>>>,
}

impl UploadSlot {
    /// The slot for `builder`, its events credited to `progress` (the
    /// load's [`postretro_level_loader::LoadPhase::Upload`] counter). Without
    /// a builder the slot credits nothing: prep alone never touches the
    /// upload phase.
    pub(super) fn new(
        upload: Option<UploadBuilder>,
        progress: Arc<LoadProgress>,
        #[cfg(test)] on_part_done: Option<super::test_support::UploadPartHook>,
    ) -> Self {
        let (builder, first_part) = upload
            .map(|upload| (upload.builder, upload.first_part))
            .unzip();
        let tracker = Arc::new(UploadTracker {
            progress,
            state: Mutex::new(TrackerState::default()),
            #[cfg(test)]
            on_part_done,
        });
        if let Some(builder) = &builder {
            // The hook holds the tracker only, never a builder or the prep:
            // the build outlives neither through it.
            let hook = Arc::clone(&tracker);
            builder.on_event(move |event| hook.event(event));
        }
        Self {
            active: builder.is_some(),
            model_cache: builder.as_ref().map(LevelSetBuilder::model_cache),
            builder: Mutex::new(builder),
            tracker,
            first_part,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    fn lock(&self) -> MutexGuard<'_, Option<LevelSetBuilder>> {
        self.builder.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Submit through the builder, unless prep stopped. Queuing a part is a
    /// channel send: nothing here waits on the upload thread.
    fn submit(&self, part: impl FnOnce(&LevelSetBuilder)) -> bool {
        match self.lock().as_ref() {
            Some(builder) => {
                if let Some(first_part) = &self.first_part {
                    first_part.get_or_init(Instant::now);
                }
                part(builder);
                true
            }
            None => false,
        }
    }

    /// The world textures' part, from the task that prepared them. Moves
    /// the prepared sidecars into the build; `materials` is cloned, since
    /// the geometry part binds them too.
    pub(super) fn textures(&self, textures: PreparedWorldTextures, materials: &[Material]) {
        self.submit(|builder| builder.textures(textures, materials.to_vec()));
    }

    /// One prepared model's part, from its prep task: a cache hit shares
    /// the cached entry; a parse uploads, offering its entry to the cache
    /// when it was keyed.
    pub(super) fn model(&self, handle: &str, part: ModelPart) {
        self.submit(|builder| match part {
            ModelPart::Cached(cached) => builder.cached_model(handle, cached),
            ModelPart::Upload {
                model,
                materials,
                offer: Some(offer),
            } => builder.offered_model(handle, *model, materials, offer),
            ModelPart::Upload {
                model,
                materials,
                offer: None,
            } => builder.model(handle, *model, materials),
        });
    }

    /// The model cache of the renderer the build belongs to; `None` for a
    /// load that builds no set, which then keys and caches nothing.
    pub(super) fn model_cache(&self) -> Option<&ModelCacheHandle> {
        self.model_cache.as_ref()
    }

    /// The geometry part, once its inputs exist.
    pub(super) fn geometry(&self, parts: LevelGeometryParts) {
        self.submit(|builder| builder.geometry(parts));
    }

    /// `finish`, once every other part's submit returned.
    pub(super) fn finish(&self) {
        self.submit(LevelSetBuilder::finish);
    }

    /// Plan the upload phase: the fixed parts plus one per predicted model.
    pub(super) fn plan(&self, predicted_models: usize) {
        if self.active {
            self.tracker
                .plan(FIXED_UPLOAD_PARTS + predicted_models as u64);
        }
    }

    /// A predicted model that failed to parse submits nothing.
    pub(super) fn forgo_model(&self) {
        if self.active {
            self.tracker.forgo(1);
        }
    }

    /// Prep ended (failure or cancel): cancel the build, so its queued
    /// parts do nothing, and drop prep's builder. Idempotent.
    pub(super) fn stop(&self) {
        self.tracker.stop();
        if let Some(builder) = self.lock().take() {
            builder.cancel();
        }
    }

    /// Prep completed with every part submitted: drop prep's builder and
    /// let the build run to its set.
    pub(super) fn release(&self) {
        drop(self.lock().take());
    }

    #[cfg(test)]
    pub(super) fn tracker(&self) -> &UploadTracker {
        &self.tracker
    }

    /// A second textures part, which the build refuses: a real upload-thread
    /// failure injected through the public builder.
    #[cfg(test)]
    pub(super) fn duplicate_textures(&self) {
        let empty = postretro_render_cpu::texture_prep::prepare_world_textures(
            &[],
            &Default::default(),
            std::path::Path::new("upload-test-no-prm-root"),
        );
        self.submit(|builder| builder.textures(empty, Vec::new()));
    }

    /// A part that raises `fault` on the upload thread.
    #[cfg(test)]
    pub(super) fn fault(&self, fault: super::test_support::UploadFault) {
        use super::test_support::UploadFault;
        self.submit(|builder| match fault {
            UploadFault::WritePastBufferEnd => builder.fault_write_past_buffer_end(),
            UploadFault::Panic => builder.fault_panic_in_part(),
        });
    }
}

/// The upload phase's counter, fed from the build's events on the upload
/// thread. Units: build parts finished (each part's writes submitted).
pub(super) struct UploadTracker {
    progress: Arc<LoadProgress>,
    state: Mutex<TrackerState>,
    /// Test seam: runs on the upload thread as each part reports, before
    /// the tracker's lock is taken; blocking here holds the upload thread.
    #[cfg(test)]
    on_part_done: Option<super::test_support::UploadPartHook>,
}

#[derive(Default)]
struct TrackerState {
    /// The part count is known (the model prediction landed).
    planned: bool,
    /// Parts finished before the plan, credited when it lands.
    early: u64,
    /// Prep ended: credit nothing more.
    stopped: bool,
    #[cfg(test)]
    done: Vec<LevelBuildPart>,
    #[cfg(test)]
    failed: bool,
}

impl UploadTracker {
    fn lock(&self) -> MutexGuard<'_, TrackerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fix the phase's total before its first credit; parts finished
    /// earlier are credited now.
    fn plan(&self, parts: u64) {
        let mut state = self.lock();
        if state.planned || state.stopped {
            return;
        }
        state.planned = true;
        self.progress.begin(parts);
        self.progress.advance(std::mem::take(&mut state.early));
    }

    fn forgo(&self, parts: u64) {
        let state = self.lock();
        if state.planned && !state.stopped {
            self.progress.forgo(parts);
        }
    }

    fn stop(&self) {
        self.lock().stopped = true;
    }

    fn event(&self, event: LevelBuildEvent) {
        #[cfg(test)]
        if let (LevelBuildEvent::PartDone(part), Some(hook)) = (event, &self.on_part_done) {
            hook(part);
        }
        let mut state = self.lock();
        match event {
            LevelBuildEvent::PartDone(part) => {
                #[cfg(test)]
                state.done.push(part);
                #[cfg(not(test))]
                let _ = part;
                if state.stopped {
                    return;
                }
                if state.planned {
                    self.progress.advance(1);
                } else {
                    state.early += 1;
                }
            }
            LevelBuildEvent::Failed => {
                #[cfg(test)]
                {
                    state.failed = true;
                }
            }
        }
    }

    /// The parts the upload thread finished so far, in order.
    #[cfg(test)]
    pub(super) fn parts_done(&self) -> Vec<LevelBuildPart> {
        self.lock().done.clone()
    }

    /// Whether the build reported a failure (a cancel reports none).
    #[cfg(test)]
    pub(super) fn failed(&self) -> bool {
        self.lock().failed
    }
}
