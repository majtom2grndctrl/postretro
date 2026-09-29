//! Per-frame SH drain pump: sync-proof reads, or async completion admission
//! and request submission. See: context/lib/rendering_pipeline.md §4

use anyhow::Result;
use postretro_level_loader::{PreparedShCluster, ShDrainBatch, ShStreamingMode};
use postretro_visibility::VisibleCells;

use super::super::sh_async_workers::ShWorkerResult;
use super::ShStreamingSession;
use crate::sh_streaming::controller::SyncReadResult;
use crate::streaming::shared_drain::SharedDrain;

/// SH's half-built batch between the two halves of a level-scope drain.
#[derive(Debug)]
pub(in crate::session) struct PendingShDrain {
    batch: ShDrainBatch,
    /// Take and submit new read requests after admission (async mode with
    /// workers running).
    submit_requests: bool,
}

impl ShStreamingSession {
    /// The frame thread only drains completed ownership transfers and queues
    /// work. Positional I/O, hash verification and decode stay on workers.
    /// SH as the drain's only resource; the level-scope step runs the same
    /// halves with lightmap blocks sharing the admission.
    #[cfg(test)]
    pub(super) fn prepare_async_batch(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<ShDrainBatch> {
        let mut drain = SharedDrain::default();
        let pending =
            self.begin_async_drain(visible_cells, camera_cell, monotonic_seconds, &mut drain)?;
        drain.admit()?;
        self.finish_drain(pending, &drain)
    }

    /// First half of this frame's drain in the loaded mode: target update,
    /// reads (sync-proof) or completion admission (async), then SH's ready
    /// clusters offered to `drain`.
    pub(in crate::session) fn begin_drain(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
        drain: &mut SharedDrain,
    ) -> Result<PendingShDrain> {
        match self.mode {
            ShStreamingMode::SyncProof => {
                self.update_targets(visible_cells, camera_cell, monotonic_seconds)?;
                while matches!(self.read_one_sync()?, SyncReadResult::Prepared(_)) {}
                self.promote_composed_clusters();
                Ok(PendingShDrain {
                    batch: self.controller.begin_drain(false, drain)?,
                    submit_requests: false,
                })
            }
            ShStreamingMode::Async => {
                self.begin_async_drain(visible_cells, camera_cell, monotonic_seconds, drain)
            }
            ShStreamingMode::Off => unreachable!("a loaded streaming session cannot be off"),
        }
    }

    fn begin_async_drain(
        &mut self,
        visible_cells: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
        drain: &mut SharedDrain,
    ) -> Result<PendingShDrain> {
        self.update_targets(visible_cells, camera_cell, monotonic_seconds)?;
        let Some(workers) = self.workers.as_ref() else {
            // A prior generation may still be finishing an uncancellable OS
            // read. Publish target deltas and miss fallback without waiting;
            // the level-scope owner starts this generation's workers after join.
            self.promote_composed_clusters();
            return Ok(PendingShDrain {
                batch: self.controller.begin_drain(true, drain)?,
                submit_requests: false,
            });
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
        // Budget policy runs inside the drain and may suppress targets.
        // Requests are taken only after it, in `finish_drain`.
        self.promote_composed_clusters();
        Ok(PendingShDrain {
            batch: self.controller.begin_drain(true, drain)?,
            submit_requests: true,
        })
    }

    /// Second half: SH's admitted clusters join the batch; then, with workers
    /// running, new requests are taken and submitted. The drain's final target
    /// set is published before submitting: an idle issuer wakes on a
    /// submission and would otherwise read a suppressed cluster against the
    /// earlier publish.
    pub(in crate::session) fn finish_drain(
        &mut self,
        pending: PendingShDrain,
        drain: &SharedDrain,
    ) -> Result<ShDrainBatch> {
        let PendingShDrain {
            mut batch,
            submit_requests,
        } = pending;
        self.controller.finish_drain(&mut batch, drain)?;
        if submit_requests {
            let requests = self.controller.take_requests()?;
            let workers = self
                .workers
                .as_ref()
                .expect("async workers were present when the drain began");
            workers.publish_targets(self.controller.targets());
            for request in requests {
                workers.submit(request).map_err(anyhow::Error::msg)?;
            }
        }
        Ok(batch)
    }
}
