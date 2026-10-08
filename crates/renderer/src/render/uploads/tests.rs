// GPU proofs for deferred upload order, submit lifetime, and pool bounds.
// See: context/plans/in-progress/per-frame-upload-batching/index.md

use super::*;
use crate::render::gpu_test_harness::{GpuCtx, try_init_gpu};

fn target(ctx: &GpuCtx, size: u64) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("upload ordering target"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
fn read(queue: &UploadQueue, ctx: &GpuCtx, source: &wgpu::Buffer) -> Vec<u8> {
    let output = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("upload proof readback"),
        size: source.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &output, 0, source.size());
    queue.submit([encoder.finish()]);
    let (send, recv) = std::sync::mpsc::channel();
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    recv.recv().unwrap().unwrap();
    let bytes = output
        .slice(..)
        .get_mapped_range()
        .expect("buffer mapped for readback")
        .to_vec();
    output.unmap();
    bytes
}

#[test]
fn staged_same_range_overlap_and_a_b_a_keep_program_order() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    for (writes, expected) in [
        (vec![(0, vec![1; 8]), (0, vec![2; 8])], vec![2; 8]),
        (
            vec![(0, vec![1; 8]), (4, vec![2; 4])],
            vec![1, 1, 1, 1, 2, 2, 2, 2],
        ),
        (
            vec![(0, vec![1; 8]), (0, vec![2; 8]), (0, vec![1; 8])],
            vec![1; 8],
        ),
    ] {
        let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
        let target = target(&ctx, 8);
        for (offset, bytes) in &writes {
            queue.write_buffer(&target, *offset, bytes);
        }
        assert_eq!(queue.counts().writes, writes.len() as u64);
        assert_eq!(read(&queue, &ctx, &target), expected);
        assert_eq!(
            queue.counts().copies,
            writes.len() as u64,
            "the frame batch never merges or deduplicates"
        );
        assert_eq!(queue.counts().batches_first, 1);
        queue.assert_empty("ordering proof exit");
    }
    eprintln!("[UploadProof] staged_same_range_overlap_and_a_b_a: 3 adapter cases ran");
}

#[test]
fn zero_writes_and_empty_skip_acquire_and_submit_nothing_extra() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    queue.flush_skipped_frame();
    assert_eq!(queue.counts().submits, 0);
    let encoder = ctx.device.create_command_encoder(&Default::default());
    queue.submit_unbatched([encoder.finish()], "empty dev-tools submit");
    assert_eq!(queue.counts().batches, 0);
    assert_eq!(queue.pool_counts(), (0, 0, 0));
    let target = target(&ctx, 8);
    queue.write_buffer(&target, 0, &[3; 8]);
    queue.flush_skipped_frame();
    assert_eq!(queue.counts().submits, 2);
    assert_eq!(queue.counts().batches, 1);
    assert_eq!(queue.pool_counts().0, 1);
    queue.flush_skipped_frame();
    assert_eq!(queue.counts().submits, 2);
}

#[test]
fn writes_between_three_submits_land_once_in_the_first_submit_after_each_write() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 8);
    for value in [1, 2, 3] {
        queue.write_buffer(&target, 0, &[value; 8]);
        assert_eq!(read(&queue, &ctx, &target), [value; 8]);
        queue.assert_empty("drain/frame submit");
        assert_eq!(queue.counts().writes, u64::from(value));
        assert_eq!(queue.counts().batches, u64::from(value));
        assert_eq!(queue.counts().batches_first, u64::from(value));
    }
    let before = queue.counts().batches;
    assert_eq!(read(&queue, &ctx, &target), [3; 8]);
    assert_eq!(
        queue.counts().batches,
        before,
        "later growth/drain submits carry no consumed writes"
    );
}

#[test]
fn rejected_alignment_and_bounds_writes_fail_before_commit() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 8);
    for (offset, bytes) in [(1, vec![0; 4]), (0, vec![0; 3]), (8, vec![0; 4])] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || queue.write_buffer(&target, offset, &bytes)
            ))
            .is_err()
        );
        assert_eq!(queue.counts().writes, 0);
        queue.assert_empty("rejected write");
    }
}

#[cfg(debug_assertions)]
#[test]
fn direct_write_and_forbidden_submits_reject_pending_uploads() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 8);
    queue.write_buffer(&target, 0, &[1; 8]);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || queue.direct_write_buffer(&target, 0, &[2; 8])
        ))
        .is_err()
    );
    for boundary in [
        "splash",
        "PNG readback",
        "dev-tools",
        "level install",
        "level unload",
        "hot-reload commit",
        "frame exit",
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || queue.submit_unbatched(std::iter::empty(), boundary)
            ))
            .is_err()
        );
    }
    assert_eq!(read(&queue, &ctx, &target), [1; 8]);
    assert_eq!(queue.counts().direct_writes, 0);
}

