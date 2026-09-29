// All-resident placement and block-table tests.

use super::*;
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

/// Decode entry `index` of a block table the way the vertex stage reads it.
fn entry(table: &[u8], index: usize) -> BlockTableEntry {
    let at = index * BLOCK_TABLE_ENTRY_BYTES;
    let words: [u32; 4] = std::array::from_fn(|w| {
        u32::from_ne_bytes(table[at + w * 4..at + w * 4 + 4].try_into().unwrap())
    });
    BlockTableEntry::from_words(words)
}

/// Large blocks that force a second layer, mixed with small ones, all at a
/// 4-texel alignment.
fn multi_layer_extents() -> Vec<(u32, u32)> {
    let mut extents = vec![(1024, 1024); 5];
    extents.extend([(60, 36), (8, 8), (1200, 400), (4, 1996), (340, 612)]);
    extents
}

#[test]
fn all_resident_placement_opens_layers_and_never_overlaps() {
    let extents = multi_layer_extents();
    let pool = place_all_resident(&extents, 4, LIGHTMAP_POOL_LAYER_EDGE).expect("every block fits");
    assert_eq!(pool.placements.len(), extents.len());
    assert!(pool.layer_count >= 2, "five 1024² blocks need two layers");
    for (a, (&pa, &ea)) in pool.placements.iter().zip(&extents).enumerate() {
        assert!(pa.layer < pool.layer_count);
        assert!(pa.x + ea.0 <= LIGHTMAP_POOL_LAYER_EDGE && pa.y + ea.1 <= LIGHTMAP_POOL_LAYER_EDGE);
        for (b, (&pb, &eb)) in pool.placements.iter().zip(&extents).enumerate().skip(a + 1) {
            let apart = pa.layer != pb.layer
                || pa.x + ea.0 <= pb.x
                || pb.x + eb.0 <= pa.x
                || pa.y + ea.1 <= pb.y
                || pb.y + eb.1 <= pa.y;
            assert!(apart, "blocks {a} and {b} overlap: {pa:?} {pb:?}");
        }
    }
}

#[test]
fn placements_land_on_multiples_of_the_block_alignment() {
    // Scale 8 direction: alignment lcm(4, 8) = 8. Extents are 8-aligned, as
    // the loader guarantees; every origin must stay a multiple of 8 so the
    // direction offset (origin / 8) and the BC block (origin / 4) are whole.
    let extents = [(24, 8), (8, 40), (16, 16), (2040, 8), (8, 2040), (64, 24)];
    let pool = place_all_resident(&extents, 8, LIGHTMAP_POOL_LAYER_EDGE).unwrap();
    for placement in &pool.placements {
        assert_eq!((placement.x % 8, placement.y % 8), (0, 0), "{placement:?}");
    }
    // An unaligned extent still gets an aligned origin for every later block.
    let pool = place_all_resident(&[(6, 6), (4, 4), (4, 4)], 4, 16).unwrap();
    for placement in &pool.placements {
        assert_eq!((placement.x % 4, placement.y % 4), (0, 0), "{placement:?}");
    }
}

#[test]
fn placement_refuses_a_block_larger_than_a_layer() {
    assert!(place_all_resident(&[(4, 4), (2052, 4)], 4, LIGHTMAP_POOL_LAYER_EDGE).is_none());
    assert!(place_all_resident(&[(0, 4)], 4, LIGHTMAP_POOL_LAYER_EDGE).is_none());
    assert_eq!(
        place_all_resident(&[], 4, LIGHTMAP_POOL_LAYER_EDGE),
        Some(AllResidentPool {
            placements: Vec::new(),
            layer_count: 0,
        })
    );
}

// AC 5, renderer half: the table plus the placement map each block-local
// texel to the pool texel `placement + local`, across layers.
#[test]
fn block_table_maps_each_block_local_texel_to_its_placement_plus_local() {
    let extents = multi_layer_extents();
    let pool = place_all_resident(&extents, 4, LIGHTMAP_POOL_LAYER_EDGE).unwrap();
    assert!(pool.placements.iter().any(|p| p.layer > 0));
    assert!(
        pool.placements
            .iter()
            .any(|p| p.layer == 0 && (p.x, p.y) != (0, 0))
    );
    let placed: Vec<_> = pool.placements.iter().copied().map(Some).collect();
    let table = block_table_bytes(&extents, &placed);
    assert_eq!(table.len(), (extents.len() + 1) * BLOCK_TABLE_ENTRY_BYTES);
    assert_eq!(entry(&table, 0), BlockTableEntry::None);

    for (block, (&(width, height), &placement)) in extents.iter().zip(&pool.placements).enumerate()
    {
        // Vertices name block `id + 1`.
        let resolved = entry(&table, block + 1);
        let samples = [
            (0, 0),
            (width - 1, 0),
            (0, height - 1),
            (width - 1, height - 1),
            (width / 2, height / 3),
        ];
        for (lx, ly) in samples {
            assert_eq!(
                resolved.pool_texel(lx, ly),
                Some((placement.layer, placement.x + lx, placement.y + ly)),
                "block {block} local ({lx}, {ly})"
            );
        }
        assert_eq!(resolved.pool_texel(width, 0), None, "past the extent");
    }
}

#[test]
fn a_missing_block_keeps_its_extent_and_samples_no_pool_texel() {
    let extents = [(16, 8), (32, 4)];
    let placements = [
        Some(BlockPlacement {
            layer: 1,
            x: 64,
            y: 128,
        }),
        None,
    ];
    let table = block_table_bytes(&extents, &placements);
    assert_eq!(
        entry(&table, 2),
        BlockTableEntry::Missing {
            width: 32,
            height: 4
        }
    );
    assert_eq!(entry(&table, 2).pool_texel(0, 0), None);
    assert_eq!(entry(&table, 1).pool_texel(3, 2), Some((1, 67, 130)));
    // Neither flag marks the miss; RESIDENT and NONE never both appear.
    for index in 0..3 {
        let flags = u32::from_ne_bytes(
            table[index * BLOCK_TABLE_ENTRY_BYTES + 12..index * BLOCK_TABLE_ENTRY_BYTES + 16]
                .try_into()
                .unwrap(),
        );
        assert_ne!(flags, BLOCK_FLAG_RESIDENT | BLOCK_FLAG_NONE);
    }
}

#[test]
fn placeholder_table_is_one_entry_that_samples_the_placeholders() {
    let table = placeholder_block_table();
    assert_eq!(table.len(), BLOCK_TABLE_ENTRY_BYTES);
    assert_eq!(entry(&table, 0), BlockTableEntry::Placeholder);
    assert_eq!(
        BlockTableEntry::Placeholder.to_words()[3],
        BLOCK_FLAG_RESIDENT,
        "the placeholder entry samples, unlike the no-lightmap entry"
    );
}
