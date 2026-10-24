// Ghost-only depth target, seeded from scene depth each frame with ghosts:
// the depth each ghost's depth-only draw writes its nearest surface to.
// See: context/lib/rendering_pipeline.md §7.9

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

    /// Copy the opaque scene's depth into this target, so every ghost draw
    /// depth-tests against the whole opaque scene. Both targets
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

#[cfg(test)]
#[path = "ghost_depth_tests.rs"]
mod tests;
