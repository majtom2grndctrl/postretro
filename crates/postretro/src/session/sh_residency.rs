//! Session-owned SH streaming lifecycle glue.
//!
//! The renderer sees only loader-owned drain batches and returns their outcome.
//! This module keeps target selection, synchronous proof reads, and permit
//! accounting on the application side. See:
//! `context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md`.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use postretro_level_loader::{
    PreparedShCluster, ShDrainBatch, ShDrainOutcome, ShStreamManifest, ShStreamingMode,
    requested_streaming_mode,
};
#[cfg(feature = "capture")]
use postretro_renderer::ShStreamingLifecycleSummary;
use postretro_renderer::{Renderer, ShResidencySnapshot};
use postretro_visibility::VisibleCells;

use super::sh_async_workers::{ShAsyncWorkers, ShWorkerRetirement};
#[cfg(feature = "capture")]
use crate::sh_streaming::budget::BytePhase;
use crate::sh_streaming::budget::{FixedGpuCharges, ShGpuBudgetInputs, StreamedPoolMinima};
use crate::sh_streaming::controller::{ShResidencyController, SyncReadResult};

#[cfg(test)]
#[path = "../sh_streaming/sync_manifest_test_fixture.rs"]
mod sync_manifest_test_fixture;

/// Controller state whose lifetime belongs to one loaded session, never to the
/// renderer. A distinct loaded manifest replaces this object before any new
/// frame can issue a batch for the next map.
#[derive(Debug)]
pub(crate) struct ShStreamingSession {
    /// Retained separately for session identity. Two distinct level loads can
    /// have identical bytes/content tags yet must receive distinct nonzero
    /// controller generations.
    manifest: Arc<ShStreamManifest>,
    mode: ShStreamingMode,
    controller: ShResidencyController,
    workers: Option<ShAsyncWorkers>,
    /// Set only after a frame actually submitted all compose work. Its next
    /// pre-compose batch may promote the corresponding accepted clusters.
    prior_compose_submitted: bool,
}

impl ShStreamingSession {
    /// Builds a controller from the renderer's actual allocation snapshot.
    #[cfg(feature = "capture")]
    pub(crate) fn from_renderer(
        manifest: Arc<ShStreamManifest>,
        renderer: &Renderer,
    ) -> Result<Self> {
        Self::from_renderer_with_mode(manifest, renderer, ShStreamingMode::SyncProof)
    }

    fn from_renderer_with_mode(
        manifest: Arc<ShStreamManifest>,
        renderer: &Renderer,
        mode: ShStreamingMode,
    ) -> Result<Self> {
        let snapshot = renderer.sh_residency_snapshot().with_context(
            || "[SH streaming] renderer has no residency snapshot for a streamed level",
        )?;
        let mut session = Self::from_snapshot(manifest.clone(), snapshot)?;
        session.mode = mode;
        Ok(session)
    }

