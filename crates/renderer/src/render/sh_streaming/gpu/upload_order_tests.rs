//! GPU proofs of production streamed SH upload and growth submission order.

use super::UploadOrderSh;
use crate::render::gpu_test_harness::{GpuCtx, try_init_gpu_with_features};
use crate::render::uploads::UploadQueue;

fn read_first_word(device: &wgpu::Device, queue: &UploadQueue, source: &wgpu::Buffer) -> [u8; 4] {
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SH growth ordering readback"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &output, 0, 4);
    queue.submit_unbatched([encoder.finish()], "SH ordering readback");
    let (send, receive) = std::sync::mpsc::channel();
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap()
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let bytes = output.slice(..).get_mapped_range()[..4].try_into().unwrap();
    output.unmap();
    bytes
}

fn assert_staged_writer_site(
    queue: &UploadQueue,
    file: &str,
    source: &str,
    call: &str,
    expected: u64,
) {
    let offset = source
        .find(call)
        .unwrap_or_else(|| panic!("writer must remain in {file}: {call}"));
    let line = source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32
        + 1;
    let writes: u64 = queue
        .writer_counts()
        .iter()
        .filter(|writer| writer.is_at(file, line))
        .map(|writer| writer.writes)
        .sum();
    assert_eq!(writes, expected, "actual staged writer {file}:{line}");
}

#[test]
fn sh_multi_submit_drain_consumes_frame_batch_before_growth_and_never_replays_it() {
    let Some(GpuCtx { device, queue: raw }) =
        try_init_gpu_with_features(wgpu::Features::TEXTURE_COMPRESSION_BC)
    else {
        return;
    };
    for (dense_slots, expected_submits, expected_growths) in [(128, 2, 1), (64, 3, 2)] {
        let mut sh = UploadOrderSh::new(&device, &raw, dense_slots);
        let queue = UploadQueue::new(&device, raw.clone(), true);
        sh.drain(&device, &queue, 0);
        let before = queue.counts();
        let marker = [7, 0, 0, 0];
        queue.write_buffer(&sh.retained_source(), 0, &marker);
        assert_eq!(queue.counts().writes, before.writes + 1);
        sh.drain(&device, &queue, 1);
        queue.assert_empty("multi-submit SH drain exit");
        let after = queue.counts();
        assert_eq!(after.submits, before.submits + expected_submits);
        assert_eq!(after.batches, before.batches + 1);
        assert_eq!(after.batches_first, before.batches_first + 1);
        assert_eq!(sh.growth_events(), expected_growths);
        // Sparse growth copies the marker from the old backing into the new
        // one. Reversing uploads and growth would leave the new sentinel zero.
        assert_eq!(
            read_first_word(&device, &queue, &sh.retained_source()),
            marker
        );
        queue.submit(std::iter::empty());
        assert_eq!(
            queue.counts().batches,
            after.batches,
            "later drain/frame submits carry no replay"
        );
    }
    eprintln!("[UploadProof] SH multi-submit growth: 2 adapter cases ran");
}

#[test]
fn streamed_indirect_compose_grid_uses_the_frame_batch_after_real_residency_install() {
    let Some(GpuCtx { device, queue: raw }) =
        try_init_gpu_with_features(wgpu::Features::TEXTURE_COMPRESSION_BC)
    else {
        return;
    };
    let mut sh = UploadOrderSh::new(&device, &raw, 128);
    let queue = UploadQueue::new(&device, raw, true);
    sh.drain(&device, &queue, 0);
    sh.compose(&device, &queue);
    assert_staged_writer_site(
        &queue,
        "sh_streaming/gpu/indirect/runtime.rs",
        include_str!("indirect/runtime.rs"),
        "queue.write_buffer(&self.grid_buffer",
        1,
    );
    assert_eq!(queue.counts().writes, 1);
    assert_eq!(queue.counts().batches, 1);
    assert_eq!(queue.counts().batches_first, 1);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    eprintln!("[UploadProof] real streamed compose grid: 1 adapter case ran");
}

#[test]
fn streamed_compose_inventory_counts_every_indirect_promotion_and_animated_writer_site() {
    let Some(GpuCtx { device, queue: raw }) =
        try_init_gpu_with_features(wgpu::Features::TEXTURE_COMPRESSION_BC)
    else {
        return;
    };
    let mut sh = UploadOrderSh::with_direct_compose(&device, &raw);
    let queue = UploadQueue::new(&device, raw, true);
    sh.drain(&device, &queue, 0);
    sh.compose(&device, &queue);
    assert_staged_writer_site(
        &queue,
        "sh_streaming/gpu/indirect/runtime.rs",
        include_str!("indirect/runtime.rs"),
        "queue.write_buffer(&self.grid_buffer",
        1,
    );
    let promotion = include_str!("../direct_compose/passes.rs");
    assert_staged_writer_site(
        &queue,
        "sh_streaming/direct_compose/passes.rs",
        promotion,
        "queue.write_buffer(\n                &self.light_term_mask",
        1,
    );
    assert_staged_writer_site(
        &queue,
        "sh_streaming/direct_compose/passes.rs",
        promotion,
        "queue.write_buffer(&self.debug_override",
        1,
    );
    // Pass A and Pass B share this writer, but both production dispatches
    // must run: their row counters are also asserted by the fixture.
    assert_staged_writer_site(
        &queue,
        "sh_streaming/direct_compose/passes.rs",
        promotion,
        "queue.write_buffer(grid_buffer",
        2,
    );
    assert_staged_writer_site(
        &queue,
        "sh_streaming/direct_compose/passes/animated_runtime.rs",
        include_str!("../direct_compose/passes/animated_runtime.rs"),
        "queue.write_buffer(&self.light_scale",
        1,
    );
    assert_eq!(queue.counts().writes, 6);
    assert_eq!(queue.counts().direct_writes, 0);
    assert_eq!(queue.counts().batches, 1);
    assert_eq!(queue.counts().batches_first, 1);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    eprintln!("[UploadProof] streamed compose inventory: all 6 writer calls at 5 source sites ran");
}
