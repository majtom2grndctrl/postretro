// Recording's App hooks: the gameplay level's bake at install, each Running
// frame's pre-view-feel view, and the one write when the first gameplay
// level ends or the session exits.
// See: context/lib/rendering_pipeline.md §12 · context/lib/boot_sequence.md §1 (Settling), §4, §5

use std::time::Instant;

use postretro_camera_route::Projection;

use crate::App;
use crate::frame_eye::FrameEyeInputs;

use super::recording::{self, RecordState, Sample};

impl App {
    /// The installed level's path as loaded: the in-flight load's, else the
    /// launch map's.
    pub(crate) fn installed_level_path(&self) -> String {
        self.level_load
            .as_ref()
            .map(|load| load.map_path.display().to_string())
            .or_else(|| {
                self.map_path
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_default()
    }

    /// Install's camera step on a gameplay level (never a frontend backdrop):
    /// while nothing has recorded, its bake identity names the recording.
    pub(crate) fn recording_level_installed(&mut self) {
        let waiting = self
            .playback
            .recording
            .as_ref()
            .is_some_and(|recording| recording.wants_level());
        if !waiting {
            return;
        }
        let Some(world) = self.level.as_ref() else {
            return;
        };
        let bake = postretro_camera_route::bake_identity(&self.installed_level_path(), world);
        if let Some(recording) = self.playback.recording.as_mut() {
            recording.note_level(bake);
        }
    }

    /// A Running frame's eye, before view feel: the presented eye, render
    /// yaw and pitch `assemble_frame_eye` takes, stamped with the instant
    /// the frame's redraw began, at its base FOV and the window's aspect. A
    /// frontend backdrop's frames offer nothing; the frame's present decides
    /// whether the view records.
    pub(crate) fn playback_record_eye(&mut self, now: Instant, inputs: &FrameEyeInputs<'_>) {
        if self.playback.recording.is_none() {
            return;
        }
        let gameplay = !self.backdrop_installed && !self.frontend_menu_is_present();
        let Some(recording) = self.playback.recording.as_mut() else {
            return;
        };
        recording.offer(gameplay.then_some((
            Sample {
                at: now,
                eye: inputs.presented_eye,
                yaw: inputs.render_yaw,
                pitch: inputs.pitch,
            },
            Projection {
                hfov_degrees: inputs.base_hfov.to_degrees(),
                aspect: inputs.aspect,
            },
        )));
    }

    /// A level request or join drained and is about to act (pin P13): a
    /// recording of the first gameplay level is written now, and nothing
    /// after it records. Before any world frame it changes nothing: the next
    /// gameplay level is then the first.
    pub(crate) fn recording_level_ends(&mut self) {
        let recording_level = self
            .playback
            .recording
            .as_ref()
            .is_some_and(|recording| recording.state() == RecordState::Recording);
        if recording_level {
            self.finish_recording("the level ended");
        }
    }

    /// `exiting`, on every exit route (pin P14): writes a recording not yet
    /// written, or warns once that none had a world frame; then writes an
    /// unfinished playback run's record, marked aborted.
    pub(crate) fn playback_session_exits(&mut self) {
        if let Some(recording) = self.playback.recording.as_ref() {
            match recording.state() {
                RecordState::Recording => self.finish_recording("the session exited"),
                RecordState::Waiting => {
                    log::warn!(
                        "[Playback] the session ended before its first world frame; no camera route recorded to {}",
                        recording.path().display(),
                    );
                    if let Some(recording) = self.playback.recording.as_mut() {
                        recording.finish();
                    }
                }
                RecordState::Ended => {}
            }
        }
        self.playback_closed_on_exit();
    }

    /// Writes the recording, once.
    fn finish_recording(&mut self, why: &str) {
        let Some(recording) = self.playback.recording.as_mut() else {
            return;
        };
        let frames = recording.samples().len();
        let path = recording.path().to_path_buf();
        let Some(file) = recording.finish() else {
            return;
        };
        match recording::write(&path, &file) {
            Ok(()) => log::info!(
                "[Playback] camera route recorded ({why}): {frames} world frames resampled to {} poses at {} Hz, written to {}",
                file.poses.len(),
                file.header.cadence_hz,
                path.display(),
            ),
            Err(err) => {
                log::error!("[Playback] {err}");
                self.exit_result = Err(anyhow::anyhow!(err));
            }
        }
    }
}
