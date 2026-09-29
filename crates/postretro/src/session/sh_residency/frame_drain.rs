//! Per-frame SH drain pump: sync-proof reads, or async completion admission
//! and request submission. See: context/lib/rendering_pipeline.md §4

use anyhow::Result;
use postretro_level_loader::{PreparedShCluster, ShDrainBatch};
use postretro_visibility::VisibleCells;

use super::super::sh_async_workers::ShWorkerResult;
use super::ShStreamingSession;
use crate::sh_streaming::controller::SyncReadResult;

impl ShStreamingSession {
    /// Synchronous proof policy: fill the controller permits from the
    /// current target set, then submit at most the controller's bounded drain
    /// batch. The remaining ready items retain their permits for a later frame.
    pub(super) fn prepare_sync_proof_batch(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        self.update_targets(visible_cells, camera_cell, monotonic_seconds)?;
        while matches!(self.read_one_sync()?, SyncReadResult::Prepared(_)) {}
        self.prepare_batch()
    }

    /// The frame thread only drains completed ownership transfers and queues
    /// work. Positional I/O, hash verification and decode stay on workers.
    pub(super) fn prepare_async_batch(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        self.update_targets(visible_cells, camera_cell, monotonic_seconds)?;
        let Some(workers) = self.workers.as_ref() else {
            // A prior generation may still be finishing an uncancellable OS
            // read. Publish target deltas and miss fallback without waiting;
            // the session starts this generation's workers after join.
            return self.prepare_batch();
        };
        // Publish before admitting or submitting, so the issuer never reads a
        // cluster this frame's target update dropped.
        workers.publish_targets(self.controller.targets());
        while let Some(completion) = workers.try_completion().map_err(anyhow::Error::msg)? {
            if !self
                .controller
                .matches_completion_identity(completion.request)
            {
                continue;
            }
            let chunk = match completion.result {
                ShWorkerResult::Cancelled => {
                    self.controller
                        .admit_cancelled_request(completion.request)?;
                    continue;
                }
                ShWorkerResult::Failed(error) => {
                    // A departed queued item only releases its permit; only
                    // targeted work can earn the one warning.
                    if self.controller.admit_failed_request(completion.request)? {
                        log::warn!(
                            "[SH streaming] cluster {} read/decode failed: {error}",
                            completion.request.cluster_id
                        );
                    }
                    continue;
                }
                ShWorkerResult::Prepared(chunk) => chunk,
            };
            // Admission drops stale or departed work and releases its permit;
            // foreign generations never reach it.
            self.controller.admit_prepared(PreparedShCluster {
                generation: completion.request.generation,
                content_tag: completion.request.content_tag,
                chunk,
            })?;
        }
        // Budget policy runs inside the drain and may suppress targets. Take
        // requests only after it, and publish its final target set before
        // submitting them: an idle issuer wakes on a submission and would
        // otherwise read a suppressed cluster against the earlier publish.
        self.promote_composed_clusters();
        let (batch, requests) = self.controller.take_async_drain_batch_and_requests()?;
        let workers = self
            .workers
            .as_ref()
            .expect("async workers were present above");
        workers.publish_targets(self.controller.targets());
        for request in requests {
            workers.submit(request).map_err(anyhow::Error::msg)?;
        }
        Ok(batch)
    }
}
