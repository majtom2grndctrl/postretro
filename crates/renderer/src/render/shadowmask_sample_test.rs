// GPU readback of the forward shader's shadowmask sampling helper and both
// consumers' owner-channel selects, against hand-built BC5 cell blocks placed in
// the mask pool and against the placeholder, uploaded through the renderer's own
// pool upload and resolved through the real block table. The harness is
// `shadowmask_sample_harness`.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)

use postretro_render_cpu::lightmap_pool::{
    AllResidentMasks, AllResidentPool, BlockAddresses, BlockPlacement, MaskAddresses,
    block_table_bytes, placeholder_block_table,
};

use super::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use super::shadowmask_sample_harness::{NOT_AN_OWNER, Probe, ProbeResult, run_probes};

use crate::lighting::lightmap::test_fixtures::{FixtureTexels, block_fixture, constant_bc5_plane};
use crate::lighting::lightmap::{
    StaticPool, StaticPoolPlan, plan_static_pool, upload_placeholder_shadowmask, upload_static_pool,
};

/// Two 8×8 cell blocks, one per pool layer, at non-zero pool offsets.
const BLOCK_EDGE: u32 = 8;
const BLOCK_COUNT: u32 = 2;
const PLACEMENTS: [BlockPlacement; 2] = [
    BlockPlacement {
        layer: 0,
        x: 16,
        y: 8,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 20,
    },
];

/// The raw byte channel `channel` holds in BC block (`bx`, `by`) of cell
/// `block`. Distinct per channel, BC block and cell block, and constant within
/// a BC block, so BC4 reproduces it exactly and any misrouted read shows as a
/// wrong value.
fn mask_byte(channel: u32, bx: u32, by: u32, block: u32) -> u8 {
    (10 + channel * 50 + bx * 20 + by * 7 + block * 3) as u8
}

/// A block's two planes, apart in the mask pool.
fn plane_places(place: BlockPlacement) -> [Option<BlockPlacement>; 2] {
    [
        Some(place),
        Some(BlockPlacement {
            x: place.x + 512,
            ..place
        }),
    ]
}

