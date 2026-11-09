//! Atomic cluster installation and dense payload validation.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use super::*;

/// Device handles an install needs only while GPU pools exist: capacity
/// growth, no-queue validation, and queue uploads. CPU-only tests pass `None`
/// against a state that has no GPU pools.
pub(super) struct InstallGpu<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a crate::render::uploads::UploadQueue,
    pub(super) sh: &'a mut crate::render::sh_volume::ShVolumeResources,
    pub(super) selection_weights: &'a wgpu::Buffer,
}

/// Everything a staged install derived before its commit point. The CPU
/// mirrors already hold the new addresses; only queue uploads and the
/// installed record remain.
pub(super) struct StagedInstall {
    /// Tiles of the nodes this install wrote first.
    pub(super) local_slots: Vec<SlotRun>,
    patches: ChunkPatches,
    referenced_nodes: Vec<StoredNode>,
    /// Every sparse row the chunk names; each one holds a reference.
    sparse_rows: Vec<(u32, u32)>,
    /// The sparse rows this install wrote first.
    pub(super) sparse_plan: Vec<SparseInstallPlan>,
    required_indirect_epoch: u64,
    required_direct_epoch: u64,
}

impl StagedInstall {
    /// Dense index of every probe the chunk names.
    pub(super) fn dense_probes(&self) -> impl Iterator<Item = u32> + '_ {
        self.patches.probes.iter().map(|probe| probe.dense)
    }

    /// Hold one more storage reference with the installed record.
    pub(super) fn push_sparse_reference(&mut self, key: (u32, u32)) {
        self.sparse_rows.push(key);
    }
}

