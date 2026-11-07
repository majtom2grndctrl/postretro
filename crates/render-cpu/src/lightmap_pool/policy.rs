// Streamed lightmap pool placement policy: per-level two-pool model and per-drain plans.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod compaction;
mod copy_order;
mod growth;
mod journal;
mod pair;
mod placement;
mod plan;
mod pool_state;
mod repack;
mod reservation;

use postretro_level_format::lightmap_pool::{
    PerPool, PlanePresence, PoolDemand, PoolKind, mask_item,
};
use postretro_level_loader::{LightmapBlockClass, LightmapPoolReport, LightmapTarget};

use super::block_table_bytes;
use super::{BlockAddresses, BlockPlacement, BlockTableEntry, MaskAddresses, Slot};
use compaction::CompactionScratch;
pub use copy_order::CopyOrderStats;
pub use growth::POOL_POLICY_IDENTITY;
use journal::{JournalOp, Touched};
pub use plan::{
    BlockUpload, DrainPlan, EvictionReason, NotAPlannedUpload, PlannedEviction, PoolCopy,
    PoolGrowth, PoolPlan, RepackKind, TableWrite,
};
pub use pool_state::PoolState;
use repack::RepackEntry;

/// One drain's inputs, as the controller's `LightmapDrainBatch` carries them,
/// minus payload bytes. The caller validates the batch
/// (`LightmapDrainBatch::validate_contract`) before planning: block ids are
/// in range, deltas sorted and disjoint, ready ids unique.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrainRequest<'a> {
    pub target_reset: Option<&'a [LightmapTarget]>,
    pub target_set: &'a [LightmapTarget],
    pub target_remove: &'a [u32],
    /// Block ids of the pairs the shared drain budget admitted.
    pub ready: &'a [u32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    class: LightmapBlockClass,
    lead: u32,
}

impl Target {
    fn is_band(self) -> bool {
        self.class == LightmapBlockClass::Band
    }
}

/// Reused per-drain buffers. Owned by the model so a steady drain allocates
/// nothing.
#[derive(Debug, Default)]
struct Scratch {
    ready: Vec<u32>,
    never_refused: Vec<u32>,
    band_ready: Vec<u32>,
    /// A mandatory or visible target changed, or a block left the pool:
    /// reservation work is due this drain. Nothing else asks for it.
    demand_changed: bool,
    /// Items of one pool without room, while reservation runs.
    unseated: Vec<u32>,
    /// Resident band blocks, nearest lead first; eviction pops the farthest.
    victims: Vec<u32>,
    victims_built: bool,
    /// The pools the last failed placement found no room in, with every band
    /// block evicted.
    short: PerPool<bool>,
    /// Extents deferred at the device layer limit this drain, by pool.
    device_limited: PerPool<Vec<(u32, u32)>>,
    /// The drain's pairs' items in the pool a repack runs on.
    repack_pending: Vec<u32>,
    /// Resident and ready items a repack must seat.
    repack_keep: Vec<RepackEntry>,
    repack_band: Vec<(u32, u32)>,
    repack_waiting: Vec<u32>,
    repack_residents: Vec<u32>,
    moves: Vec<PoolCopy>,
    /// Aligned extents of the targeted demand, for growth sizing.
    demand: Vec<(u32, u32)>,
    compaction: CompactionScratch,
}

/// The renderer's model of one level's streamed pools: the lightmap pool, the
/// mask pool, and which block sits where in each.
///
/// A block has one lightmap placement and one mask placement per present
/// plane, and those halves are one atomic pair: they install in one drain or
/// not at all, and leave together. Each pool repacks, grows and retires alone,
/// and one journal covers both, so a pair or a drain undoes whole.
/// Each drain produces a [`DrainPlan`] the GPU layer executes in order; the
/// model has already applied it, and journals every mutation so the GPU
/// layer can [`fail_install`](Self::fail_install) one pair or
/// [`abort_drain`](Self::abort_drain) the whole drain.
#[derive(Debug)]
pub struct LightmapPoolModel {
    demand: PoolDemand,
    targets: Vec<Option<Target>>,
    pools: PerPool<PoolState>,
    #[cfg(test)]
    growth_sizing: growth::GrowthSizing,
    /// Repack as the pool did before compaction, so a test can show what
    /// compaction holds that the downward pass does not.
    #[cfg(test)]
    downward_only: bool,
    journal: Vec<JournalOp>,
    drain: u64,
    touch_stamp: Vec<u64>,
    touch_index: Vec<u32>,
    touched: Vec<Touched>,
    /// The drain a block's pair last came ready in.
    pair_stamp: Vec<u64>,
    /// The drain a block's pair was last deferred in.
    deferred_stamp: Vec<u64>,
    scratch: Scratch,
    plan: DrainPlan,
    /// Passes reserving room for the demand, for the proof that a drain with
    /// unchanged targets runs none.
    #[cfg(test)]
    demand_passes: u32,
}

