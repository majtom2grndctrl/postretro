// All-resident placement and block-table tests.

use super::*;
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_BC6H, LIGHTMAP_POOL_LAYER_EDGE, LightmapBlockIndex, LightmapBlockRecord,
    LightmapHeader, LightmapMode, SectionByteRange,
};
use postretro_level_format::shadowmask_atlas::{ShadowmaskBlockIndex, ShadowmaskBlockRecord};

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
    let placed: Vec<_> = pool
        .placements
        .iter()
        .copied()
        .map(|placement| Some(BlockAddresses::without_masks(placement)))
        .collect();
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
        Some(BlockAddresses::without_masks(BlockPlacement {
            layer: 1,
            x: 64,
            y: 128,
        })),
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
        let word = u32::from_ne_bytes(
            table[index * BLOCK_TABLE_ENTRY_BYTES + 12..index * BLOCK_TABLE_ENTRY_BYTES + 16]
                .try_into()
                .unwrap(),
        );
        assert_ne!(word >> 24 & 3, BLOCK_FLAG_RESIDENT | BLOCK_FLAG_NONE);
    }
}

#[test]
fn placeholder_table_is_one_entry_that_samples_the_placeholders() {
    let table = placeholder_block_table();
    assert_eq!(table.len(), BLOCK_TABLE_ENTRY_BYTES);
    assert_eq!(entry(&table, 0), BlockTableEntry::Placeholder);
    assert_eq!(
        BlockTableEntry::Placeholder.to_words()[3] >> 24,
        BLOCK_FLAG_RESIDENT | BLOCK_FLAG_NO_MASK_POOL,
        "the placeholder entry samples, unlike the no-lightmap entry, and reads \
         the fully-visible fallback"
    );
}

fn place(layer: u32, x: u32, y: u32) -> BlockPlacement {
    BlockPlacement { layer, x, y }
}

// The table layout, word by word, at the far end of every field: a 2048
// extent, a 2044 offset, and a layer past any device's.
#[test]
fn entries_pack_three_addresses_and_the_extent_as_laid_out() {
    let entry = BlockTableEntry::Resident {
        addresses: BlockAddresses {
            lightmap: place(16383, 2044, 4),
            masks: MaskAddresses::Planes([Some(place(1, 8, 2044)), Some(place(2, 12, 16))]),
        },
        width: 2048,
        height: 2048,
    };
    let words = entry.to_words();
    assert_eq!(words[0], 511 | (1 << 9) | (16383 << 18));
    assert_eq!(words[1], 2 | (511 << 9) | (1 << 18));
    assert_eq!(words[2], 3 | (4 << 9) | (2 << 18));
    let flags = BLOCK_FLAG_RESIDENT | BLOCK_FLAG_PLANE_A | BLOCK_FLAG_PLANE_B;
    assert_eq!(words[3], 2048 | (2048 << 12) | (flags << 24));
    assert_eq!(BlockTableEntry::from_words(words), entry);
}

#[test]
fn an_entry_tells_every_mask_state_apart() {
    let lightmap = place(3, 4, 8);
    let entry = |masks| BlockTableEntry::Resident {
        addresses: BlockAddresses { lightmap, masks },
        width: 16,
        height: 8,
    };
    let at = Some(place(0, 0, 0));
    let flag_sets = [
        (MaskAddresses::NoPool, BLOCK_FLAG_NO_MASK_POOL),
        (MaskAddresses::Planes([None, None]), 0),
        (MaskAddresses::Planes([at, None]), BLOCK_FLAG_PLANE_A),
        (MaskAddresses::Planes([None, at]), BLOCK_FLAG_PLANE_B),
        (
            MaskAddresses::Planes([at, at]),
            BLOCK_FLAG_PLANE_A | BLOCK_FLAG_PLANE_B,
        ),
    ];
    for (masks, flags) in flag_sets {
        let words = entry(masks).to_words();
        assert_eq!(words[3] >> 24, BLOCK_FLAG_RESIDENT | flags, "{masks:?}");
        assert_eq!(BlockTableEntry::from_words(words), entry(masks));
    }
    // An absent plane's address stays zero, so nothing can read a stale one.
    assert_eq!(entry(MaskAddresses::Planes([None, at])).to_words()[1], 0);
    assert_eq!(
        BlockTableEntry::Missing {
            width: 16,
            height: 8
        }
        .to_words()[3],
        16 | (8 << 12)
    );
}

fn block_index(extents: &[(u16, u16)]) -> LightmapBlockIndex {
    let none = SectionByteRange { offset: 0, len: 0 };
    LightmapBlockIndex {
        header: LightmapHeader {
            block_count: extents.len() as u32,
            direction_texel_scale: 2,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            mode: LightmapMode::Shadowed,
        },
        records: extents
            .iter()
            .map(|&(width, height)| LightmapBlockRecord {
                cell_id: 0,
                width,
                height,
                irradiance: none,
                direction: none,
            })
            .collect(),
    }
}

/// Id 42's records: each block's plane A, and plane B on the first `both`.
fn mask_index(blocks: usize, both: usize) -> ShadowmaskBlockIndex {
    let plane = |len| SectionByteRange { offset: 0, len };
    ShadowmaskBlockIndex {
        owners: Vec::new(),
        records: (0..blocks)
            .map(|block| ShadowmaskBlockRecord {
                group_a: plane(1),
                group_b: plane(u32::from(block < both)),
            })
            .collect(),
    }
}