    /// Capture-specific spelling of [`Self::from_renderer`]. It deliberately
    /// has no mode selection; the capture caller applies the shared Task 10
    /// gate before it creates this proof controller.
    #[cfg(feature = "capture")]
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
            mode: ShStreamingMode::SyncProof,
            controller,
            workers: None,
            prior_compose_submitted: false,
        })
    }

    fn is_for_manifest(&self, manifest: &Arc<ShStreamManifest>) -> bool {
        Arc::ptr_eq(&self.manifest, manifest)
    }

    fn start_async_workers(&mut self) -> Result<()> {
        if self.mode == ShStreamingMode::Async && self.workers.is_none() {
            self.workers = Some(ShAsyncWorkers::new(self.manifest.clone())?);
        }
        Ok(())
    }

    fn begin_worker_retirement(&mut self) -> Option<ShWorkerRetirement> {
        self.workers.as_mut().map(ShAsyncWorkers::begin_retirement)
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
        match self.mode {
            ShStreamingMode::SyncProof => self.controller.take_drain_batch().map_err(Into::into),
            ShStreamingMode::Async => self.controller.take_async_drain_batch().map_err(Into::into),
            ShStreamingMode::Off => unreachable!("a loaded streaming session cannot be off"),
        }
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
    #[cfg(feature = "capture")]
    pub(crate) fn all_targets_sampleable(&self) -> bool {
        self.controller.all_targets_sampleable()
    }

    /// Merge the controller's frame-local policy/permit view with worker
    /// phase bytes for capture. The renderer contributes pool capacity through
    /// its own snapshot; it deliberately never reaches into this session.
    #[cfg(feature = "capture")]
    pub(crate) fn residency_lifecycle_summary(&self) -> Result<ShStreamingLifecycleSummary> {
        let controller = self.controller.report_snapshot();
        let worker = self
            .workers
            .as_ref()
            .map(ShAsyncWorkers::phase_snapshot)
            .transpose()
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default();
        let phase = |controller_phase: BytePhase, worker_phase: BytePhase, label| {
            let current = controller_phase
                .current_bytes
                .checked_add(worker_phase.current_bytes)
                .ok_or_else(|| anyhow::anyhow!("[SH streaming] {label} current bytes overflow"))?;
            // The independently checked ledgers cannot reconstruct one exact
            // combined historic instant. Their sum is a checked conservative
            // upper bound across worker/controller ownership transfer; expose
            // it under that explicit name rather than mislabeling it a peak.
            let high_water_upper_bound = controller_phase
                .high_water_bytes
                .checked_add(worker_phase.high_water_bytes)
                .ok_or_else(|| {
                    anyhow::anyhow!("[SH streaming] {label} high-water upper bound overflow")
                })?;
            Ok::<_, anyhow::Error>((current, high_water_upper_bound))
        };
        let (encoded_current_bytes, encoded_high_water_upper_bound_bytes) =
            phase(controller.cpu.encoded, worker.encoded, "encoded")?;
        let (decoding_current_bytes, decoding_high_water_upper_bound_bytes) =
            phase(controller.cpu.decoding, worker.decoding, "decoding")?;
        let (ready_current_bytes, ready_high_water_upper_bound_bytes) =
            phase(controller.cpu.ready, worker.ready, "ready")?;
        let count = |value: usize, label: &'static str| {
            u64::try_from(value).map_err(|_| anyhow::anyhow!("[SH streaming] {label} exceeds u64"))
        };
        Ok(ShStreamingLifecycleSummary {
            non_evictable_overshoot_bytes: controller.non_evictable_overshoot_bytes,
            encoded_current_bytes,
            encoded_high_water_upper_bound_bytes,
            decoding_current_bytes,
            decoding_high_water_upper_bound_bytes,
            ready_current_bytes,
            ready_high_water_upper_bound_bytes,
            permits_in_use: count(controller.permits_in_use, "permit count")?,
            target_clusters: count(controller.target_clusters, "target count")?,
            absent_clusters: count(controller.absent_clusters, "absent state count")?,
            queued_clusters: count(controller.queued_clusters, "queued state count")?,
            ready_clusters: count(controller.ready_clusters, "ready state count")?,
            installed_uncomposed_clusters: count(
                controller.installed_uncomposed_clusters,
                "installed state count",
            )?,
            sampleable_clusters: count(controller.sampleable_clusters, "sampleable state count")?,
            failed_clusters: count(controller.failed_clusters, "failed state count")?,
            misses: controller.counters.misses,
            installs: controller.counters.installs,
            evictions: controller.counters.evictions,
            retries: controller.counters.retries,
        })
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

    /// The frame thread only drains completed ownership transfers and queues
    /// work. Positional I/O, hash verification and decode stay on workers.
    fn prepare_async_batch(
        &mut self,
        visible_cells: &VisibleCells,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        self.update_targets(visible_cells, monotonic_seconds)?;
        let Some(workers) = self.workers.as_ref() else {
            // A prior generation may still be finishing an uncancellable OS
            // read. Publish target deltas and miss fallback without waiting;
            // the session starts this generation's four workers after join.
            return self.prepare_batch();
        };
        while let Some(completion) = workers.try_completion().map_err(anyhow::Error::msg)? {
            if !self
                .controller
                .matches_completion_identity(completion.request)
            {
                continue;
            }
            if !self.controller.matches_queued_request(completion.request) {
                // The controller handles a departed queued item by releasing
                // its permit; foreign generations never touch this session.
                if completion.result.is_err() {
                    self.controller.admit_failed_request(completion.request)?;
                } else if let Ok(chunk) = completion.result {
                    self.controller.admit_prepared(PreparedShCluster {
                        generation: completion.request.generation,
                        content_tag: completion.request.content_tag,
                        chunk,
                    })?;
                }
                continue;
            }
            match completion.result {
                Ok(chunk) => {
                    self.controller.admit_prepared(PreparedShCluster {
                        generation: completion.request.generation,
                        content_tag: completion.request.content_tag,
                        chunk,
                    })?;
                }
                Err(error) => {
                    if self.controller.admit_failed_request(completion.request)? {
                        log::warn!(
                            "[SH streaming] cluster {} read/decode failed: {error}",
                            completion.request.cluster_id
                        );
                    }
                }
            }
        }
        while let Some(request) = self.controller.take_next_request()? {
            workers.submit(request).map_err(anyhow::Error::msg)?;
        }
        self.prepare_batch()
    }
}

