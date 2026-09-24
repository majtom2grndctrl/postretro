//! Atomic canonical-cluster installation and dense payload validation.

use super::*;

/// Device handles an install needs only while GPU pools exist: capacity
/// growth, no-queue validation, and queue uploads. CPU-only tests pass `None`
/// against a state that has no GPU pools.
pub(super) struct InstallGpu<'a> {
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) sh: &'a mut crate::render::sh_volume::ShVolumeResources,
    pub(super) uniform_bind_group_layout: &'a wgpu::BindGroupLayout,
    pub(super) selection_weights: &'a wgpu::Buffer,
}

/// Everything a staged install derived before its commit point. The CPU
/// mirrors already hold the new addresses; only queue uploads and the
/// installed record remain.
struct StagedInstall {
    local_slots: BTreeMap<u32, u32>,
    patches: Vec<InstalledProbe>,
    sparse_plan: Vec<SparseInstallPlan>,
    required_indirect_epoch: u64,
    required_direct_epoch: u64,
}

impl ShResidencyState {
    pub(super) fn install(
        &mut self,
        mut gpu: Option<&mut InstallGpu<'_>>,
        prepared: &PreparedShCluster,
    ) -> Result<(), ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
        if self.gpu.is_some() && gpu.is_none() {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed install requires device handles while GPU pools exist",
            });
        }
        // Canonical empty clusters legitimately have no id-50 bytes or block
        // table. They carry no addresses and need no compose work, but remain
        // generation/target checked lifecycle members so ownership accounting
        // stays exact.
        if prepared.chunk.blocks.is_empty() {
            self.installed.insert(
                cluster_id,
                InstalledCluster {
                    patches: Vec::new(),
                    owned_nodes: Vec::new(),
                    sparse_rows: Vec::new(),
                    required_indirect_epoch: self.indirect_compose_epoch,
                    required_direct_epoch: self.direct_compose_epoch,
                },
            );
            self.pending_promotion.insert(cluster_id);
            return Ok(());
        }
        let has_dense_payload = self.validate_isolated_atlases(cluster_id, &prepared.chunk)?;
        // Every fallible wire/allocator operation happens before the single
        // visibility commit below. Each staged mutation is journaled, so a
        // malformed sparse family cannot strand an address or make a half
        // cluster reachable on a later drain, and rollback touches only what
        // this cluster changed.
        let mut journal = InstallJournal::default();
        let staged = match self.stage_install(
            &mut journal,
            gpu.as_deref_mut(),
            prepared,
            has_dense_payload,
        ) {
            Ok(staged) => staged,
            Err(error) => {
                self.roll_back(journal);
                return Err(error);
            }
        };
        // Commit point: the provisional mirrors are now the live ones. GPU
        // growth performed while staging is not undone on failure; a larger
        // pool with the same live contents is still a valid pool.
        drop(journal);
        if let Some(gpu) = gpu {
            self.upload_staged(gpu.queue, prepared, has_dense_payload, &staged)?;
        }
        let owned_nodes = self
            .nodes_by_owner
            .get(&cluster_id)
            .cloned()
            .unwrap_or_default();
        self.installed.insert(
            cluster_id,
            InstalledCluster {
                patches: staged.patches,
                owned_nodes,
                sparse_rows: staged
                    .sparse_plan
                    .iter()
                    .map(|row| (row.section_id, row.payload.row))
                    .collect(),
                required_indirect_epoch: staged.required_indirect_epoch,
                required_direct_epoch: staged.required_direct_epoch,
            },
        );
        self.pending_promotion.insert(cluster_id);
        Ok(())
    }

    /// The fallible half of an install: allocate addresses, rewrite compose
    /// words, mark rows, grow GPU capacity, and validate every upload without
    /// queueing it. Every CPU mirror mutation goes through `journal`; the
    /// caller rolls it back on error.
    fn stage_install(
        &mut self,
        journal: &mut InstallJournal,
        mut gpu: Option<&mut InstallGpu<'_>>,
        prepared: &PreparedShCluster,
        has_dense_payload: bool,
    ) -> Result<StagedInstall, ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
        let sparse_rows = self.collect_sparse_rows(cluster_id, &prepared.chunk)?;
        if has_dense_payload {
            self.allocate_owned_nodes(journal, gpu.as_deref_mut(), cluster_id)?;
        } else if self
            .nodes_by_owner
            .get(&cluster_id)
            .is_some_and(|nodes| !nodes.is_empty())
        {
            return Err(malformed(
                cluster_id,
                "canonical dense node owner omitted its id-34 payload",
            ));
        }
        let local_slots = if has_dense_payload {
            self.local_to_live_slots(cluster_id, &prepared.chunk)?
        } else {
            BTreeMap::new()
        };
        let patches = if has_dense_payload {
            self.install_patches(journal, cluster_id, &prepared.chunk)?
        } else {
            Vec::new()
        };
        for patch in &patches {
            let row = self.affinity_row_for_dense(patch.dense)?;
            self.journal_insert_row(journal, RowSet::IndirectDirty, row);
            self.journal_add_row_ref(journal, RowRefTable::IndirectBase, row)?;
            if self.direct_required {
                self.journal_insert_row(journal, RowSet::DirectPromotionDirty, row);
                self.journal_insert_row(journal, RowSet::DirectAnimatedDirty, row);
                self.journal_add_row_ref(journal, RowRefTable::DirectBase, row)?;
            }
        }
        let sparse_plan = self.allocate_sparse_rows(journal, cluster_id, sparse_rows)?;
        self.ensure_sparse_capacity(gpu)?;
        self.validate_staged_on_gpu(prepared, has_dense_payload, &local_slots, &sparse_plan)?;
        // Halo closures can carry the same wire blocks as their canonical
        // owner while contributing no new address. Only actual writer
        // work waits for the following compose epoch.
        let (required_indirect_epoch, required_direct_epoch) = required_compose_epochs(
            self.indirect_compose_epoch,
            self.direct_compose_epoch,
            self.direct_compose_required,
            !patches.is_empty(),
            sparse_plan.iter().map(|row| row.section_id),
        )?;
        Ok(StagedInstall {
            local_slots,
            patches,
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
        local_slots: &BTreeMap<u32, u32>,
        sparse_plan: &[SparseInstallPlan],
    ) -> Result<(), ShResidencyDrainError> {
        let Some(gpu) = self.gpu.as_ref() else {
            return Ok(());
        };
        let cluster_id = prepared.chunk.cluster_id;
        if has_dense_payload {
            let indirect = isolated_atlas(&prepared.chunk, INDIRECT_BASE_ID)
                .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;
            gpu.validate_isolated_tiles(
                gpu.base_format,
                prepared.chunk.block_bytes(indirect),
                local_slots,
            )?;
        }
        if let Some(direct) = isolated_atlas(&prepared.chunk, DIRECT_BASE_ID) {
            let format = gpu.direct_format.ok_or(malformed(
                cluster_id,
                "streamed direct pool lacks a source format",
            ))?;
            gpu.validate_isolated_tiles(format, prepared.chunk.block_bytes(direct), local_slots)?;
        }
        for row in sparse_plan {
            if row.section_id == INDIRECT_DELTA_ID {
                gpu.validate_indirect_sparse_row(row.entries.start, row.tiles.start, &row.payload)?;
            } else {
                gpu.validate_direct_sparse_rows(row.section_id, &[row.direct_upload()?])?;
            }
        }
        Ok(())
    }

    /// Queue every validated upload. Backing data lands before each compose
    /// CSR pair is published; the compose indirection follows last.
    fn upload_staged(
        &mut self,
        queue: &wgpu::Queue,
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
                queue,
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
                queue,
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
            gpu.upload_indirect_sparse_rows(queue, &indirect_rows)?;
        }
        if !direct_rows.is_empty() {
            gpu.upload_direct_sparse_rows(queue, DIRECT_DELTA_ID, &direct_rows)?;
        }
        if !animated_direct_rows.is_empty() {
            gpu.upload_direct_sparse_rows(queue, ANIMATED_DIRECT_DELTA_ID, &animated_direct_rows)?;
        }
        gpu.upload_changed_compose_words(
            queue,
            &self.compose_words,
            staged.patches.iter().map(|patch| patch.dense),
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
            // A canonical cluster may own only sparse rows. It has no dense
            // slots or probe patches to upload, but its sparse family remains
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

    fn allocate_owned_nodes(
        &mut self,
        journal: &mut InstallJournal,
        gpu: Option<&mut InstallGpu<'_>>,
        cluster_id: u32,
    ) -> Result<(), ShResidencyDrainError> {
        // Per-cluster list; cloning it keeps the node loop free to mutate the
        // allocator without borrowing the ownership index.
        let Some(nodes) = self.nodes_by_owner.get(&cluster_id).cloned() else {
            return Ok(());
        };
        for node in nodes {
            if self.node_slots.contains_key(&node) {
                continue;
            }
            let tile_count = self
                .node_layouts
                .get(&node)
                .ok_or(malformed(
                    cluster_id,
                    "canonical node lacks id-34 prefix metadata",
                ))?
                .tile_count;
            self.journal_allocate_node(journal, node, tile_count)?;
        }
        // Grow the fixed active atlas before any upload can address the new
        // ranges. `FirstFitRanges::capacity` is only an address high-water
        // and may exceed the GPU shape after fragmentation; never let that
        // turn into an out-of-bounds texture upload. The ranges above are
        // provisional: a refused growth rolls them back with the install.
        let probe_occlusion_enabled = self.probe_occlusion_enabled();
        if let Some(pools) = self.gpu.as_mut()
            && self.dense_slots.capacity > pools.shape.slots
        {
            let gpu = gpu.ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed dense growth requires device handles",
            })?;
            pools.grow_dense(
                self.dense_slots.capacity,
                gpu::DenseGrowthInputs {
                    device: gpu.device,
                    queue: gpu.queue,
                    base: &self.base_metadata,
                    sources: &self.source_metadata,
                    probe_occlusion_enabled,
                    sh: gpu.sh,
                    selection_weights: gpu.selection_weights,
                },
            )?;
        }
        Ok(())
    }

    fn probe_occlusion_enabled(&self) -> bool {
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
