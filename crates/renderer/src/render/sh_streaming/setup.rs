//! Streamed-residency construction and stable renderer bindings.

use super::*;

impl ShResidencyState {
    /// `cells` are the level's runtime cells; the sampled-row gate indexes
    /// their bounds here once, and frames name cells by id.
    #[cfg(test)]
    pub(in crate::render) fn from_manifest(
        manifest: &ShStreamManifest,
        cells: &[postretro_level_loader::CellData],
    ) -> Result<Self, ShResidencyDrainError> {
        Ok(Self::from_plan(
            plan_sh_residency(manifest, cells)?,
            manifest.base(),
            manifest.sources(),
        ))
    }

    #[cfg(test)]
    pub(in crate::render) fn from_parts(
        content_tag: [u8; 32],
        directory: &ClusterDirectorySection,
        base: &postretro_level_loader::ShStreamBaseMetadata,
        sources: &postretro_level_loader::ShStreamSourceMetadata,
        cells: &[postretro_level_loader::CellData],
    ) -> Result<Self, ShResidencyDrainError> {
        Ok(Self::from_plan(
            plan_sh_residency_parts(content_tag, directory, base, sources, cells)?,
            base,
            sources,
        ))
    }

    /// The residency state over a plan prep built from this manifest's
    /// `base` and `sources`. Cannot fail: every level-data check ran in the
    /// plan.
    pub(in crate::render) fn from_plan(
        plan: ShResidencyPlan,
        base: &postretro_level_loader::ShStreamBaseMetadata,
        sources: &postretro_level_loader::ShStreamSourceMetadata,
    ) -> Self {
        let ShResidencyPlan {
            content_tag,
            cluster_count,
            dense_node,
            dense_node_local_slot,
            node_layouts,
            dense_spans,
            sparse_capacity_floors,
            sparse_pool_rows,
            row_capacity,
            sample_regions,
            sparse_compose,
            initial_dense_slots,
        } = plan;
        let sparse_pools = sparse_pool_rows
            .into_iter()
            .map(|(section_id, rows)| (section_id, SparsePool::new(rows)))
            .collect();
        Self {
            content_tag,
            base_metadata: base.clone(),
            source_metadata: sources.clone(),
            cluster_count,
            grid_dimensions: base.grid_dimensions,
            generation: 0,
            #[cfg(feature = "dev-tools")]
            probe_readback: Default::default(),
            targets: BTreeSet::new(),
            compose_words: vec![0; base.probes.len()],
            sampled_words: vec![0; base.probes.len()],
            dense_node,
            dense_node_local_slot,
            node_layouts,
            dense_spans,
            node_slots: BTreeMap::new(),
            storage_refs: StorageRefs::with_probe_count(base.probes.len()),
            sparse_capacity_floors,
            dense_slots: FirstFitRanges::default(),
            sparse_pools,
            installed: BTreeMap::new(),
            sampleable: BTreeSet::new(),
            pending_promotion: BTreeSet::new(),
            dirty_rows: BTreeSet::new(),
            indirect_dirty_rows: BTreeSet::new(),
            indirect_resident_rows: RowBitSet::with_row_capacity(row_capacity),
            indirect_base_row_refs: BTreeMap::new(),
            indirect_delta_row_refs: BTreeMap::new(),
            direct_promotion_dirty_rows: BTreeSet::new(),
            direct_animated_dirty_rows: BTreeSet::new(),
            direct_promotion_resident_rows: RowBitSet::with_row_capacity(row_capacity),
            direct_animated_resident_rows: RowBitSet::with_row_capacity(row_capacity),
            direct_base_row_refs: BTreeMap::new(),
            direct_promotion_row_refs: BTreeMap::new(),
            direct_animated_row_refs: BTreeMap::new(),
            direct_required: sources.direct.is_some(),
            direct_compose_required: sources.direct_delta.is_some()
                || sources.animated_direct_delta.is_some(),
            animated_direct_compose_required: sources.animated_direct_delta.is_some(),
            direct_animation_descriptor_indices: sources
                .animated_direct_delta
                .as_ref()
                .map_or_else(Vec::new, |source| {
                    source.animation_descriptor_indices.clone()
                }),
            compose_planner: StreamedComposePlanner::with_row_capacity(row_capacity),
            compose_frame_plan: None,
            sample_region_index: SampleRegionIndex::from_tables(sample_regions),
            compose_input_regions: Vec::new(),
            compose_region_rows: Vec::new(),
            compose_residency_rows: Vec::new(),
            compose_membership_touched: RowQueue::with_row_capacity(row_capacity),
            compose_membership_changes: Vec::new(),
            compose_animated_weights: Vec::new(),
            indirect_compose_diagnostics: ShComposePassDiagnostics::default(),
            static_direct_compose_diagnostics: ShComposePassDiagnostics::default(),
            animated_direct_compose_diagnostics: ShComposePassDiagnostics::default(),
            compose_planning_cpu_micros: 0,
            generation_has_reset: false,
            indirect_compose_epoch: 0,
            direct_compose_epoch: 0,
            install_cpu: InstallCpuCounters::default(),
            initial_dense_slots,
            sparse_compose: std::sync::Arc::new(sparse_compose),
            gpu: None,
        }
    }

