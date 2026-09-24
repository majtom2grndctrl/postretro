//! Streamed-residency construction and stable renderer bindings.

use super::*;

impl ShResidencyState {
    pub(in crate::render) fn from_manifest(
        manifest: &ShStreamManifest,
    ) -> Result<Self, ShResidencyDrainError> {
        Self::from_parts(
            manifest.content_tag(),
            manifest.cluster_directory(),
            manifest.base(),
            manifest.sources(),
        )
    }

    pub(in crate::render) fn from_parts(
        content_tag: [u8; 32],
        directory: &ClusterDirectorySection,
        base: &postretro_level_loader::ShStreamBaseMetadata,
        sources: &postretro_level_loader::ShStreamSourceMetadata,
    ) -> Result<Self, ShResidencyDrainError> {
        let cluster_count = u32::try_from(directory.clusters.len()).map_err(|_| {
            ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "cluster count exceeds u32",
            }
        })?;
        let dense_resource = directory
            .resources
            .iter()
            .position(|resource| {
                resource.section_id == INDIRECT_BASE_ID
                    && resource.domain == ClusterResourceDomain::DenseProbe
            })
            .ok_or(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "id-49 has no id-34 dense resource",
            })?;
        let mut dense_owner = vec![None; base.probes.len()];
        let mut sparse_row_owner = BTreeMap::new();
        for cluster_id in 0..cluster_count {
            let cluster = &directory.clusters[cluster_id as usize];
            let start = usize::try_from(cluster.range_start).map_err(|_| {
                ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range start exceeds usize",
                }
            })?;
            let end = start
                .checked_add(usize::try_from(cluster.range_count).map_err(|_| {
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id,
                        reason: "cluster range count exceeds usize",
                    }
                })?)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range end overflows",
                })?;
            for range in
                directory
                    .ranges
                    .get(start..end)
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id,
                        reason: "cluster range exceeds id-49",
                    })?
            {
                let resource = directory
                    .resources
                    .get(range.resource_index as usize)
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id,
                        reason: "range resource index exceeds id-49",
                    })?;
                let range_end = range.start.checked_add(range.count).ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id,
                        reason: "range end overflows",
                    },
                )?;
                match resource.domain {
                    ClusterResourceDomain::DenseProbe
                        if range.resource_index as usize == dense_resource =>
                    {
                        for dense in range.start..range_end {
                            let Some(owner) = dense_owner.get_mut(dense as usize) else {
                                return Err(ShResidencyDrainError::MalformedChunk {
                                    cluster_id,
                                    reason: "dense range exceeds id-34 metadata",
                                });
                            };
                            let prior: Option<u32> = *owner;
                            *owner = Some(prior.map_or(cluster_id, |prior| prior.min(cluster_id)));
                        }
                    }
                    ClusterResourceDomain::AffinityCell
                        if matches!(
                            resource.section_id,
                            INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
                        ) =>
                    {
                        for row in range.start..range_end {
                            let owner = range.owner_cluster_id;
                            let key = (resource.section_id, row);
                            match sparse_row_owner.get(&key) {
                                Some(&prior) if prior != owner => {
                                    return Err(ShResidencyDrainError::MalformedChunk {
                                        cluster_id,
                                        reason: "sparse row has conflicting owners",
                                    });
                                }
                                _ => {
                                    sparse_row_owner.insert(key, owner);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let sparse_capacity_floors = sparse_capacity_floors(sources, &sparse_row_owner)?;
        let dense_layout = derive_dense_node_layout(base)?;
        let dense_node = dense_layout.nodes;
        let mut node_owner = BTreeMap::<StoredNode, u32>::new();
        for (dense_index, node) in dense_node.iter().copied().enumerate() {
            if let Some(node) = node {
                let owner =
                    dense_owner[dense_index].ok_or(ShResidencyDrainError::MissingDenseOwner {
                        cluster_id: 0,
                        dense_index: dense_index as u32,
                    })?;
                node_owner
                    .entry(node)
                    .and_modify(|prior| *prior = (*prior).min(owner))
                    .or_insert(owner);
            }
        }

        let mut nodes_by_owner = BTreeMap::<u32, Vec<StoredNode>>::new();
        for (&node, &owner) in &node_owner {
            nodes_by_owner.entry(owner).or_default().push(node);
        }

        let mut owner_dependencies = vec![BTreeSet::new(); cluster_count as usize];
        for cluster_id in 0..cluster_count {
            let cluster = &directory.clusters[cluster_id as usize];
            let start = cluster.range_start as usize;
            let end = start.checked_add(cluster.range_count as usize).ok_or(
                ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range end overflows",
                },
            )?;
            for range in
                directory
                    .ranges
                    .get(start..end)
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id,
                        reason: "cluster range exceeds id-49",
                    })?
            {
                let resource = &directory.resources[range.resource_index as usize];
                let range_end = range.start + range.count;
                if resource.section_id == INDIRECT_BASE_ID {
                    for dense in range.start..range_end {
                        // A chunk can carry the dense writer and its stored
                        // node closure from different canonical clusters. Both
                        // must already be sampleable: the writer owns the
                        // indirection word while the node owner owns the tile
                        // bytes that word addresses.
                        if let Some(Some(owner)) = dense_owner.get(dense as usize)
                            && *owner != cluster_id
                        {
                            owner_dependencies[cluster_id as usize].insert(*owner);
                        }
                        if let Some(Some(node)) = dense_node.get(dense as usize)
                            && let Some(&owner) = node_owner.get(node)
                            && owner != cluster_id
                        {
                            owner_dependencies[cluster_id as usize].insert(owner);
                        }
                    }
                } else if matches!(
                    resource.section_id,
                    INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
                ) && range.role == ClusterRangeRole::Halo
                    && range.owner_cluster_id != cluster_id
                {
                    owner_dependencies[cluster_id as usize].insert(range.owner_cluster_id);
                }
            }
        }

        let mut sparse_pools = BTreeMap::new();
        for (section_id, metadata) in [
            (INDIRECT_DELTA_ID, sources.indirect_delta.as_ref()),
            (DIRECT_DELTA_ID, sources.direct_delta.as_ref()),
            (
                ANIMATED_DIRECT_DELTA_ID,
                sources.animated_direct_delta.as_ref(),
            ),
        ] {
            if let Some(metadata) = metadata {
                sparse_pools.insert(
                    section_id,
                    SparsePool::new(metadata.affinity_offsets.len().saturating_sub(1)),
                );
            }
        }

        Ok(Self {
            content_tag,
            base_metadata: base.clone(),
            source_metadata: sources.clone(),
            cluster_count,
            grid_dimensions: base.grid_dimensions,
            generation: 0,
            targets: BTreeSet::new(),
            dense_owner,
            dense_node,
            dense_node_local_slot: dense_layout.local_slots,
            node_layouts: dense_layout.layouts,
            node_owner,
            nodes_by_owner,
            node_slots: BTreeMap::new(),
            owner_dependencies,
            sparse_row_owner,
            sparse_capacity_floors,
            dense_slots: FirstFitRanges::default(),
            sparse_pools,
            compose_words: vec![0; base.probes.len()],
            sampled_words: vec![0; base.probes.len()],
            installed: BTreeMap::new(),
            sampleable: BTreeSet::new(),
            pending_promotion: BTreeSet::new(),
            dirty_rows: BTreeSet::new(),
            indirect_dirty_rows: BTreeSet::new(),
            indirect_resident_rows: BTreeSet::new(),
            indirect_base_row_refs: BTreeMap::new(),
            indirect_delta_row_refs: BTreeMap::new(),
            direct_promotion_dirty_rows: BTreeSet::new(),
            direct_animated_dirty_rows: BTreeSet::new(),
            direct_promotion_resident_rows: BTreeSet::new(),
            direct_animated_resident_rows: BTreeSet::new(),
            direct_base_row_refs: BTreeMap::new(),
            direct_promotion_row_refs: BTreeMap::new(),
            direct_animated_row_refs: BTreeMap::new(),
            direct_required: sources.direct.is_some(),
            direct_compose_required: sources.direct_delta.is_some()
                || sources.animated_direct_delta.is_some(),
            direct_animation_descriptor_indices: sources
                .animated_direct_delta
                .as_ref()
                .map_or_else(Vec::new, |source| {
                    source.animation_descriptor_indices.clone()
                }),
            indirect_was_active: false,
            last_indirect_mask: LightTermMask::ALL,
            direct_was_active: false,
            last_direct_mask: LightTermMask::ALL,
            generation_has_reset: false,
            indirect_compose_epoch: 0,
            direct_compose_epoch: 0,
            install_cpu: InstallCpuCounters::default(),
            gpu: None,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "GPU initialization keeps explicit GPU resources and streaming metadata at the renderer boundary."
    )]
    pub(in crate::render) fn initialize_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        manifest: &ShStreamManifest,
        probe_occlusion_enabled: bool,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        let mut minimum_slots = 1u32;
        for nodes in self.nodes_by_owner.values() {
            let slots = nodes.iter().try_fold(0u32, |total, node| {
                total
                    .checked_add(
                        self.node_layouts
                            .get(node)
                            .ok_or(ShResidencyDrainError::SlotOverflow)?
                            .tile_count,
                    )
                    .ok_or(ShResidencyDrainError::SlotOverflow)
            })?;
            minimum_slots = minimum_slots.max(slots);
        }
        let whole_dense_slots = self.node_layouts.values().try_fold(0u32, |total, layout| {
            let end = layout
                .global_base_slot
                .checked_add(layout.tile_count)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            Ok(total.max(end))
        })?;
        let fixed_metadata_bytes = gpu::initial_fixed_metadata_bytes(
            manifest.base(),
            manifest.sources(),
            &device.limits(),
            sh,
        )?;
        let initial_floor = plan_initial_pool_floor(
            manifest.base(),
            manifest.sources(),
            minimum_slots,
            whole_dense_slots,
            &self.sparse_capacity_floors,
            fixed_metadata_bytes,
            sh.billboard_direct_scatter.capacity_bytes(),
            &device.limits(),
        )?;
        self.gpu = Some(StreamingGpuPools::new(
            device,
            queue,
            manifest.base(),
            manifest.sources(),
            initial_floor,
            probe_occlusion_enabled,
            sh,
            uniform_bind_group_layout,
            selection_weights,
        )?);
        Ok(())
    }

    pub(in crate::render) fn bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.gpu.as_ref().map(|gpu| &gpu.bind_group)
    }

    pub(in crate::render) fn mesh_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.gpu.as_ref().map(|gpu| &gpu.mesh_bind_group)
    }

    pub(in crate::render) fn depth_moment_view(&self) -> Option<wgpu::TextureView> {
        self.gpu.as_ref().map(StreamingGpuPools::depth_moment_view)
    }

    pub(in crate::render) fn indirect_has_active_animation(
        &self,
        animation: &crate::render::sh_volume::AnimatedLightBuffers,
    ) -> bool {
        self.gpu
            .as_ref()
            .is_some_and(|gpu| gpu.indirect_has_active_animation(animation))
    }

    pub(in crate::render) fn direct_has_active_animation(
        &self,
        animation: &crate::render::sh_volume::AnimatedLightBuffers,
    ) -> bool {
        animation.any_active_for_descriptor_indices(&self.direct_animation_descriptor_indices)
    }
}
