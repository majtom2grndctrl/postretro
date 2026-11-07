// Measurement helper: wall time of one compaction's copies on real layer shapes.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

// Each timed run submits the copies and waits for queue idle. The lightmap pool
// holds BC6H 2048² and Rg8 1024² layers and the mask pool BC5 2048², with the
// same moves in each, at about the hallway's layer count. It proves nothing and
// only prints.
//
// Run under `caffeinate`, nothing beside it, on a dev or profiling build:
//   caffeinate -dims cargo test -p postretro-renderer --lib \
//     lightmap_compaction_copy_timing -- --ignored --nocapture
//
// Optional: `POSTRETRO_COMPACTION_TIMING_LAYERS` (default 29, the spare on
// top), `POSTRETRO_COMPACTION_TIMING_BLOCK` (block edge in texels, default
// 512, 16 to a layer), `POSTRETRO_COMPACTION_TIMING_MIB` (a partial list of
// about this many pool MiB, default 100) and `POSTRETRO_COMPACTION_TIMING_RUNS`
// (timed runs per list, default 30). A lightly loaded AMD part sits at a low
// clock; a long run shows whether that moves the figure.

use std::time::{Duration, Instant};

use postretro_render_cpu::lightmap_pool::{BlockPlacement, PerPool, PoolCopy, PoolKind};

use super::super::pool::PoolFormat;
use super::textures::PoolTextures;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};

const LAYER_EDGE: u32 = 2048;
/// Irradiance 4, direction 2, mask plane 4.
const LAYER_MIB: f64 = 10.0;
const WARM_RUNS: usize = 5;
const DEFAULT_TIMED_RUNS: u32 = 30;

fn env_number(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Every block of `layers` layers hops one layer down the pool, the first
/// layer through the spare: each block moves once, and each copy's source was
/// a destination's blocker one step before, as a rotation is.
fn rotation(layers: u32, spare: u32, block: u32) -> Vec<PoolCopy> {
    let per_row = LAYER_EDGE / block;
    let mut copies = Vec::new();
    let copy = |from: u32, to: u32, slot: u32| PoolCopy {
        src: BlockPlacement {
            layer: from,
            x: (slot % per_row) * block,
            y: (slot / per_row) * block,
        },
        dst: BlockPlacement {
            layer: to,
            x: (slot % per_row) * block,
            y: (slot / per_row) * block,
        },
        width: block,
        height: block,
    };
    let slots = per_row * per_row;
    copies.extend((0..slots).map(|slot| copy(0, spare, slot)));
    for layer in 1..layers {
        copies.extend((0..slots).map(|slot| copy(layer, layer - 1, slot)));
    }
    copies.extend((0..slots).map(|slot| copy(spare, layers - 1, slot)));
    copies
}

fn run_once(
    ctx: &GpuCtx,
    pools: &PerPool<PoolTextures>,
    format: PoolFormat,
    copies: &[PoolCopy],
) -> [Duration; 2] {
    let started = Instant::now();
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    for &copy in copies {
        for kind in PoolKind::ALL {
            pools[kind].record_block_copy(&mut encoder, format, copy);
        }
    }
    let commands = encoder.finish();
    let recorded = started.elapsed();
    ctx.queue.submit(std::iter::once(commands));
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    [recorded, started.elapsed()]
}

fn report(
    ctx: &GpuCtx,
    pools: &PerPool<PoolTextures>,
    format: PoolFormat,
    label: &str,
    copies: &[PoolCopy],
) {
    let mib: f64 = copies
        .iter()
        .map(|copy| f64::from(copy.width * copy.height))
        .sum::<f64>()
        / f64::from(LAYER_EDGE * LAYER_EDGE)
        * LAYER_MIB;
    let timed_runs = env_number("POSTRETRO_COMPACTION_TIMING_RUNS", DEFAULT_TIMED_RUNS) as usize;
    let mut runs = Vec::new();
    for run in 0..WARM_RUNS + timed_runs {
        let times = run_once(ctx, pools, format, copies);
        if run >= WARM_RUNS {
            runs.push(times);
        }
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut total: Vec<f64> = runs.iter().map(|run| ms(run[1])).collect();
    let mut recorded: Vec<f64> = runs.iter().map(|run| ms(run[0])).collect();
    total.sort_unstable_by(f64::total_cmp);
    recorded.sort_unstable_by(f64::total_cmp);
    println!(
        "{label}: {} copies, {mib:.1} MiB; submit to idle ms median {:.2}, min {:.2}, max {:.2}; \
         encode ms median {:.2}; {:.2} GiB/s at the median",
        copies.len(),
        total[total.len() / 2],
        total[0],
        total[total.len() - 1],
        recorded[recorded.len() / 2],
        mib / 1024.0 / (total[total.len() / 2] / 1000.0),
    );
}

#[test]
#[ignore = "measurement helper; needs a GPU"]
fn lightmap_compaction_copy_timing() {
    let Some(ctx) = bc_gpu_or_skip("lightmap_compaction_copy_timing") else {
        return;
    };
    let layers = env_number("POSTRETRO_COMPACTION_TIMING_LAYERS", 29);
    let block = env_number("POSTRETRO_COMPACTION_TIMING_BLOCK", 512);
    let partial_mib = env_number("POSTRETRO_COMPACTION_TIMING_MIB", 100);
    let format = PoolFormat {
        irradiance: wgpu::TextureFormat::Bc6hRgbUfloat,
        direction_scale: 2,
    };
    let pools = PerPool::from_fn(|kind| PoolTextures::new(&ctx.device, kind, format, layers + 1));
    println!(
        "{layers} layers plus the spare, {:.0} MiB; {block}² blocks",
        f64::from(layers + 1) * LAYER_MIB
    );
    let whole = rotation(layers, layers, block);
    let block_mib = f64::from(block * block) / f64::from(LAYER_EDGE * LAYER_EDGE) * LAYER_MIB;
    let partial = (f64::from(partial_mib) / block_mib) as usize;
    report(
        &ctx,
        &pools,
        format,
        "partial",
        &whole[..partial.min(whole.len())],
    );
    report(&ctx, &pools, format, "every block moved once", &whole);
    let twice: Vec<PoolCopy> = whole.iter().chain(&whole).copied().collect();
    report(&ctx, &pools, format, "every block moved twice", &twice);
}
