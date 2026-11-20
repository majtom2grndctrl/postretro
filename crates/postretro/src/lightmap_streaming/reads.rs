//! Lightmap controller reads: request order and admission, completions, failures.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::PoolKind::{self, Lightmap, Mask};
use postretro_level_loader::{LightmapBlockClass, PreparedLightmapBlock};

use super::miss_causes::RequestWait;
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
    /// its slot texels against each pool's band headroom.
    Band,
    /// A mandatory or visible pair: any permit, the reserve included. The
    /// renderer places it before any band pair, so its slot texels come off
    /// the headroom band reads see, in each pool.
    NeverRefused,
}

impl LightmapResidencyController {
    /// Submits every read the permits, the in-hand byte bound, and the band
    /// limits allow, in request order: visible, pinned, mandatory by lead
    /// (nearest first), then band (priority, then lead). A frame whose path
    /// holds demand submits no new read, and a non-portal frame reads only
    /// the camera cell's baked set and the pins.
    ///
    /// Band reads stop at their half of the permits and bytes, and at the
    /// lightmap pool's band headroom from the last outcome less the slots of
    /// every pair already heading for it; the renderer's refusal stays
    /// authoritative. The mask pool running out stops only the pairs with
    /// planes: a pair that stores none asks nothing of it.
    ///
    /// Where the pass stops is its verdict on the blocks it left, kept for
    /// the frame's miss causes until the next pass.
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
        self.request_verdict = RequestPassVerdict::default();
        let mut mask_pool_full = false;
        for index in 0..self.request_order.len() {
            let block = self.request_order[index];
            let slot = self.slots[block as usize];
            let Some(target) = slot.target else {
                continue;
            };
            if self.permits_in_use >= MAX_LIGHTMAP_PERMITS {
                self.request_verdict
                    .stop(RequestWait::Permit, target.rank(block));
                break;
            }
            if slot.phase != BlockPhase::Absent
                || (self.camera_set_only && !self.demand.in_camera_set(&self.map, block))
            {
                continue;
            }
            let facts = *self.map.facts(block);
            if self.permits_in_use > 0
                && self.in_hand_bytes + facts.pair_bytes() > self.in_hand_cap()
            {
                self.request_verdict
                    .stop(RequestWait::InHandCap, target.rank(block));
                break;
            }
            let band = target.class == LightmapBlockClass::Band;
            if band {
                let wait = self.band_share_wait(&facts).or_else(|| {
                    (!self.band_room_fits(Lightmap, &facts)).then_some(RequestWait::PoolRoom)
                });
                if let Some(wait) = wait {
                    self.request_verdict.stop(wait, target.rank(block));
                    break;
                }
                if facts.texels.mask != 0 {
                    mask_pool_full = mask_pool_full || !self.band_room_fits(Mask, &facts);
                    if mask_pool_full {
                        self.request_verdict.skip_for_mask_room(target.rank(block));
                        continue;
                    }
                }
            }
            submit(ReadRequest {
                resource: StreamResource::LightmapBlock,
                key: block,
                class: target.drain_class,
                identity: self.identity(),
                ranges: facts.ranges,
            })
            .map_err(LightmapResidencyError::Submit)?;
            let slot = &mut self.slots[block as usize];
            slot.phase = BlockPhase::InFlight;
            slot.optional_in_flight = target.drain_class.tier() == ReadTier::Optional;
            self.take_permit(
                block,
                if band {
                    PairCharge::Band
                } else {
                    PairCharge::NeverRefused
                },
            );
            self.demand.recheck_if_held(block);
            self.counters.reads_requested += 1;
        }
        Ok(())
    }

    /// What keeps one more band pair from the band's share of the permits
    /// and in-hand bytes; `None` when it fits.
    fn band_share_wait(&self, facts: &BlockFacts) -> Option<RequestWait> {
        if self.band_permits >= MAX_BAND_PERMITS {
            return Some(RequestWait::Permit);
        }
        let fits = self.band_permits == 0
            || self.band_in_hand_bytes + facts.pair_bytes() <= self.band_in_hand_cap();
        (!fits).then_some(RequestWait::InHandCap)
    }

    /// Whether one more band pair fits `kind`'s band headroom less every pair
    /// in hand.
    fn band_room_fits(&self, kind: PoolKind, facts: &BlockFacts) -> bool {
        self.band_committed_texels[kind] + self.never_refused_texels[kind] + facts.texels[kind]
            <= self.pool[kind].band_headroom_texels
    }

    /// The renderer took or gave back one pool item's reserved room. Headroom
    /// moves with it, so a pair in hand moves the other way and is charged
    /// once.
    pub(super) fn set_room_reserved(&mut self, kind: PoolKind, item: u32, reserved: bool) {
        let block = kind.block_of(item);
        // A lightmap half takes its block's slot; a mask plane takes a slot
        // at that plane's own extent.
        let facts = self.map.facts(block);
        let texels = match kind {
            PoolKind::Lightmap => facts.texels.lightmap,
            PoolKind::Mask => facts.mask_plane_texels[mask_item_plane(item)],
        };
        let slot = &mut self.slots[block as usize];
        if slot.room_held.set(kind, item, reserved) && slot.charge == PairCharge::NeverRefused {
            if reserved {
                self.never_refused_texels[kind] -= texels;
            } else {
                self.never_refused_texels[kind] += texels;
            }
        }
    }

    /// `block` became mandatory or visible. A band pair in hand moves to the
    /// never-refused charge. A band read in flight needs no message: its
    /// block's class in the target set, written by the retarget, lifts its
    /// pending read at the issuer or its queued decode in the pool. It is
    /// counted once as an in-flight lift.
    pub(super) fn promote_in_hand(&mut self, block: u32) {
        let slot = &mut self.slots[block as usize];
        if slot.charge == PairCharge::Band {
            let facts = *self.map.facts(block);
            slot.charge = PairCharge::NeverRefused;
            self.band_permits -= 1;
            self.band_in_hand_bytes -= facts.pair_bytes();
            self.band_committed_texels -= facts.texels;
            self.never_refused_texels += slot.room_held.unheld_texels(&facts);
        }
        if slot.phase == BlockPhase::InFlight && slot.optional_in_flight {
            slot.optional_in_flight = false;
            self.counters.lifted_in_flight += 1;
        }
    }

    /// The issuer this controller submitted to was replaced; its reads will
    /// never complete here. Every in-flight pair returns to Absent, its
    /// permit and charges released, to be read again through the next
    /// issuer. Ready, drained, and installed pairs are untouched.
    pub(crate) fn cancel_in_flight(&mut self) {
        // The latest pass's stop describes reads that no longer exist.
        self.request_verdict = RequestPassVerdict::default();
        for block in 0..self.slots.len() as u32 {
            if self.slots[block as usize].phase == BlockPhase::InFlight {
                self.release_in_hand(block);
                self.slots[block as usize].phase = BlockPhase::Absent;
                self.demand.recheck_if_held(block);
                self.counters.cancelled_reads += 1;
            }
        }
    }

    /// The session stopped streaming: every ready pair's payload is dropped,
    /// its permit and charges released, and the pair returns to Absent.
    /// In-flight, drained, and installed pairs are untouched.
    pub(crate) fn release_ready(&mut self) {
        while let Some((block, _)) = self.ready.pop_first() {
            self.release_in_hand(block);
            self.slots[block as usize].phase = BlockPhase::Absent;
            self.demand.recheck_if_held(block);
        }
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

    /// Admits one completion from the issuer or the decode pool. A completion
    /// from another generation, for a block with no read in flight, or for a
    /// block that left demand is dropped with its buffers. A pair becomes
    /// ready only whole: the issuer completes a request after both its ranges
    /// are read, and the pool after every plane decodes. The decoded payload
    /// arrives split per plane and moves into the ready map: admission copies
    /// no payload byte.
    pub(crate) fn admit_completion(
        &mut self,
        completion: LightmapCompletion,
    ) -> Result<(), LightmapResidencyError> {
        let LightmapCompletion {
            request, result, ..
        } = completion;
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
        let payload = match result {
            LightmapReadResult::Decoded(payload) if targeted => payload,
            LightmapReadResult::Decoded(_) => {
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
                self.demand.recheck_if_held(block);
                return Ok(());
            }
            LightmapReadResult::Failed(error) => {
                self.release_in_hand(block);
                self.counters.failed_reads += 1;
                self.fail(block, targeted, "read", &error);
                return Ok(());
            }
            LightmapReadResult::DecodeFailed(error) => {
                // A corrupt block is never uploaded: it fails like a read.
                self.release_in_hand(block);
                self.counters.failed_reads += 1;
                self.fail(block, targeted, "decode", &error);
                return Ok(());
            }
        };
        // The pair keeps its permit and charges until the renderer installs
        // or refuses it.
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
        self.mark_frame(block, FrameMarks::FAILED);
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
        self.demand.recheck_if_held(block);
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
            PairCharge::NeverRefused => {
                let held = self.slots[block as usize].room_held;
                self.never_refused_texels += held.unheld_texels(&facts);
            }
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
            PairCharge::NeverRefused => {
                self.never_refused_texels -= slot.room_held.unheld_texels(&facts);
            }
        }
        self.permits_in_use -= 1;
        self.in_hand_bytes -= facts.pair_bytes();
        self.requests_due = true;
    }
}
