//! Cell-block pool walks under the brief's miss policy, over the brief set.
//!
//! Each step makes `M(c, L)` resident and never refuses it: a mandatory block
//! that finds no room under the cap first evicts band blocks (farthest lead
//! first); with none left it repacks in place when `M(c, L)` alone fits the
//! cap, and otherwise grows the pool past the cap. Band retain keeps resident
//! band blocks under the cap and prefetches the rest, nearest lead first,
//! only into free space under the cap; immediate free keeps only `M(c, L)`.
//! Every request completes within its step: no drain budget is modelled.
//!
//! Reuses the walks and allocation order of `block_pool_sim` and the freeing
//! shelf allocator of `postretro_render_cpu::lightmap_pool`.
//! See: context/plans/large-map-spatial-residency.md

use rayon::prelude::*;

use super::block_pool_sim::{FIXED_POOL_PERCENT, allocation_order};
use super::camera_walks::{WalkKind, camera_adjacency, walk_path};
use super::cell_blocks::{BlockDims, CellBlocks, POOL_LAYER_EDGE};
use crate::cell_residency_bake::portal_distance::PortalGraphInput;
use postretro_render_cpu::lightmap_pool::{BlockPool, Slot};

/// A read of a block freed at most this many steps earlier counts as thrash.
/// Over a long walk nearly every read is of a block freed at some point, so
/// only a short window separates churn from revisiting. Eight is an arbitrary
/// measurement choice, not a runtime constant.
pub(crate) const THRASH_WINDOW_STEPS: u64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BandPolicy {
    /// Free every block outside `M(c, L)` at the next step.
    ImmediateFree,
    /// Keep and prefetch band blocks under the cap; free the rest.
    BandRetain,
}

impl BandPolicy {
    pub(crate) fn label(self) -> &'static str {
        match self {
            BandPolicy::ImmediateFree => "immediate free",
            BandPolicy::BandRetain => "band retain",
        }
    }
}

pub(crate) struct BandSimInputs<'a> {
    pub blocks: &'a CellBlocks,
    /// `M(c, L)` per camera cell, parallel to the walk's camera cells.
    pub mandatory: &'a [Vec<u32>],
    /// Band per camera cell, nearest lead first.
    pub band: &'a [Vec<u32>],
    /// Shelf from-scratch layers of each `M(c, L)`: whether a repack fits.
    pub mandatory_shelf_layers: &'a [u32],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BandRun {
    pub policy: BandPolicy,
    /// `None` is uncapped: nothing is ever evicted, repacked or grown.
    pub cap: Option<u32>,
    pub steps: usize,
    pub peak_layers: usize,
    /// Steps where `M(c, L)` forced the pool to grow: a mandatory block
    /// opened a layer at or past the cap.
    pub growth_steps: usize,
    /// Steps ending with the pool over the cap. Only mandatory blocks sit at
    /// or past it: band blocks there are freed at the start of each step.
    pub over_cap_steps: usize,
    pub repack_steps: usize,
    /// Band blocks evicted to make room for mandatory ones and not moved
    /// back by a repack.
    pub band_evictions: usize,
    /// Blocks joining `M(c, L)` after the first step, and how many of them
    /// were already resident. Blocks larger than a pool layer are excluded.
    pub entering_blocks: usize,
    pub entering_resident: usize,
    /// Non-resident-to-resident transitions of mandatory blocks: reads a
    /// frame would wait on. Repack moves are not reads.
    pub demand_reads: usize,
    /// Band blocks read ahead of need.
    pub prefetch_reads: usize,
    /// Demand and prefetch bytes read.
    pub read_bytes: u64,
    /// Demand and prefetch reads of a block freed within
    /// `THRASH_WINDOW_STEPS`.
    pub thrash_reads: usize,
    pub thrash_bytes: u64,
}

