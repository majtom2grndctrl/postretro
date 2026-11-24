// The mover beauty pipelines: one per specular style over the mover pass's
// one module and layout, built at full init.
// See: context/lib/rendering_pipeline.md §7.3 (Mover register cliff)

use postretro_render_cpu::frame_uniforms::SpecularStyle;

use super::{mover_primitive_state, mover_vertex_layout};
use crate::render::SCENE_COLOR_FORMAT;
use crate::render::gpu_create::GpuCreate;

/// The mover beauty pipelines, one per specular style.
pub(super) struct MoverBeautyPipelines {
    pub(super) pixel: wgpu::RenderPipeline,
    pub(super) smooth: wgpu::RenderPipeline,
}

impl MoverBeautyPipelines {
    pub(super) fn for_style(&self, style: SpecularStyle) -> &wgpu::RenderPipeline {
        match style {
            SpecularStyle::Pixel => &self.pixel,
            SpecularStyle::Smooth => &self.smooth,
        }
    }
}

/// The fragment entry each style's beauty pipeline is built with. Both share
/// one module, its vertex entry and its bindings; each compiles only its own
/// style's light loop, which keeps each under the register cliff
/// (rendering_pipeline.md §7.3, Mover register cliff).
pub(crate) const fn beauty_fragment_entry(style: SpecularStyle) -> &'static str {
    match style {
        SpecularStyle::Pixel => "fs_pixel",
        SpecularStyle::Smooth => "fs_smooth",
    }
}

/// `style`'s beauty pipeline. Colour draws test `Equal` against the depth
/// `vs_depth` wrote in the depth pre-pass and write none, so each mover pixel
/// shades once.
pub(super) fn beauty_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    depth_format: wgpu::TextureFormat,
    style: SpecularStyle,
) -> wgpu::RenderPipeline {
    let label = match style {
        SpecularStyle::Pixel => "Kinematic Brush Pipeline (pixel)",
        SpecularStyle::Smooth => "Kinematic Brush Pipeline (smooth)",
    };
    device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(mover_vertex_layout())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: mover_primitive_state(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Equal),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(beauty_fragment_entry(style)),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
