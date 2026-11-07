//! The renderer's lightmap drain without the GPU, for the residency walks.
//! See: context/lib/rendering_pipeline.md §4

// The real `LightmapPoolModel` plans every batch as the renderer's lightmap
// stream drain does, and the outcome is built the same way. Each pool takes its
// first generation from the plan the runtime makes, and grows and retires
// alone. Texture work is not executed; its size is counted from the plan. The
// model has no device layer limit, so every deferral waits behind a retiring
// generation (O18).

use std::time::Instant;

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockPayload, LightmapHeader,
};
use postretro_level_format::lightmap_pool::{
    MASK_POOL_LAYER_BYTES, lightmap_pool_layer_bytes, streamed_pool_bytes,
};
use postretro_level_loader::{
    LevelWorld, LightmapDrainBatch, LightmapDrainOutcome, LightmapStreamManifest,
};
use postretro_render_cpu::lightmap_pool::{
    LightmapPoolModel, PerPool, PoolDemand, PoolKind, PoolPlan, RepackKind, StaticPool,
    StreamedPoolSizing, payload_mismatch, plan_streaming_pool,
};

/// Ticks a grown-out generation stays alive before its submitted work is
/// reported done. The renderer polls an `on_submitted_work_done` flag at
/// drain start; two frames in flight is the stand-in.
const RETIRE_AFTER_TICKS: u32 = 2;

/// What advances a retiring generation toward release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetireClock {
    /// Each drain call, at its start: the real-time mode's one drain a frame.
    Drains,
    /// Each frame, at [`PoolMirror::begin_frame`], however many drains the
    /// frame runs (O17): a second drain never brings release forward.
    Frames,
}

/// One pool's share of a run.
#[derive(Debug, Default, Clone)]
pub(super) struct PoolRun {
    pub(super) repacks: u64,
    pub(super) repack_copies: u64,
    /// Repacks that escalated to compaction, and those that found no order.
    pub(super) compactions: u64,
    pub(super) compaction_rollbacks: u64,
    /// Hops into free space, and target layers built over a layer that would
    /// not empty.
    pub(super) parks: u64,
    pub(super) built_over: u64,
    pub(super) compaction_copies: u64,
    /// Copy bytes of each compaction, spare hops included.
    pub(super) compaction_copy_bytes: Vec<u64>,
    /// Copy bytes of each downward repack.
    pub(super) downward_copy_bytes: Vec<u64>,
    /// CPU time of the model's planning on each drain that compacted, nanoseconds.
    pub(super) compaction_plan_nanos: Vec<u64>,
    pub(super) largest_compaction_moves: u32,
    pub(super) growths: u64,
    pub(super) first_layers: u32,
    /// The first generation's bytes, spare layer included.
    pub(super) first_bytes: u64,
    pub(super) peak_layers: u32,
    pub(super) peak_occupied_layers: u32,
}

