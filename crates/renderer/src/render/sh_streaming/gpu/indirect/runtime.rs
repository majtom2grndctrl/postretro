//! Runtime, upload, and retirement helpers for the streamed indirect carrier.

use super::*;

impl StreamingIndirectCompose {
    /// Dense growth changes only the sampled/storage atlas views and the
    /// compact atlas geometry. The renderer-owned CSR backing remains live
    /// in this family; rebuilding it here would transiently duplicate every
    /// resident id-27 row during an unrelated atlas expansion.
    pub(in crate::render::sh_streaming::gpu) fn rebind_dense(
        &mut self,
        device: &wgpu::Device,
        shape: AtlasShape,
        base_view: &wgpu::TextureView,
        total_storage_view: &wgpu::TextureView,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
    ) {
        self.grid.atlas_dimensions = [shape.extent().width, shape.extent().height];
        self.grid.atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.tiles_per_layer = shape.tiles_per_layer;
        self.grid.atlas_layer_count = shape.layers;
        self.grid.compact_atlas_tiles_per_row = shape.tiles_per_row;
        self.grid.compact_atlas_tiles_per_layer = shape.tiles_per_layer;
        let replacement = Self::build_bind_group(
            device,
            &self.bind_group_layout,
            base_view,
            total_storage_view,
            &self.sampler,
            &self.grid_buffer,
            &self.origin_buffer,
            &self.delta_subblocks,
            &self.affinity_offsets,
            &sh.animation.descriptors,
            &sh.animation.anim_samples,
            &self.affinity_lights,
            &self.descriptor_index_buffer,
            compose_indirection,
            &self.compaction_metadata,
        );
        let _ = std::mem::replace(&mut self.bind_group, replacement);
    }

