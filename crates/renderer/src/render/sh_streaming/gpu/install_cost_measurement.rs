//! Measurement helper: CPU time of SH drains over drains of several chunk
//! sizes and counts, then the same drains with the compose their installs
//! trigger. The model is a cost per cluster chunk and per MiB; a per-drain
//! term fits negative and is not offered. A drain during which a staging pool
//! (the SH pools' or the frame queue's) created a buffer runs at the cold rate
//! and is listed apart, wherever it falls. Prints only.
//!
//! Run: cargo test --release -p postretro-renderer --lib sh_install_cost_model \
//!        -- --ignored --nocapture --test-threads=1
//!
//! Synthetic chunks are all L0 probes: per brick, two 64-slot Rgba16Float
//! atlas blocks (id 34 and id 35 bases), 64 probe patches and one sparse row
//! of one L2 tile in each of ids 27, 41 and 45.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeMap;
use std::time::Instant;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::PreparedShCluster;
use wgpu::util::DeviceExt;

use super::*;
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::gpu_test_harness::bc_gpu_or_skip;
use crate::render::install_cost_model::{
    CostRun, DrainClass, MIB, SHOWCASE_PRICES, Shape, mark_drain, run_context, shuffled,
    staging_buffers_created,
};
use crate::render::{ShSampleRegion, ShSampleRegionSets};
use postretro_render_cpu::frame_uniforms::LightTermMask;

/// Bricks per cluster along each axis: 1, 4, 16 and 64 bricks per chunk.
const CLUSTER_BRICKS: [[u32; 3]; 4] = [[1, 1, 1], [2, 2, 1], [4, 4, 1], [4, 4, 4]];
/// Drain totals the shapes aim at, in MiB.
const TOTALS_MIB: [u32; 6] = [2, 4, 8, 12, 16, 32];
const ROUNDS: usize = 12;
const IDLE_FRAMES: usize = 8;

/// One synthetic map of `cluster_bricks` clusters, `clusters` of them in a
/// square-ish sheet, every cluster's chunk prepared once up front.
struct Rig {
    fixture: UploadOrderSh,
    chunks: Vec<DecodedClusterShPayload>,
    binding: wgpu::BindGroup,
    regions: [ShSampleRegion; 1],
}

fn sheet(cluster_bricks: [u32; 3], clusters: u32) -> SyntheticMap {
    let across = (f64::from(clusters).sqrt().ceil() as u32).max(1);
    let down = clusters.div_ceil(across).max(1);
    SyntheticMap::new(
        [
            4 * cluster_bricks[0] * across,
            4 * cluster_bricks[1] * down,
            4 * cluster_bricks[2],
        ],
        cluster_bricks,
    )
}