impl LightmapPoolModel {
    /// A model for a level's blocks, nothing resident, with no device layer
    /// limit. Each pool's first generation holds `first_generation` layers,
    /// which the level's sizing or a harness chose; a pool with nothing to
    /// hold has none. `None` for a zero-sized block or one larger than an
    /// `edge`² layer; the loader rejects both.
    pub fn new(demand: PoolDemand, edge: u32, first_generation: PerPool<u32>) -> Option<Self> {
        Self::with_layer_limit(demand, edge, first_generation, u32::MAX)
    }

    /// [`new`](Self::new) bounded by the device: no active generation ever
    /// holds more than `max_layers` usable layers (the device's
    /// `maxTextureArrayLayers` minus the spare). A mandatory or visible pair
    /// that would need a layer past it is deferred, a counted miss that,
    /// unlike one while a generation retires, lasts until the resident set
    /// shrinks.
    pub fn with_layer_limit(
        demand: PoolDemand,
        edge: u32,
        first_generation: PerPool<u32>,
        max_layers: u32,
    ) -> Option<Self> {
        let pools = pool_state::new_pools(&demand, edge, first_generation, max_layers)?;
        let count = demand.block_count();
        Some(Self {
            demand,
            targets: vec![None; count],
            pools,
            #[cfg(test)]
            growth_sizing: growth::GrowthSizing::default(),
            #[cfg(test)]
            downward_only: false,
            journal: Vec::new(),
            drain: 0,
            touch_stamp: vec![0; count],
            touch_index: vec![0; count],
            touched: Vec::new(),
            pair_stamp: vec![0; count],
            deferred_stamp: vec![u64::MAX; count],
            scratch: Scratch::default(),
            plan: DrainPlan::default(),
            #[cfg(test)]
            demand_passes: 0,
        })
    }

    /// One pool's shape: layers, spare, generation and retirement.
    pub fn pool(&self, kind: PoolKind) -> &PoolState {
        &self.pools[kind]
    }

    /// The GPU layer reports a pool's retiring generation's submitted work
    /// done and drops it. That pool may grow again from the next drain.
    pub fn release_retirement(&mut self, kind: PoolKind) {
        self.pools[kind].retiring = false;
    }

    pub fn block_count(&self) -> u32 {
        self.demand.block_count() as u32
    }

    /// `block`'s true extent.
    pub fn extent(&self, block: u32) -> (u32, u32) {
        self.demand.extents()[block as usize]
    }

    /// Which planes `block` stores; `None` for a level with no mask pool.
    pub fn planes(&self, block: u32) -> Option<PlanePresence> {
        self.demand
            .has_mask_pool()
            .then(|| self.demand.planes(block))
    }

    pub fn is_resident(&self, block: u32) -> bool {
        self.pools.lightmap.slot(block).is_some()
    }

    /// Where a resident block's lightmap half sits.
    pub fn placement(&self, block: u32) -> Option<BlockPlacement> {
        self.pools.lightmap.slot(block).map(placement_of)
    }

    /// Where a resident block's `plane` sits; `None` when it stores none.
    pub fn plane_placement(&self, block: u32, plane: usize) -> Option<BlockPlacement> {
        self.pools
            .mask
            .slot(mask_item(block, plane))
            .map(placement_of)
    }

    /// Every address of a resident block.
    pub fn addresses(&self, block: u32) -> Option<BlockAddresses> {
        let lightmap = self.placement(block)?;
        let masks = match self.demand.has_mask_pool() {
            true => MaskAddresses::Planes([
                self.plane_placement(block, 0),
                self.plane_placement(block, 1),
            ]),
            false => MaskAddresses::NoPool,
        };
        Some(BlockAddresses { lightmap, masks })
    }

