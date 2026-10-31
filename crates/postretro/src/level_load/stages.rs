//! One load's stages behind one handle: the prep on the engine pool and the
//! renderer's set build on the upload thread, ending in one bundle the
//! commit consumes, or in the load's one failure.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Staged pipeline, The commit cannot fail on level data, Cancellation) · research.md §9 (P10, P13, P15)

use std::path::PathBuf;
use std::sync::Arc;

use postretro_level_loader::{LevelLoadProgress, LevelWorld, LoadPhase};

use super::upload::{UploadBuilder, level_build_snapshot};
use super::{
    LevelLoadFailure, LevelPrep, LevelPrepInputs, LevelTiers, PrepTimings, PreparedGpuLevel,
    PreparedLevel, PreparedPresentation,
};
use crate::App;
use crate::collision::CollisionWorld;
use crate::load_pool::{GroupRetirement, TaskGroup};
use crate::render::{
    LevelBuildError, LevelSetBuilder, LevelSetBuilderProbe, LevelSetId, PendingLevelSet, Renderer,
};
use crate::session::level_streaming::StreamingLevelData;

/// Everything one load made, ready for the commit: prep's main-pinned
/// products and the load's renderer set, resident and unpublished.
#[derive(Debug)]
pub(crate) struct LevelBundle {
    /// The assembled level, UVs normalized, its GPU-only lighting payloads
    /// taken: today's `App::level`.
    pub(crate) world: LevelWorld,
    /// The static collision world.
    pub(crate) collision: CollisionWorld,
    /// What the commit creates the level's streaming sessions from.
    pub(crate) streaming: StreamingLevelData,
    /// The decoded sounds and every predicted model's clip-table entry and
    /// hit zones (the prediction, for counting misses against the real
    /// sweep).
    pub(crate) presentation: PreparedPresentation,
    pub(crate) timings: PrepTimings,
    /// The tiers the load was dispatched at, which every stage built at.
    pub(crate) tiers: LevelTiers,
    pub(crate) set: BundleSet,
}

/// The renderer side of a bundle.
#[derive(Debug)]
pub(crate) enum BundleSet {
    /// The load's set, built off-main and inserted into the renderer's
    /// resident sets, unpublished. The commit publishes it; a bundle that
    /// never commits releases it (`Renderer::release_level_set`).
    Resident(LevelSetId),
    /// No renderer at dispatch: prep ran alone and kept its GPU-bound
    /// products, which nothing uploaded.
    PrepOnly(Box<PreparedGpuLevel>),
}

/// Start one load's stages: prep in `group` on the pool, and, with
/// `upload`, the set build begun at dispatch (`Renderer::begin_level_set_build`
/// at the dispatch tiers), whose builder prep's tasks submit through. Poll
/// [`LevelLoadStages::try_take`] from the main thread; nothing waits.
///
/// `progress` is the whole load's counter: prep credits parse and prep, the
/// build credits [`LoadPhase::Upload`] in parts finished. Without `upload`
/// the upload phase has no work and is credited whole on entry.
pub(crate) fn start_level_load_stages(
    group: TaskGroup,
    mut inputs: LevelPrepInputs,
    upload: Option<(LevelSetBuilder, PendingLevelSet)>,
    progress: Arc<LevelLoadProgress>,
) -> LevelLoadStages {
    let tiers = inputs.tiers;
    let upload = upload.map(|(builder, pending)| {
        inputs.upload = Some(UploadBuilder(builder.clone()));
        UploadHandle {
            probe: builder.probe(),
            builder: Some(builder),
            pending: Some(pending),
            received: None,
        }
    });
    if upload.is_none() {
        // No set to build: the upload phase has no work, credited whole on
        // entry (pin P15).
        progress.phase(LoadPhase::Upload).finish();
    }
    let prep = super::start_level_prep(group, inputs, Arc::clone(&progress));
    LevelLoadStages {
        prep,
        prepared: None,
        upload,
        held_upload_failure: None,
        build_cancelled: false,
        progress,
        tiers,
        ended: false,
        #[cfg(test)]
        withhold_prep: false,
    }
}

impl App {
    /// Dispatch one load's stages: capture the prep inputs and, with a
    /// fully initialized renderer, begin its set build at the dispatch tiers
    /// (creating nothing on this thread). `None` before the renderer or the
    /// session exists, as the capture is. A renderer still booting builds no
    /// set: the load runs prep only.
    pub(crate) fn begin_level_load_stages(
        &mut self,
        group: TaskGroup,
        map_path: PathBuf,
        content_root: PathBuf,
        progress: Arc<LevelLoadProgress>,
    ) -> Option<LevelLoadStages> {
        let inputs = self.capture_level_prep_inputs(map_path, content_root)?;
        let upload = self
            .renderer
            .as_mut()
            .filter(|renderer| renderer.is_full_ready())
            .map(|renderer| renderer.begin_level_set_build(level_build_snapshot(inputs.tiers)));
        Some(start_level_load_stages(group, inputs, upload, progress))
    }
}

