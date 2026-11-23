// The glossy world overlay pass: its pipeline over its own layout, forward's
// static-diffuse forms, and the frame's draw gate.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces), §10

mod level;

pub use level::GLOSSY_STATIC_DIFFUSE;
pub(crate) use level::{GlossyLevelLayout, GlossyLevelState};

use std::num::NonZeroU64;

use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES, reflection_mask_on,
};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::gpu_create::GpuCreate;
use super::renderer_init_pipelines::ForwardStaticDiffusePipelines;
use super::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::compute_cull::{ComputeCullPipeline, SetTextureFn};

/// The overlay's shader: its own bindings and entries, the tint rule
/// forward's highlight reads, the exponent → mip function, then the probe
/// term and Fresnel it reuses from the fluid shader's modules, by name.
pub(crate) const GLOSSY_OVERLAY_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/glossy_overlay.wgsl"),
    "\n",
    include_str!("../shaders/material_shading.wgsl"),
    "\n",
    include_str!("../shaders/reflection_probe_blur.wgsl"),
    "\n",
    include_str!("../shaders/reflection_probe_lobes.wgsl"),
    "\n",
    include_str!("../shaders/fluid_motion.wgsl"),
    "\n",
    include_str!("../shaders/fluid_reflection_ray.wgsl"),
    "\n",
    include_str!("../shaders/fluid_reflection.wgsl"),
    "\n",
);

/// The overlay's groups: 0 the shared camera group, 1 the face's world
/// material (forward's group 1 layout and bind groups, so a material's live
/// uniform reaches both), 2 the level's probe records, 3 the frame's inputs
/// (forward's static diffuse and a sampler). Forward is at 16/16 sampled
/// textures, so the overlay never binds through forward's whole layout.
pub(crate) const GLOSSY_OVERLAY_MATERIAL_GROUP: u32 = 1;
pub(crate) const GLOSSY_OVERLAY_PROBE_GROUP: u32 = 2;
pub(crate) const GLOSSY_OVERLAY_INPUTS_GROUP: u32 = 3;
#[cfg(test)]
pub(crate) const GLOSSY_OVERLAY_BIND_GROUP_COUNT: usize = 4;

/// The world probe stream: one `u32` per world vertex, vertex slot 1.
pub(crate) const WORLD_PROBE_STRIDE: u64 = 4;
const WORLD_PROBE_SHADER_LOCATION: u32 = 3;

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

/// Whether the frame records the overlay: a tier above off, every bit of
/// the all-terms mask on, a level whose glossy faces are ready to draw (probe
/// records bound and D_static allocated), and a camera range over a glossy
/// bucket. The same condition puts the overlay among the sums' readers.
pub(crate) fn glossy_overlay_draws(
    quality: FluidReflectionQuality,
    light_term_mask: LightTermMask,
    level_ready: bool,
    glossy_range_drawn: bool,
) -> bool {
    quality != FluidReflectionQuality::Off
        && reflection_mask_on(light_term_mask.bits())
        && level_ready
        && glossy_range_drawn
}

/// The overlay pass, built at full init: its pipeline, its two own layouts,
/// the probe sampler, and forward's static-diffuse forms. Level state (the
/// probe stream, the probe group and the D_static target) belongs to the
/// level set.
pub(crate) struct GlossyOverlayPass {
    pipeline: wgpu::RenderPipeline,
    #[cfg(test)]
    pipeline_layout: wgpu::PipelineLayout,
    level_layout: GlossyLevelLayout,
    forward: ForwardStaticDiffusePipelines,
    /// Overlay passes recorded so far; tests read it.
    #[cfg(test)]
    recorded: std::cell::Cell<u32>,
}

impl GlossyOverlayPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        uniform_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        forward: ForwardStaticDiffusePipelines,
    ) -> Self {
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Overlay Probe BGL"),
            entries: &glossy_overlay_probe_layout_entries(),
        });
        let inputs_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Glossy Overlay Inputs BGL"),
            entries: &glossy_overlay_inputs_layout_entries(),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Glossy Overlay Probe Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glossy Overlay Pipeline Layout"),
            bind_group_layouts: &[
                Some(uniform_layout),
                Some(material_layout),
                Some(&probe_layout),
                Some(&inputs_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = overlay_pipeline(
            device,
            &pipeline_layout,
            GLOSSY_OVERLAY_SHADER_SOURCE,
            "fs_main",
        );
        let level_layout = GlossyLevelLayout {
            probe_layout,
            inputs_layout,
            sampler,
            static_diffuse_format: forward.format,
        };
        Self {
            pipeline,
            #[cfg(test)]
            pipeline_layout,
            level_layout,
            forward,
            #[cfg(test)]
            recorded: std::cell::Cell::new(0),
        }
    }

    /// What a level build binds from the pass, cloned for any thread.
    pub(crate) fn level_layout(&self) -> GlossyLevelLayout {
        self.level_layout.clone()
    }

    /// Forward's two-output forms: glossy buckets write D_static, every
    /// other bucket masks it.
    pub(crate) fn forward_forms(&self) -> &ForwardStaticDiffusePipelines {
        &self.forward
    }

    /// Record the overlay's draws over the frame's glossy camera ranges. The
    /// caller opens the pass (scene colour loaded, depth read-only) and binds
    /// group 0; `glossy` names the glossy buckets and `bind_material` binds a
    /// bucket's world material at [`GLOSSY_OVERLAY_MATERIAL_GROUP`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        state: &'a GlossyLevelState,
        vertex_buffer: &'a wgpu::Buffer,
        index_buffer: &'a wgpu::Buffer,
        cull: &'a ComputeCullPipeline,
        glossy: &dyn Fn(u32) -> bool,
        bind_material: &SetTextureFn<'a>,
    ) {
        let (Some(probe_group), Some(inputs_group)) = (state.probe_group(), state.inputs_group())
        else {
            return;
        };
        #[cfg(test)]
        self.recorded.set(self.recorded.get() + 1);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(GLOSSY_OVERLAY_PROBE_GROUP, probe_group, &[]);
        pass.set_bind_group(GLOSSY_OVERLAY_INPUTS_GROUP, inputs_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, state.world_probes().slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        cull.draw_indirect_subset(pass, Some(bind_material), glossy);
    }

    /// Overlay passes recorded so far.
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> u32 {
        self.recorded.get()
    }

    /// An overlay pipeline over the production layout whose fragment entry
    /// `entry` comes from `extra` appended to the production shader: the
    /// test-only variants (a constant per fragment, R forced to D_static).
    #[cfg(test)]
    pub(crate) fn variant_pipeline(
        &self,
        device: &wgpu::Device,
        extra: &str,
        entry: &str,
    ) -> wgpu::RenderPipeline {
        let source = format!("{GLOSSY_OVERLAY_SHADER_SOURCE}\n{extra}\n");
        overlay_pipeline(device, &self.pipeline_layout, &source, entry)
    }

    /// Put `pipeline` in the production pipeline's place, returning it.
    #[cfg(test)]
    pub(crate) fn swap_pipeline(&mut self, pipeline: wgpu::RenderPipeline) -> wgpu::RenderPipeline {
        std::mem::replace(&mut self.pipeline, pipeline)
    }
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

fn overlay_pipeline(
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
                // Slot 0: the world vertex buffer: position, UV and normal,
                // at forward's offsets and locations.
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

#[cfg(test)]
#[path = "glossy_overlay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "glossy_overlay_mip_tests.rs"]
mod mip_tests;