/// The two cell blocks in the mask pool, plane A holding channels 0/1 and plane
/// B channels 2/3, and the block table naming their placements.
fn fixture_pool(ctx: &GpuCtx) -> (wgpu::Texture, Vec<u8>) {
    let extents = [(BLOCK_EDGE, BLOCK_EDGE); BLOCK_COUNT as usize];
    let mask =
        |block: usize, channel: u32, bx: u32, by: u32| mask_byte(channel, bx, by, block as u32);
    let fixture = block_fixture(
        &extents,
        2,
        &FixtureTexels {
            irradiance: &|_, _, _| [0.0; 4],
            direction: &|_, _, _| [128, 255],
            shadowmask: Some(&mask),
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: extents.to_vec(),
        pool: AllResidentPool {
            placements: PLACEMENTS.to_vec(),
            layer_count: 2,
        },
        masks: Some(AllResidentMasks {
            planes: PLACEMENTS.iter().copied().map(plane_places).collect(),
            extents: vec![[(BLOCK_EDGE, BLOCK_EDGE); 2]; BLOCK_COUNT as usize],
            layer_count: 2,
        }),
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    let table = block_table_bytes(
        &extents,
        &[[(BLOCK_EDGE, BLOCK_EDGE); 2]; BLOCK_COUNT as usize],
        &PLACEMENTS.map(|place| {
            Some(BlockAddresses {
                lightmap: place,
                masks: MaskAddresses::Planes(plane_places(place)),
            })
        }),
    );
    (
        textures.shadowmask.expect("the plan keeps the shadowmask"),
        table,
    )
}

fn expected_mask(texel_x: u32, texel_y: u32, block: u32) -> [f32; 4] {
    std::array::from_fn(|channel| {
        f32::from(mask_byte(channel as u32, texel_x / 4, texel_y / 4, block)) / 255.0
    })
}

fn texel_center(texel: u32, size: u32) -> f32 {
    (texel as f32 + 0.5) / size as f32
}

fn assert_near(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= 1.0e-5,
        "{what}: got {actual}, expected {expected}"
    );
}

/// Both consumers select owner position `position`'s own mask channel; a
/// light that owns nothing reads no channel and the union skips it. For an
/// owner, the union's attenuation is wired to that channel: all of it at
/// entity visibility 0, none at 1. Where owners are unreadable (the
/// no-lightmap entry), world specular gives every owner zero visibility.
fn assert_selects(result: &ProbeResult, position: u32, owners_readable: bool, what: &str) {
    if position == NOT_AN_OWNER {
        assert_eq!(
            result.spec_visibility, 0.0,
            "{what}: a non-owner adds no specular"
        );
        assert_eq!(
            result.union_channel, NOT_AN_OWNER,
            "{what}: the promoted union must skip a light that does not own the face"
        );
        return;
    }
    let baked_vis = result.mask[position as usize];
    let spec_visibility = if owners_readable { baked_vis } else { 0.0 };
    assert_near(
        result.spec_visibility,
        spec_visibility,
        &format!("{what}: world specular"),
    );
    assert_eq!(
        result.union_channel, position,
        "{what}: promoted union channel"
    );
    assert_near(
        result.union_visibility,
        baked_vis,
        &format!("{what}: promoted union"),
    );
    assert!(
        baked_vis > 0.0,
        "{what}: the fixture mask must be nonzero so attenuation can show it"
    );
    assert_eq!(
        result.union_attenuation_at_entity_visibility_one, 0.0,
        "{what}: static→static union attenuation must be exactly zero"
    );
    assert_near(
        result.union_attenuation_at_entity_visibility_zero,
        baked_vis,
        &format!("{what}: a fully occluded entity attenuates by the channel's own mask"),
    );
}

// Pins: second-group-light, plane-outer-halftexel. M1 (union half), M3, M4
// (union half). The driven u = 0 / u = 1 probes hold each plane to its own
// block: the clamp keeps them off the empty pool texels beside it.
#[test]
fn every_owner_position_reads_its_own_channel_at_centers_block_edges_and_outer_uv() {
    let Some(ctx) = bc_gpu_or_skip(
        "every_owner_position_reads_its_own_channel_at_centers_block_edges_and_outer_uv",
    ) else {
        return;
    };
    let (texture, table) = fixture_pool(&ctx);

    let mut probes = Vec::new();
    let mut expectations = Vec::new();
    for block in 0..BLOCK_COUNT {
        for &texel_y in &[0, 5] {
            let v = texel_center(texel_y, BLOCK_EDGE);
            // Texel centers either side of the BC block edge inside each plane.
            for &texel_x in &[0, 3, 4, BLOCK_EDGE - 1] {
                let u = texel_center(texel_x, BLOCK_EDGE);
                expectations.push((
                    expected_mask(texel_x, texel_y, block),
                    true,
                    format!("texel ({texel_x}, {texel_y}) block {block}"),
                ));
                probes.push(([u, v], block + 1));
            }
            // Driven outer UVs a baked chart gutter never reaches: each plane
            // must clamp to its own block's edge column, never blend into the
            // pool texels beside the block.
            for (u, texel_x) in [
                (0.0, 0),
                (0.2 / BLOCK_EDGE as f32, 0),
                (1.0, BLOCK_EDGE - 1),
                (1.0 - 0.2 / BLOCK_EDGE as f32, BLOCK_EDGE - 1),
                (-0.5, 0),
                (1.5, BLOCK_EDGE - 1),
            ] {
                expectations.push((
                    expected_mask(texel_x, texel_y, block),
                    true,
                    format!("u = {u}, row {texel_y}, block {block}"),
                ));
                probes.push(([u, v], block + 1));
            }
        }
    }
    // A block id past the table resolves to the no-lightmap entry: it reads
    // all visible, and that read is no owner's baked visibility, so world
    // specular drops every owner there.
    probes.push((
        [texel_center(1, BLOCK_EDGE), texel_center(1, BLOCK_EDGE)],
        BLOCK_COUNT + 7,
    ));
    expectations.push(([1.0; 4], false, "id past the table".to_string()));

    let positions = [0u32, 1, 2, 3, NOT_AN_OWNER];
    let expanded: Vec<Probe> = probes
        .iter()
        .flat_map(|&(uv, block)| {
            positions
                .iter()
                .map(move |&position| Probe::new(uv, block, position))
        })
        .collect();
    let results = run_probes(&ctx, &texture, &table, &expanded, true);

    for (index, result) in results.iter().enumerate() {
        let (expected, owners_readable, what) = &expectations[index / positions.len()];
        let position = positions[index % positions.len()];
        for (component, (&actual, &want)) in result.mask.iter().zip(expected).enumerate() {
            assert_near(actual, want, &format!("{what}: channel {component} mask"));
        }
        assert_selects(
            result,
            position,
            *owners_readable,
            &format!("{what}, position {position}"),
        );
    }
}

// Pin: placeholder-second-group. Every channel reads fully visible at any UV
// and for any block id, through placeholder mode's one-entry table and its
// no-mask-pool flag. That read is no mask: the mask-present bit is clear, so
// world specular gives no owner a visibility, and only the union's bit gate
// (pinned by shader_tests) keeps it from subtracting.
#[test]
fn placeholder_reads_fully_lit_for_every_channel_and_block() {
    let Some(ctx) = bc_gpu_or_skip("placeholder_reads_fully_lit_for_every_channel_and_block")
    else {
        return;
    };
    let texture = upload_placeholder_shadowmask(&ctx.device, &ctx.queue);
    assert_eq!(texture.depth_or_array_layers(), 1);
    let table = placeholder_block_table();
    let probes: Vec<Probe> = [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [0.97, 0.03]]
        .into_iter()
        .flat_map(|uv| {
            [0u32, 3].into_iter().flat_map(move |block| {
                [0u32, 1, 2, 3]
                    .into_iter()
                    .map(move |position| Probe::new(uv, block, position))
            })
        })
        .collect();
    let results = run_probes(&ctx, &texture, &table, &probes, false);
    for (probe, result) in probes.iter().zip(&results) {
        let what = format!(
            "placeholder uv {:?} block {} position {}",
            probe.uv, probe.block, probe.position
        );
        assert!(
            result.mask.iter().all(|&m| m == 1.0),
            "{what}: mask {:?}",
            result.mask
        );
        assert_eq!(
            result.spec_visibility, 0.0,
            "{what}: no mask present, so no owner reads the placeholder"
        );
        assert_eq!(result.union_channel, probe.position, "{what}");
    }
}

/// `(stored, absent)` block pairs: the same texels with the all-zero plane
/// stored, then with no blob for it. Blocks 0 and 1 have plane B zero, 2 and 3
/// plane A, 4 and 5 both.
const ABSENT_PAIRS: [(u32, u32); 3] = [(0, 1), (2, 3), (4, 5)];

/// Six blocks in the real pool plan: each pair's planes hold the same bytes,
/// and each odd block stores no blob for its zero planes.
fn absent_plane_pool(ctx: &GpuCtx) -> (wgpu::Texture, Vec<u8>) {
    let extents = [(BLOCK_EDGE, BLOCK_EDGE); 6];
    let zero_plane = |block: usize, group: u32| match block / 2 {
        0 => group == 1,
        1 => group == 0,
        _ => true,
    };
    let mask = |block: usize, channel: u32, bx: u32, by: u32| {
        // A pair shares its bytes, whichever block holds them.
        match zero_plane(block, channel / 2) {
            true => 0,
            false => mask_byte(channel, bx, by, 0),
        }
    };
    let presence: Vec<[bool; 2]> = (0..extents.len())
        .map(|block| [0, 1].map(|group| block % 2 == 0 || !zero_plane(block, group)))
        .collect();
    let fixture = block_fixture(
        &extents,
        2,
        &FixtureTexels {
            irradiance: &|_, _, _| [0.0; 4],
            direction: &|_, _, _| [128, 255],
            shadowmask: Some(&mask),
        },
    )
    .with_planes(&presence);
    let StaticPool::Blocks(plan) = plan_static_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        &fixture.payloads,
        8192,
        256,
    ) else {
        panic!("the pool plans");
    };
    let masks = plan.masks.as_ref().expect("the mask pool plans");
    assert_eq!(masks.planes[5], [None, None], "block 5 has no planes");
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    (
        textures.shadowmask.expect("planes need the mask pool"),
        plan.table_bytes(),
    )
}

