//! One shared staging allocation per frame or residency upload batch.
//!
//! `Queue::write_buffer` and `Queue::write_texture` allocate a driver staging
//! buffer per call, roughly 40 µs each on Metal. A real cluster install issues
//! thousands of small writes (tile spans, scattered probe words, sparse CSR
//! pairs), so per-call allocation, not bytes, dominated its CPU cost. A batch
//! packs every write into one mapped staging buffer and records ordered copies
//! into one command buffer, so upload cost scales with copied bytes and copy
//! count.
//!
//! Copies preserve write order, including overlapping ranges. `UploadQueue`
//! consumes pending frame writes before the first subsequent scene or drain
//! submission. Raw queue writes execute ahead of submitted copies, so a raw
//! write must never follow a pending staged write to the same resource; the
//! upload chokepoint asserts this and the source drift gate owns raw escapes.

use super::UploadError;
use super::staging_pool::StagingPool;

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

const TEXTURE_ROW_ALIGNMENT: usize = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
const BUFFER_ALIGNMENT: usize = wgpu::COPY_BUFFER_ALIGNMENT as usize;

enum StagedCopy {
    Buffer {
        target: wgpu::Buffer,
        source_offset: u64,
        target_offset: u64,
        size: u64,
    },
    Texture {
        target: wgpu::Texture,
        source_offset: u64,
        bytes_per_row: u32,
        rows_per_image: u32,
        origin: wgpu::Origin3d,
        extent: wgpu::Extent3d,
    },
}

/// Recorded residency writes awaiting one staging allocation and submission.
#[derive(Default)]
pub(crate) struct StagedUploads {
    bytes: Vec<u8>,
    copies: Vec<StagedCopy>,
    merge_contiguous: bool,
    #[cfg(test)]
    record_windows: u64,
    #[cfg(test)]
    record_allocs: u64,
}

impl StagedUploads {
    /// An empty batch that stages into `scratch` (its contents are
    /// discarded), with room for about `reserve` bytes. Reusing one scratch
    /// vector keeps large cluster uploads from reallocating and faulting in
    /// fresh pages every drain.
    pub(crate) fn from_scratch(mut scratch: Vec<u8>, reserve: usize) -> Self {
        scratch.clear();
        scratch.reserve(reserve);
        Self {
            bytes: scratch,
            copies: Vec::new(),
            merge_contiguous: true,
            #[cfg(test)]
            record_windows: 0,
            #[cfg(test)]
            record_allocs: 0,
        }
    }

    /// Stage `data` for `target[offset..]`. Offset and length follow the
    /// `Queue::write_buffer` rules (multiples of four bytes). In `from_scratch`
    /// residency batches, a write continuing the previous one in the same
    /// buffer extends its copy. Default reusable frame batches keep one copy
    /// per write.
    pub(crate) fn write_buffer(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), UploadError> {
        self.stage_buffer(target, offset, data.len(), |bytes| {
            bytes.extend_from_slice(data)
        })
    }

    /// Stage f16 halves packed two per little-endian word, the low half
    /// first, with an odd tail padded by zero. Writes the halves straight
    /// into staging; sparse tile payloads are most of a cluster's bytes.
    pub(crate) fn write_buffer_f16(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        halves: &[u16],
    ) -> Result<(), UploadError> {
        let len = halves.len().div_ceil(2) * BUFFER_ALIGNMENT;
        self.stage_buffer(target, offset, len, |bytes| append_f16_words(bytes, halves))
    }

    fn stage_buffer(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        len: usize,
        append: impl FnOnce(&mut Vec<u8>),
    ) -> Result<(), UploadError> {
        if len == 0 {
            return Ok(());
        }
        if !offset.is_multiple_of(BUFFER_ALIGNMENT as u64) || !len.is_multiple_of(BUFFER_ALIGNMENT)
        {
            return Err(UploadError::Alignment);
        }
        let size = len as u64;
        let end = offset
            .checked_add(size)
            .filter(|&end| end <= target.size())
            .ok_or(UploadError::Bounds)?;
        let source_offset = self.bytes.len() as u64;
        if self.merge_contiguous
            && let Some(StagedCopy::Buffer {
                target: last_target,
                source_offset: last_source,
                target_offset: last_offset,
                size: last_size,
            }) = self.copies.last_mut()
            && *last_target == *target
            && *last_source + *last_size == source_offset
            && *last_offset + *last_size == offset
        {
            *last_size = end - *last_offset;
        } else {
            self.copies.push(StagedCopy::Buffer {
                target: target.clone(),
                source_offset,
                target_offset: offset,
                size,
            });
        }
        append(&mut self.bytes);
        debug_assert_eq!(self.bytes.len() as u64, source_offset + size);
        Ok(())
    }

