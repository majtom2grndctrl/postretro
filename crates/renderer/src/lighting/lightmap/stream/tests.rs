// GPU proofs of the streamed lightmap pool: repack through the spare layer,
// growth and retirement, pair atomicity, the P3/P6/P7/P9 orderings and the
// no-op drain, each read back through the real block table and WGSL helpers.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests", as
// in `pool_sample_test`, whose harness these reuse: set
// `POSTRETRO_REQUIRE_GPU` to make a missing adapter fail instead of skip.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use log::Level;
use postretro_level_loader::{
    LightmapBlockClass, LightmapDrainBatch, LightmapDrainOutcome, LightmapTarget,
    PreparedLightmapBlock,
};
use postretro_render_cpu::lightmap_pool::BlockTableEntry;
use postretro_test_log_capture::LogCapture;

use super::super::pool_sample_test::{
    BoundLightmap, GpuCtx, SCALE, assert_near, direction_texel, expected_texel, gpu_or_skip,
    irradiance_texel, mask_byte, resident_probe, run_probes,
};
use super::super::test_fixtures::{BlockFixture, FixtureTexels, block_fixture};
use super::super::{
    LightmapResources, StaticPool, StreamingPoolPlan, animated_block_table_bytes,
    bind_group_layout, block_table_bind_group_layout,
};
use super::{LightmapResidencyDrainError, LightmapStreamState};

const TAG: [u8; 32] = [7; 32];

/// Blocks wider than half a layer never share a shelf, so these extents
/// force the shelf layouts each test names without megabyte-wide payloads.
const WIDE: u32 = 1028;

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

