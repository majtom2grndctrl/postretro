//! Streamed SH pool uploads and compose dispatch.
//! See: context/lib/rendering_pipeline.md §4; context/lib/resource_management.md §8.

use super::*;

// A queue.write_texture call creates a Metal staging buffer. Keep each write
// bounded, but combine adjacent canonical tiles so first-frame installation
// does not make one driver allocation per 8x8 tile.
const MAX_TILES_PER_UPLOAD: u32 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IsolatedUploadSpan {
    local_slot: u32,
    live_slot: u32,
    tiles: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct MomentUploadSpan {
    origin: wgpu::Origin3d,
    bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct SampledWordUploadSpan {
    dense_start: u32,
    bytes: Vec<u8>,
}

fn pack_sampled_word_upload_spans(
    words: &[u32],
    updates: &[(u32, u16, u16)],
) -> Result<Vec<SampledWordUploadSpan>, ShResidencyDrainError> {
    // The sampled mirror only changes at promoted or invalidated probes. Keep
    // the write volume proportional to those probes, not the full SH grid.
    let changed: std::collections::BTreeSet<_> =
        updates.iter().map(|&(dense, _, _)| dense).collect();
    let mut spans: Vec<SampledWordUploadSpan> = Vec::new();
    for dense in changed {
        let word = *words
            .get(dense as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if let Some(span) = spans.last_mut()
            && u32::try_from(span.bytes.len() / std::mem::size_of::<u32>())
                .ok()
                .and_then(|words| span.dense_start.checked_add(words))
                == Some(dense)
        {
            span.bytes.extend_from_slice(&word.to_le_bytes());
        } else {
            spans.push(SampledWordUploadSpan {
                dense_start: dense,
                bytes: word.to_le_bytes().to_vec(),
            });
        }
    }
    Ok(spans)
}

fn pack_moment_upload_spans(
    words: &[u32],
    updates: &[(u32, u16, u16)],
    grid: [u32; 3],
) -> Result<Vec<MomentUploadSpan>, ShResidencyDrainError> {
    let xy = grid[0]
        .checked_mul(grid[1])
        .filter(|&value| value != 0)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let total = xy
        .checked_mul(grid[2])
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    // The last update wins exactly as it did with ordered per-probe queue writes.
    let mut latest = std::collections::BTreeMap::new();
    for &(dense, mean, mean_sq) in updates {
        if dense >= total || dense as usize >= words.len() {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        latest.insert(dense, (mean, mean_sq));
    }
    let mut spans: Vec<MomentUploadSpan> = Vec::new();
    let mut previous_dense: Option<u32> = None;
    for (dense, (mean, mean_sq)) in latest {
        let x = dense % grid[0];
        let y = (dense / grid[0]) % grid[1];
        let z = dense / xy;
        let word = words[dense as usize];
        let contiguous = previous_dense
            .is_some_and(|previous| previous.checked_add(1) == Some(dense))
            && x != 0
            && spans
                .last()
                .is_some_and(|span| span.bytes.len() < MAX_TILES_PER_UPLOAD as usize * 8);
        if !contiguous {
            spans.push(MomentUploadSpan {
                origin: wgpu::Origin3d { x, y, z },
                bytes: Vec::new(),
            });
        }
        let span = spans.last_mut().expect("the first probe starts a span");
        for value in [mean, mean_sq, word as u16, (word >> 16) as u16] {
            span.bytes.extend_from_slice(&value.to_le_bytes());
        }
        previous_dense = Some(dense);
    }
    Ok(spans)
}

fn isolated_upload_spans(
    local_to_live: &std::collections::BTreeMap<u32, u32>,
    local_tiles_per_row: u32,
    live_tiles_per_row: u32,
) -> Vec<IsolatedUploadSpan> {
    let mut spans: Vec<IsolatedUploadSpan> = Vec::new();
    for (&local_slot, &live_slot) in local_to_live {
        if let Some(last) = spans.last_mut()
            && last.tiles < MAX_TILES_PER_UPLOAD
            && last.local_slot + last.tiles == local_slot
            && last.live_slot + last.tiles == live_slot
            && last.local_slot / local_tiles_per_row == local_slot / local_tiles_per_row
            && last.live_slot / live_tiles_per_row == live_slot / live_tiles_per_row
        {
            last.tiles += 1;
        } else {
            spans.push(IsolatedUploadSpan {
                local_slot,
                live_slot,
                tiles: 1,
            });
        }
    }
    spans
}

fn pack_isolated_upload_span(
    payload: &[u8],
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    span: IsolatedUploadSpan,
    scratch: &mut Vec<u8>,
) -> Result<u32, ShResidencyDrainError> {
    let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
    let local_tiles_per_layer = local_tiles_per_row
        .checked_mul(height / PHYSICAL_TILE_DIMENSION)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let local_layer = span.local_slot / local_tiles_per_layer;
    let local_in_layer = span.local_slot % local_tiles_per_layer;
    let local_x = (local_in_layer % local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
    let local_y = (local_in_layer / local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
    let (source_row_bytes, tile_row_bytes, source_rows, tile_rows, source_x, source_y) =
        match format {
            wgpu::TextureFormat::Bc6hRgbUfloat => (
                width / 4 * 16,
                32,
                height / 4,
                2,
                local_x / 4 * 16,
                local_y / 4,
            ),
            wgpu::TextureFormat::Rgba16Float => (width * 8, 64, height, 8, local_x * 8, local_y),
            _ => unreachable!("streamed SH base formats are validated"),
        };
    let packed_row_bytes = tile_row_bytes * span.tiles;
    let packed_len = usize::try_from(packed_row_bytes)
        .ok()
        .and_then(|row| row.checked_mul(tile_rows as usize))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    scratch.resize(packed_len, 0);
    for row in 0..tile_rows {
        let source = u64::from(local_layer)
            .checked_mul(u64::from(source_rows))
            .and_then(|layer_rows| layer_rows.checked_add(u64::from(source_y + row)))
            .and_then(|rows| rows.checked_mul(u64::from(source_row_bytes)))
            .and_then(|bytes| bytes.checked_add(u64::from(source_x)))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let source = usize::try_from(source).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let target = row as usize * packed_row_bytes as usize;
        let source_end = source
            .checked_add(packed_row_bytes as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        scratch[target..target + packed_row_bytes as usize].copy_from_slice(
            payload
                .get(source..source_end)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "isolated atlas tile span is truncated",
                })?,
        );
    }
    Ok(packed_row_bytes)
}

impl StreamingGpuPools {
    /// Upload only canonical nodes from an id-50 isolated atlas block. No
    /// decoded cluster body survives this call.
    pub(in crate::render::sh_streaming) fn validate_isolated_tiles(
        &self,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &std::collections::BTreeMap<u32, u32>,
    ) -> Result<(), ShResidencyDrainError> {
        if block.len() < 20 {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas upload has no header",
            });
        }
        let format = read_u32(block, 0)?;
        let slots = read_u32(block, 4)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        let layers = read_u32(block, 16)?;
        if texture_format(format)? != expected_format
            || width == 0
            || height == 0
            || layers == 0
            || width % PHYSICAL_TILE_DIMENSION != 0
            || height % PHYSICAL_TILE_DIMENSION != 0
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas upload shape or format is invalid",
            });
        }
        let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let local_tiles_per_layer = local_tiles_per_row
            .checked_mul(height / PHYSICAL_TILE_DIMENSION)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if slots
            > local_tiles_per_layer
                .checked_mul(layers)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas slot count exceeds its layout",
            });
        }
        let payload_bytes = match expected_format {
            wgpu::TextureFormat::Bc6hRgbUfloat => width
                .checked_div(4)
                .and_then(|blocks| blocks.checked_mul(height / 4))
                .and_then(|blocks| blocks.checked_mul(layers))
                .and_then(|blocks| blocks.checked_mul(16))
                .map(u64::from),
            wgpu::TextureFormat::Rgba16Float => width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(layers))
                .and_then(|pixels| pixels.checked_mul(8))
                .map(u64::from),
            _ => None,
        }
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let actual_payload = u64::try_from(block.len().saturating_sub(20))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if actual_payload < payload_bytes {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "isolated atlas payload is truncated",
            });
        }
        if local_to_live_slot
            .iter()
            .any(|(&local, &live)| local >= slots || live >= self.shape.slots)
        {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn upload_isolated_tiles(
        &self,
        queue: &wgpu::Queue,
        destination: &wgpu::Texture,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &std::collections::BTreeMap<u32, u32>,
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_isolated_tiles(expected_format, block, local_to_live_slot)?;
        let format = read_u32(block, 0)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        debug_assert_eq!(texture_format(format)?, expected_format);
        debug_assert!(width > 0 && height > 0);
        debug_assert_eq!(width % PHYSICAL_TILE_DIMENSION, 0);
        debug_assert_eq!(height % PHYSICAL_TILE_DIMENSION, 0);
        let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let payload = &block[20..];
        let mut scratch = Vec::new();
        for span in isolated_upload_spans(
            local_to_live_slot,
            local_tiles_per_row,
            self.shape.tiles_per_row,
        ) {
            let target_layer = span.live_slot / self.shape.tiles_per_layer;
            let target_in_layer = span.live_slot % self.shape.tiles_per_layer;
            let target_x = (target_in_layer % self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_y = (target_in_layer / self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let bytes_per_row = pack_isolated_upload_span(
                payload,
                expected_format,
                width,
                height,
                span,
                &mut scratch,
            )?;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: destination,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: target_x,
                        y: target_y,
                        z: target_layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &scratch,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(
                        if expected_format == wgpu::TextureFormat::Bc6hRgbUfloat {
                            2
                        } else {
                            8
                        },
                    ),
                },
                wgpu::Extent3d {
                    width: PHYSICAL_TILE_DIMENSION * span.tiles,
                    height: PHYSICAL_TILE_DIMENSION,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn upload_compose_words(
        &self,
        queue: &wgpu::Queue,
        words: &[u32],
    ) {
        let bytes = u32_bytes(words);
        queue.write_buffer(&self.compose_indirection, 0, &bytes);
    }

    pub(in crate::render::sh_streaming) fn dispatch_indirect_compose<'a>(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        dirty_ranges: &[(u32, u32)],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.dispatch(
            queue,
            encoder,
            uniform_bind_group,
            dirty_ranges,
            timestamp_writes,
        )
    }

    pub(in crate::render::sh_streaming) fn indirect_has_active_animation(
        &self,
        animation: &crate::render::sh_volume::AnimatedLightBuffers,
    ) -> bool {
        self.indirect_compose.has_active_animation(animation)
    }

    pub(in crate::render::sh_streaming) fn sparse_entry_capacity(&self, section_id: u32) -> u32 {
        match section_id {
            27 => self.indirect_compose.entry_capacity(),
            DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION => self
                .direct_compose
                .as_ref()
                .and_then(|direct| direct.entry_capacity(section_id))
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub(in crate::render::sh_streaming) fn sparse_tile_f16_capacity(&self, section_id: u32) -> u32 {
        match section_id {
            27 => self.indirect_compose.tile_f16_capacity(),
            DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION => self
                .direct_compose
                .as_ref()
                .and_then(|direct| direct.tile_f16_capacity(section_id))
                .unwrap_or(0),
            _ => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming) fn dispatch_direct_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        light_term_mask: postretro_render_cpu::frame_uniforms::LightTermMask,
        promotion_override: DirectShDebugOverride,
        animated_override: AnimatedDirectShDebugOverride,
        promoted_animated_states: &[PromotedBakedLightState],
        promotion_ranges: &[(u32, u32)],
        animated_ranges: &[(u32, u32)],
        force_full_resident: bool,
        promotion_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
        animated_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_mut()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct compose dispatch requested without a compose pool",
            })?
            .dispatch(
                queue,
                encoder,
                uniform_bind_group,
                StreamingDirectComposeFrameInputs {
                    light_term_mask,
                    dirty: StreamingDirectDirtyRanges {
                        promotion: promotion_ranges,
                        animated: animated_ranges,
                        force_full_resident,
                    },
                    promotion_override,
                    animated_override,
                    promoted_animated_states,
                    promotion_timestamp_writes,
                    animated_timestamp_writes,
                },
            )
    }

    pub(in crate::render::sh_streaming) fn upload_indirect_sparse_rows(
        &mut self,
        queue: &wgpu::Queue,
        rows: &[&super::super::SparseInstallPlan],
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.upload_sparse_rows(queue, rows)
    }

    pub(in crate::render::sh_streaming) fn validate_indirect_sparse_row(
        &self,
        entry_start: u32,
        tile_f16_start: u32,
        row: &super::super::ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose
            .validate_sparse_row(entry_start, tile_f16_start, row)
    }

    pub(in crate::render::sh_streaming) fn upload_direct_sparse_rows(
        &mut self,
        queue: &wgpu::Queue,
        section_id: u32,
        rows: &[super::super::direct_compose::DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse upload requested without a direct compose pool",
            })?
            .upload_sparse_rows(queue, section_id, rows)
    }

    pub(in crate::render::sh_streaming) fn validate_direct_sparse_rows(
        &self,
        section_id: u32,
        rows: &[super::super::direct_compose::DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse validation requested without a direct compose pool",
            })?
            .validate_sparse_rows(section_id, rows)
    }

    pub(in crate::render::sh_streaming) fn clear_indirect_sparse_row(
        &self,
        queue: &wgpu::Queue,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.clear_row_pair(queue, row)
    }

    pub(in crate::render::sh_streaming) fn clear_all_indirect_sparse_rows(
        &self,
        queue: &wgpu::Queue,
    ) {
        self.indirect_compose.clear_all_row_pairs(queue);
    }

    pub(in crate::render::sh_streaming) fn clear_direct_sparse_row(
        &self,
        queue: &wgpu::Queue,
        section_id: u32,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse clear requested without a direct compose pool",
            })?
            .clear_sparse_row_pair(queue, section_id, row)
    }

    pub(in crate::render::sh_streaming) fn clear_all_direct_sparse_rows(
        &self,
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(direct) = self.direct_compose.as_ref() else {
            return Ok(());
        };
        direct.clear_all_row_pairs(queue, DIRECT_DELTA_SECTION)?;
        if self.has_animated_direct_pass {
            direct.clear_all_row_pairs(queue, ANIMATED_DIRECT_DELTA_SECTION)?;
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn upload_sample_words_and_moments(
        &self,
        queue: &wgpu::Queue,
        words: &[u32],
        updates: &[(u32, u16, u16)],
        grid: [u32; 3],
    ) -> Result<(), ShResidencyDrainError> {
        let spans = pack_moment_upload_spans(words, updates, grid)?;
        for span in pack_sampled_word_upload_spans(words, updates)? {
            queue.write_buffer(
                &self.sampled_indirection,
                u64::from(span.dense_start) * std::mem::size_of::<u32>() as u64,
                &span.bytes,
            );
        }
        for span in spans {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.depth_moments,
                    mip_level: 0,
                    origin: span.origin,
                    aspect: wgpu::TextureAspect::All,
                },
                &span.bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(span.bytes.len() as u32),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: span.bytes.len() as u32 / 8,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn logical_bytes_per_dense_slot(
        &self,
    ) -> Result<u64, ShResidencyDrainError> {
        let extent = wgpu::Extent3d {
            width: PHYSICAL_TILE_DIMENSION,
            height: PHYSICAL_TILE_DIMENSION,
            depth_or_array_layers: 1,
        };
        // Compose outputs are active physical capacity, not a second logical
        // source payload. Keep this in lockstep with id-50 accounting.
        texture_bytes(self.base_format, extent)
            .checked_add(
                self.direct_format
                    .map_or(0, |format| texture_bytes(format, extent)),
            )
            .ok_or(ShResidencyDrainError::SlotOverflow)
    }
}

#[cfg(test)]
mod isolated_upload_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn adjacent_tiles_coalesce_without_crossing_source_or_destination_rows() {
        // Regression: one Metal staging allocation per 8x8 tile stalled the first level frame.
        let slots = BTreeMap::from([(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
        assert_eq!(
            isolated_upload_spans(&slots, 4, 3),
            [
                IsolatedUploadSpan {
                    local_slot: 0,
                    live_slot: 0,
                    tiles: 3
                },
                IsolatedUploadSpan {
                    local_slot: 3,
                    live_slot: 3,
                    tiles: 1
                },
                IsolatedUploadSpan {
                    local_slot: 4,
                    live_slot: 4,
                    tiles: 2
                },
            ]
        );

        let scattered = BTreeMap::from([(0, 0), (1, 1), (2, 5), (3, 6)]);
        assert_eq!(
            isolated_upload_spans(&scattered, 8, 8),
            [
                IsolatedUploadSpan {
                    local_slot: 0,
                    live_slot: 0,
                    tiles: 2
                },
                IsolatedUploadSpan {
                    local_slot: 2,
                    live_slot: 5,
                    tiles: 2
                },
            ]
        );
    }

    #[test]
    fn upload_spans_remain_bounded_even_on_wide_atlas_rows() {
        let slots = (0..130).map(|slot| (slot, slot)).collect();
        assert_eq!(
            isolated_upload_spans(&slots, 256, 256),
            [
                IsolatedUploadSpan {
                    local_slot: 0,
                    live_slot: 0,
                    tiles: 128
                },
                IsolatedUploadSpan {
                    local_slot: 128,
                    live_slot: 128,
                    tiles: 2
                },
            ]
        );
    }

    #[test]
    fn packed_span_preserves_bc6h_and_rgba16f_source_rows() {
        let span = IsolatedUploadSpan {
            local_slot: 1,
            live_slot: 7,
            tiles: 2,
        };
        let mut scratch = Vec::new();
        let bc6h: Vec<u8> = (0..192).map(|index| index as u8).collect();
        assert_eq!(
            pack_isolated_upload_span(
                &bc6h,
                wgpu::TextureFormat::Bc6hRgbUfloat,
                24,
                8,
                span,
                &mut scratch,
            )
            .unwrap(),
            64
        );
        assert_eq!(&scratch[..64], &bc6h[32..96]);
        assert_eq!(&scratch[64..], &bc6h[128..192]);

        let rgba: Vec<u8> = (0..1536).map(|index| index as u8).collect();
        assert_eq!(
            pack_isolated_upload_span(
                &rgba,
                wgpu::TextureFormat::Rgba16Float,
                24,
                8,
                span,
                &mut scratch,
            )
            .unwrap(),
            128
        );
        for row in 0..8 {
            let source = row * 192 + 64;
            assert_eq!(
                &scratch[row * 128..(row + 1) * 128],
                &rgba[source..source + 128]
            );
        }
    }

    #[test]
    fn depth_moment_updates_coalesce_rows_and_keep_last_duplicate() {
        // Regression: one eight-byte Metal write per promoted probe stalled the next frame.
        let words = [
            0x0002_0001,
            0x0004_0003,
            0,
            0x0006_0005,
            0x0008_0007,
            0,
            0,
            0,
        ];
        let spans = pack_moment_upload_spans(
            &words,
            &[
                (1, 10, 11),
                (0, 20, 21),
                (1, 30, 31),
                (3, 40, 41),
                (4, 50, 51),
            ],
            [4, 2, 1],
        )
        .unwrap();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].origin, wgpu::Origin3d { x: 0, y: 0, z: 0 });
        assert_eq!(spans[0].bytes.len(), 16);
        assert_eq!(
            &spans[0].bytes[8..16],
            &[30u16, 31, 3, 4]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
        assert_eq!(spans[1].origin, wgpu::Origin3d { x: 3, y: 0, z: 0 });
        assert_eq!(spans[2].origin, wgpu::Origin3d { x: 0, y: 1, z: 0 });
        assert_eq!(spans[2].bytes.len(), 8);
    }

    #[test]
    fn sampled_word_uploads_scale_with_changed_probes() {
        // Regression: promotion uploaded the complete sampled-word mirror for a few probes.
        let mut words = vec![0; 4_096];
        words[2] = 0x0002_0001;
        words[3] = 0x0004_0003;
        words[1_024] = 0x0006_0005;
        let spans = pack_sampled_word_upload_spans(
            &words,
            &[(1_024, 1, 2), (2, 3, 4), (3, 5, 6), (1_024, 7, 8)],
        )
        .unwrap();

        assert_eq!(
            spans,
            [
                SampledWordUploadSpan {
                    dense_start: 2,
                    bytes: u32_bytes(&[0x0002_0001, 0x0004_0003]),
                },
                SampledWordUploadSpan {
                    dense_start: 1_024,
                    bytes: u32_bytes(&[0x0006_0005]),
                },
            ]
        );
        assert_eq!(spans.iter().map(|span| span.bytes.len()).sum::<usize>(), 12);
        assert!(spans.iter().map(|span| span.bytes.len()).sum::<usize>() < words.len() * 4);
    }
}
