// Ghost-only depth target and the depth pre-pass state that resolves ghost self-overlap:
// seeded from scene depth, written by the skinned and mover ghost pre-pass, then tested
// `Equal` by their colour passes. Scene depth is never written.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_transparency::GhostTransparency;

use super::pipeline_layout::prepass_attachment_extent;

/// The ghost-only depth attachment, at the scene extent. Recreated with the
/// other scene targets, never per frame.
pub(super) struct GhostDepthTarget {
    view: wgpu::TextureView,
}

impl GhostDepthTarget {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ghost Depth Texture"),
            size: prepass_attachment_extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: super::DEPTH_FORMAT,
            // COPY_DST: each frame with ghosts seeds it from scene depth.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Self {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    pub(super) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Copy the opaque scene's depth into this target, so the pre-pass and the
    /// colour passes depth-test against the whole opaque scene. Both targets
    /// are built at one extent, and depth copies cover the whole subresource.
    pub(super) fn seed_from_scene_depth(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene_depth: &wgpu::Texture,
    ) {
        let destination = self.view.texture();
        debug_assert_eq!(scene_depth.size(), destination.size());
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: scene_depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyTextureInfo {
                texture: destination,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            destination.size(),
        );
    }
}

/// The fixed-function state one transparency mode's ghost depth pre-pass draws
/// with. Pure data, so the mode → state mapping is testable without a device.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GhostDepthPipelineState {
    /// `None`: depth only, no fragment stage. Dither runs one that discards
    /// the pixels its colour pass discards.
    pub(super) fragment_entry: Option<&'static str>,
    pub(super) depth: wgpu::DepthStencilState,
    pub(super) cull_mode: Option<wgpu::Face>,
}

/// The pre-pass writes each ghost surface's depth where it is nearer than the
/// opaque scene and every ghost surface drawn so far, so the target ends
/// holding the nearest ghost surface per pixel. It has no colour target. It
/// culls exactly as the colour pass does, so both rasterize the same
/// triangles.
///
/// Dither discards the screen door's dropped pixels here too: a pixel a near
/// ghost drops must not hide a farther ghost that keeps it. Within one ghost
/// the drop is the same for every surface at a pixel, so this changes nothing
/// there.
pub(super) fn ghost_depth_pipeline_state(
    mode: GhostTransparency,
    depth_format: wgpu::TextureFormat,
) -> GhostDepthPipelineState {
    GhostDepthPipelineState {
        fragment_entry: match mode {
            GhostTransparency::Blend => None,
            GhostTransparency::Dither => Some("fs_dither_depth"),
        },
        depth: wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        },
        cull_mode: Some(wgpu::Face::Back),
    }
}

#[cfg(test)]
#[path = "ghost_depth_tests.rs"]
mod tests;
