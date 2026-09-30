//! Session-owned SH streaming lifecycle glue: controller, workers, and the
//! loader-to-renderer drain handoff. See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
#[cfg(any(test, feature = "capture"))]
use postretro_level_loader::ShDrainBatch;
use postretro_level_loader::{CellVisibility, ShDrainOutcome, ShStreamManifest, ShStreamingMode};
use postretro_renderer::{Renderer, ShResidencySnapshot, ShStreamingLiveDiagnostics};
use postretro_visibility::VisibleCells;

use super::sh_async_workers::{ShAsyncWorkers, ShWorkerRetirement, ShWorkerStats};
use super::sh_streaming_diagnostics::{ShStreamingLogWindow, assemble_live_diagnostics};
use crate::sh_streaming::budget::{FixedGpuCharges, ShGpuBudgetInputs, StreamedPoolMinima};
use crate::sh_streaming::controller::{ShResidencyController, SyncReadResult};
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::issuer::ReadRoute;

#[cfg(feature = "capture")]
mod capture_summary;
mod frame_drain;
mod session_hooks;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../../sh_streaming/sync_manifest_test_fixture.rs"]
pub(in crate::session) mod sync_manifest_test_fixture;

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
    /// Monotonic render time of the latest target update; paces the log.
    monotonic_seconds: f64,
    live: ShStreamingLiveDiagnostics,
    log_window: ShStreamingLogWindow,
    /// Read counters for the sync-proof path, which reads on the frame thread
    /// without workers. Capture runs in that mode, so its report needs them.
    sync_stats: ShWorkerStats,
}

impl ShStreamingSession {
    /// Builds a controller from the renderer's actual allocation snapshot.
    /// `cell_visibility` is the same level's id-46 section, if loaded;
    /// `hints` is its id 49, decoded once for every streamed resource.
    #[cfg(feature = "capture")]
    pub(crate) fn from_renderer(
        manifest: Arc<ShStreamManifest>,
        cell_visibility: Option<&CellVisibility>,
        renderer: &Renderer,
        hints: Arc<ClusterHints>,
    ) -> Result<Self> {
        Self::from_renderer_with_mode(
            manifest,
            cell_visibility,
            renderer,
            ShStreamingMode::SyncProof,
            hints,
        )
    }

    pub(in crate::session) fn from_renderer_with_mode(
        manifest: Arc<ShStreamManifest>,
        cell_visibility: Option<&CellVisibility>,
        renderer: &Renderer,
        mode: ShStreamingMode,
        hints: Arc<ClusterHints>,
    ) -> Result<Self> {
        let snapshot = renderer.sh_residency_snapshot().with_context(
            || "[SH streaming] renderer has no residency snapshot for a streamed level",
        )?;
        let mut session = Self::from_snapshot(manifest.clone(), snapshot, cell_visibility, hints)?;
        session.mode = mode;
        Ok(session)
    }

    /// Capture-specific spelling of [`Self::from_renderer`]. It deliberately
    /// has no mode selection; the capture caller applies the sync-proof mode
    /// gate (`require_sync_proof_mode`) before it creates this proof
    /// controller.
    #[cfg(feature = "capture")]
    pub(crate) fn for_capture(
        manifest: Arc<ShStreamManifest>,
        cell_visibility: Option<&CellVisibility>,
        renderer: &Renderer,
        hints: Arc<ClusterHints>,
    ) -> Result<Self> {
        Self::from_renderer(manifest, cell_visibility, renderer, hints)
    }

    pub(in crate::session) fn from_snapshot(
        manifest: Arc<ShStreamManifest>,
        snapshot: ShResidencySnapshot,
        cell_visibility: Option<&CellVisibility>,
        hints: Arc<ClusterHints>,
    ) -> Result<Self> {
        let controller = ShResidencyController::new(
            manifest.clone(),
            budget_inputs(snapshot),
            cell_visibility,
            hints,
        )?;
        Ok(Self {
            manifest,
            mode: ShStreamingMode::SyncProof,
            controller,
            workers: None,
            prior_compose_submitted: false,
            monotonic_seconds: 0.0,
            live: ShStreamingLiveDiagnostics::default(),
            log_window: ShStreamingLogWindow::default(),
            sync_stats: ShWorkerStats::default(),
        })
    }

    pub(in crate::session) fn is_for(
        &self,
        manifest: &Arc<ShStreamManifest>,
        mode: ShStreamingMode,
    ) -> bool {
        self.is_for_manifest(manifest) && self.mode == mode
    }

    /// Whether this session streams `manifest`, in whatever mode.
    pub(in crate::session) fn is_for_manifest(&self, manifest: &Arc<ShStreamManifest>) -> bool {
        Arc::ptr_eq(&self.manifest, manifest)
    }

    /// Starts SH-only workers with their own issuer. Production shares one
    /// level-scope issuer instead ([`Self::prepare_async_workers`]).
    #[cfg(test)]
    fn start_async_workers(&mut self) -> Result<()> {
        if self.mode == ShStreamingMode::Async && self.workers.is_none() {
            self.workers = Some(ShAsyncWorkers::new(self.manifest.clone())?);
        }
        Ok(())
    }

