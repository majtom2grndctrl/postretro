//! Level-load stages that run off the main thread: the prep stage, driven by
//! the staged parse on the engine load pool, which runs every level-data
//! check and builds every CPU product the upload stage and the commit
//! consume (the level's decoded sounds and predicted, parsed models
//! included), handing each GPU-bound product to the renderer's off-main set
//! build as soon as it exists; and the load-stage handle that joins the two
//! into one bundle for the commit. The App's load state machine
//! (`startup::level_loads`) dispatches and polls these stages; the level
//! worker and the main-thread install remain only as the tests' oracle.
//! See: context/lib/boot_sequence.md §9 · context/plans/in-progress/level-load-pipeline/index.md (Staged pipeline)

mod commit;
mod coordinator;
mod failure;
mod inputs;
mod model_cache;
mod model_key;
mod model_prediction;
mod products;
mod stages;
mod tasks;
mod upload;

#[cfg(test)]
mod failure_tests;
#[cfg(test)]
mod gpu_gate_tests;
#[cfg(test)]
mod model_cache_tests;
#[cfg(test)]
pub(crate) mod model_tests;
#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod sound_tests;
#[cfg(test)]
mod stages_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use postretro_level_loader::{LevelLoadProgress, LoadCancel, LoadPhase};

use crate::load_pool::{CancelToken, GroupRetirement, TaskGroup};

pub(crate) use commit::{LevelCommitReport, add_creation_counts};
pub(crate) use failure::{LevelLoadFailure, LoadStage, PrepKind};
pub(crate) use inputs::{LevelPrepInputs, LevelTiers};
#[cfg(test)]
pub(crate) use model_cache::ModelCacheUse;
pub(crate) use model_cache::ModelMissFallback;
#[cfg(test)]
pub(crate) use model_key::MODEL_CONTENT_KEY_VERSION;
pub(crate) use model_key::ModelInstallParams;
pub(crate) use products::{
    PrepTimings, PreparedGpuLevel, PreparedLevel, PreparedModel, PreparedModels,
    PreparedPresentation,
};
#[cfg(test)]
pub(crate) use stages::start_level_load_stages;
pub(crate) use stages::{BundleSet, LevelBundle, LevelLoadStages, LoadRetirement};
pub(crate) use tasks::level_texture_materials;

use coordinator::PrepShared;

/// The staged parse polls the engine pool's token between stages.
impl LoadCancel for CancelToken {
    fn is_cancelled(&self) -> bool {
        CancelToken::is_cancelled(self)
    }
}

/// Start one level's prep in `group`, which the prep owns from here: one
/// `Load` task runs the staged parse, and each prep task is spawned from its
/// sink as soon as the hand-outs it reads exist. Poll [`LevelPrep::try_take`]
/// from the main thread; nothing here or there blocks.
///
/// `progress` is the whole load's counter: the parse credits
/// [`LoadPhase::Parse`] in bytes, prep credits [`LoadPhase::Prep`] in tasks
/// finished.
pub(crate) fn start_level_prep(
    group: TaskGroup,
    inputs: LevelPrepInputs,
    progress: Arc<LevelLoadProgress>,
) -> LevelPrep {
    let shared = PrepShared::new(inputs, group.spawner(), &progress);
    shared.start_parse();
    LevelPrep {
        shared,
        group,
        taken: false,
    }
}

/// One level's prep in flight. Owns the load's task group.
///
/// It ends one of three ways, each reported at most once:
/// - [`try_take`](Self::try_take) yields `Ok(PreparedLevel)`;
/// - it yields `Err(LevelLoadFailure)`: the first failing task, the parse
///   included, wins, the group is cancelled at once, and every later failure
///   is dropped and reported nowhere;
/// - it is cancelled ([`cancel`](Self::cancel)): it then yields neither,
///   ever. The caller polls the returned retirement; this type keeps no
///   cancelled state to poll. A pool shutdown under it likewise leaves it
///   yielding nothing.
#[derive(Debug)]
pub(crate) struct LevelPrep {
    shared: Arc<PrepShared>,
    group: TaskGroup,
    taken: bool,
}

impl LevelPrep {
    /// The prepared level or the load's one failure, once; `None` before
    /// that and after it was taken. Never blocks. A task that panicked
    /// surfaces here, through its handle, as that task's failure.
    pub(crate) fn try_take(&mut self) -> Option<Result<PreparedLevel, LevelLoadFailure>> {
        if self.taken {
            return None;
        }
        self.shared.poll_task_handles();
        let outcome = self.shared.take_outcome()?;
        self.taken = true;
        // A failure cancels the group now: its queued tasks drop and its
        // running ones see their token. On success every task has finished.
        if outcome.is_err() {
            self.group.cancel_in_place();
        }
        Some(outcome)
    }

    /// Cancel the load: the parse stops at its next stage boundary, queued
    /// tasks drop unrun, no new task spawns, and nothing is reported. Also
    /// how a prep that yielded hands back its retirement. Never waits; poll
    /// the retirement on later frames.
    pub(crate) fn cancel(self) -> GroupRetirement {
        self.shared.cancel();
        self.group.cancel()
    }

    /// [`Self::cancel`], keeping the prep: it stops now and reports
    /// nothing, and `cancel` still hands back its retirement later. For an
    /// owner whose load already failed elsewhere (the upload).
    pub(crate) fn cancel_in_place(&self) {
        self.shared.cancel();
        self.group.cancel_in_place();
    }

    /// Whether the parse returned an accepting verdict.
    pub(crate) fn parse_accepted(&self) -> bool {
        self.shared.parse_verdict_accepted()
    }

    /// The prep's live state, for tests that watch which tasks ran.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> test_support::PrepProbe {
        test_support::PrepProbe::new(Arc::clone(&self.shared))
    }
}

/// Plan prep's share of the bar: `kinds` tasks, each credited as it
/// finishes. An empty prep set credits its whole share on entry.
fn enter_prep_progress(progress: &LevelLoadProgress, kinds: usize) {
    let prep = progress.phase(LoadPhase::Prep);
    prep.begin(kinds as u64);
    if kinds == 0 {
        prep.finish();
    }
}
