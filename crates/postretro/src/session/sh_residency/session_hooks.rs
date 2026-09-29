//! `Session` entry points for SH streaming: controller replacement, worker
//! retirement, and outcome application. See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use anyhow::{Result, bail};
use postretro_level_loader::{
    CellVisibility, ShDrainBatch, ShDrainOutcome, ShStreamManifest, ShStreamingMode,
    requested_streaming_mode,
};
use postretro_renderer::Renderer;
use postretro_visibility::VisibleCells;

use super::super::sh_async_workers::ShWorkerRetirement;
use super::{ShStreamingSession, require_loaded_streaming_mode};

impl super::super::Session {
    /// Releases the retained manifest/controller as soon as a legacy or
    /// world-free frame becomes active. A following streamed level always gets
    /// a fresh nonzero residency generation, even when its content bytes match
    /// the prior map.
    pub(crate) fn clear_sh_streaming(&mut self) {
        self.poll_sh_worker_retirement();
        if let Some(mut streaming) = self.sh_streaming.take()
            && let Some(retirement) = streaming.begin_worker_retirement()
        {
            debug_assert!(
                self.sh_worker_retirement.is_none(),
                "a replacement worker pool cannot overlap retirement"
            );
            self.sh_worker_retirement = Some(retirement);
        }
    }

    fn poll_sh_worker_retirement(&mut self) {
        let finished = self
            .sh_worker_retirement
            .as_mut()
            .is_some_and(ShWorkerRetirement::try_finish);
        if finished {
            self.sh_worker_retirement = None;
        }
    }

    /// Creates/replaces the session controller for a streamed map, updates it
    /// from the exact visible cells and camera cell for this frame, and returns
    /// its one bounded pre-compose batch. `cell_visibility` is read only when
    /// a controller is created for `manifest`, and must come from the same
    /// level. Legacy maps bypass the environment gate completely.
    pub(crate) fn prepare_sh_streaming_drain(
        &mut self,
        manifest: Option<&Arc<ShStreamManifest>>,
        cell_visibility: Option<&CellVisibility>,
        renderer: &Renderer,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        self.poll_sh_worker_retirement();
        let Some(manifest) = manifest else {
            self.clear_sh_streaming();
            return Ok(ShDrainBatch::default());
        };

        let mode = requested_streaming_mode()?;
        require_loaded_streaming_mode(mode)?;

        let needs_replacement = self
            .sh_streaming
            .as_ref()
            .is_none_or(|streaming| !streaming.is_for_manifest(manifest) || streaming.mode != mode);
        if needs_replacement {
            // Cancel the old generation now. Its handles retire off the frame
            // path; the replacement pool starts only after they have joined.
            self.clear_sh_streaming();
            self.sh_streaming = Some(ShStreamingSession::from_renderer_with_mode(
                manifest.clone(),
                cell_visibility,
                renderer,
                mode,
            )?);
        }

        let streaming = self
            .sh_streaming
            .as_mut()
            .expect("streaming controller initialized above");
        if mode == ShStreamingMode::Async && self.sh_worker_retirement.is_none() {
            streaming.start_async_workers()?;
        }
        match mode {
            ShStreamingMode::SyncProof => {
                streaming.prepare_sync_proof_batch(visible_cells, camera_cell, monotonic_seconds)
            }
            ShStreamingMode::Async => {
                streaming.prepare_async_batch(visible_cells, camera_cell, monotonic_seconds)
            }
            ShStreamingMode::Off => unreachable!("mode was checked above"),
        }
    }

    /// Applies an outcome before the app propagates a later renderer frame
    /// error. Legacy frames pass a default empty outcome safely.
    pub(crate) fn apply_sh_streaming_outcome(
        &mut self,
        outcome: ShDrainOutcome,
        renderer: &Renderer,
    ) -> Result<()> {
        let Some(streaming) = self.sh_streaming.as_mut() else {
            if outcome.accepted.is_empty()
                && outcome.dropped.is_empty()
                && outcome.deferred.is_empty()
                && outcome.evicted.is_empty()
            {
                return Ok(());
            }
            bail!(
                "[SH streaming] renderer returned a non-empty drain outcome without a session controller"
            );
        };
        streaming.apply_outcome(outcome, renderer)
    }

    /// Marks a windowed frame as having submitted its compose work. `false`
    /// deliberately leaves accepted installs uncomposed after an occluded or
    /// failed scene frame.
    pub(crate) fn mark_sh_streaming_compose_submitted(&mut self, submitted: bool) {
        if submitted && let Some(streaming) = self.sh_streaming.as_mut() {
            streaming.mark_compose_submitted();
        }
    }
}
