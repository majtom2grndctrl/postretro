// One drain's placement plan: the ordered GPU work and the outcome lists.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolKind};
use postretro_level_loader::{LightmapPoolReport, LightmapRefusal};

use super::super::{BlockPlacement, BlockTableEntry};
use super::copy_order::CopyOrderStats;

/// Why a resident block was released this drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    /// It left every target.
    Untargeted,
    /// A band block at or past the cap's layers: the cap was lowered, or a
    /// block placed past it by growth dropped into the band.
    OverCap,
    /// A band block evicted, farthest lead first, to make room for a
    /// mandatory or visible pair, or dropped by a repack.
    Pressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedEviction {
    pub block: u32,
    pub reason: EvictionReason,
}

/// A new pool generation: `to_layers + 1` array layers (the last is the
/// spare). Copy layers `0..from_layers` of the old generation at origin zero,
/// then retire the old one until submitted-work-done. `to_layers` reserves
/// the whole targeted mandatory and visible demand with headroom, so it can
/// exceed what the drain's own pairs occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolGrowth {
    pub from_layers: u32,
    pub to_layers: u32,
}

/// A same-texture rectangle copy between two distinct array layers. Extents
/// are the block's aligned allocation, so every edge stays BC-aligned and
/// divides by the direction texel scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCopy {
    pub src: BlockPlacement,
    pub dst: BlockPlacement,
    pub width: u32,
    pub height: u32,
}

/// Which repack a drain ran. Downward runs first; compaction is its
/// escalation, so a drain reports at most one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepackKind {
    /// Blocks moved only to the same or a lower layer.
    Downward,
    /// Every block moved to any layer, in the order the copy planner found.
    Compaction(CopyOrderStats),
}

/// A ready pair to upload at its true extent: its lightmap half at `placement`,
/// and each present plane at its own place in the mask pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpload {
    pub block: u32,
    pub placement: BlockPlacement,
    pub planes: [Option<BlockPlacement>; 2],
    pub width: u32,
    pub height: u32,
}

/// Vertex block-table entry `index` (block id + 1) becomes `entry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableWrite {
    pub index: u32,
    pub entry: BlockTableEntry,
}

/// `fail_install` named a block this drain does not upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotAPlannedUpload;

/// What one drain asks of one pool. Nothing here reaches the other pool.
#[derive(Debug, Default)]
pub struct PoolPlan {
    pub growth: Option<PoolGrowth>,
    /// The repack that produced `copies`, if any.
    pub repack: Option<RepackKind>,
    /// A compaction packed the demand but found no copy order through the
    /// spare: the layout stands, and the demand fell through to growth.
    pub compaction_rolled_back: bool,
    pub copies: Vec<PoolCopy>,
    /// A pair was deferred because this pool holds the device's every usable
    /// layer. Later pairs this drain at least that large skipped the victim
    /// walk; smaller ones still walked.
    pub device_limited: bool,
    /// Items that hold reserved room as of this drain and did not before. A
    /// reservation is pool state, not a table entry: nothing here reaches the
    /// GPU. Sorted.
    pub room_reserved: Vec<u32>,
    /// Items that held reserved room and hold none now: their pair landed,
    /// they left targeted demand, or a repack could not seat them. Sorted.
    pub room_returned: Vec<u32>,
}

impl PoolPlan {
    fn clear(&mut self) {
        self.growth = None;
        self.repack = None;
        self.compaction_rolled_back = false;
        self.copies.clear();
        self.device_limited = false;
        self.room_reserved.clear();
        self.room_returned.clear();
    }

    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 3] {
        [
            self.copies.capacity(),
            self.room_reserved.capacity(),
            self.room_returned.capacity(),
        ]
    }
}

/// One drain, already applied to the model. The GPU layer executes it in
/// this order, all in one submission:
///
/// 1. each pool's `growth`: allocate the new generation and copy the live
///    layers.
/// 2. each pool's `copies`: the repack's moves, in order (never within one
///    layer). A compaction's order depends on running exactly as listed: a
///    later copy may overwrite an earlier one's source.
/// 3. `uploads`: each pair's texels, recorded in the same encoder after the
///    copies, since a new block may land where a moved one used to be.
/// 4. `table_writes`: only changed entries, one per block whatever moved. A
///    freed block's entry turns non-resident in the same batch that writes
///    texels into its old region.
///
/// The outcome lists map onto `LightmapDrainOutcome`: `installed`, `refused`,
/// `deferred` (payloads owned back), `evicted`, and each pool's `room_reserved`
/// and `room_returned`.
#[derive(Debug, Default)]
pub struct DrainPlan {
    pub pools: PerPool<PoolPlan>,
    pub uploads: Vec<BlockUpload>,
    pub table_writes: Vec<TableWrite>,
    /// Pairs sampleable from this drain on: every upload, plus any ready pair
    /// whose block was already resident (no upload).
    pub installed: Vec<u32>,
    /// Band pairs with no room under a cap clear of reserved room, turned away
    /// whole, and pairs no longer targeted.
    pub refused: Vec<LightmapRefusal>,
    /// Mandatory or visible pairs that needed growth of a pool whose generation
    /// was still retiring, or a layer past the device limit: a counted miss,
    /// deferred whole. Transient while retiring, and only once demand or pool
    /// fragmentation outgrew the last growth's reserve (a second growth in one
    /// drain lands exactly at need, and the all-resident clamp can remove
    /// headroom); at the device limit it lasts while that pair stays wanted and
    /// the resident set leaves it no room.
    pub deferred: Vec<u32>,
    /// Uploads the GPU layer failed and the model rolled back.
    pub failed: Vec<u32>,
    pub evicted: Vec<PlannedEviction>,
    pub report: LightmapPoolReport,
}

impl DrainPlan {
    /// Whether this drain asks the GPU layer for a new pool texture.
    pub fn allocates_texture(&self) -> bool {
        PoolKind::ALL
            .iter()
            .any(|&kind| self.pools[kind].growth.is_some())
    }

    /// Whether `kind` ran a repack this drain.
    pub fn repacked(&self) -> PerPool<bool> {
        PerPool {
            lightmap: self.pools.lightmap.repack.is_some(),
            mask: self.pools.mask.repack.is_some(),
        }
    }

    /// Whether `kind` grew this drain.
    pub fn grew(&self) -> PerPool<bool> {
        PerPool {
            lightmap: self.pools.lightmap.growth.is_some(),
            mask: self.pools.mask.growth.is_some(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.pools.lightmap.clear();
        self.pools.mask.clear();
        self.uploads.clear();
        self.table_writes.clear();
        self.installed.clear();
        self.refused.clear();
        self.deferred.clear();
        self.failed.clear();
        self.evicted.clear();
        self.report = LightmapPoolReport::default();
    }
}