// An absent plane reads as the all-zero plane it replaces: the sampled mask, the
// owner's world specular, and the union's select and attenuation match bit for
// bit, for a light owning any channel and a light that owns nothing. Owners on
// channels 0 and 1 keep their plane A values with plane B absent.
#[test]
fn absent_plane_samples_as_stored_zero() {
    let Some(ctx) = bc_gpu_or_skip("absent_plane_samples_as_stored_zero") else {
        return;
    };
    let (texture, table) = absent_plane_pool(&ctx);
    let uvs = [
        [0.0, 0.0],
        [texel_center(1, BLOCK_EDGE), texel_center(6, BLOCK_EDGE)],
        [0.5, 0.5],
        [1.0, 1.0],
    ];
    let positions = [0u32, 1, 2, 3, NOT_AN_OWNER];
    let probes = |block: u32| -> Vec<Probe> {
        uvs.iter()
            .flat_map(|&uv| {
                positions
                    .iter()
                    .map(move |&position| Probe::new(uv, block + 1, position))
            })
            .collect()
    };
    for (stored, absent) in ABSENT_PAIRS {
        let with_blob = run_probes(&ctx, &texture, &table, &probes(stored), true);
        let without = run_probes(&ctx, &texture, &table, &probes(absent), true);
        assert_eq!(with_blob.len(), uvs.len() * positions.len());
        assert_eq!(with_blob, without, "block {stored} against block {absent}");
    }

    // Plane B absent: channels 0 and 1 read plane A's bytes, 2 and 3 zero.
    let results = run_probes(&ctx, &texture, &table, &probes(1), true);
    for (index, result) in results.iter().enumerate() {
        let position = positions[index % positions.len()];
        let what = format!("plane B absent, position {position}");
        assert_eq!(result.mask[2..], [0.0; 2], "{what}");
        assert!(result.mask[0] > 0.0 && result.mask[1] > 0.0, "{what}");
        match position {
            0 | 1 => {
                let expected = result.mask[position as usize];
                assert_near(result.spec_visibility, expected, &what);
                assert_near(result.union_visibility, expected, &what);
                assert_near(
                    result.union_attenuation_at_entity_visibility_zero,
                    expected,
                    &what,
                );
            }
            2 | 3 => {
                assert_eq!(result.spec_visibility, 0.0, "{what}");
                assert_eq!(result.union_visibility, 0.0, "{what}");
                assert_eq!(result.union_attenuation_at_entity_visibility_zero, 0.0);
            }
            _ => assert_eq!(result.union_channel, NOT_AN_OWNER, "{what}"),
        }
    }
}

