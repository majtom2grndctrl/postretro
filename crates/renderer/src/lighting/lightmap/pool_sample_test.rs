// GPU readback of the forward shader's cell-block lightmap path: blocks placed
// at non-zero offsets on two pool layers through the renderer's own pool upload,
// read through `pool_sample_harness`.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_render_cpu::lightmap_pool::{
    AllResidentMasks, AllResidentPool, BlockAddresses, BlockPlacement, MaskAddresses,
    block_table_bytes, placeholder_block_table,
};

use super::pool::upload_static_pool;
use super::pool_sample_harness::{
    BoundLightmap, Probe, SCALE, assert_near, direction_texel, expected_texel, irradiance_texel,
    mask_byte, resident_probe, run_probes,
};
use super::test_fixtures::{FixtureTexels, block_fixture};
use super::upload::{upload_placeholder_direction, upload_placeholder_irradiance};
use super::{StaticPoolPlan, upload_placeholder_shadowmask};

use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};

/// Resident blocks 0–3 and a fifth, unplaced block 4. Block 1 sits flush
/// right of block 0 and block 3 flush below block 2, so a clamp that let a
/// bilinear tap leave its block would read the neighbour's distinct values.
const EXTENTS: [(u32, u32); 5] = [(8, 8), (12, 8), (8, 12), (8, 8), (8, 8)];
const PLACEMENTS: [BlockPlacement; 4] = [
    BlockPlacement {
        layer: 0,
        x: 16,
        y: 8,
    },
    BlockPlacement {
        layer: 0,
        x: 24,
        y: 8,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 20,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 32,
    },
];
const MISSING_BLOCK: u32 = 4;

/// Plane A at the block's lightmap place and plane B beside it, so a block's
/// two planes stay apart and its neighbour stays flush in both.
fn plane_places(place: BlockPlacement) -> [Option<BlockPlacement>; 2] {
    [
        Some(place),
        Some(BlockPlacement {
            x: place.x + 512,
            ..place
        }),
    ]
}

