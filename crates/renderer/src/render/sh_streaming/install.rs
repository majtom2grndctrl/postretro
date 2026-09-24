//! Atomic canonical-cluster installation and dense payload validation.

use super::*;

impl ShResidencyState {
    pub(super) fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
        prepared: &PreparedShCluster,
    ) -> Result<(), ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
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
        // visibility commit below. Keep deterministic allocator/word mirrors
        // rollbackable so a malformed sparse family cannot strand an address
        // or make a half cluster reachable on a later drain.
        let previous_node_slots = self.node_slots.clone();
        let previous_dense_slots = self.dense_slots.clone();
        let previous_compose_words = self.compose_words.clone();
        let previous_sparse_pools = self.sparse_pools.clone();
        let previous_dirty_rows = self.dirty_rows.clone();
        let previous_indirect_dirty_rows = self.indirect_dirty_rows.clone();
        let previous_indirect_resident_rows = self.indirect_resident_rows.clone();
        let previous_indirect_base_row_refs = self.indirect_base_row_refs.clone();
        let previous_indirect_delta_row_refs = self.indirect_delta_row_refs.clone();
        let previous_direct_promotion_dirty_rows = self.direct_promotion_dirty_rows.clone();
        let previous_direct_animated_dirty_rows = self.direct_animated_dirty_rows.clone();
        let previous_direct_promotion_resident_rows = self.direct_promotion_resident_rows.clone();
        let previous_direct_animated_resident_rows = self.direct_animated_resident_rows.clone();
        let previous_direct_base_row_refs = self.direct_base_row_refs.clone();
        let previous_direct_promotion_row_refs = self.direct_promotion_row_refs.clone();
        let previous_direct_animated_row_refs = self.direct_animated_row_refs.clone();
        let result = (|| {
            let sparse_rows = self.collect_sparse_rows(cluster_id, &prepared.chunk)?;
            if has_dense_payload {
                self.allocate_owned_nodes(device, queue, sh, selection_weights, cluster_id)?;
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
                self.install_patches(cluster_id, &prepared.chunk)?
            } else {
                Vec::new()
            };
            for patch in &patches {
                self.mark_indirect_dirty_for_dense(patch.dense)?;
                if self.direct_required {
                    self.mark_direct_dirty_for_dense(patch.dense)?;
                }
            }
            for patch in &patches {
                self.add_indirect_base_row_ref(patch.dense)?;
            }
            self.refresh_indirect_resident_rows();
            if self.direct_required {
                for patch in &patches {
                    self.add_direct_base_row_ref(patch.dense)?;
                }
                self.refresh_direct_resident_rows();
            }
            let sparse_plan = self.allocate_sparse_rows(cluster_id, sparse_rows)?;
            self.ensure_sparse_capacity(
                device,
                queue,
                sh,
                uniform_bind_group_layout,
                selection_weights,
            )?;
            // Do the complete no-queue GPU validation after CPU allocation
            // but before any backing bytes or CSR pairs are written.
            if let Some(gpu) = self.gpu.as_ref() {
                if has_dense_payload {
                    let indirect = prepared
                        .chunk
                        .blocks
                        .iter()
                        .find(|block| {
                            block.kind == ISOLATED_ATLAS_BLOCK
                                && block.section_id == INDIRECT_BASE_ID
                        })
                        .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;
                    gpu.validate_isolated_tiles(
                        gpu.base_format,
                        prepared.chunk.block_bytes(indirect),
                        &local_slots,
                    )?;
                }
                if let Some(direct) = prepared.chunk.blocks.iter().find(|block| {
                    block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == DIRECT_BASE_ID
                }) {
                    let format = gpu.direct_format.ok_or(malformed(
                        cluster_id,
                        "streamed direct pool lacks a source format",
                    ))?;
                    gpu.validate_isolated_tiles(
                        format,
                        prepared.chunk.block_bytes(direct),
                        &local_slots,
                    )?;
                }
                for row in &sparse_plan {
                    if row.section_id == INDIRECT_DELTA_ID {
                        gpu.validate_indirect_sparse_row(
                            row.entries.start,
                            row.tiles.start,
                            &row.payload,
                        )?;
                    } else {
                        gpu.validate_direct_sparse_rows(row.section_id, &[row.direct_upload()?])?;
                    }
                }
            }
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
            Ok((
                local_slots,
                patches,
                sparse_plan,
                required_indirect_epoch,
                required_direct_epoch,
            ))
        })();
        let (local_slots, patches, sparse_plan, required_indirect_epoch, required_direct_epoch) =
            match result {
                Ok(result) => result,
                Err(error) => {
                    self.node_slots = previous_node_slots;
                    self.dense_slots = previous_dense_slots;
                    self.compose_words = previous_compose_words;
                    self.sparse_pools = previous_sparse_pools;
                    self.dirty_rows = previous_dirty_rows;
                    self.indirect_dirty_rows = previous_indirect_dirty_rows;
                    self.indirect_resident_rows = previous_indirect_resident_rows;
                    self.indirect_base_row_refs = previous_indirect_base_row_refs;
                    self.indirect_delta_row_refs = previous_indirect_delta_row_refs;
                    self.direct_promotion_dirty_rows = previous_direct_promotion_dirty_rows;
                    self.direct_animated_dirty_rows = previous_direct_animated_dirty_rows;
                    self.direct_promotion_resident_rows = previous_direct_promotion_resident_rows;
                    self.direct_animated_resident_rows = previous_direct_animated_resident_rows;
                    self.direct_base_row_refs = previous_direct_base_row_refs;
                    self.direct_promotion_row_refs = previous_direct_promotion_row_refs;
                    self.direct_animated_row_refs = previous_direct_animated_row_refs;
                    return Err(error);
                }
            };
        if let Some(gpu) = self.gpu.as_mut() {
            if has_dense_payload {
                let indirect = prepared
                    .chunk
                    .blocks
                    .iter()
                    .find(|block| {
                        block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == INDIRECT_BASE_ID
                    })
                    .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;
                gpu.upload_isolated_tiles(
                    queue,
                    &gpu.base,
                    gpu.base_format,
                    prepared.chunk.block_bytes(indirect),
                    &local_slots,
                )?;
            }
            if let Some(direct) = prepared.chunk.blocks.iter().find(|block| {
                block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == DIRECT_BASE_ID
            }) {
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
                    &local_slots,
                )?;
            }
            // All sparse rows were parsed, ownership-checked, and allocated
            // before this point. Upload backing data before each compose CSR
            // pair is published; the compose indirection follows last.
            let mut indirect_rows = Vec::new();
            let mut direct_rows = Vec::new();
            let mut animated_direct_rows = Vec::new();
            for row in &sparse_plan {
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
                gpu.upload_direct_sparse_rows(
                    queue,
                    ANIMATED_DIRECT_DELTA_ID,
                    &animated_direct_rows,
                )?;
            }
            gpu.upload_changed_compose_words(
                queue,
                &self.compose_words,
                patches.iter().map(|patch| patch.dense),
            )?;
        }
        let owned_nodes = self
            .nodes_by_owner
            .get(&cluster_id)
            .cloned()
            .unwrap_or_default();
        self.installed.insert(
            cluster_id,
            InstalledCluster {
                patches,
                owned_nodes,
                sparse_rows: sparse_plan
                    .iter()
                    .map(|row| (row.section_id, row.payload.row))
                    .collect(),
                required_indirect_epoch,
                required_direct_epoch,
            },
        );
        self.pending_promotion.insert(cluster_id);
        Ok(())
    }

    pub(super) fn validate_isolated_atlases(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<bool, ShResidencyDrainError> {
        let indirect = chunk.blocks.iter().find(|block| {
            block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == INDIRECT_BASE_ID
        });
        let direct_block = chunk
            .blocks
            .iter()
            .find(|block| block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == DIRECT_BASE_ID);
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
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        selection_weights: &wgpu::Buffer,
        cluster_id: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(nodes) = self.nodes_by_owner.get(&cluster_id).cloned() else {
            return Ok(());
        };
        // Reserve against the fixed active atlas before publishing any live
        // slot. `FirstFitRanges::capacity` is only an address high-water and
        // may exceed the GPU shape after fragmentation; never let that turn
        // into an out-of-bounds texture upload. A future append-preserving
        // replacement satisfies this same preflight before it swaps pools.
        let mut projected = self.dense_slots.clone();
        for node in &nodes {
            if self.node_slots.contains_key(node) {
                continue;
            }
            let layout = self.node_layouts.get(node).ok_or(malformed(
                cluster_id,
                "canonical node lacks id-34 prefix metadata",
            ))?;
            projected.allocate(layout.tile_count)?;
        }
        let probe_occlusion_enabled = self.probe_occlusion_enabled();
        if let Some(gpu) = self.gpu.as_mut()
            && projected.capacity > gpu.shape.slots
        {
            gpu.grow_dense(
                projected.capacity,
                gpu::DenseGrowthInputs {
                    device,
                    queue,
                    base: &self.base_metadata,
                    sources: &self.source_metadata,
                    probe_occlusion_enabled,
                    sh,
                    selection_weights,
                },
            )?;
        }
        for node in nodes {
            if self.node_slots.contains_key(&node) {
                continue;
            }
            let layout = self.node_layouts.get(&node).ok_or(malformed(
                cluster_id,
                "canonical node lacks id-34 prefix metadata",
            ))?;
            let range = self.dense_slots.allocate(layout.tile_count)?;
            self.node_slots.insert(node, range);
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
