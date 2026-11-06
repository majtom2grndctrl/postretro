use super::*;

fn order_of(extents: &[(u32, u32)]) -> Vec<u32> {
    let mut order: Vec<u32> = (0..extents.len() as u32).collect();
    order.sort_unstable_by_key(|&block| packing_order_key(block, extents[block as usize]));
    order
}

/// Layers the shelf allocator takes for `extents` in the order given.
fn layers_in_order(extents: &[(u32, u32)], edge: u32) -> u32 {
    let mut pool = BlockPool::new(edge, None);
    for &(width, height) in extents {
        pool.allocate(width, height).expect("every block fits");
    }
    pool.extent() as u32
}

// R24: block-id order packs this set in one layer and tallest first in two, so
// a cap counted in block-id order would sit a layer below the bound. The
// all-resident count must take the bound's order and see the second layer.
#[test]
fn all_resident_counts_the_bounds_order_not_block_id_order_r24() {
    let extents = [(56, 16), (28, 44), (24, 20)];
    assert_eq!(layers_in_order(&extents, 64), 1, "block-id order");
    assert_eq!(order_of(&extents), [1, 2, 0]);
    let tallest_first = [extents[1], extents[2], extents[0]];
    assert_eq!(layers_in_order(&tallest_first, 64), 2);
    assert_eq!(place_all_resident(&extents, 4, 64).unwrap().layer_count, 2);
}

#[test]
fn packing_order_breaks_a_height_tie_by_width_then_block_id() {
    assert_eq!(order_of(&[(8, 16), (12, 16), (8, 16)]), [1, 0, 2]);
}

#[test]
fn placements_come_back_by_block_id_whatever_the_packing_order() {
    let placements = place_all_resident(&[(56, 16), (28, 44), (24, 20)], 4, 64)
        .unwrap()
        .placements;
    let placed = |layer, x, y| BlockPlacement { layer, x, y };
    assert_eq!(placements[1], placed(0, 0, 0), "tallest");
    assert_eq!(placements[2], placed(0, 0, 44), "next, on its own shelf");
    assert_eq!(placements[0], placed(1, 0, 0), "widest, past the layer");
}

// The allocator sees slots, so the order must too: 6 and 8 share a slot height
// at alignment 4, and the wider block goes first.
#[test]
fn packing_order_ranks_aligned_slots_not_raw_extents() {
    let placements = place_all_resident(&[(4, 8), (8, 6)], 4, 16)
        .unwrap()
        .placements;
    let placed = |x| BlockPlacement { layer: 0, x, y: 0 };
    assert_eq!(placements[1], placed(0));
    assert_eq!(placements[0], placed(8));
}
