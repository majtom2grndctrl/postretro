//! Session-owned SH streaming lifecycle glue.
//!
//! The renderer sees only loader-owned drain batches and returns their outcome.
//! This module keeps target selection, synchronous proof reads, and permit
//! accounting on the application side. See:
//! `context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md`.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use postretro_level_loader::{
    ShDrainBatch, ShDrainOutcome, ShStreamManifest, ShStreamingMode, requested_streaming_mode,
};
use postretro_renderer::{Renderer, ShResidencySnapshot};
use postretro_visibility::VisibleCells;

use crate::sh_streaming::budget::{FixedGpuCharges, ShGpuBudgetInputs, StreamedPoolMinima};
use crate::sh_streaming::controller::{ShResidencyController, SyncReadResult};

/// The named development-mode failure emitted for a valid streamed map before
/// Task 11 supplies bounded asynchronous reads.
const ASYNC_STREAMING_NOT_IMPLEMENTED: &str = "[SH streaming] bounded async mode is not yet implemented at this checkpoint; \
     set POSTRETRO_SH_STREAMING=sync-proof or POSTRETRO_SH_STREAMING=off";

/// Controller state whose lifetime belongs to one loaded session, never to the
/// renderer. A distinct loaded manifest replaces this object before any new
/// frame can issue a batch for the next map.
#[derive(Debug)]
pub(crate) struct ShStreamingSession {
    /// Retained separately for session identity. Two distinct level loads can
    /// have identical bytes/content tags yet must receive distinct nonzero
    /// controller generations.
    manifest: Arc<ShStreamManifest>,
    controller: ShResidencyController,
    /// Set only after a frame actually submitted all compose work. Its next
    /// pre-compose batch may promote the corresponding accepted clusters.
    prior_compose_submitted: bool,
}

impl ShStreamingSession {
    /// Builds a controller from the renderer's actual allocation snapshot.
    pub(crate) fn from_renderer(
        manifest: Arc<ShStreamManifest>,
        renderer: &Renderer,
    ) -> Result<Self> {
        let snapshot = renderer.sh_residency_snapshot().with_context(
            || "[SH streaming] renderer has no residency snapshot for a streamed level",
        )?;
        Self::from_snapshot(manifest, snapshot)
    }

    /// Capture-specific spelling of [`Self::from_renderer`]. It deliberately
    /// has no mode selection; the capture caller applies the shared Task 10
    /// gate before it creates this proof controller.
    pub(crate) fn for_capture(
        manifest: Arc<ShStreamManifest>,
        renderer: &Renderer,
    ) -> Result<Self> {
        Self::from_renderer(manifest, renderer)
    }

    fn from_snapshot(
        manifest: Arc<ShStreamManifest>,
        snapshot: ShResidencySnapshot,
    ) -> Result<Self> {
        let controller = ShResidencyController::new(manifest.clone(), budget_inputs(snapshot))?;
        Ok(Self {
            manifest,
            controller,
            prior_compose_submitted: false,
        })
    }

    fn is_for_manifest(&self, manifest: &Arc<ShStreamManifest>) -> bool {
        Arc::ptr_eq(&self.manifest, manifest)
    }

    /// Updates the controller from one real visibility result. Capture uses
    /// the same method with its deterministic fixed cell set.
    pub(crate) fn update_targets(
        &mut self,
        visible_cells: &VisibleCells,
        monotonic_seconds: f64,
    ) -> Result<()> {
        self.controller
            .update_targets(visible_cells, monotonic_seconds)
            .map_err(Into::into)
    }

    /// Reads one target through the proof-only synchronous path.
    pub(crate) fn read_one_sync(&mut self) -> Result<SyncReadResult> {
        self.controller
            .read_one_sync_at_target_time()
            .map_err(Into::into)
    }

    /// Prepares the bounded loader-to-renderer handoff after target/read work.
    /// A prior successful compose submission is promoted at this next drain;
    /// a failed or skipped frame leaves its installs uncomposed.
    pub(crate) fn prepare_batch(&mut self) -> Result<ShDrainBatch> {
        self.promote_composed_clusters();
        self.controller.take_drain_batch().map_err(Into::into)
    }

