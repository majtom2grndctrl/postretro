//! Coupled id-34/id-35 atlas allocations.
//!
//! Dense SH base/compose textures share a slot geometry and therefore grow as
//! one family. Sparse compose buffers deliberately live elsewhere: expanding
//! this family must not duplicate ids 27, 41, or 45.

use crate::render::gpu_create::GpuCreate;
use postretro_level_loader::{ShStreamBaseMetadata, ShStreamSourceMetadata};

use super::ShResidencyDrainError;
use super::direct_compose::StreamingDirectViews;
use super::gpu::{AtlasShape, array_view, texture_format};
use crate::render::sh_volume::ShVolumeResources;

pub(super) struct DenseTextures {
    pub(super) base_format: wgpu::TextureFormat,
    pub(super) direct_format: Option<wgpu::TextureFormat>,
    pub(super) base: wgpu::Texture,
    pub(super) total: wgpu::Texture,
    pub(super) base_view: wgpu::TextureView,
    pub(super) total_storage_view: wgpu::TextureView,
    pub(super) total_sampled_view: wgpu::TextureView,
    pub(super) direct_base: Option<wgpu::Texture>,
    pub(super) direct_base_view: Option<wgpu::TextureView>,
    /// The promotion-free direct atlas the shared group 3 samples.
    pub(super) direct_atlas: Option<ComposedPool>,
    /// The promotion handoff atlas the mesh superset samples.
    pub(super) direct_handoff: Option<ComposedPool>,
    /// Pass A's output on id-45 levels, which Pass B reads.
    pub(super) direct_intermediate: Option<ComposedPool>,
}

impl DenseTextures {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        sh: &mut ShVolumeResources,
    ) -> Result<Self, ShResidencyDrainError> {
        let base_format = texture_format(base.irradiance_format)?;
        let extent = shape.extent();
        let base_texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Base Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: base_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let total_texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Streamed SH Total Atlas Pool"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let base_view = array_view(&base_texture, "Streamed SH Base Atlas View");
        let total_storage_view = array_view(&total_texture, "Streamed SH Total Storage View");
        let total_sampled_view = array_view(&total_texture, "Streamed SH Total Sampled View");

        let (direct_base, direct_base_view, direct_format) = if let Some(direct) = &sources.direct {
            let format = texture_format(direct.irradiance_format)?;
            let texture = device.new_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Base Atlas Pool"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = array_view(&texture, "Streamed Direct SH Base Atlas View");
            sh.direct.enable_streamed_atlas(queue);
            (Some(texture), Some(view), Some(format))
        } else {
            (None, None, None)
        };
        let direct_compose_required =
            sources.direct_delta.is_some() || sources.animated_direct_delta.is_some();
        let (direct_atlas, direct_handoff, direct_intermediate) = if direct_compose_required {
            if direct_base.is_none() {
                return Err(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed direct compose has no id-35 base atlas",
                });
            }
            let intermediate = sources
                .animated_direct_delta
                .is_some()
                .then(|| ComposedPool::new(device, extent, "Streamed Direct SH Intermediate"));
            (
                Some(ComposedPool::new(device, extent, "Streamed Direct SH")),
                Some(ComposedPool::new(
                    device,
                    extent,
                    "Streamed Direct SH Promotion Handoff",
                )),
                intermediate,
            )
        } else {
            (None, None, None)
        };

        Ok(Self {
            base_format,
            direct_format,
            base: base_texture,
            total: total_texture,
            base_view,
            total_storage_view,
            total_sampled_view,
            direct_base,
            direct_base_view,
            direct_atlas,
            direct_handoff,
            direct_intermediate,
        })
    }

    /// Binding-15 views for the two sample bind groups: the shared group 3
    /// gets the promotion-free direct atlas, the mesh superset the promotion
    /// handoff atlas. A level with id 35 but no id 41/45 composes nothing, so
    /// both sample the base; with no id 35 both keep the whole-load dummy.
    pub(super) fn direct_sample_views<'a>(
        &'a self,
        sh: &'a ShVolumeResources,
    ) -> (&'a wgpu::TextureView, &'a wgpu::TextureView) {
        let shared = self
            .direct_atlas
            .as_ref()
            .map(|pool| &pool.sampled_view)
            .or(self.direct_base_view.as_ref())
            .unwrap_or(&sh.direct.atlas_view);
        let mesh = self
            .direct_handoff
            .as_ref()
            .map(|pool| &pool.sampled_view)
            .or(self.direct_base_view.as_ref())
            .unwrap_or(&sh.direct.handoff_atlas_view);
        (shared, mesh)
    }

    /// The direct compose views for this generation, `None` when the level
    /// composes no direct SH.
    pub(super) fn direct_views<'a>(
        &'a self,
        selection_weights: &'a wgpu::Buffer,
    ) -> Result<Option<StreamingDirectViews<'a>>, ShResidencyDrainError> {
        let (Some(atlas), Some(handoff)) = (&self.direct_atlas, &self.direct_handoff) else {
            return Ok(None);
        };
        Ok(Some(StreamingDirectViews {
            base: self
                .direct_base_view
                .as_ref()
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed direct compose lost its id-35 base view",
                })?,
            intermediate: self.direct_intermediate.as_ref(),
            promotion_free: atlas,
            handoff,
            selection_weights,
        }))
    }

    /// Every Rgba16Float direct pool this generation allocates.
    pub(super) fn direct_composed_pools(&self) -> impl Iterator<Item = &ComposedPool> {
        [
            &self.direct_atlas,
            &self.direct_handoff,
            &self.direct_intermediate,
        ]
        .into_iter()
        .flatten()
    }
}

/// One streamed Rgba16Float compose target: written through its storage view
/// by compose, sampled through its sampled view.
pub(super) struct ComposedPool {
    pub(super) texture: wgpu::Texture,
    pub(super) storage_view: wgpu::TextureView,
    pub(super) sampled_view: wgpu::TextureView,
}

impl ComposedPool {
    fn new(device: &wgpu::Device, extent: wgpu::Extent3d, label: &str) -> Self {
        let texture = device.new_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("{label} Atlas Pool")),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let storage_view = array_view(&texture, &format!("{label} Storage View"));
        let sampled_view = array_view(&texture, &format!("{label} Sampled View"));
        Self {
            texture,
            storage_view,
            sampled_view,
        }
    }
}