    pub(in crate::render::sh_streaming::gpu) fn validate_sparse_row(
        &self,
        entry_start: u32,
        tile_f16_start: u32,
        row: &ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        let entry_end = entry_start
            .checked_add(row.entry_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let tile_end = tile_f16_start
            .checked_add(row.tile_f16_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if entry_end > self.entry_capacity || tile_end > self.tile_f16_capacity {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed indirect sparse pool needs growth",
            });
        }
        if row.lights.len() != row.entry_count as usize
            || row.entry_tile_f16_offsets.len() != row.entry_count as usize
            || row.tile_f16.len() != row.tile_f16_count as usize
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "parsed sparse row does not match its header counts",
            });
        }
        if tile_f16_start % 2 != 0 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed sparse tile allocation is not word aligned",
            });
        }
        let cell_count = checked_cell_count(self.grid.affinity_dims)?;
        let entry_offset_base = cell_count
            .checked_mul(3)
            .and_then(|base| base.checked_add(entry_start))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let compaction_end = entry_offset_base
            .checked_add(row.entry_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if u64::from(compaction_end)
            .checked_mul(4)
            .is_none_or(|end| end > self.compaction_metadata.size())
        {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed indirect compaction metadata needs growth",
            });
        }
        if !sparse_offsets_fit_allocation(&row.entry_tile_f16_offsets, 0, row.tile_f16_count) {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed sparse entry points outside its tile allocation",
            });
        }
        let pair_offset = u64::from(row.row)
            .checked_mul(8)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if pair_offset
            .checked_add(8)
            .is_none_or(|end| end > self.affinity_offsets.size())
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse row exceeds metadata",
            });
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming::gpu) fn upload_sparse_row(
        &mut self,
        queue: &wgpu::Queue,
        entry_start: u32,
        tile_f16_start: u32,
        row: &ParsedSparseRow,
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_sparse_row(entry_start, tile_f16_start, row)?;
        queue.write_buffer(
            &self.affinity_lights,
            u64::from(entry_start) * 4,
            &u32_bytes(&row.lights),
        );
        let packed = u16_words(&row.tile_f16)?;
        if !packed.is_empty() {
            queue.write_buffer(
                &self.delta_subblocks,
                u64::from(tile_f16_start / 2) * 4,
                &u32_bytes(&packed),
            );
        }
        let cell_count = checked_cell_count(self.grid.affinity_dims)?;
        let entry_offset_base = cell_count
            .checked_mul(3)
            .and_then(|base| base.checked_add(entry_start))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let offsets: Vec<u32> = row
            .entry_tile_f16_offsets
            .iter()
            .map(|offset| {
                tile_f16_start
                    .checked_add(*offset)
                    .ok_or(ShResidencyDrainError::SlotOverflow)
            })
            .collect::<Result<_, _>>()?;
        queue.write_buffer(
            &self.compaction_metadata,
            u64::from(entry_offset_base) * 4,
            &u32_bytes(&offsets),
        );
        let pair_index = u64::from(row.row)
            .checked_mul(2)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let entry_end = entry_start
            .checked_add(row.entry_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        queue.write_buffer(
            &self.affinity_offsets,
            pair_index * 4,
            &u32_bytes(&[entry_start, entry_end]),
        );
        Ok(())
    }

    pub(in crate::render::sh_streaming::gpu) fn dispatch<'a>(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        dirty_ranges: &[(u32, u32)],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        let upload = build_dynamic_compose_grid_upload_for_ranges(
            self.grid,
            STREAMED_SH_PHYSICAL_TILE_STRIDE,
            dirty_ranges,
            self.max_workgroups_x,
            self.dynamic_alignment,
            self.max_buffer_size,
        )
        .ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dirty compose range exceeds adapter limits",
        })?;
        if u64::try_from(upload.bytes.len()).map_err(|_| ShResidencyDrainError::SlotOverflow)?
            > self.grid_capacity
        {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed dirty compose record count exceeds pool capacity",
            });
        }
        queue.write_buffer(&self.grid_buffer, 0, &upload.bytes);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Streamed SH Compose"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, uniform_bind_group, &[]);
        for dispatch in &upload.dispatches {
            pass.set_bind_group(1, &self.bind_group, &[dispatch.dynamic_offset]);
            pass.dispatch_workgroups(dispatch.workgroup_count, 1, 1);
        }
        Ok(())
    }

    pub(in crate::render::sh_streaming::gpu) fn clear_row_pair(
        &self,
        queue: &wgpu::Queue,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let offset = u64::from(row)
            .checked_mul(8)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if offset
            .checked_add(8)
            .is_none_or(|end| end > self.affinity_offsets.size())
        {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        queue.write_buffer(&self.affinity_offsets, offset, &u32_bytes(&[0, 0]));
        Ok(())
    }

    pub(in crate::render::sh_streaming::gpu) fn clear_all_row_pairs(&self, queue: &wgpu::Queue) {
        let zeroes = vec![0; usize::try_from(self.affinity_offsets.size()).unwrap_or(0)];
        if !zeroes.is_empty() {
            queue.write_buffer(&self.affinity_offsets, 0, &zeroes);
        }
    }

    pub(in crate::render::sh_streaming::gpu) fn fixed_metadata_bytes(&self) -> u64 {
        let cell_words = checked_cell_count(self.grid.affinity_dims)
            .and_then(|count| {
                u64::from(count)
                    .checked_mul(3)
                    .ok_or(ShResidencyDrainError::SlotOverflow)
            })
            .expect("validated streamed affinity dimensions must fit fixed metadata");
        let sparse_dummy_bytes = (!self.source_present).then_some(
            self.affinity_lights
                .size()
                .checked_add(self.delta_subblocks.size())
                .and_then(|bytes| {
                    u64::from(self.entry_capacity)
                        .checked_mul(4)
                        .and_then(|entries| bytes.checked_add(entries))
                })
                .expect("validated streamed sparse dummy buffers must fit fixed metadata"),
        );
        [
            self.grid_capacity,
            self.affinity_offsets.size(),
            cell_words
                .checked_mul(4)
                .expect("validated streamed CSR prefix must fit fixed metadata"),
            self.origin_buffer.size(),
            // This buffer is always allocated at least one u32 for a legal
            // empty storage binding, so count the actual allocation rather
            // than the logical descriptor-index vector length.
            self.descriptor_index_buffer.size(),
            sparse_dummy_bytes.unwrap_or(0),
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .expect("validated streamed fixed metadata must fit u64")
    }

    pub(in crate::render::sh_streaming::gpu) fn active_pool_bytes(&self) -> u64 {
        if !self.source_present {
            return 0;
        }
        let cell_words = checked_cell_count(self.grid.affinity_dims)
            .and_then(|count| {
                u64::from(count)
                    .checked_mul(3)
                    .ok_or(ShResidencyDrainError::SlotOverflow)
            })
            .expect("validated streamed affinity dimensions must fit active capacity");
        self.affinity_lights
            .size()
            .checked_add(self.delta_subblocks.size())
            .and_then(|bytes| {
                cell_words
                    .checked_mul(4)
                    .and_then(|prefix| self.compaction_metadata.size().checked_sub(prefix))
                    .and_then(|tail| bytes.checked_add(tail))
            })
            .expect("validated streamed active sparse capacity must fit u64")
    }

    pub(in crate::render::sh_streaming::gpu) fn retired_sparse_capacity_bytes(
        &self,
    ) -> Result<u64, ShResidencyDrainError> {
        checked_indirect_sparse_backing_bytes([
            self.affinity_offsets.size(),
            self.affinity_lights.size(),
            self.delta_subblocks.size(),
            self.compaction_metadata.size(),
        ])
    }

    /// Consume a displaced carrier after its replacement has been published.
    /// Only the independently grown id-27 storage backings survive the
    /// submission fence; all compose state and binding references drop here.
    pub(in crate::render::sh_streaming::gpu) fn into_retired_sparse_resources(
        self,
    ) -> RetiredIndirectSparseResources {
        let capacity_bytes = checked_indirect_sparse_backing_bytes([
            self.affinity_offsets.size(),
            self.affinity_lights.size(),
            self.delta_subblocks.size(),
            self.compaction_metadata.size(),
        ])
        .expect("validated indirect sparse retirement buffers must fit u64");
        RetiredIndirectSparseResources {
            affinity_offsets: self.affinity_offsets,
            affinity_lights: self.affinity_lights,
            delta_subblocks: self.delta_subblocks,
            compaction_metadata: self.compaction_metadata,
            capacity_bytes,
        }
    }

    pub(in crate::render::sh_streaming::gpu) const fn entry_capacity(&self) -> u32 {
        self.entry_capacity
    }

    pub(in crate::render::sh_streaming::gpu) const fn tile_f16_capacity(&self) -> u32 {
        self.tile_f16_capacity
    }

    pub(in crate::render::sh_streaming::gpu) fn has_active_animation(
        &self,
        animation: &crate::render::sh_volume::AnimatedLightBuffers,
    ) -> bool {
        animation.any_active_for_descriptor_indices(&self.animation_descriptor_indices)
    }

    pub(in crate::render::sh_streaming::gpu) fn copy_retained_to(
        &self,
        destination: &Self,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        copy_buffer(
            encoder,
            &self.affinity_offsets,
            &destination.affinity_offsets,
        );
        copy_buffer(encoder, &self.affinity_lights, &destination.affinity_lights);
        copy_buffer(encoder, &self.delta_subblocks, &destination.delta_subblocks);
        copy_buffer(
            encoder,
            &self.compaction_metadata,
            &destination.compaction_metadata,
        );
    }
}

fn checked_indirect_sparse_backing_bytes(buffers: [u64; 4]) -> Result<u64, ShResidencyDrainError> {
    buffers.into_iter().try_fold(0u64, |total, bytes| {
        total
            .checked_add(bytes)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_sparse_backing_uses_only_the_four_storage_buffers() {
        assert_eq!(
            checked_indirect_sparse_backing_bytes([8, 12, 16, 20]),
            Ok(56)
        );
    }

    #[test]
    fn retired_sparse_backing_rejects_overflow() {
        assert_eq!(
            checked_indirect_sparse_backing_bytes([u64::MAX, 1, 0, 0]),
            Err(ShResidencyDrainError::SlotOverflow)
        );
    }
}
