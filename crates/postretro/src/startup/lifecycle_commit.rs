//! A level load's commit: the one frame that installs a bundle the load's
//! stages built off the main thread. It runs the install's main-pinned stages
//! in the main-thread install's order, through that install's own stage
//! functions, with each product the bundle holds in place of computing it, and
//! creates no GPU object outside its counted fallbacks.
//! See: context/lib/boot_sequence.md §3 · context/plans/in-progress/level-load-pipeline/index.md (Commit holds only main-pinned work, The commit cannot fail on level data, Settings and scene extent, dispatch to commit)

use super::lifecycle_install::{LevelLightSeed, LevelPresentation};
use super::lifecycle_install_presentation::{LevelFogMasks, LevelSounds};
use super::lifecycle_model_sweep::{LevelModels, PreparedModelSweep};
use super::*;
use crate::level_load::{
    BundleSet, CheckedLevelBundle, CommitBundle, LevelBundle, LevelCommitReport, LevelLoadFailure,
    ModelInstallParams, ModelMissFallback, ModelOverPredictions,
};
use crate::render::{LevelSetId, ModelCacheCommit};
use crate::session::level_streaming::{
    PreparedLevelStreaming, StreamingLevelData, prepare_level_streaming,
};
use crate::startup::render_profile::renderer_surface_depth_quality;
use postretro_renderer::{PreviousLevelSet, gpu_creation_counts};

/// What the commit's checks hand on: the set to publish (`None` for a
/// renderer-less commit) and the level's streaming sessions, built against
/// that set.
struct LevelCommitChecks {
    set: Option<LevelSetId>,
    streaming: Option<PreparedLevelStreaming>,
}

