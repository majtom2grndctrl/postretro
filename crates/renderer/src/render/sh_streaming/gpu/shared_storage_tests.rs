//! GPU proofs that shared SH storage survives the real queue order. CPU
//! mirrors cannot show this: only the buffers reveal a zero that landed after
//! the install it was meant to precede.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeSet;

use super::*;

use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};

fn read_words(
    device: &wgpu::Device,
    queue: &UploadQueue,
    source: &wgpu::Buffer,
    count: usize,
) -> Vec<u32> {
    let size = (count * 4) as u64;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SH shared-storage proof readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, size);
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let words = buffer
        .slice(..)
        .get_mapped_range()
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&word| u32::from_le_bytes(word))
        .collect();
    buffer.unmap();
    words
}

fn gpu_compose_words(f: &UploadOrderSh, device: &wgpu::Device, queue: &UploadQueue) -> Vec<u32> {
    let pools = f.state.gpu.as_ref().unwrap();
    read_words(
        device,
        queue,
        &pools.compose_indirection,
        f.state.compose_words.len(),
    )
}

fn gpu_sampled_words(f: &UploadOrderSh, device: &wgpu::Device, queue: &UploadQueue) -> Vec<u32> {
    let pools = f.state.gpu.as_ref().unwrap();
    read_words(
        device,
        queue,
        &pools.sampled_indirection,
        f.state.sampled_words.len(),
    )
}

/// Brick 0 is shared: cluster 0 names it alone, cluster 1 names it and brick 1.
fn shared_brick_map() -> SyntheticMap {
    SyntheticMap::new([8, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_halo_brick(1, 0)
}

#[test]
fn eviction_zeros_reach_the_queue_before_the_same_drains_install() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH shared word queue order") else {
        return;
    };
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, shared_brick_map());
    let queue = UploadQueue::new(&device, raw, true);
    f.state.targets = BTreeSet::from([0]);
    f.drain(&device, &queue, 0);
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&0));
    let shared = f.map.cluster_probes(0);
    assert!(
        shared
            .iter()
            .all(|&dense| f.state.sampled_words[dense as usize] != 0)
    );

    // Cluster 0 frees the shared brick's words and cluster 1 rewrites those
    // same indices in one drain. Reversed, the zero would win on the GPU.
    let outcome = f
        .drain_batch(
            &device,
            &queue,
            ShDrainBatch {
                target_add: vec![1],
                target_remove: vec![0],
                evictions: vec![0],
                ..Default::default()
            },
            &[1],
        )
        .unwrap();
    assert_eq!(outcome.evicted, [0]);
    assert_eq!(outcome.accepted, [1]);

    let words = gpu_compose_words(&f, &device, &queue);
    assert_eq!(words, f.state.compose_words);
    for &dense in &shared {
        assert_ne!(words[dense as usize], 0, "probe {dense} kept a stale zero");
    }
    // The sampler sees a miss until the new writer's compose lands, not a
    // word for slots compose has not filled.
    let sampled = gpu_sampled_words(&f, &device, &queue);
    assert_eq!(sampled, f.state.sampled_words);
    assert!(shared.iter().all(|&dense| sampled[dense as usize] == 0));

    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&1));
    let sampled = gpu_sampled_words(&f, &device, &queue);
    assert_eq!(sampled, f.state.sampled_words);
    assert!(shared.iter().all(|&dense| sampled[dense as usize] != 0));
    eprintln!("[ShRefProof] shared word survived evict-then-install on the real queue; ran on GPU");
}

#[test]
fn eviction_with_a_live_reference_stages_nothing_and_the_last_reference_clears() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH live reference eviction") else {
        return;
    };
    // Cluster 1 names everything cluster 0 does, so cluster 0 frees nothing.
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, shared_brick_map());
    let queue = UploadQueue::new(&device, raw, true);
    f.drain(&device, &queue, 1);
    f.drain(&device, &queue, 0);
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert_eq!(f.state.sampleable, BTreeSet::from([0, 1]));
    let before = gpu_compose_words(&f, &device, &queue);

    let mut uploads = f.state.begin_uploads(0);
    f.state.evict(&mut uploads, 0).unwrap();
    assert_eq!(uploads.copy_count(), 0, "a live reference clears nothing");
    f.state.submit_uploads(uploads, &device, &queue);
    assert_eq!(gpu_compose_words(&f, &device, &queue), before);

    let mut uploads = f.state.begin_uploads(0);
    f.state.evict(&mut uploads, 1).unwrap();
    assert!(uploads.copy_count() > 0, "the last reference clears");
    f.state.submit_uploads(uploads, &device, &queue);
    let words = gpu_compose_words(&f, &device, &queue);
    assert!(words.iter().all(|&word| word == 0));
    assert!(
        gpu_sampled_words(&f, &device, &queue)
            .iter()
            .all(|&word| word == 0)
    );
    assert!(f.state.node_slots.is_empty());
    eprintln!("[ShRefProof] live reference staged no clears, last reference zeroed; ran on GPU");
}
