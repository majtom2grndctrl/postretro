//! Lightmap's half of the level-scope drain: ready offers, batches, outcomes.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::{
    LightmapBlockClass, LightmapDrainBatch, LightmapDrainOutcome, LightmapTarget,
};

use super::*;
use crate::streaming::drain_budget::DrainItem;
use crate::streaming::shared_drain::SharedDrain;

impl LightmapResidencyController {
    /// Offers every ready pair to the shared drain as one item, so a pair is
    /// admitted or deferred whole. Nothing is offered while the renderer has
    /// not yet returned the previous batch's outcome.
    pub(crate) fn offer_ready(
        &self,
        drain: &mut SharedDrain,
    ) -> Result<(), LightmapResidencyError> {
        if self.drain_outstanding {
            return Ok(());
        }
        for &block in self.ready.keys() {
            let Some(target) = self.slots[block as usize].target else {
                continue;
            };
            let facts = self.map.facts(block);
            drain.offer(
                DrainItem::pair(
                    target.rank(block),
                    facts.lightmap_bytes,
                    facts.shadowmask_bytes,
                )
                .map_err(|_| LightmapResidencyError::DrainBytesOverflow)?,
            );
        }
        Ok(())
    }

    /// Builds this drain's batch after the shared admission: the pool cap,
    /// a target reset on the generation's first drain or the sorted deltas
    /// since the last batch, and the admitted pairs. `None` while the
    /// previous batch's outcome is outstanding; its deltas wait.
    pub(crate) fn finish_drain(
        &mut self,
        drain: &SharedDrain,
    ) -> Result<Option<LightmapDrainBatch>, LightmapResidencyError> {
        if self.drain_outstanding {
            return Ok(None);
        }
        self.build_batch(drain.admitted_keys(StreamResource::LightmapBlock))
            .map(Some)
    }

    /// The batch carrying `admitted` ready pairs, plus the target reset or
    /// deltas. Marks the batch outstanding until [`Self::apply_outcome`].
    pub(super) fn build_batch(
        &mut self,
        admitted: impl IntoIterator<Item = u32>,
    ) -> Result<LightmapDrainBatch, LightmapResidencyError> {
        let mut batch = LightmapDrainBatch {
            generation: self.generation,
            content_tag: self.content_tag,
            pool_cap_layers: self.levers.pool_cap_layers(),
            ..LightmapDrainBatch::default()
        };
        if self.needs_target_reset {
            let mut reset = Vec::new();
            for (block, slot) in self.slots.iter_mut().enumerate() {
                let block = block as u32;
                slot.sent = slot.target.map(|target| target.wire(block));
                slot.pending_delta = false;
                reset.extend(slot.sent);
            }
            self.pending_delta.clear();
            batch.target_reset = Some(reset);
            self.needs_target_reset = false;
        } else {
            self.pending_delta.sort_unstable();
            for &block in &self.pending_delta {
                let slot = &mut self.slots[block as usize];
                slot.pending_delta = false;
                let now = slot.target.map(|target| target.wire(block));
                if now == slot.sent {
                    continue;
                }
                match now {
                    Some(target) => batch.target_set.push(target),
                    None => batch.target_remove.push(block),
                }
                slot.sent = now;
            }
            self.pending_delta.clear();
            self.counters.target_deltas +=
                (batch.target_set.len() + batch.target_remove.len()) as u64;
        }
        for block in admitted {
            let Some(prepared) = self.ready.remove(&block) else {
                return Err(LightmapResidencyError::InvalidDrainOutcome(format!(
                    "admitted block {block} is not ready"
                )));
            };
            self.slots[block as usize].phase = BlockPhase::InDrain;
            self.in_drain.push(block);
            batch.ready.push(prepared);
        }
        self.drain_outstanding = true;
        self.counters.drains += 1;
        Ok(batch)
    }