    /// Applies the renderer's ownership result before the caller inspects or
    /// propagates a later scene/surface failure. The current snapshot updates
    /// physical charges only after the renderer has accepted/deferred/dropped
    /// this batch.
    pub(crate) fn apply_outcome(
        &mut self,
        outcome: ShDrainOutcome,
        renderer: &Renderer,
    ) -> Result<()> {
        self.controller.apply_drain_outcome(outcome)?;
        let snapshot = renderer.sh_residency_snapshot().with_context(
            || "[SH streaming] renderer lost its residency snapshot before outcome accounting",
        )?;
        self.controller
            .update_gpu_charges(fixed_gpu_charges(snapshot))?;
        Ok(())
    }

    /// Records that the preceding renderer call submitted its compose work.
    /// The controller will not make newly installed clusters sampleable until a
    /// later [`Self::prepare_batch`] consumes this latch.
    pub(crate) fn mark_compose_submitted(&mut self) {
        self.prior_compose_submitted = true;
    }

    /// Frame N can only publish a cluster for sampling at Frame N+1's
    /// pre-compose seam, and only after the caller marked Frame N as submitted.
    pub(crate) fn promote_composed_clusters(&mut self) {
        if consume_prior_compose_submission(&mut self.prior_compose_submitted) {
            self.controller.promote_composed_clusters();
        }
    }

    /// Capture uses this to continue its deterministic preload/render loop
    /// until the complete current visible/owner closure is sampleable.
    #[cfg_attr(not(feature = "capture"), allow(dead_code))]
    pub(crate) fn all_targets_sampleable(&self) -> bool {
        self.controller.all_targets_sampleable()
    }

    /// Synchronous proof policy: fill the four controller permits from the
    /// current target set, then submit at most the controller's bounded drain
    /// batch. The remaining ready items retain their permits for a later frame.
    fn prepare_sync_proof_batch(
        &mut self,
        visible_cells: &VisibleCells,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        self.update_targets(visible_cells, monotonic_seconds)?;
        while matches!(self.read_one_sync()?, SyncReadResult::Prepared(_)) {}
        self.prepare_batch()
    }
}

impl super::Session {
    /// Releases the retained manifest/controller as soon as a legacy or
    /// world-free frame becomes active. A following streamed level always gets
    /// a fresh nonzero residency generation, even when its content bytes match
    /// the prior map.
    pub(crate) fn clear_sh_streaming(&mut self) {
        self.sh_streaming = None;
    }