#[test]
fn submitted_staging_stays_checked_out_until_its_completion_callback() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 8);
    queue.write_buffer(&target, 0, &[1; 8]);
    let mut first_encoder = ctx.device.create_command_encoder(&Default::default());
    let first_staging = queue
        .batch
        .borrow_mut()
        .record_reusing(
            &mut queue.pool.borrow_mut(),
            &ctx.device,
            &mut first_encoder,
        )
        .unwrap();
    let first_identity = first_staging.clone();
    let first_submit = queue.submit([first_encoder.finish()]);
    queue.pool.borrow().recycle(first_staging);
    // Native map callbacks need a later submit/poll. GPU work may already be done;
    // this proves callback exclusion without assuming that the GPU is still busy.
    assert_eq!(queue.pool.borrow().allocation_counts().callback_windows, 0);
    assert_eq!(queue.pool.borrow().free_storage().0, 0);
    queue.write_buffer(&target, 0, &[2; 8]);
    let mut second_encoder = ctx.device.create_command_encoder(&Default::default());
    let second_staging = queue
        .batch
        .borrow_mut()
        .record_reusing(
            &mut queue.pool.borrow_mut(),
            &ctx.device,
            &mut second_encoder,
        )
        .unwrap();
    assert_eq!(
        queue.counts().writes,
        2,
        "both writes use the real frame batch"
    );
    assert_ne!(first_identity, second_staging);
    let second_submit = queue.submit([second_encoder.finish()]);
    // The second submit may deliver the first callback. Waiting on the first
    // submission guarantees its return, while the second buffer remains checked out.
    ctx.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(first_submit),
            timeout: None,
        })
        .unwrap();
    assert_eq!(queue.pool.borrow().allocation_counts().callback_windows, 1);
    assert_eq!(queue.pool.borrow().free_storage().0, 1);
    let before = queue.pool_counts().1;
    let reused = queue.pool.borrow_mut().acquire(&ctx.device, 8);
    assert_eq!(reused, first_identity);
    assert_eq!(queue.pool_counts().1, before);
    reused.unmap();
    queue.pool.borrow().recycle(reused);
    queue.pool.borrow().recycle(second_staging);
    ctx.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(second_submit),
            timeout: None,
        })
        .unwrap();
    assert_eq!(queue.pool.borrow().allocation_counts().callback_windows, 3);
    assert_eq!(read(&queue, &ctx, &target), [2; 8]);
    eprintln!(
        "[UploadProof] submitted-staging lifetime: 1 adapter case ran; distinct next-acquire identity before callback, original identity after completion; GPU lag is not forced"
    );
}

#[test]
fn two_frames_in_flight_reach_a_bounded_staging_steady_state() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 4096);
    let bytes = [7u8; 4096];
    let mut submissions = std::collections::VecDeque::with_capacity(3);
    let mut warmed = None;
    for frame in 0..160 {
        queue.write_buffer(&target, 0, &bytes);
        submissions.push_back(queue.submit(std::iter::empty()));
        if submissions.len() > 2 {
            let index = submissions.pop_front().unwrap();
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(index),
                    timeout: None,
                })
                .unwrap();
        }
        assert!(queue.pool_counts().2 <= 4);
        if frame == 32 {
            warmed = Some(queue.pool_counts().1);
        }
        if frame > 32 {
            assert_eq!(
                queue.pool_counts().1,
                warmed.unwrap(),
                "no new driver staging buffers after warmup"
            );
        }
    }
    assert_eq!(queue.counts().writes, 160);
    assert_eq!(queue.counts().batches, 160);
    eprintln!(
        "[UploadProof] two-in-flight: created={} live={} bytes/frame={} copies/frame=1",
        queue.pool_counts().1,
        queue.pool_counts().2,
        bytes.len()
    );
}