/// A 16×8 lightmap block whose one owned face sits at lightmap x 8..16, its
/// mask chart at mask x 0..8 (offset -8, 0). The mask block is 8×8: plane A
/// whole, its BC column 0 the face's own padded chart and column 1 an
/// abutting chart; plane B a 4×4 sub-rect. Plane A sits flush right of plane
/// B in the mask pool, so a plane-B read past its sub-rect would land on plane
/// A's texels.
const SUB_RECT_BLOCK: (u32, u32) = (16, 8);
const SUB_RECT_OFFSET: [i16; 2] = [-8, 0];
const PLANE_B_PLACE: BlockPlacement = BlockPlacement {
    layer: 0,
    x: 64,
    y: 8,
};
const PLANE_A_PLACE: BlockPlacement = BlockPlacement {
    layer: 0,
    x: 68,
    y: 8,
};

fn plane_a_byte(channel: u32, bx: u32, by: u32) -> u8 {
    (40 + channel * 30 + bx * 100 + by * 7) as u8
}

fn plane_b_byte(channel: u32) -> u8 {
    (200 + channel * 10) as u8
}

fn sub_rect_pool(ctx: &GpuCtx) -> (wgpu::Texture, Vec<u8>) {
    let extents = [SUB_RECT_BLOCK];
    let mut fixture = block_fixture(
        &extents,
        2,
        &FixtureTexels {
            irradiance: &|_, _, _| [0.0; 4],
            direction: &|_, _, _| [128, 255],
            shadowmask: Some(&|_, _, _, _| 0),
        },
    );
    fixture.payloads[0].shadowmask = Some([
        constant_bc5_plane(8, 8, plane_a_byte),
        constant_bc5_plane(4, 4, |channel, _, _| plane_b_byte(channel)),
    ]);
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: extents.to_vec(),
        pool: AllResidentPool {
            placements: vec![BlockPlacement {
                layer: 0,
                x: 0,
                y: 0,
            }],
            layer_count: 1,
        },
        masks: Some(AllResidentMasks {
            planes: vec![[Some(PLANE_A_PLACE), Some(PLANE_B_PLACE)]],
            extents: vec![[(8, 8), (4, 4)]],
            layer_count: 1,
        }),
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    (
        textures.shadowmask.expect("the plan keeps the shadowmask"),
        plan.table_bytes(),
    )
}

