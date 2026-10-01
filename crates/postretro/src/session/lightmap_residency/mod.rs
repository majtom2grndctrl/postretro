//! Session-owned lightmap streaming glue: controller, route, completions, and
//! the renderer handoff seam. See: context/lib/rendering_pipeline.md §4

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{
    LevelWorld, LightmapDrainBatch, LightmapDrainOutcome, LightmapStreamManifest, PrlReadCounters,
};
use postretro_renderer::{
    LightmapResidencyDrainError, LightmapStreamCounters, LightmapStreamingLiveDiagnostics,
};

use super::lightmap_streaming_diagnostics::{
    LightmapStreamingLogWindow, SectionReadBytes, assemble_live_diagnostics,
};
use crate::lightmap_streaming::controller::{LightmapPreloadReads, LightmapResidencyController};
use crate::lightmap_streaming::demand::DemandFrame;
#[cfg(feature = "capture")]
use crate::lightmap_streaming::levers::LightmapLevers;
use crate::lightmap_streaming::route::{
    LightmapCompletion, LightmapReadRoute, LightmapRouteLedger, lightmap_route,
};
use crate::lightmap_streaming::source::{LightmapBlockSource, ManifestBlockSource};
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::issuer::{ReadIssuer, ReadRoute};
use crate::streaming::shared_drain::SharedDrain;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod walk_measurement;

/// The loaded level's streamed-lightmap inputs. Present only when the level's
/// `LightmapStorage` is `Streaming`, which the loader selects only with a
/// usable id-51 set and portals. The level's id-49 hints are decoded once at
/// level scope and passed beside it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LightmapLevelView<'a> {
    pub(crate) manifest: &'a Arc<LightmapStreamManifest>,
    pub(crate) residency_set: &'a CellResidencySetSection,
}

impl<'a> LightmapLevelView<'a> {
    pub(crate) fn of(world: &'a LevelWorld) -> Option<Self> {
        Some(Self {
            manifest: world.lightmap_stream_manifest()?,
            residency_set: world.cell_residency_set.as_ref()?,
        })
    }
}

/// What a renderer lightmap-drain error means for the level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RendererDrainFailure {
    /// The renderer holds no streamed pool: it fell back to the placeholder
    /// lightmap (device limits). The level stops streaming its lightmap.
    NotStreaming,
    /// A GPU-side failure the renderer rolled back whole. The pairs return
    /// to Absent and are read again, until the failure recurs on too many
    /// drains in a row and the level stops streaming its lightmap.
    RolledBack,
    /// The batch broke the drain contract (identity, ids, generation): a bug,
    /// and fatal.
    Contract,
}

impl RendererDrainFailure {
    pub(crate) fn of(error: &LightmapResidencyDrainError) -> Self {
        match error {
            LightmapResidencyDrainError::NotStreaming => Self::NotStreaming,
            LightmapResidencyDrainError::InvalidBatch(_)
            | LightmapResidencyDrainError::StaleGeneration { .. }
            | LightmapResidencyDrainError::GenerationResetRequired { .. } => Self::Contract,
            LightmapResidencyDrainError::GpuCapacity { .. }
            | LightmapResidencyDrainError::Upload(_) => Self::RolledBack,
        }
    }
}

/// What a synchronous preload read and the renderer installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LightmapPreloadSummary {
    pub(crate) reads: LightmapPreloadReads,
    pub(crate) installed: u32,
    /// Pairs the renderer handed back unplaced. One preload batch into a
    /// fresh pool cannot defer; a nonzero count is a contract break.
    pub(crate) deferred: u32,
    /// Pairs whose payload could not fill their block.
    pub(crate) failed_installs: u32,
    pub(crate) elapsed: Duration,
}

/// Lightmap residency for one loaded level generation. Its reads go through
/// the level-scope issuer, which owns the route once it spawns.
pub(crate) struct LightmapStreamingSession {
    /// The loaded manifest this session streams. Weak, so session identity
    /// neither extends the manifest's life nor shows in its strong count;
    /// `source` keeps the allocation alive, so the address cannot be reused.
    manifest: Option<Weak<LightmapStreamManifest>>,
    source: Arc<dyn LightmapBlockSource>,
    controller: LightmapResidencyController,
    ledger: Arc<LightmapRouteLedger>,
    completions: Receiver<LightmapCompletion>,
    /// Handed to the level-scope issuer when it spawns.
    route: Option<LightmapReadRoute>,
    /// The latest batch, waiting for the renderer's lightmap drain. While it
    /// waits, the controller builds no further batch.
    awaiting_renderer: Option<LightmapDrainBatch>,
    /// The level file's per-section read counters; `None` for a test source.
    read_counters: Option<Arc<PrlReadCounters>>,
    /// Renderer drains that failed and were rolled back; the first warns.
    rolled_back_drains: u64,
    /// Rolled-back drains since the renderer last returned an outcome.
    consecutive_rolled_back_drains: u32,
    /// The level declined this lightmap while the issuer still delivers into
    /// this session. It stays parked for the rest of the level, untargeted,
    /// holding no payload and only draining its completion queue, until the
    /// level unloads or changes.
    declined: bool,
    live: LightmapStreamingLiveDiagnostics,
    log_window: LightmapStreamingLogWindow,
}

