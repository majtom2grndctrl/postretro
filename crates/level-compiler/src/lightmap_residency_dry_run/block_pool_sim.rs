//! Dynamic fragmentation of a cell-block pool: camera walks across portal
//! adjacency, and at each step the pool frees blocks that left M(c) and
//! allocates the ones that joined it with the freeing shelf allocator. A cell
//! in M(c) demands every block it owns.
//!
//! Runs an uncapped pool (immediate free) for the peak it grows to, then fixed
//! pools with immediate free or LRU retention; an allocation that fails with
//! nothing left to evict triggers a defragmentation, a from-scratch repack of
//! the mandatory blocks.
//!
//! The baseline is the same shelf allocator packing each M(c) from scratch,
//! so excess over it is fragmentation alone, not the shelf-vs-MaxRects
//! packing gap.

use std::collections::BTreeSet;

use rayon::prelude::*;

use super::camera_walks::{WalkKind, camera_adjacency, component_count, walk_path};
use super::cell_blocks::{BlockDims, CellBlocks, POOL_LAYER_EDGE};
use crate::cell_residency_bake::portal_distance::PortalGraphInput;
use postretro_render_cpu::lightmap_pool::{BlockPool, Slot};

/// Steps per walk.
pub(crate) const SIM_STEPS: usize = 20_000;
pub(crate) const SIM_SEED: u64 = 0x5EED_B10C;

/// Fixed pool sizes, as a percentage of the shelf from-scratch worst layer
/// count.
pub(crate) const FIXED_POOL_PERCENT: [u32; 2] = [100, 125];

pub(crate) struct SimInputs<'a> {
    pub blocks: &'a CellBlocks,
    pub camera_cells: &'a [u32],
    /// Cell-granular M(c) per camera cell, parallel to `camera_cells`.
    pub sets: &'a [Vec<u32>],
    /// Shelf from-scratch layers per camera cell (`shelf_layers_from_scratch`),
    /// parallel to `camera_cells`: the no-fragmentation baseline.
    pub static_layers: &'a [u32],
    pub graph: &'a PortalGraphInput,
    /// Largest of `static_layers`; sizes the fixed pools.
    pub static_worst_layers: u32,
    pub steps: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eviction {
    /// Free a block the step its cell leaves M(c).
    Immediate,
    /// Keep departed blocks until an allocation needs their space, oldest
    /// first.
    Lru,
}

impl Eviction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Eviction::Immediate => "immediate",
            Eviction::Lru => "LRU",
        }
    }
}

/// An uncapped pool under immediate free.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnboundedRun {
    /// Most layers the pool ever had to hold (highest non-empty layer + 1).
    pub peak_layers: usize,
    /// Largest shelf from-scratch layer count among the cells the walk visited.
    pub walk_static_peak: u32,
    /// Steps whose pool extent exceeded that step's shelf from-scratch count.
    pub steps_over_static: usize,
    /// Largest `extent - static` over the walk.
    pub max_excess: i64,
    /// Mean `extent - static` over the walk.
    pub mean_excess: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FixedRun {
    pub pool_layers: u32,
    pub eviction: Eviction,
    /// Steps where an allocation failed with nothing left to evict: each one
    /// repacks (defragments) the mandatory blocks from scratch.
    pub defrag_steps: usize,
    /// Steps where even the repack could not place every mandatory block.
    /// The repack is the shelf from-scratch packing, so a pool at least the
    /// shelf worst never hard-fails.
    pub hard_fail_steps: usize,
    pub hard_fail_blocks: usize,
    /// LRU blocks evicted to make room.
    pub evictions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WalkResult {
    pub kind: WalkKind,
    pub steps: usize,
    pub distinct_cells: usize,
    /// Jumps to a non-adjacent cell: no camera-cell neighbour, a stalled
    /// random walk, or the tour exhausted its component.
    pub teleports: usize,
    pub unbounded: UnboundedRun,
    pub fixed: Vec<FixedRun>,
}

/// Every walk kind over one portal adjacency.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoolWalks {
    /// Portal-connected components among camera cells.
    pub camera_components: usize,
    pub walks: Vec<WalkResult>,
}

