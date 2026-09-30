// Per-drain planner: target deltas, frees, cap eviction, never-refused placement, band placement.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::cmp::Reverse;

use postretro_level_loader::{LightmapDrainBatch, LightmapTarget};

use super::plan::{BlockUpload, EvictionReason, PlannedEviction, PoolGrowth};
use super::{DrainPlan, DrainRequest, JournalOp, LightmapPoolModel, Slot, Target, placement_of};

impl LightmapPoolModel {
    /// Plan one drain from the controller's batch. Only the ready pairs'
    /// block ids are read, never their payloads.
    pub fn plan_batch(&mut self, batch: &LightmapDrainBatch) -> &DrainPlan {
        self.plan_batch_inner(batch, false)
    }

    /// [`plan_batch`](Self::plan_batch) for the first batch of a new
    /// generation while the model still holds an earlier one's residency. A
    /// new generation comes from a new controller, which holds nothing
    /// resident: the drain first frees every placement, and its table writes
    /// turn those entries non-resident. The frees are not reported evicted,
    /// since the new controller never saw those blocks resident. The layers,
    /// the textures and any retiring generation stand.
    pub fn plan_batch_from_empty(&mut self, batch: &LightmapDrainBatch) -> &DrainPlan {
        self.plan_batch_inner(batch, true)
    }

    fn plan_batch_inner(&mut self, batch: &LightmapDrainBatch, from_empty: bool) -> &DrainPlan {
        let mut ready = std::mem::take(&mut self.scratch.ready);
        ready.clear();
        ready.extend(batch.ready.iter().map(|prepared| prepared.block));
        self.plan_drain_inner(
            DrainRequest {
                pool_cap_layers: batch.pool_cap_layers,
                target_reset: batch.target_reset.as_deref(),
                target_set: &batch.target_set,
                target_remove: &batch.target_remove,
                ready: &ready,
            },
            from_empty,
        );
        self.scratch.ready = ready;
        &self.plan
    }

    /// Plan one drain and apply it to the model.
    ///
    /// Frees blocks that left every target, then evicts band blocks at or
    /// past the cap. Mandatory and visible pairs are never refused. With no
    /// room under the current layers, a pair evicts band blocks farthest
    /// lead first, then puts back each victim whose rect is still free. Failing
    /// that, the drain repacks in place: under the cap, then, in a pool
    /// already grown past the cap, within its layers. Failing that, it grows
    /// a new generation. A pair that needs growth while a generation still
    /// retires, or a layer past the device limit, is deferred. Band pairs
    /// fit under the cap or are refused; they never repack or grow the pool.
    pub fn plan_drain(&mut self, request: DrainRequest<'_>) -> &DrainPlan {
        self.plan_drain_inner(request, false)
    }

    fn plan_drain_inner(&mut self, request: DrainRequest<'_>, from_empty: bool) -> &DrainPlan {
        self.drain += 1;
        self.journal.clear();
        self.touched.clear();
        self.plan.clear();
        self.scratch.victims_built = false;

        if from_empty {
            self.forget_residency();
        }
        let cap_before = self.cap_eff();
        self.set_cap(request.pool_cap_layers);
        let full_scan = request.target_reset.is_some() || self.cap_eff() < cap_before;
        self.apply_targets(request);
        self.evict_band_over_cap(full_scan, request.target_set);
        self.classify_ready(request.ready);
        let repacked = self.place_never_refused();
        self.place_band();
        self.finish(repacked);
        &self.plan
    }

    fn apply_targets(&mut self, request: DrainRequest<'_>) {
        let to_target = |t: &LightmapTarget| Target {
            class: t.class,
            lead: t.lead,
        };
        if let Some(reset) = request.target_reset {
            let mut next = reset.iter().peekable();
            for block in 0..self.block_count() {
                let target = next.next_if(|t| t.block == block).map(to_target);
                self.set_target(block, target);
            }
        }
        for t in request.target_set {
            self.set_target(t.block, Some(to_target(t)));
        }
        for &block in request.target_remove {
            self.set_target(block, None);
        }
    }

