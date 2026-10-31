// The level load in flight: its generation, whether its stages run yet, and the whole load's progress counter.
// See: context/lib/boot_sequence.md §1, §4 · context/plans/in-progress/level-load-pipeline/index.md (Cancellation, Progress covers the whole load)

use std::path::PathBuf;
use std::sync::Arc;

use postretro_level_loader::LevelLoadProgress;

use crate::level_load::{LevelLoadFailure, LevelLoadStages};

/// The App's level-load state machine. At most one load is active; a
/// cancelled or superseded load leaves only its retirement, which the App's
/// one retirement list polls.
///
/// A load's life:
/// 1. **Queued** ([`ActivePhase::Queued`]): resolved, its loading screen up,
///    no stage work at all (pin P12). It stays here until a full-ready
///    renderer and the session exist and every older generation's stages
///    have joined, so generations never overlap (pin P4) and no load's set
///    build ever begins on a renderer that is not full-ready.
/// 2. **Staged** ([`ActivePhase::Staged`]): dispatched with
///    `App::begin_level_load_stages`, which snapshots the tiers then (pin
///    P21). Every Loading frame polls it without blocking.
/// 3. It ends exactly once: committed on the frame its bundle is ready,
///    failed through `finish_level_failure`, or cancelled (superseded, a
///    queued unload, suspend), which reports nothing (pin P20).
#[derive(Default)]
pub(crate) struct LevelLoads {
    /// The last generation handed out; each load takes the next.
    generation: u64,
    active: Option<ActiveLevelLoad>,
    /// A dispatch that could not start (the engine pool failed to build):
    /// the active load's one failure, routed by the next Loading frame.
    pub(crate) pending_dispatch_failure: Option<LevelLoadFailure>,
    /// Test seams.
    #[cfg(test)]
    pub(crate) test: LoadTestSeams,
}

impl std::fmt::Debug for LevelLoads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelLoads")
            .field("generation", &self.generation)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// One load, from its request to its end.
pub(crate) struct ActiveLevelLoad {
    pub(crate) generation: u64,
    pub(crate) map_path: PathBuf,
    pub(crate) content_root: PathBuf,
    /// The whole load's counter: parse, prep and upload credit it from their
    /// threads, Settling from the main thread. The loading screen reads it.
    pub(crate) progress: Arc<LevelLoadProgress>,
    pub(crate) phase: ActivePhase,
    /// The renderer's uncaptured-error capture is armed for this load.
    pub(crate) error_capture_armed: bool,
}

impl std::fmt::Debug for ActiveLevelLoad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveLevelLoad")
            .field("generation", &self.generation)
            .field("map_path", &self.map_path)
            .field("phase", &self.phase)
            .field("error_capture_armed", &self.error_capture_armed)
            .finish_non_exhaustive()
    }
}

/// Whether the active load's stages run yet.
pub(crate) enum ActivePhase {
    /// Waiting to dispatch; no stage work.
    Queued,
    /// Dispatched: prep on the pool, the set build on the upload thread.
    Staged(Box<LevelLoadStages>),
}

impl std::fmt::Debug for ActivePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivePhase::Queued => f.write_str("Queued"),
            ActivePhase::Staged(stages) => f.debug_tuple("Staged").field(stages).finish(),
        }
    }
}

impl LevelLoads {
    /// Make a queued load for `map_path` the active one, under the next
    /// generation. The caller has already cancelled any active load.
    pub(crate) fn start(
        &mut self,
        map_path: PathBuf,
        content_root: PathBuf,
        progress: Arc<LevelLoadProgress>,
    ) -> u64 {
        debug_assert!(self.active.is_none(), "cancel the active load first");
        self.generation += 1;
        self.active = Some(ActiveLevelLoad {
            generation: self.generation,
            map_path,
            content_root,
            progress,
            phase: ActivePhase::Queued,
            error_capture_armed: false,
        });
        self.generation
    }

    pub(crate) fn active(&self) -> Option<&ActiveLevelLoad> {
        self.active.as_ref()
    }

    pub(crate) fn active_mut(&mut self) -> Option<&mut ActiveLevelLoad> {
        self.active.as_mut()
    }

    /// End the active load's tenure here; the caller retires its stages.
    pub(crate) fn take_active(&mut self) -> Option<ActiveLevelLoad> {
        self.active.take()
    }

    /// The active load's stages, once dispatched.
    #[cfg(test)]
    pub(crate) fn staged(&self) -> Option<&LevelLoadStages> {
        match &self.active.as_ref()?.phase {
            ActivePhase::Staged(stages) => Some(stages),
            ActivePhase::Queued => None,
        }
    }

    /// Whether the active load was dispatched.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_staged(&self) -> bool {
        matches!(
            self.active.as_ref().map(|active| &active.phase),
            Some(ActivePhase::Staged(_))
        )
    }
}

/// A test's edit of every dispatch's prep inputs.
#[cfg(test)]
pub(crate) type EditInputs = Box<dyn Fn(&mut crate::level_load::LevelPrepInputs)>;

/// A test's stand-in for what a Loading frame's transport poll received.
#[cfg(test)]
pub(crate) type TransportReceipt = Box<dyn FnOnce(&mut crate::App)>;

/// What tests reach into the state machine with.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct LoadTestSeams {
    /// Applied to every dispatch's prep inputs: hooks, roots.
    pub(crate) edit_inputs: Option<EditInputs>,
    /// Loading-frame transport polls, counted.
    pub(crate) transport_polls: usize,
    /// Every load dispatched, by generation, with whether a level was
    /// installed at that moment.
    pub(crate) dispatched: Vec<(u64, bool)>,
    /// Runs once on the next Loading frame, right after its transport poll:
    /// stands in for what that poll received.
    pub(crate) on_transport_poll: Option<TransportReceipt>,
    /// What the last bundle's set build created on the upload thread, all
    /// parts and its model parts, read as its stages retire.
    pub(crate) last_upload_creations: Option<(
        postretro_renderer::GpuCreationCounts,
        postretro_renderer::GpuCreationCounts,
    )>,
}