fn first_generation(
    index: &LightmapBlockIndex,
    masks: Option<&ShadowmaskBlockIndex>,
    sizing: StreamedPoolSizing,
    device_layers: u32,
) -> PerPool<u32> {
    let plan = plan_streaming_pool(
        Some(index),
        masks,
        [0; 32],
        sizing,
        LIGHTMAP_POOL_LAYER_EDGE,
        device_layers,
    );
    let StaticPool::Streaming(plan) = plan else {
        panic!("expected a streamed pool, got {plan:?}");
    };
    plan.first_generation_layers
}

// A 2048² layer holds one of these and nothing beside it.
const FULL_LAYER: (u16, u16) = (2048, 2048);

#[test]
fn first_generation_is_bound_plus_margin_capped() {
    let index = block_index(&[FULL_LAYER; 20]);
    let masks = mask_index(20, 4);
    let sized = |lightmap, mask| {
        let bound = PerPool { lightmap, mask };
        let sizing = StreamedPoolSizing::from_bound(bound);
        let layers = first_generation(&index, Some(&masks), sizing, 2048);
        let model = LightmapPoolModel::new(
            PoolDemand::of_indices(&index, Some(&masks)),
            LIGHTMAP_POOL_LAYER_EDGE,
            layers,
        )
        .unwrap();
        for kind in PoolKind::ALL {
            assert_eq!(
                model.pool(kind).layers(),
                layers[kind],
                "the model takes it"
            );
        }
        layers
    };
    let layers = |lightmap, mask| PerPool { lightmap, mask };

    // All-resident is 20 lightmap and 24 mask layers. Each pool's bound stands
    // alone: the pools' worst cells can differ.
    assert_eq!(sized(8, 4), layers(10, 5), "bound times 5/4");
    assert_eq!(sized(7, 13), layers(9, 17), "rounded up, each its own");
    assert_eq!(sized(18, 21), layers(20, 24), "capped at all-resident");
    assert_eq!(
        sized(0, 0),
        layers(1, 1),
        "a pool with data takes one layer"
    );
}

// No mask pool, and no spare to pay for: a level with no id 42, and one with id
// 42 and no stored plane. A bound carried over from another load changes
// neither: the mask pool sizes from what this level's demand holds.
#[test]
fn a_mask_pool_with_nothing_to_hold_has_no_layers() {
    let index = block_index(&[FULL_LAYER; 3]);
    let sizing = StreamedPoolSizing::from_bound(PerPool {
        lightmap: 2,
        mask: 2,
    });
    let absent = ShadowmaskBlockIndex {
        owners: Vec::new(),
        records: vec![
            ShadowmaskBlockRecord {
                group_a: SectionByteRange { offset: 0, len: 0 },
                group_b: SectionByteRange { offset: 0, len: 0 },
            };
            3
        ],
    };
    for masks in [None, Some(&absent)] {
        let layers = first_generation(&index, masks, sizing, 2048);
        assert_eq!((layers.lightmap, layers.mask), (3, 0));
        let demand = PoolDemand::of_indices(&index, masks);
        let model = LightmapPoolModel::new(demand, LIGHTMAP_POOL_LAYER_EDGE, layers).unwrap();
        let mask = model.pool(PoolKind::Mask);
        assert_eq!((mask.layers(), mask.texture_allocations()), (0, 0));
    }
}

// Mask planes are one extent each, present planes only, at the block's
// alignment: a block with two planes counts twice, one with none not at all.
#[test]
fn all_resident_mask_layers_count_present_planes_only() {
    let index = block_index(&[FULL_LAYER; 6]);
    let layers = PoolDemand::of_indices(&index, Some(&mask_index(6, 2)))
        .all_resident(LIGHTMAP_POOL_LAYER_EDGE)
        .unwrap();
    assert_eq!((layers.lightmap, layers.mask), (6, 8));
}

// An override sets that pool's first generation and nothing else, past what the
// level would size itself and past all-resident alike. The device still bounds
// a pool beside its spare.
#[test]
fn a_harness_figure_sets_one_pools_first_generation() {
    let index = block_index(&[FULL_LAYER; 6]);
    let masks = mask_index(6, 2);
    let sizing = |lightmap, mask| StreamedPoolSizing {
        bound: PerPool {
            lightmap: 4,
            mask: 4,
        },
        first_generation_override: PerPool { lightmap, mask },
    };
    let sized = |lightmap, mask, device| {
        first_generation(&index, Some(&masks), sizing(lightmap, mask), device)
    };
    let layers = |lightmap, mask| PerPool { lightmap, mask };
    assert_eq!(sized(None, None, 2048), layers(5, 5));
    assert_eq!(
        sized(Some(2), None, 2048),
        layers(2, 5),
        "the lightmap pool alone"
    );
    assert_eq!(
        sized(None, Some(3), 2048),
        layers(5, 3),
        "the mask pool alone"
    );
    assert_eq!(sized(Some(9), Some(11), 2048), layers(9, 11));
    assert_eq!(
        sized(Some(9), None, 8),
        layers(7, 5),
        "the spare fits the device"
    );
}

// The identity recorded beside measurements must change with the sizing policy.
#[test]
fn pool_policy_identity_changed_with_the_sizing_policy() {
    let retired = ["demand-sized growth headroom", "4/3"].join(" ");
    assert_ne!(POOL_POLICY_IDENTITY, retired);
    assert!(
        !include_str!("policy/growth.rs").contains(&retired),
        "the retired policy string survives in the source"
    );
}