/// The main thread's half of the set build.
struct UploadHandle {
    /// The handle's own builder clone, kept only to cancel the build.
    builder: Option<LevelSetBuilder>,
    probe: LevelSetBuilderProbe,
    pending: Option<PendingLevelSet>,
    /// The set, received and resident, until the bundle takes it.
    received: Option<LevelSetId>,
}

/// One load's prep and set build, polled as one.
///
/// It ends one of three ways, each reported at most once:
/// - [`try_take`](Self::try_take) yields the [`LevelBundle`], once prep
///   finished and the set arrived;
/// - it yields the load's one [`LevelLoadFailure`] (pin P10): a prep failure
///   stops the build (no more parts, no `finish`), a build failure cancels
///   prep, and when both fail together prep's is reported. A build failure
///   while the parse's verdict is pending waits for it, so a parse rejection
///   wins, as prep's pre-verdict failures do;
/// - it is cancelled ([`cancel`](Self::cancel)): it reports nothing.
///
/// After an outcome, `cancel` hands back the retirement to poll.
pub(crate) struct LevelLoadStages {
    prep: LevelPrep,
    prepared: Option<PreparedLevel>,
    upload: Option<UploadHandle>,
    /// A build failure waiting for the parse's verdict.
    held_upload_failure: Option<LevelLoadFailure>,
    /// The build reported its cancel: prep failed, and stopped it.
    build_cancelled: bool,
    progress: Arc<LevelLoadProgress>,
    tiers: LevelTiers,
    ended: bool,
    /// Test seam: leave prep's outcome unpolled, so a set can arrive before
    /// the bundle is ready.
    #[cfg(test)]
    withhold_prep: bool,
}