// Regression: four small free buffers forced every larger frame to allocate fresh staging.
#[test]
fn full_small_pool_transitions_to_allocation_free_larger_frames() {
    const WARMUP: usize = 32;
    const MEASURED: usize = 128;
    const LARGE_BYTES: usize = 1024 * 1024;
    const FRAMES_IN_FLIGHT: usize = 2;
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, LARGE_BYTES as u64);
    let mut initial = Vec::with_capacity(FRAME_STAGING_MAX_BUFFERS);
    // Check out all small batches before any callback can return one. This
    // reproduces the full small free list even on an immediately completing GPU.
    for _ in 0..FRAME_STAGING_MAX_BUFFERS {
        queue.write_buffer(&target, 0, &[1; 8]);
        assert_eq!(queue.batch.borrow().copy_count(), 1);
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        let staging = queue
            .batch
            .borrow_mut()
            .record_reusing(&mut queue.pool.borrow_mut(), &ctx.device, &mut encoder)
            .unwrap();
        initial.push((encoder.finish(), staging));
    }
    for (command, staging) in initial {
        queue.submit([command]);
        queue.pool.borrow().recycle(staging);
    }
    queue.submit_unbatched(std::iter::empty(), "small pool callback progress");
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let initial_free = queue.pool.borrow().free_storage();
    assert_eq!(initial_free.0, FRAME_STAGING_MAX_BUFFERS);
    assert_eq!(
        initial_free.2,
        FRAME_STAGING_MIN_BYTES * FRAME_STAGING_MAX_BUFFERS as u64
    );
    assert_eq!(queue.pool_counts().1, FRAME_STAGING_MAX_BUFFERS as u64);

    let bytes = vec![7u8; LARGE_BYTES];
    let mut submissions: [Option<wgpu::SubmissionIndex>; FRAMES_IN_FLIGHT] =
        std::array::from_fn(|_| None);
    let mut warmed_created = 0;
    let mut warmed_capacities = None;
    let mut proof_before = None;
    let mut writer_allocs = 0;
    for frame in 0..WARMUP + MEASURED {
        if let Some(index) = submissions[frame % FRAMES_IN_FLIGHT].take() {
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(index),
                    timeout: None,
                })
                .unwrap();
        }
        if frame == WARMUP {
            warmed_created = queue.pool_counts().1;
            warmed_capacities = Some(queue.batch.borrow().storage_capacities());
            proof_before = Some(queue.pool.borrow().allocation_counts());
        }
        let (_, allocs) =
            allocation_tests::measure_storage(|| queue.write_buffer(&target, 0, &bytes));
        if frame >= WARMUP {
            writer_allocs += allocs;
        }
        submissions[frame % FRAMES_IN_FLIGHT] = Some(queue.submit(std::iter::empty()));
        let free = queue.pool.borrow().free_storage();
        assert!(free.0 <= FRAME_STAGING_MAX_BUFFERS);
        assert_eq!(free.1, FRAME_STAGING_MAX_BUFFERS);
        assert!(free.2 <= FRAME_STAGING_MAX_BYTES);
        if frame >= WARMUP {
            assert_eq!(queue.pool_counts().1, warmed_created);
            // The cap bounds free storage, not live buffers (free + in flight). Small
            // buffers are evicted only when a return finds the free list full, so
            // whether frame 1's map callback lands before frame 2 acquires decides if
            // live settles at four or five. Both are allocation-free; bound the
            // buffers in flight instead.
            let checked_out = queue.pool_counts().2 - free.0 as u64;
            assert!(checked_out <= FRAMES_IN_FLIGHT as u64);
            assert_eq!(
                queue.batch.borrow().storage_capacities(),
                warmed_capacities.unwrap()
            );
        }
    }
    for index in submissions.into_iter().flatten() {
        ctx.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .unwrap();
    }
    let before = proof_before.unwrap();
    let after = queue.pool.borrow().allocation_counts();
    assert_eq!(writer_allocs, 0);
    assert_eq!(after.acquire_allocs - before.acquire_allocs, 0);
    assert_eq!(after.recycle_allocs - before.recycle_allocs, 0);
    assert_eq!(after.callback_allocs - before.callback_allocs, 0);
    assert!(after.callback_windows - before.callback_windows >= MEASURED as u64);
    assert_eq!(
        queue.counts().writes,
        (FRAME_STAGING_MAX_BUFFERS + WARMUP + MEASURED) as u64
    );
    assert_eq!(queue.counts().batches, (WARMUP + MEASURED) as u64);
    let free = queue.pool.borrow().free_storage();
    assert_eq!(free.0 as u64, queue.pool_counts().2);
    assert!(free.2 >= 2 * LARGE_BYTES as u64);
    eprintln!(
        "[UploadProof] full-small-pool transition: 1 adapter case ran; measured_frames={MEASURED} bytes_per_frame={LARGE_BYTES} created={warmed_created} free_buffers={} free_bytes={}; renderer writer/acquire/recycle/callback allocations=0; wgpu API internals excluded",
        free.0, free.2
    );
}