    /// Resident block ids, in no particular order.
    pub fn resident_blocks(&self) -> &[u32] {
        &self.pools.lightmap.resident
    }

    /// Table entry `block + 1` as the model holds it now.
    pub fn table_entry(&self, block: u32) -> BlockTableEntry {
        let (width, height) = self.demand.extents()[block as usize];
        match self.addresses(block) {
            Some(addresses) => BlockTableEntry::Resident {
                addresses,
                width,
                height,
            },
            None => BlockTableEntry::Missing { width, height },
        }
    }

    /// The whole table, for the pool's creation at level install. Later
    /// drains write only [`DrainPlan::table_writes`].
    pub fn table_bytes(&self) -> Vec<u8> {
        let addresses: Vec<Option<BlockAddresses>> =
            (0..self.block_count()).map(|b| self.addresses(b)).collect();
        block_table_bytes(self.demand.extents(), &addresses)
    }

    /// The last drain's plan, updated by `fail_install` and cleared by
    /// `abort_drain`.
    pub fn plan(&self) -> &DrainPlan {
        &self.plan
    }

    /// The pool halves of `block`: its lightmap item, then each present plane.
    fn halves(&self, block: u32) -> impl Iterator<Item = (PoolKind, u32)> + use<> {
        let planes = self.demand.planes(block);
        let mask = |plane: usize| planes[plane].then(|| (PoolKind::Mask, mask_item(block, plane)));
        [Some((PoolKind::Lightmap, block)), mask(0), mask(1)]
            .into_iter()
            .flatten()
    }

    /// Whether a half of `block` sits in `kind`.
    fn has_half_in(&self, kind: PoolKind, block: u32) -> bool {
        self.halves(block).any(|(half, _)| half == kind)
    }

    /// `block`'s extent rounded to its slot, the same in either pool.
    fn slot_extent(&self, block: u32) -> (u32, u32) {
        self.pools.lightmap.alloc_extent(block)
    }

    fn target(&self, block: u32) -> Option<Target> {
        self.targets[block as usize]
    }

    fn is_band(&self, block: u32) -> bool {
        self.target(block).is_some_and(Target::is_band)
    }

    /// Whether `block` is a mandatory or visible target.
    fn is_never_refused(&self, block: u32) -> bool {
        self.target(block).is_some_and(|t| !t.is_band())
    }

    /// Whether `block`'s pair is in this drain.
    fn is_pair(&self, block: u32) -> bool {
        self.pair_stamp[block as usize] == self.drain
    }

    fn report(&self, repacked: PerPool<bool>, grew: PerPool<bool>) -> LightmapPoolReport {
        PerPool {
            lightmap: self.pools.lightmap.report(repacked.lightmap, grew.lightmap),
            mask: self.pools.mask.report(repacked.mask, grew.mask),
        }
    }

    /// Capacities of every per-drain buffer, for the steady-drain allocation
    /// proof.
    #[cfg(test)]
    fn scratch_capacities(&self) -> Vec<usize> {
        let s = &self.scratch;
        let p = &self.plan;
        let mut capacities = s.compaction.capacities();
        for kind in PoolKind::ALL {
            capacities.extend(self.pools[kind].capacities());
            capacities.extend(p.pools[kind].capacities());
            capacities.push(s.device_limited[kind].capacity());
        }
        capacities.extend([
            self.journal.capacity(),
            self.touched.capacity(),
            s.ready.capacity(),
            s.never_refused.capacity(),
            s.band_ready.capacity(),
            s.unseated.capacity(),
            s.victims.capacity(),
            s.repack_pending.capacity(),
            s.repack_keep.capacity(),
            s.repack_band.capacity(),
            s.repack_waiting.capacity(),
            s.repack_residents.capacity(),
            s.moves.capacity(),
            s.demand.capacity(),
            p.uploads.capacity(),
            p.table_writes.capacity(),
            p.installed.capacity(),
            p.refused.capacity(),
            p.deferred.capacity(),
            p.failed.capacity(),
            p.evicted.capacity(),
        ]);
        capacities
    }
}

fn placement_of(slot: Slot) -> BlockPlacement {
    BlockPlacement {
        layer: slot.layer,
        x: slot.x,
        y: slot.y,
    }
}

#[cfg(test)]
mod gpu_mirror;
#[cfg(test)]
mod pair_tests;
#[cfg(test)]
mod per_pool_tests;
