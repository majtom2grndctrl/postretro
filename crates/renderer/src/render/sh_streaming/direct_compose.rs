//! Streamed direct-SH composition over the shared 8×8 dense-slot pool.
//!
//! The residency owner remains the sole allocator. This facade owns only the
//! direct compose pipelines and routes allocator-assigned id-41/id-45 rows to
//! their matching existing-binding sparse pools.

mod layout;
mod passes;
mod sparse;

use postretro_level_loader::{ShStreamBaseMetadata, ShStreamSourceMetadata};
use postretro_render_cpu::frame_uniforms::LightTermMask;

use passes::{StreamingAnimatedPass, StreamingPromotionPass};
pub(super) use sparse::DirectSparseRowUpload;
use sparse::{RetiredDirectSparseResources, checked_ledger_sum};

use super::gpu::StagedUploads;
use super::{AtlasShape, ShResidencyDrainError, SparseCapacityFloors};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::renderer_types::PromotedBakedLightState;
use crate::render::sh_volume::ShVolumeResources;

/// Texture views supplied by the streamed GPU-pool owner. Id 45 makes Pass A
/// write `intermediate_storage`, then Pass B samples it and writes
/// `total_storage`; id 41 alone lets Pass A write the final target directly.
pub(super) struct StreamingDirectViews<'a> {
    pub(super) base: &'a wgpu::TextureView,
    pub(super) intermediate_storage: Option<&'a wgpu::TextureView>,
    pub(super) intermediate_sampled: Option<&'a wgpu::TextureView>,
    pub(super) total_storage: &'a wgpu::TextureView,
    pub(super) selection_weights: &'a wgpu::Buffer,
}

/// Sorted distinct flattened affinity rows selected by the residency owner.
pub(super) struct StreamingDirectDirtyRows<'a> {
    pub(super) promotion: &'a [u32],
    pub(super) animated: &'a [u32],
    pub(super) force_full_resident: bool,
}

/// Per-frame state that the legacy direct compose paths also retain. Streaming
/// only changes the rows and physical atlas stride; it keeps masks, promotion
/// weights, animation attenuation, and diagnostics byte-for-byte compatible.
pub(super) struct StreamingDirectComposeFrameInputs<'a> {
    pub(super) light_term_mask: LightTermMask,
    pub(super) dirty: StreamingDirectDirtyRows<'a>,
    pub(super) promotion_override: DirectShDebugOverride,
    pub(super) animated_override: AnimatedDirectShDebugOverride,
    pub(super) promoted_animated_states: &'a [PromotedBakedLightState],
    pub(super) promotion_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    pub(super) animated_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
}

pub(super) struct StreamingDirectCompose {
    promotion: StreamingPromotionPass,
    animated: Option<StreamingAnimatedPass>,
    fixed_metadata_bytes: u64,
    active_capacity_bytes: u64,
}

/// One direct sparse family displaced by an append-preserving buffer growth.
/// The sibling family stays active, so an id-41 expansion cannot transiently
/// duplicate an unrelated id-45 backing pool (or vice versa). This retains
/// only the four sparse storage buffers needed by the completed submission.
pub(super) struct RetiredDirectSparsePass {
    resources: RetiredDirectSparseResources,
}

impl RetiredDirectSparsePass {
    pub(super) fn capacity_bytes(&self) -> u64 {
        self.resources.capacity_bytes()
    }
}

/// An uncommitted direct sparse candidate. Its private carrier can still
/// reference the current dense views because it becomes the live pass at
/// commit; it is never inserted into a retirement ticket.
pub(super) struct DirectSparseReplacement {
    carrier: DirectSparseReplacementCarrier,
}

enum DirectSparseReplacementCarrier {
    Promotion(Box<StreamingPromotionPass>),
    Animated(Box<StreamingAnimatedPass>),
}

