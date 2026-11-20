// Measurement helper: CPU time of lightmap drains as a cost per drain, per
// pair and per MiB, over drains of several pair sizes and counts on a warm
// staging pool. A drain during which a staging pool created a buffer runs at
// the cold rate and is listed apart, wherever it falls. Prints only.
//
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Run: cargo test --release -p postretro-renderer --lib lightmap_install_cost_model \
//        -- --ignored --nocapture --test-threads=1
//
// Fixture pairs are Rgba16Float irradiance (8 B/texel), not the shipped BC6H,
// so a pair of a given byte size has more texels and rows here.

use std::collections::BTreeMap;

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{
    LightmapBlockClass, LightmapDrainBatch, LightmapTarget, PreparedLightmapBlock,
};
use postretro_render_cpu::lightmap_pool::{PerPool, PoolDemand};

use super::super::StreamingPoolPlan;
use super::super::pool_sample_harness::{SCALE, direction_texel, irradiance_texel, mask_byte};
use super::super::test_fixtures::{BlockFixture, FixtureTexels, block_fixture};
use super::LightmapStreamState;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::install_cost_model::{
    CostRun, DrainClass, MIB, SHOWCASE_PRICES, Shape, mark_drain, run_context, shuffled,
    staging_buffers_created,
};
use crate::render::uploads::UploadQueue;

const TAG: [u8; 32] = [7; 32];
/// First-generation layers per pool: room for the largest drain beside the
/// one it retires, so no measured drain repacks or grows.
const LAYERS: u32 = 4;
/// Pair extents, small to large; the last is about the showcase's 14 MiB pair.
const EXTENTS: [(u32, u32); 6] = [
    (64, 64),
    (128, 128),
    (256, 256),
    (512, 512),
    (1024, 1024),
    (1280, 1216),
];
/// Drain totals the shapes aim at, in MiB.
const TOTALS_MIB: [u32; 6] = [2, 4, 8, 12, 16, 32];
const ROUNDS: usize = 12;

/// A streamed lightmap state over pairs of a few extents. Each extent owns
/// two sets of blocks, used alternately, so a drain never targets a block the
/// drain before it is retiring.
struct Rig {
    state: LightmapStreamState,
    queue: UploadQueue,
    /// One payload per extent; every block of that extent installs a copy.
    payloads: Vec<LightmapBlockPayload>,
    sets: Vec<[Vec<u32>; 2]>,
    next_set: Vec<usize>,
    /// The previous drain's blocks, ascending: this drain retires them.
    installed: Vec<u32>,
    drained: bool,
}

fn pair_fixture(extents: &[(u32, u32)]) -> BlockFixture {
    block_fixture(
        extents,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: Some(&mask_byte),
        },
    )
}

/// Decoded bytes of one pair, as the drain budget counts them.
fn pair_bytes(payload: &LightmapBlockPayload) -> u64 {
    (payload.irradiance.len()
        + payload.direction.len()
        + payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len())) as u64
}

impl Rig {
    /// `fixture` holds one pair per extent; `max_items[e]` is the most pairs
    /// of `extents[e]` any drain carries.
    fn new(
        ctx: &GpuCtx,
        extents: &[(u32, u32)],
        fixture: BlockFixture,
        max_items: &[usize],
    ) -> Self {
        let mut block_extents = Vec::new();
        let sets = extents
            .iter()
            .zip(max_items)
            .map(|(&extent, &items)| {
                std::array::from_fn(|_| {
                    (0..items)
                        .map(|_| {
                            block_extents.push(extent);
                            block_extents.len() as u32 - 1
                        })
                        .collect()
                })
            })
            .collect();
        let mut header = fixture.index.header;
        header.block_count = block_extents.len() as u32;
        let planes = vec![[true; 2]; block_extents.len()];
        let plan = StreamingPoolPlan {
            header,
            demand: PoolDemand::new(header.block_alignment(), block_extents, Some(planes)),
            content_tag: TAG,
            first_generation_layers: PerPool::both(LAYERS),
            max_array_layers: ctx.device.limits().max_texture_array_layers,
        };
        Self {
            state: LightmapStreamState::new(&ctx.device, &plan, 0),
            queue: UploadQueue::new(&ctx.device, ctx.queue.clone(), true),
            payloads: fixture.payloads,
            next_set: vec![0; extents.len()],
            sets,
            installed: Vec::new(),
            drained: false,
        }
    }

