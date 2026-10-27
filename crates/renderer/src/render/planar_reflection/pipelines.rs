// Planar mask and resolve pipelines over the fluid position stream, built at full init.
// See: context/lib/rendering_pipeline.md §7.3a

use super::*;
use postretro_render_cpu::fluid_surface::FLUID_SURFACE_VERTEX_STRIDE;

pub(super) const MASK_SHADER_SOURCE: &str = include_str!("../../shaders/planar_mask.wgsl");
/// The resolve's own text, which the IP scan reads: the composed helpers are
/// pinned where SSR's pass composes them.
#[cfg(test)]
pub(super) const RESOLVE_AUTHORED_SOURCE: &str = include_str!("../../shaders/planar_resolve.wgsl");
/// The resolve composed with the shared motion and ray helpers, as the
/// reflection pass composes them, so a pixel-style cell's offset, tilted
/// normal and ray are SSR's by construction.
pub(super) const RESOLVE_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/planar_resolve.wgsl"),
    "
",
    include_str!("../../shaders/fluid_motion.wgsl"),
    "
",
    include_str!("../../shaders/fluid_reflection_ray.wgsl"),
    "
",
);

/// The fluid vertex stream's position alone (location 0): the mask reads no
/// normal, UV, drift or probe index.
const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    offset: 0,
    shader_location: 0,
    format: wgpu::VertexFormat::Float32x3,
}];

fn position_stream() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout {
        array_stride: FLUID_SURFACE_VERTEX_STRIDE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &POSITION_ATTRIBUTES,
    })]
}

fn uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(PLANAR_INPUT_BYTES),
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

/// The mask's group 0 entries: the inputs uniform and the scene depth.
pub(super) fn mask_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [uniform_entry(), texture_entry(1, true)]
}

/// The resolve's group 2 entries: the inputs uniform, mirror colour and
/// mirror depth. Groups 0 and 1 are the shared camera and world material, as
/// the reflection pass's; there is no probe group: planar reads no probe
/// resource.
pub(super) fn resolve_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        uniform_entry(),
        texture_entry(1, false),
        texture_entry(2, true),
    ]
}

pub(super) struct PlanarPipelines {
    pub mask_layout: wgpu::BindGroupLayout,
    pub resolve_layout: wgpu::BindGroupLayout,
    pub mask: wgpu::RenderPipeline,
    pub resolve: wgpu::RenderPipeline,
}

/// The resolve pipeline layout's group indices: camera, world material (its
/// texel grid), and the planar inputs with the mirror's colour and depth.
pub(super) const RESOLVE_CAMERA_GROUP: u32 = 0;
pub(super) const RESOLVE_MATERIAL_GROUP: u32 = 1;
pub(super) const RESOLVE_INPUTS_GROUP: u32 = 2;

impl PlanarPipelines {
    pub fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Planar Mask BGL"),
            entries: &mask_bind_group_layout_entries(),
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Planar Resolve BGL"),
            entries: &resolve_bind_group_layout_entries(),
        });
        let mask = mask_pipeline(device, &mask_layout);
        let resolve = resolve_pipeline(device, camera_layout, material_layout, &resolve_layout);
        Self {
            mask_layout,
            resolve_layout,
            mask,
            resolve,
        }
    }
}

/// Marks the selected plane's visible water in the mirror's depth: the mirror
/// target is the main screen flipped in x, so a mask texel stands under the
/// water pixel it mirrors. Writes the far value there and leaves the clear
/// (0) elsewhere, so a later `Less` draw lands only on visible water.
fn mask_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Planar Mask Shader"),
        source: wgpu::ShaderSource::Wgsl(MASK_SHADER_SOURCE.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Planar Mask Pipeline Layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Planar Mask Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_planar_mask"),
            buffers: &position_stream(),
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
            entry_point: Some("fs_planar_mask"),
            targets: &[],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Rasters the selected plane's fluid draws into the planar layer at its own
/// extent, over the fluid vertex stream's position, normal and UV. Its own
/// depth orders fluid against fluid, nearest wins; scene depth never
/// occludes, as the SSR target's.
fn resolve_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    material_layout: &wgpu::BindGroupLayout,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Planar Resolve Shader"),
        source: wgpu::ShaderSource::Wgsl(RESOLVE_SHADER_SOURCE.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Planar Resolve Pipeline Layout"),
        bind_group_layouts: &[Some(camera_layout), Some(material_layout), Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Planar Resolve Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_planar_resolve"),
            buffers: &crate::render::fluid_surface::FluidSurfacePass::planar_resolve_vertex_stream(
            ),
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_planar_resolve"),
            targets: &[Some(wgpu::ColorTargetState {
                format: PLANAR_LAYER_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
