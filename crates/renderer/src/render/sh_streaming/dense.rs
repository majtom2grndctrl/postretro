//! Coupled id-34/id-35 atlas allocations.
//!
//! Dense SH base/compose textures share a slot geometry and therefore grow as
//! one family. Sparse compose buffers deliberately live elsewhere: expanding
//! this family must not duplicate ids 27, 41, or 45.

use postretro_level_loader::{ShStreamBaseMetadata, ShStreamSourceMetadata};

use super::ShResidencyDrainError;
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
    pub(super) direct_intermediate: Option<wgpu::Texture>,
    pub(super) direct_intermediate_storage_view: Option<wgpu::TextureView>,
    pub(super) direct_intermediate_sampled_view: Option<wgpu::TextureView>,
    pub(super) direct_total: Option<wgpu::Texture>,
    pub(super) direct_total_storage_view: Option<wgpu::TextureView>,
    pub(super) direct_total_sampled_view: Option<wgpu::TextureView>,
}

impl DenseTextures {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
        shape: AtlasShape,
        sh: &mut ShVolumeResources,
    ) -> Result<Self, ShResidencyDrainError> {
        let base_format = texture_format(base.irradiance_format)?;
        let extent = shape.extent();
        let base_texture = device.create_texture(&wgpu::TextureDescriptor {
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
        let total_texture = device.create_texture(&wgpu::TextureDescriptor {
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
            let texture = device.create_texture(&wgpu::TextureDescriptor {
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
        let (
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
        ) = if direct_compose_required {
            if direct_base.is_none() {
                return Err(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed direct compose has no id-35 base atlas",
                });
            }
            let total = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Streamed Direct SH Total Atlas Pool"),
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
            let total_storage = array_view(&total, "Streamed Direct SH Total Storage View");
            let total_sampled = array_view(&total, "Streamed Direct SH Total Sampled View");
            if sources.animated_direct_delta.is_some() {
                let intermediate = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Streamed Direct SH Intermediate Atlas Pool"),
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
                let storage = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Storage View",
                );
                let sampled = array_view(
                    &intermediate,
                    "Streamed Direct SH Intermediate Sampled View",
                );
                (
                    Some(intermediate),
                    Some(storage),
                    Some(sampled),
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            } else {
                (
                    None,
                    None,
                    None,
                    Some(total),
                    Some(total_storage),
                    Some(total_sampled),
                )
            }
        } else {
            (None, None, None, None, None, None)
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
            direct_intermediate,
            direct_intermediate_storage_view,
            direct_intermediate_sampled_view,
            direct_total,
            direct_total_storage_view,
            direct_total_sampled_view,
        })
    }
}