impl BandRun {
    pub(crate) fn hit_rate(&self) -> f64 {
        self.entering_resident as f64 / self.entering_blocks.max(1) as f64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BandWalk {
    pub kind: WalkKind,
    pub steps: usize,
    pub runs: Vec<BandRun>,
}

/// A pool cap and what sized it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolCap {
    pub layers: u32,
    pub basis: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BandWalks {
    pub caps: Vec<PoolCap>,
    pub walks: Vec<BandWalk>,
}

/// Caps the walks run at: the shelf p95, the one cap below the worst so
/// growth past it can occur, then each `FIXED_POOL_PERCENT` of the shelf
/// worst. Equal caps merge into one, naming every basis.
pub(crate) fn pool_caps(shelf_worst: u32, shelf_p95: u32) -> Vec<PoolCap> {
    let candidates = std::iter::once((shelf_p95, "shelf p95".to_string())).chain(
        FIXED_POOL_PERCENT.iter().map(|&percent| {
            (
                (shelf_worst * percent).div_ceil(100),
                format!("{percent}% shelf worst"),
            )
        }),
    );
    let mut caps: Vec<PoolCap> = Vec::new();
    for (layers, basis) in candidates {
        match caps.iter_mut().find(|cap| cap.layers == layers) {
            Some(cap) => cap.basis = format!("{} = {basis}", cap.basis),
            None => caps.push(PoolCap { layers, basis }),
        }
    }
    caps
}

/// Every walk kind, uncapped and at each of `caps`, under both policies.
pub(crate) fn run_band_walks(
    inputs: &BandSimInputs<'_>,
    camera_cells: &[u32],
    graph: &PortalGraphInput,
    caps: Vec<PoolCap>,
    steps: usize,
    seed: u64,
) -> BandWalks {
    let adjacency = camera_adjacency(graph, camera_cells);
    let run_caps: Vec<Option<u32>> = std::iter::once(None)
        .chain(caps.iter().map(|cap| Some(cap.layers)))
        .collect();
    let walks = WalkKind::ALL
        .par_iter()
        .map(|&kind| {
            let (path, _) = walk_path(kind, &adjacency, steps, seed);
            let runs: Vec<(Option<u32>, BandPolicy)> = run_caps
                .iter()
                .flat_map(|&cap| {
                    [
                        (cap, BandPolicy::ImmediateFree),
                        (cap, BandPolicy::BandRetain),
                    ]
                })
                .collect();
            BandWalk {
                kind,
                steps: path.len(),
                runs: runs
                    .par_iter()
                    .map(|&(cap, policy)| simulate_band(inputs, &path, cap, policy))
                    .collect(),
            }
        })
        .collect();
    BandWalks { caps, walks }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Demand,
    Prefetch,
    /// A repack moving a resident block: no read. A move is not a free for
    /// thrash purposes either, so it leaves `freed_at` alone.
    Move,
}

/// Resident blocks by cell, plus the step each was last freed (0: never).
struct BandResidency<'a> {
    blocks: &'a CellBlocks,
    slots: Vec<Option<Slot>>,
    freed_at: Vec<u64>,
    resident: Vec<u32>,
    step: u64,
}

impl BandResidency<'_> {
    fn is_resident(&self, cell: u32) -> bool {
        self.slots[cell as usize].is_some()
    }

    fn freed_this_step(&self, cell: u32) -> bool {
        self.freed_at[cell as usize] == self.step
    }

    fn place(&mut self, cell: u32, slot: Slot, placement: Placement, run: &mut BandRun) {
        self.slots[cell as usize] = Some(slot);
        self.resident.push(cell);
        match placement {
            Placement::Move => return,
            Placement::Demand => run.demand_reads += 1,
            Placement::Prefetch => run.prefetch_reads += 1,
        }
        let bytes = self.blocks.block_bytes[cell as usize];
        run.read_bytes += bytes;
        let freed_at = self.freed_at[cell as usize];
        if freed_at > 0 && self.step - freed_at <= THRASH_WINDOW_STEPS {
            run.thrash_reads += 1;
            run.thrash_bytes += bytes;
        }
    }

    /// Free every resident block `departs` selects by cell and slot.
    fn free_where(&mut self, pool: &mut BlockPool, mut departs: impl FnMut(u32, Slot) -> bool) {
        let (slots, freed_at, step) = (&mut self.slots, &mut self.freed_at, self.step);
        self.resident.retain(|&cell| {
            let slot = slots[cell as usize].expect("a listed block is resident");
            if !departs(cell, slot) {
                return true;
            }
            slots[cell as usize] = None;
            pool.free(slot)
                .expect("a resident slot names its live allocation");
            freed_at[cell as usize] = step;
            false
        });
    }
}

