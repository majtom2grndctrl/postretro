//! Streamed SH pool uploads and compose dispatch.
//! See: context/lib/rendering_pipeline.md §4; context/lib/resource_management.md §8.

use super::*;

/// Largest upload scratch vector kept between batches. Budgeted drains stage
/// well under this; an oversized cluster's scratch is released after use.
const MAX_RETAINED_UPLOAD_SCRATCH_BYTES: usize = 16 * 1024 * 1024;

/// One copy region of the depth-moment volume: `rows` texel rows of equal
/// width stacked in y, tightly packed.
#[derive(Debug, PartialEq, Eq)]
struct MomentUploadSpan {
    origin: wgpu::Origin3d,
    rows: u32,
    bytes: Vec<u8>,
}

impl MomentUploadSpan {
    fn row_bytes(&self) -> usize {
        self.bytes.len() / self.rows as usize
    }
}

#[derive(Debug, PartialEq, Eq)]
struct WordUploadSpan {
    dense_start: u32,
    bytes: Vec<u8>,
}

/// Longest run of unchanged words a span re-uploads to join its neighbors.
/// Each word mirror equals its GPU buffer outside the changed set (every
/// mutation is uploaded in the drain that makes it, and a rolled-back install
/// uploads nothing), so a filled gap rewrites identical values. One copy
/// command costs far more than a kilobyte of staging.
const WORD_SPAN_MAX_GAP: u32 = 256;

