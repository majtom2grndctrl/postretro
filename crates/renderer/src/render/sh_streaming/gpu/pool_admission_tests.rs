//! GPU proofs of capacity admission: evict, then grow, then refuse. Real
//! drains over a pool the fixture sizes, because only the buffers show a zero
//! landing after the install it was meant to precede.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::collections::BTreeSet;

use postretro_test_log_capture::LogCapture;

use super::limit_tests::{
    finish_retirement, first_texture_pixel, hold_dense_retirement, replace_pools, replace_shape,
};
use super::shared_storage_tests::{gpu_compose_words, gpu_sampled_words};
use super::*;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};

/// One L0 brick: the unit these pools are sized in.
const BRICK: u32 = 64;
const OUTGREW_DENSE: &str = "mandatory demand outgrew the SH dense pool";

/// A row of one-brick clusters with no sparse payload, so dense capacity is
/// the only question.
fn bricks(count: u32) -> SyntheticMap {
    let rows: Vec<u32> = (0..count).collect();
    SyntheticMap::new([4 * count, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &rows)
}

/// A row of one-brick clusters, each with a sparse row of one entry, so sparse
/// capacity counts too.
fn bricks_with_rows(count: u32) -> SyntheticMap {
    SyntheticMap::new([4 * count, 4, 4], [1, 1, 1]).direct_base_only()
}

fn pool_of(layers: u32) -> AtlasShape {
    AtlasShape {
        tiles_per_row: 8,
        tiles_per_layer: BRICK,
        layers,
        slots: BRICK * layers,
    }
}

/// `map` over a pool of `layers` one-brick layers, up to the device's layers.
fn fixture(
    device: &wgpu::Device,
    raw: &wgpu::Queue,
    map: SyntheticMap,
    layers: u32,
) -> UploadOrderSh {
    let mut f = UploadOrderSh::with_map(device, raw, 128, map);
    replace_shape(&mut f, device, raw, &device.limits(), pool_of(layers));
    f
}

/// `map` over the pool production sizes from a stored bound of `dense_slots`,
/// with production's whole-map cap.
fn bound_sized_fixture(
    device: &wgpu::Device,
    raw: &wgpu::Queue,
    map: SyntheticMap,
    dense_slots: u32,
) -> UploadOrderSh {
    let mut f = UploadOrderSh::with_map(device, raw, 128, map);
    f.state.pool_bound.dense_slots = dense_slots;
    f.state.pool_bound.sparse = BTreeMap::from([(27, (0, 0))]);
    f.state
        .initialize_gpu_with_limits(
            device,
            raw,
            &f.map.base(),
            &f.map.sources(),
            false,
            &mut f.sh,
            &f.pipelines,
            &f.weights,
            &device.limits(),
        )
        .unwrap();
    f
}

/// One-tile layers, so the whole-map cap is exact and fragmentation shows.
fn exact_pool(slots: u32) -> AtlasShape {
    AtlasShape {
        tiles_per_row: 1,
        tiles_per_layer: 1,
        layers: slots,
        slots,
    }
}

/// One 8-tile cluster, then three 1-tile clusters with no sparse rows, in a pool
/// exactly the map's size. One-tile layers make the cap exact, so fragmentation
/// shows.
fn map_sized_pool(device: &wgpu::Device, raw: &wgpu::Queue) -> UploadOrderSh {
    let map = SyntheticMap::new([16, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &[0, 1, 2, 3])
        .with_density_levels(&[1, 2, 2, 2]);
    let mut f = UploadOrderSh::with_map(device, raw, 128, map);
    let whole = f.state.whole_dense_slots;
    assert_eq!(whole, 11);
    replace_shape(&mut f, device, raw, &device.limits(), exact_pool(whole));
    let pools = f.state.gpu.as_mut().unwrap();
    let sparse = (
        pools.sparse_entry_capacity(27),
        pools.sparse_tile_f16_capacity(27),
    );
    pools.whole_map = WholeMapCap {
        dense_slots: whole,
        sparse: BTreeMap::from([(27, sparse)]),
    };
    f
}

/// Install `clusters` with room to spare, as an earlier frame's work.
fn settle_residents(
    f: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    clusters: &[u32],
) {
    let outcome = f
        .drain_batch(device, queue, ShDrainBatch::default(), clusters)
        .unwrap();
    assert_eq!(outcome.accepted, clusters);
    f.compose(device, queue);
    f.drain_batch(device, queue, ShDrainBatch::default(), &[])
        .unwrap();
}

fn admit(
    f: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    mandatory: &[u32],
    victims: &[u32],
    ready: &[u32],
) -> postretro_level_loader::ShDrainOutcome {
    f.drain_batch(
        device,
        queue,
        ShDrainBatch {
            mandatory: mandatory.to_vec(),
            victims: victims.to_vec(),
            ..ShDrainBatch::default()
        },
        ready,
    )
    .unwrap()
}

fn deferred_ids(outcome: &postretro_level_loader::ShDrainOutcome) -> Vec<u32> {
    outcome
        .deferred
        .iter()
        .map(|prepared| prepared.chunk.cluster_id)
        .collect()
}

#[test]
fn full_pool_evicts_optional_lowest_priority_first() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH capacity eviction order") else {
        return;
    };
    let mut f = fixture(&device, &raw, bricks(4), 2);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);

    // The list's order is the policy; one brick's room takes one victim.
    let outcome = admit(&mut f, &device, &queue, &[2], &[1, 0], &[2]);
    assert_eq!(outcome.evicted, [1]);
    assert_eq!(outcome.accepted, [2]);
    assert!(f.state.installed.contains_key(&0));
    assert_eq!(f.growth_events(), 0);

    // Optional work on a full pool is refused: nothing it could displace is
    // named, and it neither grows the pool nor touches mandatory residents.
    let outcome = admit(&mut f, &device, &queue, &[], &[], &[3]);
    assert_eq!(outcome.pool_refused, [3]);
    assert!(outcome.evicted.is_empty() && outcome.refused.is_empty());
    assert!(!f.state.installed.contains_key(&3));
    assert!(f.state.installed.contains_key(&2));
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 2);
    assert_eq!(f.growth_events(), 0);
    eprintln!("[ShPoolProof] victim order and optional refusal ran on GPU");
}

