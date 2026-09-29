// Lightmap cell-block pool data logic: where each block's texels live in the
// pool layers, and the forward vertex stage's block table.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod block_allocator;
mod block_table;
mod policy;

pub use block_allocator::{BlockPool, RestoreConflict, ShelfLayer, Slot, StaleFree};
pub use block_table::{
    BLOCK_FLAG_NONE, BLOCK_FLAG_RESIDENT, BLOCK_TABLE_ENTRY_BYTES, BlockTableEntry,
    block_table_bytes, placeholder_block_table,
};
pub use policy::{
    BlockUpload, DrainPlan, DrainRequest, EvictionReason, LightmapPoolModel, NotAPlannedUpload,
    PlannedEviction, PoolCopy, PoolGrowth, TableWrite,
};

/// Top-left texel of a block inside the pool: array layer plus offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockPlacement {
    pub layer: u32,
    pub x: u32,
    pub y: u32,
}

/// Every block of a level placed at once: the all-resident pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllResidentPool {
    /// Where each block's texels live, indexed by block id.
    pub placements: Vec<BlockPlacement>,
    /// Array layers the pool needs to hold every placement.
    pub layer_count: u32,
}

/// Place every block, in block-id order, into square `edge`² layers with the
/// shelf allocator, opening layers as needed.
///
/// Each request is rounded up to `alignment` (the id-22 header's
/// `block_alignment`), so every origin is a sum of aligned extents and lands
/// on a multiple of it: BC blocks stay whole and direction offsets
/// (`offset / scale`) stay integral. `None` when a block is zero-sized or
/// larger than a layer; the loader rejects both.
pub fn place_all_resident(
    extents: &[(u32, u32)],
    alignment: u32,
    edge: u32,
) -> Option<AllResidentPool> {
    let alignment = alignment.max(1);
    let mut pool = BlockPool::new(edge, None);
    let placements = extents
        .iter()
        .map(|&(width, height)| {
            let slot = pool.allocate(
                width.checked_next_multiple_of(alignment)?,
                height.checked_next_multiple_of(alignment)?,
            )?;
            Some(BlockPlacement {
                layer: slot.layer,
                x: slot.x,
                y: slot.y,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(AllResidentPool {
        placements,
        layer_count: pool.extent() as u32,
    })
}

#[cfg(test)]
#[path = "lightmap_pool/tests.rs"]
mod tests;
