//! The level-load state machine's App side: starting a load, dispatching its
//! stages, the Loading frame that polls them and commits the bundle on the
//! frame it is ready, and cancellation (supersede, a queued unload, suspend)
//! with every cancelled load's stages handed to the App's one retirement list.
//! Nothing here waits on a stage thread or the GPU.
//! See: context/lib/boot_sequence.md §1 (Loading screen), §4 · context/plans/in-progress/level-load-pipeline/index.md (Cancellation, The commit cannot fail on level data, Settings and scene extent, dispatch to commit) · research.md §9 (P1–P8, P10, P12, P20, P22, P24)

use super::*;
use crate::level_load::{BundleSet, CommitBundle, LevelCommitReport, LevelLoadFailure};
use crate::startup::level_loads::{ActivePhase, LoadOrder};
use crate::startup::loading_screen::LoadingStep;

/// What one Loading frame left to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadingFrame {
    /// Nothing ended: paint the loading screen.
    Paint,
    /// The load committed (Settling now) or failed (Frontend, or the loop's
    /// exit for a CLI boot load).
    Ended,
}

impl App {
    /// Start a load: the loading screen and a queued load under the next
    /// generation, then dispatch its stages at once when nothing blocks it.
    ///
    /// A load still in flight is superseded: its stages are cancelled and
    /// retire, it reports nothing, and the loading screen keeps its tree and
    /// UI clock, shows the new level's name and restarts the bar at 0 on the
    /// new load's own counter, which the old stages never credit.
    pub(in crate::startup) fn begin_level_load(&mut self, load: InFlightLevelLoad) {
        let superseded = self.cancel_active_level_load();
        self.level_timings = StartupTimings::new();
        let progress = superseded
            .then(|| self.supersede_loading_screen(&load.entry))
            .flatten()
            .unwrap_or_else(|| self.begin_loading_screen(&load.entry));
        self.level_loads
            .start(load.map_path.clone(), load.content_root.clone(), progress);
        self.level_load = Some(load);
        self.boot_state = BootState::Loading;
        self.try_dispatch_level_load();
    }

    /// Dispatch the queued load's stages when nothing blocks them: a
    /// full-ready renderer and the session exist, and no older generation's
    /// stages still run (generations never overlap, pin P4; after a suspend
    /// the resumed load waits for the old device's stages, pin P6). A load
    /// never dispatches against a renderer that is not full-ready: it stays
    /// queued, doing no stage work (pin P12), so every production bundle
    /// carries a resident set. The tiers are read here, at dispatch (pin
    /// P21). A load beside a running level dispatches by the same rules.
    /// Never waits.
    pub(in crate::startup) fn try_dispatch_level_load(&mut self) {
        let Some(active) = self.level_loads.active() else {
            return;
        };
        if !matches!(active.phase, ActivePhase::Queued) {
            return;
        }
        if !self.stage_retirements.is_empty() || self.session.is_none() {
            return;
        }
        if !self
            .renderer
            .as_ref()
            .is_some_and(crate::render::Renderer::is_full_ready)
        {
            return;
        }
        let (generation, map_path, content_root, progress) = (
            active.generation,
            active.map_path.clone(),
            active.content_root.clone(),
            std::sync::Arc::clone(&active.progress),
        );
        let loading_screen_order = matches!(active.order, LoadOrder::LoadingScreen);
        let group = match self.load_pool.get_or_build() {
            Ok(pool) => pool.group(),
            Err(error) => {
                // Left queued: the next Loading frame routes the failure.
                self.level_loads.pending_dispatch_failure = Some(LevelLoadFailure::dispatch(
                    format!("the engine load pool could not start: {error}"),
                ));
                return;
            }
        };
        // While this load is uncommitted and no level runs, an uncaptured
        // wgpu error is this load's failure, not wgpu's panic: armed before
        // any of its stages starts, under its generation, so an error held
        // for a cancelled load never reaches a later one. Beside a running
        // level an uncaptured error cannot be told from the running level's
        // own, so the capture stays disarmed and wgpu's default holds; the
        // errors the upload thread's scopes catch still fail the load.
        let capture = loading_screen_order && !self.level_is_installed_state();
        if capture && let Some(renderer) = self.renderer.as_ref() {
            renderer.arm_load_error_capture(generation);
        }
        let Some(stages) = self.begin_level_load_stages(group, map_path, content_root, progress)
        else {
            if capture && let Some(renderer) = self.renderer.as_ref() {
                renderer.disarm_load_error_capture();
            }
            return;
        };
        #[cfg(test)]
        self.level_loads
            .test
            .dispatched
            .push((generation, self.level.is_some()));
        if let Some(active) = self.level_loads.active_mut() {
            active.phase = ActivePhase::Staged(Box::new(stages));
            active.error_capture_armed = capture;
        }
        self.level_timings.record("load_dispatch");
    }

