// GPU proofs of the streamed lightmap and mask pools, each read back through
// the real block table and WGSL helpers. This file holds the shared streamed
// level; each proof sits in a module under `tests/`.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests", as
// in `pool_sample_test`, whose harness these reuse: set
// `POSTRETRO_REQUIRE_GPU` to make a missing adapter fail instead of skip.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use log::Level;
use postretro_level_loader::{
    GpuLightingPayloads, LightmapBlockClass, LightmapDrainBatch, LightmapDrainOutcome,
    LightmapTarget, PreparedLightmapBlock,
};
use postretro_render_cpu::lightmap_pool::{
    BlockTableEntry, PerPool, PlanePresence, PoolDemand, PoolKind,
};
use postretro_test_log_capture::LogCapture;

use super::super::bound_draw_harness::draw_probes;
use super::super::pool_sample_harness::{
    BoundLightmap, Probe, ProbeResult, SCALE, assert_near, direction_texel, expected_texel,
    irradiance_texel, mask_byte, resident_probe, run_probes,
};
use super::super::test_fixtures::{BlockFixture, FixtureTexels, block_fixture};
use super::super::{
    LightmapResources, StaticPool, StreamingPoolPlan, animated_block_table_bytes,
    bind_group_layout, block_table_bind_group_layout,
};
use super::{LightmapResidencyDrainError, LightmapStreamState};
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};

mod mask_states;
mod pairs_and_generations;
mod reload;
mod repack_and_growth;
mod two_pools;
mod upload_order;

const TAG: [u8; 32] = [7; 32];

/// Blocks wider than half a layer never share a shelf, so these extents
/// force the shelf layouts each test names without megabyte-wide payloads.
const WIDE: u32 = 1028;

/// The mask pool's size where a test proves the lightmap pool: room for every
/// plane of its fixture, so the mask pool never grows or repacks beside it.
const MASK_ROOM: u32 = 8;

fn mandatory(block: u32) -> LightmapTarget {
    LightmapTarget {
        block,
        class: LightmapBlockClass::Mandatory,
        lead: 0,
    }
}

fn band(block: u32, lead: u32) -> LightmapTarget {
    LightmapTarget {
        block,
        class: LightmapBlockClass::Band,
        lead,
    }
}

fn fixture(extents: &[(u32, u32)]) -> BlockFixture {
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

fn plan(
    ctx: &GpuCtx,
    fixture: &BlockFixture,
    cap: PerPool<u32>,
    max_layers: u32,
) -> StreamingPoolPlan {
    StreamingPoolPlan {
        header: fixture.index.header,
        demand: PoolDemand::of_indices(&fixture.index, fixture.shadowmask.as_ref()),
        content_tag: TAG,
        pool_cap_layers: cap,
        max_array_layers: max_layers.min(ctx.device.limits().max_texture_array_layers),
    }
}

fn read_buffer(ctx: &GpuCtx, buffer: &wgpu::Buffer) -> Vec<u8> {
    let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stream test readback"),
        size: buffer.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, buffer.size());
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("map stream test readback")
    });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll stream test device");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback")
        .to_vec();
    readback.unmap();
    bytes
}

/// A `LightmapResources` over `fixture` and `pool`, as level install builds it.
fn level_resources(
    ctx: &GpuCtx,
    fixture: &BlockFixture,
    pool: &StaticPool,
    payloads: GpuLightingPayloads,
    floor: u64,
) -> LightmapResources {
    let animated = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stream test animated placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = animated.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    LightmapResources::new(
        &ctx.device,
        &ctx.queue,
        Some(&fixture.index),
        fixture.shadowmask.as_ref(),
        pool,
        payloads,
        &bind_group_layout(&ctx.device),
        &block_table_bind_group_layout(&ctx.device),
        &view,
        &view,
        &animated_block_table_bytes(None).expect("the empty table always builds"),
        floor,
    )
}