    /// Band blocks never sit at or past the cap's layers. A lowered cap (or
    /// a reset) scans every resident block; otherwise only blocks whose class
    /// changed this drain can have become such a band block.
    fn evict_band_over_cap(&mut self, full_scan: bool, target_set: &[LightmapTarget]) {
        let cap = self.cap_eff();
        let mut over = std::mem::take(&mut self.scratch.over_cap);
        over.clear();
        let over_cap = |model: &Self, block: u32| {
            model.is_band(block) && model.slots[block as usize].is_some_and(|s| s.layer >= cap)
        };
        if full_scan {
            over.extend(
                self.resident
                    .iter()
                    .copied()
                    .filter(|&block| over_cap(self, block)),
            );
            over.sort_unstable();
        } else {
            over.extend(
                target_set
                    .iter()
                    .map(|t| t.block)
                    .filter(|&block| over_cap(self, block)),
            );
        }
        for &block in &over {
            self.release(block, EvictionReason::OverCap);
        }
        self.scratch.over_cap = over;
    }

    fn classify_ready(&mut self, ready: &[u32]) {
        let mut never_refused = std::mem::take(&mut self.scratch.never_refused);
        let mut band = std::mem::take(&mut self.scratch.band_ready);
        never_refused.clear();
        band.clear();
        for &block in ready {
            match self.target(block) {
                None => self.plan.refused.push(block),
                Some(_) if self.is_resident(block) => self.plan.installed.push(block),
                Some(target) if target.is_band() => band.push(block),
                Some(_) => never_refused.push(block),
            }
        }
        // Tallest first, as shelf packing prefers (the dry run's order).
        never_refused.sort_unstable_by_key(|&block| {
            let (width, height) = self.alloc_extent(block);
            (Reverse(height), Reverse(width), block)
        });
        band.sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
        self.scratch.never_refused = never_refused;
        self.scratch.band_ready = band;
    }

    /// Returns whether the drain repacked.
    fn place_never_refused(&mut self) -> bool {
        let list = std::mem::take(&mut self.scratch.never_refused);
        let checkpoint = self.journal.len();
        let mut repacked = false;
        if !list.iter().all(|&block| self.place_without_growth(block)) {
            // Start over from the post-free state: a repack moves band blocks
            // evicted above as victims too, since their texels are still in
            // the pool until this plan executes.
            self.rollback_to(checkpoint);
            let cap = self.cap_eff();
            repacked =
                self.repack(&list, cap) || (self.layers > cap && self.repack(&list, self.layers));
            if !repacked {
                self.place_with_growth(&list);
            }
        }
        self.scratch.never_refused = list;
        repacked
    }

    /// Place under the cap, then anywhere in the current layers, evicting
    /// band blocks farthest lead first until one of those fits. Evictions
    /// stand only when the block ends up placed, and then only for victims
    /// that cannot go back where they were: a failed attempt restores every
    /// victim, so a pair that then grows or defers costs the band nothing.
    fn place_without_growth(&mut self, block: u32) -> bool {
        let (width, height) = self.alloc_extent(block);
        let checkpoint = self.journal.len();
        loop {
            if let Some(slot) = self.allocate_in_layers(width, height) {
                self.place(block, slot);
                self.reinstate_spare_victims(checkpoint);
                return true;
            }
            let Some(victim) = self.next_victim() else {
                self.rollback_to(checkpoint);
                return false;
            };
            self.release(victim, EvictionReason::Pressure);
        }
    }

    /// First fit under the cap, then within the current layers.
    fn allocate_in_layers(&mut self, width: u32, height: u32) -> Option<Slot> {
        let cap = self.cap_eff();
        let under_cap = self.pool.allocate_within(width, height, Some(cap as usize));
        if under_cap.is_some() || self.layers <= cap {
            return under_cap;
        }
        let layers = self.layers as usize;
        self.pool.allocate_within(width, height, Some(layers))
    }

    /// After a placement closes the journal, put back each victim evicted
    /// before the last one whose rect is still free: farthest-lead-first
    /// order can evict blocks the final slot never touched. The last victim
    /// is the one that made room. A reinstated victim returns to the top of
    /// the victim stack in lead order, so the next pair still evicts
    /// farthest first.
    fn reinstate_spare_victims(&mut self, checkpoint: usize) {
        let placed_at = self.journal.len() - 1;
        if placed_at <= checkpoint + 1 {
            return;
        }
        for at in (checkpoint..placed_at - 1).rev() {
            let JournalOp::Free { block, slot } = self.journal[at] else {
                continue;
            };
            if self.pool.restore(slot).is_ok() {
                self.place(block, slot);
                self.scratch.victims.push(block);
            }
        }
    }

