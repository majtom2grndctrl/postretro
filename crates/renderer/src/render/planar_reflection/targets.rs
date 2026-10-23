use super::*;

pub(super) struct PlanarTargets {
    pub extent: (u32, u32),
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub coverage: wgpu::TextureView,
    pub resolved: wgpu::TextureView,
    pub mask_inputs: wgpu::BindGroup,
    pub resolve_inputs: wgpu::BindGroup,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    extent: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
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
        .create_view(&Default::default())
}

impl PlanarTargets {
    pub fn new(
        device: &wgpu::Device,
        pass: &PlanarReflectionPass,
        scene_depth: &wgpu::TextureView,
        extent: (u32, u32),
    ) -> Self {
        let color = texture(device, "Planar World Color", extent, SCENE_COLOR_FORMAT);
        let depth = texture(device, "Planar World Depth", extent, DEPTH_FORMAT);
        let coverage = texture(
            device,
            "Planar Water Coverage",
            extent,
            wgpu::TextureFormat::R8Unorm,
        );
        let resolved = texture(
            device,
            "Planar Resolved Reflection",
            extent,
            SCENE_COLOR_FORMAT,
        );
        let mask_inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Mask Inputs"),
            layout: &pass.mask_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pass.inputs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(scene_depth),
                },
            ],
        });
        let resolve_inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Resolve Inputs"),
            layout: &pass.resolve_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pass.inputs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&coverage),
                },
            ],
        });
        Self {
            extent,
            color,
            depth,
            coverage,
            resolved,
            mask_inputs,
            resolve_inputs,
        }
    }
}

fn uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(256),
        },
        count: None,
    }
}
fn texture_entry(binding: u32, depth: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: if depth {
                wgpu::TextureSampleType::Depth
            } else {
                wgpu::TextureSampleType::Float { filterable: false }
            },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn mask_pipeline(
    device: &wgpu::Device,
) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Planar Mask Layout"),
        entries: &[uniform_entry(), texture_entry(1, true)],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Planar Mask Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/planar_mask.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Planar Mask Pipeline Layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Planar Mask Pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_mask"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                }],
            })],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_mask"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    (bgl, pipeline)
}

pub(super) fn resolve_pipeline(
    device: &wgpu::Device,
) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Planar Resolve Layout"),
        entries: &[
            uniform_entry(),
            texture_entry(1, false),
            texture_entry(2, true),
            texture_entry(3, false),
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Planar Resolve Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/planar_resolve.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Planar Resolve Pipeline Layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Planar Resolve Pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_resolve"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_resolve"),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    (bgl, pipeline)
}
