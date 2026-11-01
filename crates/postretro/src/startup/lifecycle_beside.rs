//! Loads beside a running level: the pipeline's second commit order, a
//! test-only seam until a production verb drives it (hub U3 owns that verb,
//! the seamless swap and its UX). The load
//! shares the loading-screen order's machinery (its generation in
//! `LevelLoads`, its dispatch rules, its stages and their retirement), but
//! nothing unloads, no loading screen starts, and nothing it credits reaches
//! `loading.progress`. Every Running frame polls it without blocking; a
//! ready bundle is held until its caller releases the running level and
//! commits it. Nothing here waits on a stage thread or the GPU.
//! See: context/lib/boot_sequence.md §4 · context/plans/in-progress/level-load-pipeline/index.md (One pipeline, two commit orders, Cancellation, The commit cannot fail on level data, Progress covers the whole load) · research.md §9 (P16, P26–P28, P30, P31)

use std::sync::Arc;

use postretro_level_loader::LevelLoadProgress;

use super::*;
use crate::level_load::{LevelBundle, LevelCommitReport};
use crate::startup::level_loads::{ActivePhase, LoadOrder};

/// What one Running frame's poll found of a load beside the running level.
// The frame loop ignores it; only tests read it, a test-only seam until hub
// U3 adds a production verb that acts on a held bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BesideLoadStep {
    /// No load runs beside the level.
    Idle,
    /// Queued or staged: nothing ended.
    Running,
    /// Its bundle is ready and held for its caller to commit.
    Held,
    /// It ended without a bundle this frame: failed (reported once) or
    /// cancelled because no level runs. The running level is untouched.
    Ended,
}

