//! One staging allocation per residency upload batch.
//!
//! `Queue::write_buffer` and `Queue::write_texture` allocate a driver staging
//! buffer per call, roughly 40 µs each on Metal. A real cluster install issues
//! thousands of small writes (tile spans, scattered probe words, sparse CSR
//! pairs), so per-call allocation, not bytes, dominated its CPU cost. A batch
//! packs every write into one mapped staging buffer and records ordered copies
//! into one command buffer, so upload cost scales with copied bytes and copy
//! count.
//!
//! Ordering contract: a batch is submitted before any later queue write or
//! submission touches the same resources. `Queue::submit` flushes earlier
//! `write_*` calls ahead of the batch, so program order is preserved as long
//! as no `write_*` to a batched resource happens between recording a batch
//! and submitting it.

use super::ShResidencyDrainError;
use super::staging_pool::StagingPool;

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
pub(in crate::render::sh_streaming) struct StagedUploads {
    bytes: Vec<u8>,
    copies: Vec<StagedCopy>,
}

impl StagedUploads {
    /// An empty batch that stages into `scratch` (its contents are
    /// discarded), with room for about `reserve` bytes. Reusing one scratch
    /// vector keeps large cluster uploads from reallocating and faulting in
    /// fresh pages every drain.
    pub(in crate::render::sh_streaming) fn from_scratch(
        mut scratch: Vec<u8>,
        reserve: usize,
    ) -> Self {
        scratch.clear();
        scratch.reserve(reserve);
        Self {
            bytes: scratch,
            copies: Vec::new(),
        }
    }

    /// Stage `data` for `target[offset..]`. Offset and length follow the
    /// `Queue::write_buffer` rules (multiples of four bytes). A write that
    /// continues the previous one in the same buffer extends its copy.
    pub(in crate::render::sh_streaming) fn write_buffer(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ShResidencyDrainError> {
        self.stage_buffer(target, offset, data.len(), |bytes| {
            bytes.extend_from_slice(data)
        })
    }

    /// Stage f16 halves packed two per little-endian word, the low half
    /// first, with an odd tail padded by zero. Writes the halves straight
    /// into staging; sparse tile payloads are most of a cluster's bytes.
    pub(in crate::render::sh_streaming) fn write_buffer_f16(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        halves: &[u16],
    ) -> Result<(), ShResidencyDrainError> {
        let len = halves.len().div_ceil(2) * BUFFER_ALIGNMENT;
        self.stage_buffer(target, offset, len, |bytes| append_f16_words(bytes, halves))
    }

    fn stage_buffer(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        len: usize,
        append: impl FnOnce(&mut Vec<u8>),
    ) -> Result<(), ShResidencyDrainError> {
        if len == 0 {
            return Ok(());
        }
        if offset % BUFFER_ALIGNMENT as u64 != 0 || len % BUFFER_ALIGNMENT != 0 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed buffer upload is not word aligned",
            });
        }
        let size = len as u64;
        let end = offset
            .checked_add(size)
            .filter(|&end| end <= target.size())
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let source_offset = self.bytes.len() as u64;
        if let Some(StagedCopy::Buffer {
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
    pub(in crate::render::sh_streaming) fn write_texture(
        &mut self,
        target: &wgpu::Texture,
        origin: wgpu::Origin3d,
        data: &[u8],
        bytes_per_row: u32,
        rows_per_image: u32,
        extent: wgpu::Extent3d,
    ) -> Result<(), ShResidencyDrainError> {
        let rows = usize::try_from(rows_per_image)
            .ok()
            .and_then(|rows| rows.checked_mul(extent.depth_or_array_layers as usize))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let (source_offset, padded_row) =
            stage_texture_rows(&mut self.bytes, data, bytes_per_row as usize, rows)?;
        self.copies.push(StagedCopy::Texture {
            target: target.clone(),
            source_offset: source_offset as u64,
            bytes_per_row: u32::try_from(padded_row)
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?,
            rows_per_image,
            origin,
            extent,
        });
        Ok(())
    }

    /// Copy every staged write through one recycled staging buffer and one
    /// command buffer, then hand back the scratch vector. An empty batch
    /// allocates and submits nothing.
    pub(in crate::render::sh_streaming) fn submit(
        mut self,
        pool: &mut StagingPool,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<u8> {
        if self.copies.is_empty() {
            return self.bytes;
        }
        self.bytes
            .resize(self.bytes.len().next_multiple_of(BUFFER_ALIGNMENT), 0);
        let len = self.bytes.len() as u64;
        let staging = pool.acquire(device, len);
        staging
            .get_mapped_range_mut(..len)
            .copy_from_slice(&self.bytes);
        staging.unmap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Upload Copies"),
        });
        for copy in &self.copies {
            match copy {
                StagedCopy::Buffer {
                    target,
                    source_offset,
                    target_offset,
                    size,
                } => encoder.copy_buffer_to_buffer(
                    &staging,
                    *source_offset,
                    target,
                    *target_offset,
                    *size,
                ),
                StagedCopy::Texture {
                    target,
                    source_offset,
                    bytes_per_row,
                    rows_per_image,
                    origin,
                    extent,
                } => encoder.copy_buffer_to_texture(
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
                ),
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
        pool.recycle(staging);
        self.bytes
    }
}

/// Append f16 halves packed two per little-endian word, the low half first;
/// an odd tail pads its word with zero.
pub(in crate::render::sh_streaming) fn append_f16_words(bytes: &mut Vec<u8>, halves: &[u16]) {
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
) -> Result<(usize, usize), ShResidencyDrainError> {
    if row_bytes == 0 || rows == 0 || row_bytes.checked_mul(rows) != Some(data.len()) {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "streamed texture upload rows disagree with their payload",
        });
    }
    let padded_row = row_bytes.next_multiple_of(TEXTURE_ROW_ALIGNMENT);
    let start = staging.len().next_multiple_of(TEXTURE_ROW_ALIGNMENT);
    let end = padded_row
        .checked_mul(rows)
        .and_then(|bytes| bytes.checked_add(start))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
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
