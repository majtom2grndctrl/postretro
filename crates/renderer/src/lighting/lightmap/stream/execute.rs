// Executes one planned drain of both pools on the GPU: one encoder, one upload
// batch, one submission; then updates the drain counters.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_loader::PreparedLightmapBlock;
use postretro_render_cpu::lightmap_pool::{BLOCK_TABLE_ENTRY_BYTES, PerPool, PoolKind, RepackKind};

use super::super::pool::block_region_writes;
use super::textures::PoolTextures;
use super::{DrainEffects, LightmapResidencyDrainError, LightmapStreamState};
use crate::render::StagedUploads;

/// What one drain's execution did on the GPU.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Executed {
    submitted: bool,
    grew: PerPool<bool>,
    uploads: u64,
    install_bytes: u64,
    table_writes: u64,
    copies: PerPool<u64>,
}

impl LightmapStreamState {
    /// Record the model's current plan in field order, for both pools: each
    /// pool's growth and repack moves, then the pair uploads after the moves
    /// (a new block may land where a moved one was), then the table writes,
    /// all into one encoder, submitted once. A move lands in its pool's new
    /// generation when that pool grew, and in its live one otherwise; a
    /// retiring generation is never touched. Uploads and table writes are
    /// staged copies recorded after the moves, never `Queue::write_*`, which
    /// would land ahead of this submission's command buffer.
    ///
    /// On an error nothing was submitted; the caller aborts the drain.
    pub(super) fn execute(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        ready: &[PreparedLightmapBlock],
    ) -> Result<Executed, LightmapResidencyDrainError> {
        let plan = self.model.plan();
        let pools_idle = PoolKind::ALL
            .iter()
            .all(|&kind| plan.pools[kind].growth.is_none() && plan.pools[kind].copies.is_empty());
        if pools_idle && plan.uploads.is_empty() && plan.table_writes.is_empty() {
            return Ok(Executed::default());
        }
        let mut executed = Executed {
            submitted: true,
            copies: PerPool::from_fn(|kind| plan.pools[kind].copies.len() as u64),
            ..Executed::default()
        };

        let mut grown: PerPool<Option<PoolTextures>> = PerPool::default();
        for kind in PoolKind::ALL {
            if let Some(growth) = plan.pools[kind].growth {
                let required_layers = growth.to_layers + 1;
                if required_layers > self.max_array_layers {
                    return Err(LightmapResidencyDrainError::GpuCapacity {
                        required_layers,
                        max_layers: self.max_array_layers,
                    });
                }
                grown[kind] = Some(PoolTextures::new(
                    device,
                    kind,
                    self.format,
                    required_layers,
                ));
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed Lightmap Drain"),
        });
        for kind in PoolKind::ALL {
            let pool = &plan.pools[kind];
            let Some(generation) = &self.pools[kind] else {
                debug_assert!(pool.growth.is_none() && pool.copies.is_empty());
                continue;
            };
            if let (Some(growth), Some(next)) = (pool.growth, &grown[kind]) {
                generation
                    .live
                    .copy_layers_into(&mut encoder, next, growth.from_layers);
            }
            let target = grown[kind].as_ref().unwrap_or(&generation.live);
            debug_assert_eq!(
                target.array_layers(),
                self.model.pool(kind).spare_layer() + 1
            );
            for &copy in &pool.copies {
                target.record_block_copy(&mut encoder, self.format, copy);
            }
        }

        let targets = PerPool::from_fn(|kind| {
            grown[kind]
                .as_ref()
                .or(self.pools[kind].as_ref().map(|generation| &generation.live))
        });
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
                upload.planes,
                (upload.width, upload.height),
                payload,
            );
            for write in writes {
                let texture = targets[write.plane.pool()]
                    .expect("a half placed in a pool has that pool's generation")
                    .plane(write.plane);
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

        for kind in PoolKind::ALL {
            if let Some(next) = grown[kind].take() {
                self.pools[kind]
                    .as_mut()
                    .expect("a pool that grew has a generation")
                    .replace(queue.raw(), next);
                executed.grew[kind] = true;
            }
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
        if executed.submitted {
            c.submissions += 1;
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            c.last_drain_install_micros = micros;
            c.max_drain_install_micros = c.max_drain_install_micros.max(micros);
        }
        for kind in PoolKind::ALL {
            let pool = &plan.pools[kind];
            if pool.repack.is_some() {
                c.repacks[kind] += 1;
                c.repack_copy_commands[kind] += executed.copies[kind];
            }
            c.compactions[kind] +=
                u64::from(matches!(pool.repack, Some(RepackKind::Compaction(_))));
            c.compaction_rollbacks[kind] += u64::from(pool.compaction_rolled_back);
            if executed.grew[kind] {
                let generation = self.pools[kind]
                    .as_ref()
                    .expect("a pool that grew has a generation");
                c.growths[kind] += 1;
                c.pool_texture_sets[kind] += 1;
                c.pool_layers[kind] = self.model.pool(kind).layers();
                c.active_pool_bytes[kind] = generation.live.bytes();
                c.retiring_pool_bytes[kind] = generation.retiring_bytes();
            }
            debug_assert_eq!(
                c.pool_texture_sets[kind],
                self.model.pool(kind).texture_allocations()
            );
        }
        if executed.grew.any() {
            c.growth_transient_peak_bytes = c.growth_transient_peak_bytes.max(c.held_bytes());
            effects.pool_replaced = true;
            effects.meter_changed = true;
        }
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