fn plan(ctx: &GpuCtx, fixture: &BlockFixture, cap: u32, max_layers: u32) -> StreamingPoolPlan {
    StreamingPoolPlan {
        header: fixture.index.header,
        extents: fixture
            .index
            .records
            .iter()
            .map(|r| (u32::from(r.width), u32::from(r.height)))
            .collect(),
        with_shadowmask: true,
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

/// One streamed level on a real device: the state under test, and the
/// fixture its drain batches clone payloads from.
struct Stream {
    ctx: GpuCtx,
    queue: crate::render::uploads::UploadQueue,
    fixture: BlockFixture,
    state: LightmapStreamState,
    generation: u64,
    cap: u32,
}

impl Stream {
    fn new(test: &str, extents: &[(u32, u32)], cap: u32) -> Option<Self> {
        Self::with_max_layers(test, extents, cap, u32::MAX)
    }

    fn with_max_layers(
        test: &str,
        extents: &[(u32, u32)],
        cap: u32,
        max_layers: u32,
    ) -> Option<Self> {
        let ctx = gpu_or_skip(test)?;
        let fixture = fixture(extents);
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
            pool_cap_layers: self.cap,
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
            pool_cap_layers: self.cap,
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

fn assert_samples_own_texels(ctx: &GpuCtx, state: &LightmapStreamState, fixture: &BlockFixture) {
    let table = read_buffer(ctx, state.table());
    assert_eq!(
        table,
        state.model().table_bytes(),
        "the GPU block table equals the model's"
    );
    let textures = state.textures();
    let bound = BoundLightmap {
        irradiance: textures.irradiance.clone(),
        direction: textures.direction.clone(),
        shadowmask: textures.shadowmask.clone().expect("fixtures keep id 42"),
        table,
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
    assert_eq!(stream.state.model().layers(), 1);
    stream.assert_samples_own_texels();
}

// AC 10, GPU half: a repack moves blocks through the spare layer of the one
// pool texture; every resident block then samples its own texels, and no
// second pool texture is ever created.
#[test]
fn repack_through_the_spare_layer_keeps_every_block_on_its_own_texels_without_a_second_pool() {
    let Some(mut stream) = Stream::new(
        "repack_through_the_spare_layer_keeps_every_block_on_its_own_texels_without_a_second_pool",
        &FRAGMENTED,
        1,
    ) else {
        return;
    };
    fragmented_single_layer(&mut stream);
    let placed_before = stream.state.model().placement(1);

    let outcome = stream.drain(stream.delta(vec![mandatory(2)], vec![0], vec![stream.prepared(2)]));
    assert!(outcome.pool.repacked && !outcome.pool.grew);
    assert_eq!(outcome.installed, vec![2]);
    assert_eq!(outcome.evicted, vec![0]);
    let plan = stream.state.model().plan();
    let spare = stream.state.model().spare_layer();
    assert!(!plan.copies.is_empty(), "the repack moved resident blocks");
    assert!(
        plan.copies
            .iter()
            .all(|copy| copy.src.layer == spare || copy.dst.layer == spare),
        "one layer: every move stages through the spare"
    );
    assert_ne!(
        stream.state.model().placement(1),
        placed_before,
        "block 1 moved"
    );

    let counters = stream.state.counters();
    assert_eq!(counters.repacks, 1);
    assert_eq!(counters.repack_copy_commands, plan.copies.len() as u64);
    assert_eq!(
        counters.pool_texture_sets, 1,
        "a repack creates no pool texture"
    );
    assert_eq!(stream.state.model().texture_allocations(), 1);
    assert_eq!(counters.growths, 0);
    assert_eq!(counters.submissions, 2, "one submission per drain");
    stream.assert_samples_own_texels();
}

// P7, repack half: an install that fails after a repack leaves the repack's
// moves standing and every entry addressing its own block.
#[test]
fn a_failed_install_after_a_repack_leaves_every_entry_on_its_own_block() {
    let Some(mut stream) = Stream::new(
        "a_failed_install_after_a_repack_leaves_every_entry_on_its_own_block",
        &FRAGMENTED,
        1,
    ) else {
        return;
    };
    fragmented_single_layer(&mut stream);
    let mut broken = stream.prepared(2);
    broken.payload.irradiance.truncate(64);

    let outcome = stream.drain(stream.delta(vec![mandatory(2)], vec![0], vec![broken]));
    assert!(outcome.pool.repacked);
    assert!(outcome.installed.is_empty());
    assert_eq!(outcome.failed, vec![2], "the failed pair fails whole");
    assert!(!stream.state.model().is_resident(2));
    assert!(stream.state.model().is_resident(1) && stream.state.model().is_resident(3));
    let counters = stream.state.counters();
    assert_eq!(counters.failed_installs, 1);
    assert_eq!(counters.last_drain_uploads, 0);
    assert_eq!(
        counters.last_drain_install_bytes, 0,
        "no plane of block 2 uploaded"
    );
    stream.assert_samples_own_texels();
}

// Growth, plus P7's growth half: a mandatory block that fits no repack grows
// a new generation; the old one retires and is released only once its
// submitted work is done, and a pair failing in the growth drain leaves every
// other entry on its own block in the new generation.
#[test]
fn growth_keeps_every_block_on_its_own_texels_and_retires_the_old_set_until_work_is_done() {
    let Some(mut stream) = Stream::new(
        "growth_keeps_every_block_on_its_own_texels_and_retires_the_old_set_until_work_is_done",
        &[(WIDE, 1100), (WIDE, 1200), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let first_generation_bytes = stream.state.counters().active_pool_bytes;
    let mut broken = stream.prepared(2);
    broken.payload.shadowmask = None;

    let outcome = stream.drain(stream.delta(
        vec![mandatory(1), mandatory(2)],
        vec![],
        vec![stream.prepared(1), broken],
    ));
    assert!(outcome.pool.grew && outcome.pool.retiring);
    assert_eq!(outcome.installed, vec![1]);
    assert_eq!(outcome.failed, vec![2]);
    assert!(outcome.refused.is_empty());
    assert_eq!(outcome.pool.layers, 2);
    let counters = stream.state.counters();
    assert_eq!(counters.growths, 1);
    assert_eq!(counters.pool_texture_sets, 2);
    assert_eq!(counters.pool_layers, 2);
    assert_eq!(counters.retiring_pool_bytes, first_generation_bytes);
    assert_eq!(
        counters.growth_transient_peak_bytes,
        counters.active_pool_bytes + first_generation_bytes
    );
    assert_eq!(
        stream
            .state
            .textures()
            .irradiance
            .size()
            .depth_or_array_layers,
        3
    );

    // Nothing has polled the device, so the submitted-work callback has not
    // run: a no-op drain keeps the old set retiring.
    stream.drain(stream.delta(vec![], vec![], vec![]));
    let retiring = stream.state.retiring_pool().expect("still retiring");
    assert!(!retiring.is_complete());
    assert!(stream.state.model().retiring());

    // Sampling polls the device to completion; the next drain releases it.
    stream.assert_samples_own_texels();
    assert!(
        stream
            .state
            .retiring_pool()
            .is_some_and(|r| r.is_complete())
    );
    let outcome = stream.drain(stream.delta(vec![], vec![], vec![stream.prepared(2)]));
    assert!(!outcome.pool.retiring);
    assert_eq!(
        outcome.installed,
        vec![2],
        "a retry installs in the new set"
    );
    assert!(stream.state.retiring_pool().is_none());
    assert!(!stream.state.model().retiring());
    assert_eq!(stream.state.counters().retiring_pool_bytes, 0);
    stream.assert_samples_own_texels();
}

// AC 8, renderer side: a pair missing a shadowmask group, or carrying a short
// one, fails whole — none of its planes upload and its entry never turns
// resident — while a well-formed pair in the same drain installs.
#[test]
fn a_pair_missing_a_shadowmask_group_fails_whole_and_its_entry_stays_non_resident() {
    let Some(mut stream) = Stream::new(
        "a_pair_missing_a_shadowmask_group_fails_whole_and_its_entry_stays_non_resident",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    let whole = stream.prepared(0);
    let whole_bytes = whole.payload.irradiance.len()
        + whole.payload.direction.len()
        + whole
            .payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len());
    let mut no_groups = stream.prepared(1);
    no_groups.payload.shadowmask = None;
    let mut short_group = stream.prepared(2);
    short_group
        .payload
        .shadowmask
        .as_mut()
        .expect("fixture keeps id 42")[1]
        .truncate(8);

    let targets = vec![mandatory(0), mandatory(1), mandatory(2)];
    let outcome = stream.drain(stream.reset(targets, vec![whole, no_groups, short_group]));
    assert_eq!(outcome.installed, vec![0]);
    assert_eq!(outcome.failed, vec![1, 2]);
    assert!(outcome.refused.is_empty());
    let counters = stream.state.counters();
    assert_eq!(counters.failed_installs, 2);
    assert_eq!(counters.last_drain_uploads, 1);
    assert_eq!(counters.last_drain_install_bytes, whole_bytes as u64);
    assert_eq!(
        counters.last_drain_table_writes, 1,
        "only block 0's entry changed"
    );
    for block in [1, 2] {
        assert!(matches!(
            stream.state.model().table_entry(block),
            BlockTableEntry::Missing { .. }
        ));
    }
    stream.assert_samples_own_texels();
}

// P6: evicting A and placing B in A's region happen in one submission; the
// table read back shows A non-resident and B on its own texels, never A
// through B's texels.
#[test]
fn an_evicted_block_turns_non_resident_in_the_submission_that_fills_its_region() {
    let Some(mut stream) = Stream::new(
        "an_evicted_block_turns_non_resident_in_the_submission_that_fills_its_region",
        &[(WIDE, 1100), (WIDE, 1100)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![band(0, 7)], vec![stream.prepared(0)]));
    let a_region = stream.state.model().placement(0).expect("A resident");
    let submissions = stream.state.counters().submissions;

    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert_eq!(outcome.installed, vec![1]);
    assert_eq!(outcome.evicted, vec![0]);
    assert_eq!(
        stream.state.model().placement(1),
        Some(a_region),
        "B lands in A's region"
    );
    assert_eq!(stream.state.counters().submissions, submissions + 1);
    assert_eq!(stream.state.counters().last_drain_table_writes, 2);
    stream.assert_samples_own_texels();
}

// P3: a batch from another generation, or with another level's identity, is
// rejected before anything changes; the next valid batch still drains.
#[test]
fn a_batch_from_another_generation_is_rejected_without_mutating_state() {
    let Some(mut stream) = Stream::new(
        "a_batch_from_another_generation_is_rejected_without_mutating_state",
        &[(8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.generation = 2;
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let table = stream.table();
    let counters = stream.state.counters();

    stream.generation = 1;
    let stale = stream.reset(vec![mandatory(1)], vec![stream.prepared(1)]);
    assert_eq!(
        stream.try_drain(stale).unwrap_err(),
        LightmapResidencyDrainError::StaleGeneration {
            current: 2,
            received: 1
        }
    );
    stream.generation = 3;
    let unreset = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    assert_eq!(
        stream.try_drain(unreset).unwrap_err(),
        LightmapResidencyDrainError::GenerationResetRequired {
            current: 2,
            received: 3
        }
    );
    stream.generation = 2;
    let mut foreign = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    foreign.content_tag = [9; 32];
    assert!(matches!(
        stream.try_drain(foreign),
        Err(LightmapResidencyDrainError::InvalidBatch(_))
    ));
    let mut mixed = stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]);
    mixed.ready[0].generation = 1;
    assert!(matches!(
        stream.try_drain(mixed),
        Err(LightmapResidencyDrainError::InvalidBatch(_))
    ));

    assert_eq!(
        stream.state.counters(),
        counters,
        "no rejected batch counted"
    );
    assert!(!stream.state.model().is_resident(1));
    assert_eq!(stream.table(), table, "no rejected batch wrote the table");

    stream.generation = 3;
    let outcome =
        stream.drain(stream.reset(vec![mandatory(0), mandatory(1)], vec![stream.prepared(1)]));
    assert_eq!(outcome.installed, vec![1]);
    stream.assert_samples_own_texels();
}

// A pair that needs growth past the device's array-layer limit is deferred
// and owned back: nothing grows, nothing is submitted, and the table stays as
// it was. The miss lasts while the pair stays wanted, so it warns once per
// level however many drains defer it.
#[test]
fn a_pair_needing_growth_past_the_device_layer_limit_is_deferred() {
    let Some(mut stream) = Stream::with_max_layers(
        "a_pair_needing_growth_past_the_device_layer_limit_is_deferred",
        &[(WIDE, 1100), (WIDE, 1200)],
        1,
        2,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let table = stream.table();
    let before = stream.state.counters();
    let logs = LogCapture::start();
    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert!(outcome.installed.is_empty() && !outcome.pool.grew);
    let deferred: Vec<u32> = outcome.deferred.iter().map(|p| p.block).collect();
    assert_eq!(deferred, vec![1], "the pair is owned back");
    let counters = stream.state.counters();
    assert_eq!(counters.deferred_pairs, before.deferred_pairs + 1);
    assert_eq!(
        counters.submissions, before.submissions,
        "nothing submitted"
    );
    assert_eq!(counters.pool_texture_sets, 1);
    assert_eq!(stream.state.model().layers(), 1);
    assert!(!stream.state.model().retiring());
    assert!(!stream.state.model().is_resident(1));
    assert_eq!(stream.table(), table);
    stream.assert_samples_own_texels();

    let outcome = stream.drain(stream.delta(vec![], vec![], vec![stream.prepared(1)]));
    assert_eq!(outcome.deferred.len(), 1, "deferred again");
    logs.assert_logged_once(Level::Warn, "a permanent miss");
}

// A new generation mid-level comes from a fresh controller that holds nothing
// resident: its first drain frees every placement in the same submission,
// reports none of them evicted, and installs only what its batch carries.
#[test]
fn a_new_generation_starts_from_an_empty_residency_without_reporting_evictions() {
    let Some(mut stream) = Stream::new(
        "a_new_generation_starts_from_an_empty_residency_without_reporting_evictions",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(
        vec![mandatory(0), mandatory(1)],
        vec![stream.prepared(0), stream.prepared(1)],
    ));
    let before = stream.state.counters();

    stream.generation = 2;
    let outcome =
        stream.drain(stream.reset(vec![mandatory(1), mandatory(2)], vec![stream.prepared(2)]));
    assert_eq!(outcome.installed, vec![2]);
    assert!(outcome.evicted.is_empty(), "{:?}", outcome.evicted);
    assert!(!stream.state.model().is_resident(0));
    assert!(
        !stream.state.model().is_resident(1),
        "the new controller never read block 1"
    );
    let counters = stream.state.counters();
    assert_eq!(counters.last_drain_table_writes, 3);
    assert_eq!(counters.evictions, before.evictions);
    assert_eq!(counters.pool_texture_sets, 1, "the textures stand");
    stream.assert_samples_own_texels();
}

// AC 15, renderer half: a drain that changes nothing issues no table write,
// no upload and no submission; a reclassification writes nothing either, and
// a removal writes exactly its one entry.
#[test]
fn a_no_op_drain_issues_no_table_writes_no_uploads_and_no_submission() {
    let Some(mut stream) = Stream::new(
        "a_no_op_drain_issues_no_table_writes_no_uploads_and_no_submission",
        &[(8, 8), (8, 8), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(
        vec![mandatory(0), mandatory(1)],
        vec![stream.prepared(0), stream.prepared(1)],
    ));
    let installed = stream.state.counters();
    assert_eq!(installed.last_drain_table_writes, 2);
    assert_eq!(
        installed.table_entries_written,
        4 + 2,
        "the install table, then two"
    );

    for batch in [
        stream.delta(vec![], vec![], vec![]),
        stream.delta(vec![band(1, 3)], vec![], vec![]),
    ] {
        stream.drain(batch);
        let counters = stream.state.counters();
        assert_eq!(counters.last_drain_table_writes, 0);
        assert_eq!(counters.last_drain_uploads, 0);
        assert_eq!(counters.last_drain_install_bytes, 0);
        assert_eq!(
            counters.submissions, installed.submissions,
            "nothing submitted"
        );
        assert_eq!(
            counters.table_entries_written,
            installed.table_entries_written
        );
    }
    stream.drain(stream.delta(vec![], vec![1], vec![]));
    let counters = stream.state.counters();
    assert_eq!(counters.last_drain_table_writes, 1);
    assert_eq!(counters.submissions, installed.submissions + 1);
    stream.assert_samples_own_texels();
}

/// A streamed `LightmapResources` over `fixture`, as level install builds it.
fn streamed_resources(ctx: &GpuCtx, fixture: &BlockFixture, floor: u64) -> LightmapResources {
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
        &StaticPool::Streaming(plan(ctx, fixture, 1, u32::MAX)),
        postretro_level_loader::GpuLightingPayloads::default(),
        &bind_group_layout(&ctx.device),
        &block_table_bind_group_layout(&ctx.device),
        &view,
        &view,
        &animated_block_table_bytes(None),
        floor,
    )
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
        pool_cap_layers: 1,
        target_reset: reset,
        target_set: set,
        ready,
        ..LightmapDrainBatch::default()
    }
}

// Growth through the resource set, then P9 / AC 13: group 4 rebinds the new
// generation and the meter follows it with the retiring set counted apart;
// a reload while that set is alive releases both pools (the queue callback
// keeps only its flag), and the new level's first drain sees none of the
// old level's state and refuses its generation.
#[test]
fn reload_while_a_generation_retires_releases_both_pools_and_refuses_the_old_generation() {
    let Some(ctx) = gpu_or_skip(
        "reload_while_a_generation_retires_releases_both_pools_and_refuses_the_old_generation",
    ) else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let level_a = fixture(&[(WIDE, 1100), (WIDE, 1200)]);
    let mut resources = streamed_resources(&ctx, &level_a, 0);
    let installed_rows = resources.residency.clone();
    let first = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(
                5,
                Some(vec![mandatory(0)]),
                vec![],
                vec![prepared(&level_a, 5, 0)],
            ),
        )
        .expect("install A")
        .0;
    assert_eq!(first.installed, vec![0]);
    let (grown, meter_changed) = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(5, None, vec![mandatory(1)], vec![prepared(&level_a, 5, 1)]),
        )
        .expect("grow for C");
    assert!(grown.pool.grew && meter_changed);
    let state = resources.stream_state().expect("streamed");
    assert_eq!(state.counters().pool_layers, 2);
    assert_eq!(
        resources.residency[0].bytes,
        installed_rows[0].bytes * 3 / 2,
        "the irradiance row follows the 3-array-layer generation"
    );
    assert_eq!(
        resources.retiring_bytes(),
        state.counters().retiring_pool_bytes
    );
    assert!(resources.retiring_bytes() > 0);
    let flag = Arc::clone(
        state
            .retiring_pool()
            .expect("retiring before any poll")
            .completion_flag(),
    );
    assert!(!flag.load(Ordering::Acquire));
    let floor = resources.generation_high_water();
    assert_eq!(floor, 5);

    // Reload: the new level's resources replace the old ones, as install does.
    drop(resources);
    assert_eq!(
        Arc::strong_count(&flag),
        2,
        "only this test and the pending queue callback hold the flag: the retiring set dropped"
    );
    let level_b = fixture(&[(8, 8), (8, 8)]);
    let mut resources = streamed_resources(&ctx, &level_b, floor);
    let old = resources.drain_streaming(
        &ctx.device,
        &queue,
        batch(
            5,
            Some(vec![mandatory(0)]),
            vec![],
            vec![prepared(&level_b, 5, 0)],
        ),
    );
    assert_eq!(
        old.unwrap_err(),
        LightmapResidencyDrainError::StaleGeneration {
            current: 5,
            received: 5
        }
    );
    let state = resources.stream_state().expect("streamed");
    let counters = state.counters();
    assert_eq!(counters.drains, 0);
    assert_eq!(counters.pool_texture_sets, 1);
    assert_eq!(counters.retiring_pool_bytes, 0);
    assert!(state.retiring_pool().is_none());
    assert_eq!(resources.retiring_bytes(), 0);

    let outcome = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(
                6,
                Some(vec![mandatory(1)]),
                vec![],
                vec![prepared(&level_b, 6, 1)],
            ),
        )
        .expect("the new level's first drain")
        .0;
    assert_eq!(outcome.installed, vec![1]);
    assert!(!outcome.pool.retiring);
    let state = resources.stream_state().expect("streamed");
    assert_samples_own_texels(&ctx, state, &level_b);

    // The poll above ran the old callback: it set the flag and dropped its
    // clone, touching nothing of level B.
    assert!(flag.load(Ordering::Acquire));
    assert_eq!(Arc::strong_count(&flag), 1);
}

// Upload batching P4/P7: the actual drain must consume preceding frame
// writes before its own table copies, and leave later writes for the scene.
#[test]
fn frame_writes_land_before_lightmap_drain_copies_and_later_writes_wait_for_scene() {
    let Some(mut stream) = Stream::new(
        "frame_writes_land_before_lightmap_drain_copies_and_later_writes_wait_for_scene",
        &[(8, 8)],
        1,
    ) else {
        return;
    };
    // Deliberately share the table target to make reversed command order
    // observable: the drain's resident entry must replace these older bytes.
    stream
        .queue
        .write_buffer(stream.state.table(), 16, &[0xff; 16]);
    assert_eq!(stream.queue.counts().writes, 1);
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    stream.queue.assert_empty("completed lightmap drain");
    assert_eq!(stream.queue.counts().submits, 1);
    assert_eq!(stream.queue.counts().batches, 1);
    assert_eq!(stream.table(), stream.state.model().table_bytes());

    let later = [0x5a; 16];
    stream.queue.write_buffer(stream.state.table(), 16, &later);
    assert_eq!(stream.queue.counts().writes, 2);
    assert_eq!(stream.queue.counts().batches, 1);
    stream.queue.submit(std::iter::empty());
    stream.queue.assert_empty("completed scene submit");
    assert_eq!(stream.queue.counts().batches, 2);
    assert_eq!(&stream.table()[16..32], &later);
    stream.queue.submit(std::iter::empty());
    assert_eq!(
        stream.queue.counts().batches,
        2,
        "each frame write lands once"
    );
}

// Upload batching P1: an execution failure rolls back the real placement
// model while preserving staged frame bytes for the next submit that frame.
#[test]
fn failed_lightmap_growth_rolls_back_without_consuming_pending_frame_writes() {
    let Some(mut stream) = Stream::new(
        "failed_lightmap_growth_rolls_back_without_consuming_pending_frame_writes",
        &[(WIDE, 1100), (WIDE, 1200)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let previous_table = stream.table();
    let mut sh = crate::render::UploadOrderSh::new(&stream.ctx.device, &stream.ctx.queue, 128);
    sh.drain(&stream.ctx.device, &stream.queue, 0);
    let counts = stream.queue.counts();
    // Force the execution guard to reject a growth the CPU model admitted.
    // Unlike a stale-batch rejection, this exercises abort_drain's rollback.
    stream.state.max_array_layers = 2;
    let target = stream.ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pending frame write across failed lightmap drain"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let bytes = [1, 2, 3, 4];
    stream.queue.write_buffer(&target, 0, &bytes);
    let error = stream
        .try_drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]))
        .expect_err("execution guard refuses the candidate generation");
    assert_eq!(
        error,
        LightmapResidencyDrainError::GpuCapacity {
            required_layers: 3,
            max_layers: 2,
        }
    );
    assert_eq!(stream.queue.counts().writes, counts.writes + 1);
    assert_eq!(stream.queue.counts().submits, counts.submits);
    assert_eq!(stream.queue.counts().batches, counts.batches);
    assert!(stream.state.model().is_resident(0));
    assert!(!stream.state.model().is_resident(1));
    assert_eq!(stream.table(), previous_table);
    sh.drain(&stream.ctx.device, &stream.queue, 1);
    stream
        .queue
        .assert_empty("SH growth submit after failed lightmap drain");
    assert_eq!(stream.queue.counts().submits, counts.submits + 2);
    assert_eq!(
        stream.queue.counts().batches_first,
        counts.batches_first + 1
    );
    assert_eq!(stream.queue.counts().batches, counts.batches + 1);
    assert_eq!(read_buffer(&stream.ctx, &target), bytes);
    stream.queue.submit(std::iter::empty());
    assert_eq!(stream.queue.counts().batches, counts.batches + 1);
}

#[test]
fn empty_lightmap_drain_does_not_acquire_frame_staging_or_submit() {
    let Some(mut stream) = Stream::new(
        "empty_lightmap_drain_does_not_acquire_frame_staging_or_submit",
        &[(8, 8)],
        1,
    ) else {
        return;
    };
    let pool = stream.queue.pool_counts();
    stream.drain(stream.reset(vec![], vec![]));
    assert_eq!(stream.queue.counts().writes, 0);
    assert_eq!(stream.queue.counts().submits, 0);
    assert_eq!(stream.queue.counts().batches, 0);
    assert_eq!(stream.queue.pool_counts(), pool);
}

// Upload batching P4: use both real drain implementations, including the
// SH sparse growth's first submit, before the frame's consuming commands.
#[test]
fn lightmap_then_sh_growth_then_scene_each_consumes_only_its_preceding_frame_writes() {
    let Some(mut stream) = Stream::new(
        "lightmap_then_sh_growth_then_scene_each_consumes_only_its_preceding_frame_writes",
        &[(8, 8)],
        1,
    ) else {
        return;
    };
    let mut sh = crate::render::UploadOrderSh::new(&stream.ctx.device, &stream.ctx.queue, 128);
    sh.drain(&stream.ctx.device, &stream.queue, 0);
    let before = stream.queue.counts();

    let source = sh.retained_source();
    stream.queue.write_buffer(&source, 0, &[1, 0, 0, 0]);
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    stream.queue.assert_empty("lightmap drain consumed W1");
    assert_eq!(&read_buffer(&stream.ctx, &source)[..4], &[1, 0, 0, 0]);
    assert_eq!(stream.queue.counts().writes, before.writes + 1);
    assert_eq!(stream.queue.counts().batches, before.batches + 1);
    assert_eq!(
        stream.queue.counts().batches_first,
        before.batches_first + 1
    );

    stream.queue.write_buffer(&source, 0, &[2, 0, 0, 0]);
    let submits = stream.queue.counts().submits;
    sh.drain(&stream.ctx.device, &stream.queue, 1);
    stream.queue.assert_empty("SH drain consumed W2");
    assert_eq!(
        stream.queue.counts().submits,
        submits + 2,
        "growth and install both submit"
    );
    assert_eq!(stream.queue.counts().writes, before.writes + 2);
    assert_eq!(stream.queue.counts().batches, before.batches + 2);
    assert_eq!(
        stream.queue.counts().batches_first,
        before.batches_first + 2
    );
    let grown_source = sh.retained_source();
    assert_eq!(&read_buffer(&stream.ctx, &grown_source)[..4], &[2, 0, 0, 0]);

    // A frame command consumes the new backing: W3 must precede that command,
    // after the already-completed lightmap and SH copies.
    stream.queue.write_buffer(&grown_source, 0, &[3, 0, 0, 0]);
    let output = stream.ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("scene consumes W3 after both drains"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut encoder = stream
        .ctx
        .device
        .create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&grown_source, 0, &output, 0, 4);
    stream.queue.submit([encoder.finish()]);
    stream.queue.assert_empty("frame submit consumed W3");
    assert_eq!(stream.queue.counts().writes, before.writes + 3);
    assert_eq!(stream.queue.counts().batches, before.batches + 3);
    assert_eq!(
        stream.queue.counts().batches_first,
        before.batches_first + 3
    );
    assert_eq!(read_buffer(&stream.ctx, &output), [3, 0, 0, 0]);
    stream.queue.submit(std::iter::empty());
    assert_eq!(stream.queue.counts().batches, before.batches + 3);
    eprintln!("[UploadProof] real lightmap/SH/frame submit order: 1 adapter case ran");
}