impl std::fmt::Debug for LevelLoadStages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelLoadStages")
            .field("prepared", &self.prepared.is_some())
            .field(
                "received",
                &self.upload.as_ref().and_then(|upload| upload.received),
            )
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl LevelLoadStages {
    /// The bundle or the load's one failure, once; `None` before that and
    /// after it. Never blocks and creates nothing: the set's receipt only
    /// inserts what the upload thread built. A load with a set needs the
    /// renderer to receive it; with `None` (a suspended renderer) the build
    /// is not polled.
    pub(crate) fn try_take(
        &mut self,
        mut renderer: Option<&mut Renderer>,
    ) -> Option<Result<LevelBundle, LevelLoadFailure>> {
        if self.ended {
            return None;
        }
        if let Some(failure) = self.poll_prep() {
            return Some(Err(self.fail(failure, renderer)));
        }
        if let Some(renderer) = renderer.as_deref_mut() {
            self.poll_upload(renderer);
        }
        if self.held_upload_failure.is_some() || self.build_cancelled {
            // A prep failure that stopped the build, or that settled while
            // the build was failing, wins.
            if let Some(failure) = self.poll_prep() {
                return Some(Err(self.fail(failure, renderer)));
            }
            if self.held_upload_failure.is_some()
                && (self.prepared.is_some() || self.prep.parse_accepted())
                && let Some(failure) = self.held_upload_failure.take()
            {
                return Some(Err(self.fail(failure, renderer)));
            }
            return None;
        }
        let set_ready = self
            .upload
            .as_ref()
            .is_none_or(|upload| upload.received.is_some());
        if self.prepared.is_none() || !set_ready {
            return None;
        }
        Some(self.bundle(renderer))
    }

    /// Prep's outcome, once: a success is kept for the bundle, a failure
    /// returned.
    fn poll_prep(&mut self) -> Option<LevelLoadFailure> {
        #[cfg(test)]
        if self.withhold_prep {
            return None;
        }
        if self.prepared.is_some() {
            return None;
        }
        match self.prep.try_take()? {
            Ok(prepared) => {
                self.prepared = Some(prepared);
                None
            }
            Err(failure) => Some(failure),
        }
    }

    fn poll_upload(&mut self, renderer: &mut Renderer) {
        let Some(upload) = self.upload.as_mut() else {
            return;
        };
        let Some(pending) = upload.pending.as_mut() else {
            return;
        };
        match pending.try_take(renderer) {
            None => {}
            Some(Ok(id)) => {
                upload.pending = None;
                upload.received = Some(id);
                // The build is done: the last write was submitted before it
                // reported (pin P13). Upload's share is whole.
                upload.builder = None;
                self.progress.phase(LoadPhase::Upload).finish();
            }
            Some(Err(LevelBuildError::Cancelled)) => {
                upload.pending = None;
                self.build_cancelled = true;
            }
            Some(Err(error)) => {
                upload.pending = None;
                self.held_upload_failure = Some(LevelLoadFailure::upload(&error));
            }
        }
    }

    /// End the load with `failure`: stop prep and the build, release a set
    /// already received. Nothing else is ever reported.
    fn fail(
        &mut self,
        failure: LevelLoadFailure,
        renderer: Option<&mut Renderer>,
    ) -> LevelLoadFailure {
        self.ended = true;
        self.prep.cancel_in_place();
        self.prepared = None;
        self.held_upload_failure = None;
        if let Some(upload) = self.upload.as_mut() {
            upload.stop(renderer);
        }
        failure
    }

    fn bundle(&mut self, renderer: Option<&mut Renderer>) -> Result<LevelBundle, LevelLoadFailure> {
        let Some(prepared) = self.prepared.take() else {
            unreachable!("try_take bundles only once prep yielded");
        };
        let PreparedLevel {
            world,
            gpu,
            collision,
            streaming,
            presentation,
            timings,
        } = prepared;
        // The geometry part dropped its share before the set arrived, and
        // every prep reader before its product was stored.
        let world = match Arc::try_unwrap(world) {
            Ok(world) => world,
            Err(_) => {
                let failure = LevelLoadFailure {
                    stage: super::LoadStage::Upload,
                    message: "the level is still shared after its set arrived".to_string(),
                    parse_error: None,
                };
                return Err(self.fail(failure, renderer));
            }
        };
        let set = match (self.upload.as_mut(), gpu) {
            (Some(upload), _) => {
                let Some(id) = upload.received.take() else {
                    unreachable!("try_take bundles only once the set arrived");
                };
                BundleSet::Resident(id)
            }
            (None, Some(gpu)) => BundleSet::PrepOnly(Box::new(gpu)),
            (None, None) => {
                let failure = LevelLoadFailure {
                    stage: super::LoadStage::Upload,
                    message: "a load without a set builder kept no GPU products".to_string(),
                    parse_error: None,
                };
                return Err(self.fail(failure, renderer));
            }
        };
        self.ended = true;
        Ok(LevelBundle {
            world,
            collision,
            streaming,
            presentation,
            timings,
            tiers: self.tiers,
            set,
        })
    }

    /// Cancel the load, or hand back the retirement of one that ended. It
    /// reports nothing after this. Drops every builder clone (the slot
    /// prep's tasks submit through, and this handle's), cancels the build so
    /// its queued parts do nothing, releases a set already received that no
    /// bundle took, and cancels prep. Never waits: poll the retirement on
    /// later frames. A set received by a renderer that is gone went with it.
    pub(crate) fn cancel(self, renderer: Option<&mut Renderer>) -> LoadRetirement {
        let Self {
            prep, mut upload, ..
        } = self;
        let builder = upload.as_mut().map(|upload| {
            upload.stop(renderer);
            upload.probe.clone()
        });
        drop(upload);
        LoadRetirement {
            group: prep.cancel(),
            builder,
        }
    }

    /// The prep's live state, for tests that watch it run.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> super::test_support::PrepProbe {
        self.prep.probe()
    }

    /// Leave prep's outcome unpolled until `false` again.
    #[cfg(test)]
    pub(crate) fn withhold_prep(&mut self, withhold: bool) {
        self.withhold_prep = withhold;
    }

    /// The set, once received and before a bundle took it.
    #[cfg(test)]
    pub(crate) fn received_set(&self) -> Option<LevelSetId> {
        self.upload.as_ref().and_then(|upload| upload.received)
    }

    /// The build's probe, for tests that check every builder dropped.
    #[cfg(test)]
    pub(crate) fn builder_probe(&self) -> Option<LevelSetBuilderProbe> {
        self.upload.as_ref().map(|upload| upload.probe.clone())
    }
}

impl UploadHandle {
    /// Cancel the build and drop this handle's builder and pending result;
    /// release a received set.
    fn stop(&mut self, renderer: Option<&mut Renderer>) {
        if let Some(builder) = self.builder.take() {
            builder.cancel();
        }
        self.pending = None;
        if let Some(id) = self.received.take()
            && let Some(renderer) = renderer
        {
            renderer.release_level_set(id);
        }
    }
}

/// A cancelled or ended load still winding down: prep's started tasks
/// returning, and the build's queued parts running out on the upload thread.
/// Poll [`Self::try_finish`] on later frames; nothing here waits.
pub(crate) struct LoadRetirement {
    group: GroupRetirement,
    builder: Option<LevelSetBuilderProbe>,
}

impl std::fmt::Debug for LoadRetirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadRetirement")
            .field("group", &self.group)
            .field(
                "build_released",
                &self.builder.as_ref().map(LevelSetBuilderProbe::is_released),
            )
            .finish()
    }
}

impl LoadRetirement {
    /// True once every prep task the load started returned and no builder
    /// or queued part of its build remains.
    pub(crate) fn try_finish(&mut self) -> bool {
        let group = self.group.try_finish();
        let build = self
            .builder
            .as_ref()
            .is_none_or(LevelSetBuilderProbe::is_released);
        group && build
    }
}

// Compile-time guard: the bundle crosses no thread, but everything it holds
// came from the pool and the upload thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LevelBundle>();
};
