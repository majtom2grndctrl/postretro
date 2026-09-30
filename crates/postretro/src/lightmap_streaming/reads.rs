//! Lightmap controller reads: request order and admission, completions, failures.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{LightmapBlockClass, PreparedLightmapBlock};

use super::*;
use crate::lightmap_streaming::route::{LightmapCompletion, LightmapReadResult};
use crate::streaming::request::{ReadIdentity, ReadRequest, StreamResource};

/// What a pair in hand (in flight, ready, or in a drain) is charged against,
/// from its permit until it is installed, refused, failed, or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PairCharge {
    /// Not in hand.
    None,
    /// A band pair: the band's share of the permits and in-hand bytes, and
    /// its slot texels against the pool's band headroom.
    Band,
    /// A mandatory or visible pair: any permit, the reserve included. The
    /// renderer places it before any band pair, so its slot texels come off
    /// the headroom band reads see.
    NeverRefused,
}

impl LightmapResidencyController {
    /// Resubmits promoted in-flight reads at the mandatory tier, then submits
    /// every read the permits, the in-hand byte bound, and the band limits
    /// allow, in request order: visible, pinned, mandatory by lead (nearest
    /// first), then band (priority, then lead). A frame whose path holds
    /// demand submits no new read, and a non-portal frame reads only the
    /// camera cell's baked set and the pins.
    ///
    /// Band reads stop at their half of the permits and bytes, and at the
    /// pool's band headroom from the last outcome less the slots of every
    /// pair already heading for the pool; the renderer's refusal stays
    /// authoritative.
    pub(crate) fn take_requests(
        &mut self,
        submit: &mut dyn FnMut(ReadRequest) -> Result<(), &'static str>,
    ) -> Result<(), LightmapResidencyError> {
        self.submit_tier_raises(submit)?;
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
            if slot.phase != BlockPhase::Absent
                || (self.camera_set_only && !self.demand.in_camera_set(&self.map, block))
            {
                continue;
            }
            let facts = *self.map.facts(block);
            if self.permits_in_use > 0
                && self.in_hand_bytes + facts.pair_bytes() > MAX_IN_HAND_PAIR_BYTES
            {
                break;
            }
            let band = target.class == LightmapBlockClass::Band;
            if band && !self.band_read_fits(&facts) {
                break;
            }
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
            slot.read_tier = target.drain_class.tier();
            self.take_permit(
                block,
                if band {
                    PairCharge::Band
                } else {
                    PairCharge::NeverRefused
                },
            );
            self.counters.reads_requested += 1;
        }
        Ok(())
    }

    /// Whether one more band pair fits the band's share of the permits and
    /// in-hand bytes, and the pool's band headroom less every pair in hand.
    fn band_read_fits(&self, facts: &BlockFacts) -> bool {
        if self.band_permits >= MAX_BAND_PERMITS {
            return false;
        }
        if self.band_permits > 0
            && self.band_in_hand_bytes + facts.pair_bytes() > MAX_BAND_IN_HAND_PAIR_BYTES
        {
            return false;
        }
        self.band_committed_texels + self.never_refused_texels + facts.texels
            <= self.pool.band_headroom_texels
    }

    /// A band read promoted in flight is still queued at the optional tier;
    /// resubmitting it at the mandatory tier lets the issuer raise the
    /// pending request's tier, so it is read before optional work. The
    /// issuer absorbs the duplicate: one read, one completion.
    fn submit_tier_raises(
        &mut self,
        submit: &mut dyn FnMut(ReadRequest) -> Result<(), &'static str>,
    ) -> Result<(), LightmapResidencyError> {
        for index in 0..self.promotions.len() {
            let block = self.promotions[index];
            let slot = self.slots[block as usize];
            self.slots[block as usize].promotion_queued = false;
            let raises = slot.phase == BlockPhase::InFlight
                && slot.read_tier == ReadTier::Optional
                && slot
                    .target
                    .is_some_and(|target| target.drain_class.tier() == ReadTier::Mandatory);
            if !raises {
                continue;
            }
            submit(ReadRequest {
                resource: StreamResource::LightmapBlock,
                key: block,
                tier: ReadTier::Mandatory,
                identity: self.identity(),
                ranges: self.map.facts(block).ranges,
            })
            .map_err(LightmapResidencyError::Submit)?;
            self.slots[block as usize].read_tier = ReadTier::Mandatory;
            self.counters.tier_raises += 1;
        }
        self.promotions.clear();
        Ok(())
    }

    /// `block` became mandatory or visible. A band pair in hand moves to the
    /// never-refused charge; a band read in flight is queued for a tier
    /// raise.
    pub(super) fn promote_in_hand(&mut self, block: u32) {
        let facts = *self.map.facts(block);
        let slot = &mut self.slots[block as usize];
        if slot.charge == PairCharge::Band {
            slot.charge = PairCharge::NeverRefused;
            self.band_permits -= 1;
            self.band_in_hand_bytes -= facts.pair_bytes();
            self.band_committed_texels -= facts.texels;
            self.never_refused_texels += facts.texels;
        }
        if slot.phase == BlockPhase::InFlight
            && slot.read_tier == ReadTier::Optional
            && !slot.promotion_queued
        {
            slot.promotion_queued = true;
            self.promotions.push(block);
        }
    }

    /// The issuer this controller submitted to was replaced; its reads will
    /// never complete here. Every in-flight pair returns to Absent, its
    /// permit and charges released, to be read again through the next
    /// issuer. Ready, drained, and installed pairs are untouched.
    pub(crate) fn cancel_in_flight(&mut self) {
        for block in 0..self.slots.len() as u32 {
            if self.slots[block as usize].phase == BlockPhase::InFlight {
                self.release_in_hand(block);
                self.slots[block as usize].phase = BlockPhase::Absent;
                self.counters.cancelled_reads += 1;
            }
        }
        for index in 0..self.promotions.len() {
            let block = self.promotions[index];
            self.slots[block as usize].promotion_queued = false;
        }
        self.promotions.clear();
    }

    pub(super) fn rebuild_request_order(&mut self) {
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
                // Left demand while in flight: dropped here, released.
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

    /// Takes one pair's permit and in-hand bytes, charged as `charge`.
    pub(super) fn take_permit(&mut self, block: u32, charge: PairCharge) {
        let facts = *self.map.facts(block);
        match charge {
            PairCharge::None => {}
            PairCharge::Band => {
                self.band_permits += 1;
                self.band_in_hand_bytes += facts.pair_bytes();
                self.band_committed_texels += facts.texels;
            }
            PairCharge::NeverRefused => self.never_refused_texels += facts.texels,
        }
        self.slots[block as usize].charge = charge;
        self.permits_in_use += 1;
        self.in_hand_bytes += facts.pair_bytes();
    }

    /// Returns one pair's permit, in-hand bytes, and charge.
    pub(super) fn release_in_hand(&mut self, block: u32) {
        let facts = *self.map.facts(block);
        let slot = &mut self.slots[block as usize];
        match std::mem::replace(&mut slot.charge, PairCharge::None) {
            PairCharge::None => {}
            PairCharge::Band => {
                self.band_permits -= 1;
                self.band_in_hand_bytes -= facts.pair_bytes();
                self.band_committed_texels -= facts.texels;
            }
            PairCharge::NeverRefused => self.never_refused_texels -= facts.texels,
        }
        self.permits_in_use -= 1;
        self.in_hand_bytes -= facts.pair_bytes();
        self.requests_due = true;
    }
}