pub(crate) fn simulate_band(
    inputs: &BandSimInputs<'_>,
    path: &[u32],
    cap: Option<u32>,
    policy: BandPolicy,
) -> BandRun {
    let cell_count = inputs.blocks.dims.len();
    let limit = cap.map(|layers| layers as usize);
    let past_cap = |slot: Slot| limit.is_some_and(|cap| slot.layer as usize >= cap);
    let mut pool = BlockPool::new(POOL_LAYER_EDGE, None);
    let mut state = BandResidency {
        blocks: inputs.blocks,
        slots: vec![None; cell_count],
        freed_at: vec![0; cell_count],
        resident: Vec::new(),
        step: 0,
    };
    let mut in_mandatory = vec![0u64; cell_count];
    let mut in_band = vec![0u64; cell_count];
    let mut run = BandRun {
        policy,
        cap,
        steps: path.len(),
        peak_layers: 0,
        growth_steps: 0,
        over_cap_steps: 0,
        repack_steps: 0,
        band_evictions: 0,
        entering_blocks: 0,
        entering_resident: 0,
        demand_reads: 0,
        prefetch_reads: 0,
        read_bytes: 0,
        thrash_reads: 0,
        thrash_bytes: 0,
    };
    let retain_band = policy == BandPolicy::BandRetain;
    for (i, &camera) in path.iter().enumerate() {
        let step = i as u64 + 1;
        state.step = step;
        let mandatory = &inputs.mandatory[camera as usize];
        let band = &inputs.band[camera as usize];
        let order = allocation_order(inputs.blocks, mandatory);
        if i > 0 {
            for &(cell, _) in &order {
                if in_mandatory[cell as usize] != step - 1 {
                    run.entering_blocks += 1;
                    run.entering_resident += usize::from(state.is_resident(cell));
                }
            }
        }
        for &cell in mandatory {
            in_mandatory[cell as usize] = step;
        }
        if retain_band {
            for &cell in band {
                in_band[cell as usize] = step;
            }
        }
        // Band blocks past the cap are refused, so a mandatory block placed
        // there during growth leaves once it drops into the band: the pool
        // shrinks back under the cap instead of staying over it.
        state.free_where(&mut pool, |cell, slot| {
            in_mandatory[cell as usize] != step
                && (in_band[cell as usize] != step || past_cap(slot))
        });
        // What a repack moves rather than re-reads, with each block's last
        // free so a move can undo a victim's.
        let band_resident: Vec<(u32, u64)> = band
            .iter()
            .filter(|&&c| state.is_resident(c) && in_mandatory[c as usize] != step)
            .map(|&c| (c, state.freed_at[c as usize]))
            .collect();

        let mut victims = 0;
        let mut repack_dropped = None;
        let mut grew = false;
        'mandatory: for &(cell, dims) in &order {
            if state.is_resident(cell) {
                continue;
            }
            loop {
                if let Some(slot) = pool.allocate_within(dims.width, dims.height, limit) {
                    state.place(cell, slot, Placement::Demand, &mut run);
                    break;
                }
                let victim = band
                    .iter()
                    .rev()
                    .copied()
                    .find(|&c| state.is_resident(c) && in_mandatory[c as usize] != step);
                if let Some(victim) = victim {
                    state.free_where(&mut pool, |c, _| c == victim);
                    victims += 1;
                    continue;
                }
                let fits_cap = limit.is_some_and(|cap| {
                    inputs.mandatory_shelf_layers[camera as usize] as usize <= cap
                });
                if fits_cap {
                    repack_dropped = Some(repack(
                        &mut state,
                        &mut pool,
                        &order,
                        &band_resident,
                        limit,
                        &mut run,
                    ));
                    break 'mandatory;
                }
                let extent = pool.extent();
                let slot = pool
                    .allocate_within(dims.width, dims.height, None)
                    .expect("an uncapped pool always places a block that fits a layer");
                grew |= slot.layer as usize >= extent;
                state.place(cell, slot, Placement::Demand, &mut run);
                break;
            }
        }
        run.growth_steps += usize::from(grew);
        run.repack_steps += usize::from(repack_dropped.is_some());
        run.band_evictions += repack_dropped.unwrap_or(victims);

        if retain_band {
            for (cell, dims) in inputs.blocks.set_dims(band) {
                // A block freed this step, as a victim or past the cap, is not
                // read back in the same step: that read would undo the free.
                if dims.fits_pool_layer()
                    && !state.is_resident(cell)
                    && !state.freed_this_step(cell)
                    && let Some(slot) = pool.allocate_within(dims.width, dims.height, limit)
                {
                    state.place(cell, slot, Placement::Prefetch, &mut run);
                }
            }
        }
        let extent = pool.extent();
        run.peak_layers = run.peak_layers.max(extent);
        run.over_cap_steps += usize::from(limit.is_some_and(|cap| extent > cap));
    }
    run
}

/// Compact in place under the cap, as the brief's repack does: `M(c, L)` in
/// allocation order, then `band_resident` nearest lead first. Resident blocks
/// move without a read. A step's placements are planned before any texel is
/// written, so a band block evicted as a victim earlier in the step still
/// holds its texels and moves too; one that no longer fits under the cap is
/// dropped. Returns the dropped count.
fn repack(
    state: &mut BandResidency<'_>,
    pool: &mut BlockPool,
    order: &[(u32, BlockDims)],
    band_resident: &[(u32, u64)],
    limit: Option<usize>,
    run: &mut BandRun,
) -> usize {
    let was_resident: Vec<bool> = order
        .iter()
        .map(|&(cell, _)| state.is_resident(cell))
        .collect();
    pool.clear();
    for cell in std::mem::take(&mut state.resident) {
        state.slots[cell as usize] = None;
    }
    for (&(cell, dims), &moved) in order.iter().zip(&was_resident) {
        let slot = pool
            .allocate_within(dims.width, dims.height, limit)
            .expect("a mandatory set whose shelf count fits the cap repacks under it");
        let placement = if moved {
            Placement::Move
        } else {
            Placement::Demand
        };
        state.place(cell, slot, placement, run);
    }
    let mut dropped = 0;
    for &(cell, freed_at) in band_resident {
        let dims = state.blocks.dims[cell as usize].expect("a resident block has dims");
        match pool.allocate_within(dims.width, dims.height, limit) {
            Some(slot) => {
                state.freed_at[cell as usize] = freed_at;
                state.place(cell, slot, Placement::Move, run);
            }
            None => {
                state.freed_at[cell as usize] = state.step;
                dropped += 1;
            }
        }
    }
    dropped
}