impl std::fmt::Debug for LightmapStreamingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightmapStreamingSession")
            .field("controller", &self.controller)
            .field("route_spawned", &self.route.is_none())
            .finish_non_exhaustive()
    }
}

impl LightmapStreamingSession {
    /// `hints` is the level's id 49, decoded once at level scope; `None`
    /// without id 49.
    pub(crate) fn new(view: LightmapLevelView<'_>, hints: Option<&ClusterHints>) -> Result<Self> {
        let mut session = Self::with_source(
            Arc::new(ManifestBlockSource::new(Arc::clone(view.manifest))),
            view.residency_set,
            hints,
        )?;
        session.manifest = Some(Arc::downgrade(view.manifest));
        session.read_counters = Some(Arc::clone(view.manifest.read_counters()));
        Ok(session)
    }

    pub(in crate::session) fn with_source(
        source: Arc<dyn LightmapBlockSource>,
        residency_set: &CellResidencySetSection,
        hints: Option<&ClusterHints>,
    ) -> Result<Self> {
        let controller =
            LightmapResidencyController::new(Arc::clone(&source), residency_set, hints)?;
        let ledger = Arc::new(LightmapRouteLedger::default());
        let (route, completions) = lightmap_route(
            Arc::clone(&source),
            Arc::clone(controller.target_bitset()),
            Arc::clone(&ledger),
        );
        Ok(Self {
            manifest: None,
            source,
            controller,
            ledger,
            completions,
            route: Some(route),
            awaiting_renderer: None,
            read_counters: None,
            rolled_back_drains: 0,
            consecutive_rolled_back_drains: 0,
            declined: false,
            live: LightmapStreamingLiveDiagnostics::default(),
            log_window: LightmapStreamingLogWindow::default(),
        })
    }

    pub(crate) fn is_for(&self, manifest: &Arc<LightmapStreamManifest>) -> bool {
        self.manifest
            .as_ref()
            .is_some_and(|streamed| std::ptr::eq(streamed.as_ptr(), Arc::as_ptr(manifest)))
    }

    /// The streamed manifest's identity, without keeping it alive; `None`
    /// for a test source.
    pub(in crate::session) fn manifest_identity(&self) -> Option<Weak<LightmapStreamManifest>> {
        self.manifest.clone()
    }

    /// Demand from the camera cell's baked set and the pins alone: the spawn
    /// camera cell at level install.
    pub(crate) fn update_camera_set(
        &mut self,
        residency_set: &CellResidencySetSection,
        camera_cell: u32,
    ) {
        self.controller
            .update_camera_set(residency_set, camera_cell);
    }