#[test]
fn optional_work_never_evicts_even_with_victims_named() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH optional refusal") else {
        return;
    };
    let mut f = fixture(&device, &raw, bricks(4), 2);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);

    // Mandatory work elsewhere in the batch named these victims. An optional
    // install has no claim on them, so it refuses and the list goes unused.
    let outcome = admit(&mut f, &device, &queue, &[1], &[0], &[3]);
    assert_eq!(outcome.pool_refused, [3]);
    assert!(outcome.evicted.is_empty());
    assert_eq!(f.state.installed.len(), 2);
    eprintln!("[ShPoolProof] optional install left named victims alone; ran on GPU");
}

#[test]
fn a_victim_sharing_everything_frees_nothing_and_the_walk_continues() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH shared victim") else {
        return;
    };
    // Cluster 1 names brick 0 too, so cluster 0 holds nothing alone.
    let map = bricks(3).with_halo_brick(1, 0);
    let mut f = fixture(&device, &raw, map, 2);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);
    let held = f.state.node_slots.len();

    let outcome = admit(&mut f, &device, &queue, &[2], &[0, 1], &[2]);
    assert_eq!(outcome.evicted, [0, 1], "the walk went past a free of zero");
    assert_eq!(outcome.accepted, [2]);
    assert_eq!(held, 2);
    assert_eq!(f.state.node_slots.len(), 1);
    assert_eq!(f.growth_events(), 0);
    eprintln!("[ShPoolProof] victim sharing every node freed nothing, walk continued; ran on GPU");
}

