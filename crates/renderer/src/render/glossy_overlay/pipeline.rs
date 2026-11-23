// The glossy overlay's render pipeline: its own groups' layout entries, its
// additive blend, and the Equal-depth redraw over forward's vertex stream.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §10

use std::num::NonZeroU64;

use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES,
};

use crate::render::gpu_create::GpuCreate;
use crate::render::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};

/// The world probe stream: one `u32` per world vertex, vertex slot 1.
const WORLD_PROBE_STRIDE: u64 = 4;
const WORLD_PROBE_SHADER_LOCATION: u32 = 3;
/// The world vertex's packed tangent, at forward's offset, after the probe
/// stream's location.
const TANGENT_SHADER_LOCATION: u32 = 4;

/// The level probe group's entries: the colour array, share table, distance
/// array and sums, each read by the fragment stage alone.
pub(crate) fn glossy_overlay_probe_layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    };
    let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    };
    [
        texture(0),
        uniform(1, FLUID_PROBE_TABLE_BYTES),
        texture(2),
        uniform(3, FLUID_PROBE_SUMS_BYTES),
    ]
}

/// The frame inputs group's entries: D_static, read by `textureLoad` at the
/// fragment's pixel, and the probe reads' linear-clamp sampler.
pub(crate) fn glossy_overlay_inputs_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// The overlay's additive composite: source × 1 + destination × 1 over the
/// HDR scene colour, a signed source, alpha kept.
pub(crate) fn glossy_overlay_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

pub(super) fn overlay_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    source: &str,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Glossy Overlay Shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Glossy Overlay Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[
                // Slot 0: the world vertex buffer: position, UV and normal
                // at forward's offsets and locations, and the tangent at
                // forward's offset.
                Some(wgpu::VertexBufferLayout {
                    array_stride: postretro_render_data::geometry::WorldVertex::STRIDE
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: TANGENT_SHADER_LOCATION,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }),
                // Slot 1: the level's world probe stream.
                Some(wgpu::VertexBufferLayout {
                    array_stride: WORLD_PROBE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: WORLD_PROBE_SHADER_LOCATION,
                        format: wgpu::VertexFormat::Uint32,
                    }],
                }),
            ],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        // Forward's rasterization, so the redraw covers the same fragments.
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        // Equal against the pre-pass depth, as forward tests it; a mover or
        // character drawn over the face since fails it.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Equal),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend: Some(glossy_overlay_blend_state()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
