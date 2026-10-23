// Ghost-only depth target seeded from scene depth, and the ghost draw state,
// with the depth-only draw that keeps each ghost's nearest surface.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_transparency::GhostTransparency;

use super::pipeline_layout::prepass_attachment_extent;

/// The fixed-function state one ghost pipeline draws with: a mode's colour
/// draw in `mesh_ghost::ghost_pipeline_state`, its depth-only draw in
/// [`ghost_depth_pipeline_state`]. Pure data, so the mode → state mapping is
/// testable without a device.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GhostPipelineState {
    pub(super) fragment_entry: &'static str,
    pub(super) blend: Option<wgpu::BlendState>,
    pub(super) write_mask: wgpu::ColorWrites,
    pub(super) depth: wgpu::DepthStencilState,
    pub(super) cull_mode: Option<wgpu::Face>,
}

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

/// A ghost's depth-only draw, run just before its colour draw in the same
/// pass: it writes each of the ghost's surfaces where it is nearer than the
/// opaque scene and everything drawn so far, so the target holds the ghost's
/// nearest surface per pixel when its colour draw tests `Equal`. Ghosts draw
/// farthest first, so a farther ghost's colour is already down where a
/// nearer one then writes over its depth. It targets the scene colour with
/// colour writes off, so it can share the colour draws' pass. It culls
/// exactly as the colour draw does, so both rasterize the same triangles.
///
/// Dither discards the screen door's dropped pixels here too: a pixel a near
/// ghost drops must not hide a farther ghost that keeps it. Within one ghost
/// the drop is the same for every surface at a pixel, so this changes nothing
/// there.
///
/// A ghost face exactly coplanar with the opaque scene fails `Less`, leaves
/// the seeded depth, and then passes the colour draw's `Equal`: it draws, and
/// z-fights with the scene either way.
pub(super) fn ghost_depth_pipeline_state(
    mode: GhostTransparency,
    depth_format: wgpu::TextureFormat,
) -> GhostPipelineState {
    GhostPipelineState {
        fragment_entry: match mode {
            GhostTransparency::Blend => "fs_blend_depth",
            GhostTransparency::Dither => "fs_dither_depth",
        },
        blend: None,
        write_mask: wgpu::ColorWrites::empty(),
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