#[test]
fn a_victim_holding_what_the_install_shares_is_counted_after_it_goes() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH victim shared with install")
    else {
        return;
    };
    // Mandatory cluster 1 names bricks 0 and 1. Resident cluster 0 holds brick
    // 0, so evicting it takes away storage the install was counting on.
    let map = bricks(3).with_halo_brick(1, 0);
    let mut f = fixture(&device, &raw, map, 2);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 2]);
    let shared = f.map.cluster_probes(0);

    let outcome = admit(&mut f, &device, &queue, &[1], &[0, 2], &[1]);
    assert_eq!(outcome.evicted, [0, 2], "one brick freed, two now needed");
    assert_eq!(outcome.accepted, [1]);

    // Each zero landed before the retry reused its index: reversed, the
    // eviction's zero would win on the GPU and strand the shared probes.
    let words = gpu_compose_words(&f, &device, &queue);
    assert_eq!(words, f.state.compose_words);
    assert!(shared.iter().all(|&dense| words[dense as usize] != 0));
    let sampled = gpu_sampled_words(&f, &device, &queue);
    assert_eq!(sampled, f.state.sampled_words);
    assert!(sampled.iter().all(|&word| word == 0));
    eprintln!("[ShPoolProof] eviction zeros preceded the retry on the real queue; ran on GPU");
}

#[test]
fn past_capacity_grows_once_and_warns_by_name() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH growth by name") else {
        return;
    };
    let capture = LogCapture::start();
    let mut f = fixture(&device, &raw, bricks(5), 2);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);

    // Two of the tier's clusters are ready and one is still being read. The
    // first shortfall sizes for all of them, so the others need no second
    // growth, and doubling's four layers would overshoot this tier's five.
    let outcome = admit(&mut f, &device, &queue, &[0, 1, 2, 3, 4], &[], &[2, 3]);
    assert_eq!(outcome.accepted, [2, 3]);
    assert!(outcome.refused.is_empty() && outcome.deferred.is_empty());
    assert_eq!(f.growth_events(), 1);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 5);
    capture.assert_logged_once(log::Level::Warn, OUTGREW_DENSE);

    let outcome = admit(&mut f, &device, &queue, &[0, 1, 2, 3, 4], &[], &[4]);
    assert_eq!(outcome.accepted, [4]);
    assert_eq!(f.growth_events(), 1);
    capture.assert_logged_once(log::Level::Warn, OUTGREW_DENSE);
    eprintln!("[ShPoolProof] one growth to the whole tier, warned by name; ran on GPU");
}

#[test]
fn growth_counts_shared_storage_once() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH union sizing") else {
        return;
    };
    // Clusters 1 and 2 both name brick 0 and their own brick. Summed they
    // would read four bricks with cluster 0; the union is three.
    let map = bricks(3).with_halo_brick(1, 0).with_halo_brick(2, 0);
    let mut f = fixture(&device, &raw, map, 1);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);

    let outcome = admit(&mut f, &device, &queue, &[0, 1, 2], &[], &[1, 2]);
    assert_eq!(outcome.accepted, [1, 2]);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 3);
    assert_eq!(f.growth_events(), 1);
    eprintln!("[ShPoolProof] growth took the unique union; ran on GPU");
}

#[test]
fn a_sparse_shortfall_evicts_and_grows_like_a_dense_one() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH sparse admission") else {
        return;
    };
    let capture = LogCapture::start();
    let map = SyntheticMap::new([16, 4, 4], [1, 1, 1]).direct_base_only();
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
    let stride = delta_probe_f16_stride(4) as u32;
    // Room for two rows and everything dense: only the rows can run short.
    let capacities = BTreeMap::from([(27, (3, 2 + 2 * stride))]);
    replace_pools(
        &mut f,
        &device,
        &raw,
        &device.limits(),
        pool_of(4),
        &capacities,
    );
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);

    let outcome = admit(&mut f, &device, &queue, &[2], &[0], &[2]);
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [2]);
    assert_eq!(f.growth_events(), 0);

    let outcome = admit(&mut f, &device, &queue, &[1, 2, 3], &[], &[3]);
    assert_eq!(outcome.accepted, [3]);
    assert_eq!(f.growth_events(), 1);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 4);
    capture.assert_logged_once(log::Level::Warn, "outgrew the SH sparse id-27 pool");
    capture.assert_not_logged(log::Level::Warn, OUTGREW_DENSE);
    eprintln!("[ShPoolProof] sparse rows evicted, then grew once; ran on GPU");
}

