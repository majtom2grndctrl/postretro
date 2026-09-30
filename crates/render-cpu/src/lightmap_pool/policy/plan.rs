// One drain's placement plan: the ordered GPU work and the outcome lists.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::LightmapPoolReport;

use super::super::{BlockPlacement, BlockTableEntry};

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
/// then retire the old one until submitted-work-done.
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

/// A ready pair to upload at `placement`, at its true extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpload {
    pub block: u32,
    pub placement: BlockPlacement,
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

/// One drain, already applied to the model. The GPU layer executes it in
/// field order, all in one submission:
///
/// 1. `growth`: allocate the new generation and copy the live layers.
/// 2. `copies`: the repack's moves, in order (never within one layer).
/// 3. `uploads`: each pair's texels, recorded in the same encoder after the
///    copies, since a new block may land where a moved one used to be.
/// 4. `table_writes`: only changed entries. A freed block's entry turns
///    non-resident in the same batch that writes texels into its old region.
///
/// The outcome lists map onto `LightmapDrainOutcome`: `installed`, `refused`,
/// `deferred` (payloads owned back) and `evicted`.
#[derive(Debug, Default)]
pub struct DrainPlan {
    pub growth: Option<PoolGrowth>,
    pub copies: Vec<PoolCopy>,
    pub uploads: Vec<BlockUpload>,
    pub table_writes: Vec<TableWrite>,
    /// Pairs sampleable from this drain on: every upload, plus any ready pair
    /// whose block was already resident (no upload).
    pub installed: Vec<u32>,
    /// Band pairs with no room under the cap, and pairs no longer targeted.
    pub refused: Vec<u32>,
    /// Mandatory or visible pairs that needed growth while a generation was
    /// still retiring, or a layer past the device limit: a counted transient
    /// miss.
    pub deferred: Vec<u32>,
    /// Uploads the GPU layer failed and the model rolled back.
    pub failed: Vec<u32>,
    pub evicted: Vec<PlannedEviction>,
    pub report: LightmapPoolReport,
}

impl DrainPlan {
    /// Whether this drain asks the GPU layer for a new pool texture.
    pub fn allocates_texture(&self) -> bool {
        self.growth.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.growth = None;
        self.copies.clear();
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
