//! Lightmap controller reads: request order and admission, completions, failures.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{LightmapBlockClass, PreparedLightmapBlock};

use super::*;
use crate::lightmap_streaming::route::{LightmapCompletion, LightmapReadResult};
use crate::streaming::request::{ReadIdentity, ReadRequest, StreamResource};

impl LightmapResidencyController {
    /// Submits every read the permits, the in-hand byte bound, and the band
    /// headroom allow, in request order: visible, pinned, mandatory by lead
    /// (nearest first), then band (priority, then lead). A frame whose path
    /// holds demand submits nothing. Band reads stop at the pool's band
    /// headroom from the last outcome; the renderer's refusal stays
    /// authoritative.
    pub(crate) fn take_requests(
        &mut self,
        submit: &mut dyn FnMut(ReadRequest) -> Result<(), &'static str>,
    ) -> Result<(), LightmapResidencyError> {
        if !self.may_request || !self.requests_due {
            return Ok(());
        }
        if self.request_order_stale {
            self.rebuild_request_order();
        }
        self.requests_due = false;
        for index in 0..self.request_order.len() {
            if self.permits_in_use >= MAX_LIGHTMAP_PERMITS {
                break;
            }
            let block = self.request_order[index];
            let slot = self.slots[block as usize];
            let Some(target) = slot.target else {
                continue;
            };
            if slot.phase != BlockPhase::Absent {
                continue;
            }
            let facts = *self.map.facts(block);
            if self.permits_in_use > 0
                && self.in_hand_bytes + facts.pair_bytes() > MAX_IN_HAND_PAIR_BYTES
            {
                break;
            }
            let band_texels = if target.class == LightmapBlockClass::Band {
                if self.band_committed_texels + facts.texels > self.pool.band_headroom_texels {
                    break;
                }
                facts.texels
            } else {
                0
            };
            submit(ReadRequest {
                resource: StreamResource::LightmapBlock,
                key: block,
                tier: target.drain_class.tier(),
                identity: self.identity(),
                ranges: facts.ranges,
            })
            .map_err(LightmapResidencyError::Submit)?;
            let slot = &mut self.slots[block as usize];
            slot.phase = BlockPhase::InFlight;
            slot.band_texels = band_texels;
            self.band_committed_texels += band_texels;
            self.permits_in_use += 1;
            self.in_hand_bytes += facts.pair_bytes();
            self.counters.reads_requested += 1;
        }
        Ok(())
    }

    fn rebuild_request_order(&mut self) {
        let mut order = std::mem::take(&mut self.request_order);
        order.clear();
        order.extend(self.demand.demanded_blocks(&self.map));
        let slots = &self.slots;
        order.retain(|&block| slots[block as usize].target.is_some());
        order.sort_unstable_by_key(|&block| {
            slots[block as usize]
                .target
                .map(|target| target.rank(block))
        });
        order.dedup();
        self.request_order = order;
        self.request_order_stale = false;
    }

    /// Admits one issuer completion. A completion from another generation,
    /// for a block with no read in flight, or for a block that left demand is
    /// dropped with its buffers. A read pair becomes ready only whole: the
    /// issuer completes a request after both its ranges are read.
    pub(crate) fn admit_completion(
        &mut self,
        completion: LightmapCompletion,
    ) -> Result<(), LightmapResidencyError> {
        let LightmapCompletion { request, result } = completion;
        let block = request.key;
        if request.resource != StreamResource::LightmapBlock
            || request.identity != self.identity()
            || block as usize >= self.slots.len()
        {
            self.counters.stale_completions += 1;
            return Ok(());
        }
        if self.slots[block as usize].phase != BlockPhase::InFlight {
            self.counters.duplicate_completions += 1;
            return Ok(());
        }
        let targeted = self.slots[block as usize].target.is_some();
        let (lightmap, shadowmask) = match result {
            LightmapReadResult::Read {
                lightmap,
                shadowmask,
            } if targeted => (lightmap, shadowmask),
            LightmapReadResult::Read { .. } => {
                // Left demand while in flight (P4): dropped here, released.
                self.counters.departed_reads += 1;
                self.release_in_hand(block);
                self.slots[block as usize].phase = BlockPhase::Absent;
                return Ok(());
            }
            LightmapReadResult::Cancelled => {
                self.counters.cancelled_reads += 1;
                self.release_in_hand(block);
                self.slots[block as usize].phase = BlockPhase::Absent;
                return Ok(());
            }
            LightmapReadResult::Failed(error) => {
                self.release_in_hand(block);
                self.counters.failed_reads += 1;
                self.fail(block, targeted, "read", &error);
                return Ok(());
            }
        };
        match self
            .source
            .payload_from_pair_bytes(block, lightmap, shadowmask)
        {
            Ok(payload) => {
                // The pair keeps its permit and charges until the renderer
                // installs or refuses it.
                self.ready.insert(
                    block,
                    PreparedLightmapBlock {
                        generation: self.generation,
                        content_tag: self.content_tag,
                        block,
                        payload,
                    },
                );
                self.slots[block as usize].phase = BlockPhase::Ready;
            }
            Err(error) => {
                self.release_in_hand(block);
                self.counters.failed_reads += 1;
                self.fail(block, true, "payload split", &error);
            }
        }
        Ok(())
    }

    /// One failure of `block`'s pair. A targeted block waits in `Failed`; an
    /// untargeted one is simply absent. Only a block's first failure warns.
    pub(super) fn fail(
        &mut self,
        block: u32,
        targeted: bool,
        stage: &str,
        error: &dyn std::fmt::Display,
    ) {
        let slot = &mut self.slots[block as usize];
        slot.failures = slot.failures.saturating_add(1);
        if slot.failures == 1 {
            log::warn!("[Lightmap streaming] block {block} {stage} failed: {error}");
        }
        slot.phase = if targeted || slot.failures >= MAX_BLOCK_FAILURES {
            BlockPhase::Failed
        } else {
            BlockPhase::Absent
        };
    }

    fn identity(&self) -> ReadIdentity {
        ReadIdentity {
            generation: self.generation,
            content_tag: self.content_tag,
            item_hash: [0; 32],
        }
    }

    /// Returns one pair's permit, in-hand bytes, and band texel charge.
    pub(super) fn release_in_hand(&mut self, block: u32) {
        let facts = *self.map.facts(block);
        let slot = &mut self.slots[block as usize];
        self.band_committed_texels -= slot.band_texels;
        slot.band_texels = 0;
        self.permits_in_use -= 1;
        self.in_hand_bytes -= facts.pair_bytes();
        self.requests_due = true;
    }
}