#[test]
fn capacity_order_is_evict_grow_refuse() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH evict grow refuse") else {
        return;
    };
    let queue = UploadQueue::new(&device, raw.clone(), true);

    // Evict: the victim covers the shortfall, so nothing grows.
    let mut f = fixture(&device, &raw, bricks(4), 2);
    settle_residents(&mut f, &device, &queue, &[0, 1]);
    let outcome = admit(&mut f, &device, &queue, &[2], &[0, 1], &[2]);
    assert_eq!((outcome.evicted, outcome.accepted), (vec![0], vec![2]));
    assert_eq!(f.growth_events(), 0);

    // Grow: the list runs out first, so every victim goes before any growth.
    let mut f = fixture(&device, &raw, bricks(4), 2);
    settle_residents(&mut f, &device, &queue, &[0, 1]);
    let outcome = admit(&mut f, &device, &queue, &[1, 2, 3], &[0], &[2, 3]);
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [2, 3]);
    assert_eq!(f.growth_events(), 1);

    // Refuse: past the device, what could be evicted was evicted first.
    // Cluster 3 names three bricks where two layers can ever exist.
    let map = bricks(4).with_halo_brick(3, 1).with_halo_brick(3, 2);
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
    let mut limits = device.limits();
    limits.max_texture_array_layers = 2;
    replace_shape(&mut f, &device, &raw, &limits, pool_of(2));
    settle_residents(&mut f, &device, &queue, &[0]);
    let outcome = admit(&mut f, &device, &queue, &[3], &[0], &[3]);
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.refused, [3]);
    assert!(outcome.accepted.is_empty());
    assert_eq!(f.growth_events(), 0);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 2);
    eprintln!("[ShPoolProof] evict, then grow, then refuse each ran on GPU");
}

#[test]
fn deferred_behind_retiring_generation_evicts_and_grows_nothing() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH deferred behind retirement")
    else {
        return;
    };
    let mut f = fixture(&device, &raw, bricks(4), 1);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);
    let outcome = admit(&mut f, &device, &queue, &[0, 1], &[], &[1]);
    assert_eq!(outcome.accepted, [1]);
    assert_eq!(f.growth_events(), 1);
    let retirement = hold_dense_retirement(&mut f);

    // A named victim would make room, and the frames keep offering it.
    for _ in 0..3 {
        let outcome = admit(&mut f, &device, &queue, &[1, 3], &[0], &[3]);
        assert_eq!(deferred_ids(&outcome), [3]);
        assert!(outcome.evicted.is_empty() && outcome.accepted.is_empty());
        assert_eq!(f.growth_events(), 1);
        assert_eq!(f.state.installed.len(), 2);
    }

    retirement.store(true, Ordering::Release);
    let outcome = admit(&mut f, &device, &queue, &[1, 3], &[0], &[3]);
    assert_eq!(outcome.accepted, [3]);
    assert!(f.state.gpu.as_ref().unwrap().retiring_dense.is_none());
    finish_retirement(&mut f, &device);
    eprintln!("[ShPoolProof] deferred install waited out the generation, then landed; ran on GPU");
}

// Regression: a sparse refusal threw away the dense family's wait, so the drain
// evicted every frame until the fence cleared.
#[test]
fn a_retiring_wait_outranks_another_familys_refusal() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH wait beside refusal") else {
        return;
    };
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, bricks_with_rows(4));
    let stride = delta_probe_f16_stride(4) as u32;
    // Room for two rows, and no more once the cap below stands.
    let rows = (3, 2 + 2 * stride);
    let capacities = BTreeMap::from([(27, rows)]);
    replace_pools(
        &mut f,
        &device,
        &raw,
        &device.limits(),
        pool_of(1),
        &capacities,
    );
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);
    let outcome = admit(&mut f, &device, &queue, &[0, 1], &[], &[1]);
    assert_eq!(outcome.accepted, [1]);
    assert_eq!(f.growth_events(), 1);
    let retirement = hold_dense_retirement(&mut f);
    f.state
        .gpu
        .as_mut()
        .unwrap()
        .whole_map
        .sparse
        .insert(27, rows);

    // Dense is short behind the fence, and sparse is short past its cap.
    for _ in 0..3 {
        let outcome = admit(&mut f, &device, &queue, &[1, 2], &[0], &[2]);
        assert_eq!(deferred_ids(&outcome), [2]);
        assert!(outcome.evicted.is_empty() && outcome.whole_map_refused.is_empty());
        assert_eq!(f.state.installed.len(), 2);
    }

    retirement.store(true, Ordering::Release);
    let outcome = admit(&mut f, &device, &queue, &[1, 2], &[0], &[2]);
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [2]);
    finish_retirement(&mut f, &device);
    eprintln!("[ShPoolProof] a retiring wait held its victims past a sparse refusal; ran on GPU");
}

