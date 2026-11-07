// GPU proofs of the level states a mask pool can be in, all-resident and
// streamed: dropped alone past the device, a level with id 42 and no stored plane,
// and the two modes sampling alike. Each reads through the level's bound
// resources.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_render_cpu::lightmap_pool::StreamedPoolSizing;

use super::super::super::{plan_static_pool, plan_streaming_pool};
use super::*;
use crate::render::residency::ResidencyAllocationState;

const SHADOWMASK_ROW: usize = 2;

fn all_resident(
    ctx: &GpuCtx,
    fixture: &BlockFixture,
    max_layers: u32,
) -> (StaticPool, LightmapResources) {
    let pool = plan_static_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        &fixture.payloads,
        8192,
        max_layers,
    );
    let payloads = GpuLightingPayloads {
        blocks: fixture.payloads.clone(),
    };
    let resources = level_resources(ctx, fixture, &pool, payloads, 0);
    (pool, resources)
}

fn streamed(ctx: &GpuCtx, fixture: &BlockFixture, layers: u32) -> LightmapResources {
    let pool = plan_streaming_pool(
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        TAG,
        StreamedPoolSizing {
            first_generation_override: PerPool::both(Some(layers)),
            ..StreamedPoolSizing::default()
        },
        8192,
        256,
    );
    level_resources(ctx, fixture, &pool, GpuLightingPayloads::default(), 0)
}

fn every_block_mandatory(count: u32) -> Vec<LightmapTarget> {
    (0..count).map(mandatory).collect()
}

fn block_probes(extents: &[(u32, u32)], owner_position: u32) -> Vec<Probe> {
    let mut probes = Vec::new();
    for (block, &(width, height)) in extents.iter().enumerate() {
        for x in [0, 1, width / 2, width - 1] {
            for y in [0, 1, height / 2, height - 1] {
                probes.push(Probe {
                    uv: [
                        (x as f32 + 0.5) / width as f32,
                        (y as f32 + 0.5) / height as f32,
                    ],
                    block: block as u32 + 1,
                    owner_position,
                });
            }
        }
        // Past the block, where only the half-texel clamp keeps a tap home.
        for uv in [[-0.5, 0.5], [1.5, 0.5], [0.5, -0.5], [0.5, 1.5]] {
            probes.push(Probe {
                uv,
                block: block as u32 + 1,
                owner_position,
            });
        }
    }
    probes
}

// An all-resident level whose mask pool passes the device's array layers keeps
// its lightmap and drops the mask pool alone, by name: no plane installs, the
// placeholder binds, the meter says so, and the forward path reads the
// fully-visible fallback with no owner reading it.
#[test]
fn all_resident_mask_past_device_layers_drops_alone() {
    let Some(ctx) = bc_gpu_or_skip("all_resident_mask_past_device_layers_drops_alone") else {
        return;
    };
    // Two lightmap layers and, a plane each, four mask layers.
    let extents = [(2048, 1100), (2048, 1100)];
    let level = fixture(&extents);
    let logs = LogCapture::start();
    let (pool, resources) = all_resident(&ctx, &level, 3);
    logs.assert_logged_once(Level::Error, "the mask pool needs 4 array layer(s)");
    logs.assert_logged_once(Level::Error, "ShadowmaskAtlas rejected");

    let StaticPool::Blocks(plan) = &pool else {
        panic!("the lightmap stays: {pool:?}");
    };
    assert_eq!(plan.pool.layer_count, 2);
    assert!(plan.masks.is_none(), "no plane installs");
    assert!(!resources.shadowmask_present);
    let [irradiance, direction, shadowmask] = &resources.residency;
    assert_eq!(irradiance.state, ResidencyAllocationState::Data);
    assert_eq!(direction.state, ResidencyAllocationState::Data);
    assert_eq!(shadowmask.state, ResidencyAllocationState::Fallback);
    assert!(
        shadowmask.bytes < 16,
        "the placeholder: {}",
        shadowmask.bytes
    );

    let results = draw_probes(&ctx, &resources, &block_probes(&extents, 0));
    assert!(!results.is_empty());
    for result in &results {
        assert_eq!(result.mask, [1.0; 4], "the fully-visible fallback");
        assert!(!result.owners_readable, "no mask, no owner reads");
        assert_eq!(result.owner_visibility, 0.0);
        assert!(result.irradiance[0] >= 1.0, "the lightmap still reads");
        assert!(!result.missing);
    }
}

