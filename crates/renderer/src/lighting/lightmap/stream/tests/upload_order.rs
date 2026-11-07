// GPU proofs that a drain's submission orders against the frame's other uploads.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::*;

// The actual drain consumes the frame's preceding writes before its own table
// copies, and leaves later writes for the scene.
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

// An execution failure rolls back the real placement model while preserving
// staged frame bytes for the next submit that frame.
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

// Both real drain implementations, the SH sparse growth's first submit
// included, ahead of the frame's consuming commands: each submit takes only the
// frame writes staged before it.
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