impl PoolRun {
    fn record_repack(&mut self, plan: &PoolPlan, layer_bytes: u64, plan_nanos: u64) {
        if plan.repack.is_some() {
            self.repacks += 1;
            self.repack_copies += plan.copies.len() as u64;
        }
        self.compaction_rollbacks += u64::from(plan.compaction_rolled_back);
        let copied_texels: u64 = plan
            .copies
            .iter()
            .map(|copy| u64::from(copy.width) * u64::from(copy.height))
            .sum();
        let copied_bytes = copied_texels * layer_bytes / u64::from(LIGHTMAP_POOL_LAYER_EDGE).pow(2);
        match plan.repack {
            Some(RepackKind::Compaction(order)) => {
                self.compactions += 1;
                self.parks += u64::from(order.parked);
                self.built_over += u64::from(order.built_over);
                self.compaction_copies += plan.copies.len() as u64;
                self.largest_compaction_moves =
                    self.largest_compaction_moves.max(order.moved_blocks);
                self.compaction_plan_nanos.push(plan_nanos);
                self.compaction_copy_bytes.push(copied_bytes);
            }
            Some(RepackKind::Downward) => self.downward_copy_bytes.push(copied_bytes),
            None => {}
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct PoolStats {
    pub(super) drains: u64,
    pub(super) submissions: u64,
    pub(super) uploads: u64,
    pub(super) install_bytes: u64,
    pub(super) deferred: u64,
    pub(super) evictions: u64,
    pub(super) refusals: u64,
    pub(super) failed: u64,
    /// Both pools' active plus retiring bytes, spare layers included, at their
    /// peak.
    pub(super) peak_pool_bytes: u64,
    /// Largest active-plus-retiring bytes both pools held at once after a growth.
    pub(super) growth_transient_peak_bytes: u64,
    /// CPU time of the model's planning per submitted drain, nanoseconds.
    pub(super) plan_nanos: Vec<u64>,
    /// Payload bytes each submitted drain uploaded.
    pub(super) drain_install_bytes: Vec<u64>,
    /// The settled-entry preload's one drain: pairs uploaded and their bytes.
    pub(super) preload_uploads: u64,
    pub(super) preload_install_bytes: u64,
    pub(super) pools: PerPool<PoolRun>,
}

impl PoolStats {
    /// Growths of either pool.
    pub(super) fn growths(&self) -> u64 {
        PoolKind::ALL
            .iter()
            .map(|&kind| self.pools[kind].growths)
            .sum()
    }
}

/// The level's real texel layout, which every planned upload's payload must
/// fill.
#[derive(Debug, Clone, Copy)]
struct PayloadLayout {
    header: LightmapHeader,
}

/// One pool's grown-out generation, spare layer included.
#[derive(Debug, Clone, Copy)]
struct RetiringGeneration {
    bytes: u64,
    ticks_left: u32,
}

pub(super) struct PoolMirror {
    model: LightmapPoolModel,
    /// `None` for a test source, whose payloads carry no texel layout.
    layout: Option<PayloadLayout>,
    content_tag: [u8; 32],
    /// Bytes of one array layer of each pool.
    layer_bytes: PerPool<u64>,
    clock: RetireClock,
    /// Each pool's grown-out generation: a pool grows and retires alone.
    retiring: PerPool<Option<RetiringGeneration>>,
    /// Frames begun; 0 before the first, so a preload drain marks nothing.
    frame: u64,
    /// Per block: the frame whose latest drain carrying it deferred it, else
    /// 0. A later drain that carries it and places it clears the mark.
    deferred_on: Vec<u64>,
    /// Per block: its texel area, width × height, the bake-gap gate's unit.
    block_texels: Vec<u64>,
    pub(super) stats: PoolStats,
}

/// Each block's texel area from its extent.
fn texel_areas(extents: &[(u32, u32)]) -> Vec<u64> {
    extents
        .iter()
        .map(|&(width, height)| u64::from(width) * u64::from(height))
        .collect()
}

/// How the runtime sizes `world`'s pools, each `pool_layers` figure standing in
/// for that pool's first generation: the seam a capture scene's override takes.
pub(super) fn pool_sizing(
    world: &LevelWorld,
    pool_layers: PerPool<Option<u32>>,
) -> StreamedPoolSizing {
    let streaming = crate::render::level_world_to_geometry(world, &[])
        .lightmap_streaming
        .expect("the level streams its lightmap");
    StreamedPoolSizing {
        first_generation_override: pool_layers,
        ..streaming.pool_sizing
    }
}

impl PoolMirror {
    /// The model over the plan the runtime makes for `manifest`, at no device
    /// limit, so a walk starts from the first generation the level allocates.
    pub(super) fn new(
        manifest: &LightmapStreamManifest,
        sizing: StreamedPoolSizing,
        clock: RetireClock,
    ) -> Self {
        let index = manifest.lightmap_index();
        let header = index.header;
        let StaticPool::Streaming(plan) = plan_streaming_pool(
            Some(index),
            manifest.shadowmask_index(),
            manifest.content_tag(),
            sizing,
            LIGHTMAP_POOL_LAYER_EDGE,
            u32::MAX,
        ) else {
            panic!("the loader checked that the level's blocks stream");
        };
        let block_texels = texel_areas(plan.demand.extents());
        let model = LightmapPoolModel::with_layer_limit(
            plan.demand,
            LIGHTMAP_POOL_LAYER_EDGE,
            plan.first_generation_layers,
            plan.max_array_layers.saturating_sub(1),
        )
        .expect("the plan checked every block fits a pool layer");
        let layer_bytes = PerPool {
            lightmap: lightmap_pool_layer_bytes(&header).expect("a streamed level's formats"),
            mask: MASK_POOL_LAYER_BYTES,
        };
        Self::with_model(
            model,
            block_texels,
            Some(PayloadLayout { header }),
            manifest.content_tag(),
            layer_bytes,
            clock,
        )
    }

    /// A mirror over a test source's blocks and planes in layers of `edge`²
    /// texels, each pool starting at `first_generation`. Payloads are not
    /// checked against a texel layout.
    #[cfg(test)]
    pub(super) fn for_test_source(
        source: &dyn crate::lightmap_streaming::source::LightmapBlockSource,
        edge: u32,
        first_generation: PerPool<u32>,
        clock: RetireClock,
    ) -> Self {
        let summaries: Vec<_> = (0..source.block_count())
            .map(|block| source.block_summary(block).expect("block in range"))
            .collect();
        let extents: Vec<(u32, u32)> = summaries
            .iter()
            .map(|summary| (u32::from(summary.width), u32::from(summary.height)))
            .collect();
        let block_texels = texel_areas(&extents);
        let planes = summaries.iter().map(|summary| summary.planes).collect();
        let demand = PoolDemand::new(source.block_alignment(), extents, Some(planes));
        let model = LightmapPoolModel::new(demand, edge, first_generation)
            .expect("test blocks fit a layer");
        Self::with_model(
            model,
            block_texels,
            None,
            source.content_tag(),
            PerPool::default(),
            clock,
        )
    }

    fn with_model(
        model: LightmapPoolModel,
        block_texels: Vec<u64>,
        layout: Option<PayloadLayout>,
        content_tag: [u8; 32],
        layer_bytes: PerPool<u64>,
        clock: RetireClock,
    ) -> Self {
        let blocks = model.block_count() as usize;
        let pools = PerPool::from_fn(|kind| {
            let first_layers = model.pool(kind).layers();
            PoolRun {
                first_layers,
                first_bytes: streamed_pool_bytes(first_layers, layer_bytes[kind]),
                peak_layers: first_layers,
                ..PoolRun::default()
            }
        });
        let mut mirror = Self {
            model,
            layout,
            content_tag,
            layer_bytes,
            clock,
            retiring: PerPool::default(),
            frame: 0,
            deferred_on: vec![0; blocks],
            block_texels,
            stats: PoolStats {
                pools,
                ..PoolStats::default()
            },
        };
        mirror.stats.peak_pool_bytes = mirror.active_bytes();
        mirror
    }

    pub(super) fn model(&self) -> &LightmapPoolModel {
        &self.model
    }

    pub(super) fn layer_bytes(&self) -> PerPool<u64> {
        self.layer_bytes
    }

    /// Both pools' active generations, spare layers included.
    pub(super) fn active_bytes(&self) -> u64 {
        PoolKind::ALL
            .iter()
            .map(|&kind| {
                streamed_pool_bytes(self.model.pool(kind).layers(), self.layer_bytes[kind])
            })
            .sum()
    }

    /// Both pools' retiring generations.
    fn retiring_bytes(&self) -> u64 {
        PoolKind::ALL
            .iter()
            .filter_map(|&kind| self.retiring[kind])
            .map(|generation| generation.bytes)
            .sum()
    }

    /// Moves the settled-entry preload's drain out of the per-drain figures, so they
    /// describe in-play drains only. Pool shape and peaks carry over.
    pub(super) fn begin_play(&mut self) {
        let s = &mut self.stats;
        s.preload_uploads = s.uploads;
        s.preload_install_bytes = s.install_bytes;
        s.drains = 0;
        s.submissions = 0;
        s.uploads = 0;
        s.install_bytes = 0;
        s.plan_nanos.clear();
        s.drain_install_bytes.clear();
    }

    /// Starts a frame: under [`RetireClock::Frames`], advances each retiring
    /// generation once. Deferral marks from earlier frames lapse.
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
        if self.clock == RetireClock::Frames {
            self.tick_retirement();
        }
    }

    /// Whether either pool's grown-out generation is still retiring.
    pub(super) fn retiring(&self) -> bool {
        PoolKind::ALL
            .iter()
            .any(|&kind| self.retiring[kind].is_some())
    }

    /// `block`'s texel area, width × height.
    pub(super) fn block_texels(&self, block: u32) -> u64 {
        self.block_texels[block as usize]
    }

    /// Whether this frame's latest drain carrying `block` deferred it.
    pub(super) fn deferred_this_frame(&self, block: u32) -> bool {
        self.frame != 0 && self.deferred_on[block as usize] == self.frame
    }

    /// As the renderer's drain start: each pool releases its own generation
    /// once its work is done, and the other pool's stands.
    fn tick_retirement(&mut self) {
        for kind in PoolKind::ALL {
            match &mut self.retiring[kind] {
                Some(generation) if generation.ticks_left == 0 => {
                    self.model.release_retirement(kind);
                    self.retiring[kind] = None;
                }
                Some(generation) => generation.ticks_left -= 1,
                None => {}
            }
        }
    }

    pub(super) fn drain(&mut self, batch: LightmapDrainBatch) -> LightmapDrainOutcome {
        batch
            .validate_contract(self.model.block_count(), self.content_tag)
            .expect("the controller's batch honours the drain contract");
        if self.clock == RetireClock::Drains {
            self.tick_retirement();
        }

        let started = Instant::now();
        self.model.plan_batch(&batch);
        let mut failed = Vec::new();
        for upload in &self.model.plan().uploads {
            let fits = batch
                .ready
                .iter()
                .find(|p| p.block == upload.block)
                .is_some_and(|p| self.payload_fits(upload.block, &p.payload));
            if !fits {
                failed.push(upload.block);
            }
        }
        for &block in &failed {
            self.model
                .fail_install(block)
                .expect("a failed block is one of this drain's planned uploads");
        }
        let plan_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);

        let plan = self.model.plan();
        debug_assert!(
            !plan.pools.lightmap.device_limited && !plan.pools.mask.device_limited,
            "the mirror's model has no device layer limit"
        );
        for prepared in &batch.ready {
            self.deferred_on[prepared.block as usize] = 0;
        }
        for &block in &plan.deferred {
            self.deferred_on[block as usize] = self.frame;
        }
        let submitted = !plan.uploads.is_empty()
            || !plan.table_writes.is_empty()
            || PoolKind::ALL.iter().any(|&kind| {
                plan.pools[kind].growth.is_some() || !plan.pools[kind].copies.is_empty()
            });
        let drain_install_bytes: u64 = plan
            .uploads
            .iter()
            .filter_map(|u| batch.ready.iter().find(|p| p.block == u.block))
            .map(|p| payload_bytes(&p.payload))
            .sum();
        let s = &mut self.stats;
        s.drains += 1;
        if submitted {
            s.submissions += 1;
            s.plan_nanos.push(plan_nanos);
            s.drain_install_bytes.push(drain_install_bytes);
        }
        s.uploads += plan.uploads.len() as u64;
        s.install_bytes += drain_install_bytes;
        s.deferred += plan.deferred.len() as u64;
        s.evictions += plan.evicted.len() as u64;
        s.refusals += plan.refused.len() as u64;
        s.failed += plan.failed.len() as u64;
        let mut grew = false;
        for kind in PoolKind::ALL {
            let pool = &plan.pools[kind];
            let live = self.model.pool(kind);
            let run = &mut self.stats.pools[kind];
            run.record_repack(pool, self.layer_bytes[kind], plan_nanos);
            run.peak_occupied_layers = run.peak_occupied_layers.max(live.occupied_layers());
            run.peak_layers = run.peak_layers.max(live.layers());
            if let Some(growth) = pool.growth {
                run.growths += 1;
                grew = true;
                self.retiring[kind] = Some(RetiringGeneration {
                    bytes: streamed_pool_bytes(growth.from_layers, self.layer_bytes[kind]),
                    ticks_left: RETIRE_AFTER_TICKS,
                });
            }
        }
        let held = self.active_bytes() + self.retiring_bytes();
        let s = &mut self.stats;
        s.peak_pool_bytes = s.peak_pool_bytes.max(held);
        if grew {
            s.growth_transient_peak_bytes = s.growth_transient_peak_bytes.max(held);
        }
        self.outcome(batch)
    }

    fn payload_fits(&self, block: u32, payload: &LightmapBlockPayload) -> bool {
        let Some(layout) = &self.layout else {
            return true;
        };
        let (extent, planes) = (self.model.extent(block), self.model.planes(block));
        payload_mismatch(&layout.header, extent, planes, payload).is_none()
    }

    /// As `LightmapStreamState::outcome`.
    fn outcome(&self, batch: LightmapDrainBatch) -> LightmapDrainOutcome {
        let plan = self.model.plan();
        let deferred = if plan.deferred.is_empty() {
            Vec::new()
        } else {
            batch
                .ready
                .into_iter()
                .filter(|prepared| plan.deferred.binary_search(&prepared.block).is_ok())
                .collect()
        };
        LightmapDrainOutcome {
            installed: plan.installed.clone(),
            refused: plan.refused.clone(),
            failed: plan.failed.clone(),
            deferred,
            evicted: plan.evicted.iter().map(|eviction| eviction.block).collect(),
            room_reserved: PerPool {
                lightmap: plan.pools.lightmap.room_reserved.clone(),
                mask: plan.pools.mask.room_reserved.clone(),
            },
            room_returned: PerPool {
                lightmap: plan.pools.lightmap.room_returned.clone(),
                mask: plan.pools.mask.room_returned.clone(),
            },
            pool: plan.report,
        }
    }
}

fn payload_bytes(payload: &LightmapBlockPayload) -> u64 {
    (payload.irradiance.len()
        + payload.direction.len()
        + payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len())) as u64
}
