//! Isolated tile uploads and compose dispatch surface for streamed SH pools.

use super::*;

impl StreamingGpuPools {
    /// Upload only canonical nodes from an id-50 isolated atlas block. Each
    /// scratch is one physical 8×8 cell; no decoded cluster body survives this
    /// call.
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
        let slots = read_u32(block, 4)?;
        let width = read_u32(block, 8)?;
        let height = read_u32(block, 12)?;
        let layers = read_u32(block, 16)?;
        debug_assert_eq!(texture_format(format)?, expected_format);
        debug_assert!(width > 0 && height > 0 && layers > 0);
        debug_assert_eq!(width % PHYSICAL_TILE_DIMENSION, 0);
        debug_assert_eq!(height % PHYSICAL_TILE_DIMENSION, 0);
        let local_tiles_per_row = width / PHYSICAL_TILE_DIMENSION;
        let local_tiles_per_layer = local_tiles_per_row
            .checked_mul(height / PHYSICAL_TILE_DIMENSION)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let payload = &block[20..];
        for (&local_slot, &live_slot) in local_to_live_slot {
            debug_assert!(local_slot < slots && live_slot < self.shape.slots);
            let local_layer = local_slot / local_tiles_per_layer;
            let local_in_layer = local_slot % local_tiles_per_layer;
            let local_x = (local_in_layer % local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let local_y = (local_in_layer / local_tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_layer = live_slot / self.shape.tiles_per_layer;
            let target_in_layer = live_slot % self.shape.tiles_per_layer;
            let target_x = (target_in_layer % self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            let target_y = (target_in_layer / self.shape.tiles_per_row) * PHYSICAL_TILE_DIMENSION;
            match expected_format {
                wgpu::TextureFormat::Bc6hRgbUfloat => {
                    let blocks_per_row = width / 4;
                    let blocks_per_image = blocks_per_row
                        .checked_mul(height / 4)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_block_x = local_x / 4;
                    let source_block_y = local_y / 4;
                    let image_start = usize::try_from(local_layer)
                        .ok()
                        .and_then(|layer| layer.checked_mul(blocks_per_image as usize))
                        .and_then(|blocks| blocks.checked_mul(16))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let row_bytes = usize::try_from(blocks_per_row)
                        .ok()
                        .and_then(|blocks| blocks.checked_mul(16))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_start = image_start
                        .checked_add(usize::try_from(source_block_y).unwrap() * row_bytes)
                        .and_then(|offset| {
                            offset.checked_add(usize::try_from(source_block_x).unwrap() * 16)
                        })
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let second_row = source_start
                        .checked_add(row_bytes)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let mut scratch = [0u8; 64];
                    scratch[..32].copy_from_slice(
                        payload.get(source_start..source_start + 32).ok_or(
                            ShResidencyDrainError::MalformedChunk {
                                cluster_id: 0,
                                reason: "BC6H isolated atlas tile is truncated",
                            },
                        )?,
                    );
                    scratch[32..].copy_from_slice(payload.get(second_row..second_row + 32).ok_or(
                        ShResidencyDrainError::MalformedChunk {
                            cluster_id: 0,
                            reason: "BC6H isolated atlas second row is truncated",
                        },
                    )?);
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
                            bytes_per_row: Some(32),
                            rows_per_image: Some(2),
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                wgpu::TextureFormat::Rgba16Float => {
                    let image_start = usize::try_from(local_layer)
                        .ok()
                        .and_then(|layer| layer.checked_mul(width as usize))
                        .and_then(|texels| texels.checked_mul(height as usize))
                        .and_then(|texels| texels.checked_mul(8))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let row_bytes = usize::try_from(width)
                        .ok()
                        .and_then(|pixels| pixels.checked_mul(8))
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let source_start = image_start
                        .checked_add(usize::try_from(local_y).unwrap() * row_bytes)
                        .and_then(|offset| {
                            offset.checked_add(usize::try_from(local_x).unwrap() * 8)
                        })
                        .ok_or(ShResidencyDrainError::SlotOverflow)?;
                    let mut scratch = [0u8; 512];
                    for row in 0..8usize {
                        let source = source_start
                            .checked_add(row * row_bytes)
                            .ok_or(ShResidencyDrainError::SlotOverflow)?;
                        scratch[row * 64..(row + 1) * 64].copy_from_slice(
                            payload.get(source..source + 64).ok_or(
                                ShResidencyDrainError::MalformedChunk {
                                    cluster_id: 0,
                                    reason: "RGBA16F isolated atlas tile is truncated",
                                },
                            )?,
                        );
                    }
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
                            bytes_per_row: Some(64),
                            rows_per_image: Some(8),
                        },
                        wgpu::Extent3d {
                            width: 8,
                            height: 8,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                _ => unreachable!("streamed SH base formats are validated"),
            }
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

    pub(in crate::render::sh_streaming) fn upload_indirect_sparse_row(
        &mut self,
        queue: &wgpu::Queue,
        entry_start: u32,
        tile_f16_start: u32,
        row: &super::super::ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        self.indirect_compose
            .upload_sparse_row(queue, entry_start, tile_f16_start, row)
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
        let bytes = u32_bytes(words);
        queue.write_buffer(&self.sampled_indirection, 0, &bytes);
        let xy = grid[0]
            .checked_mul(grid[1])
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        for &(dense, mean, mean_sq) in updates {
            let x = dense % grid[0];
            let y = (dense / grid[0]) % grid[1];
            let z = dense / xy;
            let word = *words
                .get(dense as usize)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let pixels = [mean, mean_sq, word as u16, (word >> 16) as u16];
            let mut bytes = [0u8; 8];
            for (index, value) in pixels.into_iter().enumerate() {
                bytes[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.depth_moments,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
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
