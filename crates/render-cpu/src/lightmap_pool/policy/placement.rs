// Per-drain planner: target deltas, frees, cap eviction, never-refused placement, band placement.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::cmp::Reverse;

use postretro_level_loader::{LightmapDrainBatch, LightmapTarget};

use super::plan::{BlockUpload, EvictionReason, PlannedEviction, PoolGrowth};
use super::{DrainPlan, DrainRequest, LightmapPoolModel, Target, placement_of};

impl LightmapPoolModel {
    /// Plan one drain from the controller's batch. Only the ready pairs'
    /// block ids are read, never their payloads.
    pub fn plan_batch(&mut self, batch: &LightmapDrainBatch) -> &DrainPlan {
        let mut ready = std::mem::take(&mut self.scratch.ready);
        ready.clear();
        ready.extend(batch.ready.iter().map(|prepared| prepared.block));
        self.plan_drain(DrainRequest {
            pool_cap_layers: batch.pool_cap_layers,
            target_reset: batch.target_reset.as_deref(),
            target_set: &batch.target_set,
            target_remove: &batch.target_remove,
            ready: &ready,
        });
        self.scratch.ready = ready;
        &self.plan
    }

    /// Plan one drain and apply it to the model.
    ///
    /// Frees blocks that left every target, then evicts band blocks at or
    /// past the cap (P8). Mandatory and visible pairs are never refused:
    /// with no room under the current layers they evict band blocks
    /// farthest lead first, then repack in place if every mandatory and
    /// visible block fits under the cap, then grow a new generation, or
    /// defer while one is still retiring. Band pairs fit under the cap or
    /// are refused; they never repack or grow the pool.
    pub fn plan_drain(&mut self, request: DrainRequest<'_>) -> &DrainPlan {
        self.drain += 1;
        self.journal.clear();
        self.touched.clear();
        self.plan.clear();
        self.scratch.victims_built = false;

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

    /// P8: band blocks never sit at or past the cap's layers. A lowered cap
    /// (or a reset) scans every resident block; otherwise only blocks whose
    /// class changed this drain can have become such a band block.
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
            repacked = self.repack(&list);
            if !repacked {
                self.place_with_growth(&list);
            }
        }
        self.scratch.never_refused = list;
        repacked
    }

    /// Place under the cap, then anywhere in the current layers, evicting
    /// band blocks farthest lead first until one of those fits.
    fn place_without_growth(&mut self, block: u32) -> bool {
        let (width, height) = self.alloc_extent(block);
        loop {
            let cap = self.cap_eff();
            let layers = self.layers;
            let slot = self
                .pool
                .allocate_within(width, height, Some(cap as usize))
                .or_else(|| {
                    (layers > cap)
                        .then(|| {
                            self.pool
                                .allocate_within(width, height, Some(layers as usize))
                        })
                        .flatten()
                });
            if let Some(slot) = slot {
                self.place(block, slot);
                return true;
            }
            let Some(victim) = self.next_victim() else {
                return false;
            };
            self.release(victim, EvictionReason::Pressure);
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

    /// The repack could not fit every mandatory and visible block under the
    /// cap: grow past it. One generation per drain, and none while the last
    /// one is still retiring; a pair that needs growth then is deferred.
    fn place_with_growth(&mut self, list: &[u32]) {
        let retiring = self.retiring;
        let start_layers = self.layers;
        for &block in list {
            if self.place_without_growth(block) {
                continue;
            }
            if retiring {
                self.plan.deferred.push(block);
                continue;
            }
            let (width, height) = self.alloc_extent(block);
            let slot = self
                .pool
                .allocate_within(width, height, None)
                .expect("an uncapped pool places any block that fits a layer");
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
            if touched.start.is_some() && !self.is_resident(touched.block) {
                self.plan.evicted.push(PlannedEviction {
                    block: touched.block,
                    reason: touched.reason.expect("an evicted block records why"),
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