impl Rig {
    fn new(device: &wgpu::Device, raw: &wgpu::Queue, map: SyntheticMap) -> Self {
        // Pools that hold the whole map: no dense growth in a measured drain.
        let dense_slots = map.state().whole_dense_slots;
        let fixture = UploadOrderSh::with_map(device, raw, dense_slots, map);
        let chunks = (0..fixture.map.cluster_count())
            .map(|cluster| fixture.prepared(cluster).chunk)
            .collect();
        let grid = fixture.map.base().grid_dimensions;
        let mask = LightTermMask::ALL;
        // Bytes 88..92 are `FrameUniforms::light_term_mask` (`build_uniform_data`).
        let mut uniform_bytes = [0u8; postretro_render_cpu::frame_uniforms::UNIFORM_SIZE];
        uniform_bytes[88..92].copy_from_slice(&mask.bits().to_ne_bytes());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("SH install-cost uniforms"),
            contents: &uniform_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH install-cost uniform binding"),
            layout: &fixture.uniforms,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            fixture,
            chunks,
            binding,
            // The whole grid samples, so every installed row composes.
            regions: [ShSampleRegion::new(
                glam::Vec3::ZERO,
                glam::Vec3::from_array(grid.map(|axis| axis as f32)),
            )],
        }
    }

    fn created(&self, queue: &UploadQueue) -> u64 {
        let pools = self.fixture.state.gpu.as_ref().expect("GPU pools");
        staging_buffers_created(&[&pools.staging], queue)
    }

    fn growth_events(&self) -> u64 {
        self.fixture.growth_events()
    }

    fn scratch_capacity(&self) -> usize {
        self.fixture
            .state
            .gpu
            .as_ref()
            .expect("GPU pools")
            .upload_scratch
            .capacity()
    }

    /// The batch that installs `ready` and evicts `retiring` in one drain,
    /// every ready cluster mandatory, as the controller sends a burst it must
    /// admit. Production receives it already owned from the decode pool, so
    /// callers build it before starting a timer.
    fn batch(&self, ready: &[u32], retiring: &[u32]) -> ShDrainBatch {
        let state = &self.fixture.state;
        let prepared = ready
            .iter()
            .map(|&cluster| PreparedShCluster {
                generation: state.generation,
                content_tag: state.content_tag,
                chunk: self.chunks[cluster as usize].clone(),
            })
            .collect();
        ShDrainBatch {
            generation: state.generation,
            content_tag: state.content_tag,
            target_add: ready.to_vec(),
            target_remove: retiring.to_vec(),
            evictions: retiring.to_vec(),
            mandatory: ready.to_vec(),
            ready: prepared,
            ..ShDrainBatch::default()
        }
    }

    /// `ShResidencyState::drain` on a batch built beforehand.
    fn drain(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        batch: ShDrainBatch,
    ) -> postretro_level_loader::ShDrainOutcome {
        let f = &mut self.fixture;
        f.state
            .drain(device, queue, &mut f.sh, &f.weights, batch)
            .expect("production SH drain")
    }

    /// The frame's compose after a drain, as production records it with
    /// default debug overrides: plan, encode every pass, submit.
    fn compose(&mut self, device: &wgpu::Device, queue: &UploadQueue) {
        let mask = LightTermMask::ALL;
        let promotion = DirectShDebugOverride::default();
        let animated = AnimatedDirectShDebugOverride::default();
        let state = &mut self.fixture.state;
        state
            .prepare_compose_frame(
                ShSampleRegionSets {
                    visible_cells: &postretro_visibility::VisibleCells::DrawAll,
                    fog_cells: &[],
                    movers: &self.regions,
                },
                None,
                false,
                false,
                true,
                false,
                false,
                animated.active(),
                mask,
                promotion,
                animated,
                &[],
                &[],
                &[],
            )
            .expect("streamed compose plan");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("install-cost compose"),
        });
        state
            .dispatch_indirect_compose(queue, &mut encoder, &self.binding, false, mask, None)
            .expect("streamed indirect compose");
        if state.direct_compose_required {
            state
                .dispatch_direct_compose(
                    queue,
                    &mut encoder,
                    &self.binding,
                    true,
                    mask,
                    promotion,
                    animated,
                    &[],
                    None,
                    None,
                )
                .expect("streamed direct compose");
        }
        queue.submit([encoder.finish()]);
    }

    /// Rows the last compose wrote, across its passes.
    fn rows_composed(&self) -> u64 {
        let snapshot = self.fixture.state.snapshot();
        snapshot.indirect_compose.rows_composed
            + snapshot.static_direct_compose.rows_composed
            + snapshot.animated_direct_compose.rows_composed
    }
}

/// What one measured frame cost: the drain alone, the production install
/// counter inside it, and drain plus compose.
struct Frame {
    install_micros: u64,
    drain_micros: u64,
    total_micros: u64,
    drain_class: DrainClass,
    total_class: DrainClass,
    rows_composed: u64,
}

/// Evict `retiring` in a drain of its own, so the measured drain after it
/// carries installs only, then wait for the GPU. Returns its CPU time.
fn evict(rig: &mut Rig, device: &wgpu::Device, queue: &UploadQueue, retiring: &[u32]) -> u64 {
    let batch = rig.batch(&[], retiring);
    let started = Instant::now();
    let outcome = rig.drain(device, queue, batch);
    let micros = started.elapsed().as_micros() as u64;
    assert_eq!(
        outcome.evicted.len(),
        retiring.len(),
        "every cluster evicts"
    );
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    micros
}

