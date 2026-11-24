// The glossy floor's screen-space layer, its source half: the highlight-free
// scene colour forward, the mover pass and the skinned-mesh pass write at the
// tiers that run the layer, and the scene-colour mip chain the floor march's
// hit reads.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

mod chain;
mod targets;

pub use targets::{GLOSSY_HIGHLIGHT_FREE, GLOSSY_SCENE_COLOUR_CHAIN};
pub(crate) use targets::{GlossyFloorLayout, GlossyFloorTargets, binds_an_up_facing_face};

use crate::render::SCENE_COLOR_FORMAT;
use crate::render::gpu_create::GpuCreate;

/// The scene-colour chain's downsample: one full-screen triangle per level.
pub(crate) const SCENE_COLOUR_CHAIN_SHADER: &str =
    include_str!("../shaders/scene_colour_chain.wgsl");

/// What an opaque colour draw writes: the scene colour alone, or, on a frame
/// that runs the floor layer, the scene colour and then the colour with its
/// highlight left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColourOutputs {
    Colour,
    HighlightFree,
}

impl ColourOutputs {
    /// The form a frame draws: highlight-free when it runs the floor layer.
    pub(crate) fn of(floor_layer: bool) -> Self {
        if floor_layer {
            Self::HighlightFree
        } else {
            Self::Colour
        }
    }
}

/// The highlight-free colour's format, and so the chain's: four bytes a
/// texel where the adapter renders `Rg11b10Ufloat`, which the renderer then
/// requests (`request_renderer_device`), else the scene colour's. The target
/// is written in full every floor-layer frame at scene extent, so its bytes
/// are bandwidth on every forward pixel; it is a reflection source, blurred
/// before it is read, so the shorter float's rounding (at worst 2⁻⁶ of a
/// value, toward zero where the backend truncates) never shows, and it never
/// leaves a highlight in: the colour less its highlight is taken in f32
/// before the one store.
pub(crate) fn highlight_free_format(features: wgpu::Features) -> wgpu::TextureFormat {
    if features.contains(wgpu::Features::RG11B10UFLOAT_RENDERABLE) {
        wgpu::TextureFormat::Rg11b10Ufloat
    } else {
        SCENE_COLOR_FORMAT
    }
}

/// The floor layer's renderer-lifetime half, built at full init so a tier
/// change compiles nothing: the chain's downsample pipeline, the layout and
/// sampler its per-level groups bind, and the targets' format.
pub(crate) struct GlossyFloorPass {
    downsample: wgpu::RenderPipeline,
    layout: GlossyFloorLayout,
    /// Chains recorded so far; tests read it.
    #[cfg(test)]
    chains: std::cell::Cell<u32>,
}

impl GlossyFloorPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Floor Chain Source BGL"),
            entries: &chain_source_layout_entries(),
        });
        // Linear and clamped: each tap's bilinear footprint is the box
        // filter's, and a border texel never reads across the screen.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Glossy Floor Chain Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glossy Floor Chain Shader"),
            source: wgpu::ShaderSource::Wgsl(SCENE_COLOUR_CHAIN_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glossy Floor Chain Pipeline Layout"),
            bind_group_layouts: &[Some(&source_layout)],
            immediate_size: 0,
        });
        let downsample = device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Glossy Floor Chain Downsample"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_chain"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_chain_downsample"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            downsample,
            layout: GlossyFloorLayout::new(source_layout, sampler, format),
            #[cfg(test)]
            chains: std::cell::Cell::new(0),
        }
    }

    /// What a level build binds from the pass, cloned for any thread.
    pub(crate) fn level_layout(&self) -> GlossyFloorLayout {
        self.layout.clone()
    }

    /// Chains recorded so far.
    #[cfg(test)]
    pub(crate) fn chains(&self) -> u32 {
        self.chains.get()
    }
}

/// The chain source group: one sampled level (the highlight-free colour for
/// level 0, the level above for every other) and the clamped linear sampler.
fn chain_source_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
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

#[cfg(test)]
#[path = "glossy_floor_tests.rs"]
mod tests;
