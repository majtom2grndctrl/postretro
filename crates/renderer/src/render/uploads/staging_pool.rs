// Recycled mapped staging buffers shared by frame and residency uploads.
// See: context/lib/rendering_pipeline.md §1, §4

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

const MIN_STAGING_BYTES: u64 = 1024 * 1024;
const LARGE_STAGING_STEP_BYTES: u64 = 4 * 1024 * 1024;
const MAX_POOLED_STAGING_BYTES: u64 = 64 * 1024 * 1024;

struct FreeBuffers {
    buffers: Vec<wgpu::Buffer>,
    bytes: u64,
}
struct PoolState {
    free: Mutex<FreeBuffers>,
    live: AtomicU64,
    created: AtomicU64,
    acquired: AtomicU64,
    max_bytes: u64,
    max_buffers: usize,
    #[cfg(test)]
    allocation_proof: AllocationProof,
}

#[cfg(test)]
#[derive(Default)]
struct AllocationProof {
    acquire_windows: AtomicU64,
    acquire_allocs: AtomicU64,
    recycle_windows: AtomicU64,
    recycle_allocs: AtomicU64,
    callback_windows: AtomicU64,
    callback_allocs: AtomicU64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PoolAllocationCounts {
    pub acquire_windows: u64,
    pub acquire_allocs: u64,
    pub recycle_windows: u64,
    pub recycle_allocs: u64,
    pub callback_windows: u64,
    pub callback_allocs: u64,
}

// Test builds count renderer logic around GPU calls, excluding allocations
// owned by wgpu (especially map_async's boxed callback).
#[inline]
fn gpu_call<T>(call: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    {
        super::allocation_tests::exclude_wgpu(call)
    }
    #[cfg(not(test))]
    {
        call()
    }
}

pub(crate) struct StagingPool {
    state: Arc<PoolState>,
    minimum: u64,
}

impl Default for StagingPool {
    fn default() -> Self {
        Self::bounded(MIN_STAGING_BYTES, MAX_POOLED_STAGING_BYTES, 64)
    }
}

impl StagingPool {
    pub(crate) fn bounded(minimum: u64, max_bytes: u64, max_buffers: usize) -> Self {
        Self {
            state: Arc::new(PoolState {
                free: Mutex::new(FreeBuffers {
                    buffers: Vec::with_capacity(max_buffers),
                    bytes: 0,
                }),
                live: AtomicU64::new(0),
                created: AtomicU64::new(0),
                acquired: AtomicU64::new(0),
                max_bytes,
                max_buffers,
                #[cfg(test)]
                allocation_proof: AllocationProof::default(),
            }),
            minimum,
        }
    }

    pub(crate) fn acquire(&mut self, device: &wgpu::Device, len: u64) -> wgpu::Buffer {
        #[cfg(test)]
        {
            let (result, allocs) =
                super::allocation_tests::measure_storage(|| self.acquire_inner(device, len));
            self.state
                .allocation_proof
                .acquire_windows
                .fetch_add(1, Ordering::Relaxed);
            self.state
                .allocation_proof
                .acquire_allocs
                .fetch_add(allocs as u64, Ordering::Relaxed);
            result
        }
        #[cfg(not(test))]
        {
            self.acquire_inner(device, len)
        }
    }

    #[inline]
    fn acquire_inner(&mut self, device: &wgpu::Device, len: u64) -> wgpu::Buffer {
        self.state.acquired.fetch_add(1, Ordering::Relaxed);
        let requested_class = staging_buffer_size_with_minimum(len, self.minimum);
        let mut free = self
            .state
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            let index = free
                .buffers
                .iter()
                .enumerate()
                .filter(|(_, buffer)| gpu_call(|| buffer.size()) >= len)
                .min_by_key(|(_, buffer)| gpu_call(|| buffer.size()))
                .map(|(index, _)| index);
            let Some(index) = index else {
                break;
            };
            let buffer = free.buffers.swap_remove(index);
            let size = gpu_call(|| buffer.size());
            free.bytes -= size;
            // A transient must leave room for two buffers of the current class.
            // Otherwise its returns crowd out the second buffer on every frame pair.
            if size > requested_class
                && requested_class <= self.state.max_bytes / 2
                && size > self.state.max_bytes.saturating_sub(requested_class)
            {
                self.state.live.fetch_sub(1, Ordering::Relaxed);
                gpu_call(|| drop(buffer));
                continue;
            }
            return buffer;
        }
        drop(free);
        self.state.created.fetch_add(1, Ordering::Relaxed);
        self.state.live.fetch_add(1, Ordering::Relaxed);
        let descriptor = wgpu::BufferDescriptor {
            label: Some("Renderer Upload Staging"),
            size: requested_class,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        };
        gpu_call(|| device.create_buffer(&descriptor))
    }

    /// wgpu fires this callback after the submission reading the buffer completes.
    /// The free vector is preallocated, including callbacks' return storage.
    pub(crate) fn recycle(&self, buffer: wgpu::Buffer) {
        #[cfg(test)]
        {
            let (_, allocs) =
                super::allocation_tests::measure_storage(|| self.recycle_inner(buffer));
            self.state
                .allocation_proof
                .recycle_windows
                .fetch_add(1, Ordering::Relaxed);
            self.state
                .allocation_proof
                .recycle_allocs
                .fetch_add(allocs as u64, Ordering::Relaxed);
        }
        #[cfg(not(test))]
        self.recycle_inner(buffer);
    }

