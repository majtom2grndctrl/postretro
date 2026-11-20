// Playback's App hooks: arming on install, the Settling view, each world
// frame's begin and end, aborts on level requests and joins, the settle
// outcome, and the stall timeout's event-loop wait.
// See: context/lib/rendering_pipeline.md §12 · context/lib/boot_sequence.md §1 (Settling), §4

use std::net::SocketAddr;
use std::time::Instant;

use winit::event_loop::{ActiveEventLoop, ControlFlow};

use crate::App;
use crate::app::world_less_frame::HeldLevelView;
use crate::startup::{BootState, LevelRequest, LevelSource};

use super::marks::LifeKey;
use super::route::{self, Identity};
use super::run::{Phase, RowKind};
use super::stall::WaitScreen;

/// How a run ends.
enum Ending {
    Complete,
    /// The record is marked aborted with the reason, and the session exits
    /// non-zero naming it.
    Aborted(String),
    /// The session is already exiting on this error: the record is marked
    /// aborted with its message, and the session's own error stays the exit.
    Failed(String),
}

/// What a Settling frame does with its held view.
pub(crate) enum SettlingView {
    Hold(HeldLevelView),
    /// The installed level refused the route: the run stops, naming why.
    Refused(String),
}

impl App {
    /// Install's camera step on the session's first gameplay level: the
    /// route's bake must be the level's (id-51 hash and walk epoch). A match
    /// arms the run; a mismatch is held for the first Settling frame, which
    /// stops the run. A frontend backdrop is never the route's level, nor
    /// a recording's. A connected client's level is its host's world, which
    /// a route cannot measure: the run aborts instead of arming.
    pub(crate) fn playback_level_installed(&mut self) {
        let backdrop = self.frontend_menu_is_present();
        if !backdrop {
            self.recording_level_installed();
        }
        let path = self.installed_level_path();
        let connected_client = self.is_connected_client();
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        if backdrop || active.run.phase() != Phase::AwaitingInstall {
            return;
        }
        if connected_client {
            active.run.abort();
            self.end_playback_run(Ending::Aborted(format!(
                "level {path} installed as a connected client: a route measures a local level"
            )));
            return;
        }
        let Some(world) = self.level.as_ref() else {
            return;
        };
        let bake = postretro_camera_route::bake_identity(&path, world);
        if active.level_installed(&path, bake) {
            self.playback_arm_timing();
        }
    }

    /// A Settling frame's held view: the route's first pose at its FOV while
    /// the run is armed, else `fallback`. `None` after stopping a run whose
    /// route the level refused.
    pub(crate) fn playback_settling_view(
        &mut self,
        event_loop: &ActiveEventLoop,
        fallback: HeldLevelView,
    ) -> Option<HeldLevelView> {
        match self.playback_settling_view_for(fallback) {
            SettlingView::Hold(view) => Some(view),
            SettlingView::Refused(refusal) => {
                log::error!("[Playback] {refusal}");
                self.exit_result = Err(anyhow::anyhow!(refusal));
                event_loop.exit();
                None
            }
        }
    }

    pub(crate) fn playback_settling_view_for(&mut self, fallback: HeldLevelView) -> SettlingView {
        let aspect = self.camera.aspect();
        let Some(active) = self.playback.active.as_mut() else {
            return SettlingView::Hold(fallback);
        };
        if let Some(refusal) = active.refusal.take() {
            self.playback.active = None;
            self.playback.stall = None;
            return SettlingView::Refused(refusal);
        }
        let held = active
            .run
            .settling_pose()
            .and_then(|pose| active.view(pose, 0.0))
            .map(|view| view.held_view(aspect));
        SettlingView::Hold(held.unwrap_or(fallback))
    }

    /// The reveal edge of a level entry: the route's Settling outcome goes in
    /// the record header.
    pub(crate) fn playback_level_revealed(&mut self, timed_out: bool) {
        if let Some(active) = self.playback.active.as_mut()
            && active.run.phase() == Phase::Settling
        {
            active.record.set_settle(timed_out);
        }
    }

