//! Dynamic fragmentation of a cell-block pool: camera walks across portal
//! adjacency, and at each step the pool frees blocks that left M(c) and
//! allocates the ones that joined it with the freeing shelf allocator.
//!
//! Runs an uncapped pool (immediate free) for the peak it grows to, then fixed
//! pools with immediate free or LRU retention; an allocation that fails with
//! nothing left to evict triggers a defragmentation, a from-scratch repack of
//! the mandatory blocks.

use std::collections::BTreeSet;

use rayon::prelude::*;

use super::block_allocator::{BlockPool, Slot};
use super::camera_walks::{WalkKind, camera_adjacency, walk_path};
use super::cell_blocks::{BlockDims, CellBlocks, POOL_LAYER_EDGE};
use super::portal_distance::PortalGraphInput;

/// Steps per walk.
pub(crate) const SIM_STEPS: usize = 20_000;
pub(crate) const SIM_SEED: u64 = 0x5EED_B10C;

/// Fixed pool sizes, as a percentage of the static worst layer count.
pub(crate) const FIXED_POOL_PERCENT: [u32; 2] = [100, 125];

pub(crate) struct SimInputs<'a> {
    pub blocks: &'a CellBlocks,
    pub camera_cells: &'a [u32],
    /// Cell-granular M(c) per camera cell, parallel to `camera_cells`.
    pub sets: &'a [Vec<u32>],
    /// MaxRects from-scratch layers per camera cell, parallel to `camera_cells`.
    pub static_layers: &'a [u32],
    pub graph: &'a PortalGraphInput,
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
    /// Largest static MaxRects layer count among the cells the walk visited.
    pub walk_static_peak: u32,
    /// Steps whose pool extent exceeded that step's static layer count.
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
    /// Jumps to a non-adjacent cell: no camera-cell neighbour, or the tour
    /// exhausted its component.
    pub teleports: usize,
    pub unbounded: UnboundedRun,
    pub fixed: Vec<FixedRun>,
}

pub(crate) fn run_walks(inputs: &SimInputs<'_>) -> Vec<WalkResult> {
    let adjacency = camera_adjacency(inputs.graph, inputs.camera_cells);
    let fixed_sizes: Vec<u32> = FIXED_POOL_PERCENT
        .iter()
        .map(|&percent| (inputs.static_worst_layers * percent).div_ceil(100))
        .collect();
    WalkKind::ALL
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
        .collect()
}

/// Mandatory blocks of `set` that fit a pool layer, in allocation order:
/// tallest first, as shelf packing prefers.
fn allocation_order(blocks: &CellBlocks, set: &[u32]) -> Vec<(u32, BlockDims)> {
    let mut order: Vec<(u32, BlockDims)> = blocks
        .set_dims(set)
        .filter(|(_, dims)| dims.fits_pool_layer())
        .collect();
    order.sort_by(|(ca, a), (cb, b)| {
        b.height
            .cmp(&a.height)
            .then(b.width.cmp(&a.width))
            .then(ca.cmp(cb))
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

/// Resident blocks: slot per cell, and an LRU order keyed by last use.
struct Residency {
    slots: Vec<Option<Slot>>,
    last_used: Vec<u64>,
    lru: BTreeSet<(u64, u32)>,
    in_set: Vec<u64>,
}

impl Residency {
    fn new(cell_count: usize) -> Self {
        Self {
            slots: vec![None; cell_count],
            last_used: vec![0; cell_count],
            lru: BTreeSet::new(),
            in_set: vec![0; cell_count],
        }
    }

    fn insert(&mut self, cell: u32, slot: Slot, step: u64) {
        self.slots[cell as usize] = Some(slot);
        self.last_used[cell as usize] = step;
        self.lru.insert((step, cell));
    }

    fn evict(&mut self, cell: u32, pool: &mut BlockPool) {
        if let Some(slot) = self.slots[cell as usize].take() {
            pool.free(slot);
            self.lru.remove(&(self.last_used[cell as usize], cell));
        }
    }

    fn touch(&mut self, cell: u32, step: u64) {
        if self.slots[cell as usize].is_some() {
            self.lru.remove(&(self.last_used[cell as usize], cell));
            self.lru.insert((step, cell));
        }
        self.last_used[cell as usize] = step;
    }

    /// Mark `set` mandatory for `step`: immediate eviction frees every block
    /// outside it, and every block inside it is stamped with `step`, so only
    /// blocks outside the set are ever evictable.
    fn begin_step(&mut self, set: &[u32], step: u64, eviction: Eviction, pool: &mut BlockPool) {
        for &cell in set {
            self.in_set[cell as usize] = step;
        }
        if eviction == Eviction::Immediate {
            let departed: Vec<u32> = self
                .lru
                .iter()
                .map(|&(_, cell)| cell)
                .filter(|&cell| self.in_set[cell as usize] != step)
                .collect();
            for cell in departed {
                self.evict(cell, pool);
            }
        }
        for &cell in set {
            self.touch(cell, step);
        }
    }

    /// Oldest resident block outside the current set.
    fn oldest_evictable(&self, step: u64) -> Option<u32> {
        self.lru
            .iter()
            .next()
            .filter(|&&(used, _)| used < step)
            .map(|&(_, cell)| cell)
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
        let set = &inputs.sets[camera as usize];
        residency.begin_step(set, step, Eviction::Immediate, &mut pool);
        for (cell, dims) in allocation_order(inputs.blocks, set) {
            if residency.slots[cell as usize].is_none() {
                let slot = pool
                    .allocate(dims.width, dims.height)
                    .expect("an uncapped pool always places a block that fits a layer");
                residency.insert(cell, slot, step);
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
        let set = &inputs.sets[camera as usize];
        residency.begin_step(set, step, eviction, &mut pool);
        let order = allocation_order(inputs.blocks, set);
        let mut stuck = false;
        'blocks: for &(cell, dims) in &order {
            if residency.slots[cell as usize].is_some() {
                continue;
            }
            loop {
                if let Some(slot) = pool.allocate(dims.width, dims.height) {
                    residency.insert(cell, slot, step);
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
        for &cell in set {
            residency.in_set[cell as usize] = step;
        }
        let mut failed = 0;
        for &(cell, dims) in &order {
            match pool.allocate(dims.width, dims.height) {
                Some(slot) => residency.insert(cell, slot, step),
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