    /// The decode pool and SH's issuer route for the level-scope issuer, when
    /// this async session has no workers yet. The workers join the session
    /// once the owner has spawned the issuer ([`Self::attach_workers`]).
    pub(in crate::session) fn prepare_async_workers(
        &self,
    ) -> Result<Option<(ShAsyncWorkers, Box<dyn ReadRoute>)>> {
        if self.mode != ShStreamingMode::Async || self.workers.is_some() {
            return Ok(None);
        }
        Ok(Some(ShAsyncWorkers::prepare(self.manifest.clone())?))
    }

    pub(in crate::session) fn attach_workers(&mut self, workers: ShAsyncWorkers) {
        debug_assert!(self.workers.is_none(), "one worker set per SH session");
        self.workers = Some(workers);
    }

    /// The controller's residency generation: a recreated session gets a new
    /// one, so tests can tell a kept session from a replaced one.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.controller.generation()
    }

    pub(in crate::session) fn begin_worker_retirement(&mut self) -> Option<ShWorkerRetirement> {
        self.workers.as_mut().map(ShAsyncWorkers::begin_retirement)
    }

    /// Updates the controller from one real visibility result and the
    /// locator's camera cell (`None` without a level). Capture uses the same
    /// method with its deterministic fixed cell set.
    pub(crate) fn update_targets(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<()> {
        self.controller
            .update_targets(visible_cells, camera_cell, monotonic_seconds)?;
        self.monotonic_seconds = monotonic_seconds;
        Ok(())
    }

    /// Reads one target through the proof-only synchronous path.
    pub(crate) fn read_one_sync(&mut self) -> Result<SyncReadResult> {
        let started = Instant::now();
        let result = self.controller.read_one_sync_at_target_time()?;
        if let SyncReadResult::Prepared(cluster_id) = result {
            // One uncoalesced read per chunk. Its latency spans read and
            // decode, which the synchronous path performs as one call.
            let encoded_bytes = self.manifest.payloads().index[cluster_id as usize].payload_len;
            self.sync_stats.record_read(encoded_bytes, 1, 0);
            self.sync_stats.read_latency.record(started.elapsed());
        }
        Ok(result)
    }

    /// Read counters for the active mode: the async workers', or the
    /// session's own when sync-proof reads on the frame thread.
    fn read_stats(&self) -> Result<Option<ShWorkerStats>> {
        match self.mode {
            ShStreamingMode::SyncProof => Ok(Some(self.sync_stats)),
            _ => self
                .workers
                .as_ref()
                .map(ShAsyncWorkers::stats)
                .transpose()
                .map_err(anyhow::Error::msg),
        }
    }

    /// Prepares the bounded loader-to-renderer handoff after target/read work.
    /// A prior successful compose submission is promoted at this next drain;
    /// a failed or skipped frame leaves its installs uncomposed.
    #[cfg(feature = "capture")]
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
        self.refresh_diagnostics(Some(&snapshot))
    }

    /// Reassembles the live diagnostics after the renderer has reported this
    /// frame's installs, then offers them to the periodic log.
    fn refresh_diagnostics(&mut self, renderer: Option<&ShResidencySnapshot>) -> Result<()> {
        let worker = self.read_stats()?;
        assemble_live_diagnostics(
            &mut self.live,
            &self.controller.report_snapshot(),
            worker.as_ref(),
            renderer,
        );
        self.log_window.observe(self.monotonic_seconds, &self.live);
        Ok(())
    }

    /// The latest assembled view, for the dev-tools Streaming tab.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn live_diagnostics(&self) -> &ShStreamingLiveDiagnostics {
        &self.live
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
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn all_targets_sampleable(&self) -> bool {
        self.controller.all_targets_sampleable()
    }

    #[cfg(test)]
    pub(in crate::session) fn set_mode_for_test(&mut self, mode: ShStreamingMode) {
        self.mode = mode;
    }

    /// A renderer that accepts every ready cluster and submits compose.
    #[cfg(test)]
    pub(in crate::session) fn accept_drain_for_test(&mut self, batch: &ShDrainBatch) -> Result<()> {
        self.controller.apply_drain_outcome(ShDrainOutcome {
            accepted: batch
                .ready
                .iter()
                .map(|prepared| prepared.chunk.cluster_id)
                .collect(),
            ..ShDrainOutcome::default()
        })?;
        self.mark_compose_submitted();
        Ok(())
    }
}

impl Drop for ShStreamingSession {
    fn drop(&mut self) {
        // Stopping cancels the workers' handle on the level-scope issuer,
        // whose thread the level owner joins, and joins the decode threads
        // while this session still retains the manifest and its file.
        // Cancellation never bypasses completion identity.
        if let Some(mut workers) = self.workers.take() {
            workers.stop();
        }
    }
}

/// Applies the sync-proof mode gate after the loader has already validated a
/// streamed manifest: static capture streams SH only in sync-proof mode.
/// Legacy/off never has a manifest to reach it.
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

pub(in crate::session) fn require_loaded_streaming_mode(mode: ShStreamingMode) -> Result<()> {
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
