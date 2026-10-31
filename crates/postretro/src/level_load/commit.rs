//! What a level load's commit reports: the set it published, and every GPU
//! object it created on the main thread, by counted fallback.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Commit holds only main-pinned work, Off the main thread)

use crate::render::{LevelSetId, ModelCacheCommit};
use postretro_renderer::GpuCreationCounts;

/// One commit's report. The commit creates no GPU object of its own; what
/// it creates belongs to one of the counted fallbacks here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LevelCommitReport {
    /// The set the commit published; `None` for a renderer-less commit.
    pub(crate) set: Option<LevelSetId>,
    /// Every GPU object created on the main thread across the commit.
    pub(crate) created: GpuCreationCounts,
    /// What the publish created: only an extent rebuild or a reflection
    /// tier reconcile does, each counted below.
    pub(crate) publish_created: GpuCreationCounts,
    /// Publishes that rebuilt the set's extent-bound groups and fog target:
    /// the scene extent the renderer committed moved since the build.
    pub(crate) extent_rebuilds: u64,
    /// Publishes that re-keyed the set's reflection targets: the renderer's
    /// kept reflection tier moved since the build.
    pub(crate) reflection_tier_rebuilds: u64,
    /// Distinct models the commit's real sweep held that the prediction
    /// missed, each loaded synchronously.
    pub(crate) model_misses: usize,
    pub(crate) model_miss_created: GpuCreationCounts,
    /// Distinct models the load predicted and built that the real sweep
    /// does not draw, each released from the published set (creating
    /// nothing).
    pub(crate) model_over_predictions: usize,
    /// Cache entries that release freed: nothing else referenced them.
    pub(crate) model_over_prediction_freed: usize,
    /// Sprite-collection registration: a named exception the commit keeps.
    pub(crate) sprite_created: GpuCreationCounts,
    /// The model cache step after the publish.
    pub(crate) model_cache: Option<ModelCacheCommit>,
}

impl LevelCommitReport {
    /// What the commit created outside every counted fallback: zero by the
    /// commit's contract. A publish that created something without counting
    /// an extent rebuild or a tier reconcile counts here.
    pub(crate) fn uncounted_creations(&self) -> GpuCreationCounts {
        let publish = if self.extent_rebuilds + self.reflection_tier_rebuilds > 0 {
            self.publish_created
        } else {
            GpuCreationCounts::default()
        };
        self.created.since(add_creation_counts(
            publish,
            add_creation_counts(self.model_miss_created, self.sprite_created),
        ))
    }
}

/// `a` plus `b`, kind by kind.
pub(crate) fn add_creation_counts(a: GpuCreationCounts, b: GpuCreationCounts) -> GpuCreationCounts {
    GpuCreationCounts {
        buffers: a.buffers + b.buffers,
        textures: a.textures + b.textures,
        bind_groups: a.bind_groups + b.bind_groups,
        pipelines: a.pipelines + b.pipelines,
    }
}