/// The probe at lightmap texel `(x, y)` of the sub-rect fixture's block.
fn sub_rect_probe(x: f32, y: f32) -> Probe {
    let (width, height) = SUB_RECT_BLOCK;
    Probe::new([x / width as f32, y / height as f32], 1, 0).offset_by(SUB_RECT_OFFSET)
}

/// Plane A's value at mask BC block `(bx, by)` and plane B's one value.
fn sub_rect_mask(bx: u32, by: u32) -> [f32; 4] {
    [
        plane_a_byte(0, bx, by),
        plane_a_byte(1, bx, by),
        plane_b_byte(0),
        plane_b_byte(1),
    ]
    .map(|byte| f32::from(byte) / 255.0)
}

// A face reads its mask through its offset, and a filtered sample at its
// chart's border, out to its padding ring's texel centres, reads only its own
// chart's texels, never the chart abutting it in the mask block.
#[test]
fn a_border_sample_reads_its_own_padding_not_the_abutting_chart() {
    let Some(ctx) = bc_gpu_or_skip("a_border_sample_reads_its_own_padding_not_the_abutting_chart")
    else {
        return;
    };
    let (texture, table) = sub_rect_pool(&ctx);
    // Lightmap texel x 8.5..11.5 is mask x 0.5..3.5: the face's own chart,
    // BC column 0, out to its last padding texel's centre.
    let cases = [
        (
            sub_rect_probe(8.5, 1.5),
            sub_rect_mask(0, 0),
            "own chart, first texel",
        ),
        (
            sub_rect_probe(11.5, 1.5),
            sub_rect_mask(0, 0),
            "own padding ring centre",
        ),
        (
            sub_rect_probe(11.5, 6.5),
            sub_rect_mask(0, 1),
            "own padding, lower BC row",
        ),
        (
            sub_rect_probe(13.5, 6.5),
            sub_rect_mask(1, 1),
            "the abutting chart's texel",
        ),
    ];
    let probes: Vec<Probe> = cases.iter().map(|case| case.0).collect();
    let results = run_probes(&ctx, &texture, &table, &probes, true);
    for (result, (_, expected, what)) in results.iter().zip(&cases) {
        for (channel, (&actual, &want)) in result.mask[..2].iter().zip(expected).enumerate() {
            assert_near(actual, want, &format!("{what}: channel {channel}"));
        }
    }
}

// Each plane clamps to its own extent. Plane B, a 4×4 sub-rect, clamps
// to its own edge texel wherever plane A's wider extent would let a sample
// reach, so it never reads the plane-A placement flush beside it in the pool.
#[test]
fn plane_b_samples_clamp_to_the_sub_rect_not_the_mask_extent() {
    let Some(ctx) = bc_gpu_or_skip("plane_b_samples_clamp_to_the_sub_rect_not_the_mask_extent")
    else {
        return;
    };
    let (texture, table) = sub_rect_pool(&ctx);
    let cases = [
        // Mask x 5.5 and 7.5: inside plane A, past plane B's 4 texels.
        (sub_rect_probe(13.5, 1.5), sub_rect_mask(1, 0)),
        (sub_rect_probe(15.5, 7.5), sub_rect_mask(1, 1)),
        // Driven past the block edge: plane A clamps to its column 7, plane
        // B to its column 3.
        (sub_rect_probe(16.0, 8.0), sub_rect_mask(1, 1)),
        (sub_rect_probe(24.0, 3.0), sub_rect_mask(1, 0)),
    ];
    let probes: Vec<Probe> = cases.iter().map(|case| case.0).collect();
    let results = run_probes(&ctx, &texture, &table, &probes, true);
    for (index, (result, (_, expected))) in results.iter().zip(&cases).enumerate() {
        for (channel, (&actual, &want)) in result.mask.iter().zip(expected).enumerate() {
            assert_near(actual, want, &format!("probe {index}: channel {channel}"));
        }
    }
}