    /// Install `items` pairs of `extent`, retiring the previous drain's pairs
    /// in the same batch, then wait for the GPU as a presented frame would.
    /// Returns the drain's CPU time and its class.
    fn drain(&mut self, device: &wgpu::Device, extent: usize, items: usize) -> (u64, DrainClass) {
        let set = self.next_set[extent];
        self.next_set[extent] ^= 1;
        let blocks = self.sets[extent][set][..items].to_vec();
        let ready = blocks
            .iter()
            .map(|&block| PreparedLightmapBlock {
                generation: 1,
                content_tag: TAG,
                block,
                payload: self.payloads[extent].clone(),
            })
            .collect();
        let targets: Vec<LightmapTarget> = blocks
            .iter()
            .map(|&block| LightmapTarget {
                block,
                class: LightmapBlockClass::Mandatory,
                lead: 0,
            })
            .collect();
        let retiring = std::mem::replace(&mut self.installed, blocks);
        let batch = if self.drained {
            LightmapDrainBatch {
                generation: 1,
                content_tag: TAG,
                target_set: targets,
                target_remove: retiring,
                ready,
                ..LightmapDrainBatch::default()
            }
        } else {
            LightmapDrainBatch {
                generation: 1,
                content_tag: TAG,
                target_reset: Some(targets),
                ready,
                ..LightmapDrainBatch::default()
            }
        };
        self.drained = true;

        let before = self.state.counters();
        let created_before = staging_buffers_created(&[&self.state.staging], &self.queue);
        let scratch_before = self.state.upload_scratch.capacity();
        let (outcome, _) = self
            .state
            .drain(device, &self.queue, batch)
            .expect("drain succeeds");
        let after = self.state.counters();
        let created_after = staging_buffers_created(&[&self.state.staging], &self.queue);
        assert_eq!(outcome.installed.len(), items, "every pair installs");
        let excluded = if after.growths.total() > before.growths.total() {
            Some("pool growth")
        } else if after.repacks.total() > before.repacks.total() {
            Some("pool repack")
        } else if after.deferred_pairs > before.deferred_pairs {
            Some("pairs deferred")
        } else if self.state.upload_scratch.capacity() > scratch_before {
            Some("upload scratch grew (CPU allocation)")
        } else {
            None
        };
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        (
            after.last_drain_install_micros,
            DrainClass::of(created_before, created_after, excluded),
        )
    }
}

/// Each extent's pair count for each total, one shape per distinct
/// `(extent, items)`: its bucket is the total it lands nearest. A total the
/// extent overshoots or undershoots by more than half is left out.
fn shapes(pair_sizes: &[u64]) -> Vec<(usize, Shape)> {
    let mut nearest: BTreeMap<(usize, u32), (f64, u32)> = BTreeMap::new();
    for (extent, &bytes) in pair_sizes.iter().enumerate() {
        for total in TOTALS_MIB {
            let target = f64::from(total) * MIB;
            let items = (target / bytes as f64).round() as u32;
            if items == 0 {
                continue;
            }
            let error = (f64::from(items) * bytes as f64 - target).abs() / target;
            if error > 0.5 {
                continue;
            }
            let entry = nearest.entry((extent, items)).or_insert((error, total));
            if error < entry.0 {
                *entry = (error, total);
            }
        }
    }
    nearest
        .into_iter()
        .map(|((extent, items), (_, total))| {
            let (w, h) = EXTENTS[extent];
            (
                extent,
                Shape {
                    label: format!("{items} x {w}x{h}"),
                    items,
                    bytes: u64::from(items) * pair_sizes[extent],
                    nominal_mib: total,
                },
            )
        })
        .collect()
}