    /// Creates/replaces the session controller for a streamed map, updates it
    /// from the exact visible cells for this frame, and returns its one bounded
    /// pre-compose batch. Legacy maps bypass the environment gate completely.
    pub(crate) fn prepare_sh_streaming_drain(
        &mut self,
        manifest: Option<&Arc<ShStreamManifest>>,
        renderer: &Renderer,
        visible_cells: &VisibleCells,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        let Some(manifest) = manifest else {
            self.clear_sh_streaming();
            return Ok(ShDrainBatch::default());
        };

        require_sync_proof_mode(requested_streaming_mode()?)?;

        let needs_replacement = self
            .sh_streaming
            .as_ref()
            .is_none_or(|streaming| !streaming.is_for_manifest(manifest));
        if needs_replacement {
            self.sh_streaming = Some(ShStreamingSession::from_renderer(
                manifest.clone(),
                renderer,
            )?);
        }

        self.sh_streaming
            .as_mut()
            .expect("streaming controller was initialized above")
            .prepare_sync_proof_batch(visible_cells, monotonic_seconds)
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

/// Applies the temporary Task 10 runtime gate after the loader has already
/// validated a streamed manifest. Windowed play and static capture share this
/// exact decision; legacy/off never has a manifest to reach it.
pub(crate) fn require_sync_proof_mode(mode: ShStreamingMode) -> Result<()> {
    match mode {
        ShStreamingMode::SyncProof => Ok(()),
        ShStreamingMode::Async => bail!(ASYNC_STREAMING_NOT_IMPLEMENTED),
        // The loader yields a legacy `ShStorage` for `off`, so reaching this
        // branch means the environment changed after the map load.
        ShStreamingMode::Off => bail!(
            "[SH streaming] mode changed to off after this streamed map was loaded; reload the map"
        ),
    }
}

/// Consumes the renderer-submission proof at the following frame boundary.
/// Keeping the `take` explicit makes it impossible for one submission to
/// promote a later unrelated install more than once.
fn consume_prior_compose_submission(prior_compose_submitted: &mut bool) -> bool {
    std::mem::take(prior_compose_submitted)
}

fn budget_inputs(snapshot: ShResidencySnapshot) -> ShGpuBudgetInputs {
    ShGpuBudgetInputs {
        fixed: fixed_gpu_charges(snapshot),
        pool_minima: StreamedPoolMinima {
            dense_group_bytes: snapshot.dense_group_minimum_bytes,
            indirect_delta_bytes: snapshot.indirect_delta_minimum_bytes,
            direct_delta_bytes: snapshot.direct_delta_minimum_bytes,
            animated_direct_delta_bytes: snapshot.animated_direct_delta_minimum_bytes,
        },
        renderer_effective_floor_bytes: Some(snapshot.effective_floor_bytes),
    }
}

fn fixed_gpu_charges(snapshot: ShResidencySnapshot) -> FixedGpuCharges {
    FixedGpuCharges {
        fixed_metadata_bytes: snapshot.fixed_metadata_bytes,
        whole_resident_scatter_bytes: snapshot.whole_resident_scatter_bytes,
        active_pool_capacity_bytes: snapshot.active_capacity_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_budget_uses_real_renderer_pool_figures() {
        let snapshot = ShResidencySnapshot {
            fixed_metadata_bytes: 11,
            whole_resident_scatter_bytes: 13,
            active_capacity_bytes: 17,
            effective_floor_bytes: 41,
            dense_group_minimum_bytes: Some(5),
            indirect_delta_minimum_bytes: Some(2),
            direct_delta_minimum_bytes: None,
            animated_direct_delta_minimum_bytes: Some(3),
            ..ShResidencySnapshot::default()
        };

        let inputs = budget_inputs(snapshot);
        assert_eq!(inputs.fixed.fixed_metadata_bytes, 11);
        assert_eq!(inputs.fixed.whole_resident_scatter_bytes, 13);
        assert_eq!(inputs.fixed.active_pool_capacity_bytes, 17);
        assert_eq!(inputs.pool_minima.dense_group_bytes, Some(5));
        assert_eq!(inputs.pool_minima.indirect_delta_bytes, Some(2));
        assert_eq!(inputs.pool_minima.direct_delta_bytes, None);
        assert_eq!(inputs.pool_minima.animated_direct_delta_bytes, Some(3));
        assert_eq!(inputs.renderer_effective_floor_bytes, Some(41));
    }

    #[test]
    fn runtime_mode_gate_allows_only_sync_proof_at_this_checkpoint() {
        assert!(require_sync_proof_mode(ShStreamingMode::SyncProof).is_ok());
        let async_error = require_sync_proof_mode(ShStreamingMode::Async).unwrap_err();
        assert!(async_error.to_string().contains("not yet implemented"));
        let late_off_error = require_sync_proof_mode(ShStreamingMode::Off).unwrap_err();
        assert!(late_off_error.to_string().contains("changed to off"));
    }

    #[test]
    fn session_submission_latch_defers_promotion_until_the_next_frame_boundary() {
        let mut prior_compose_submitted = false;

        // Frame N accepted an install, but a later acquire/scene failure did
        // not submit compose work. Frame N+1 must retain it uncomposed.
        assert!(!consume_prior_compose_submission(
            &mut prior_compose_submitted
        ));

        // A successful Frame N+1 compose submission can only be consumed at
        // the following pre-compose seam, and exactly once.
        prior_compose_submitted = true;
        assert!(consume_prior_compose_submission(
            &mut prior_compose_submitted
        ));
        assert!(
            !consume_prior_compose_submission(&mut prior_compose_submitted),
            "one completed frame cannot publish more than once"
        );
    }
}