    /// Stage one texture region. `data` is tightly packed exactly as
    /// `Queue::write_texture` takes it: `rows_per_image` block rows of
    /// `bytes_per_row` bytes per depth slice.
    pub(crate) fn write_texture(
        &mut self,
        target: &wgpu::Texture,
        origin: wgpu::Origin3d,
        data: &[u8],
        bytes_per_row: u32,
        rows_per_image: u32,
        extent: wgpu::Extent3d,
    ) -> Result<(), UploadError> {
        let rows = usize::try_from(rows_per_image)
            .ok()
            .and_then(|rows| rows.checked_mul(extent.depth_or_array_layers as usize))
            .ok_or(UploadError::Bounds)?;
        let (source_offset, padded_row) =
            stage_texture_rows(&mut self.bytes, data, bytes_per_row as usize, rows)?;
        self.copies.push(StagedCopy::Texture {
            target: target.clone(),
            source_offset: source_offset as u64,
            bytes_per_row: u32::try_from(padded_row).map_err(|_| UploadError::Bounds)?,
            rows_per_image,
            origin,
            extent,
        });
        Ok(())
    }

    /// Copy every staged write through one recycled staging buffer and one
    /// command buffer, then hand back the scratch vector. An empty batch
    /// allocates and submits nothing.
    pub(crate) fn submit(
        self,
        pool: &mut StagingPool,
        device: &wgpu::Device,
        queue: &super::UploadQueue,
    ) -> Vec<u8> {
        if self.copies.is_empty() {
            return self.bytes;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Upload Copies"),
        });
        let recorded = self.record(pool, device, &mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        recorded.finish(pool)
    }

