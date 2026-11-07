// Lightmap cell-block pool data logic: where each block's texels live in the
// pool layers, and the forward vertex stage's block table.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod block_table;
mod payload;
mod policy;
mod static_plan;

pub use block_table::{
    BLOCK_FLAG_NO_MASK_POOL, BLOCK_FLAG_NONE, BLOCK_FLAG_PLANE_A, BLOCK_FLAG_PLANE_B,
    BLOCK_FLAG_RESIDENT, BLOCK_TABLE_ENTRY_BYTES, BlockAddresses, BlockTableEntry, MaskAddresses,
    block_table_bytes, placeholder_block_table,
};
pub use payload::payload_mismatch;
pub use policy::{
    BlockUpload, CopyOrderStats, DrainPlan, DrainRequest, EvictionReason, LightmapPoolModel,
    NotAPlannedUpload, POOL_POLICY_IDENTITY, PlannedEviction, PoolCopy, PoolGrowth, PoolPlan,
    PoolState, RepackKind, TableWrite,
};
pub use postretro_level_format::lightmap_pool::{
    AllResidentMasks, AllResidentPool, BlockPlacement, BlockPool, PerPool, PlanePresence,
    PoolDemand, PoolKind, RestoreConflict, ShelfLayer, Slot, StaleFree, mask_item, mask_item_plane,
    packing_order_key, place_all_resident,
};
pub use static_plan::{
    StaticPool, StaticPoolPlan, StreamedPoolSizing, StreamingPoolPlan, plan_static_pool,
    plan_streaming_pool,
};

#[cfg(test)]
#[path = "lightmap_pool/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lightmap_pool/block_allocator_tests.rs"]
mod block_allocator_tests;