    /// Every field the residency plan seeded, formatted in one fixed order,
    /// for the prep-equivalence tests and the set summary: equal to
    /// `residency_plan_summary` of the plan the state was built from.
    #[cfg(any(test, feature = "test-seams"))]
    pub(in crate::render) fn plan_summary(&self) -> String {
        let sparse_pool_rows: BTreeMap<u32, usize> = self
            .sparse_pools
            .iter()
            .map(|(&section, pool)| (section, pool.row_pairs.len()))
            .collect();
        format!(
            "{:?}",
            (
                (
                    self.content_tag,
                    self.cluster_count,
                    &self.dense_node,
                    &self.dense_node_local_slot,
                    &self.node_layouts,
                    &self.dense_spans,
                ),
                (
                    &self.sparse_capacity_floors,
                    sparse_pool_rows,
                    // The planner's row space, as the row sets it sized.
                    self.indirect_resident_rows.word_count(),
                    self.sample_region_index.tables(),
                    &*self.sparse_compose,
                    self.initial_dense_slots,
                ),
            )
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "GPU initialization keeps explicit GPU resources and streaming metadata at the renderer boundary."
    )]
    pub(in crate::render) fn initialize_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        manifest: &ShStreamManifest,
        probe_occlusion_enabled: bool,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        pipelines: &StreamingShPipelines,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        let limits = device.limits();
        self.initialize_gpu_with_limits(
            device,
            queue,
            manifest.base(),
            manifest.sources(),
            probe_occlusion_enabled,
            sh,
            pipelines,
            selection_weights,
            &limits,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn initialize_gpu_with_limits(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        base: &postretro_level_loader::ShStreamBaseMetadata,
        sources: &postretro_level_loader::ShStreamSourceMetadata,
        probe_occlusion_enabled: bool,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        pipelines: &StreamingShPipelines,
        selection_weights: &wgpu::Buffer,
        limits: &wgpu::Limits,
        extra_floor_charge: u64,
    ) -> Result<(), ShResidencyDrainError> {
        // The plan computed the dense slot minima; only the floor below reads
        // live buffer sizes.
        let InitialDenseSlots {
            minimum: minimum_slots,
            whole: whole_dense_slots,
        } = self.initial_dense_slots;
        let fixed_metadata_bytes = gpu::initial_fixed_metadata_bytes(base, sources, limits, sh)?;
        let initial_floor = plan_initial_pool_floor(
            base,
            sources,
            minimum_slots,
            whole_dense_slots,
            &self.sparse_capacity_floors,
            fixed_metadata_bytes
                .checked_add(extra_floor_charge)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
            sh.billboard_direct_scatter.capacity_bytes(),
            limits,
        )?;
        self.gpu = Some(StreamingGpuPools::new(
            device,
            limits,
            queue,
            base,
            sources,
            std::sync::Arc::clone(&self.sparse_compose),
            initial_floor,
            probe_occlusion_enabled,
            sh,
            pipelines,
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

/// [`ShResidencyState::plan_summary`]'s fields, read from a plan.
#[cfg(test)]
pub(in crate::render) fn residency_plan_summary(plan: &ShResidencyPlan) -> String {
    format!(
        "{:?}",
        (
            (
                plan.content_tag,
                plan.cluster_count,
                &plan.dense_node,
                &plan.dense_node_local_slot,
                &plan.node_layouts,
                &plan.dense_spans,
            ),
            (
                &plan.sparse_capacity_floors,
                &plan.sparse_pool_rows,
                plan.row_capacity.div_ceil(64),
                &plan.sample_regions,
                &plan.sparse_compose,
                plan.initial_dense_slots,
            ),
        )
    )
}