#[test]
fn growth_with_generation_retiring_keeps_contents_until_done() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH retiring contents") else {
        return;
    };
    let mut f = fixture(&device, &raw, bricks(3), 1);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);
    let marker = [0, 60, 0, 56, 0, 52, 0, 60];
    let mut uploads = f.state.begin_uploads(marker.len());
    uploads
        .write_texture(
            &f.state.gpu.as_ref().unwrap().total,
            wgpu::Origin3d::ZERO,
            &marker,
            8,
            1,
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        )
        .unwrap();
    f.state.submit_uploads(uploads, &device, &queue);

    let outcome = admit(&mut f, &device, &queue, &[0, 1], &[], &[1]);
    assert_eq!(outcome.accepted, [1]);
    let retirement = hold_dense_retirement(&mut f);
    let retiring_bytes = f.state.gpu.as_ref().unwrap().retiring_capacity_bytes();
    assert!(retiring_bytes > 0);

    // A second growth is needed and the first generation has not retired.
    let outcome = admit(&mut f, &device, &queue, &[0, 1, 2], &[], &[2]);
    assert_eq!(deferred_ids(&outcome), [2]);
    let pools = f.state.gpu.as_ref().unwrap();
    assert_eq!(pools.retiring_capacity_bytes(), retiring_bytes);
    assert_eq!(
        first_texture_pixel(
            &device,
            &queue,
            &pools.retiring_dense.as_ref().unwrap().dense.total
        ),
        marker,
        "the old generation's contents stay until submitted work is done"
    );
    assert_eq!(f.growth_events(), 1);

    retirement.store(true, Ordering::Release);
    let outcome = admit(&mut f, &device, &queue, &[0, 1, 2], &[], &[2]);
    assert_eq!(outcome.accepted, [2]);
    assert_eq!(f.growth_events(), 2);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 3);
    finish_retirement(&mut f, &device);
    assert_eq!(f.state.gpu.as_ref().unwrap().retiring_capacity_bytes(), 0);
    eprintln!("[ShPoolProof] old generation kept its contents, then freed; ran on GPU");
}

#[test]
fn walk_within_bound_never_grows() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH walk within bound") else {
        return;
    };
    const CLUSTERS: u32 = 8;
    let capture = LogCapture::start();
    // The stored bound is the walk's mandatory window of two. Its headroom
    // holds no third brick, so the band read ahead is what must be refused.
    let mut f = bound_sized_fixture(&device, &raw, bricks(CLUSTERS), 2 * BRICK);
    let initial = f.state.gpu.as_ref().unwrap().shape;
    assert_eq!(initial.slots.div_ceil(BRICK), 3);
    assert!(initial.slots < 3 * BRICK);
    let queue = UploadQueue::new(&device, raw, true);
    f.state.targets = BTreeSet::from([0, 1, 2]);
    let outcome = admit(&mut f, &device, &queue, &[0, 1], &[], &[0, 1, 2]);
    assert_eq!(outcome.accepted, [0, 1]);
    assert_eq!(outcome.pool_refused, [2]);

    for camera in 1..CLUSTERS - 1 {
        let lead = camera + 1;
        let band = (lead + 1 < CLUSTERS).then_some(lead + 1);
        let mut ready = vec![lead];
        ready.extend(band);
        f.compose(&device, &queue);
        let outcome = f
            .drain_batch(
                &device,
                &queue,
                ShDrainBatch {
                    target_add: ready.iter().copied().filter(|&id| id > lead).collect(),
                    target_remove: vec![camera - 1],
                    evictions: vec![camera - 1],
                    mandatory: vec![camera, lead],
                    ..ShDrainBatch::default()
                },
                &ready,
            )
            .unwrap();
        assert_eq!(outcome.accepted, [lead], "camera at {camera}");
        assert_eq!(outcome.evicted, [camera - 1]);
        assert_eq!(outcome.pool_refused, band.into_iter().collect::<Vec<_>>());
        assert!(outcome.refused.is_empty() && outcome.deferred.is_empty());
        assert_eq!(f.state.installed.len(), 2);
    }
    assert_eq!(f.growth_events(), 0);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape, initial);
    capture.assert_not_logged(log::Level::Warn, "outgrew");
    eprintln!("[ShPoolProof] walk within the bound never grew; ran on GPU");
}

