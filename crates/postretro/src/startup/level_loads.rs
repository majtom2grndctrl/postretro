// The level load in flight: its generation, its commit order (loading screen or beside a running level), whether its stages run yet, and the whole load's progress counter.
// See: context/lib/boot_sequence.md §1, §4 · context/plans/done/level-load-pipeline/index.md (One pipeline, two commit orders, Cancellation, Progress covers the whole load)

use std::path::PathBuf;
use std::sync::Arc;

use postretro_level_loader::LevelLoadProgress;

use crate::level_load::{LevelBundle, LevelLoadFailure, LevelLoadStages};
use crate::startup::InFlightLevelLoad;

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
///
/// A load's [`LoadOrder`] says which commit order it runs in. A load beside
/// a running level ([`LoadOrder::Beside`]) shares every step above, but its
/// Running frames poll it, a ready bundle waits in [`ActivePhase::Ready`]
/// until its caller commits it, and a failure leaves the running level
/// Running.
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
    pub(crate) order: LoadOrder,
    pub(crate) phase: ActivePhase,
    /// The renderer's uncaptured-error capture is armed for this load.
    pub(crate) error_capture_armed: bool,
}

impl std::fmt::Debug for ActiveLevelLoad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveLevelLoad")
            .field("generation", &self.generation)
            .field("map_path", &self.map_path)
            .field("order", &self.order)
            .field("phase", &self.phase)
            .field("error_capture_armed", &self.error_capture_armed)
            .finish_non_exhaustive()
    }
}

impl ActiveLevelLoad {
    /// The load runs beside a running level.
    pub(crate) fn is_beside(&self) -> bool {
        matches!(self.order, LoadOrder::Beside(_))
    }
}

/// Which of the pipeline's two commit orders a load runs in.
#[derive(Debug)]
pub(crate) enum LoadOrder {
    /// The loading-screen order: the installed level unloaded first, the
    /// loading screen up, the bundle committed on the Loading frame it is
    /// ready.
    LoadingScreen,
    /// Beside a running level (a test and dev surface; hub U3 owns the
    /// swap): no unload, no loading screen, no `loading.progress` credit,
    /// and the uncaptured-error capture stays disarmed. The request's
    /// metadata waits here until the commit installs it.
    // A test-only seam until hub U3 adds a production verb that starts one.
    #[cfg_attr(not(test), allow(dead_code))]
    Beside(Box<InFlightLevelLoad>),
}

/// Whether the active load's stages run yet.
pub(crate) enum ActivePhase {
    /// Waiting to dispatch; no stage work.
    Queued,
    /// Dispatched: prep on the pool, the set build on the upload thread.
    Staged(Box<LevelLoadStages>),
    /// A load beside a running level whose bundle is ready, its stages
    /// handed to retirement: held, its set resident and unpublished, until
    /// its caller commits it. A cancel releases the set.
    Ready(Box<LevelBundle>),
}

impl std::fmt::Debug for ActivePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivePhase::Queued => f.write_str("Queued"),
            ActivePhase::Staged(stages) => f.debug_tuple("Staged").field(stages).finish(),
            ActivePhase::Ready(bundle) => f.debug_tuple("Ready").field(&bundle.set).finish(),
        }
    }
}

impl LevelLoads {
    /// Make a queued loading-screen load for `map_path` the active one,
    /// under the next generation. The caller has already cancelled any
    /// active load.
    pub(crate) fn start(
        &mut self,
        map_path: PathBuf,
        content_root: PathBuf,
        progress: Arc<LevelLoadProgress>,
    ) -> u64 {
        self.start_in(LoadOrder::LoadingScreen, map_path, content_root, progress)
    }

    /// Make a queued load of `load` beside the running level the active
    /// one, under the next generation. `progress` is the load's own counter,
    /// which its stages credit and nothing reads into `loading.progress`.
    /// The caller has already cancelled any active load.
    // A test-only seam until hub U3 adds a production verb that calls it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn start_beside(
        &mut self,
        load: InFlightLevelLoad,
        progress: Arc<LevelLoadProgress>,
    ) -> u64 {
        let (map_path, content_root) = (load.map_path.clone(), load.content_root.clone());
        self.start_in(
            LoadOrder::Beside(Box::new(load)),
            map_path,
            content_root,
            progress,
        )
    }

    fn start_in(
        &mut self,
        order: LoadOrder,
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
            order,
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
            ActivePhase::Queued | ActivePhase::Ready(_) => None,
        }
    }

    /// Whether the active load was dispatched.
    #[cfg(test)]
    pub(crate) fn is_staged(&self) -> bool {
        matches!(
            self.active.as_ref().map(|active| &active.phase),
            Some(ActivePhase::Staged(_))
        )
    }

    /// The active load runs beside a running level.
    pub(crate) fn beside_active(&self) -> bool {
        self.active.as_ref().is_some_and(ActiveLevelLoad::is_beside)
    }

    /// The bundle a load beside a running level holds, once ready.
    // A test-only seam until hub U3 adds a production verb: only the beside
    // commit and tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn held_bundle(&self) -> Option<&LevelBundle> {
        match &self.active.as_ref()?.phase {
            ActivePhase::Ready(bundle) => Some(bundle),
            ActivePhase::Queued | ActivePhase::Staged(_) => None,
        }
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
