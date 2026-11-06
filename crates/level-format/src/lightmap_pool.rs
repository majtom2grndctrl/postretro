// Shared CPU lightmap block placement for the compiler and renderer.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::cmp::Reverse;

mod block_allocator;
mod sizing;

pub use block_allocator::{BlockPool, RestoreConflict, ShelfLayer, Slot, StaleFree};
pub use sizing::{
    LIGHTMAP_POOL_MARGIN, MASK_POOL_LAYER_BYTES, PoolMargin, all_resident_pool_bytes,
    first_generation_layers, lightmap_pool_layer_bytes, streamed_pool_bytes,
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

/// A block's place in the one from-scratch packing order: tallest first, then
/// widest, then lowest block id. `extent` is the block's slot, rounded up to
/// the alignment. The all-resident pack, a compaction and the compiled bound
/// all sort by this key: a bound is a promise that the runtime's pack fits, so
/// the two must pack alike. Shelf packing wants its tall blocks first.
pub fn packing_order_key(block: u32, extent: (u32, u32)) -> (Reverse<u32>, Reverse<u32>, u32) {
    let (width, height) = extent;
    (Reverse(height), Reverse(width), block)
}

/// Place every block into square `edge`² layers with the shelf allocator, in
/// [`packing_order_key`] order, opening layers as needed. Placements come back
/// by block id whatever the order.
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
    let slots = extents
        .iter()
        .map(|&(width, height)| {
            Some((
                width.checked_next_multiple_of(alignment)?,
                height.checked_next_multiple_of(alignment)?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let mut order: Vec<u32> = (0..slots.len() as u32).collect();
    order.sort_unstable_by_key(|&block| packing_order_key(block, slots[block as usize]));
    let mut pool = BlockPool::new(edge, None);
    let mut placements = vec![
        BlockPlacement {
            layer: 0,
            x: 0,
            y: 0
        };
        slots.len()
    ];
    for block in order {
        let (width, height) = slots[block as usize];
        let slot = pool.allocate(width, height)?;
        placements[block as usize] = BlockPlacement {
            layer: slot.layer,
            x: slot.x,
            y: slot.y,
        };
    }
    Some(AllResidentPool {
        placements,
        layer_count: pool.extent() as u32,
    })
}

#[cfg(test)]
mod tests;