// AC 30, on the real growth path. The fixture's bound sits under the whole
// map, so a tier past the pool grows, and stops at the map however far the
// tier reads: its figure is the union, and the union cannot pass the map.
#[test]
fn pool_never_exceeds_the_whole_map() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH whole-map cap") else {
        return;
    };
    const CLUSTERS: u32 = 5;
    let mut f = bound_sized_fixture(&device, &raw, bricks(CLUSTERS), BRICK);
    let whole = f.state.whole_dense_slots;
    assert_eq!(whole, CLUSTERS * BRICK);
    let initial = f.state.gpu.as_ref().unwrap().shape;
    assert!(initial.slots < whole);
    let cap = whole.div_ceil(initial.tiles_per_layer) * initial.tiles_per_layer;
    let queue = UploadQueue::new(&device, raw.clone(), true);

    let tier: Vec<u32> = (0..CLUSTERS).collect();
    let outcome = admit(&mut f, &device, &queue, &tier, &[], &tier);
    assert_eq!(outcome.accepted, tier);
    assert!(outcome.refused.is_empty() && outcome.deferred.is_empty());
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.slots, cap);
    finish_retirement(&mut f, &device);

    // A stored bound above the map changes nothing: load caps it too.
    let f = bound_sized_fixture(&device, &raw, bricks(CLUSTERS), u32::MAX);
    let shape = f.state.gpu.as_ref().unwrap().shape;
    assert!(shape.slots >= whole && shape.slots - whole < shape.tiles_per_layer);
    eprintln!("[ShPoolProof] growth and load both stopped at the whole map; ran on GPU");
}

// The fact the controller stops evicting on, read from real capacity in every
// family: false below the map, true from the drain growth takes them all there,
// true at load when the bound does. Mixed node sizes, so placement order could
// strand slots if anything had been freed.
#[test]
fn drains_report_when_the_pool_holds_the_whole_map() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH whole-map report") else {
        return;
    };
    let levels = [1, 2, 0, 2, 1];
    let map = || bricks_with_rows(5).with_density_levels(&levels);
    let at_whole_sparse = |f: &UploadOrderSh| {
        let pools = f.state.gpu.as_ref().unwrap();
        let (entries, tiles) = pools.whole_map.sparse[&27];
        pools.sparse_entry_capacity(27) == entries && pools.sparse_tile_f16_capacity(27) == tiles
    };
    let mut f = bound_sized_fixture(&device, &raw, map(), 16);
    let queue = UploadQueue::new(&device, raw.clone(), true);

    let outcome = admit(&mut f, &device, &queue, &[1], &[], &[1]);
    assert_eq!(outcome.accepted, [1]);
    assert!(!outcome.pool_holds_whole_map);
    assert!(!at_whole_sparse(&f));
    finish_retirement(&mut f, &device);

    let tier = [0, 1, 2, 3, 4];
    let outcome = admit(&mut f, &device, &queue, &tier, &[], &[4, 2, 0, 3]);
    assert_eq!(outcome.accepted, [4, 2, 0, 3]);
    assert!(at_whole_sparse(&f));
    assert!(outcome.pool_holds_whole_map, "growth reached the map");
    finish_retirement(&mut f, &device);

    let mut f = bound_sized_fixture(&device, &raw, map(), u32::MAX);
    assert!(at_whole_sparse(&f));
    let outcome = admit(&mut f, &device, &queue, &tier, &[], &[4, 2, 0, 3, 1]);
    assert_eq!(outcome.accepted, [4, 2, 0, 3, 1]);
    assert!(outcome.refused.is_empty() && outcome.whole_map_refused.is_empty());
    assert_eq!(f.growth_events(), 0);
    assert!(outcome.pool_holds_whole_map, "the bound reached the map");
    eprintln!("[ShPoolProof] drains reported whole-map capacity from real pools; ran on GPU");
}