fn record_pool_transition_batch(
    queue: &UploadQueue,
    ctx: &GpuCtx,
    target: &wgpu::Buffer,
    bytes: &[u8],
) -> (wgpu::CommandBuffer, wgpu::Buffer, usize) {
    let (_, writer_allocs) =
        allocation_tests::measure_storage(|| queue.write_buffer(target, 0, bytes));
    assert_eq!(queue.batch.borrow().copy_count(), 1);
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    let staging = queue
        .batch
        .borrow_mut()
        .record_reusing(&mut queue.pool.borrow_mut(), &ctx.device, &mut encoder)
        .unwrap();
    (encoder.finish(), staging, writer_allocs)
}

// Regression: one cap-sized transient forced every subsequent smaller frame pair to allocate.
#[test]
fn transient_class_yields_to_two_smaller_batches_in_either_callback_order() {
    const MIB: usize = 1024 * 1024;
    const WARMUP: usize = 32;
    const MEASURED: usize = 128;
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let large_bytes = vec![3; 3 * MIB];
    for requested_bytes in [MIB, 2 * MIB] {
        let regular_bytes = vec![7; requested_bytes];
        for callback_order in [[0, 1], [1, 0]] {
            let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
            let target = target(&ctx, (3 * MIB) as u64);
            let [(large_command, large, _), (small_command, small, _)] =
                [large_bytes.as_slice(), regular_bytes.as_slice()]
                    .map(|bytes| record_pool_transition_batch(&queue, &ctx, &target, bytes));
            assert_eq!(large.size(), FRAME_STAGING_MAX_BYTES);
            assert_eq!(small.size(), requested_bytes as u64);
            queue.submit([large_command]);
            queue.submit([small_command]);
            let mut returning = [Some(large), Some(small)];
            // Deliver each real map callback separately to force both orders.
            for index in callback_order {
                queue
                    .pool
                    .borrow()
                    .recycle(returning[index].take().unwrap());
                ctx.device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .unwrap();
            }
            assert_eq!(queue.pool.borrow().free_storage().0, 1);
            assert_eq!(
                queue.pool.borrow().free_storage().2,
                FRAME_STAGING_MAX_BYTES
            );
            assert_eq!(queue.pool_counts().2, 1);

            let mut warmed_created = 0;
            let mut warmed_capacities = None;
            let mut proof_before = None;
            let mut record_before = (0, 0);
            let mut writer_allocs = 0;
            let mut submissions: [Option<wgpu::SubmissionIndex>; 2] = std::array::from_fn(|_| None);
            for frame in 0..WARMUP + MEASURED {
                // Wait only on two frames back; the other submit stays in flight.
                if let Some(index) = submissions[frame % 2].take() {
                    ctx.device
                        .poll(wgpu::PollType::Wait {
                            submission_index: Some(index),
                            timeout: None,
                        })
                        .unwrap();
                }
                if frame == WARMUP {
                    warmed_created = queue.pool_counts().1;
                    warmed_capacities = Some(queue.batch.borrow().storage_capacities());
                    proof_before = Some(queue.pool.borrow().allocation_counts());
                    record_before = queue.batch.borrow().record_allocation_counts();
                }
                let (_, allocs) = allocation_tests::measure_storage(|| {
                    queue.write_buffer(&target, 0, &regular_bytes)
                });
                submissions[frame % 2] = Some(queue.submit(std::iter::empty()));
                let free = queue.pool.borrow().free_storage();
                assert!(free.0 <= FRAME_STAGING_MAX_BUFFERS);
                assert_eq!(free.1, FRAME_STAGING_MAX_BUFFERS);
                assert!(free.2 <= FRAME_STAGING_MAX_BYTES);
                assert!(queue.pool_counts().2 <= 2);
                if frame >= WARMUP {
                    assert_eq!(queue.pool_counts().2, 2);
                    writer_allocs += allocs;
                    assert_eq!(queue.pool_counts().1, warmed_created);
                    assert_eq!(
                        queue.batch.borrow().storage_capacities(),
                        warmed_capacities.unwrap()
                    );
                }
            }
            for index in submissions.into_iter().flatten() {
                ctx.device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(index),
                        timeout: None,
                    })
                    .unwrap();
            }
            let free = queue.pool.borrow().free_storage();
            assert_eq!(free.0, 2);
            assert_eq!(free.2, 2 * requested_bytes as u64);
            let before = proof_before.unwrap();
            let after = queue.pool.borrow().allocation_counts();
            let record = queue.batch.borrow().record_allocation_counts();
            assert_eq!(warmed_created, 4);
            assert_eq!(writer_allocs, 0);
            assert_eq!(after.acquire_allocs - before.acquire_allocs, 0);
            assert_eq!(after.recycle_allocs - before.recycle_allocs, 0);
            assert_eq!(after.callback_allocs - before.callback_allocs, 0);
            assert!(after.callback_windows - before.callback_windows >= MEASURED as u64);
            assert_eq!(record.0 - record_before.0, MEASURED as u64);
            assert_eq!(record.1 - record_before.1, 0);
            assert_eq!(queue.counts().writes, (2 + WARMUP + MEASURED) as u64);
            assert_eq!(queue.counts().batches, (WARMUP + MEASURED) as u64);
            eprintln!(
                "[UploadProof] transient-to-smaller-pairs: 1 adapter case ran; bytes_per_batch={requested_bytes} callback_order={callback_order:?} measured_frames={MEASURED} created={warmed_created}; renderer writer/acquire/record/recycle/callback allocations=0; wgpu API internals excluded"
            );
        }
    }
}

