//! Session-owned lightmap streaming glue: controller, route, completions, and
//! the renderer handoff seam. See: context/lib/rendering_pipeline.md §4

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_loader::{
    LevelWorld, LightmapDrainBatch, LightmapDrainOutcome, LightmapStreamManifest,
};

use crate::lightmap_streaming::controller::{LightmapPreloadReads, LightmapResidencyController};
use crate::lightmap_streaming::demand::DemandFrame;
use crate::lightmap_streaming::route::{
    LightmapCompletion, LightmapReadRoute, LightmapRouteLedger, lightmap_route,
};
use crate::lightmap_streaming::source::{LightmapBlockSource, ManifestBlockSource};
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::issuer::{ReadIssuer, ReadRoute};
use crate::streaming::shared_drain::SharedDrain;

#[cfg(test)]
mod tests;

/// The loaded level's streamed-lightmap inputs. Present only when the level's
/// `LightmapStorage` is `Streaming`, which the loader selects only with a
/// usable id-51 set and portals.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LightmapLevelView<'a> {
    pub(crate) manifest: &'a Arc<LightmapStreamManifest>,
    pub(crate) residency_set: &'a CellResidencySetSection,
    pub(crate) cluster_directory: Option<&'a ClusterDirectorySection>,
}

impl<'a> LightmapLevelView<'a> {
    pub(crate) fn of(world: &'a LevelWorld) -> Option<Self> {
        Some(Self {
            manifest: world.lightmap_stream_manifest()?,
            residency_set: world.cell_residency_set.as_ref()?,
            cluster_directory: world.cluster_directory(),
        })
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
    pub(crate) fn new(view: LightmapLevelView<'_>) -> Result<Self> {
        let mut session = Self::with_source(
            Arc::new(ManifestBlockSource::new(Arc::clone(view.manifest))),
            view.residency_set,
            view.cluster_directory,
        )?;
        session.manifest = Some(Arc::downgrade(view.manifest));
        Ok(session)
    }

    pub(in crate::session) fn with_source(
        source: Arc<dyn LightmapBlockSource>,
        residency_set: &CellResidencySetSection,
        cluster_directory: Option<&ClusterDirectorySection>,
    ) -> Result<Self> {
        let hints = cluster_directory
            .map(ClusterHints::decode)
            .transpose()
            .context("[Lightmap streaming] id-49 cluster hints")?;
        let controller =
            LightmapResidencyController::new(Arc::clone(&source), residency_set, hints.as_ref())?;
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
        })
    }

    pub(crate) fn is_for(&self, manifest: &Arc<LightmapStreamManifest>) -> bool {
        self.manifest
            .as_ref()
            .is_some_and(|streamed| std::ptr::eq(streamed.as_ptr(), Arc::as_ptr(manifest)))
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

    /// Demand from one frame's visibility without draining: capture's fixed
    /// view, before its preload.
    #[cfg_attr(
        not(feature = "capture"),
        allow(dead_code, reason = "capture's preload reads its view's demand")
    )]
    pub(crate) fn update_demand(&mut self, frame: DemandFrame<'_>) {
        self.controller.update(frame);
    }

    /// Makes the current mandatory and visible targets resident before a
    /// first frame: reads them synchronously through the level's positional
    /// reader, hands them to `install` (the renderer's lightmap drain) as one
    /// batch, and applies its outcome. Blocks in `keep_missing` stay
    /// targeted but unread. Call after a demand update and before this
    /// session's first drain or read.
    pub(crate) fn preload(
        &mut self,
        keep_missing: &[u32],
        install: impl FnOnce(LightmapDrainBatch) -> Result<LightmapDrainOutcome>,
    ) -> Result<LightmapPreloadSummary> {
        if self.awaiting_renderer.is_some() {
            bail!("[Lightmap streaming] preload found a parked drain batch");
        }
        let started = Instant::now();
        let (batch, reads) = self.controller.preload_batch(keep_missing)?;
        let outcome = install(batch)?;
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
        self.controller.apply_outcome(outcome).map_err(Into::into)
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "Task 11 reads the controller for diagnostics")
    )]
    pub(crate) fn controller(&self) -> &LightmapResidencyController {
        &self.controller
    }

    #[allow(dead_code, reason = "Task 11 wires the pool-cap and lead sliders")]
    pub(crate) fn controller_mut(&mut self) -> &mut LightmapResidencyController {
        &mut self.controller
    }

    /// Drops everything but the source, which the retiring issuer's route may
    /// still read through until its thread joins.
    pub(in crate::session) fn into_retained_source(self) -> Arc<dyn LightmapBlockSource> {
        self.source
    }

    #[cfg(test)]
    pub(in crate::session) fn ledger(&self) -> &Arc<LightmapRouteLedger> {
        &self.ledger
    }

    #[cfg(test)]
    pub(in crate::session) fn parked_batch(&self) -> Option<&LightmapDrainBatch> {
        self.awaiting_renderer.as_ref()
    }
}