// Regression: a family a device limit left short still read as whole, so the
// controller stopped evicting and that family refused with evictable rows.
#[test]
fn a_sparse_family_below_the_map_reports_false_and_its_rows_still_evict() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH sparse below the map") else {
        return;
    };
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, bricks_with_rows(4));
    let stride = delta_probe_f16_stride(4) as u32;
    // Dense holds all four clusters; sparse has room for two of their rows.
    let capacities = BTreeMap::from([(27, (3, 2 + 2 * stride))]);
    replace_pools(
        &mut f,
        &device,
        &raw,
        &device.limits(),
        pool_of(4),
        &capacities,
    );
    f.state.gpu.as_mut().unwrap().whole_map = WholeMapCap {
        dense_slots: 4 * BRICK,
        sparse: BTreeMap::from([(27, (5, 2 + 4 * stride))]),
    };
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1]);

    let outcome = admit(&mut f, &device, &queue, &[], &[], &[]);
    assert!(
        !outcome.pool_holds_whole_map,
        "nothing left, but sparse is short"
    );

    let outcome = admit(&mut f, &device, &queue, &[2], &[0], &[2]);
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [2]);
    assert_eq!(f.growth_events(), 0);
    eprintln!("[ShPoolProof] short sparse family reported false and evicted; ran on GPU");
}

// Regression: a pool at the cap stopped evicting for good, so a node larger than
// any hole stayed black until reload. The first release ends the fact, and the
// victims that close the gap keep flowing.
#[test]
fn an_eviction_at_the_cap_ends_the_whole_map_fact_and_victims_still_flow() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH release at the cap") else {
        return;
    };
    let mut f = map_sized_pool(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0, 1, 2]);
    let outcome = admit(&mut f, &device, &queue, &[], &[], &[]);
    assert!(outcome.pool_holds_whole_map, "nothing has left yet");

    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                evictions: vec![0],
                target_remove: vec![0],
                ..ShDrainBatch::default()
            },
            &[3],
        )
        .unwrap();
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [3]);
    assert!(
        !outcome.pool_holds_whole_map,
        "the cap stands, a hole opened"
    );

    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                target_add: vec![0],
                mandatory: vec![0],
                victims: vec![3],
                ..ShDrainBatch::default()
            },
            &[0],
        )
        .unwrap();
    assert_eq!(outcome.evicted, [3]);
    assert_eq!(outcome.accepted, [0]);
    assert!(!outcome.pool_holds_whole_map);
    assert_eq!(f.growth_events(), 0);
    eprintln!("[ShPoolProof] release at the cap ended the fact, victim closed the gap; ran on GPU");
}

// What a pool at the cap can still meet once a range has come back: first-fit
// needs contiguous slots and nothing compacts live storage, so a mandatory
// cluster can be refused with enough free slots in total.
#[test]
fn fragmentation_at_the_cap_evicts_then_refuses_by_name() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH cap fragmentation") else {
        return;
    };
    let capture = LogCapture::start();
    let mut f = map_sized_pool(&device, &raw);
    let whole = f.state.whole_dense_slots;
    let queue = UploadQueue::new(&device, raw, true);

    // The 8-tile cluster goes, and a 1-tile cluster takes its address.
    settle_residents(&mut f, &device, &queue, &[0, 1, 2]);
    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                evictions: vec![0],
                target_remove: vec![0],
                ..ShDrainBatch::default()
            },
            &[3],
        )
        .unwrap();
    assert_eq!((outcome.evicted, outcome.accepted), (vec![0], vec![3]));
    let live: u32 = f.state.node_slots.values().map(|range| range.len).sum();
    assert_eq!(whole - live, 8, "the request's size is free in total");

    // Seven free at the old address and one at the tail: eight, not together.
    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                target_add: vec![0],
                mandatory: vec![0],
                ..ShDrainBatch::default()
            },
            &[0],
        )
        .unwrap();
    assert_eq!(outcome.whole_map_refused, [0]);
    assert!(outcome.refused.is_empty(), "no device limit was reached");
    assert!(outcome.accepted.is_empty());
    assert_eq!(f.growth_events(), 0);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.slots, whole);
    capture.assert_logged_once(
        log::Level::Warn,
        "at the SH dense atlas pool's whole-map cap",
    );

    // A victim is spent before any refusal, and here its slot closes the gap.
    let outcome = admit(&mut f, &device, &queue, &[0], &[3], &[0]);
    assert_eq!(outcome.evicted, [3]);
    assert_eq!(outcome.accepted, [0]);
    eprintln!(
        "[ShPoolProof] stranded slots refused by name at the cap, victim admitted; ran on GPU"
    );
}

