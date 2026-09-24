//! Transactional per-family growth and retirement.
//!
//! Dense id-34/id-35 atlases and each sparse CSR family own independent
//! active-plus-one-retiring lifetimes. This module performs all fallible
//! preflight before queue submission, then commits replacement handles
//! atomically.

use super::*;

impl StreamingGpuPools {
    pub(in crate::render::sh_streaming) fn grow_dense(
        &mut self,
        required_slots: u32,
        inputs: DenseGrowthInputs<'_>,
    ) -> Result<(), ShResidencyDrainError> {
        let DenseGrowthInputs {
            device,
            queue,
            base,
            sources,
            probe_occlusion_enabled,
            sh,
            selection_weights,
        } = inputs;
        if required_slots <= self.shape.slots {
            return Ok(());
        }
        let shape = self
            .shape
            .grown_for_slots(required_slots, &device.limits())?;
        if self.retiring_dense.is_some() {
            return Err(ShResidencyDrainError::GpuRetirementPressure {
                family: "dense atlas",
            });
        }
        let current_indirect_active = self.indirect_compose.active_pool_bytes();
        let current_direct_active = self
            .direct_compose
            .as_ref()
            .map_or(0, StreamingDirectCompose::active_capacity_bytes);
        let replacement_active_capacity =
            self.active_capacity_for(shape, current_indirect_active, current_direct_active)?;
        let retiring_dense_capacity = self
            .active_capacity_bytes
            .checked_sub(current_indirect_active)
            .and_then(|bytes| bytes.checked_sub(current_direct_active))
            .and_then(|bytes| bytes.checked_add(self.grid_info.size()))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let replacement = DenseTextures::new(device, queue, base, sources, shape, sh)?;
        let replacement_grid_info = create_grid_info(device, base, shape, probe_occlusion_enabled);
        let (replacement_bind_group, replacement_mesh_bind_group) = create_sample_bind_groups(
            device,
            sh,
            &replacement,
            &replacement_grid_info,
            &self.depth_moments,
        );
        // Validate every fallible direct view requirement before either
        // compose carrier is rebound to the replacement atlas.
        let direct_views = self
            .direct_compose
            .as_ref()
            .map(|_| {
                if self.has_animated_direct_pass
                    && (replacement.direct_intermediate_storage_view.is_none()
                        || replacement.direct_intermediate_sampled_view.is_none())
                {
                    return Err(ShResidencyDrainError::GpuCapacity {
                        reason: "streamed dense growth lost its animated direct intermediate atlas",
                    });
                }
                Ok(StreamingDirectViews {
                    base: replacement.direct_base_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed dense growth lost its id-35 base atlas",
                        },
                    )?,
                    intermediate_storage: replacement.direct_intermediate_storage_view.as_ref(),
                    intermediate_sampled: replacement.direct_intermediate_sampled_view.as_ref(),
                    total_storage: replacement.direct_total_storage_view.as_ref().ok_or(
                        ShResidencyDrainError::GpuCapacity {
                            reason: "streamed dense growth lost its direct compose output atlas",
                        },
                    )?,
                    selection_weights,
                })
            })
            .transpose()?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Dense Atlas Growth Copy"),
        });
        let old_extent = self.shape.extent();
        copy_texture(&mut encoder, &self.base, &replacement.base, old_extent);
        copy_texture(&mut encoder, &self.total, &replacement.total, old_extent);
        if let (Some(source), Some(destination)) = (&self.direct_base, &replacement.direct_base) {
            copy_texture(&mut encoder, source, destination, old_extent);
        }
        if let (Some(source), Some(destination)) =
            (&self.direct_intermediate, &replacement.direct_intermediate)
        {
            copy_texture(&mut encoder, source, destination, old_extent);
        }
        if let (Some(source), Some(destination)) = (&self.direct_total, &replacement.direct_total) {
            copy_texture(&mut encoder, source, destination, old_extent);
        }

        // Submitted command buffers retain their bindings. Drop the displaced
        // bind groups after rewiring: putting compose groups in the dense
        // retirement ticket would cross-pin independently grown sparse pools.
        self.indirect_compose.rebind_dense(
            device,
            shape,
            &replacement.base_view,
            &replacement.total_storage_view,
            &self.compose_indirection,
            sh,
        );
        if let (Some(direct), Some(views)) = (self.direct_compose.as_mut(), direct_views) {
            direct.rebind_dense(device, shape, views, &self.compose_indirection, sh);
        }

        let old = DenseTextures {
            base_format: std::mem::replace(&mut self.base_format, replacement.base_format),
            direct_format: std::mem::replace(&mut self.direct_format, replacement.direct_format),
            base: std::mem::replace(&mut self.base, replacement.base),
            total: std::mem::replace(&mut self.total, replacement.total),
            base_view: std::mem::replace(&mut self.base_view, replacement.base_view),
            total_storage_view: std::mem::replace(
                &mut self.total_storage_view,
                replacement.total_storage_view,
            ),
            total_sampled_view: std::mem::replace(
                &mut self.total_sampled_view,
                replacement.total_sampled_view,
            ),
            direct_base: std::mem::replace(&mut self.direct_base, replacement.direct_base),
            direct_base_view: std::mem::replace(
                &mut self.direct_base_view,
                replacement.direct_base_view,
            ),
            direct_intermediate: std::mem::replace(
                &mut self.direct_intermediate,
                replacement.direct_intermediate,
            ),
            direct_intermediate_storage_view: std::mem::replace(
                &mut self.direct_intermediate_storage_view,
                replacement.direct_intermediate_storage_view,
            ),
            direct_intermediate_sampled_view: std::mem::replace(
                &mut self.direct_intermediate_sampled_view,
                replacement.direct_intermediate_sampled_view,
            ),
            direct_total: std::mem::replace(&mut self.direct_total, replacement.direct_total),
            direct_total_storage_view: std::mem::replace(
                &mut self.direct_total_storage_view,
                replacement.direct_total_storage_view,
            ),
            direct_total_sampled_view: std::mem::replace(
                &mut self.direct_total_sampled_view,
                replacement.direct_total_sampled_view,
            ),
        };
        let old_grid_info = std::mem::replace(&mut self.grid_info, replacement_grid_info);
        self.bind_group = replacement_bind_group;
        self.mesh_bind_group = replacement_mesh_bind_group;
        self.shape = shape;
        self.probe_occlusion_enabled = probe_occlusion_enabled;
        self.growth
            .record(1, self.active_capacity_bytes, replacement_active_capacity);
        self.active_capacity_bytes = replacement_active_capacity;

        queue.submit(std::iter::once(encoder.finish()));
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        queue.on_submitted_work_done(move || {
            callback_complete.store(true, Ordering::Release);
        });
        self.retiring_dense = Some(RetiringDenseGeneration {
            capacity_bytes: retiring_dense_capacity,
            dense: old,
            grid_info: old_grid_info,
            complete,
        });
        Ok(())
    }

    /// Append-preservingly grow one or more sparse families.  A replacement
    /// keeps the dense slot geometry stable, copies every live GPU-visible
    /// row and indirection word, and retains the old generation through the
    /// submission fence just like dense layer growth.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::render::sh_streaming) fn grow_sparse(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        required: &SparseCapacityFloors,
        sh: &mut ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        let mut floors = self.sparse_capacity_floors.clone();
        let mut sections_to_grow = Vec::new();
        for (&section_id, &(required_entries, required_tiles)) in required {
            let current_entries = self.sparse_entry_capacity(section_id);
            let current_tiles = self.sparse_tile_f16_capacity(section_id);
            if required_entries <= current_entries && required_tiles <= current_tiles {
                continue;
            }
            let waiting_for_retirement = sparse_family_has_retiring_generation(
                section_id,
                self.retiring_indirect.is_some(),
                self.retiring_direct_promotion.is_some(),
                self.retiring_direct_animated.is_some(),
            );
            if waiting_for_retirement {
                return Err(ShResidencyDrainError::GpuRetirementPressure {
                    family: "sparse compose",
                });
            }
            let grown_entries = current_entries
                .checked_mul(2)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(required_entries);
            let grown_tiles = current_tiles
                .checked_mul(2)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
                .max(required_tiles);
            floors.insert(section_id, (grown_entries, grown_tiles));
            sections_to_grow.push(section_id);
        }
        if sections_to_grow.is_empty() {
            return Ok(());
        }
        // Construct every replacement before encoding a copy or swapping an
        // active family. A later constructor failure must leave all active
        // buffers and their CPU allocation mirrors untouched.
        let indirect_replacement = if sections_to_grow.contains(&27) {
            let floor = *floors.get(&27).ok_or(ShResidencyDrainError::SlotOverflow)?;
            Some(StreamingIndirectCompose::new(
                device,
                base,
                sources,
                self.shape,
                &self.base_view,
                &self.total_storage_view,
                &self.compose_indirection,
                sh,
                uniform_bind_group_layout,
                Some(floor),
            )?)
        } else {
            None
        };
        let mut promotion_replacement = None;
        let mut animated_replacement = None;
        if sections_to_grow.iter().any(|&section| {
            matches!(
                section,
                DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION
            )
        }) {
            let views = StreamingDirectViews {
                base: self
                    .direct_base_view
                    .as_ref()
                    .ok_or(ShResidencyDrainError::GpuCapacity {
                        reason: "streamed direct sparse growth lost its id-35 base view",
                    })?,
                intermediate_storage: self.direct_intermediate_storage_view.as_ref(),
                intermediate_sampled: self.direct_intermediate_sampled_view.as_ref(),
                total_storage: self.direct_total_storage_view.as_ref().ok_or(
                    ShResidencyDrainError::GpuCapacity {
                        reason: "streamed direct sparse growth lost its total view",
                    },
                )?,
                selection_weights,
            };
            let direct =
                self.direct_compose
                    .as_ref()
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "sparse direct growth requested without a direct compose pass",
                    })?;
            for &section_id in &sections_to_grow {
                if !matches!(
                    section_id,
                    DIRECT_DELTA_SECTION | ANIMATED_DIRECT_DELTA_SECTION
                ) {
                    continue;
                }
                let floor = *floors
                    .get(&section_id)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?;
                let replacement = direct.build_sparse_replacement(
                    device,
                    base,
                    sources,
                    self.shape,
                    StreamingDirectViews {
                        base: views.base,
                        intermediate_storage: views.intermediate_storage,
                        intermediate_sampled: views.intermediate_sampled,
                        total_storage: views.total_storage,
                        selection_weights: views.selection_weights,
                    },
                    &self.compose_indirection,
                    sh,
                    uniform_bind_group_layout,
                    section_id,
                    floor,
                )?;
                match section_id {
                    DIRECT_DELTA_SECTION => promotion_replacement = Some(replacement),
                    ANIMATED_DIRECT_DELTA_SECTION => animated_replacement = Some(replacement),
                    _ => unreachable!("only direct sparse sections reach this branch"),
                }
            }
        }

        // Compute every accounting result before a submission or active
        // binding swap. After this point the transaction consists only of
        // queue encoding, submission, and infallible replacements.
        let current_indirect_active = self.indirect_compose.active_pool_bytes();
        let replacement_indirect_active = indirect_replacement.as_ref().map_or(
            current_indirect_active,
            StreamingIndirectCompose::active_pool_bytes,
        );
        let retiring_indirect_capacity = indirect_replacement
            .as_ref()
            .map(|_| self.indirect_compose.retired_sparse_capacity_bytes())
            .transpose()?;
        let (current_direct_active, current_promotion_active, current_animated_active) =
            self.direct_compose.as_ref().map_or((0, 0, 0), |direct| {
                (
                    direct.active_capacity_bytes(),
                    direct.promotion_active_capacity_bytes(),
                    direct.animated_active_capacity_bytes(),
                )
            });
        let replacement_promotion_active = promotion_replacement.as_ref().map_or(
            current_promotion_active,
            DirectSparseReplacement::active_capacity_bytes,
        );
        let replacement_animated_active = animated_replacement.as_ref().map_or(
            current_animated_active,
            DirectSparseReplacement::active_capacity_bytes,
        );
        let replacement_direct_active = current_direct_active
            .checked_sub(current_promotion_active)
            .and_then(|bytes| bytes.checked_sub(current_animated_active))
            .and_then(|bytes| bytes.checked_add(replacement_promotion_active))
            .and_then(|bytes| bytes.checked_add(replacement_animated_active))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let replacement_active_capacity = self.active_capacity_for(
            self.shape,
            replacement_indirect_active,
            replacement_direct_active,
        )?;
        let (retiring_promotion_capacity, retiring_animated_capacity) = self
            .direct_compose
            .as_ref()
            .map_or(Ok((None, None)), |direct| {
                Ok((
                    promotion_replacement
                        .as_ref()
                        .map(|_| direct.promotion_retired_sparse_capacity_bytes())
                        .transpose()?,
                    animated_replacement
                        .as_ref()
                        .map(|_| direct.animated_retired_sparse_capacity_bytes())
                        .transpose()?,
                ))
            })?;

        // All fallible allocation/validation is complete. Encode retained
        // data into the candidates, submit once, then publish every swap as
        // one infallible CPU transaction.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Streamed SH Sparse Pool Growth Copy"),
        });
        if let Some(replacement) = indirect_replacement.as_ref() {
            self.indirect_compose
                .copy_retained_to(replacement, &mut encoder);
        }
        if promotion_replacement.is_some() || animated_replacement.is_some() {
            let direct = self.direct_compose.as_ref().expect(
                "prevalidated direct sparse replacements require a live direct compose pass",
            );
            if let Some(replacement) = promotion_replacement.as_ref() {
                direct.copy_to_sparse_replacement(replacement, &mut encoder);
            }
            if let Some(replacement) = animated_replacement.as_ref() {
                direct.copy_to_sparse_replacement(replacement, &mut encoder);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
        let complete = Arc::new(AtomicBool::new(false));
        let callback_complete = Arc::clone(&complete);
        queue.on_submitted_work_done(move || {
            callback_complete.store(true, Ordering::Release);
        });
        if let Some(replacement) = indirect_replacement {
            let compose = std::mem::replace(&mut self.indirect_compose, replacement);
            let resources = compose.into_retired_sparse_resources();
            let capacity_bytes = retiring_indirect_capacity
                .expect("indirect replacement has a precomputed retirement capacity");
            debug_assert_eq!(resources.capacity_bytes(), capacity_bytes);
            self.retiring_indirect = Some(RetiringIndirectGeneration {
                capacity_bytes,
                resources,
                complete: Arc::clone(&complete),
            });
        }
        if let Some(replacement) = promotion_replacement {
            let pass = self
                .direct_compose
                .as_mut()
                .expect("prevalidated id-41 replacement requires direct compose")
                .commit_sparse_replacement(replacement);
            let capacity_bytes = retiring_promotion_capacity
                .expect("id-41 replacement has a precomputed retirement capacity");
            debug_assert_eq!(pass.capacity_bytes(), capacity_bytes);
            self.retiring_direct_promotion = Some(RetiringDirectSparseGeneration {
                capacity_bytes,
                pass,
                complete: Arc::clone(&complete),
            });
        }
        if let Some(replacement) = animated_replacement {
            let pass = self
                .direct_compose
                .as_mut()
                .expect("prevalidated id-45 replacement requires direct compose")
                .commit_sparse_replacement(replacement);
            let capacity_bytes = retiring_animated_capacity
                .expect("id-45 replacement has a precomputed retirement capacity");
            debug_assert_eq!(pass.capacity_bytes(), capacity_bytes);
            self.retiring_direct_animated = Some(RetiringDirectSparseGeneration {
                capacity_bytes,
                pass,
                complete,
            });
        }
        self.sparse_capacity_floors = floors;
        self.growth.record(
            sections_to_grow.len(),
            self.active_capacity_bytes,
            replacement_active_capacity,
        );
        self.active_capacity_bytes = replacement_active_capacity;
        Ok(())
    }

    pub(in crate::render::sh_streaming) fn release_completed_retirement(&mut self) {
        if self
            .retiring_dense
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_dense = None;
        }
        if self
            .retiring_indirect
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_indirect = None;
        }
        if self
            .retiring_direct_promotion
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_direct_promotion = None;
        }
        if self
            .retiring_direct_animated
            .as_ref()
            .is_some_and(|retiring| retiring.complete.load(Ordering::Acquire))
        {
            self.retiring_direct_animated = None;
        }
    }

    pub(in crate::render::sh_streaming) fn retiring_capacity_bytes(&self) -> u64 {
        [
            self.retiring_dense
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_indirect
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_direct_promotion
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
            self.retiring_direct_animated
                .as_ref()
                .map(|retiring| retiring.capacity_bytes),
        ]
        .into_iter()
        .flatten()
        .try_fold(0u64, u64::checked_add)
        .expect("validated retired streamed pool capacities must not overflow")
    }

    fn active_capacity_for(
        &self,
        shape: AtlasShape,
        indirect_active: u64,
        direct_active: u64,
    ) -> Result<u64, ShResidencyDrainError> {
        let extent = shape.extent();
        let mut bytes = texture_bytes(self.base_format, extent)
            .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if let Some(format) = self.direct_format {
            bytes = bytes
                .checked_add(texture_bytes(format, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        if self.direct_total.is_some() {
            bytes = bytes
                .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        if self.direct_intermediate.is_some() {
            bytes = bytes
                .checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Float, extent))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        bytes = bytes
            .checked_add(indirect_active)
            .and_then(|bytes| bytes.checked_add(direct_active))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(bytes)
    }

    pub(in crate::render::sh_streaming) const fn probe_occlusion_enabled(&self) -> bool {
        self.probe_occlusion_enabled
    }

    pub(in crate::render::sh_streaming) const fn effective_floor_bytes(&self) -> u64 {
        self.effective_floor_bytes
    }

    pub(in crate::render::sh_streaming) fn depth_moment_view(&self) -> wgpu::TextureView {
        self.depth_moments
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("Streamed SH Depth Moment Shadow View"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            })
    }
}
