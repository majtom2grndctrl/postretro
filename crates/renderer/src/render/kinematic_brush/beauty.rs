// The mover beauty pipelines: one per specular style and output form over the
// mover pass's one module and layout, all built at full init.
// See: context/lib/rendering_pipeline.md §7.3 (Mover register cliff), §7.3a (Glossy world faces)

use postretro_render_cpu::frame_uniforms::SpecularStyle;

use super::{mover_primitive_state, mover_vertex_layout};
use crate::render::SCENE_COLOR_FORMAT;
use crate::render::glossy_floor::ColourOutputs;
use crate::render::gpu_create::GpuCreate;

/// The mover beauty pipelines: per output form, one per specular style.
pub(crate) struct MoverBeautyPipelines {
    pub(super) colour: MoverStylePipelines,
    pub(super) highlight_free: MoverStylePipelines,
}

/// One output form's pipelines, one per specular style.
pub(super) struct MoverStylePipelines {
    pub(super) pixel: wgpu::RenderPipeline,
    pub(super) smooth: wgpu::RenderPipeline,
}

impl MoverBeautyPipelines {
    /// Every style and form through `build`, so a style or tier change
    /// compiles nothing.
    pub(super) fn new(
        build: impl Fn(SpecularStyle, ColourOutputs) -> wgpu::RenderPipeline,
    ) -> Self {
        let forms = |outputs| MoverStylePipelines {
            pixel: build(SpecularStyle::Pixel, outputs),
            smooth: build(SpecularStyle::Smooth, outputs),
        };
        Self {
            colour: forms(ColourOutputs::Colour),
            highlight_free: forms(ColourOutputs::HighlightFree),
        }
    }

    pub(super) fn for_style(
        &self,
        style: SpecularStyle,
        outputs: ColourOutputs,
    ) -> &wgpu::RenderPipeline {
        let forms = match outputs {
            ColourOutputs::Colour => &self.colour,
            ColourOutputs::HighlightFree => &self.highlight_free,
        };
        match style {
            SpecularStyle::Pixel => &forms.pixel,
            SpecularStyle::Smooth => &forms.smooth,
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

/// The highlight-free form's entry per style: the same shading, plus the
/// colour with its highlight left out at location 1.
pub(crate) const fn highlight_free_fragment_entry(style: SpecularStyle) -> &'static str {
    match style {
        SpecularStyle::Pixel => "fs_pixel_highlight_free",
        SpecularStyle::Smooth => "fs_smooth_highlight_free",
    }
}

/// `style`'s beauty pipeline writing `outputs`; the highlight-free colour
/// takes `highlight_free_format`. Colour draws test `Equal` against the
/// depth `vs_depth` wrote in the depth pre-pass and write none, so each
/// mover pixel shades once.
pub(super) fn beauty_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    depth_format: wgpu::TextureFormat,
    style: SpecularStyle,
    outputs: ColourOutputs,
    highlight_free_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let (label, entry_point) = match (style, outputs) {
        (SpecularStyle::Pixel, ColourOutputs::Colour) => (
            "Kinematic Brush Pipeline (pixel)",
            beauty_fragment_entry(style),
        ),
        (SpecularStyle::Smooth, ColourOutputs::Colour) => (
            "Kinematic Brush Pipeline (smooth)",
            beauty_fragment_entry(style),
        ),
        (SpecularStyle::Pixel, ColourOutputs::HighlightFree) => (
            "Kinematic Brush Pipeline (pixel, highlight-free)",
            highlight_free_fragment_entry(style),
        ),
        (SpecularStyle::Smooth, ColourOutputs::HighlightFree) => (
            "Kinematic Brush Pipeline (smooth, highlight-free)",
            highlight_free_fragment_entry(style),
        ),
    };
    let colour = Some(wgpu::ColorTargetState {
        format: SCENE_COLOR_FORMAT,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
    let highlight_free = Some(wgpu::ColorTargetState {
        format: highlight_free_format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
    let targets = [colour, highlight_free];
    let targets = match outputs {
        ColourOutputs::Colour => &targets[..1],
        ColourOutputs::HighlightFree => &targets[..],
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
            entry_point: Some(entry_point),
            targets,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