    pub(in crate::startup) fn run_loading_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_dt: f32,
    ) -> bool {
        if self.step_loading_frame(event_loop, frame_dt) == LoadingFrame::Paint {
            // Loading redraws unconditionally; painting never advances state.
            self.paint_loading_frame(event_loop, frame_dt);
            self.request_redraw();
        }
        false
    }

    /// One Loading frame up to its paint: poll the transport (a load may
    /// outlast the netcode timeout), then decide the step. A bundle ready
    /// this frame commits now, after this frame's transport poll (a relevel
    /// it received drains next frame, after the commit, pin P2) and before
    /// this frame's options and extent steps, which a Loading frame never
    /// reaches.
    pub(in crate::startup) fn step_loading_frame(
        &mut self,
        event_loop: &impl LoopExit,
        frame_dt: f32,
    ) -> LoadingFrame {
        let _ = self.poll_world_less_transport(frame_dt);
        #[cfg(test)]
        {
            self.level_loads.test.transport_polls += 1;
            if let Some(received) = self.level_loads.test.on_transport_poll.take() {
                received(self);
            }
        }
        match self.next_loading_step() {
            LoadingStep::Paint => LoadingFrame::Paint,
            LoadingStep::Commit(bundle) => {
                self.commit_ready_level_bundle(*bundle, event_loop);
                LoadingFrame::Ended
            }
            LoadingStep::Fail(failure) => {
                self.finish_level_failure(failure.to_string(), event_loop);
                LoadingFrame::Ended
            }
        }
    }

    /// Decide one Loading frame: dispatch a queued load when it may start,
    /// route an uncaptured wgpu error held for this load, then poll the
    /// stages once. A ready bundle is taken now and its stages handed to
    /// retirement; nothing here blocks.
    pub(in crate::startup) fn next_loading_step(&mut self) -> LoadingStep {
        if let Some(failure) = self.level_loads.pending_dispatch_failure.take() {
            return LoadingStep::Fail(failure);
        }
        self.try_dispatch_level_load();
        let Some(active) = self.level_loads.active_mut() else {
            return LoadingStep::Paint;
        };
        let generation = active.generation;
        let ActivePhase::Staged(stages) = &mut active.phase else {
            return LoadingStep::Paint;
        };
        if let Some(error) = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.take_load_error(generation))
        {
            return LoadingStep::Fail(LevelLoadFailure::uncaptured(error));
        }
        match stages.try_take(self.renderer.as_mut()) {
            None => LoadingStep::Paint,
            Some(Err(failure)) => LoadingStep::Fail(failure),
            Some(Ok(bundle)) => {
                // The bundle is taken: line C's `bundle_ready` closes here,
                // with the stages that ran inside it.
                self.level_timings
                    .record_containing("bundle_ready", bundle.line_c_marks());
                #[cfg(test)]
                let creations = stages
                    .upload_creations()
                    .zip(stages.model_upload_creations());
                #[cfg(test)]
                {
                    self.level_loads.test.last_upload_creations = creations;
                }
                // An uncaptured error recorded since the check above is still
                // this load's failure: retiring disarms the capture and would
                // drop it. The failure route retires the stages; the bundle
                // never commits, so its own set goes with it.
                if let Some(error) = self
                    .renderer
                    .as_ref()
                    .and_then(|renderer| renderer.take_load_error(generation))
                {
                    if let (BundleSet::Resident(id), Some(renderer)) =
                        (&bundle.set, self.renderer.as_mut())
                    {
                        renderer.release_level_set(*id);
                    }
                    return LoadingStep::Fail(LevelLoadFailure::uncaptured(error));
                }
                // The stages are done: retire them now (the bundle owns the
                // set, so nothing is released) and drop the error capture
                // before any commit-frame GPU work.
                self.retire_active_level_load();
                LoadingStep::Commit(Box::new(bundle))
            }
        }
    }

    /// Commit a ready bundle, checked or not; its report, or `None` when a
    /// commit check failed and took the one failure route. Nothing else here
    /// can fail, and a checked bundle (the beside order's) fails nowhere.
    ///
    /// Line C folds the commit frame into one `commit_frame` mark, from the
    /// mark before it (the bundle taken, or the running level's release
    /// beside it) to Settling entered, its stage marks inside it and summing
    /// to it.
    pub(in crate::startup) fn commit_ready_level_bundle(
        &mut self,
        bundle: impl Into<CommitBundle>,
        event_loop: &impl LoopExit,
    ) -> Option<LevelCommitReport> {
        self.level_timings.open_group();
        let committed = self.commit_level_bundle(bundle);
        if committed.is_ok() {
            self.level_timings.record("enter_settling");
        }
        self.level_timings.close_group("commit_frame");
        match committed {
            Ok(report) => {
                let uncounted = report.uncounted_creations();
                if uncounted.total() > 0 {
                    log::warn!(
                        "[Loader] the level commit created GPU objects outside its counted fallbacks: {uncounted:?}"
                    );
                }
                Some(report)
            }
            Err(failure) => {
                self.finish_level_failure(failure.to_string(), event_loop);
                None
            }
        }
    }

    /// End the active load's tenure without reporting anything: cancel its
    /// stages (a cancelled load's errors, the cancel included, reach no
    /// failure path, pin P20), hand them to the retirement list, and disarm
    /// the uncaptured-error capture. Also how a load whose outcome was taken
    /// hands back its stages. A load beside a running level holding a ready
    /// bundle releases the bundle's set; its stages already retire. Returns
    /// whether a load was active.
    pub(in crate::startup) fn cancel_active_level_load(&mut self) -> bool {
        let Some(active) = self.level_loads.take_active() else {
            return false;
        };
        if active.error_capture_armed
            && let Some(renderer) = self.renderer.as_ref()
        {
            renderer.disarm_load_error_capture();
        }
        match active.phase {
            ActivePhase::Queued => {}
            ActivePhase::Staged(stages) => {
                let retirement = stages.cancel(self.renderer.as_mut());
                self.stage_retirements
                    .push_load(active.generation, retirement);
            }
            ActivePhase::Ready(bundle) => {
                // Nothing commits: the bundle's set goes with it, and the
                // model entries it would have offered drop uncommitted.
                if let (BundleSet::Resident(id), Some(renderer)) =
                    (&bundle.set, self.renderer.as_mut())
                {
                    renderer.release_level_set(*id);
                }
            }
        }
        self.level_loads.pending_dispatch_failure = None;
        true
    }

    /// [`Self::cancel_active_level_load`] for a load that ended (committed
    /// or failed): the same hand-off, under the name its callers mean.
    pub(in crate::startup) fn retire_active_level_load(&mut self) {
        self.cancel_active_level_load();
    }

    /// An unload request drained while a load is in flight: cancel the load
    /// and enter Frontend, ending its loading screen (pin P3). A backdrop
    /// arrives only through a load the same return already queued (pin P24).
    pub(in crate::startup) fn abandon_level_load_for_frontend(&mut self) {
        if self.boot_state != BootState::Loading || !self.cancel_active_level_load() {
            return;
        }
        self.level_load = None;
        self.end_loading_screen();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
        }
        self.boot_state = BootState::Frontend;
        self.request_redraw();
    }

    /// Suspend, before the renderer drops: cancel the load in flight. A set
    /// it received is released on the renderer that holds it, every builder
    /// clone drops (so the retired upload thread's channel can close), a
    /// ready bundle drops uncommitted with every model entry it would have
    /// offered (pin P7), and its stages join through the retirement list on
    /// later frames, Booting and Splash included (pins P6, P22).
    pub(crate) fn cancel_level_load_for_suspend(&mut self) {
        self.cancel_active_level_load();
    }
}