    /// A Running frame, after its tick loop and before its eye and streaming
    /// step: pick the frame's pose, log a pawn life-state change against the
    /// previous frame, and mark the frame's GPU work with the index its row
    /// will take.
    pub(crate) fn playback_begin_world_frame(&mut self, now: Instant) {
        let life_key = self.playback_life_key();
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        if !active.run.is_playing() {
            return;
        }
        active.presented = false;
        active.streamed = false;
        active.frame_interval_us = active.last_frame_start.map(|previous| {
            u64::try_from(now.duration_since(previous).as_micros()).unwrap_or(u64::MAX)
        });
        active.last_frame_start = Some(now);
        let Some(plan) = active.run.begin_world_frame(now) else {
            return;
        };
        if let Some(previous) = active.life.observe(life_key) {
            let name = life_key.and_then(|key| {
                self.session.as_ref().and_then(|session| {
                    let names = &session.scripting.script_ctx.data_registry;
                    names
                        .borrow()
                        .life_state_names
                        .get(usize::from(key.life_state))
                        .cloned()
                })
            });
            // The record names it on the next row written: this frame's, or
            // a later one's when this frame writes none.
            log::info!(
                "[Playback] row {}: local pawn life state {:?} -> {:?} ({})",
                active.record.next_frame(),
                previous.map(|key| key.life_state),
                life_key.map(|key| key.life_state),
                name.as_deref().unwrap_or("unnamed"),
            );
            active.record.push_life_state(life_key, name, previous);
        }
        if plan.kind == RowKind::Route && !active.record.has_rendered() {
            active.record.set_rendered(
                active.file.header.projection.hfov_degrees,
                self.camera.aspect(),
            );
        }
        let frame = active.record.next_frame();
        let needs_context = !active.record.has_context();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_frame_mark(Some(frame));
        }
        if needs_context
            && let Some(context) = self.playback_run_context()
            && let Some(active) = self.playback.active.as_mut()
        {
            active.record.set_context(context);
        }
        self.playback_sample_begin();
    }

    /// The followed local pawn's life-state identity, when one exists.
    fn playback_life_key(&self) -> Option<LifeKey> {
        self.playback.active.as_ref()?;
        let session = self.session.as_ref()?;
        let registry = session.scripting.script_ctx.registry.borrow();
        let pawn = crate::followed_player_pawn(&registry)?;
        let movement = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            .ok()?;
        Some(LifeKey {
            pawn: pawn.to_raw(),
            teleport_generation: movement.teleport.generation,
            life_state: movement.life_state,
            life_state_entry: movement.life_state_entry,
            table_revision: movement.life_state_table_revision,
        })
    }

    /// The frame's end: writes its row, if it has one, and advances. After
    /// the last pose's route frame the record is written, once, and the
    /// session exits 0, or non-zero if the write failed.
    pub(crate) fn playback_end_frame(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(recording) = self.playback.recording.as_mut() {
            recording.end_frame();
        }
        self.playback_finish_frame();
        self.playback_take_exit(event_loop);
    }

    pub(crate) fn playback_finish_frame(&mut self) {
        let fill = self.playback_row_fill();
        let session = self.session.as_ref();
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        let presented = std::mem::take(&mut active.presented);
        let streamed = std::mem::take(&mut active.streamed);
        let planned = active.run.current().is_some();
        let outcome = active.run.finish_frame(presented, streamed);
        if let Some(plan) = outcome.row {
            let marks = active.marks.take_row_marks();
            active.record.push_row(
                plan,
                active.frame_interval_us,
                marks,
                fill.unwrap_or_default(),
            );
            // From the first row on, hold rows included, so a run aborted
            // during the hold still names a seeded cap.
            let cap = super::sample::cap_report(session);
            active.record.set_cap(cap);
            active
                .windows
                .after_row(active.record.rows(), active.args.pacing.key(), cap);
            // The run's first world frame disarms the stall timeout.
            if plan.kind != RowKind::NoDraw {
                self.playback.stall = None;
            }
        }
        if planned {
            active.marks.after_frame(presented);
        }
        self.playback_take_gpu_samples();
        if outcome.complete {
            self.end_playback_run(Ending::Complete);
        }
    }

    /// Why `request` would end a playing run, naming it; `None` when no run
    /// is playing.
    pub(crate) fn playback_request_abort_reason(&self, request: &LevelRequest) -> Option<String> {
        let active = self.playback.active.as_ref()?;
        if !active.run.is_playing() {
            return None;
        }
        Some(match request {
            LevelRequest::Unload => "level unload requested".to_string(),
            LevelRequest::HostLevel(id) => format!("host level `{id}` requested"),
            LevelRequest::Load(source) if self.active_level_source.as_ref() == Some(source) => {
                format!("level reload requested ({})", describe_source(source))
            }
            LevelRequest::Load(source) => {
                format!("level change requested ({})", describe_source(source))
            }
        })
    }

    /// Why a frontend join to `addr` would end the run: any run not yet
    /// ended, one still awaiting its level included. A run launched with no
    /// map waits on the frontend, and a join there would install the host's
    /// level as the route's, whose eye a connected client adjusts.
    pub(crate) fn playback_join_abort_reason(&self, addr: SocketAddr) -> Option<String> {
        let active = self.playback.active.as_ref()?;
        active
            .run
            .is_live()
            .then(|| format!("join to host {addr} requested"))
    }

    /// Aborts a run not yet ended for `reason`: the record is written marked
    /// aborted with the rows so far, and the session exits non-zero. Returns
    /// whether a run aborted; the caller then leaves the request undone.
    pub(crate) fn playback_abort(&mut self, reason: Option<String>) -> bool {
        let Some(reason) = reason else {
            return false;
        };
        let aborted = self
            .playback
            .active
            .as_mut()
            .is_some_and(|active| active.run.abort());
        if aborted {
            self.end_playback_run(Ending::Aborted(reason));
        }
        aborted
    }

    /// `exiting`: a run that had not ended writes its record marked aborted,
    /// with its final partial window before `run_end`. A session already
    /// exiting on an error records that error as the reason and keeps it as
    /// the exit. Otherwise the person closed the window or quit mid-route:
    /// the run is aborted like any other and the session exits non-zero, so
    /// an agent reading only the exit code never takes it for a result.
    pub(crate) fn playback_closed_on_exit(&mut self) {
        let Some(active) = self.playback.active.as_mut() else {
            return;
        };
        active.run.abort();
        let ending = match &self.exit_result {
            Err(err) => Ending::Failed(format!("{err:#}")),
            Ok(()) => Ending::Aborted("window closed or quit before the route ended".to_string()),
        };
        self.end_playback_run(ending);
    }

    /// Writes the record once, marked with how the run ended, and leaves
    /// the exit to the next hook holding the event loop.
    fn end_playback_run(&mut self, ending: Ending) {
        let Some(mut active) = self.playback.active.take() else {
            return;
        };
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_frame_mark(None);
        }
        self.playback_finish_gpu(&mut active);
        // The final partial window, before the record's write and so before
        // `run_end`: no window follows the run's end.
        active.windows.finish(
            active.record.rows(),
            active.args.pacing.key(),
            super::sample::cap_report(self.session.as_ref()),
        );
        let report = active.args.report.clone();
        match &ending {
            Ending::Complete => active.record.set_complete(),
            Ending::Aborted(reason) | Ending::Failed(reason) => {
                active.record.set_aborted(reason.clone())
            }
        }
        let rows = active.record.next_frame();
        let written = active.record.write(&report);
        // The first error stays the exit: a `Failed` ending's own, or one
        // set earlier in the same iteration (a refusal, a Settling error)
        // before the stall or an abort ended the run.
        match (ending, written) {
            (Ending::Complete, Ok(())) => log::info!(
                "[Playback] camera route complete: {rows} frames recorded to {}",
                report.display(),
            ),
            (Ending::Aborted(reason), Ok(())) => {
                log::error!(
                    "[Playback] camera route aborted: {reason}; {rows} frames recorded to {}",
                    report.display(),
                );
                if self.exit_result.is_ok() {
                    self.exit_result = Err(anyhow::anyhow!("camera route aborted: {reason}"));
                }
            }
            (Ending::Failed(reason), Ok(())) => log::error!(
                "[Playback] camera route aborted: {reason}; {rows} frames recorded to {}",
                report.display(),
            ),
            (_, Err(err)) => {
                log::error!("[Playback] {err}");
                if self.exit_result.is_ok() {
                    self.exit_result = Err(anyhow::anyhow!(err));
                }
            }
        }
        self.playback.stall = None;
        self.playback.exit_pending = true;
    }

    /// Exits the event loop once a run has ended. Returns whether it did,
    /// so a frame stops before any more work.
    pub(crate) fn playback_take_exit(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.playback.exit_pending {
            return false;
        }
        self.release_cursor_for_exit();
        event_loop.exit();
        true
    }

    /// `about_to_wait`: exits an ended run, and fails a stalled one. While
    /// the stall timeout is armed the loop wakes at its deadline, because a
    /// hidden window gets no redraw to check it from; once disarmed the
    /// default wait returns. Returns whether the session is exiting.
    pub(crate) fn playback_about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> bool {
        self.playback_check_stall(Instant::now());
        if self.playback_take_exit(event_loop) {
            return true;
        }
        match self.playback.stall {
            Some(stall) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(stall.deadline()));
                self.playback.waiting_on_stall = true;
            }
            None if self.playback.waiting_on_stall => {
                event_loop.set_control_flow(ControlFlow::Wait);
                self.playback.waiting_on_stall = false;
            }
            None => {}
        }
        false
    }

    /// Fails the run when the stall timeout has passed with no world frame.
    pub(crate) fn playback_check_stall(&mut self, now: Instant) {
        let Some(stall) = self.playback.stall else {
            return;
        };
        let Some(reason) = stall.expired(now, self.playback_wait_screen()) else {
            return;
        };
        self.playback.stall = None;
        log::error!("[Playback] {reason}");
        if let Some(active) = self.playback.active.as_mut() {
            // Any phase: a run that never armed still writes its record.
            active.run.abort();
            self.end_playback_run(Ending::Aborted(reason));
        } else {
            self.exit_result = Err(anyhow::anyhow!(reason));
            self.playback.exit_pending = true;
        }
    }

    fn playback_wait_screen(&self) -> WaitScreen {
        match self.boot_state {
            BootState::Booting | BootState::Splash => WaitScreen::Splash,
            BootState::Loading => WaitScreen::Loading,
            BootState::Settling => WaitScreen::Settling,
            BootState::FirstLaunchHold => WaitScreen::FirstLaunchPanel,
            BootState::Frontend => WaitScreen::FrontendMenu,
            BootState::Running if self.frontend_menu_is_present() => WaitScreen::FrontendMenu,
            BootState::Running => WaitScreen::Level,
        }
    }
}

impl super::ActivePlayback {
    /// A gameplay level installed with `bake` while the run awaits its
    /// level. A match arms the run and drops any refusal an earlier level
    /// left, so a Settling frame never stops a matching level naming another;
    /// a mismatch holds its refusal for the next Settling frame. Returns
    /// whether the run armed.
    fn level_installed(&mut self, path: &str, bake: String) -> bool {
        match route::match_level(&self.file, &bake) {
            Ok(identity) => {
                if identity == Identity::Unproven {
                    log::warn!(
                        "[Playback] level {path} has no id-51 hash; camera route {} accepted unproven",
                        self.args.poses.display(),
                    );
                }
                self.refusal = None;
                self.record.set_level(bake, identity);
                self.run.arm();
                true
            }
            Err(reason) => {
                self.refusal = Some(format!(
                    "camera route {} does not match level {path}: {reason}",
                    self.args.poses.display(),
                ));
                false
            }
        }
    }
}

fn describe_source(source: &LevelSource) -> String {
    match source {
        LevelSource::Catalog(id) => format!("catalog `{id}`"),
        LevelSource::Path(path) => path.display().to_string(),
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
pub(crate) mod tests;
