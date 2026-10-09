//! Lightmap controller preload for capture and tests: synchronous pair reads.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{LightmapBlockClass, LightmapDrainBatch, PreparedLightmapBlock};

use super::*;

/// What one synchronous preload read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "capture"),
    allow(dead_code, reason = "capture and tests preload synchronously")
)]
pub(crate) struct LightmapPreloadReads {
    /// Pairs read whole and placed in the batch.
    pub(crate) pairs: u32,
    /// Their stored bytes, both halves.
    pub(crate) bytes: u64,
    /// Pairs whose read or split failed; they stay non-resident.
    pub(crate) failed: u32,
}

impl LightmapResidencyController {
    /// Reads every mandatory and visible target synchronously through the
    /// block source, then returns one batch holding them all for the renderer
    /// to install before a first frame. Band targets are left to in-play
    /// prefetch. Blocks in `keep_missing` are never read (a capture's forced
    /// miss); they stay targeted and non-resident.
    ///
    /// Install time is not frame time. The shared per-drain budget bounds what
    /// a frame spends installing; no frame renders until this batch is
    /// installed, so the whole set goes in one batch. One batch also means at
    /// most one pool growth and no deferral: the pool is fresh, so no
    /// generation is retiring. The cost is a transient: every pair's bytes are
    /// held together until the drain.
    ///
    /// Call once, after a demand update and before any read or drain. The
    /// renderer's outcome goes back through [`Self::apply_outcome`].
    #[cfg_attr(
        not(feature = "capture"),
        allow(dead_code, reason = "capture and tests preload synchronously")
    )]
    pub(crate) fn preload_batch(
        &mut self,
        keep_missing: &[u32],
    ) -> Result<(LightmapDrainBatch, LightmapPreloadReads), LightmapResidencyError> {
        if self.drain_outstanding || self.permits_in_use != 0 {
            return Err(LightmapResidencyError::PreloadAfterStart);
        }
        if self.request_order_stale {
            self.rebuild_request_order();
        }
        let mut reads = LightmapPreloadReads::default();
        for index in 0..self.request_order.len() {
            let block = self.request_order[index];
            let slot = self.slots[block as usize];
            let Some(target) = slot.target else {
                continue;
            };
            if target.class == LightmapBlockClass::Band
                || slot.phase != BlockPhase::Absent
                || keep_missing.contains(&block)
            {
                continue;
            }
            let pair_bytes = self.map.facts(block).pair_bytes();
            self.counters.reads_requested += 1;
            match self.source.read_block_pair(block) {
                Ok(payload) => {
                    self.ready.insert(
                        block,
                        PreparedLightmapBlock {
                            generation: self.generation,
                            content_tag: self.content_tag,
                            block,
                            payload,
                        },
                    );
                    // Held like an issuer read, so the outcome releases it.
                    self.slots[block as usize].phase = BlockPhase::Ready;
                    self.take_permit(block, PairCharge::NeverRefused);
                    reads.pairs += 1;
                    reads.bytes += pair_bytes;
                }
                Err(error) => {
                    self.counters.failed_reads += 1;
                    reads.failed += 1;
                    self.fail(block, true, "preload read", &error);
                }
            }
        }
        let ready: Vec<u32> = self.ready.keys().copied().collect();
        let batch = self.build_batch(ready)?;
        Ok((batch, reads))
    }
}
