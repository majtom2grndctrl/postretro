// Shelf allocator tests, moved with the allocator from the level compiler's
// residency dry run.

use super::{BlockPool, RestoreConflict, ShelfLayer, Slot, StaleFree};
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

#[test]
fn freeing_a_block_makes_its_space_reusable() {
    let mut pool = BlockPool::new(LIGHTMAP_POOL_LAYER_EDGE, Some(1));
    let whole = pool
        .allocate(2048, 2048)
        .expect("empty layer holds a full block");
    assert!(pool.allocate(4, 4).is_none(), "the one layer is full");
    pool.free(whole).unwrap();
    assert_eq!(pool.extent(), 0);
    assert!(pool.allocate(2048, 2048).is_some());

    // Two half-height shelves merge back into one full-height shelf.
    let mut layer = ShelfLayer::new(2048, 2048);
    let top = layer.allocate(2048, 1024, 0).unwrap();
    let bottom = layer.allocate(2048, 1024, 1).unwrap();
    assert!(layer.allocate(4, 4, 2).is_none());
    layer.free(top.0, top.1, 2048, 1024, 0).unwrap();
    layer.free(bottom.0, bottom.1, 2048, 1024, 1).unwrap();
    assert!(layer.is_empty());
    assert_eq!(layer.allocate(2048, 2048, 3), Some((0, 0)));

    // Freed spans merge within a shelf whichever order they free in.
    let mut layer = ShelfLayer::new(2048, 2048);
    let spans: Vec<_> = (0..3)
        .map(|owner| layer.allocate(680, 2048, owner).unwrap())
        .collect();
    assert!(layer.allocate(700, 4, 3).is_none());
    for i in [1, 0, 2] {
        layer
            .free(spans[i].0, spans[i].1, 680, 2048, i as u64)
            .unwrap();
    }
    assert_eq!(layer.allocate(2048, 2048, 4), Some((0, 0)));
}

#[test]
fn stale_double_and_mis_sized_frees_are_refused_without_freeing_the_new_owner() {
    let mut pool = BlockPool::new(LIGHTMAP_POOL_LAYER_EDGE, Some(1));
    let first = pool.allocate(1024, 1024).unwrap();
    pool.free(first).unwrap();
    assert_eq!(pool.free(first), Err(StaleFree), "double free");

    // The new owner reuses the old origin; the stale slot must not free it.
    let second = pool.allocate(1024, 1024).unwrap();
    assert_eq!(
        (second.layer, second.x, second.y),
        (first.layer, first.x, first.y)
    );
    assert_eq!(pool.free(first), Err(StaleFree), "stale free");
    let mis_sized = Slot {
        width: 512,
        ..second
    };
    assert_eq!(pool.free(mis_sized), Err(StaleFree), "wrong width");
    assert_eq!(pool.extent(), 1, "the new owner's block is still resident");
    pool.free(second).unwrap();
    assert_eq!(pool.extent(), 0);

    // An owner id outlives `clear`, so a pre-clear slot cannot free a new one.
    let before = pool.allocate(64, 64).unwrap();
    pool.clear();
    let after = pool.allocate(64, 64).unwrap();
    assert_eq!(pool.free(before), Err(StaleFree));
    pool.free(after).unwrap();
}

#[test]
fn zero_size_allocations_are_refused() {
    let mut pool = BlockPool::new(LIGHTMAP_POOL_LAYER_EDGE, None);
    assert!(pool.allocate(0, 16).is_none());
    assert!(pool.allocate(16, 0).is_none());
    assert_eq!(pool.extent(), 0, "no layer opened for a refused request");
    let mut layer = ShelfLayer::new(64, 64);
    assert!(layer.allocate(0, 0, 0).is_none());
    assert!(layer.is_empty());
}

#[test]
fn restore_undoes_frees_newest_first_even_after_their_shelves_merged() {
    let mut pool = BlockPool::new(64, Some(1));
    // `tall` opens a 24-high shelf and `short` shares it; `below` opens the
    // next shelf.
    let tall = pool.allocate(16, 24).unwrap();
    let short = pool.allocate(16, 16).unwrap();
    let below = pool.allocate(64, 8).unwrap();
    assert_eq!((short.y, below.y), (0, 24));
    for slot in [tall, short, below] {
        pool.free(slot).unwrap();
    }
    assert_eq!(pool.extent(), 0, "every shelf merged back");

    // Newest first. Restoring `short` rebuilds its shelf only 16 high, so
    // `tall` must borrow the empty rows beneath it.
    for slot in [below, short, tall] {
        pool.restore(slot).unwrap();
    }
    assert_eq!(pool.restore(tall), Err(RestoreConflict), "already occupied");
    assert!(
        pool.allocate(64, 40).is_none(),
        "the restored rows are taken"
    );
    // The restored slots keep their owners, so their frees are accepted.
    for slot in [tall, short, below] {
        pool.free(slot).unwrap();
    }
    let whole = pool.allocate(64, 64).unwrap();
    assert_eq!((whole.layer, whole.x, whole.y), (0, 0, 0));
}

// Shelf A (64 high) held V1 and V2; both freed, then P took its top 40 rows,
// leaving an empty 24-row shelf below. Restoring V1 needs those 24 rows, but
// P holds its span: the restore is refused before it borrows anything.
#[test]
fn a_refused_restore_leaves_the_layer_untouched() {
    let mut layer = ShelfLayer::new(128, 64);
    let v1 = layer.allocate(64, 64, 1).unwrap();
    let v2 = layer.allocate(64, 64, 2).unwrap();
    assert_eq!((v1, v2), ((0, 0), (64, 0)));
    layer.free(v1.0, v1.1, 64, 64, 1).unwrap();
    layer.free(v2.0, v2.1, 64, 64, 2).unwrap();
    assert_eq!(layer.allocate(128, 40, 3), Some((0, 0)));

    let before = layer.clone();
    assert_eq!(layer.restore(0, 0, 64, 64, 1), Err(RestoreConflict));
    assert_eq!(layer, before, "the empty rows below stay unborrowed");
    assert_eq!(
        layer.allocate(128, 24, 4),
        Some((0, 40)),
        "the 24 free rows still take a block"
    );
}
