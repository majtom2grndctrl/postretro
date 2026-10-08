// Executes one planned lightmap drain on the GPU: one encoder, one upload
// batch, one submission; then updates the drain counters.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::PreparedLightmapBlock;
use postretro_render_cpu::lightmap_pool::BLOCK_TABLE_ENTRY_BYTES;

use super::super::pool::block_region_writes;
use super::textures::{PoolTextures, RetiringPool};
use super::{DrainEffects, LightmapResidencyDrainError, LightmapStreamState};
use crate::render::StagedUploads;

/// What one drain's execution did on the GPU.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Executed {
    submitted: bool,
    grew: bool,
    uploads: u64,
    install_bytes: u64,
    table_writes: u64,
    copies: u64,
}

impl LightmapStreamState {
    /// Record the model's current plan in field order — growth, then the
    /// repack's moves, then the pair uploads after the moves (a new block
    /// may land where a moved one was), then the table writes — into one
    /// encoder, and submit it once. Uploads and table writes are staged
    /// copies recorded after the moves, never `Queue::write_*`, which would
    /// land ahead of this submission's command buffer.
    ///
    /// On an error nothing was submitted; the caller aborts the drain.
    pub(super) fn execute(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        ready: &[PreparedLightmapBlock],
    ) -> Result<Executed, LightmapResidencyDrainError> {
        let plan = self.model.plan();
        if plan.growth.is_none()
            && plan.copies.is_empty()
            && plan.uploads.is_empty()
            && plan.table_writes.is_empty()
        {
            return Ok(Executed::default());
        }
        let mut executed = Executed {
            submitted: true,
            copies: plan.copies.len() as u64,
            ..Executed::default()
        };

        let grown = match plan.growth {
            Some(growth) => {
                let required_layers = growth.to_layers + 1;
                if required_layers > self.max_array_layers {
                    return Err(LightmapResidencyDrainError::GpuCapacity {
                        required_layers,
                        max_layers: self.max_array_layers,
                    });
                }
                Some((
                    growth.from_layers,
                    PoolTextures::new(device, self.format, required_layers, self.with_shadowmask),
                ))
            }
            None => None,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed Lightmap Drain"),
        });
        if let Some((from_layers, next)) = &grown {
            self.textures
                .copy_layers_into(&mut encoder, next, *from_layers);
        }
        let target = grown.as_ref().map_or(&self.textures, |(_, next)| next);
        debug_assert_eq!(target.array_layers(), self.model.spare_layer() + 1);
        for &copy in &plan.copies {
            target.record_block_copy(&mut encoder, self.format, copy);
        }

        let reserve = plan
            .uploads
            .iter()
            .filter_map(|u| ready.iter().find(|p| p.block == u.block))
            .map(|p| payload_bytes(&p.payload))
            .sum::<usize>()
            + plan.table_writes.len() * BLOCK_TABLE_ENTRY_BYTES;
        let mut uploads =
            StagedUploads::from_scratch(std::mem::take(&mut self.upload_scratch), reserve);
        let upload_error = |error| LightmapResidencyDrainError::Upload(format!("{error:?}"));
        for upload in &plan.uploads {
            let payload = &ready
                .iter()
                .find(|p| p.block == upload.block)
                .expect("fail_mismatched_payloads failed every upload without a payload")
                .payload;
            let writes = block_region_writes(
                self.format,
                upload.placement,
                (upload.width, upload.height),
                payload,
                self.with_shadowmask,
            );
            for write in writes {
                let texture = target
                    .plane(write.plane)
                    .expect("shadowmask writes only when the pool keeps one");
                uploads
                    .write_texture(
                        texture,
                        write.origin,
                        write.data,
                        write.bytes_per_row,
                        write.rows,
                        write.extent,
                    )
                    .map_err(upload_error)?;
                executed.install_bytes += write.data.len() as u64;
            }
            executed.uploads += 1;
        }
        for write in &plan.table_writes {
            let mut entry = [0u8; BLOCK_TABLE_ENTRY_BYTES];
            for (bytes, word) in entry
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(write.entry.to_words())
            {
                bytes.copy_from_slice(&word.to_ne_bytes());
            }
            let offset = u64::from(write.index) * BLOCK_TABLE_ENTRY_BYTES as u64;
            uploads
                .write_buffer(&self.table, offset, &entry)
                .map_err(upload_error)?;
        }
        executed.table_writes = plan.table_writes.len() as u64;

        let recorded = uploads.record(&mut self.staging, device, &mut encoder);
        queue.submit(std::iter::once(encoder.finish()));
        self.upload_scratch = recorded.finish(&self.staging);

        if let Some((_, next)) = grown {
            let previous = std::mem::replace(&mut self.textures, next);
            self.retiring = Some(RetiringPool::after_submitted_work(queue.raw(), previous));
            executed.grew = true;
        }
        Ok(executed)
    }

    pub(super) fn record_drain(
        &mut self,
        executed: Executed,
        started: std::time::Instant,
        effects: &mut DrainEffects,
    ) {
        let plan = self.model.plan();
        let c = &mut self.counters;
        c.drains += 1;
        c.installs += executed.uploads;
        c.failed_installs += plan.failed.len() as u64;
        c.deferred_pairs += plan.deferred.len() as u64;
        c.evictions += plan.evicted.len() as u64;
        c.table_entries_written += executed.table_writes;
        c.last_drain_table_writes = executed.table_writes;
        c.last_drain_uploads = executed.uploads;
        c.last_drain_install_bytes = executed.install_bytes;
        if plan.report.repacked {
            c.repacks += 1;
            c.repack_copy_commands += executed.copies;
        }
        if executed.submitted {
            c.submissions += 1;
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            c.last_drain_install_micros = micros;
            c.max_drain_install_micros = c.max_drain_install_micros.max(micros);
        }
        if executed.grew {
            c.growths += 1;
            c.pool_texture_sets += 1;
            c.pool_layers = self.model.layers();
            c.active_pool_bytes = self.textures.bytes();
            c.retiring_pool_bytes = self.retiring.as_ref().map_or(0, RetiringPool::bytes);
            c.growth_transient_peak_bytes = c
                .growth_transient_peak_bytes
                .max(c.active_pool_bytes + c.retiring_pool_bytes);
            effects.pool_replaced = true;
            effects.meter_changed = true;
        }
        debug_assert_eq!(c.pool_texture_sets, self.model.texture_allocations());
    }
}

fn payload_bytes(payload: &postretro_level_format::lightmap::LightmapBlockPayload) -> usize {
    payload.irradiance.len()
        + payload.direction.len()
        + payload
            .shadowmask
            .as_ref()
            .map_or(0, |[a, b]| a.len() + b.len())
}