impl App {
    /// Install `bundle` in one frame: the load state machine calls this on
    /// the Loading frame its bundle is ready, and the beside order on its
    /// caller's commit, with the bundle already checked. The main-thread
    /// oracle, `install_level_payload`, shares the identity, segment A and
    /// world stages, not the checks or the set commit.
    ///
    /// Stages, in the main-thread install's order, with what each takes from
    /// the bundle:
    /// 1. The checks that can still fail, before anything mutates
    ///    ([`Self::check_level_bundle`]; an already checked bundle skips
    ///    them): the bundle's set still resident on a full renderer, and the
    ///    level's streaming sessions built against that set's residency
    ///    snapshot from `bundle.streaming`. None reads level data; the range
    ///    and collision checks, like every level-data check, ran in prep. A
    ///    failure releases the bundle's set and returns the load's one
    ///    `LevelLoadFailure`, with nothing else changed and no parity
    ///    published.
    /// 2. Identity: environment volumes, the planar verdict, the level tags,
    ///    then parity, the join seed and the relevel id (pin P14).
    /// 3. Segment A: gravity and navigation.
    /// 4. The renderer stage: publish the set, enter its models' offers in
    ///    the cache, apply the planar verdict, read the light seed. A
    ///    prep-only bundle (no renderer at dispatch) takes the renderer-less
    ///    route instead and installs the level and `bundle.collision` alone.
    /// 5. `bundle.world` becomes `App::level`; the light bridge; segment B,
    ///    which installs `bundle.collision` before its mover build and whose
    ///    model sweep installs each prepared model's clip entry and hit zones
    ///    (a model the prediction missed loads synchronously, counted, a
    ///    cache hit when any entry holds its content; one it over-predicted
    ///    leaves the published set, counted, creating nothing); the
    ///    post-load passes; the camera pose; the fog masks (CPU only: the set
    ///    holds the target and masks); sprite collections (counted); the
    ///    bundle's decoded sounds. Then the cache's one sweep, after the
    ///    model install: an entry only a released level held stays
    ///    hittable until the miss fallback ran.
    /// 6. `finish_level_payload`'s tail: loading-tree images promoted, the
    ///    streaming sessions from step 1 installed, then the CPU timer, the
    ///    host's own pawn, the load's end and Settling.
    ///
    /// Past step 1 nothing returns an error: the streaming install fails on
    /// nothing, and no stage function after it returns a `Result`.
    pub(crate) fn commit_level_bundle(
        &mut self,
        bundle: impl Into<CommitBundle>,
    ) -> Result<LevelCommitReport, LevelLoadFailure> {
        let commit_start = gpu_creation_counts();
        // Before anything mutates: every check that can still fail, unless
        // the beside order ran them before it released the running level.
        let checked = match bundle.into() {
            CommitBundle::Checked(checked) => *checked,
            CommitBundle::Unchecked(bundle) => match self.check_level_bundle(*bundle) {
                Err(failure) => {
                    return Err(failure);
                }
                Ok(checked) => {
                    self.level_timings.record("commit_checks");
                    checked
                }
            },
        };
        let CheckedLevelBundle {
            world,
            collision,
            presentation,
            tiers,
            prm_cache_root,
            set,
            streaming,
        } = checked;
        self.install_level_identity(&world);
        let script_ctx = self.install_level_segment_a(&world);
        let mut report = LevelCommitReport::default();
        match set {
            None => self.install_level_without_renderer(world, collision),
            Some(id) => {
                let light_seed = self.commit_level_set(id, &world, &mut report);
                let mut fallback = ModelMissFallback::default();
                let mut over_predictions = ModelOverPredictions::default();
                let params = ModelInstallParams::at_tier(renderer_surface_depth_quality(
                    tiers.surface_depth_quality,
                ));
                report.sprite_created = self.install_level_world_stages(
                    world,
                    &script_ctx,
                    light_seed,
                    collision,
                    &prm_cache_root,
                    LevelPresentation {
                        models: LevelModels::Prepared(PreparedModelSweep {
                            models: presentation.models,
                            params,
                            fallback: &mut fallback,
                            fallback_created: &mut report.model_miss_created,
                            over_predictions: &mut over_predictions,
                        }),
                        fog_masks: LevelFogMasks::InSet,
                        sounds: LevelSounds::Decoded(presentation.sounds),
                    },
                );
                report.model_misses = fallback.misses();
                report.model_over_predictions = over_predictions.count();
                self.sweep_committed_model_cache(&mut report);
            }
        }
        // A level tree drawing a loading-only image keeps it loaded.
        self.promote_level_tree_images();
        // The level's streaming sessions exist before Settling asks them
        // anything, so a missing session means "not streamed".
        self.install_prepared_level_streaming_sessions(streaming);
        self.enter_installed_level_settling();
        report.created = gpu_creation_counts().since(commit_start);
        Ok(report)
    }

    /// The commit's checks on `bundle` ([`Self::check_level_commit`]),
    /// before anything mutates: the bundle, checked, for
    /// [`Self::commit_level_bundle`], or the load's one failure with the
    /// bundle's own set released, since nothing commits. They read only the
    /// bundle's unpublished set and its streaming data, touch no session
    /// state and start no thread, so the beside order runs them while the
    /// running level still runs, before it releases that level.
    pub(super) fn check_level_bundle(
        &mut self,
        bundle: LevelBundle,
    ) -> Result<CheckedLevelBundle, LevelLoadFailure> {
        let LevelBundle {
            world,
            collision,
            streaming,
            presentation,
            timings: _,
            upload_time: _,
            tiers,
            prm_cache_root,
            set,
        } = bundle;
        match self.check_level_commit(&world, &set, streaming) {
            Ok(checks) => Ok(CheckedLevelBundle {
                world,
                collision,
                presentation,
                tiers,
                prm_cache_root,
                set: checks.set,
                streaming: checks.streaming,
            }),
            Err(failure) => {
                // Nothing commits: the bundle's own set goes with it.
                if let (BundleSet::Resident(id), Some(renderer)) = (&set, self.renderer.as_mut()) {
                    renderer.release_level_set(*id);
                }
                Err(failure)
            }
        }
    }

