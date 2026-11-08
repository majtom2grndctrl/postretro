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

pub(super) use layout::{animated_bgl_entries, promotion_bgl_entries};
pub(super) use passes::ANIMATED_DIRECT_COMPOSE_ENTRY_POINT;
use passes::{StreamingAnimatedPass, StreamingPromotionPass};
pub(super) use sparse::DirectSparseRowUpload;
use sparse::{RetiredDirectSparseResources, checked_ledger_sum};

use super::dense::ComposedPool;
use super::gpu::StagedUploads;
use super::{
    AtlasShape, ShResidencyDrainError, SparseCapacities, SparseComposeLayouts,
    StreamingShPipelines, gpu,
};
use crate::render::animated_direct_sh_compose::AnimatedDirectShDebugOverride;
use crate::render::direct_sh_compose::DirectShDebugOverride;
use crate::render::renderer_types::PromotedBakedLightState;
use crate::render::sh_volume::ShVolumeResources;

/// Texture views supplied by the streamed GPU-pool owner. Id 45 makes Pass A
/// write `intermediate`, then Pass B samples it and the base and writes both
/// finals; id 41 alone lets Pass A write both final targets directly. The
/// promotion-free atlas feeds the shared group 3, the handoff atlas the mesh
/// superset.
#[derive(Clone, Copy)]
pub(super) struct StreamingDirectViews<'a> {
    pub(super) base: &'a wgpu::TextureView,
    pub(super) intermediate: Option<&'a ComposedPool>,
    pub(super) promotion_free: &'a ComposedPool,
    pub(super) handoff: &'a ComposedPool,
    pub(super) selection_weights: &'a wgpu::Buffer,
}

impl<'a> StreamingDirectViews<'a> {
    /// Pass A's two storage outputs and whether it stores the promotion-free
    /// one itself (no id 45) or leaves it to Pass B.
    fn promotion_outputs(
        &self,
        animated: bool,
    ) -> Result<PromotionOutputs<'a>, ShResidencyDrainError> {
        let handoff = if animated {
            &self
                .intermediate
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed animated direct compose requires an intermediate atlas",
                })?
                .storage_view
        } else {
            &self.handoff.storage_view
        };
        Ok(PromotionOutputs {
            handoff,
            promotion_free: &self.promotion_free.storage_view,
            stores_promotion_free: !animated,
        })
    }

    fn animated_views(&self) -> Result<AnimatedViews<'a>, ShResidencyDrainError> {
        Ok(AnimatedViews {
            intermediate_sampled: &self
                .intermediate
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed animated direct compose requires an intermediate sampled view",
                })?
                .sampled_view,
            base_sampled: self.base,
            handoff_storage: &self.handoff.storage_view,
            promotion_free_storage: &self.promotion_free.storage_view,
        })
    }
}

/// Pass A's storage outputs; `handoff` is the intermediate on id-45 levels.
pub(super) struct PromotionOutputs<'a> {
    pub(super) handoff: &'a wgpu::TextureView,
    pub(super) promotion_free: &'a wgpu::TextureView,
    pub(super) stores_promotion_free: bool,
}

/// Pass B's textures: intermediate and base in, both final atlases out.
pub(super) struct AnimatedViews<'a> {
    pub(super) intermediate_sampled: &'a wgpu::TextureView,
    pub(super) base_sampled: &'a wgpu::TextureView,
    pub(super) handoff_storage: &'a wgpu::TextureView,
    pub(super) promotion_free_storage: &'a wgpu::TextureView,
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
        limits: &wgpu::Limits,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        views: StreamingDirectViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        pipelines: &StreamingShPipelines,
        sparse_compose: &SparseComposeLayouts,
        sparse_capacities: &SparseCapacities,
    ) -> Result<Self, ShResidencyDrainError> {
        let promotion = StreamingPromotionPass::new(
            device,
            limits,
            base,
            gpu::planned_direct_layout(sparse_compose)?,
            shape,
            views.base,
            views.promotion_outputs(sources.animated_direct_delta.is_some())?,
            views.selection_weights,
            compose_indirection,
            pipelines,
            sparse_capacities.get(&41).copied(),
        )?;
        let animated = match sources.animated_direct_delta.as_ref() {
            Some(source) => Some(StreamingAnimatedPass::new(
                device,
                limits,
                base,
                source,
                shape,
                views.animated_views()?,
                compose_indirection,
                sh,
                pipelines,
                gpu::planned_animated_direct_layout(sparse_compose)?,
                sparse_capacities.get(&45).copied(),
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
        self.promotion.rebind_dense(
            device,
            shape,
            views.base,
            views
                .promotion_outputs(self.animated.is_some())
                .expect("prevalidated animated direct growth has an intermediate storage view"),
            views.selection_weights,
            compose_indirection,
        );
        if let Some(animated) = self.animated.as_mut() {
            animated.rebind_dense(
                device,
                shape,
                views
                    .animated_views()
                    .expect("prevalidated animated direct growth has an intermediate sampled view"),
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
        limits: &wgpu::Limits,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        views: StreamingDirectViews<'_>,
        compose_indirection: &wgpu::Buffer,
        sh: &ShVolumeResources,
        pipelines: &StreamingShPipelines,
        sparse_compose: &SparseComposeLayouts,
        section_id: u32,
        sparse_capacity: (u32, u32),
    ) -> Result<DirectSparseReplacement, ShResidencyDrainError> {
        match section_id {
            41 => {
                let replacement = StreamingPromotionPass::new(
                    device,
                    limits,
                    base,
                    gpu::planned_direct_layout(sparse_compose)?,
                    shape,
                    views.base,
                    views.promotion_outputs(self.animated.is_some())?,
                    views.selection_weights,
                    compose_indirection,
                    pipelines,
                    Some(sparse_capacity),
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
                    limits,
                    base,
                    source,
                    shape,
                    views.animated_views()?,
                    compose_indirection,
                    sh,
                    pipelines,
                    gpu::planned_animated_direct_layout(sparse_compose)?,
                    Some(sparse_capacity),
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
    pub(super) fn validate_sparse_rows_for_capacity(
        &self,
        section_id: u32,
        rows: &[DirectSparseRowUpload<'_>],
        capacity: (u32, u32),
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.validate_sparse_rows(rows, capacity),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 rows supplied without an animated direct compose pass",
                })?
                .validate_sparse_rows(rows, capacity),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    /// Upload rows after capacity validation has accepted every direct
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
        queue: &crate::render::uploads::UploadQueue,
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

    pub(super) fn dispatch_promotion(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
        light_term_mask: LightTermMask,
        debug_override: DirectShDebugOverride,
        rows: &[u32],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<usize, ShResidencyDrainError> {
        self.promotion.dispatch(
            queue,
            encoder,
            light_term_mask,
            debug_override,
            rows,
            timestamp_writes,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "animated compose keeps its queue, encoder, bindings, state, rows, and timestamps explicit"
    )]
    pub(super) fn dispatch_animated(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        debug_override: AnimatedDirectShDebugOverride,
        promoted_animated_states: &[PromotedBakedLightState],
        rows: &[u32],
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) -> Result<usize, ShResidencyDrainError> {
        self.animated.as_mut().map_or(Ok(0), |animated| {
            animated.dispatch(
                queue,
                encoder,
                uniform_bind_group,
                debug_override,
                promoted_animated_states,
                rows,
                timestamp_writes,
            )
        })
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
