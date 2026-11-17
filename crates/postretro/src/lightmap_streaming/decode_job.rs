//! One read lightmap pair's decode on the level's decode pool.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

use super::route::{LightmapCompletion, LightmapReadResult, LightmapRouteLedger};
use super::source::LightmapBlockSource;
use crate::streaming::decode_pool::DecodeJob;
use crate::streaming::request::{ReadRequest, ReadTier};

/// A pair's stored bytes, charged to the route ledger at the pair's decoded
/// size from the moment it is queued until the frame consumes its
/// completion. A decode that fails, a job the pool discards, and a job that
/// finds the pool retiring each return that charge themselves.
pub(super) struct LightmapDecodeJob {
    pub(super) request: ReadRequest,
    pub(super) lightmap: Vec<u8>,
    pub(super) shadowmask: Option<Vec<u8>>,
    pub(super) charged_bytes: u64,
    pub(super) source: Arc<dyn LightmapBlockSource>,
    pub(super) ledger: Arc<LightmapRouteLedger>,
    pub(super) completed: SyncSender<LightmapCompletion>,
}

impl DecodeJob for LightmapDecodeJob {
    fn tier(&self) -> ReadTier {
        self.request.tier
    }

    fn run(self: Box<Self>, retiring: &AtomicBool) {
        let Self {
            request,
            lightmap,
            shadowmask,
            charged_bytes,
            source,
            ledger,
            completed,
        } = *self;
        let decoded = source.payload_from_pair_bytes(request.key, lightmap, shadowmask);
        if retiring.load(Ordering::Acquire) {
            ledger.release(charged_bytes);
            return;
        }
        let completion = match decoded {
            Ok(payload) => LightmapCompletion {
                request,
                result: LightmapReadResult::Decoded(payload),
                charged_bytes,
            },
            Err(error) => {
                ledger.release(charged_bytes);
                LightmapCompletion {
                    request,
                    result: LightmapReadResult::DecodeFailed(error),
                    charged_bytes: 0,
                }
            }
        };
        let charged = completion.charged_bytes;
        if completed.send(completion).is_err() {
            // The session is gone; its bytes drop here, undelivered.
            ledger.release(charged);
        }
    }

    fn discard(self: Box<Self>) {
        self.ledger.release(self.charged_bytes);
    }
}
