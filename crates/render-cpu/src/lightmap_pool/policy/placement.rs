// Per-drain planner: target deltas, frees, cap eviction, reserved room, each pool's repack.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolKind};
use postretro_level_loader::{LightmapDrainBatch, LightmapRefusal, LightmapTarget};

use super::journal::{HALVES, half_at};
use super::plan::{BlockUpload, EvictionReason, PlannedEviction, RepackKind};
use super::reservation::RepackScope;
use super::{DrainPlan, DrainRequest, LightmapPoolModel, Target};

impl LightmapPoolModel {
    /// Plan one drain from the controller's batch. Only the ready pairs'
    /// block ids are read, never their payloads. The batch's one cap bounds
    /// both pools.
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
                pool_cap_layers: PerPool::both(batch.pool_cap_layers),
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
    /// past a cap. Mandatory and visible pairs are never refused. When targets
    /// changed, every such target's items with no room reserve what is free in
    /// each pool. A pair lands whole: each half in its reserved room, or under
    /// its pool's cap, or anywhere in its current layers, evicting band blocks
    /// farthest lead first, then putting back each victim all of whose rects
    /// are still free. Failing that, each pool short of room repacks alone, in
    /// place, for its whole targeted demand: downward under the cap, then, in a
    /// pool already grown past the cap, downward within its layers, then by
    /// compaction into any layer, once per demand. A demand past a pool repacks
    /// for its pairs alone, and failing that grows a new generation of that
    /// pool sized to the whole targeted mandatory and visible demand with
    /// headroom. A pair that needs growth of a pool still retiring, or a layer
    /// past the device limit, defers whole and grows nothing. Band pairs fit
    /// under the caps, clear of reserved room, or are refused whole; they never
    /// repack or grow a pool.
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
        let caps_before = PerPool {
            lightmap: self.pools.lightmap.cap_eff(),
            mask: self.pools.mask.cap_eff(),
        };
        self.set_caps(request.pool_cap_layers);
        let cap_lowered = PoolKind::ALL
            .iter()
            .any(|&kind| self.pools[kind].cap_eff() < caps_before[kind]);
        let full_scan = request.target_reset.is_some() || cap_lowered;
        self.apply_targets(request);
        self.evict_band_over_cap(full_scan, request.target_set);
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

    /// Band blocks never sit at or past a cap's layers: a half past one takes
    /// the whole block out. A lowered cap (or a reset) scans every resident
    /// block; otherwise only blocks whose class changed this drain can have
    /// become such a band block.
    fn evict_band_over_cap(&mut self, full_scan: bool, target_set: &[LightmapTarget]) {
        let mut over = std::mem::take(&mut self.scratch.over_cap);
        over.clear();
        if full_scan {
            over.extend(
                self.pools
                    .lightmap
                    .resident
                    .iter()
                    .copied()
                    .filter(|&block| self.is_band_past_a_cap(block)),
            );
            over.sort_unstable();
        } else {
            over.extend(
                target_set
                    .iter()
                    .map(|t| t.block)
                    .filter(|&block| self.is_band_past_a_cap(block)),
            );
        }
        for &block in &over {
            self.release(block, EvictionReason::OverCap);
        }
        self.scratch.over_cap = over;
    }

    fn is_band_past_a_cap(&self, block: u32) -> bool {
        self.is_band(block)
            && self.halves(block).any(|(kind, item)| {
                let pool = &self.pools[kind];
                pool.slot(item).is_some_and(|s| s.layer >= pool.cap_eff())
            })
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
            for half in 0..HALVES {
                let (kind, item) = half_at(touched.block, half);
                if held[half] && !touched.held_at_start[half] {
                    self.plan.pools[kind].room_reserved.push(item);
                } else if touched.held_at_start[half] && !held[half] {
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