    #[inline]
    fn recycle_inner(&self, buffer: wgpu::Buffer) {
        let state = Arc::clone(&self.state);
        let remapped = gpu_call(|| buffer.clone());
        gpu_call(|| {
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |result| {
                    #[cfg(test)]
                    {
                        let (_, allocs) = super::allocation_tests::measure_storage(|| {
                            return_mapped(&state, remapped, result)
                        });
                        state
                            .allocation_proof
                            .callback_windows
                            .fetch_add(1, Ordering::Relaxed);
                        state
                            .allocation_proof
                            .callback_allocs
                            .fetch_add(allocs as u64, Ordering::Relaxed);
                    }
                    #[cfg(not(test))]
                    return_mapped(&state, remapped, result);
                })
        });
    }

    #[cfg(test)]
    pub(super) fn allocation_counts(&self) -> PoolAllocationCounts {
        let proof = &self.state.allocation_proof;
        PoolAllocationCounts {
            acquire_windows: proof.acquire_windows.load(Ordering::Relaxed),
            acquire_allocs: proof.acquire_allocs.load(Ordering::Relaxed),
            recycle_windows: proof.recycle_windows.load(Ordering::Relaxed),
            recycle_allocs: proof.recycle_allocs.load(Ordering::Relaxed),
            callback_windows: proof.callback_windows.load(Ordering::Relaxed),
            callback_allocs: proof.callback_allocs.load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(super) fn free_storage(&self) -> (usize, usize, u64) {
        let free = self
            .state
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (free.buffers.len(), free.buffers.capacity(), free.bytes)
    }

    pub(crate) fn counts(&self) -> (u64, u64, u64) {
        (
            self.state.acquired.load(Ordering::Relaxed),
            self.state.created.load(Ordering::Relaxed),
            self.state.live.load(Ordering::Relaxed),
        )
    }
}

#[inline]
fn return_mapped(
    state: &PoolState,
    remapped: wgpu::Buffer,
    result: Result<(), wgpu::BufferAsyncError>,
) {
    let mut free = state.free.lock().unwrap_or_else(PoisonError::into_inner);
    let size = gpu_call(|| remapped.size());
    if result.is_ok() && size <= state.max_bytes {
        // Small retained classes must not permanently exclude a larger steady workload.
        // Keep equal/larger classes: they can already serve this returning buffer's batch.
        while free.buffers.len() >= state.max_buffers
            || size > state.max_bytes.saturating_sub(free.bytes)
        {
            let smaller = free
                .buffers
                .iter()
                .enumerate()
                .filter(|(_, buffer)| gpu_call(|| buffer.size()) < size)
                .min_by_key(|(_, buffer)| gpu_call(|| buffer.size()))
                .map(|(index, _)| index);
            let Some(index) = smaller else {
                break;
            };
            let evicted = free.buffers.swap_remove(index);
            free.bytes -= gpu_call(|| evicted.size());
            state.live.fetch_sub(1, Ordering::Relaxed);
            gpu_call(|| drop(evicted));
        }
    }
    if result.is_ok()
        && free.buffers.len() < state.max_buffers
        && size <= state.max_bytes.saturating_sub(free.bytes)
    {
        free.bytes += size;
        free.buffers.push(remapped);
    } else {
        state.live.fetch_sub(1, Ordering::Relaxed);
        gpu_call(|| drop(remapped));
    }
}

#[cfg(test)]
fn staging_buffer_size(len: u64) -> u64 {
    staging_buffer_size_with_minimum(len, MIN_STAGING_BYTES)
}
fn staging_buffer_size_with_minimum(len: u64, minimum: u64) -> u64 {
    if len <= LARGE_STAGING_STEP_BYTES {
        len.max(minimum).next_power_of_two()
    } else {
        len.next_multiple_of(LARGE_STAGING_STEP_BYTES)
    }
}

#[cfg(test)]
fn best_fit(sizes: &[u64], len: u64) -> Option<usize> {
    sizes
        .iter()
        .enumerate()
        .filter(|&(_, &size)| size >= len)
        .min_by_key(|&(_, &size)| size)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffers_round_to_reusable_size_classes() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(staging_buffer_size(4), MIB);
        assert_eq!(staging_buffer_size(MIB), MIB);
        assert_eq!(staging_buffer_size(MIB + 1), 2 * MIB);
        assert_eq!(staging_buffer_size(3 * MIB), 4 * MIB);
        // An 18 MB cluster batch takes 20 MiB, not the next power of two.
        assert_eq!(staging_buffer_size(18_000_000), 20 * MIB);
    }

    #[test]
    fn a_small_batch_takes_the_smallest_buffer_that_fits() {
        // Regression: first fit let a promotion batch occupy the large buffer
        // an oversized install needed, forcing a fresh zero-filled allocation.
        let sizes = [32 << 20, 1 << 20, 8 << 20];
        assert_eq!(best_fit(&sizes, 200_000), Some(1));
        assert_eq!(best_fit(&sizes, 5 << 20), Some(2));
        assert_eq!(best_fit(&sizes, 20 << 20), Some(0));
        assert_eq!(best_fit(&sizes, 40 << 20), None);
    }
}