    /// Record every staged write into `encoder` after whatever it already
    /// holds, through one recycled staging buffer. The caller submits the
    /// encoder, then hands the result back with [`RecordedUploads::finish`].
    /// An empty batch acquires no staging buffer.
    pub(crate) fn record(
        mut self,
        pool: &mut StagingPool,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> RecordedUploads {
        let staging = self.record_reusing(pool, device, encoder);
        RecordedUploads {
            scratch: self.bytes,
            staging,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.copies.is_empty()
    }
    pub(crate) fn byte_len(&self) -> usize {
        self.bytes.len()
    }
    pub(crate) fn copy_count(&self) -> usize {
        self.copies.len()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn touches_buffer(&self, buffer: &wgpu::Buffer) -> bool {
        self.copies
            .iter()
            .any(|copy| matches!(copy, StagedCopy::Buffer { target, .. } if target == buffer))
    }

    /// Keep both CPU vectors allocated across frame submissions.
    pub(crate) fn record_reusing(
        &mut self,
        pool: &mut StagingPool,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<wgpu::Buffer> {
        #[cfg(test)]
        {
            let (result, allocs) = super::allocation_tests::measure_storage(|| {
                self.record_reusing_inner(pool, device, encoder)
            });
            self.record_windows += 1;
            self.record_allocs += allocs as u64;
            result
        }
        #[cfg(not(test))]
        {
            self.record_reusing_inner(pool, device, encoder)
        }
    }

    #[cfg(test)]
    pub(super) fn storage_capacities(&self) -> (usize, usize) {
        (self.bytes.capacity(), self.copies.capacity())
    }

    #[cfg(test)]
    pub(super) fn record_allocation_counts(&self) -> (u64, u64) {
        (self.record_windows, self.record_allocs)
    }

    #[inline]
    fn record_reusing_inner(
        &mut self,
        pool: &mut StagingPool,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<wgpu::Buffer> {
        if self.copies.is_empty() {
            return None;
        }
        self.bytes
            .resize(self.bytes.len().next_multiple_of(BUFFER_ALIGNMENT), 0);
        let len = self.bytes.len() as u64;
        let staging = pool.acquire(device, len);
        let mut mapped = gpu_call(|| {
            staging
                .get_mapped_range_mut(..len)
                .expect("the staging pool hands out mapped buffers of at least `len` bytes")
        });
        mapped.copy_from_slice(&self.bytes);
        gpu_call(|| drop(mapped));
        gpu_call(|| staging.unmap());
        for copy in &self.copies {
            match copy {
                StagedCopy::Buffer {
                    target,
                    source_offset,
                    target_offset,
                    size,
                } => gpu_call(|| {
                    encoder.copy_buffer_to_buffer(
                        &staging,
                        *source_offset,
                        target,
                        *target_offset,
                        *size,
                    )
                }),
                StagedCopy::Texture {
                    target,
                    source_offset,
                    bytes_per_row,
                    rows_per_image,
                    origin,
                    extent,
                } => gpu_call(|| {
                    encoder.copy_buffer_to_texture(
                        wgpu::TexelCopyBufferInfo {
                            buffer: &staging,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: *source_offset,
                                bytes_per_row: Some(*bytes_per_row),
                                rows_per_image: Some(*rows_per_image),
                            },
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: target,
                            mip_level: 0,
                            origin: *origin,
                            aspect: wgpu::TextureAspect::All,
                        },
                        *extent,
                    )
                }),
            }
        }
        self.bytes.clear();
        self.copies.clear();
        Some(staging)
    }
}

/// A batch recorded into a caller's encoder. Its staging buffer rejoins the
/// pool only after the submission that reads it.
#[must_use = "finish after submitting the encoder, or the staging buffer is lost"]
pub(crate) struct RecordedUploads {
    scratch: Vec<u8>,
    staging: Option<wgpu::Buffer>,
}

impl RecordedUploads {
    /// Recycle the staging buffer (call after `Queue::submit`) and hand back
    /// the scratch vector.
    pub(crate) fn finish(self, pool: &StagingPool) -> Vec<u8> {
        if let Some(staging) = self.staging {
            pool.recycle(staging);
        }
        self.scratch
    }
}

/// Append f16 halves packed two per little-endian word, the low half first;
/// an odd tail pads its word with zero.
pub(crate) fn append_f16_words(bytes: &mut Vec<u8>, halves: &[u16]) {
    if cfg!(target_endian = "little") {
        bytes.extend_from_slice(bytemuck::cast_slice(halves));
    } else {
        bytes.extend(halves.iter().flat_map(|half| half.to_le_bytes()));
    }
    if halves.len() % 2 == 1 {
        bytes.extend_from_slice(&[0, 0]);
    }
}

/// Append `rows` tightly packed rows of `row_bytes` to `staging`, each padded
/// to the buffer-to-texture row alignment and starting on an aligned offset.
/// Returns the region's start offset and padded row pitch.
fn stage_texture_rows(
    staging: &mut Vec<u8>,
    data: &[u8],
    row_bytes: usize,
    rows: usize,
) -> Result<(usize, usize), UploadError> {
    if row_bytes == 0 || rows == 0 || row_bytes.checked_mul(rows) != Some(data.len()) {
        return Err(UploadError::TexturePayload);
    }
    let padded_row = row_bytes.next_multiple_of(TEXTURE_ROW_ALIGNMENT);
    let start = staging.len().next_multiple_of(TEXTURE_ROW_ALIGNMENT);
    let end = padded_row
        .checked_mul(rows)
        .and_then(|bytes| bytes.checked_add(start))
        .ok_or(UploadError::Bounds)?;
    staging.resize(start, 0);
    staging.reserve(end - start);
    for row in data.chunks_exact(row_bytes) {
        staging.extend_from_slice(row);
        staging.resize(staging.len() + padded_row - row_bytes, 0);
    }
    Ok((start, padded_row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_halves_pack_low_half_first_and_pad_an_odd_tail() {
        let mut bytes = Vec::new();
        append_f16_words(&mut bytes, &[0x0201, 0x0403, 0x0605]);
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 0, 0]);
    }

    #[test]
    fn texture_rows_start_aligned_and_pad_each_row_to_the_copy_pitch() {
        let mut staging = vec![7u8; 12];
        let data: Vec<u8> = (0..64).collect();
        let (start, pitch) = stage_texture_rows(&mut staging, &data, 32, 2).unwrap();
        assert_eq!((start, pitch), (256, 256));
        assert_eq!(staging.len(), 256 + 512);
        assert!(staging[12..256].iter().all(|&byte| byte == 0));
        assert_eq!(&staging[256..288], &data[..32]);
        assert!(staging[288..512].iter().all(|&byte| byte == 0));
        assert_eq!(&staging[512..544], &data[32..]);
    }

    #[test]
    fn texture_rows_keep_an_already_aligned_pitch_tight() {
        let mut staging = Vec::new();
        let data = vec![1u8; 512 * 3];
        let (start, pitch) = stage_texture_rows(&mut staging, &data, 512, 3).unwrap();
        assert_eq!((start, pitch), (0, 512));
        assert_eq!(staging, data);
    }

    #[test]
    fn texture_rows_reject_a_payload_that_does_not_fill_its_rows() {
        let mut staging = Vec::new();
        assert!(stage_texture_rows(&mut staging, &[0; 63], 32, 2).is_err());
        assert!(stage_texture_rows(&mut staging, &[], 0, 0).is_err());
        assert!(staging.is_empty());
    }
}