pub(crate) fn run_walks(inputs: &SimInputs<'_>) -> PoolWalks {
    let adjacency = camera_adjacency(inputs.graph, inputs.camera_cells);
    let fixed_sizes: Vec<u32> = FIXED_POOL_PERCENT
        .iter()
        .map(|&percent| (inputs.static_worst_layers * percent).div_ceil(100))
        .collect();
    let walks = WalkKind::ALL
        .par_iter()
        .map(|&kind| {
            let (path, teleports) = walk_path(kind, &adjacency, inputs.steps, inputs.seed);
            let mut distinct = path.clone();
            distinct.sort_unstable();
            distinct.dedup();
            let unbounded = simulate_unbounded(inputs, &path);
            let runs: Vec<(u32, Eviction)> = fixed_sizes
                .iter()
                .flat_map(|&n| [(n, Eviction::Immediate), (n, Eviction::Lru)])
                .collect();
            let fixed = runs
                .par_iter()
                .map(|&(layers, eviction)| simulate_fixed(inputs, &path, layers, eviction))
                .collect();
            WalkResult {
                kind,
                steps: path.len(),
                distinct_cells: distinct.len(),
                teleports,
                unbounded,
                fixed,
            }
        })
        .collect();
    PoolWalks {
        camera_components: component_count(&adjacency),
        walks,
    }
}

/// Every block of `set`'s cells, keyed by block id, in allocation order:
/// tallest first, as shelf packing prefers. Every block fits a pool layer.
pub(crate) fn allocation_order(blocks: &CellBlocks, set: &[u32]) -> Vec<(u32, BlockDims)> {
    let mut order: Vec<(u32, BlockDims)> = blocks.set_dims(set).collect();
    order.sort_by(|(block_a, a), (block_b, b)| {
        b.height
            .cmp(&a.height)
            .then(b.width.cmp(&a.width))
            .then(block_a.cmp(block_b))
    });
    order
}

/// Layers the shelf allocator needs for `set` from an empty pool.
pub(crate) fn shelf_layers_from_scratch(blocks: &CellBlocks, set: &[u32]) -> u32 {
    let mut pool = BlockPool::new(POOL_LAYER_EDGE, None);
    for (_, dims) in allocation_order(blocks, set) {
        pool.allocate(dims.width, dims.height)
            .expect("an uncapped pool always places a block that fits a layer");
    }
    pool.extent() as u32
}

/// Resident blocks: slot per block, and an LRU order keyed by last use.
struct Residency {
    slots: Vec<Option<Slot>>,
    last_used: Vec<u64>,
    lru: BTreeSet<(u64, u32)>,
    in_set: Vec<u64>,
}

impl Residency {
    fn new(block_count: usize) -> Self {
        Self {
            slots: vec![None; block_count],
            last_used: vec![0; block_count],
            lru: BTreeSet::new(),
            in_set: vec![0; block_count],
        }
    }

    fn insert(&mut self, block: u32, slot: Slot, step: u64) {
        self.slots[block as usize] = Some(slot);
        self.last_used[block as usize] = step;
        self.lru.insert((step, block));
    }

    fn evict(&mut self, block: u32, pool: &mut BlockPool) {
        if let Some(slot) = self.slots[block as usize].take() {
            pool.free(slot)
                .expect("a resident slot names its live allocation");
            self.lru.remove(&(self.last_used[block as usize], block));
        }
    }

    fn touch(&mut self, block: u32, step: u64) {
        if self.slots[block as usize].is_some() {
            self.lru.remove(&(self.last_used[block as usize], block));
            self.lru.insert((step, block));
        }
        self.last_used[block as usize] = step;
    }

    /// Mark the `mandatory` blocks for `step`: immediate eviction frees every
    /// block outside them, and every block among them is stamped with `step`,
    /// so only blocks outside the set are ever evictable.
    fn begin_step(
        &mut self,
        mandatory: &[(u32, BlockDims)],
        step: u64,
        eviction: Eviction,
        pool: &mut BlockPool,
    ) {
        for &(block, _) in mandatory {
            self.in_set[block as usize] = step;
        }
        if eviction == Eviction::Immediate {
            let departed: Vec<u32> = self
                .lru
                .iter()
                .map(|&(_, block)| block)
                .filter(|&block| self.in_set[block as usize] != step)
                .collect();
            for block in departed {
                self.evict(block, pool);
            }
        }
        for &(block, _) in mandatory {
            self.touch(block, step);
        }
    }

