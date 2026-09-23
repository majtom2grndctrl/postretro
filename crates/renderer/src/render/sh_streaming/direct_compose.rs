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

use super::{AtlasShape, ShResidencyDrainError};
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

/// Coalesced flattened affinity-row ranges. The residency owner must union
/// resident rows into these slices when `force_full_resident` is set: Pass A
/// uses id 35 + 41, while Pass B uses id 35 + 41 + 45.
pub(super) struct StreamingDirectDirtyRanges<'a> {
    pub(super) promotion: &'a [(u32, u32)],
    pub(super) animated: &'a [(u32, u32)],
    pub(super) force_full_resident: bool,
}

/// Per-frame state that the legacy direct compose paths also retain. Streaming
/// only changes the rows and physical atlas stride; it keeps masks, promotion
/// weights, animation attenuation, and diagnostics byte-for-byte compatible.
pub(super) struct StreamingDirectComposeFrameInputs<'a> {
    pub(super) light_term_mask: LightTermMask,
    pub(super) dirty: StreamingDirectDirtyRanges<'a>,
    pub(super) promotion_override: DirectShDebugOverride,
    pub(super) animated_override: AnimatedDirectShDebugOverride,
    pub(super) promoted_animated_states: &'a [PromotedBakedLightState],
}

pub(super) struct StreamingDirectCompose {
    promotion: StreamingPromotionPass,
    animated: Option<StreamingAnimatedPass>,
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
            )?),
            None => None,
        };
        Ok(Self {
            promotion,
            animated,
        })
    }

    /// A streaming direct compose instance is created only for id-41/id-45
    /// maps. Static id-35-only maps sample their base atlas directly and never
    /// instantiate this type, so they do not advance a direct compose epoch.
    pub(super) const fn has_composed_output(&self) -> bool {
        true
    }

    /// Preflight an entire owned family before the GPU owner queues any row.
    /// This makes a multi-family install atomic at the renderer boundary.
    pub(super) fn validate_sparse_rows(
        &self,
        section_id: u32,
        rows: &[DirectSparseRowUpload],
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
        queue: &wgpu::Queue,
        section_id: u32,
        rows: &[DirectSparseRowUpload],
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.upload_sparse_rows(queue, rows),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 rows supplied without an animated direct compose pass",
                })?
                .upload_sparse_rows(queue, rows),
            _ => Err(unsupported_sparse_section(section_id)),
        }
    }

    /// Remove a reachable id-41/id-45 row before allocator reuse or a
    /// generation reset. Payload memory can be reused only after this pair is
    /// zeroed, so a stale CSR entry cannot address new data.
    pub(super) fn clear_sparse_row_pair(
        &self,
        queue: &wgpu::Queue,
        section_id: u32,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        match section_id {
            41 => self.promotion.clear_row_pair(queue, row),
            45 => self
                .animated
                .as_ref()
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "id-45 row clear requested without an animated direct compose pass",
                })?
                .clear_row_pair(queue, row),
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
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn fixed_metadata_bytes(&self) -> u64 {
        self.promotion.fixed_metadata_bytes()
            + self
                .animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::fixed_metadata_bytes)
    }

    pub(super) fn active_capacity_bytes(&self) -> u64 {
        self.promotion.active_capacity_bytes()
            + self
                .animated
                .as_ref()
                .map_or(0, StreamingAnimatedPass::active_capacity_bytes)
    }
}

fn unsupported_sparse_section(section_id: u32) -> ShResidencyDrainError {
    let _ = section_id;
    ShResidencyDrainError::MalformedChunk {
        cluster_id: 0,
        reason: "streamed direct compose received an unsupported sparse section",
    }
}
