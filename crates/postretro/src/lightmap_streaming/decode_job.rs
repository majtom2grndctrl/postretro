//! One read lightmap pair's decode on the level's decode pool.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use postretro_level_loader::PrlLoadError;

use super::route::{LightmapCompletion, LightmapReadResult, LightmapRouteLedger};
use super::source::LightmapBlockSource;
use crate::streaming::decode_pool::{DecodeJob, DecodeRank};
use crate::streaming::request::{ReadRequest, StreamResource};
use crate::streaming::target_set::TargetSet;

/// A pair's stored bytes, charged to the route ledger at the pair's decoded
/// size from the moment it is queued until the frame consumes its
/// completion. A pair whose block left its targets is not decoded. A decode
/// that fails or panics, a departed pair, a job the pool discards, a job
/// that finds the pool retiring, and a job dropped still holding it each
/// return that charge themselves.
pub(super) struct LightmapDecodeJob {
    pub(super) request: ReadRequest,
    pub(super) lightmap: Vec<u8>,
    pub(super) shadowmask: Option<Vec<u8>>,
    /// Charged and not yet released or handed to a completion.
    pub(super) charged_bytes: u64,
    pub(super) source: Arc<dyn LightmapBlockSource>,
    /// The controller's block targets, checked before the decode; the
    /// block's membership and class there are the job's rank.
    pub(super) targets: Arc<TargetSet>,
    pub(super) ledger: Arc<LightmapRouteLedger>,
    /// Taken by the pair's one completion.
    pub(super) completed: Option<SyncSender<LightmapCompletion>>,
}

impl LightmapDecodeJob {
    fn release(&mut self) {
        self.ledger.release(std::mem::take(&mut self.charged_bytes));
    }

    /// Sends the pair's one completion. False when the session is gone.
    fn complete(&mut self, result: LightmapReadResult, charged_bytes: u64) -> bool {
        self.completed.take().is_some_and(|completed| {
            completed
                .send(LightmapCompletion {
                    request: self.request,
                    result,
                    charged_bytes,
                })
                .is_ok()
        })
    }

    /// Returns the job's charge and completes its pair with `result`. The
    /// controller frees the pair's permit when it admits it; a gone session
    /// has nothing left to free.
    fn complete_uncharged(&mut self, result: LightmapReadResult) {
        self.release();
        self.complete(result, 0);
    }
}

impl DecodeJob for LightmapDecodeJob {
    /// Departed once the block leaves its targets (its run then completes
    /// cancelled without a decode), otherwise the block's live class.
    fn rank(&self) -> DecodeRank {
        DecodeRank::live(
            &self.targets,
            self.request.key,
            self.request.class,
            StreamResource::LightmapBlock,
        )
    }

    fn run(&mut self, retiring: &AtomicBool, queue_wait: Duration) {
        if retiring.load(Ordering::Acquire) {
            return self.release();
        }
        let block = self.request.key;
        if !self.targets.contains(block) {
            // Left its targets while queued (a declined session untargets
            // every block): no decode. A block targeted again before this
            // check decodes.
            return self.complete_uncharged(LightmapReadResult::Cancelled);
        }
        let started = Instant::now();
        let decoded = self.source.payload_from_pair_bytes(
            block,
            std::mem::take(&mut self.lightmap),
            self.shadowmask.take(),
        );
        self.ledger.record_decode(queue_wait, started.elapsed());
        if retiring.load(Ordering::Acquire) {
            return self.release();
        }
        match decoded {
            Ok(payload) => {
                let charged_bytes = std::mem::take(&mut self.charged_bytes);
                if !self.complete(LightmapReadResult::Decoded(payload), charged_bytes) {
                    // The session is gone; its bytes drop here, undelivered.
                    self.ledger.release(charged_bytes);
                }
            }
            Err(error) => self.complete_uncharged(LightmapReadResult::DecodeFailed(error)),
        }
    }

    fn discard(mut self: Box<Self>) {
        self.release();
    }

    /// The decode panicked before delivering: the pair fails like a corrupt
    /// one and is never uploaded. Never panics: the release is an atomic
    /// subtraction and a send to a gone session is ignored, so the pair
    /// completes unless the pool is retiring.
    fn panicked(mut self: Box<Self>, retiring: &AtomicBool) {
        if retiring.load(Ordering::Acquire) {
            return self.release();
        }
        let block = self.request.key;
        self.complete_uncharged(LightmapReadResult::DecodeFailed(
            PrlLoadError::SectionValidation {
                section: StreamResource::LightmapBlock.label(),
                message: format!("lightmap block {block} decode panicked"),
            },
        ));
    }
}

/// Returns any charge still held by a job dropped without its cleanup having
/// run. The release is an atomic subtraction, so it cannot panic while
/// unwinding.
impl Drop for LightmapDecodeJob {
    fn drop(&mut self) {
        self.release();
    }
}