    /// Capture's fixed view: its camera cell's baked set plus every drawn
    /// cell's blocks as visible, whatever the visibility path, without
    /// draining. See [`LightmapResidencyController::update_capture_view`].
    #[cfg_attr(
        not(feature = "capture"),
        allow(dead_code, reason = "capture's preload reads its view's demand")
    )]
    pub(crate) fn update_capture_demand(&mut self, frame: DemandFrame<'_>) {
        self.controller.update_capture_view(frame);
    }

    /// Makes the current mandatory and visible targets resident before a
    /// first frame: reads them synchronously through the level's positional
    /// reader, hands them to `install` (the renderer's lightmap drain) as one
    /// batch, and applies its outcome. Blocks in `keep_missing` stay
    /// targeted but unread. Call after a demand update and before this
    /// session's first drain or read.
    ///
    /// A renderer error is returned as the typed
    /// [`LightmapResidencyDrainError`] (see [`RendererDrainFailure::of`]),
    /// after the controller has taken the batch's pairs back.
    pub(crate) fn preload(
        &mut self,
        keep_missing: &[u32],
        install: impl FnOnce(
            LightmapDrainBatch,
        ) -> Result<LightmapDrainOutcome, LightmapResidencyDrainError>,
    ) -> Result<LightmapPreloadSummary> {
        if self.awaiting_renderer.is_some() {
            bail!("[Lightmap streaming] preload found a parked drain batch");
        }
        let started = Instant::now();
        let (batch, reads) = self.controller.preload_batch(keep_missing)?;
        let outcome = match install(batch) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.controller.abort_drain();
                return Err(error.into());
            }
        };
        let summary = LightmapPreloadSummary {
            reads,
            installed: outcome.installed.len() as u32,
            deferred: outcome.deferred.len() as u32,
            failed_installs: outcome.failed.len() as u32,
            elapsed: started.elapsed(),
        };
        self.controller.apply_outcome(outcome)?;
        Ok(summary)
    }

    /// Whether the camera cell's mandatory set is resident, as of the latest
    /// demand update. See [`LightmapResidencyController::settled`].
    pub(crate) fn settled(&self) -> bool {
        self.controller.settled()
    }

    /// The route for the level-scope issuer; `None` once taken.
    pub(in crate::session) fn take_route(&mut self) -> Option<Box<dyn ReadRoute>> {
        self.route
            .take()
            .map(|route| Box::new(route) as Box<dyn ReadRoute>)
    }

    /// The level-scope issuer is being replaced while this session lives on
    /// (only SH changed). A fresh route waits for the next issuer; every read
    /// in flight on the old one returns to Absent, to be read again, while
    /// ready, drained and resident pairs stay. Returns the old completion
    /// queue: the retiring issuer may still deliver into it, and whatever it
    /// delivers must release its bytes once that thread has joined.
    pub(in crate::session) fn detach_from_issuer(&mut self) -> Receiver<LightmapCompletion> {
        let (route, completions) = lightmap_route(
            Arc::clone(&self.source),
            Arc::clone(self.controller.target_bitset()),
            Arc::clone(&self.ledger),
        );
        self.route = Some(route);
        self.controller.cancel_in_flight();
        std::mem::replace(&mut self.completions, completions)
    }

    /// First half of this frame's drain: demand from this frame's visibility,
    /// completed reads admitted, and ready pairs offered to `drain`.
    pub(in crate::session) fn begin_drain(
        &mut self,
        frame: DemandFrame<'_>,
        drain: &mut SharedDrain,
    ) -> Result<()> {
        self.controller.update(frame);
        loop {
            match self.completions.try_recv() {
                Ok(completion) => {
                    let read_bytes = completion.result.read_bytes();
                    let admitted = self.controller.admit_completion(completion);
                    // Consumed: kept as a ready payload or dropped.
                    self.ledger.release(read_bytes);
                    admitted?;
                }
                Err(TryRecvError::Empty) => break,
                // The route holds the sender until the issuer exits; only a
                // spawned route can disconnect.
                Err(TryRecvError::Disconnected) if self.route.is_none() => {
                    bail!("[Lightmap streaming] completion queue disconnected")
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.controller.offer_ready(drain)?;
        Ok(())
    }

    /// Second half: the admitted pairs and target deltas become this drain's
    /// batch, parked for the renderer, then new reads go to the issuer.
    pub(in crate::session) fn finish_drain(
        &mut self,
        drain: &SharedDrain,
        issuer: Option<&ReadIssuer>,
    ) -> Result<()> {
        if let Some(batch) = self.controller.finish_drain(drain)? {
            debug_assert!(
                self.awaiting_renderer.is_none(),
                "the controller holds batches while an outcome is outstanding"
            );
            self.awaiting_renderer = Some(batch);
        }
        if let Some(issuer) = issuer {
            self.controller
                .take_requests(&mut |request| issuer.submit(request))?;
        }
        Ok(())
    }

    /// The renderer's lightmap drain takes the parked batch, installs it, and
    /// returns its outcome through [`Self::apply_outcome`] before the next
    /// frame prepares another.
    pub(crate) fn take_drain_batch_for_renderer(&mut self) -> Option<LightmapDrainBatch> {
        self.awaiting_renderer.take()
    }

    pub(crate) fn apply_outcome(&mut self, outcome: LightmapDrainOutcome) -> Result<()> {
        self.consecutive_rolled_back_drains = 0;
        self.controller.apply_outcome(outcome).map_err(Into::into)
    }

    /// The renderer failed the parked batch. The controller takes its pairs
    /// back (they return to Absent and are read again) and re-sends every
    /// target in the next batch. A rolled-back drain warns once per session.
    pub(crate) fn renderer_drain_failed(
        &mut self,
        error: &LightmapResidencyDrainError,
    ) -> RendererDrainFailure {
        self.controller.abort_drain();
        let failure = RendererDrainFailure::of(error);
        if failure == RendererDrainFailure::RolledBack {
            self.rolled_back_drains += 1;
            self.consecutive_rolled_back_drains += 1;
            if self.rolled_back_drains == 1 {
                log::warn!(
                    "[Lightmap streaming] renderer drain failed and was rolled back: {error}; \
                     its pairs will be read again"
                );
            }
        }
        failure
    }

    /// Rolled-back drains since the renderer last returned an outcome.
    pub(crate) fn consecutive_rolled_back_drains(&self) -> u32 {
        self.consecutive_rolled_back_drains
    }

    /// The level declined this lightmap, but the running issuer still
    /// delivers into this session's queue: it stays parked, inert, for the
    /// rest of the level, and retires when the level unloads or changes. Its
    /// parked batch is dropped unsent. The controller never admits another
    /// completion, so every pair in hand, in flight, ready or drained,
    /// releases its payload, permit and in-hand bytes.
    pub(in crate::session) fn park_declined(&mut self) {
        self.declined = true;
        self.awaiting_renderer = None;
        self.controller.abort_drain();
        self.controller.release_ready();
        self.controller.cancel_in_flight();
        // Untarget every block so the shared issuer cancels this session's
        // pending reads at once instead of reading them for nothing.
        self.controller
            .target_bitset()
            .publish(&std::collections::BTreeSet::new());
    }

    /// A parked, declined session is an inert sink for the rest of the level:
    /// the shared issuer SH reads through keeps running and may still deliver
    /// cancelled or finished lightmap reads into this queue. Each frame this
    /// releases their bytes so the queue never fills and the ledger stays
    /// exact.
    pub(in crate::session) fn drain_declined_completions(&mut self) {
        while let Ok(completion) = self.completions.try_recv() {
            self.ledger.release(completion.result.read_bytes());
        }
    }

    /// Whether the level declined this lightmap; see [`Self::park_declined`].
    pub(in crate::session) fn is_declined(&self) -> bool {
        self.declined
    }

    /// Closes the frame after its drain outcome: counts the frame's visible
    /// misses, reassembles the live diagnostics from the controller, the
    /// route, the level's read counters and `renderer`, and offers them to
    /// the periodic log. Allocation-free unless a log line is due.
    pub(crate) fn finish_frame(
        &mut self,
        renderer: Option<&LightmapStreamCounters>,
        now_seconds: f64,
    ) {
        self.refresh_diagnostics(renderer);
        self.log_window.observe(now_seconds, &self.live);
    }

    /// Counts the latest frame's visible misses and reassembles the live
    /// diagnostics, without the log: capture's fixed view after its preload.
    pub(crate) fn refresh_diagnostics(&mut self, renderer: Option<&LightmapStreamCounters>) {
        self.controller.count_visible_misses();
        assemble_live_diagnostics(
            &mut self.live,
            &self.controller,
            &self.ledger,
            renderer,
            self.read_counters
                .as_deref()
                .map(|counters| counters as &dyn SectionReadBytes),
        );
    }

    /// The latest assembled view, for the Streaming tab and the capture report.
    #[cfg(any(feature = "capture", feature = "dev-tools"))]
    pub(crate) fn live_diagnostics(&self) -> &LightmapStreamingLiveDiagnostics {
        &self.live
    }

    /// Capture's pool-cap override writes here; the dev-tools sliders go
    /// through `set_slider_levers`.
    #[cfg(feature = "capture")]
    pub(crate) fn levers_mut(&mut self) -> &mut LightmapLevers {
        self.controller.levers_mut()
    }

    /// The levers as the dev-tools Streaming tab edits them.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn slider_levers(&self) -> postretro_renderer::LightmapStreamingLevers {
        let levers = self.controller.levers();
        postretro_renderer::LightmapStreamingLevers {
            pool_cap_layers: levers.pool_cap_layers(),
            lead_metres: levers.lead_metres(),
            max_lead_metres: levers.max_lead_metres(),
        }
    }

    /// Applies the Streaming tab's levers. The lead takes effect at the next
    /// demand update; the cap rides the next drain batch.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn set_slider_levers(
        &mut self,
        sliders: postretro_renderer::LightmapStreamingLevers,
    ) {
        let levers = self.controller.levers_mut();
        levers.set_pool_cap_layers(sliders.pool_cap_layers);
        levers.set_lead_metres(sliders.lead_metres);
    }

    #[cfg(test)]
    pub(crate) fn controller(&self) -> &LightmapResidencyController {
        &self.controller
    }

    /// Drops everything but the source, which the retiring issuer's route may
    /// still read through until its thread joins.
    pub(in crate::session) fn into_retained_source(self) -> Arc<dyn LightmapBlockSource> {
        self.source
    }

    pub(in crate::session) fn ledger(&self) -> &Arc<LightmapRouteLedger> {
        &self.ledger
    }

    #[cfg(test)]
    pub(in crate::session) fn parked_batch(&self) -> Option<&LightmapDrainBatch> {
        self.awaiting_renderer.as_ref()
    }
}