impl ShResidencyState {
    pub(super) fn install(
        &mut self,
        gpu: Option<&mut InstallGpu<'_>>,
        prepared: &PreparedShCluster,
    ) -> Result<(), ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
        let trace_started = log::log_enabled!(log::Level::Trace).then(std::time::Instant::now);
        let mut upload_copy_regions = 0;
        if self.gpu.is_some() && gpu.is_none() {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed install requires device handles while GPU pools exist",
            });
        }
        // Canonical empty clusters legitimately have no id-50 bytes or block
        // table. They carry no addresses and need no compose work, but remain
        // generation/target checked lifecycle members so residency accounting
        // stays exact.
        if prepared.chunk.blocks.is_empty() {
            self.installed.insert(
                cluster_id,
                InstalledCluster {
                    patches: Vec::new(),
                    referenced_nodes: Vec::new(),
                    sparse_rows: Vec::new(),
                    required_indirect_epoch: self.indirect_compose_epoch,
                    required_direct_epoch: self.direct_compose_epoch,
                },
            );
            self.pending_promotion.insert(cluster_id);
            return Ok(());
        }
        let has_dense_payload = self.validate_isolated_atlases(cluster_id, &prepared.chunk)?;
        // Every fallible step runs under this journal, up to and including
        // staging the uploads, so a failure anywhere restores every mirror and
        // reference count. A malformed sparse family cannot strand an address
        // or make a half cluster reachable on a later drain, and rollback
        // touches only what this cluster changed.
        let mut journal = InstallJournal::default();
        let mut staged = match self.stage_install(&mut journal, prepared, has_dense_payload) {
            Ok(staged) => staged,
            Err(error) => {
                self.roll_back(journal);
                return Err(error);
            }
        };
        if let Some(gpu) = gpu {
            // Mover channel cells this cluster places or completes, checked
            // before any upload: a malformed id-57 row refuses the cluster.
            let channel_writes =
                match self.stage_mover_channel(&mut journal, gpu, prepared, &mut staged) {
                    Ok(writes) => writes,
                    Err(error) => {
                        self.roll_back(journal);
                        // A pool may have grown before the refusal: the
                        // snapshot reports what the table holds now.
                        self.observe_mover_pool(&gpu.sh.mover_channel);
                        return Err(error);
                    }
                };
            // Nearly every decoded byte is uploaded; reserve for it once.
            let mut uploads =
                self.begin_uploads(prepared.chunk.bytes.len() + prepared.chunk.bytes.len() / 8);
            let staged_uploads = self
                .stage_uploads(&mut uploads, prepared, has_dense_payload, &staged)
                .and_then(|()| {
                    gpu.sh
                        .mover_channel
                        .stage_writes(&mut uploads, &channel_writes)
                        .map_err(|_| {
                            malformed(cluster_id, "mover channel cell write is out of bounds")
                        })
                });
            if let Err(error) = staged_uploads {
                // Discarding the batch keeps each GPU word equal to its
                // mirror: a rolled-back install uploads nothing.
                gpu.sh.mover_channel.undo_cluster(&channel_writes);
                self.roll_back(journal);
                // The pool keeps any growth this cluster caused.
                self.observe_mover_pool(&gpu.sh.mover_channel);
                return Err(error);
            }
            upload_copy_regions = uploads.copy_count();
            self.submit_uploads(uploads, gpu.device, gpu.queue);
            gpu.sh.mover_channel.commit_cluster(&channel_writes);
            self.observe_mover_pool(&gpu.sh.mover_channel);
        }
        self.commit_install(cluster_id, staged);
        if let Some(started) = trace_started {
            log::trace!(
                "[Renderer] streamed SH install: cluster={cluster_id} upload_copy_regions={upload_copy_regions} install_cpu_micros={}",
                started.elapsed().as_micros()
            );
        }
        Ok(())
    }

    /// The visibility commit: the provisional mirrors are now the live ones.
    pub(super) fn commit_install(&mut self, cluster_id: u32, staged: StagedInstall) {
        self.installed.insert(
            cluster_id,
            InstalledCluster {
                patches: staged.patches.probes,
                referenced_nodes: staged.referenced_nodes,
                sparse_rows: staged.sparse_rows,
                required_indirect_epoch: staged.required_indirect_epoch,
                required_direct_epoch: staged.required_direct_epoch,
            },
        );
        self.pending_promotion.insert(cluster_id);
    }

    /// The fallible half of an install: reference and allocate storage,
    /// rewrite compose words, mark rows, check the pools hold it, and validate
    /// every upload without queueing it. Every CPU mirror mutation goes
    /// through `journal`; the caller rolls it back on error.
    pub(super) fn stage_install(
        &mut self,
        journal: &mut InstallJournal,
        prepared: &PreparedShCluster,
        has_dense_payload: bool,
    ) -> Result<StagedInstall, ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
        let sparse_rows = self.collect_sparse_rows(cluster_id, &prepared.chunk)?;
        let referenced_sparse_rows: Vec<(u32, u32)> = sparse_rows
            .iter()
            .map(|(section_id, row)| (*section_id, row.row))
            .collect();
        let (nodes, local_slots, patches) = if has_dense_payload {
            let nodes = self.reference_chunk_nodes(journal, cluster_id, &prepared.chunk)?;
            let local_slots =
                self.local_to_live_slots(cluster_id, &prepared.chunk, &nodes.first_written)?;
            let patches = self.install_patches(journal, cluster_id, &prepared.chunk)?;
            (nodes, local_slots, patches)
        } else {
            Default::default()
        };
        // Row refs and dirty rows follow stored words, not references: a
        // shared word changes nothing compose reads.
        let dense_rows = patches
            .first_written
            .iter()
            .map(|&dense| self.affinity_row_for_dense(dense))
            .collect::<Result<Vec<_>, _>>()?;
        for (row, count) in row_counts(dense_rows) {
            self.journal_insert_row(journal, RowSet::IndirectDirty, row);
            self.journal_add_row_refs(journal, RowRefTable::IndirectBase, row, count)?;
            if self.direct_required {
                // An id-35 base without id-41/id-45 is sampled directly: no
                // direct pass will ever consume these dirty rows.
                if self.direct_compose_required {
                    self.journal_insert_row(journal, RowSet::DirectPromotionDirty, row);
                    self.journal_insert_row(journal, RowSet::DirectAnimatedDirty, row);
                }
                self.journal_add_row_refs(journal, RowRefTable::DirectBase, row, count)?;
            }
        }
        let sparse_plan = self.allocate_sparse_rows(journal, cluster_id, sparse_rows)?;
        self.validate_staged_on_gpu(
            prepared,
            has_dense_payload,
            &local_slots,
            &sparse_plan,
            self.dense_slots.capacity,
            Some(&self.sparse_pools),
        )?;
        // A write waits for the following compose epoch. A cluster that wrote
        // nothing can still be sharing storage an earlier install wrote, so
        // it also waits for every install not yet promoted.
        let (written_indirect_epoch, written_direct_epoch) = required_compose_epochs(
            self.indirect_compose_epoch,
            self.direct_compose_epoch,
            self.direct_compose_required,
            !patches.first_written.is_empty(),
            sparse_plan.iter().map(|row| row.section_id),
        )?;
        let (pending_indirect_epoch, pending_direct_epoch) = self.highest_pending_epochs();
        let required_indirect_epoch = written_indirect_epoch.max(pending_indirect_epoch);
        let required_direct_epoch = written_direct_epoch.max(pending_direct_epoch);
        // Fatal metadata, wire and address contracts must win over a
        // recoverable capacity verdict. All CPU reservations above roll back
        // together, and what happens next belongs to `pool_admission`.
        self.check_pool_room()?;
        self.validate_staged_on_gpu(
            prepared,
            has_dense_payload,
            &local_slots,
            &sparse_plan,
            self.gpu.as_ref().map_or(0, |pools| pools.shape.slots),
            None,
        )?;
        Ok(StagedInstall {
            local_slots,
            patches,
            referenced_nodes: nodes.referenced,
            sparse_rows: referenced_sparse_rows,
            sparse_plan,
            required_indirect_epoch,
            required_direct_epoch,
        })
    }

    /// Complete no-queue GPU validation after CPU allocation but before any
    /// backing bytes or CSR pairs are written.
    fn validate_staged_on_gpu(
        &self,
        prepared: &PreparedShCluster,
        has_dense_payload: bool,
        local_slots: &[SlotRun],
        sparse_plan: &[SparseInstallPlan],
        dense_capacity: u32,
        planned_sparse: Option<&BTreeMap<u32, SparsePool>>,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(gpu) = self.gpu.as_ref() else {
            return Ok(());
        };
        let cluster_id = prepared.chunk.cluster_id;
        if has_dense_payload {
            let indirect = isolated_atlas(&prepared.chunk, INDIRECT_BASE_ID)
                .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;
            gpu.validate_isolated_tiles_for_capacity(
                gpu.base_format,
                prepared.chunk.block_bytes(indirect),
                local_slots,
                dense_capacity.max(gpu.shape.slots),
            )?;
        }
        if let Some(direct) = isolated_atlas(&prepared.chunk, DIRECT_BASE_ID) {
            let format = gpu.direct_format.ok_or(malformed(
                cluster_id,
                "streamed direct pool lacks a source format",
            ))?;
            gpu.validate_isolated_tiles_for_capacity(
                format,
                prepared.chunk.block_bytes(direct),
                local_slots,
                dense_capacity.max(gpu.shape.slots),
            )?;
        }
        for row in sparse_plan {
            let current = (
                gpu.sparse_entry_capacity(row.section_id),
                gpu.sparse_tile_f16_capacity(row.section_id),
            );
            let capacity = planned_sparse
                .and_then(|pools| pools.get(&row.section_id))
                .map_or(current, |pool| {
                    (
                        current.0.max(pool.entries.capacity),
                        current.1.max(pool.tiles.capacity),
                    )
                });
            if row.section_id == INDIRECT_DELTA_ID {
                gpu.validate_indirect_sparse_row_for_capacity(
                    row.entries.start,
                    row.tiles.start,
                    &row.payload,
                    capacity,
                )?;
            } else {
                gpu.validate_direct_sparse_rows_for_capacity(
                    row.section_id,
                    &[row.direct_upload()?],
                    capacity,
                )?;
            }
        }
        Ok(())
    }

    /// Stage every validated upload into one batch. Backing data is copied
    /// before each compose CSR pair is published; the compose indirection
    /// follows last.
    fn stage_uploads(
        &mut self,
        uploads: &mut StagedUploads,
        prepared: &PreparedShCluster,
        has_dense_payload: bool,
        staged: &StagedInstall,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let cluster_id = prepared.chunk.cluster_id;
        if has_dense_payload {
            let indirect = isolated_atlas(&prepared.chunk, INDIRECT_BASE_ID)
                .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;
            gpu.upload_isolated_tiles(
                uploads,
                &gpu.base,
                gpu.base_format,
                prepared.chunk.block_bytes(indirect),
                &staged.local_slots,
            )?;
        }
        if let Some(direct) = isolated_atlas(&prepared.chunk, DIRECT_BASE_ID) {
            let destination = gpu.direct_base.as_ref().ok_or(malformed(
                cluster_id,
                "chunk carries id-35 but streamed direct pool is absent",
            ))?;
            let format = gpu
                .direct_format
                .ok_or(malformed(cluster_id, "streamed direct pool lacks a format"))?;
            gpu.upload_isolated_tiles(
                uploads,
                destination,
                format,
                prepared.chunk.block_bytes(direct),
                &staged.local_slots,
            )?;
        }
        let mut indirect_rows = Vec::new();
        let mut direct_rows = Vec::new();
        let mut animated_direct_rows = Vec::new();
        for row in &staged.sparse_plan {
            match row.section_id {
                INDIRECT_DELTA_ID => indirect_rows.push(row),
                DIRECT_DELTA_ID => direct_rows.push(row.direct_upload()?),
                ANIMATED_DIRECT_DELTA_ID => animated_direct_rows.push(row.direct_upload()?),
                _ => unreachable!("sparse source was validated before upload"),
            }
        }
        if !indirect_rows.is_empty() {
            gpu.upload_indirect_sparse_rows(uploads, &indirect_rows)?;
        }
        if !direct_rows.is_empty() {
            gpu.upload_direct_sparse_rows(uploads, DIRECT_DELTA_ID, &direct_rows)?;
        }
        if !animated_direct_rows.is_empty() {
            gpu.upload_direct_sparse_rows(
                uploads,
                ANIMATED_DIRECT_DELTA_ID,
                &animated_direct_rows,
            )?;
        }
        gpu.upload_changed_compose_words(
            uploads,
            &self.compose_words,
            staged.patches.first_written.iter().copied(),
        )
    }

    pub(super) fn validate_isolated_atlases(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<bool, ShResidencyDrainError> {
        let indirect = isolated_atlas(chunk, INDIRECT_BASE_ID);
        let direct_block = isolated_atlas(chunk, DIRECT_BASE_ID);
        let has_probe_patches = chunk
            .blocks
            .iter()
            .any(|block| block.kind == PROBE_PATCH_BLOCK);
        let Some(indirect) = indirect else {
            if direct_block.is_some() || has_probe_patches {
                return Err(malformed(
                    cluster_id,
                    "dense chunk omits its required id-34 isolated atlas",
                ));
            }
            // A cluster may name only sparse rows. It has no dense slots or
            // probe patches to upload, but its sparse family remains
            // independently installable under the same transaction.
            return Ok(false);
        };
        if !has_probe_patches {
            return Err(malformed(
                cluster_id,
                "id-34 isolated atlas has no probe-patch block",
            ));
        }
        validate_isolated_atlas_block(
            cluster_id,
            chunk.block_bytes(indirect),
            indirect.element_count,
        )?;
        // Id-35 is a coupled member when its header projection was present;
        // a one-sided manifest intentionally has no such requirement. Do not
        // accept a partial ready cluster in the paired case.
        if self.direct_required && direct_block.is_none() {
            return Err(malformed(
                cluster_id,
                "chunk omits required id-35 member of the shared dense pool",
            ));
        }
        if let Some(direct) = direct_block {
            validate_isolated_atlas_block(
                cluster_id,
                chunk.block_bytes(direct),
                direct.element_count,
            )?;
            if direct.element_count != indirect.element_count {
                return Err(malformed(
                    cluster_id,
                    "id-35 isolated atlas has a different shared-slot count",
                ));
            }
        }
        Ok(true)
    }

    /// Start an upload batch with room for about `reserve` bytes.
    pub(super) fn begin_uploads(&mut self, reserve: usize) -> StagedUploads {
        self.gpu.as_mut().map_or_else(
            || StagedUploads::from_scratch(Vec::new(), 0),
            |pools| pools.begin_uploads(reserve),
        )
    }

    /// Submit one upload batch. CPU-only states record nothing to submit.
    pub(super) fn submit_uploads(
        &mut self,
        uploads: StagedUploads,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
    ) {
        if let Some(pools) = self.gpu.as_mut() {
            pools.submit_uploads(uploads, device, queue);
        }
    }

    pub(super) fn probe_occlusion_enabled(&self) -> bool {
        self.gpu
            .as_ref()
            .is_some_and(StreamingGpuPools::probe_occlusion_enabled)
    }

    pub(super) fn release_retired_generations(&mut self) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.release_completed_retirement();
        }
    }
}

fn isolated_atlas(
    chunk: &DecodedClusterShPayload,
    section_id: u32,
) -> Option<&postretro_level_format::cluster_sh_payloads::DecodedClusterShBlock> {
    chunk
        .blocks
        .iter()
        .find(|block| block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == section_id)
}