#[test]
#[ignore = "measurement helper; needs a GPU"]
fn lightmap_install_cost_model() {
    let Some(ctx) = bc_gpu_or_skip("lightmap_install_cost_model") else {
        return;
    };
    let fixture = pair_fixture(&EXTENTS);
    let bytes: Vec<u64> = fixture.payloads.iter().map(pair_bytes).collect();
    let shaped = shapes(&bytes);
    let mut max_items = vec![0usize; EXTENTS.len()];
    for (extent, shape) in &shaped {
        max_items[*extent] = max_items[*extent].max(shape.items as usize);
    }
    let mut rig = Rig::new(&ctx, &EXTENTS, fixture, &max_items);
    let extent_of: Vec<usize> = shaped.iter().map(|(extent, _)| *extent).collect();
    let mut run = CostRun::new(
        "lightmap drain install CPU (last_drain_install_micros)",
        shaped.into_iter().map(|(_, shape)| shape).collect(),
    );
    for round in 0..ROUNDS {
        for shape in shuffled(run.shapes.len(), 0x5eed + round as u64) {
            let items = run.shapes[shape].items as usize;
            let index = run.samples.len();
            mark_drain(&ctx.device, &rig.queue, "lightmap", index, "begin");
            let (micros, class) = rig.drain(&ctx.device, extent_of[shape], items);
            mark_drain(&ctx.device, &rig.queue, "lightmap", index, "end");
            run.record(shape, micros, class);
        }
    }
    run.write_sequence("lightmap");
    run.report(
        &run_context(
            &ctx.device,
            "Rgba16Float irradiance pairs with direction and two shadowmask planes (shipped \
             irradiance is BC6H); drain = plan + stage + record + submit, retiring the previous \
             drain's pairs",
        ),
        &SHOWCASE_PRICES,
    );
}

// Pin P12: a drain larger than every earlier one in the run needs a staging
// buffer the pool has never made, so it runs cold and never counts warm.
#[test]
fn install_cost_run_reports_a_drain_larger_than_every_earlier_one_cold() {
    let Some(ctx) =
        bc_gpu_or_skip("install_cost_run_reports_a_drain_larger_than_every_earlier_one_cold")
    else {
        return;
    };
    // Four 64² pairs fit the pool's smallest staging class; one 1024² pair
    // needs a class nearly ten times larger.
    let extents = [(64, 64), (1024, 1024)];
    let fixture = pair_fixture(&extents);
    let bytes: Vec<u64> = fixture.payloads.iter().map(pair_bytes).collect();
    let mut rig = Rig::new(&ctx, &extents, fixture, &[4, 1]);
    let shapes = vec![
        Shape {
            label: "4 x 64x64".into(),
            items: 4,
            bytes: 4 * bytes[0],
            nominal_mib: 0,
        },
        Shape {
            label: "1 x 1024x1024".into(),
            items: 1,
            bytes: bytes[1],
            nominal_mib: 9,
        },
    ];
    let mut run = CostRun::new("P12", shapes);
    for (shape, extent, items) in [(0, 0, 4), (0, 0, 4), (1, 1, 1), (0, 0, 4), (1, 1, 1)] {
        let (micros, class) = rig.drain(&ctx.device, extent, items);
        run.record(shape, micros, class);
    }
    let classes: Vec<DrainClass> = run.samples.iter().map(|s| s.class).collect();
    assert!(matches!(classes[0], DrainClass::Cold { .. }), "{classes:?}");
    assert_eq!(classes[1], DrainClass::Warm, "{classes:?}");
    assert!(
        matches!(classes[2], DrainClass::Cold { .. }),
        "the largest drain so far is cold: {classes:?}"
    );
    assert_eq!(classes[3], DrainClass::Warm, "{classes:?}");
    assert_eq!(classes[4], DrainClass::Warm, "{classes:?}");
}