    fn next_victim(&mut self) -> Option<u32> {
        if !self.scratch.victims_built {
            let mut victims = std::mem::take(&mut self.scratch.victims);
            victims.clear();
            victims.extend(
                self.resident
                    .iter()
                    .copied()
                    .filter(|&block| self.is_band(block)),
            );
            victims
                .sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
            self.scratch.victims = victims;
            self.scratch.victims_built = true;
        }
        while let Some(block) = self.scratch.victims.pop() {
            if self.is_resident(block) && self.is_band(block) {
                return Some(block);
            }
        }
        None
    }

    /// No repack could fit every mandatory and visible block: grow past the
    /// current layers. One generation per drain, none while the last one is
    /// still retiring, and never past the device's layer limit; a pair that
    /// needs growth then is deferred.
    ///
    /// Once one pair is deferred at the device limit, the pool cannot grow
    /// again this drain, so the pairs after it skip the victim walk: each
    /// takes free space it fits as is, or is deferred. Without the skip, a
    /// level past the device limit repeats a full walk per pair on every
    /// drain, since its deferred pairs come back ready. A later drain walks
    /// again, so a changed resident set still gets its chance.
    fn place_with_growth(&mut self, list: &[u32]) {
        let retiring = self.retiring;
        let start_layers = self.layers;
        for &block in list {
            let (width, height) = self.alloc_extent(block);
            if self.plan.device_limited {
                match self.allocate_in_layers(width, height) {
                    Some(slot) => self.place(block, slot),
                    None => self.plan.deferred.push(block),
                }
                continue;
            }
            if self.place_without_growth(block) {
                continue;
            }
            if retiring {
                self.plan.deferred.push(block);
                continue;
            }
            let limit = Some(self.max_layers as usize);
            let Some(slot) = self.pool.allocate_within(width, height, limit) else {
                self.plan.deferred.push(block);
                self.plan.device_limited = true;
                continue;
            };
            self.place(block, slot);
            if slot.layer >= self.layers {
                if self.layers == start_layers {
                    self.record_growth(start_layers);
                }
                self.layers = slot.layer + 1;
            }
        }
        if self.layers > start_layers {
            self.plan.growth = Some(PoolGrowth {
                from_layers: start_layers,
                to_layers: self.layers,
            });
        }
    }

    fn place_band(&mut self) {
        let list = std::mem::take(&mut self.scratch.band_ready);
        let cap = self.cap_eff() as usize;
        for &block in &list {
            let (width, height) = self.alloc_extent(block);
            match self.pool.allocate_within(width, height, Some(cap)) {
                Some(slot) => self.place(block, slot),
                None => self.plan.refused.push(block),
            }
        }
        self.scratch.band_ready = list;
    }

    fn finish(&mut self, repacked: bool) {
        for list in [&self.scratch.never_refused, &self.scratch.band_ready] {
            for &block in list {
                let Some(slot) = self.slots[block as usize] else {
                    continue;
                };
                let (width, height) = self.extents[block as usize];
                self.plan.uploads.push(BlockUpload {
                    block,
                    placement: placement_of(slot),
                    width,
                    height,
                });
                self.plan.installed.push(block);
            }
        }
        for touched in &self.touched {
            if let Some(reason) = touched.reason
                && !touched.forgotten
                && touched.start.is_some()
                && !self.is_resident(touched.block)
            {
                self.plan.evicted.push(PlannedEviction {
                    block: touched.block,
                    reason,
                });
            }
        }
        if repacked {
            self.schedule_repack_copies();
        }
        self.rebuild_table_writes();
        self.plan.installed.sort_unstable();
        self.plan.refused.sort_unstable();
        self.plan.deferred.sort_unstable();
        self.plan
            .evicted
            .sort_unstable_by_key(|eviction| eviction.block);
        self.plan.report = self.report(repacked, self.plan.growth.is_some());
    }
}
