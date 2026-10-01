// Count renderer-owned upload storage work through the approved allocator.
// See: per-frame-upload-batching AC 27/28; testing_guide §Resource bounds

use super::{FRAME_STAGING_MAX_BUFFERS, UploadQueue};
use crate::render::gpu_test_harness::try_init_gpu;
use postretro_sim::alloc_probe::AllocSnapshot;
use std::cell::Cell;

thread_local! {
    // Cumulative foreign allocations on this thread; deltas let nested scopes
    // exclude each allocation once without global test/thread interference.
    static FOREIGN_ALLOCS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn measure_storage<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let foreign_before = FOREIGN_ALLOCS.with(Cell::get);
    let snapshot = AllocSnapshot::arm();
    let result = work();
    let total = snapshot.allocs_since();
    let foreign = FOREIGN_ALLOCS.with(Cell::get).wrapping_sub(foreign_before);
    assert!(
        foreign <= total,
        "foreign allocation windows must nest within measured work"
    );
    (result, total - foreign)
}

/// Excludes only allocations made while a wgpu API is executing. In particular,
/// map_async's boxed callback, mapped-range bookkeeping, command encoding, and
/// driver resource creation are foreign. The callback's renderer pool insertion
/// has its own measured window on whichever thread executes it.
pub(super) fn exclude_wgpu<T>(work: impl FnOnce() -> T) -> T {
    let foreign_before = FOREIGN_ALLOCS.with(Cell::get);
    let snapshot = AllocSnapshot::arm();
    let result = work();
    let total = snapshot.allocs_since();
    let nested = FOREIGN_ALLOCS.with(Cell::get).wrapping_sub(foreign_before);
    assert!(
        nested <= total,
        "nested foreign windows cannot double-count allocations"
    );
    FOREIGN_ALLOCS.with(|count| count.set(count.get().wrapping_add(total - nested)));
    result
}

#[test]
fn allocation_probe_counts_renderer_work_and_excludes_nested_foreign_work() {
    let (_, counted) = measure_storage(|| {
        std::hint::black_box(vec![1u8; 128]);
        exclude_wgpu(|| {
            std::hint::black_box(vec![2u8; 128]);
            exclude_wgpu(|| {
                std::hint::black_box(vec![3u8; 128]);
            });
        });
    });
    assert_eq!(
        counted, 1,
        "renderer allocation must be seen while both foreign allocations are excluded"
    );
}

#[test]
fn warmed_frame_upload_storage_is_allocation_free_with_two_frames_in_flight() {
    const WARMUP: usize = 64;
    const MEASURED: usize = 240;
    const WRITES: usize = 64;
    const WRITE_BYTES: usize = 32;
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[UploadProof] allocation/two-in-flight skipped: no GPU adapter/device");
        return;
    };
    let queue = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let targets = [0, 1].map(|_| {
        ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Allocation proof frame target"),
            size: (WRITES * WRITE_BYTES) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    });
    let mut submissions: [Option<wgpu::SubmissionIndex>; 2] = std::array::from_fn(|_| None);
    let mut capacities = None;
    let mut warmed_created = 0;
    let mut proof_before = None;
    let mut record_before = (0, 0);
    let mut writes_allocated = 0usize;
    let mut max_live = 0u64;
    let mut payload = [0u8; WRITE_BYTES];

    for frame in 0..WARMUP + MEASURED {
        // The test harness waits on two submissions back. Production still
        // recycles by map callbacks and adds no device.poll.
        if let Some(index) = submissions[frame % 2].take() {
            ctx.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(index),
                    timeout: None,
                })
                .unwrap();
        }
        if frame == WARMUP {
            capacities = Some(queue.batch.borrow().storage_capacities());
            warmed_created = queue.pool_counts().1;
            proof_before = Some(queue.pool.borrow().allocation_counts());
            record_before = queue.batch.borrow().record_allocation_counts();
        }
        payload.fill(frame as u8);
        let write_probe = AllocSnapshot::arm();
        for write in 0..WRITES {
            queue.write_buffer(&targets[write % 2], (write * WRITE_BYTES) as u64, &payload);
        }
        let write_allocs = write_probe.allocs_since();
        if frame >= WARMUP {
            writes_allocated += write_allocs;
        }
        submissions[frame % 2] = Some(queue.submit(std::iter::empty()));
        queue.assert_empty("allocation proof completed frame");
        max_live = max_live.max(queue.pool_counts().2);
        assert!(queue.pool_counts().2 <= FRAME_STAGING_MAX_BUFFERS as u64);
        if frame >= WARMUP {
            assert_eq!(
                queue.pool_counts().1,
                warmed_created,
                "no new staging buffer after warmup"
            );
            assert_eq!(
                queue.batch.borrow().storage_capacities(),
                capacities.unwrap(),
                "byte/copy storage must stop growing"
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
    let proof = queue.pool.borrow().allocation_counts();
    let record = queue.batch.borrow().record_allocation_counts();
    assert_eq!(
        writes_allocated, 0,
        "whole renderer write path must not allocate after warmup"
    );
    assert_eq!(
        proof.acquire_windows - before.acquire_windows,
        MEASURED as u64
    );
    assert_eq!(
        proof.recycle_windows - before.recycle_windows,
        MEASURED as u64
    );
    assert!(
        proof.callback_windows - before.callback_windows >= MEASURED as u64,
        "real GPU callbacks must execute; capacities alone are not proof"
    );
    assert_eq!(proof.callback_windows, (WARMUP + MEASURED) as u64);
    assert_eq!(
        proof.acquire_allocs - before.acquire_allocs,
        0,
        "acquire must build no per-call heap list"
    );
    assert_eq!(
        proof.recycle_allocs - before.recycle_allocs,
        0,
        "recycle capture/registration logic must not allocate"
    );
    assert_eq!(
        proof.callback_allocs - before.callback_allocs,
        0,
        "callback free-list insertion must not allocate"
    );
    assert_eq!(record.0 - record_before.0, MEASURED as u64);
    assert_eq!(
        record.1 - record_before.1,
        0,
        "whole renderer record control/storage work must not allocate"
    );
    let free = queue.pool.borrow().free_storage();
    assert_eq!(
        free.0 as u64,
        queue.pool_counts().2,
        "all submitted staging buffers returned to the pool"
    );
    assert_eq!(
        free.1, FRAME_STAGING_MAX_BUFFERS,
        "free-list backing remains preallocated"
    );
    assert_eq!(queue.counts().writes, ((WARMUP + MEASURED) * WRITES) as u64);
    assert_eq!(queue.counts().direct_writes, 0);
    assert_eq!(queue.counts().batches, (WARMUP + MEASURED) as u64);
    assert_eq!(queue.counts().copies, ((WARMUP + MEASURED) * WRITES) as u64);
    eprintln!(
        "[UploadProof] allocation/two-in-flight: 1 adapter case ran; measured_frames={MEASURED} writes_per_frame={WRITES} created={warmed_created} max_live={max_live} storage_capacities={:?}; renderer allocations=0; excluded wgpu API internals, including boxed map_async callback",
        capacities.unwrap()
    );
}