/// Install `ready` in one drain, compose, then wait for the GPU as a
/// presented frame would.
fn frame(rig: &mut Rig, device: &wgpu::Device, queue: &UploadQueue, ready: &[u32]) -> Frame {
    let created_before = rig.created(queue);
    let growth_before = rig.growth_events();
    let scratch_before = rig.scratch_capacity();
    let batch = rig.batch(ready, &[]);
    let started = Instant::now();
    let outcome = rig.drain(device, queue, batch);
    let drain_micros = started.elapsed().as_micros() as u64;
    let created_after_drain = rig.created(queue);
    let compose_started = Instant::now();
    rig.compose(device, queue);
    let compose_micros = compose_started.elapsed().as_micros() as u64;
    let created_after_compose = rig.created(queue);
    let rows_composed = rig.rows_composed();
    assert_eq!(
        outcome.accepted.len(),
        ready.len(),
        "every cluster installs"
    );
    let excluded = if rig.growth_events() > growth_before {
        Some("pool growth")
    } else if rig.scratch_capacity() > scratch_before {
        Some("upload scratch grew (CPU allocation)")
    } else {
        None
    };
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    Frame {
        install_micros: rig.fixture.state.install_cpu.last_drain_micros,
        drain_micros,
        total_micros: drain_micros + compose_micros,
        drain_class: DrainClass::of(created_before, created_after_drain, excluded),
        total_class: DrainClass::of(created_before, created_after_compose, excluded),
        rows_composed,
    }
}

/// Chunk counts for each total, one shape per distinct count; a total the
/// chunk size misses by more than half is left out.
fn counts(chunk_bytes: u64) -> BTreeMap<u32, u32> {
    let mut nearest: BTreeMap<u32, (f64, u32)> = BTreeMap::new();
    for total in TOTALS_MIB {
        let target = f64::from(total) * MIB;
        let items = (target / chunk_bytes as f64).round() as u32;
        let error = (f64::from(items) * chunk_bytes as f64 - target).abs() / target;
        if items == 0 || error > 0.5 {
            continue;
        }
        let entry = nearest.entry(items).or_insert((error, total));
        if error < entry.0 {
            *entry = (error, total);
        }
    }
    nearest
        .into_iter()
        .map(|(items, (_, total))| (items, total))
        .collect()
}

#[test]
#[ignore = "measurement helper; needs a GPU"]
fn sh_install_cost_model() {
    let Some(ctx) = bc_gpu_or_skip("sh_install_cost_model") else {
        return;
    };
    let device = &ctx.device;
    let mut install = CostRun::new(
        "SH install alone (production install_cpu counter: install_ready)",
        Vec::new(),
    );
    let mut drain = CostRun::new(
        "SH install drain (ShResidencyState::drain on a batch built beforehand, clones excluded; evictions run in a drain before it)",
        Vec::new(),
    );
    let mut composed = CostRun::new(
        "SH drain (batch built beforehand) + the compose its installs trigger (plan, encode, submit)",
        Vec::new(),
    );
    let mut notes = Vec::new();
    for (config, cluster_bricks) in CLUSTER_BRICKS.into_iter().enumerate() {
        // A two-cluster probe map sizes the chunk; chunks are uniform.
        let chunk_bytes = Rig::new(device, &ctx.queue, sheet(cluster_bricks, 2)).chunks[0]
            .bytes
            .len() as u64;
        let counts = counts(chunk_bytes);
        let max_items = counts
            .keys()
            .copied()
            .max()
            .expect("a count per chunk size");
        let mut rig = Rig::new(device, &ctx.queue, sheet(cluster_bricks, 2 * max_items));
        let queue = UploadQueue::new(device, ctx.queue.clone(), true);
        let bricks: u32 = cluster_bricks.iter().product();
        let first_shape = install.shapes.len();
        let mut items_of = Vec::new();
        for (&items, &total) in &counts {
            let shape = Shape {
                label: format!("{items} x {bricks}-brick"),
                items,
                bytes: u64::from(items) * chunk_bytes,
                nominal_mib: total,
            };
            install.shapes.push(shape.clone());
            drain.shapes.push(shape.clone());
            composed.shapes.push(shape);
            items_of.push(items);
        }

        // Size the sparse pools to the whole map once, then empty them:
        // growth is the warm-up's, never a measured drain's.
        let all: Vec<u32> = (0..rig.fixture.map.cluster_count()).collect();
        let warmup = frame(&mut rig, device, &queue, &all);
        evict(&mut rig, device, &queue, &all);
        notes.push(format!(
            "{bricks}-brick chunks of {:.1} KiB, {} clusters: warm-up install of all {:.1} ms \
             ({:?}), not in the model",
            chunk_bytes as f64 / 1024.0,
            all.len(),
            warmup.total_micros as f64 / 1000.0,
            warmup.total_class,
        ));

        let mut retiring: Vec<u32> = Vec::new();
        let mut set = 0;
        let mut rows_per_item = Vec::new();
        let mut evict_per_cluster = Vec::new();
        for round in 0..ROUNDS {
            for local in shuffled(items_of.len(), 0x5eed + (config * 100 + round) as u64) {
                let items = items_of[local];
                let ready: Vec<u32> = (0..items).map(|i| set * max_items + i).collect();
                set ^= 1;
                if !retiring.is_empty() {
                    let micros = evict(&mut rig, device, &queue, &retiring);
                    evict_per_cluster.push(micros as f64 / retiring.len() as f64);
                }
                let index = composed.samples.len();
                mark_drain(device, &queue, "sh", index, "begin");
                let measured = frame(&mut rig, device, &queue, &ready);
                mark_drain(device, &queue, "sh", index, "end");
                assert!(measured.rows_composed > 0, "installs compose their rows");
                rows_per_item.push(measured.rows_composed as f64 / f64::from(items));
                let shape = first_shape + local;
                install.record(shape, measured.install_micros, measured.drain_class);
                drain.record(shape, measured.drain_micros, measured.drain_class);
                composed.record(shape, measured.total_micros, measured.total_class);
                retiring = ready;
            }
        }
        // Frames that install nothing: the compose a frame pays anyway.
        evict(&mut rig, device, &queue, &retiring);
        let mut idle: Vec<u64> = (0..IDLE_FRAMES)
            .map(|_| frame(&mut rig, device, &queue, &[]).total_micros)
            .collect();
        idle.sort_unstable();
        rows_per_item.sort_by(f64::total_cmp);
        evict_per_cluster.sort_by(f64::total_cmp);
        notes.push(format!(
            "  rows composed per installed chunk: median {:.1}; eviction drain median {:.1} µs per \
             cluster; idle frame (no install) drain + compose median {:.3} ms",
            rows_per_item[rows_per_item.len() / 2],
            evict_per_cluster[evict_per_cluster.len() / 2],
            idle[idle.len() / 2] as f64 / 1000.0,
        ));
    }
    let context = run_context(
        device,
        "synthetic L0 SH chunks: Rgba16Float atlas slots plus probe patches and sparse rows of \
         ids 27, 41 and 45",
    );
    for note in &notes {
        println!("{note}");
    }
    composed.write_sequence("sh");
    install.report(&context, &SHOWCASE_PRICES);
    drain.report(&context, &SHOWCASE_PRICES);
    composed.report(&context, &SHOWCASE_PRICES);
}