    /// Takes back ownership after the renderer consumed the outstanding
    /// batch. Every drained pair is installed, refused (band only), deferred,
    /// or failed exactly once; only resident, non-mandatory blocks may be
    /// evicted. The whole outcome is validated before any state changes.
    pub(crate) fn apply_outcome(
        &mut self,
        outcome: LightmapDrainOutcome,
    ) -> Result<(), LightmapResidencyError> {
        self.validate_outcome(&outcome)?;
        for &block in &outcome.evicted {
            self.residency.remove_resident(self.map.facts(block));
            self.slots[block as usize].phase = BlockPhase::Absent;
            self.counters.evictions += 1;
            self.requests_due = true;
        }
        for &block in &outcome.installed {
            self.residency.add_resident(self.map.facts(block));
            self.release_in_hand(block);
            self.slots[block as usize].phase = BlockPhase::Installed;
            self.counters.installs += 1;
        }
        for &block in &outcome.refused {
            self.release_in_hand(block);
            let slot = &mut self.slots[block as usize];
            slot.phase = BlockPhase::Refused;
            slot.refused_headroom = outcome.pool.band_headroom_texels;
            self.refused_blocks += 1;
            self.counters.refusals += 1;
        }
        for &block in &outcome.failed {
            // The payload could not fill its block: handled as a failed read.
            self.release_in_hand(block);
            self.counters.failed_installs += 1;
            let targeted = self.slots[block as usize].target.is_some();
            self.fail(
                block,
                targeted,
                "install",
                &"the renderer rejected its payload",
            );
        }
        for prepared in outcome.deferred {
            // A transient miss: the pair stays owned, and in hand, for a
            // later drain.
            self.slots[prepared.block as usize].phase = BlockPhase::Ready;
            self.ready.insert(prepared.block, prepared);
            self.counters.deferrals += 1;
        }
        let headroom_grew = outcome.pool.band_headroom_texels > self.pool.band_headroom_texels;
        self.pool = outcome.pool;
        if headroom_grew {
            self.requests_due = true;
            self.unrefuse_with_room(self.pool.band_headroom_texels);
        }
        self.in_drain.clear();
        self.drain_outstanding = false;
        Ok(())
    }

    /// The renderer failed the outstanding batch and rolled it back whole:
    /// no pair installed, no target changed, nothing evicted. Every drained
    /// pair returns to Absent, its buffers and permit released, to be read
    /// again; the next batch re-sends every target as a reset. Does nothing
    /// without an outstanding batch.
    pub(crate) fn abort_drain(&mut self) {
        if !self.drain_outstanding {
            return;
        }
        for index in 0..self.in_drain.len() {
            let block = self.in_drain[index];
            self.release_in_hand(block);
            self.slots[block as usize].phase = BlockPhase::Absent;
        }
        self.in_drain.clear();
        self.drain_outstanding = false;
        self.needs_target_reset = true;
        self.counters.aborted_drains += 1;
    }

    /// Refused band blocks become requestable again once the pool reports
    /// room for at least one more of their slots than it had when it refused
    /// them, so a refused block is not re-read on every small gain.
    fn unrefuse_with_room(&mut self, headroom: u64) {
        if self.refused_blocks == 0 {
            return;
        }
        for (block, slot) in self.slots.iter_mut().enumerate() {
            if slot.phase == BlockPhase::Refused
                && slot
                    .refused_headroom
                    .saturating_add(self.map.facts(block as u32).texels)
                    <= headroom
            {
                slot.phase = BlockPhase::Absent;
                self.refused_blocks -= 1;
            }
        }
    }

    fn validate_outcome(
        &mut self,
        outcome: &LightmapDrainOutcome,
    ) -> Result<(), LightmapResidencyError> {
        let invalid = |message: String| Err(LightmapResidencyError::InvalidDrainOutcome(message));
        if !self.drain_outstanding {
            return invalid("an outcome arrived with no batch outstanding".into());
        }
        let scratch = &mut self.outcome_scratch;
        scratch.clear();
        scratch.extend(
            outcome
                .installed
                .iter()
                .chain(&outcome.refused)
                .chain(&outcome.failed),
        );
        scratch.extend(outcome.deferred.iter().map(|prepared| prepared.block));
        scratch.sort_unstable();
        self.in_drain.sort_unstable();
        if *scratch != self.in_drain {
            return invalid(
                "installed, refused, deferred, and failed must account for every drained pair once"
                    .into(),
            );
        }
        for &block in &outcome.refused {
            if !matches!(
                self.slots[block as usize].sent,
                Some(LightmapTarget {
                    class: LightmapBlockClass::Band,
                    ..
                })
            ) {
                return invalid(format!(
                    "block {block} was refused but is not a band target"
                ));
            }
        }
        for prepared in &outcome.deferred {
            if prepared.generation != self.generation || prepared.content_tag != self.content_tag {
                return invalid(format!(
                    "deferred block {} does not match this generation",
                    prepared.block
                ));
            }
        }
        scratch.clear();
        scratch.extend(&outcome.evicted);
        scratch.sort_unstable();
        if scratch.windows(2).any(|pair| pair[0] == pair[1]) {
            return invalid("an evicted block repeats".into());
        }
        for &block in &outcome.evicted {
            let Some(slot) = self.slots.get(block as usize) else {
                return invalid(format!("evicted block {block} is out of range"));
            };
            if slot.phase != BlockPhase::Installed {
                return invalid(format!("evicted block {block} was not installed"));
            }
            if slot
                .sent
                .is_some_and(|target| target.class != LightmapBlockClass::Band)
            {
                return invalid(format!(
                    "block {block} is a mandatory or visible target and cannot be evicted"
                ));
            }
        }
        Ok(())
    }
}