impl DirectSparseReplacement {
    pub(super) fn active_capacity_bytes(&self) -> u64 {
        match &self.carrier {
            DirectSparseReplacementCarrier::Promotion(pass) => pass.active_capacity_bytes(),
            DirectSparseReplacementCarrier::Animated(pass) => pass.active_capacity_bytes(),
        }
    }
}

impl StreamingDirectCompose {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        views: StreamingDirectViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        sparse_capacity_floors: &SparseCapacityFloors,
    ) -> Result<Self, ShResidencyDrainError> {
        let promotion_output = if sources.animated_direct_delta.is_some() {
            views
                .intermediate_storage
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed animated direct compose requires an intermediate atlas",
                })?
        } else {
            views.total_storage
        };
        let promotion = StreamingPromotionPass::new(
            device,
            base,
            sources.direct_delta.as_ref(),
            shape,
            views.base,
            promotion_output,
            views.selection_weights,
            compose_indirection,
            sparse_capacity_floors.get(&41).copied(),
        )?;
        let animated = match sources.animated_direct_delta.as_ref() {
            Some(source) => Some(StreamingAnimatedPass::new(
                device,
                base,
                source,
                shape,
                views.intermediate_sampled.ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed animated direct compose requires an intermediate sampled view",
                })?,
                views.total_storage,
                compose_indirection,
                sh,
                uniform_bind_group_layout,
                sparse_capacity_floors.get(&45).copied(),
            )?),
            None => None,
        };
        let fixed_metadata_bytes = checked_ledger_sum(&[
            promotion.fixed_metadata_bytes(),
            animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::fixed_metadata_bytes),
        ])?;
        let active_capacity_bytes = checked_ledger_sum(&[
            promotion.active_capacity_bytes(),
            animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::active_capacity_bytes),
        ])?;
        Ok(Self {
            promotion,
            animated,
            fixed_metadata_bytes,
            active_capacity_bytes,
        })
    }

    /// Rebuild only the dense-atlas bindings. This never reallocates the
    /// direct sparse pools: id-41/id-45 capacity has its own replacement
    /// lifecycle and must not double during a 34/35 atlas layer append.
    pub(super) fn rebind_dense(
        &mut self,
        device: &wgpu::Device,
        shape: AtlasShape,
        views: StreamingDirectViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
    ) {
        let promotion_output = if self.animated.is_some() {
            views
                .intermediate_storage
                .expect("prevalidated animated direct growth has an intermediate storage view")
        } else {
            views.total_storage
        };
        self.promotion.rebind_dense(
            device,
            shape,
            views.base,
            promotion_output,
            views.selection_weights,
            compose_indirection,
        );
        if let Some(animated) = self.animated.as_mut() {
            animated.rebind_dense(
                device,
                shape,
                views
                    .intermediate_sampled
                    .expect("prevalidated animated direct growth has an intermediate sampled view"),
                views.total_storage,
                compose_indirection,
                sh,
            );
        }
    }

    /// Build one sparse backing candidate without changing the live direct
    /// passes. The GPU owner validates every requested family this way before
    /// it encodes copies or publishes any replacement.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_sparse_replacement(
        &self,
        device: &wgpu::Device,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        views: StreamingDirectViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        section_id: u32,
        sparse_floor: (u32, u32),
    ) -> Result<DirectSparseReplacement, ShResidencyDrainError> {
        match section_id {
            41 => {
                let output = if self.animated.is_some() {
                    views
                        .intermediate_storage
                        .ok_or(ShResidencyDrainError::GpuCapacity {
                            reason: "streamed direct id-41 growth lost its intermediate atlas",
                        })?
                } else {
                    views.total_storage
                };
                let replacement = StreamingPromotionPass::new(
                    device,
                    base,
                    sources.direct_delta.as_ref(),
                    shape,
                    views.base,
                    output,
                    views.selection_weights,
                    compose_indirection,
                    Some(sparse_floor),
                )?;
                Ok(DirectSparseReplacement {
                    carrier: DirectSparseReplacementCarrier::Promotion(Box::new(replacement)),
                })
            }
            45 => {
                let source = sources.animated_direct_delta.as_ref().ok_or(
                    ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-45 growth requested without streamed animated-direct metadata",
                    },
                )?;
                let replacement = StreamingAnimatedPass::new(
                    device,
                    base,
                    source,
                    shape,
                    views
                        .intermediate_sampled
                        .ok_or(ShResidencyDrainError::GpuCapacity {
                            reason: "streamed id-45 growth lost its sampled intermediate atlas",
                        })?,
                    views.total_storage,
                    compose_indirection,
                    sh,
                    uniform_bind_group_layout,
                    Some(sparse_floor),
                )?;
                self.animated
                    .as_ref()
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "id-45 growth requested without an animated direct compose pass",
                    })?;
                Ok(DirectSparseReplacement {
                    carrier: DirectSparseReplacementCarrier::Animated(Box::new(replacement)),
                })
            }
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    /// Encode only backing-buffer copies into a prevalidated candidate. This
    /// has no CPU-visible side effect and therefore remains safe to abandon
    /// if another family failed its construction earlier in the transaction.
    pub(super) fn copy_to_sparse_replacement(
        &self,
        replacement: &DirectSparseReplacement,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        match &replacement.carrier {
            DirectSparseReplacementCarrier::Promotion(destination) => {
                self.promotion.copy_retained_to(destination, encoder)
            }
            DirectSparseReplacementCarrier::Animated(destination) => {
                if let Some(source) = self.animated.as_ref() {
                    source.copy_retained_to(destination, encoder);
                }
            }
        }
    }

    /// Publish a fully constructed and copy-encoded sparse candidate. No
    /// fallible work remains at this point, so a multi-family growth can swap
    /// every active pass atomically after its single queue submission.
    pub(super) fn commit_sparse_replacement(
        &mut self,
        replacement: DirectSparseReplacement,
    ) -> RetiredDirectSparsePass {
        let resources = match replacement.carrier {
            DirectSparseReplacementCarrier::Promotion(replacement) => {
                std::mem::replace(&mut self.promotion, *replacement).into_retired_sparse_resources()
            }
            DirectSparseReplacementCarrier::Animated(replacement) => {
                let animated = self
                    .animated
                    .as_mut()
                    .expect("prevalidated id-45 replacement requires an animated pass");
                std::mem::replace(animated, *replacement).into_retired_sparse_resources()
            }
        };
        self.refresh_ledger_bytes();
        RetiredDirectSparsePass { resources }
    }

    /// Preflight an entire owned family before the GPU owner queues any row.
    /// This makes a multi-family install atomic at the renderer boundary.
    pub(super) fn validate_sparse_rows(
        &self,
        section_id: u32,
        rows: &[DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.validate_sparse_rows(rows),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 rows supplied without an animated direct compose pass",
                })?
                .validate_sparse_rows(rows),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    /// Upload rows after `validate_sparse_rows` has accepted every direct
    /// family in the install. Each pool queues entry/tile payload writes before
    /// publishing its `(start,end)` CSR pair.
    pub(super) fn upload_sparse_rows(
        &self,
        uploads: &mut StagedUploads,
        section_id: u32,
        rows: &[DirectSparseRowUpload<'_>],
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.upload_sparse_rows(uploads, rows),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 rows supplied without an animated direct compose pass",
                })?
                .upload_sparse_rows(uploads, rows),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    /// Remove a reachable id-41/id-45 row before allocator reuse or a
    /// generation reset. Payload memory can be reused only after this pair is
    /// zeroed, so a stale CSR entry cannot address new data.
    pub(super) fn clear_sparse_row_pair(
        &self,
        uploads: &mut StagedUploads,
        section_id: u32,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.clear_row_pair(uploads, row),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 row clear requested without an animated direct compose pass",
                })?
                .clear_row_pair(uploads, row),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    pub(super) fn clear_all_row_pairs(
        &self,
        queue: &wgpu::Queue,
        section_id: u32,
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.clear_all_row_pairs(queue),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 row reset requested without an animated direct compose pass",
                })?
                .clear_all_row_pairs(queue),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    pub(super) fn dispatch(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        inputs: StreamingDirectComposeFrameInputs<'_>,
    ) -> Result<(), ShResidencyDrainError> {
        let StreamingDirectComposeFrameInputs {
            light_term_mask,
            dirty,
            promotion_override,
            animated_override,
            promoted_animated_states,
            promotion_timestamp_writes,
            animated_timestamp_writes,
        } = inputs;
        if dirty.force_full_resident
            && (dirty.promotion.is_empty()
                || (self.animated.is_some() && dirty.animated.is_empty()))
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "forced streamed direct compose must include resident row ranges for every pass",
            });
        }
        if !dirty.promotion.is_empty() {
            self.promotion.dispatch(
                queue,
                encoder,
                light_term_mask,
                promotion_override,
                dirty.promotion,
                promotion_timestamp_writes,
            )?;
        }
        if !dirty.animated.is_empty() {
            if let Some(animated) = self.animated.as_mut() {
                animated.dispatch(
                    queue,
                    encoder,
                    uniform_bind_group,
                    animated_override,
                    promoted_animated_states,
                    dirty.animated,
                    animated_timestamp_writes,
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn fixed_metadata_bytes(&self) -> u64 {
        self.fixed_metadata_bytes
    }

    pub(super) fn active_capacity_bytes(&self) -> u64 {
        self.active_capacity_bytes
    }

    pub(super) fn promotion_active_capacity_bytes(&self) -> u64 {
        self.promotion.active_capacity_bytes()
    }

    pub(super) fn promotion_retired_sparse_capacity_bytes(
        &self,
    ) -> Result<u64, ShResidencyDrainError> {
        self.promotion.retired_sparse_capacity_bytes()
    }

    pub(super) fn animated_active_capacity_bytes(&self) -> u64 {
        self.animated
            .as_ref()
            .map_or(0, StreamingAnimatedPass::active_capacity_bytes)
    }

    pub(super) fn animated_retired_sparse_capacity_bytes(
        &self,
    ) -> Result<u64, ShResidencyDrainError> {
        self.animated
            .as_ref()
            .map_or(Ok(0), StreamingAnimatedPass::retired_sparse_capacity_bytes)
    }

    fn refresh_ledger_bytes(&mut self) {
        self.fixed_metadata_bytes = checked_ledger_sum(&[
            self.promotion.fixed_metadata_bytes(),
            self.animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::fixed_metadata_bytes),
        ])
        .expect("validated direct compose fixed ledger must not overflow");
        self.active_capacity_bytes = checked_ledger_sum(&[
            self.promotion.active_capacity_bytes(),
            self.animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::active_capacity_bytes),
        ])
        .expect("validated direct compose active ledger must not overflow");
    }

    pub(super) fn entry_capacity(&self, section_id: u32) -> Option<u32> {
        match section_id {
            41 => Some(self.promotion.entry_capacity()),
            45 => self
                .animated
                .as_ref()
                .map(StreamingAnimatedPass::entry_capacity),
            _ => None,
        }
    }

    pub(super) fn tile_f16_capacity(&self, section_id: u32) -> Option<u32> {
        match section_id {
            41 => Some(self.promotion.tile_f16_capacity()),
            45 => self
                .animated
                .as_ref()
                .map(StreamingAnimatedPass::tile_f16_capacity),
            _ => None,
        }
    }
}

fn unsupported_sparse_section(section_id: u32) -> ShResidencyDrainError {
    let _ = section_id;
    ShResidencyDrainError::MalformedChunk {
        cluster_id: 0,
        reason: "streamed direct compose received an unsupported sparse section",
    }
}
