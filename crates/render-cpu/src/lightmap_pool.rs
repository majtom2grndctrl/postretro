// Lightmap cell-block pool data logic: where each block's texels live in the
// pool layers, and the forward vertex stage's block table.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod block_table;
mod policy;

pub use block_table::{
    BLOCK_FLAG_NONE, BLOCK_FLAG_RESIDENT, BLOCK_TABLE_ENTRY_BYTES, BlockTableEntry,
    block_table_bytes, placeholder_block_table,
};
pub use policy::{
    BlockUpload, DrainPlan, DrainRequest, EvictionReason, LightmapPoolModel, NotAPlannedUpload,
    PlannedEviction, PoolCopy, PoolGrowth, TableWrite,
};
pub use postretro_level_format::lightmap_pool::{
    AllResidentPool, BlockPlacement, BlockPool, RestoreConflict, ShelfLayer, Slot, StaleFree,
    place_all_resident,
};

#[cfg(test)]
#[path = "lightmap_pool/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lightmap_pool/block_allocator_tests.rs"]
mod block_allocator_tests;
