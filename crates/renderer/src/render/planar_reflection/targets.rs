// Planar mirror colour and depth targets and the mask and resolve bind groups over them.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;

pub(super) struct PlanarTargets {
    pub extent: (u32, u32),
    _color: wgpu::Texture,
    pub color: wgpu::TextureView,
    _depth: wgpu::Texture,
    /// The mask writes its far value here; the mirror's depth prepass and
    /// opaque draws then land only on masked water.
    pub depth: wgpu::TextureView,
    pub mask_inputs: wgpu::BindGroup,
    pub resolve_inputs: wgpu::BindGroup,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    extent: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: extent.0,
            height: extent.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | if cfg!(test) {
                wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    })
}

impl PlanarTargets {
    /// Built only when the planar layer is created, resized or follows
    /// recreated scene targets: never per frame.
    pub fn new(
        device: &wgpu::Device,
        pipelines: &pipelines::PlanarPipelines,
        inputs_buffer: &wgpu::Buffer,
        scene_depth: &wgpu::TextureView,
        extent: (u32, u32),
    ) -> Self {
        let color_texture = texture(device, "Planar Mirror Color", extent, SCENE_COLOR_FORMAT);
        let color = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_texture = texture(device, "Planar Mirror Depth", extent, DEPTH_FORMAT);
        let depth = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Mask Inputs"),
            layout: &pipelines.mask_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: inputs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(scene_depth),
                },
            ],
        });
        let resolve_inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Resolve Inputs"),
            layout: &pipelines.resolve_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: inputs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });
        Self {
            extent,
            _color: color_texture,
            color,
            _depth: depth_texture,
            depth,
            mask_inputs,
            resolve_inputs,
        }
    }
}