impl Drop for ShStreamingSession {
    fn drop(&mut self) {
        // The manager joins its workers while this session still retains the
        // manifest/file. Cancellation never bypasses completion identity.
        if let Some(mut workers) = self.workers.take() {
            workers.stop();
        }
    }
}

impl super::Session {
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
    /// from the exact visible cells for this frame, and returns its one bounded
    /// pre-compose batch. Legacy maps bypass the environment gate completely.
    pub(crate) fn prepare_sh_streaming_drain(
        &mut self,
        manifest: Option<&Arc<ShStreamManifest>>,
        renderer: &Renderer,
        visible_cells: &VisibleCells,
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
                streaming.prepare_sync_proof_batch(visible_cells, monotonic_seconds)
            }
            ShStreamingMode::Async => {
                streaming.prepare_async_batch(visible_cells, monotonic_seconds)
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

/// Applies the temporary Task 10 runtime gate after the loader has already
/// validated a streamed manifest. Windowed play and static capture share this
/// exact decision; legacy/off never has a manifest to reach it.
#[cfg(any(test, feature = "capture"))]
pub(crate) fn require_sync_proof_mode(mode: ShStreamingMode) -> Result<()> {
    match mode {
        ShStreamingMode::SyncProof => Ok(()),
        ShStreamingMode::Async => {
            bail!("[SH streaming] static capture requires POSTRETRO_SH_STREAMING=sync-proof")
        }
        // The loader yields a legacy `ShStorage` for `off`, so reaching this
        // branch means the environment changed after the map load.
        ShStreamingMode::Off => bail!(
            "[SH streaming] mode changed to off after this streamed map was loaded; reload the map"
        ),
    }
}

fn require_loaded_streaming_mode(mode: ShStreamingMode) -> Result<()> {
    if mode == ShStreamingMode::Off {
        bail!(
            "[SH streaming] mode changed to off after this streamed map was loaded; reload the map"
        );
    }
    Ok(())
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
    use postretro_test_log_capture::LogCapture;
    use std::time::{Duration, Instant};

    // Regression: the permitted retry for one failed worker request emitted a duplicate warning.
    #[test]
    fn async_worker_failure_warns_once_across_same_identity_retry() {
        let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
        let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
        let mut session = ShStreamingSession::from_snapshot(
            manifest,
            ShResidencySnapshot {
                effective_floor_bytes: 1024 * 1024,
                ..ShResidencySnapshot::default()
            },
        )
        .unwrap();
        session.mode = ShStreamingMode::Async;
        session.start_async_workers().unwrap();

        // The manifest retains an open file. Truncating that same file after
        // load makes both real positional worker reads fail.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(0)
            .unwrap();
        let capture = LogCapture::start();
        let visible = VisibleCells::Culled(vec![0]);
        let empty = VisibleCells::Culled(Vec::new());
        session.prepare_async_batch(&visible, 0.0).unwrap();

        let wait_for_failure = |session: &mut ShStreamingSession, time| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while session.controller.state(0)
                != Some(crate::sh_streaming::controller::ClusterResidencyState::Failed)
            {
                assert!(Instant::now() < deadline, "worker failure did not arrive");
                std::thread::yield_now();
                session.prepare_async_batch(&visible, time).unwrap();
            }
        };
        wait_for_failure(&mut session, 0.0);
        capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");

        session.prepare_async_batch(&empty, 0.1).unwrap();
        session.prepare_async_batch(&empty, 2.1).unwrap();
        session.prepare_async_batch(&visible, 2.2).unwrap();
        assert_eq!(session.controller.counters().retries, 1);
        wait_for_failure(&mut session, 2.2);
        assert_eq!(session.controller.permits_in_use(), 0);
        capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
    }

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
        // Regression: a level whose entire SH allocation is below the 256 MiB
        // requested floor must still accept the renderer's exact physical floor.
        let accounting = crate::sh_streaming::budget::ShResidencyAccounting::new(inputs).unwrap();
        assert_eq!(accounting.effective_floor_bytes().unwrap(), 41);
    }

    #[test]
    fn capture_mode_gate_requires_sync_proof() {
        assert!(require_sync_proof_mode(ShStreamingMode::SyncProof).is_ok());
        let async_error = require_sync_proof_mode(ShStreamingMode::Async).unwrap_err();
        assert!(async_error.to_string().contains("static capture requires"));
        let late_off_error = require_sync_proof_mode(ShStreamingMode::Off).unwrap_err();
        assert!(late_off_error.to_string().contains("changed to off"));
        assert!(require_loaded_streaming_mode(ShStreamingMode::Async).is_ok());
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