#[test]
fn larger_free_class_remains_reusable_when_another_requested_class_fits() {
    const MIB: u64 = 1024 * 1024;
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, 2 * MIB);
    let bytes = vec![7; 2 * MIB as usize];
    let (command, larger, _) = record_pool_transition_batch(&queue, &ctx, &target, &bytes);
    let original = larger.clone();
    queue.submit([command]);
    queue.pool.borrow().recycle(larger);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let created = queue.pool_counts().1;
    let reused = queue.pool.borrow_mut().acquire(&ctx.device, MIB);
    assert_eq!(reused, original);
    assert_eq!(queue.pool_counts().1, created);
    assert_eq!(queue.pool_counts().2, 1);
    reused.unmap();
    queue.pool.borrow().recycle(reused);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    assert_eq!(queue.pool.borrow().free_storage().2, 2 * MIB);
    eprintln!(
        "[UploadProof] larger-class reuse: 1 adapter case ran; 2MiB buffer serves1MiB without fresh allocation"
    );
}

#[test]
fn oversized_frame_gets_fresh_staging_and_returns_within_pool_cap() {
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let target = target(&ctx, FRAME_STAGING_MAX_BYTES * 2);
    queue.write_buffer(&target, 0, &[1; 8]);
    queue.flush_skipped_frame();
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let before = queue.pool_counts().1;
    let retained_before = queue.pool.borrow().free_storage();
    queue.write_buffer(&target, 0, &vec![2; FRAME_STAGING_MAX_BYTES as usize + 4]);
    queue.flush_skipped_frame();
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    assert_eq!(queue.pool_counts().1, before + 1);
    assert!(queue.pool_counts().2 <= FRAME_STAGING_MAX_BUFFERS as u64);
    assert_eq!(queue.pool.borrow().free_storage(), retained_before);
    assert_eq!(queue.pool_counts().2, retained_before.0 as u64);
}

#[test]
fn larger_return_evicts_enough_smaller_buffers_to_preserve_the_byte_cap() {
    const MIB: u64 = 1024 * 1024;
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let mut pool = StagingPool::bounded(MIB, FRAME_STAGING_MAX_BYTES, FRAME_STAGING_MAX_BUFFERS);
    let buffers: Vec<_> = (0..FRAME_STAGING_MAX_BUFFERS)
        .map(|_| pool.acquire(&ctx.device, MIB))
        .collect();
    for buffer in buffers {
        buffer.unmap();
        pool.recycle(buffer);
    }
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    assert_eq!(pool.free_storage().2, FRAME_STAGING_MAX_BYTES);

    let larger = pool.acquire(&ctx.device, 2 * MIB);
    let larger_identity = larger.clone();
    larger.unmap();
    pool.recycle(larger);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    assert_eq!(pool.free_storage(), (3, FRAME_STAGING_MAX_BUFFERS, 4 * MIB));
    assert_eq!(pool.counts(), (5, 5, 3));
    assert_eq!(pool.allocation_counts().callback_allocs, 0);
    let reused = pool.acquire(&ctx.device, 2 * MIB);
    assert_eq!(reused, larger_identity);
    assert_eq!(pool.counts().1, 5);
    reused.unmap();
    pool.recycle(reused);
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    eprintln!(
        "[UploadProof] byte-cap admission: 1 adapter case ran; two small buffers evicted, larger identity reused, renderer callbacks allocate nothing"
    );
}