// AC 10 (R3), renderer half under a capacity victim. The app half is
// `read_finishing_for_a_capacity_victim_is_dropped`.
#[test]
fn read_finishing_for_an_evicted_victim_is_dropped() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH victim read drop") else {
        return;
    };
    let mut f = fixture(&device, &raw, bricks(3), 1);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);
    let outcome = admit(&mut f, &device, &queue, &[1], &[0], &[1]);
    assert_eq!(outcome.evicted, [0]);
    let held: Vec<_> = f.state.node_slots.keys().copied().collect();

    // The read finishes after the victim left the targets.
    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                target_remove: vec![0],
                ..ShDrainBatch::default()
            },
            &[0],
        )
        .unwrap();
    assert_eq!(outcome.dropped, [0]);
    assert!(outcome.pool_refused.is_empty() && outcome.accepted.is_empty());
    assert!(!f.state.installed.contains_key(&0));
    assert_eq!(f.state.node_slots.keys().copied().collect::<Vec<_>>(), held);
    eprintln!("[ShPoolProof] read for an evicted victim dropped, nothing retained; ran on GPU");
}

// R10: a generation change clears everything the old level held. What the
// loader still has in flight dies at the controller's identity check; see
// `reload_mid_flight_leaves_no_pending_read_from_the_old_level`.
#[test]
fn reload_mid_flight_leaves_nothing_from_the_old_level() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH reload mid flight") else {
        return;
    };
    let map = bricks(3).with_halo_brick(1, 0);
    let mut f = fixture(&device, &raw, map, 1);
    let queue = UploadQueue::new(&device, raw, true);
    settle_residents(&mut f, &device, &queue, &[0]);
    let outcome = admit(&mut f, &device, &queue, &[0, 1], &[], &[1]);
    assert_eq!(outcome.accepted, [1]);
    assert!(!f.state.storage_refs.is_empty());
    let retirement = hold_dense_retirement(&mut f);

    let reset_generation = f.state.generation + 1;
    let words = f.state.cluster_count.div_ceil(64) as usize;
    let content_tag = f.state.content_tag;
    let outcome = f
        .state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.weights,
            ShDrainBatch {
                generation: reset_generation,
                content_tag,
                target_reset: Some(vec![0; words]),
                ..ShDrainBatch::default()
            },
        )
        .unwrap();
    assert_eq!(outcome.accepted.len() + outcome.evicted.len(), 0);
    assert!(f.state.storage_refs.is_empty());
    assert!(f.state.node_slots.is_empty() && f.state.installed.is_empty());
    assert!(f.state.targets.is_empty() && f.state.pending_promotion.is_empty());
    assert!(
        gpu_compose_words(&f, &device, &queue)
            .iter()
            .all(|&word| word == 0)
    );
    assert!(
        gpu_sampled_words(&f, &device, &queue)
            .iter()
            .all(|&word| word == 0)
    );

    // The old generation's textures may still feed submitted work, so they
    // outlive the reset and leave at the next drain after that work is done.
    assert!(f.state.gpu.as_ref().unwrap().retiring_dense.is_some());
    retirement.store(true, Ordering::Release);
    f.state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.weights,
            ShDrainBatch {
                generation: reset_generation,
                content_tag,
                ..ShDrainBatch::default()
            },
        )
        .unwrap();
    assert_eq!(f.state.gpu.as_ref().unwrap().retiring_capacity_bytes(), 0);
    eprintln!(
        "[ShPoolProof] reload cleared references, words and the retiring generation; ran on GPU"
    );
}