/// A streamed `LightmapResources` over `fixture`, each pool sized by `cap`.
fn sized_resources(
    ctx: &GpuCtx,
    fixture: &BlockFixture,
    cap: PerPool<u32>,
    floor: u64,
) -> LightmapResources {
    let pool = StaticPool::Streaming(plan(ctx, fixture, cap, u32::MAX));
    level_resources(ctx, fixture, &pool, GpuLightingPayloads::default(), floor)
}

fn prepared(fixture: &BlockFixture, generation: u64, block: u32) -> PreparedLightmapBlock {
    PreparedLightmapBlock {
        generation,
        content_tag: TAG,
        block,
        payload: fixture.payloads[block as usize].clone(),
    }
}

fn batch(
    generation: u64,
    reset: Option<Vec<LightmapTarget>>,
    set: Vec<LightmapTarget>,
    ready: Vec<PreparedLightmapBlock>,
) -> LightmapDrainBatch {
    LightmapDrainBatch {
        generation,
        content_tag: TAG,
        pool_cap_layers: MASK_ROOM,
        target_reset: reset,
        target_set: set,
        ready,
        ..LightmapDrainBatch::default()
    }
}

/// One streamed level on a real device: the state under test, and the
/// fixture its drain batches clone payloads from.
struct Stream {
    ctx: GpuCtx,
    queue: crate::render::uploads::UploadQueue,
    fixture: BlockFixture,
    state: LightmapStreamState,
    generation: u64,
    cap: PerPool<u32>,
}

impl Stream {
    fn new(test: &str, extents: &[(u32, u32)], cap: u32) -> Option<Self> {
        let cap = PerPool {
            lightmap: cap,
            mask: MASK_ROOM,
        };
        Self::sized(test, extents, cap, u32::MAX)
    }

    /// Each pool's first generation sized apart, within `max_layers` array
    /// layers. The batch's one cap is the larger size: a pool's own layers
    /// still bound it.
    fn sized(
        test: &str,
        extents: &[(u32, u32)],
        cap: PerPool<u32>,
        max_layers: u32,
    ) -> Option<Self> {
        Self::with_fixture(test, fixture(extents), cap, max_layers)
    }

    fn with_fixture(
        test: &str,
        fixture: BlockFixture,
        cap: PerPool<u32>,
        max_layers: u32,
    ) -> Option<Self> {
        let ctx = bc_gpu_or_skip(test)?;
        let state =
            LightmapStreamState::new(&ctx.device, &plan(&ctx, &fixture, cap, max_layers), 0);
        let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
        Some(Self {
            ctx,
            queue,
            fixture,
            state,
            generation: 1,
            cap,
        })
    }

    fn batch_cap(&self) -> u32 {
        self.cap.lightmap.max(self.cap.mask)
    }

    fn prepared(&self, block: u32) -> PreparedLightmapBlock {
        PreparedLightmapBlock {
            generation: self.generation,
            content_tag: TAG,
            block,
            payload: self.fixture.payloads[block as usize].clone(),
        }
    }

    /// The first drain of the generation: a target reset.
    fn reset(
        &self,
        targets: Vec<LightmapTarget>,
        ready: Vec<PreparedLightmapBlock>,
    ) -> LightmapDrainBatch {
        LightmapDrainBatch {
            generation: self.generation,
            content_tag: TAG,
            pool_cap_layers: self.batch_cap(),
            target_reset: Some(targets),
            ready,
            ..LightmapDrainBatch::default()
        }
    }

    fn delta(
        &self,
        set: Vec<LightmapTarget>,
        remove: Vec<u32>,
        ready: Vec<PreparedLightmapBlock>,
    ) -> LightmapDrainBatch {
        LightmapDrainBatch {
            generation: self.generation,
            content_tag: TAG,
            pool_cap_layers: self.batch_cap(),
            target_set: set,
            target_remove: remove,
            ready,
            ..LightmapDrainBatch::default()
        }
    }

    fn try_drain(
        &mut self,
        batch: LightmapDrainBatch,
    ) -> Result<LightmapDrainOutcome, LightmapResidencyDrainError> {
        self.state
            .drain(&self.ctx.device, &self.queue, batch)
            .map(|(outcome, _)| outcome)
    }