// All-resident and streamed modes place planes in the same pool shape and sample
// alike, bit for bit: blocks in every presence, in a streamed pool whose arrival
// order places them elsewhere.
#[test]
fn all_resident_and_streamed_sample_identically() {
    let Some(ctx) = bc_gpu_or_skip("all_resident_and_streamed_sample_identically") else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let extents = [(8, 8), (12, 8), (8, 12), (16, 8), (8, 8), (24, 16)];
    let presence = [
        [true, true],
        [true, false],
        [false, true],
        [false, false],
        [true, true],
        [false, true],
    ];
    let level = fixture(&extents).with_planes(&presence);
    let (pool, resident) = all_resident(&ctx, &level, 256);
    let StaticPool::Blocks(plan) = &pool else {
        panic!("all-resident: {pool:?}");
    };
    let mut streaming = streamed(&ctx, &level, 4);
    let count = extents.len() as u32;
    // The short blocks arrive first, so they take the place the tallest block
    // holds all-resident.
    let ready = |blocks: [u32; 3]| blocks.map(|block| prepared(&level, 1, block)).into();
    let targets = |blocks: [u32; 3]| blocks.map(mandatory).into();
    streaming
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(1, Some(targets([0, 1, 4])), vec![], ready([4, 1, 0])),
        )
        .expect("the first half installs");
    streaming
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(1, None, targets([2, 3, 5]), ready([5, 3, 2])),
        )
        .expect("the second half installs");

    let state = streaming.stream_state().expect("streamed");
    let model = state.model();
    assert!(
        (0..count)
            .any(|block| model.placement(block) != Some(plan.pool.placements[block as usize])),
        "arrival order placed the blocks elsewhere"
    );
    let masks = plan.masks.as_ref().expect("the mask pool plans");
    for (block, stored) in presence.iter().enumerate() {
        for (plane, &stored) in stored.iter().enumerate() {
            assert_eq!(masks.planes[block][plane].is_some(), stored);
            assert_eq!(model.plane_placement(block as u32, plane).is_some(), stored);
        }
    }
    let resident_row = &resident.residency[SHADOWMASK_ROW];
    assert_eq!(resident_row.state, ResidencyAllocationState::Data);
    let shadowmask = state.shadowmask().expect("planes need the mask pool");
    assert_eq!(shadowmask.width(), LIGHTMAP_POOL_LAYER_EDGE);
    assert_eq!(shadowmask.height(), LIGHTMAP_POOL_LAYER_EDGE);
    assert_eq!(shadowmask.format(), wgpu::TextureFormat::Bc5RgUnorm);
    assert_eq!(
        resident_row.bytes,
        u64::from(masks.layer_count) * 2048 * 2048,
        "the same layers, with no spare"
    );

    for owner_position in 0..4 {
        let probes = block_probes(&extents, owner_position);
        let all_resident: Vec<ProbeResult> = draw_probes(&ctx, &resident, &probes);
        let streamed: Vec<ProbeResult> = draw_probes(&ctx, &streaming, &probes);
        assert_eq!(all_resident.len(), probes.len());
        assert_eq!(all_resident, streamed, "owner position {owner_position}");
    }
}

// A level with id 42 and no stored plane has masks that read zero, in both modes:
// it reports mask-present, places nothing, counts no pool bytes and binds the
// placeholder, and it reads neither the fully-visible fallback of a level with
// no id 42 nor a miss. A growth keeps the placeholder bound.
#[test]
fn id_42_with_no_plane_reads_zero_in_both_modes() {
    let Some(ctx) = bc_gpu_or_skip("id_42_with_no_plane_reads_zero_in_both_modes") else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let extents = [(WIDE, 1100), (WIDE, 1200)];
    let level = fixture(&extents).with_planes(&[[false; 2]; 2]);
    let assert_reads_zero = |resources: &LightmapResources, what: &str| {
        assert!(resources.shadowmask_present, "{what}: mask-present");
        let row = &resources.residency[SHADOWMASK_ROW];
        assert_eq!(row.state, ResidencyAllocationState::Dummy, "{what}");
        assert!(row.bytes < 16, "{what}: the placeholder, {}", row.bytes);
        let results = draw_probes(&ctx, resources, &block_probes(&extents, 1));
        assert!(!results.is_empty());
        for result in &results {
            assert_eq!(result.mask, [0.0; 4], "{what}: zero, not the fallback");
            assert!(result.owners_readable, "{what}: a resident block");
            assert_eq!(result.owner_visibility, 0.0, "{what}");
            assert!(!result.missing, "{what}: not a miss");
            assert!(result.irradiance[0] >= 1.0, "{what}: its lightmap reads");
        }
    };

    let (pool, resources) = all_resident(&ctx, &level, 256);
    let StaticPool::Blocks(plan) = &pool else {
        panic!("all-resident: {pool:?}");
    };
    assert_eq!(plan.masks.as_ref().expect("masks").layer_count, 0);
    assert_reads_zero(&resources, "all-resident");

    let mut resources = streamed(&ctx, &level, 1);
    let counters = resources.stream_counters().expect("streamed");
    assert_eq!(counters.pool_layers.mask, 0);
    assert_eq!(counters.pool_texture_sets.mask, 0);
    assert_eq!(counters.active_pool_bytes.mask, 0, "no pool bytes");
    let ready = vec![prepared(&level, 1, 0), prepared(&level, 1, 1)];
    let (outcome, _) = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(1, Some(every_block_mandatory(2)), vec![], ready),
        )
        .expect("both blocks install");
    assert!(
        outcome.pool.lightmap.grew,
        "the rebind keeps the placeholder"
    );
    assert_eq!(outcome.pool.mask.layers, 0);
    assert_eq!(outcome.installed, vec![0, 1]);
    assert_reads_zero(&resources, "streamed");
}