    /// Oldest resident block outside the current set.
    fn oldest_evictable(&self, step: u64) -> Option<u32> {
        self.lru
            .iter()
            .next()
            .filter(|&&(used, _)| used < step)
            .map(|&(_, block)| block)
    }
}

fn simulate_unbounded(inputs: &SimInputs<'_>, path: &[u32]) -> UnboundedRun {
    let mut pool = BlockPool::new(POOL_LAYER_EDGE, None);
    let mut residency = Residency::new(inputs.blocks.dims.len());
    let mut run = UnboundedRun {
        peak_layers: 0,
        walk_static_peak: 0,
        steps_over_static: 0,
        max_excess: 0,
        mean_excess: 0.0,
    };
    let mut excess_sum = 0i64;
    for (i, &camera) in path.iter().enumerate() {
        let step = i as u64 + 1;
        let order = allocation_order(inputs.blocks, &inputs.sets[camera as usize]);
        residency.begin_step(&order, step, Eviction::Immediate, &mut pool);
        for &(block, dims) in &order {
            if residency.slots[block as usize].is_none() {
                let slot = pool
                    .allocate(dims.width, dims.height)
                    .expect("an uncapped pool always places a block that fits a layer");
                residency.insert(block, slot, step);
            }
        }
        let extent = pool.extent();
        let static_layers = inputs.static_layers[camera as usize];
        run.peak_layers = run.peak_layers.max(extent);
        run.walk_static_peak = run.walk_static_peak.max(static_layers);
        let excess = extent as i64 - i64::from(static_layers);
        run.max_excess = run.max_excess.max(excess);
        excess_sum += excess;
        if excess > 0 {
            run.steps_over_static += 1;
        }
    }
    run.mean_excess = excess_sum as f64 / path.len().max(1) as f64;
    run
}

pub(crate) fn simulate_fixed(
    inputs: &SimInputs<'_>,
    path: &[u32],
    pool_layers: u32,
    eviction: Eviction,
) -> FixedRun {
    let mut pool = BlockPool::new(POOL_LAYER_EDGE, Some(pool_layers as usize));
    let mut residency = Residency::new(inputs.blocks.dims.len());
    let mut run = FixedRun {
        pool_layers,
        eviction,
        defrag_steps: 0,
        hard_fail_steps: 0,
        hard_fail_blocks: 0,
        evictions: 0,
    };
    for (i, &camera) in path.iter().enumerate() {
        let step = i as u64 + 1;
        let order = allocation_order(inputs.blocks, &inputs.sets[camera as usize]);
        residency.begin_step(&order, step, eviction, &mut pool);
        let mut stuck = false;
        'blocks: for &(block, dims) in &order {
            if residency.slots[block as usize].is_some() {
                continue;
            }
            loop {
                if let Some(slot) = pool.allocate(dims.width, dims.height) {
                    residency.insert(block, slot, step);
                    break;
                }
                match residency.oldest_evictable(step) {
                    Some(victim) => {
                        residency.evict(victim, &mut pool);
                        run.evictions += 1;
                    }
                    None => {
                        stuck = true;
                        break 'blocks;
                    }
                }
            }
        }
        if !stuck {
            continue;
        }
        // Defragment: drop everything and repack the mandatory blocks.
        run.defrag_steps += 1;
        pool.clear();
        residency = Residency::new(inputs.blocks.dims.len());
        for &(block, _) in &order {
            residency.in_set[block as usize] = step;
        }
        let mut failed = 0;
        for &(block, dims) in &order {
            match pool.allocate(dims.width, dims.height) {
                Some(slot) => residency.insert(block, slot, step),
                None => failed += 1,
            }
        }
        if failed > 0 {
            run.hard_fail_steps += 1;
            run.hard_fail_blocks += failed;
        }
    }
    run
}
