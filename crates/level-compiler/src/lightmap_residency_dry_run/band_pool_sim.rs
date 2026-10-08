//! Cell-block pool walks under the brief's miss policy, over the brief set.
//!
//! Each step is one drain of the renderer's placement policy,
//! `postretro_render_cpu::lightmap_pool::LightmapPoolModel`: `M(c, L)` is
//! targeted mandatory and never refused, and band retain also targets the
//! band, nearest lead first; immediate free targets only `M(c, L)`. Every
//! targeted block that is not resident is ready at once and every growth
//! retires before the next step: no drain budget or GPU latency is modelled.
//!
//! Reuses the walks of `block_pool_sim`.
//! See: context/plans/large-map-spatial-residency.md

use rayon::prelude::*;

use super::block_pool_sim::FIXED_POOL_PERCENT;
use super::camera_walks::{WalkKind, camera_adjacency, walk_path};
use super::cell_blocks::{CellBlocks, POOL_LAYER_EDGE};
use crate::cell_residency_bake::portal_distance::PortalGraphInput;
use postretro_level_loader::{LightmapBlockClass, LightmapTarget};
use postretro_render_cpu::lightmap_pool::{DrainRequest, EvictionReason, LightmapPoolModel};

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BandRun {
    pub policy: BandPolicy,
    /// `None` is uncapped: the first generation holds every block, so
    /// nothing is evicted for room.
    pub cap: Option<u32>,
    pub steps: usize,
    /// Most layers holding a block at a step's end.
    pub peak_layers: usize,
    /// Steps that allocated a new pool generation: a mandatory block needed a
    /// layer past the pool's, and a repack under the cap could not hold
    /// `M(c, L)`. The pool never shrinks, so later steps reuse those layers.
    pub growth_steps: usize,
    /// Steps ending with blocks at or past the cap. Only mandatory blocks
    /// sit there: band blocks past it are evicted at the start of each step.
    pub over_cap_steps: usize,
    pub repack_steps: usize,
    /// Band blocks evicted to make room for mandatory ones and not moved
    /// back by a repack.
    pub band_evictions: usize,
    /// Blocks joining `M(c, L)` after the first step, and how many of them
    /// were already resident; a cell joining brings every block it owns.
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

/// Per-step target deltas against the model's accepted targets. A staged
/// cell targets every block it owns, at the cell's class and lead.
struct SimTargets<'a> {
    blocks: &'a CellBlocks,
    current: Vec<Option<(LightmapBlockClass, u32)>>,
    next: Vec<Option<(LightmapBlockClass, u32)>>,
    targeted: Vec<u32>,
    next_list: Vec<u32>,
    set: Vec<LightmapTarget>,
    remove: Vec<u32>,
}

impl SimTargets<'_> {
    /// Stage `M(c, L)` as mandatory, then `band` (if any) at its position
    /// as lead, which keeps its nearest-first order.
    fn stage(&mut self, mandatory: &[u32], band: &[u32]) {
        self.next_list.clear();
        let classes = mandatory
            .iter()
            .map(|&cell| (cell, LightmapBlockClass::Mandatory, 0))
            .chain(
                band.iter()
                    .enumerate()
                    .map(|(lead, &cell)| (cell, LightmapBlockClass::Band, lead as u32)),
            );
        for (cell, class, lead) in classes {
            for block in self.blocks.blocks_of_cell(cell) {
                if self.next[block as usize].is_none() {
                    self.next[block as usize] = Some((class, lead));
                    self.next_list.push(block);
                }
            }
        }
        self.set.clear();
        self.remove.clear();
        for &block in &self.next_list {
            let next = self.next[block as usize];
            if next != self.current[block as usize] {
                let (class, lead) = next.expect("staged");
                self.set.push(LightmapTarget { block, class, lead });
            }
        }
        for &block in &self.targeted {
            if self.next[block as usize].is_none() {
                self.remove.push(block);
            }
            self.current[block as usize] = None;
        }
        self.set.sort_unstable_by_key(|target| target.block);
        self.remove.sort_unstable();
        for &block in &self.next_list {
            self.current[block as usize] = self.next[block as usize].take();
        }
        std::mem::swap(&mut self.targeted, &mut self.next_list);
    }
}

pub(crate) fn simulate_band(
    inputs: &BandSimInputs<'_>,
    path: &[u32],
    cap: Option<u32>,
    policy: BandPolicy,
) -> BandRun {
    let blocks = inputs.blocks;
    let block_count = blocks.dims.len();
    let extents = blocks.dims.iter().map(|d| (d.width, d.height)).collect();
    let cap_layers = cap.unwrap_or(u32::MAX);
    let mut model = LightmapPoolModel::new(extents, blocks.alignment, POOL_LAYER_EDGE, cap_layers)
        .expect("aligned blocks that fit a pool layer");
    let mut targets = SimTargets {
        blocks,
        current: vec![None; block_count],
        next: vec![None; block_count],
        targeted: Vec::new(),
        next_list: Vec::new(),
        set: Vec::new(),
        remove: Vec::new(),
    };
    let mut in_mandatory = vec![0u64; block_count];
    let mut freed_at = vec![0u64; block_count];
    let mut ready = Vec::new();
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
        let mandatory = &inputs.mandatory[camera as usize];
        let band: &[u32] = if retain_band {
            &inputs.band[camera as usize]
        } else {
            &[]
        };
        for block in blocks.set_blocks(mandatory) {
            if i > 0 && in_mandatory[block as usize] != step - 1 {
                run.entering_blocks += 1;
                run.entering_resident += usize::from(model.is_resident(block));
            }
            in_mandatory[block as usize] = step;
        }
        targets.stage(mandatory, band);
        ready.clear();
        ready.extend(
            targets
                .targeted
                .iter()
                .copied()
                .filter(|&block| !model.is_resident(block)),
        );
        let plan = model.plan_drain(DrainRequest {
            pool_cap_layers: cap_layers,
            target_reset: None,
            target_set: &targets.set,
            target_remove: &targets.remove,
            ready: &ready,
        });
        for &block in &plan.installed {
            let bytes = blocks.block_bytes[block as usize];
            if in_mandatory[block as usize] == step {
                run.demand_reads += 1;
            } else {
                run.prefetch_reads += 1;
            }
            run.read_bytes += bytes;
            let freed = freed_at[block as usize];
            if freed > 0 && step - freed <= THRASH_WINDOW_STEPS {
                run.thrash_reads += 1;
                run.thrash_bytes += bytes;
            }
        }
        for eviction in &plan.evicted {
            freed_at[eviction.block as usize] = step;
            run.band_evictions += usize::from(eviction.reason == EvictionReason::Pressure);
        }
        run.repack_steps += usize::from(plan.report.repacked);
        run.growth_steps += usize::from(plan.report.grew);
        debug_assert!(plan.deferred.is_empty(), "every growth retires in its step");
        model.release_retirement();
        let occupied = model.occupied_layers();
        run.peak_layers = run.peak_layers.max(occupied as usize);
        run.over_cap_steps += usize::from(cap.is_some_and(|cap| occupied > cap));
    }
    run
}
