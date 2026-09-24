//! Recycled mapped staging buffers for residency upload batches.
//!
//! A freshly created mapped buffer costs a driver allocation plus zero-filling
//! every page, several milliseconds for a cluster-sized batch. The pool hands
//! each batch the smallest free buffer that fits and remaps it once the GPU
//! has consumed it, so steady-state uploads allocate nothing. Best fit keeps a
//! small promotion batch from occupying the large buffer the next oversized
//! install needs.

use std::sync::{Arc, Mutex, PoisonError};

/// Smallest staging buffer. Sizes are powers of two from here up to
/// [`LARGE_STAGING_STEP_BYTES`], then multiples of it, so a buffer is reusable
/// by later batches of similar size without zero-filling far more pages than
/// an oversized batch needs.
const MIN_STAGING_BYTES: u64 = 1024 * 1024;
const LARGE_STAGING_STEP_BYTES: u64 = 4 * 1024 * 1024;
/// Free staging memory kept for reuse. Buffers returned beyond this are
/// released instead of pooled.
const MAX_POOLED_STAGING_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub(in crate::render::sh_streaming) struct StagingPool {
    free: Vec<wgpu::Buffer>,
    /// Buffers whose remap completed; filled from wgpu's map callback.
    returned: Arc<Mutex<Vec<wgpu::Buffer>>>,
}

impl StagingPool {
    /// A mapped `MAP_WRITE | COPY_SRC` buffer of at least `len` bytes.
    pub(in crate::render::sh_streaming) fn acquire(
        &mut self,
        device: &wgpu::Device,
        len: u64,
    ) -> wgpu::Buffer {
        self.collect_returned();
        let sizes: Vec<u64> = self.free.iter().map(wgpu::Buffer::size).collect();
        if let Some(index) = best_fit(&sizes, len) {
            return self.free.swap_remove(index);
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Streamed SH Upload Staging"),
            size: staging_buffer_size(len),
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        })
    }

    /// Return an unmapped buffer after the submission that reads it. It
    /// rejoins the pool once its remap completes, which wgpu reports only
    /// after that submission has finished on the GPU.
    pub(in crate::render::sh_streaming) fn recycle(&self, buffer: wgpu::Buffer) {
        let returned = Arc::clone(&self.returned);
        let remapped = buffer.clone();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Write, move |result| {
                if result.is_ok() {
                    returned
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(remapped);
                }
            });
    }

    fn collect_returned(&mut self) {
        let returned: Vec<wgpu::Buffer> = self
            .returned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        let mut pooled: u64 = self.free.iter().map(wgpu::Buffer::size).sum();
        for buffer in returned {
            if pooled + buffer.size() <= MAX_POOLED_STAGING_BYTES {
                pooled += buffer.size();
                self.free.push(buffer);
            }
        }
    }
}

/// Size of a new staging buffer for a `len`-byte batch.
fn staging_buffer_size(len: u64) -> u64 {
    if len <= LARGE_STAGING_STEP_BYTES {
        len.max(MIN_STAGING_BYTES).next_power_of_two()
    } else {
        len.next_multiple_of(LARGE_STAGING_STEP_BYTES)
    }
}

/// Index of the smallest size that holds `len`.
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
