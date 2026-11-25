// The glossy floor's screen-space layer: the highlight-free scene colour
// forward, the mover pass and the skinned-mesh pass write at the tiers that
// run the layer, the scene-colour mip chain, and the march whose hit reads
// it, into the floor layer's target the overlay composites.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors)

mod chain;
mod march;
mod targets;

#[cfg(test)]
pub(crate) use march::{
    GLOSSY_FLOOR_LAYER_FORMAT, GLOSSY_FLOOR_MARCH_SHADER_SOURCE, GLOSSY_FLOOR_MARCH_UNIFORM_BYTES,
    glossy_floor_depth_stencil_state, glossy_floor_level_layout_entries,
    glossy_floor_scene_layout_entries, march_pipeline,
};
pub(crate) use march::{GLOSSY_FLOOR_MATERIAL_GROUP, GlossyFloorMarchPipelines};
pub use targets::{
    GLOSSY_FLOOR_LAYER, GLOSSY_FLOOR_LAYER_DEPTH, GLOSSY_HIGHLIGHT_FREE, GLOSSY_SCENE_COLOUR_CHAIN,
};
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
/// change compiles nothing: the chain's downsample pipeline, the march's
/// pipelines, the layouts and samplers the per-level groups bind, the
/// targets' format, and the march's group over the frame's scene depth.
pub(crate) struct GlossyFloorPass {
    downsample: wgpu::RenderPipeline,
    march: GlossyFloorMarchPipelines,
    layout: GlossyFloorLayout,
    scene_layout: wgpu::BindGroupLayout,
    /// Scene depth, rebuilt with the scene targets.
    scene_group: wgpu::BindGroup,
    /// Chains recorded so far; tests read it.
    #[cfg(test)]
    chains: std::cell::Cell<u32>,
    /// Marches recorded so far; tests read it.
    #[cfg(test)]
    marches: std::cell::Cell<u32>,
    /// The march's layout, for test-only variant pipelines.
    #[cfg(test)]
    march_layout: wgpu::PipelineLayout,
}

impl GlossyFloorPass {
    /// Build every floor-layer pipeline over the shared camera
    /// (`uniform_layout`) and world-material (`material_layout`) layouts,
    /// with the march's scene group over `scene_depth`, at full init.
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        uniform_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        scene_depth: &wgpu::TextureView,
    ) -> Self {
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
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Floor Scene BGL"),
            entries: &march::glossy_floor_scene_layout_entries(),
        });
        let level_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Floor Level BGL"),
            entries: &march::glossy_floor_level_layout_entries(),
        });
        let march_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glossy Floor March Pipeline Layout"),
            bind_group_layouts: &[
                Some(uniform_layout),
                Some(material_layout),
                Some(&scene_layout),
                Some(&level_layout),
            ],
            immediate_size: 0,
        });
        // Trilinear and clamped: a hit reads between two chain levels, and
        // a footprint at the screen's edge never wraps.
        let march_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Glossy Floor March Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // c = 0 wherever the overlay binds it: the overlay's inputs hold it
        // while a level has no floor-layer target.
        let placeholder = device.new_texture(&wgpu::TextureDescriptor {
            label: Some("Glossy Floor Layer Placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: march::GLOSSY_FLOOR_LAYER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let placeholder = placeholder.create_view(&wgpu::TextureViewDescriptor::default());
        let march = GlossyFloorMarchPipelines::new(device, &march_layout);
        let scene_group = march::scene_group(device, &scene_layout, scene_depth);
        Self {
            downsample,
            march,
            layout: GlossyFloorLayout::new(
                source_layout,
                sampler,
                format,
                level_layout,
                march_sampler,
                placeholder,
            ),
            scene_layout,
            scene_group,
            #[cfg(test)]
            chains: std::cell::Cell::new(0),
            #[cfg(test)]
            marches: std::cell::Cell::new(0),
            #[cfg(test)]
            march_layout,
        }
    }

    /// Follow a recreated scene depth: the march's scene group rebuilds over
    /// it. The scene-target rebuild is the only caller; no frame builds it.
    pub(crate) fn rebuild_scene_inputs(
        &mut self,
        device: &wgpu::Device,
        scene_depth: &wgpu::TextureView,
    ) {
        self.scene_group = march::scene_group(device, &self.scene_layout, scene_depth);
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

    /// Marches recorded so far.
    #[cfg(test)]
    pub(crate) fn marches(&self) -> u32 {
        self.marches.get()
    }

    /// A march pipeline drawing `entry` from the march's shader with `extra`
    /// WGSL composed after it (test-only).
    #[cfg(test)]
    pub(crate) fn march_variant(
        &self,
        device: &wgpu::Device,
        extra: &str,
        entry: &str,
    ) -> wgpu::RenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glossy Floor March Variant Shader"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{extra}\n", march::GLOSSY_FLOOR_MARCH_SHADER_SOURCE).into(),
            ),
        });
        march_pipeline(device, &self.march_layout, &shader, entry)
    }

    /// Put `march` in the production march pipelines' place, returning them
    /// (test-only).
    #[cfg(test)]
    pub(crate) fn swap_march(
        &mut self,
        march: GlossyFloorMarchPipelines,
    ) -> GlossyFloorMarchPipelines {
        std::mem::replace(&mut self.march, march)
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

#[cfg(test)]
#[path = "glossy_floor_march_tests.rs"]
pub(crate) mod march_tests;

#[cfg(test)]
#[path = "glossy_floor_register_screen_test.rs"]
mod register_screen_test;

#[cfg(test)]
#[path = "glossy_floor_blur_frame_test.rs"]
mod blur_frame_test;

#[cfg(test)]
#[path = "glossy_floor_march_inputs_frame_test.rs"]
mod march_inputs_frame_test;