/// The two-layer pool holding blocks 0–3, and a table naming them resident
/// and block 4 missing.
fn fixture_pool(ctx: &GpuCtx) -> BoundLightmap {
    let resident = &EXTENTS[..PLACEMENTS.len()];
    let fixture = block_fixture(
        resident,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: Some(&mask_byte),
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: resident.to_vec(),
        pool: AllResidentPool {
            placements: PLACEMENTS.to_vec(),
            layer_count: 2,
        },
        masks: Some(AllResidentMasks {
            planes: PLACEMENTS.iter().copied().map(plane_places).collect(),
            layer_count: 2,
        }),
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    let mut placements: Vec<_> = PLACEMENTS
        .iter()
        .map(|&place| {
            Some(BlockAddresses {
                lightmap: place,
                masks: MaskAddresses::Planes(plane_places(place)),
            })
        })
        .collect();
    placements.push(None);
    BoundLightmap {
        irradiance: textures.irradiance,
        direction: textures.direction,
        shadowmask: textures.shadowmask.expect("the plan keeps the shadowmask"),
        table: block_table_bytes(&EXTENTS, &placements),
        mask_present: true,
    }
}

// Every resident block reads its own texels at pool offset + block-local
// texel on both layers, and a block-local coordinate past any edge clamps to
// the block's own edge texel instead of reaching the neighbour placed flush
// against it.
#[test]
fn resident_blocks_read_their_own_texels_on_both_layers_and_clamp_at_their_edges() {
    let Some(ctx) = bc_gpu_or_skip(
        "resident_blocks_read_their_own_texels_on_both_layers_and_clamp_at_their_edges",
    ) else {
        return;
    };
    let bound = fixture_pool(&ctx);
    let mut probes = Vec::new();
    let mut expected = Vec::new();
    for (block, &(width, height)) in EXTENTS[..PLACEMENTS.len()].iter().enumerate() {
        let center = |t: u32, size: u32| (t as f32 + 0.5) / size as f32;
        for y in 0..height {
            for x in 0..width {
                probes.push(resident_probe(
                    [center(x, width), center(y, height)],
                    block as u32,
                ));
                expected.push((block, x, y, "texel center"));
            }
        }
        // Past every edge, and just inside the far edges where an unclamped
        // bilinear tap would blend the neighbour in.
        let (w, h) = (width as f32, height as f32);
        for (uv, x, y) in [
            ([-0.4, center(2, height)], 0, 2),
            ([1.3, center(2, height)], width - 1, 2),
            ([1.0, center(3, height)], width - 1, 3),
            ([(w - 0.1) / w, center(1, height)], width - 1, 1),
            ([center(2, width), -0.25], 2, 0),
            ([center(2, width), 1.6], 2, height - 1),
            ([center(1, width), (h - 0.05) / h], 1, height - 1),
            ([-1.0, 2.0], 0, height - 1),
        ] {
            probes.push(resident_probe(uv, block as u32));
            expected.push((block, x, y, "clamped edge"));
        }
    }
    let results = run_probes(&ctx, &bound, &probes);
    for ((probe, result), &(block, x, y, what)) in probes.iter().zip(&results).zip(&expected) {
        let what = format!("block {block} local ({x}, {y}) [{what}] uv {:?}", probe.uv);
        let (irradiance, direction, mask) = expected_texel(block, x, y);
        assert_near(
            &result.irradiance[..3],
            &irradiance,
            1.0e-3,
            &format!("{what}: irradiance"),
        );
        assert_near(
            &result.direction[..2],
            &direction,
            1.0e-5,
            &format!("{what}: direction"),
        );
        assert_near(&result.mask, &mask, 1.0e-5, &format!("{what}: shadowmask"));
        assert!(!result.missing, "{what}: a resident block is not a miss");
        assert!(
            result.owners_readable,
            "{what}: a resident block is readable"
        );
        assert_near(
            &[result.owner_visibility, result.union_channel0_visibility],
            &[mask[0], mask[0]],
            1.0e-5,
            &format!("{what}: channel-0 selects"),
        );
    }
}

// A block whose entry is not resident reads zero irradiance and zero
// shadowmask, and its owners may not read the mask at all, so every owner's
// specular and the union visibility drop. The no-lightmap entry reads zero
// irradiance, the neutral direction and an all-visible shadowmask; that read
// is no owner's baked visibility, so owners may not read it either.
#[test]
fn missing_and_no_lightmap_entries_drop_or_neutralize_their_terms() {
    let Some(ctx) =
        bc_gpu_or_skip("missing_and_no_lightmap_entries_drop_or_neutralize_their_terms")
    else {
        return;
    };
    let bound = fixture_pool(&ctx);
    let probe = |block: u32, owner_position: u32| Probe {
        uv: [0.4, 0.6],
        block,
        owner_position,
    };
    let mut probes: Vec<Probe> = (0..4).map(|s| probe(MISSING_BLOCK + 1, s)).collect();
    probes.push(probe(0, 0));
    probes.push(probe(0, 3));
    // Past the table: the vertex stage reads entry 0.
    probes.push(probe(EXTENTS.len() as u32 + 30, 0));
    let results = run_probes(&ctx, &bound, &probes);
    for (position, result) in results[..4].iter().enumerate() {
        let what = format!("missing, owner {position}");
        assert_eq!(result.irradiance[..3], [0.0; 3], "{what}: irradiance");
        assert_eq!(result.mask, [0.0; 4], "{what}: shadowmask");
        assert!(result.missing, "{what}");
        assert!(!result.owners_readable, "{what}: a miss drops every owner");
        assert_eq!(result.owner_visibility, 0.0, "{what}: owner specular drops");
        assert_eq!(result.union_channel0_visibility, 0.0, "{what}: union drops");
    }
    for (what, result) in [
        ("no lightmap", &results[4]),
        ("no lightmap, owner 3", &results[5]),
        ("past the table", &results[6]),
    ] {
        assert_eq!(result.irradiance[..3], [0.0; 3], "{what}: irradiance");
        assert_eq!(
            result.direction,
            [0.5, 1.0, 0.5, 0.0],
            "{what}: neutral direction"
        );
        assert_eq!(result.mask, [1.0; 4], "{what}: all-visible shadowmask");
        assert!(!result.missing, "{what}");
        assert!(
            !result.owners_readable,
            "{what}: the no-lightmap entry drops every owner"
        );
        assert_eq!(result.owner_visibility, 0.0, "{what}: owner specular drops");
    }
}

// Placeholder mode matches the pre-block look: every vertex reads the white
// irradiance, +Y direction and all-visible shadowmask placeholders. The
// placeholder is no mask, so the mask-present bit is clear and no owner
// reads it; an all-visible texel must not light owners.
#[test]
fn placeholder_mode_reads_the_neutral_placeholders_for_every_block_id() {
    let Some(ctx) =
        bc_gpu_or_skip("placeholder_mode_reads_the_neutral_placeholders_for_every_block_id")
    else {
        return;
    };
    let bound = BoundLightmap {
        irradiance: upload_placeholder_irradiance(&ctx.device, &ctx.queue),
        direction: upload_placeholder_direction(&ctx.device, &ctx.queue),
        shadowmask: upload_placeholder_shadowmask(&ctx.device, &ctx.queue),
        table: placeholder_block_table(),
        mask_present: false,
    };
    let probes: Vec<Probe> = [0u32, 1, 9]
        .into_iter()
        .flat_map(|block| {
            [[0.0, 0.0], [0.7, 0.2], [1.0, 1.0]].map(|uv| Probe {
                uv,
                block,
                owner_position: 2,
            })
        })
        .collect();
    let results = run_probes(&ctx, &bound, &probes);
    for (probe, result) in probes.iter().zip(&results) {
        let what = format!("placeholder block {} uv {:?}", probe.block, probe.uv);
        assert_eq!(result.irradiance[..3], [1.0; 3], "{what}: white irradiance");
        assert_near(
            &result.direction[..2],
            &[128.0 / 255.0, 1.0],
            1.0e-5,
            &format!("{what}: +Y direction"),
        );
        assert_eq!(result.mask, [1.0; 4], "{what}: all-visible shadowmask");
        assert!(!result.missing, "{what}");
        assert!(!result.owners_readable, "{what}: no mask, no owner reads");
        assert_eq!(result.owner_visibility, 0.0, "{what}: no owner specular");
    }
}

/// Two blocks whose lightmap, plane A and plane B sit at three layers and
/// offsets, each block's three places another block's neighbours.
fn three_address_pool(ctx: &GpuCtx) -> (BoundLightmap, [(u32, u32); 2]) {
    let at = |layer, x, y| BlockPlacement { layer, x, y };
    let extents = [(8, 8), (12, 8)];
    let lightmap = [at(0, 16, 8), at(1, 1024, 512)];
    let planes = [
        [Some(at(2, 2040, 2040)), Some(at(1, 200, 96))],
        [Some(at(0, 200, 96)), Some(at(2, 16, 8))],
    ];
    let fixture = block_fixture(
        &extents,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: Some(&mask_byte),
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: extents.to_vec(),
        pool: AllResidentPool {
            placements: lightmap.to_vec(),
            layer_count: 2,
        },
        masks: Some(AllResidentMasks {
            planes: planes.to_vec(),
            layer_count: 3,
        }),
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    let bound = BoundLightmap {
        irradiance: textures.irradiance,
        direction: textures.direction,
        shadowmask: textures.shadowmask.expect("the plan keeps the shadowmask"),
        table: plan.table_bytes(),
        mask_present: true,
    };
    (bound, extents)
}

// A block reads its lightmap from one address, plane A from a second and plane
// B from a third: layers and offsets all differ, and each plane's channels
// come back from its own place.
#[test]
fn block_samples_three_addresses() {
    let Some(ctx) = bc_gpu_or_skip("block_samples_three_addresses") else {
        return;
    };
    let (bound, extents) = three_address_pool(&ctx);
    let mut probes = Vec::new();
    let mut expected = Vec::new();
    for (block, &(width, height)) in extents.iter().enumerate() {
        for y in 0..height {
            for x in 0..width {
                let center = |t: u32, size: u32| (t as f32 + 0.5) / size as f32;
                probes.push(resident_probe(
                    [center(x, width), center(y, height)],
                    block as u32,
                ));
                expected.push((block, x, y));
            }
        }
    }
    let results = run_probes(&ctx, &bound, &probes);
    assert_eq!(results.len(), probes.len());
    for (result, &(block, x, y)) in results.iter().zip(&expected) {
        let what = format!("block {block} local ({x}, {y})");
        let (irradiance, direction, mask) = expected_texel(block, x, y);
        assert_near(&result.irradiance[..3], &irradiance, 1.0e-3, &what);
        assert_near(&result.direction[..2], &direction, 1.0e-5, &what);
        assert_near(&result.mask, &mask, 1.0e-5, &what);
        assert!(result.owners_readable, "{what}");
    }
}

// The block states read as they did: in a level with masks the no-lightmap
// entry reads all-visible, a miss reads zero, and a resident block its own mask.
// In a level without id 42 a resident block reads the fully-visible fallback
// through the no-mask-pool flag, the no-lightmap entry the same, and a miss
// still zero. Without id 42 the mask-present bit is clear, so no owner reads
// any of it.
#[test]
fn shadowmask_block_states_read_as_before() {
    let Some(ctx) = bc_gpu_or_skip("shadowmask_block_states_read_as_before") else {
        return;
    };
    let with_masks = fixture_pool(&ctx);
    let probe = |block: u32| Probe {
        uv: [0.4, 0.6],
        block,
        owner_position: 1,
    };
    let probes = [probe(0), probe(MISSING_BLOCK + 1), probe(1), probe(99)];
    let results = run_probes(&ctx, &with_masks, &probes);
    let [none, miss, resident, past_the_table] = &results[..] else {
        panic!("four probes");
    };
    assert_eq!(none.mask, [1.0; 4], "no lightmap reads all-visible");
    assert!(!none.owners_readable && !none.missing);
    assert_eq!(miss.mask, [0.0; 4], "a miss reads zero");
    assert!(miss.missing && !miss.owners_readable);
    let (_, _, own) = expected_texel(0, 3, 4);
    assert_near(&resident.mask, &own, 1.0e-5, "a resident block");
    assert!(resident.owners_readable);
    assert_eq!(past_the_table.mask, [1.0; 4], "past the table is entry 0");

    let extents = [(8, 8), (8, 8)];
    let fixture = block_fixture(
        &extents,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: None,
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: extents.to_vec(),
        pool: AllResidentPool {
            placements: PLACEMENTS[..2].to_vec(),
            layer_count: 1,
        },
        masks: None,
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    assert!(textures.shadowmask.is_none(), "no id 42, no mask pool");
    let without_masks = BoundLightmap {
        irradiance: textures.irradiance,
        direction: textures.direction,
        shadowmask: upload_placeholder_shadowmask(&ctx.device, &ctx.queue),
        table: block_table_bytes(
            &extents,
            &[Some(BlockAddresses::without_masks(PLACEMENTS[0])), None],
        ),
        mask_present: false,
    };
    let results = run_probes(&ctx, &without_masks, &[probe(1), probe(0), probe(2)]);
    let [resident, none, miss] = &results[..] else {
        panic!("three probes");
    };
    assert_eq!(resident.mask, [1.0; 4], "the fully-visible fallback");
    assert_ne!(
        resident.irradiance[..3],
        [0.0; 3],
        "its lightmap still reads"
    );
    assert!(!resident.owners_readable, "no mask, no owner reads");
    assert_eq!(resident.owner_visibility, 0.0);
    assert_eq!(none.mask, [1.0; 4]);
    assert_eq!(miss.mask, [0.0; 4], "a miss still reads zero");
    assert!(miss.missing);
}
