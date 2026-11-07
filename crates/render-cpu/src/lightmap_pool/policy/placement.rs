// Per-drain planner: target deltas, frees, reserved room, each pool's repack.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolKind};
use postretro_level_loader::{LightmapDrainBatch, LightmapRefusal, LightmapTarget};

use super::journal::half_at;
use super::plan::{BlockUpload, PlannedEviction, RepackKind};
use super::reservation::RepackScope;
use super::{DrainPlan, DrainRequest, LightmapPoolModel, Target};

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
    /// the textures and any retiring generation stand, in both pools.
    pub fn plan_batch_from_empty(&mut self, batch: &LightmapDrainBatch) -> &DrainPlan {
        self.plan_batch_inner(batch, true)
    }

    fn plan_batch_inner(&mut self, batch: &LightmapDrainBatch, from_empty: bool) -> &DrainPlan {
        let mut ready = std::mem::take(&mut self.scratch.ready);
        ready.clear();
        ready.extend(batch.ready.iter().map(|prepared| prepared.block));
        self.plan_drain_inner(
            DrainRequest {
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
    /// Frees blocks that left every target. Mandatory and visible pairs are
    /// never refused. When targets changed, every such target's items with no
    /// room reserve what is free in each pool. A pair lands whole: each half
    /// in its reserved room or anywhere in its pool's layers, evicting band
    /// blocks farthest lead first, then putting back each victim all of whose
    /// rects are still free. Failing that, each pool short of room repacks
    /// alone, in place, for its whole targeted demand: downward, then by
    /// compaction into any layer, once per demand. A demand past a pool
    /// repacks for its pairs alone, and failing that grows a new generation of
    /// that pool sized to the whole targeted mandatory and visible demand with
    /// the shared margin. A pair that needs growth of a pool still retiring,
    /// or a layer past the device limit, defers whole and grows nothing. Band
    /// pairs take what room is free, clear of reserved room, or are refused
    /// whole; they never repack or grow a pool.
    pub fn plan_drain(&mut self, request: DrainRequest<'_>) -> &DrainPlan {
        self.plan_drain_inner(request, false)
    }

    fn plan_drain_inner(&mut self, request: DrainRequest<'_>, from_empty: bool) -> &DrainPlan {
        self.drain += 1;
        self.journal.clear();
        self.touched.clear();
        self.plan.clear();
        self.scratch.victims_built = false;
        self.scratch.demand_changed = false;

        if from_empty {
            self.forget_residency();
        }
        self.apply_targets(request);
        self.classify_ready(request.ready);
        if self.scratch.demand_changed {
            #[cfg(test)]
            {
                self.demand_passes += 1;
            }
            for kind in PoolKind::ALL {
                self.reopen_compaction(kind);
                self.hold_room_for_demand(kind);
            }
        }
        self.place_never_refused();
        self.place_band();
        self.finish();
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

    fn classify_ready(&mut self, ready: &[u32]) {
        let mut never_refused = std::mem::take(&mut self.scratch.never_refused);
        let mut band = std::mem::take(&mut self.scratch.band_ready);
        never_refused.clear();
        band.clear();
        for &block in ready {
            match self.target(block) {
                None => {
                    let short = PerPool {
                        lightmap: true,
                        mask: self.has_half_in(PoolKind::Mask, block),
                    };
                    self.plan.refused.push(LightmapRefusal { block, short });
                }
                Some(_) if self.is_resident(block) => self.plan.installed.push(block),
                Some(target) if target.is_band() => band.push(block),
                Some(_) => {
                    self.pair_stamp[block as usize] = self.drain;
                    never_refused.push(block);
                }
            }
        }
        // Tallest first, as shelf packing prefers (the dry run's order).
        self.sort_tallest_first(PoolKind::Lightmap, &mut never_refused);
        band.sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
        self.scratch.never_refused = never_refused;
        self.scratch.band_ready = band;
    }

    fn place_never_refused(&mut self) {
        let list = std::mem::take(&mut self.scratch.never_refused);
        let checkpoint = self.journal.len();
        if !list
            .iter()
            .all(|&block| self.place_without_growth(block, None))
        {
            // Start over from the post-free state: a repack moves band blocks
            // evicted above as victims too, since their texels are still in
            // the pool until this plan executes.
            let short = self.scratch.short;
            self.rollback_to(checkpoint);
            if !self.pairs_failed_last_drain(&list) {
                let short = PerPool {
                    lightmap: short.lightmap || !self.seats_alone(PoolKind::Lightmap, &list),
                    mask: short.mask || !self.seats_alone(PoolKind::Mask, &list),
                };
                for kind in PoolKind::ALL {
                    if short[kind] {
                        self.plan.pools[kind].repack = self.repack_for(kind, &list);
                    }
                }
            }
            self.place_with_growth(&list);
            for kind in PoolKind::ALL {
                if self.plan.pools[kind].growth.is_some() {
                    self.hold_room_for_demand(kind);
                }
            }
        }
        self.scratch.never_refused = list;
    }

    /// Whether `list`'s halves in `kind`'s pool fit, band evictions included,
    /// with the other pool left out of it. Changes nothing.
    fn seats_alone(&mut self, kind: PoolKind, list: &[u32]) -> bool {
        let checkpoint = self.journal.len();
        let seats = list
            .iter()
            .all(|&block| self.place_without_growth(block, Some(kind)));
        self.rollback_to(checkpoint);
        seats
    }

    /// These pairs were deferred last drain and nothing has made room since,
    /// so every repack would fail as it did then. A jam behind a retiring
    /// generation or the device limit costs a drain no repack and no
    /// reservation work.
    fn pairs_failed_last_drain(&self, list: &[u32]) -> bool {
        !self.scratch.demand_changed
            && list
                .iter()
                .all(|&block| self.deferred_stamp[block as usize] == self.drain - 1)
    }

    /// Whole demand first, downward and then by compaction, so what repacks
    /// here does not repack again as the demand's pairs arrive. Past the pool,
    /// the drain's pairs still repack rather than grow. Each pool repacks on
    /// its own items alone: `list`'s items take reserved room, and the pairs
    /// land whole once every pool has had its turn.
    fn repack_for(&mut self, kind: PoolKind, list: &[u32]) -> Option<RepackKind> {
        let mut pending = std::mem::take(&mut self.scratch.repack_pending);
        pending.clear();
        for &block in list {
            pending.extend(
                self.halves(block)
                    .filter(|&(half, _)| half == kind)
                    .map(|(_, item)| item),
            );
        }
        let repack = self.repack_with(kind, &pending);
        self.scratch.repack_pending = pending;
        repack
    }

    fn repack_with(&mut self, kind: PoolKind, pending: &[u32]) -> Option<RepackKind> {
        if !self.pools[kind].room.compaction_spent {
            if self.repack_downward(kind, pending, RepackScope::WholeDemand) {
                return Some(RepackKind::Downward);
            }
            if let Some(stats) = self.compact(kind, pending) {
                return Some(RepackKind::Compaction(stats));
            }
        }
        self.repack_downward(kind, pending, RepackScope::DrainPairs)
            .then_some(RepackKind::Downward)
    }

    fn finish(&mut self) {
        for list in [&self.scratch.never_refused, &self.scratch.band_ready] {
            for &block in list {
                let Some(placement) = self.placement(block) else {
                    continue;
                };
                let (width, height) = self.demand.extents()[block as usize];
                self.plan.uploads.push(BlockUpload {
                    block,
                    placement,
                    planes: [
                        self.plane_placement(block, 0),
                        self.plane_placement(block, 1),
                    ],
                    width,
                    height,
                });
                self.plan.installed.push(block);
            }
        }
        for at in 0..self.touched.len() {
            let touched = self.touched[at];
            if let Some(reason) = touched.reason
                && !touched.forgotten
                && touched.start[0].is_some()
                && !self.is_resident(touched.block)
            {
                self.plan.evicted.push(PlannedEviction {
                    block: touched.block,
                    reason,
                });
            }
            let held = self.held_halves(touched.block);
            for (half, &holds) in held.iter().enumerate() {
                let (kind, item) = half_at(touched.block, half);
                if holds && !touched.held_at_start[half] {
                    self.plan.pools[kind].room_reserved.push(item);
                } else if touched.held_at_start[half] && !holds {
                    self.plan.pools[kind].room_returned.push(item);
                }
            }
        }
        for &block in &self.plan.deferred {
            self.deferred_stamp[block as usize] = self.drain;
        }
        for kind in PoolKind::ALL {
            // A compaction already planned its copies, before it could commit.
            if self.plan.pools[kind].repack == Some(RepackKind::Downward) {
                self.schedule_repack_copies(kind);
            }
            let plan = &mut self.plan.pools[kind];
            plan.room_reserved.sort_unstable();
            plan.room_returned.sort_unstable();
        }
        self.rebuild_table_writes();
        self.plan.installed.sort_unstable();
        self.plan.refused.sort_unstable_by_key(|r| r.block);
        self.plan.deferred.sort_unstable();
        self.plan
            .evicted
            .sort_unstable_by_key(|eviction| eviction.block);
        self.plan.report = self.report(self.plan.repacked(), self.plan.grew());
    }
}