impl App {
    /// Start a load of `map_path` beside the Running level, under the next
    /// generation: no unload, no loading screen, `boot_state` stays
    /// `Running`. Its stages dispatch at once when nothing blocks them (a
    /// full-ready renderer, the session, an empty retirement list), else on
    /// a later Running frame, reading the tiers then (pins P21, P27). The
    /// load credits its own counter, which nothing reads into
    /// `loading.progress`. A load beside the level already in flight is
    /// superseded: cancelled, its stages retiring, reporting nothing.
    /// Returns false, starting nothing, unless a level is Running.
    // A test-only seam until hub U3 adds a production verb that starts one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn begin_level_load_beside(
        &mut self,
        map_path: PathBuf,
        content_root: PathBuf,
    ) -> bool {
        if self.boot_state != BootState::Running {
            log::warn!(
                "[Loader] a load beside a running level needs a Running level; ignoring {}",
                map_path.display()
            );
            return false;
        }
        // Only a load beside the level can be active while it runs.
        self.cancel_active_level_load();
        let Some(mut load) = self.resolve_level_source(LevelSource::Path(map_path)) else {
            return false;
        };
        load.content_root = content_root;
        // The running level's line C may still wait for its reveal frame's
        // present (`pending_level_log`): log it now, with the marks it has,
        // rather than let the reset below hand it this load's marks. Its
        // `first_level_frame` then closes here instead of at that present.
        if std::mem::take(&mut self.pending_level_log) {
            self.level_timings.log_level_line();
        }
        self.level_timings = StartupTimings::new();
        self.level_loads
            .start_beside(load, Arc::new(LevelLoadProgress::new()));
        self.try_dispatch_level_load();
        true
    }

    /// Every Running frame, before its input and game logic: poll a load
    /// beside the running level once, never waiting. A queued load
    /// dispatches when nothing blocks it; a staged one is polled without
    /// blocking (the retirement list is polled at the top of every frame).
    /// A ready bundle is taken, its stages handed to retirement, and held,
    /// not committed. A failure takes the one failure route, whose beside
    /// half leaves the running level Running. The running level draws and
    /// simulates on as if nothing ran: nothing the stages created is bound
    /// before a commit. A beside load outliving its Running level (no
    /// production path leaves one) is cancelled.
    pub(crate) fn poll_beside_level_load(&mut self, event_loop: &impl LoopExit) -> BesideLoadStep {
        if !self.level_loads.beside_active() {
            return BesideLoadStep::Idle;
        }
        if self.boot_state != BootState::Running {
            self.cancel_active_level_load();
            return BesideLoadStep::Ended;
        }
        if let Some(failure) = self.level_loads.pending_dispatch_failure.take() {
            self.finish_level_failure(failure.to_string(), event_loop);
            return BesideLoadStep::Ended;
        }
        self.try_dispatch_level_load();
        let Some(active) = self.level_loads.active_mut() else {
            return BesideLoadStep::Idle;
        };
        let stages = match &mut active.phase {
            ActivePhase::Queued => return BesideLoadStep::Running,
            ActivePhase::Ready(_) => return BesideLoadStep::Held,
            ActivePhase::Staged(stages) => stages,
        };
        match stages.try_take(self.renderer.as_mut()) {
            None => BesideLoadStep::Running,
            Some(Err(failure)) => {
                self.finish_level_failure(failure.to_string(), event_loop);
                BesideLoadStep::Ended
            }
            Some(Ok(bundle)) => {
                self.hold_beside_bundle(bundle);
                BesideLoadStep::Held
            }
        }
    }

    /// The beside load's bundle is ready: its stages are done, so hand them
    /// to retirement now (the bundle owns the set, so nothing is released)
    /// and hold the bundle in their place.
    fn hold_beside_bundle(&mut self, bundle: LevelBundle) {
        let renderer = self.renderer.as_mut();
        let Some(active) = self.level_loads.active_mut() else {
            return;
        };
        let generation = active.generation;
        // The bundle is taken: line C's `bundle_ready` closes here, with
        // the stages that ran inside it.
        self.level_timings
            .record_containing("bundle_ready", bundle.line_c_marks());
        let ActivePhase::Staged(stages) =
            std::mem::replace(&mut active.phase, ActivePhase::Ready(Box::new(bundle)))
        else {
            unreachable!("only a staged load yields a bundle");
        };
        #[cfg(test)]
        {
            self.level_loads.test.last_upload_creations = stages
                .upload_creations()
                .zip(stages.model_upload_creations());
        }
        let retirement = stages.cancel(renderer);
        self.stage_retirements.push_load(generation, retirement);
    }

    /// Commit the bundle a load beside the running level holds. First the
    /// commit's checks (`check_level_bundle`), while the running level still
    /// runs: they read only the bundle's unpublished set and streaming data,
    /// touch no session state and start no thread. A failing check takes the
    /// failure route's beside half: the bundle's set released, one report,
    /// the running level left Running. Then release the running level
    /// (`unload_level`, which drops only its own set and frees no cache
    /// entry) and commit the checked bundle through `commit_level_bundle`
    /// in the same frame (pin P30), which runs no check twice and cannot
    /// fail. The release frees nothing the bundle holds (its set, fog
    /// target and extent-bound groups stay resident, its cached models
    /// referenced) and cancels nothing; the commit rebuilds the set's
    /// extent-bound groups once if the scene extent moved since its build
    /// (pin P16), loads its predicted models with no miss and no re-upload,
    /// and frees the entries only the released level referenced. The commit
    /// enters Settling, as every commit does (hub U3 owns the visual
    /// hand-off); with no loading screen up, Settling credits no
    /// `loading.progress`. `None` when no beside load holds a ready bundle,
    /// which changes nothing, or when its checks fail: the held load is then
    /// consumed, its line C marks through `commit_checks` stay recorded, and
    /// its set is released. The commit itself cannot return `None`, for it
    /// gets a bundle already checked.
    // A test-only seam until hub U3 adds a production verb that commits one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn commit_ready_beside_bundle(
        &mut self,
        event_loop: &impl LoopExit,
    ) -> Option<LevelCommitReport> {
        self.level_loads.held_bundle()?;
        let active = self.level_loads.take_active()?;
        let (ActivePhase::Ready(bundle), LoadOrder::Beside(load)) = (active.phase, active.order)
        else {
            unreachable!("a held bundle belongs to a load beside the running level");
        };
        // Line C: how long the ready bundle waited for its caller, the
        // commit's checks, then the running level's release; the commit
        // frame follows.
        self.level_timings.record("bundle_held");
        let checked = match self.check_level_bundle(*bundle) {
            Ok(checked) => checked,
            Err(failure) => {
                // The load is no longer active, and its stages retired when
                // the bundle was held: this is its one report.
                self.finish_beside_level_failure(&failure.to_string());
                return None;
            }
        };
        self.level_timings.record("commit_checks");
        if self.level_is_installed_state() {
            self.unload_level();
        }
        self.level_timings.record("running_release");
        self.level_load = Some(*load);
        self.commit_ready_level_bundle(checked, event_loop)
    }

    /// Cancel a load beside the running level, if one is active: its stages
    /// retire through the list every frame polls, a held bundle's set is
    /// released, nothing is reported, and the running level is untouched
    /// (pin P28). Returns whether one was cancelled.
    pub(in crate::startup) fn cancel_beside_level_load(&mut self) -> bool {
        self.level_loads.beside_active() && self.cancel_active_level_load()
    }

    /// The failure route's beside half: the load's one report, its stages
    /// retiring, and nothing else. No state change toward Frontend, no exit:
    /// the running level stays Running with its cache entries (pin P31).
    /// The beside commit reaches it directly when its checks refuse the held
    /// bundle: the load is no longer active then, and its stages already
    /// retired when the bundle was held.
    pub(in crate::startup) fn finish_beside_level_failure(&mut self, reason: &str) {
        self.retire_active_level_load();
        log::error!(
            "[Loader] {reason}; the load beside the running level failed and it keeps running"
        );
    }
}