    /// The commit's checks, before it mutates anything: a full renderer
    /// still holding the bundle's set, built and unpublished, and the
    /// level's streaming sessions, built against that set's residency
    /// snapshot (`prepare_level_streaming`: the snapshot's presence, the
    /// budget it yields, the generation clocks). None reads level data:
    /// every level-data check ran in prep. A prep-only bundle checks
    /// nothing; its renderer-less route creates no session.
    fn check_level_commit(
        &self,
        world: &postretro_level_loader::LevelWorld,
        set: &BundleSet,
        streaming: StreamingLevelData,
    ) -> Result<LevelCommitChecks, LevelLoadFailure> {
        let id = match set {
            BundleSet::PrepOnly(_) => {
                return Ok(LevelCommitChecks {
                    set: None,
                    streaming: None,
                });
            }
            BundleSet::Resident(id) => *id,
        };
        let Some(renderer) = self
            .renderer
            .as_ref()
            .filter(|renderer| renderer.is_full_ready())
        else {
            return Err(LevelLoadFailure::commit(
                "the renderer that built the level's set is gone",
            ));
        };
        if !renderer.resident_level_sets().contains(&id) {
            return Err(LevelLoadFailure::commit(format!(
                "the level's set {id:?} is no longer resident"
            )));
        }
        let streaming = prepare_level_streaming(
            world,
            renderer.level_set_sh_residency_snapshot(id),
            streaming,
        )
        .map_err(|error| LevelLoadFailure::commit(format!("{error:#}")))?;
        Ok(LevelCommitChecks {
            set: Some(id),
            streaming: Some(streaming),
        })
    }

    /// The commit's renderer stage. Publish the bundle's set, retiring the
    /// active one: the set already holds the level's textures, normalized
    /// geometry, fluid surfaces, fog target and masks and its models'
    /// entries, built off the main thread at the dispatch tiers, so no live
    /// tier applies here (pin P17). The publish compares the extent the set
    /// was built at with the renderer's committed scene extent and rebuilds
    /// once if they differ (pin P16; a resize recorded but not yet committed
    /// is the next frame's, pin P23). Then the cache step enters the set's
    /// new model entries, freeing nothing: the commit sweeps once after its
    /// model install ([`Self::sweep_committed_model_cache`]), so the miss
    /// fallback can still hit an entry only the released level held, and
    /// this load's own offers. The level's planar verdict applies after the
    /// publish lent the set's planar targets, so a verdict without a
    /// candidate releases them. The light seed reads the published set's
    /// scripted-sample offset.
    fn commit_level_set(
        &mut self,
        id: LevelSetId,
        world: &postretro_level_loader::LevelWorld,
        report: &mut LevelCommitReport,
    ) -> LevelLightSeed {
        let renderer = self
            .renderer
            .as_mut()
            .expect("the commit's checks found the renderer");
        let extent_rebuilds = renderer.extent_rebuilds();
        let reflection_tier_rebuilds = renderer.reflection_tier_rebuilds();
        let before = gpu_creation_counts();
        renderer.publish_level_set(id, PreviousLevelSet::Retire);
        report.publish_created = gpu_creation_counts().since(before);
        report.extent_rebuilds = renderer.extent_rebuilds() - extent_rebuilds;
        report.reflection_tier_rebuilds =
            renderer.reflection_tier_rebuilds() - reflection_tier_rebuilds;
        report.model_cache =
            renderer
                .enter_model_cache_offers(id)
                .map(|inserted| ModelCacheCommit {
                    inserted,
                    ..ModelCacheCommit::default()
                });
        report.set = Some(id);
        self.sync_planar_candidate();
        self.level_timings.record("set_publish");
        self.install_level_renderer_tail(world)
    }

    /// The commit's one model-cache sweep, after its model install: frees
    /// every entry nothing references any more (one only the released level
    /// held that the miss fallback did not hit, an over-prediction's) and
    /// completes the report's cache step. Creates nothing.
    fn sweep_committed_model_cache(&mut self, report: &mut LevelCommitReport) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(freed) = renderer.sweep_model_cache() else {
            return;
        };
        if let Some(cache) = report.model_cache.as_mut() {
            cache.freed = freed;
            cache.retained = renderer.model_cache_keys().len();
        }
    }
}
