// Measurement helper (spatial-residency--lightmap-cell-blocks AC 22, install
// time per drain): CPU time of repeated in-play-sized drains on a real
// device, with the staging pool warm, next to the one cold first drain a
// synchronous preload pays (capture's). Not a proof; prints only.
//
// Run: cargo test -p postretro-renderer --lib lightmap_drain_install_timing \
//        -- --ignored --nocapture
//
// Fixture blocks are Rgba16Float irradiance (8 B/texel), not the evidence
// maps' BC6H, so compare the per-MiB rate, not per-block times.

use postretro_level_loader::{
    LightmapBlockClass, LightmapDrainBatch, LightmapTarget, PreparedLightmapBlock,
};

use super::super::StreamingPoolPlan;
use super::super::pool_sample_test::{
    SCALE, direction_texel, gpu_or_skip, irradiance_texel, mask_byte,
};
use super::super::test_fixtures::{FixtureTexels, block_fixture};
use super::LightmapStreamState;

const TAG: [u8; 32] = [3; 32];
/// 256² blocks: 512 KiB irradiance, 32 KiB direction, 2 × 32 KiB shadowmask.
const EDGE: u32 = 256;
/// Pairs per drain: just under the shared 8 MiB drain budget.
const PAIRS_PER_DRAIN: usize = 13;
const SETS: usize = 4;
const DRAINS: usize = 120;

#[test]
#[ignore = "measurement helper; needs a GPU"]
fn lightmap_drain_install_timing() {
    let Some(ctx) = gpu_or_skip("lightmap_drain_install_timing") else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let blocks = PAIRS_PER_DRAIN * SETS;
    let extents = vec![(EDGE, EDGE); blocks];
    let fixture = block_fixture(
        &extents,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: Some(&mask_byte),
        },
    );
    let plan = StreamingPoolPlan {
        header: fixture.index.header,
        extents: extents.clone(),
        with_shadowmask: true,
        content_tag: TAG,
        pool_cap_layers: 1,
        max_array_layers: ctx.device.limits().max_texture_array_layers,
    };
    let mut state = LightmapStreamState::new(&ctx.device, &plan, 0);
    let pair_bytes = {
        let p = &fixture.payloads[0];
        p.irradiance.len()
            + p.direction.len()
            + p.shadowmask.as_ref().map_or(0, |[a, b]| a.len() + b.len())
    } as f64;
    let drain_mib = pair_bytes * PAIRS_PER_DRAIN as f64 / (1024.0 * 1024.0);
    let set = |k: usize| (k % SETS) * PAIRS_PER_DRAIN..(k % SETS + 1) * PAIRS_PER_DRAIN;
    let mut micros = Vec::new();
    for drain in 0..DRAINS {
        let ready: Vec<PreparedLightmapBlock> = set(drain)
            .map(|block| PreparedLightmapBlock {
                generation: 1,
                content_tag: TAG,
                block: block as u32,
                payload: fixture.payloads[block].clone(),
            })
            .collect();
        let targets: Vec<LightmapTarget> = set(drain)
            .map(|block| LightmapTarget {
                block: block as u32,
                class: LightmapBlockClass::Mandatory,
                lead: 0,
            })
            .collect();
        let batch = if drain == 0 {
            LightmapDrainBatch {
                generation: 1,
                content_tag: TAG,
                pool_cap_layers: 1,
                target_reset: Some(targets),
                ready,
                ..LightmapDrainBatch::default()
            }
        } else {
            let mut remove: Vec<u32> = set(drain - 1).map(|b| b as u32).collect();
            remove.sort_unstable();
            LightmapDrainBatch {
                generation: 1,
                content_tag: TAG,
                pool_cap_layers: 1,
                target_set: targets,
                target_remove: remove,
                ready,
                ..LightmapDrainBatch::default()
            }
        };
        let (outcome, _) = state
            .drain(&ctx.device, &queue, batch)
            .expect("drain succeeds");
        assert_eq!(outcome.installed.len(), PAIRS_PER_DRAIN);
        micros.push(state.counters().last_drain_install_micros);
        // A frame presents between drains.
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
    }
    let first = micros[0];
    let mut warm = micros[1..].to_vec();
    warm.sort_unstable();
    let at = |q: f64| warm[((warm.len() as f64 * q).ceil() as usize).clamp(1, warm.len()) - 1];
    let mean = warm.iter().sum::<u64>() as f64 / warm.len() as f64;
    println!(
        "{PAIRS_PER_DRAIN} pairs of {EDGE}² per drain = {drain_mib:.2} MiB; first (cold staging) \
         {:.2} ms = {:.2} ms/MiB; warm {} drains: mean {:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} \
         ms; warm mean {:.2} ms/MiB",
        first as f64 / 1000.0,
        first as f64 / 1000.0 / drain_mib,
        warm.len(),
        mean / 1000.0,
        at(0.5) as f64 / 1000.0,
        at(0.95) as f64 / 1000.0,
        at(1.0) as f64 / 1000.0,
        mean / 1000.0 / drain_mib,
    );
}