// Pin P12: a drain needing a staging class larger than every earlier drain's
// finds none in the pool, so it runs cold and never counts warm. Both pools
// the SH drain can draw from are watched, as in the measurement.
#[test]
fn sh_install_cost_run_reports_a_drain_larger_than_every_earlier_one_cold() {
    let Some(ctx) =
        bc_gpu_or_skip("sh_install_cost_run_reports_a_drain_larger_than_every_earlier_one_cold")
    else {
        return;
    };
    let device = &ctx.device;
    let cluster_bricks = [2, 2, 1];
    let chunk_bytes = Rig::new(device, &ctx.queue, sheet(cluster_bricks, 2)).chunks[0]
        .bytes
        .len() as u64;
    // One small chunk fits the pool's smallest staging class; the large drain
    // carries about 9 MiB of chunks, a class several times larger.
    assert!(
        chunk_bytes < MIB as u64 / 2,
        "a chunk is far below the smallest class"
    );
    let large = (9.0 * MIB / chunk_bytes as f64).ceil() as u32;
    let mut rig = Rig::new(device, &ctx.queue, sheet(cluster_bricks, large + 1));
    let queue = UploadQueue::new(device, ctx.queue.clone(), true);
    let small_set = vec![large];
    let large_set: Vec<u32> = (0..large).collect();
    let mut classes = Vec::new();
    let mut retiring: Vec<u32> = Vec::new();
    // Three drains only. Whether a repeat of the large drain later runs warm
    // is the pool's retention under load, not the classifier: with other GPU
    // tests running, the pool can drop its free buffers and recreate them.
    for ready in [&small_set, &small_set, &large_set] {
        if !retiring.is_empty() {
            evict(&mut rig, device, &queue, &retiring);
        }
        classes.push(frame(&mut rig, device, &queue, ready).drain_class);
        retiring = ready.clone();
    }
    assert!(matches!(classes[0], DrainClass::Cold { .. }), "{classes:?}");
    assert_eq!(classes[1], DrainClass::Warm, "{classes:?}");
    assert!(
        matches!(classes[2], DrainClass::Cold { .. }),
        "the largest drain so far is cold: {classes:?}"
    );
}