fn pack_word_upload_spans(
    words: &[u32],
    changed: impl IntoIterator<Item = u32>,
) -> Result<Vec<WordUploadSpan>, ShResidencyDrainError> {
    let mut changed: Vec<u32> = changed.into_iter().collect();
    changed.sort_unstable();
    changed.dedup();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for dense in changed {
        if dense as usize >= words.len() {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        match ranges.last_mut() {
            Some((_, end)) if dense - *end <= WORD_SPAN_MAX_GAP => *end = dense + 1,
            _ => ranges.push((dense, dense + 1)),
        }
    }
    Ok(ranges
        .into_iter()
        .map(|(start, end)| WordUploadSpan {
            dense_start: start,
            bytes: u32_bytes(&words[start as usize..end as usize]),
        })
        .collect())
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
    // The last update wins exactly as it did with ordered per-probe queue
    // writes. A stable sort keeps duplicates in update order.
    for &(dense, _, _) in updates {
        if dense >= total || dense as usize >= words.len() {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
    }
    let mut latest = updates.to_vec();
    latest.sort_by_key(|&(dense, _, _)| dense);
    latest.dedup_by(|later, earlier| {
        if later.0 == earlier.0 {
            *earlier = *later;
            true
        } else {
            false
        }
    });
    // Texel rows first: one per run of consecutive probes along x.
    let mut row_spans: Vec<MomentUploadSpan> = Vec::new();
    let mut previous_dense: Option<u32> = None;
    for (dense, mean, mean_sq) in latest {
        let x = dense % grid[0];
        let y = (dense / grid[0]) % grid[1];
        let z = dense / xy;
        let word = words[dense as usize];
        let contiguous =
            previous_dense.is_some_and(|previous| previous.checked_add(1) == Some(dense)) && x != 0;
        if !contiguous {
            row_spans.push(MomentUploadSpan {
                origin: wgpu::Origin3d { x, y, z },
                rows: 1,
                bytes: Vec::new(),
            });
        }
        let span = row_spans.last_mut().expect("the first probe starts a span");
        for value in [mean, mean_sq, word as u16, (word >> 16) as u16] {
            span.bytes.extend_from_slice(&value.to_le_bytes());
        }
        previous_dense = Some(dense);
    }
    // Then stack rows of equal x extent on consecutive y into one region; a
    // cluster's box of probes becomes about one copy per z slice.
    let mut spans: Vec<MomentUploadSpan> = Vec::new();
    let mut open = std::collections::HashMap::<(u32, u32, usize), usize>::new();
    for row in row_spans {
        let key = (row.origin.z, row.origin.x, row.bytes.len());
        if let Some(&index) = open.get(&key) {
            let region = &mut spans[index];
            if region.origin.y + region.rows == row.origin.y {
                region.rows += 1;
                region.bytes.extend_from_slice(&row.bytes);
                continue;
            }
        }
        open.insert(key, spans.len());
        spans.push(row);
    }
    Ok(spans)
}

impl StreamingGpuPools {
    /// Start an upload batch on the pools' reusable scratch vector.
    pub(in crate::render::sh_streaming) fn begin_uploads(
        &mut self,
        reserve: usize,
    ) -> StagedUploads {
        StagedUploads::from_scratch(std::mem::take(&mut self.upload_scratch), reserve)
    }

    /// Submit one recorded upload batch through the pools' staging buffers and
    /// keep its scratch vector for the next batch, unless it grew past the
    /// retention cap.
    pub(in crate::render::sh_streaming) fn submit_uploads(
        &mut self,
        uploads: StagedUploads,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let scratch = uploads.submit(&mut self.staging, device, queue);
        if scratch.capacity() <= MAX_RETAINED_UPLOAD_SCRATCH_BYTES {
            self.upload_scratch = scratch;
        }
    }

    /// Upload only canonical nodes from an id-50 isolated atlas block. No
    /// decoded cluster body survives this call.
    pub(in crate::render::sh_streaming) fn validate_isolated_tiles(
        &self,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &[SlotRun],
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
        if local_to_live_slot.iter().any(|run| {
            run.local.checked_add(run.len).is_none_or(|end| end > slots)
                || run
                    .live
                    .checked_add(run.len)
                    .is_none_or(|end| end > self.shape.slots)
        }) {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn upload_isolated_tiles(
        &self,
        uploads: &mut StagedUploads,
        destination: &wgpu::Texture,
        expected_format: wgpu::TextureFormat,
        block: &[u8],
        local_to_live_slot: &[SlotRun],
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_isolated_tiles(expected_format, block, local_to_live_slot)?;
        let format = read_u32(block, 0)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        debug_assert_eq!(texture_format(format)?, expected_format);
        debug_assert!(width > 0 && height > 0);
        debug_assert_eq!(width % PHYSICAL_TILE_DIMENSION, 0);
        debug_assert_eq!(height % PHYSICAL_TILE_DIMENSION, 0);
        let payload = &block[20..];
        let plan = plan_isolated_uploads(local_to_live_slot, self.shape.tiles_per_row);
        let mut scratch = Vec::new();
        for &span in &plan.spans {
            let target_layer = span.live_slot / self.shape.tiles_per_layer;
            let target_in_layer = span.live_slot % self.shape.tiles_per_layer;
            let target_x = (target_in_layer % self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_y = (target_in_layer / self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let bytes_per_row = pack_isolated_upload_span(
                payload,
                expected_format,
                width,
                height,
                &plan,
                span,
                &mut scratch,
            )?;
            uploads.write_texture(
                destination,
                wgpu::Origin3d {
                    x: target_x,
                    y: target_y,
                    z: target_layer,
                },
                &scratch,
                bytes_per_row,
                if expected_format == wgpu::TextureFormat::Bc6hRgbUfloat {
                    2
                } else {
                    8
                },
                wgpu::Extent3d {
                    width: PHYSICAL_TILE_DIMENSION * span.tiles,
                    height: PHYSICAL_TILE_DIMENSION,
                    depth_or_array_layers: 1,
                },
            )?;
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

    pub(in crate::render::sh_streaming) fn upload_changed_compose_words(
        &self,
        uploads: &mut StagedUploads,
        words: &[u32],
        changed: impl IntoIterator<Item = u32>,
    ) -> Result<(), ShResidencyDrainError> {
        for span in pack_word_upload_spans(words, changed)? {
            uploads.write_buffer(
                &self.compose_indirection,
                u64::from(span.dense_start) * std::mem::size_of::<u32>() as u64,
                &span.bytes,
            )?;
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn dispatch_indirect_compose<'a>(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        rows: &[u32],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose
            .dispatch(queue, encoder, uniform_bind_group, rows, timestamp_writes)
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
        promotion_rows: &[u32],
        animated_rows: &[u32],
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
                    dirty: StreamingDirectDirtyRows {
                        promotion: promotion_rows,
                        animated: animated_rows,
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
        uploads: &mut StagedUploads,
        rows: &[&super::super::SparseInstallPlan],
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.upload_sparse_rows(uploads, rows)
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
        uploads: &mut StagedUploads,
        section_id: u32,
        rows: &[super::super::direct_compose::DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse upload requested without a direct compose pool",
            })?
            .upload_sparse_rows(uploads, section_id, rows)
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
        uploads: &mut StagedUploads,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose.clear_row_pair(uploads, row)
    }

    pub(in crate::render::sh_streaming) fn clear_all_indirect_sparse_rows(
        &self,
        queue: &wgpu::Queue,
    ) {
        self.indirect_compose.clear_all_row_pairs(queue);
    }

    pub(in crate::render::sh_streaming) fn clear_direct_sparse_row(
        &self,
        uploads: &mut StagedUploads,
        section_id: u32,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.direct_compose
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse clear requested without a direct compose pool",
            })?
            .clear_sparse_row_pair(uploads, section_id, row)
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
        uploads: &mut StagedUploads,
        words: &[u32],
        updates: &[(u32, u16, u16)],
        grid: [u32; 3],
    ) -> Result<(), ShResidencyDrainError> {
        let spans = pack_moment_upload_spans(words, updates, grid)?;
        for span in pack_word_upload_spans(words, updates.iter().map(|&(dense, _, _)| dense))? {
            uploads.write_buffer(
                &self.sampled_indirection,
                u64::from(span.dense_start) * std::mem::size_of::<u32>() as u64,
                &span.bytes,
            )?;
        }
        for span in spans {
            let row_bytes =
                u32::try_from(span.row_bytes()).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
            uploads.write_texture(
                &self.depth_moments,
                span.origin,
                &span.bytes,
                row_bytes,
                span.rows,
                wgpu::Extent3d {
                    width: row_bytes / 8,
                    height: span.rows,
                    depth_or_array_layers: 1,
                },
            )?;
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
mod upload_packing_tests {
    use super::*;

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
    fn depth_moment_rows_of_equal_extent_stack_into_one_region() {
        // Regression: one copy per probe row made promotion cost scale with
        // a cluster's row count.
        let grid = [4, 3, 2];
        let words = vec![0; 24];
        // Rows y=0..2 of x=1..3 in slice z=0, then x=1..3 at y=1 in z=1, then
        // a narrower row that cannot join.
        let mut updates = Vec::new();
        for y in 0..2 {
            for x in 1..3 {
                updates.push((x + y * 4, 1, 2));
            }
        }
        updates.extend([(1 + 4 + 12, 3, 4), (2 + 4 + 12, 3, 4), (1 + 8 + 12, 5, 6)]);
        let spans = pack_moment_upload_spans(&words, &updates, grid).unwrap();
        assert_eq!(
            spans
                .iter()
                .map(|span| (span.origin, span.rows, span.bytes.len()))
                .collect::<Vec<_>>(),
            [
                (wgpu::Origin3d { x: 1, y: 0, z: 0 }, 2, 32),
                (wgpu::Origin3d { x: 1, y: 1, z: 1 }, 1, 16),
                (wgpu::Origin3d { x: 1, y: 2, z: 1 }, 1, 8),
            ]
        );
    }

    #[test]
    fn sampled_word_uploads_scale_with_changed_probes() {
        // Regression: promotion uploaded the complete sampled-word mirror for a few probes.
        let mut words = vec![0; 4_096];
        words[2] = 0x0002_0001;
        words[3] = 0x0004_0003;
        words[1_024] = 0x0006_0005;
        let spans = pack_word_upload_spans(&words, [1_024, 2, 3, 1_024]).unwrap();

        assert_eq!(
            spans,
            [
                WordUploadSpan {
                    dense_start: 2,
                    bytes: u32_bytes(&[0x0002_0001, 0x0004_0003]),
                },
                WordUploadSpan {
                    dense_start: 1_024,
                    bytes: u32_bytes(&[0x0006_0005]),
                },
            ]
        );
        assert_eq!(spans.iter().map(|span| span.bytes.len()).sum::<usize>(), 12);
        assert!(spans.iter().map(|span| span.bytes.len()).sum::<usize>() < words.len() * 4);
    }

    #[test]
    fn word_spans_bridge_short_gaps_from_the_mirror() {
        let mut words = vec![0; 4_096];
        words[10] = 0xa;
        words[11] = 0xb; // Unchanged, but inside a bridgeable gap.
        words[12] = 0xc;
        // One word past the longest bridgeable gap after index 12.
        let far = 13 + WORD_SPAN_MAX_GAP + 1;
        words[far as usize] = 0xd;
        let spans = pack_word_upload_spans(&words, [12, 10, far]).unwrap();
        assert_eq!(
            spans,
            [
                WordUploadSpan {
                    dense_start: 10,
                    bytes: u32_bytes(&[0xa, 0xb, 0xc]),
                },
                WordUploadSpan {
                    dense_start: far,
                    bytes: u32_bytes(&[0xd]),
                },
            ]
        );
    }

    #[test]
    fn compose_word_uploads_cover_only_installed_or_evicted_dense_probes() {
        let mut words = vec![0; 4_096];
        words[1] = 0x1111_1111; // An unchanged resident probe.
        words[2] = 0x2222_2222;
        words[3] = 0x3333_3333;
        words[1_024] = 0x4444_4444;

        let installed = pack_word_upload_spans(&words, [1_024, 3, 2, 2]).unwrap();
        assert_eq!(
            installed,
            [
                WordUploadSpan {
                    dense_start: 2,
                    bytes: u32_bytes(&[words[2], words[3]]),
                },
                WordUploadSpan {
                    dense_start: 1_024,
                    bytes: u32_bytes(&[words[1_024]]),
                },
            ]
        );

        words[2] = 0;
        words[3] = 0;
        let evicted = pack_word_upload_spans(&words, [3, 2]).unwrap();
        assert_eq!(
            evicted,
            [WordUploadSpan {
                dense_start: 2,
                bytes: u32_bytes(&[0, 0]),
            }]
        );
        assert!(pack_word_upload_spans(&words, []).unwrap().is_empty());
        assert!(matches!(
            pack_word_upload_spans(&words, [words.len() as u32]),
            Err(ShResidencyDrainError::SlotOverflow)
        ));
    }
}