    fn drain(&mut self, batch: LightmapDrainBatch) -> LightmapDrainOutcome {
        self.try_drain(batch).expect("drain must succeed")
    }

    fn table(&self) -> Vec<u8> {
        read_buffer(&self.ctx, self.state.table())
    }

    /// The GPU table equals the model's, every resident block samples its
    /// own texels (edges and interior) through the WGSL helpers, and every
    /// other block reads as a miss. Polls the device.
    fn assert_samples_own_texels(&self) {
        assert_samples_own_texels(&self.ctx, &self.state, &self.fixture);
    }
}

/// A block's mask channels as its stored planes leave them: an absent plane's
/// two channels read zero.
fn stored_mask(mask: [f32; 4], planes: Option<PlanePresence>) -> [f32; 4] {
    let planes = planes.expect("fixtures keep id 42");
    std::array::from_fn(|channel| match planes[channel / 2] {
        true => mask[channel],
        false => 0.0,
    })
}

fn assert_samples_own_texels(ctx: &GpuCtx, state: &LightmapStreamState, fixture: &BlockFixture) {
    let table = read_buffer(ctx, state.table());
    assert_eq!(
        table,
        state.model().table_bytes(),
        "the GPU block table equals the model's"
    );
    let bound = BoundLightmap {
        irradiance: state.irradiance().clone(),
        direction: state.direction().clone(),
        shadowmask: state.shadowmask().expect("fixtures keep id 42").clone(),
        table,
        mask_present: true,
    };
    let mut probes = Vec::new();
    let mut expected = Vec::new();
    for (block, record) in fixture.index.records.iter().enumerate() {
        let (width, height) = (u32::from(record.width), u32::from(record.height));
        match state.model().table_entry(block as u32) {
            BlockTableEntry::Resident { .. } => {
                let coords = |size: u32| [0, 1, size / 2, size - 2, size - 1];
                for x in coords(width) {
                    for y in coords(height) {
                        let uv = [
                            (x as f32 + 0.5) / width as f32,
                            (y as f32 + 0.5) / height as f32,
                        ];
                        probes.push(resident_probe(uv, block as u32));
                        expected.push(Some((block, x, y)));
                    }
                }
            }
            _ => {
                probes.push(resident_probe([0.5, 0.5], block as u32));
                expected.push(None);
            }
        }
    }
    let results = run_probes(ctx, &bound, &probes);
    for (result, expected) in results.iter().zip(&expected) {
        match *expected {
            Some((block, x, y)) => {
                let what = format!("block {block} local ({x}, {y})");
                let (irradiance, direction, mask) = expected_texel(block, x, y);
                let mask = stored_mask(mask, state.model().planes(block as u32));
                assert!(!result.missing, "{what}: resident");
                assert_near(&result.irradiance[..3], &irradiance, 1.0e-3, &what);
                assert_near(&result.direction[..2], &direction, 1.0e-5, &what);
                assert_near(&result.mask, &mask, 1.0e-5, &what);
            }
            None => {
                assert!(result.missing, "a non-resident block reads as a miss");
                assert_eq!(result.irradiance[..3], [0.0; 3]);
            }
        }
    }
}

/// Blocks 0 and 1 stacked on the one layer, band block 3 below them. Block
/// 0 leaves and block 2 (1280 tall) fits no shelf even with 3 evicted, while
/// 1, 2 and 3 fit the one-layer cap together: a repack.
const FRAGMENTED: [(u32, u32); 4] = [(WIDE, 768), (WIDE, 512), (WIDE, 1280), (128, 128)];

fn fragmented_single_layer(stream: &mut Stream) {
    let ready = vec![stream.prepared(0), stream.prepared(1), stream.prepared(3)];
    let outcome = stream.drain(stream.reset(vec![mandatory(0), mandatory(1), band(3, 3)], ready));
    assert_eq!(outcome.installed, vec![0, 1, 3]);
    assert_eq!(stream.state.model().pool(PoolKind::Lightmap).layers(), 1);
    stream.assert_samples_own_texels();
}
