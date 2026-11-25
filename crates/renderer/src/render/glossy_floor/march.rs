// The glossy floor layer's march: its pipelines (one per reflection style and
// material form) over its own layout, the frame's scene-depth group, and the
// recording of one level's march into its floor-layer target.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Floors; Screen-space layer)

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionPreset;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::render_extent::Extent;

use super::{GlossyFloorPass, GlossyFloorTargets};
use crate::compute_cull::{ComputeCullPipeline, SetTextureFn};
use crate::render::DEPTH_FORMAT;
use crate::render::glossy_overlay::{
    GLOSSY_STYLES, GlossyOverlayForm, glossy_primitive_state, glossy_vertex_buffers, style_index,
};
use crate::render::gpu_create::GpuCreate;

/// The march's shader: its own bindings, vertex stage and entries, the
/// chain level and read at the hit, the glossy surface, the overlay's
/// shading inputs (so both layers reflect one ray from one point), the tint
/// rule's texel read, forward's Surface Depth march, the probe term's
/// exponent → mip function, then the ray and water's march, by name. Never
/// the probe term's search, shadow or live lighting (`fluid_reflection.wgsl`).
pub(crate) const GLOSSY_FLOOR_MARCH_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/glossy_floor_march.wgsl"),
    "\n",
    include_str!("../../shaders/glossy_floor_blur.wgsl"),
    "\n",
    include_str!("../../shaders/glossy_surface.wgsl"),
    "\n",
    include_str!("../../shaders/glossy_overlay_shading.wgsl"),
    "\n",
    include_str!("../../shaders/material_shading.wgsl"),
    "\n",
    include_str!("../../shaders/surface_depth.wgsl"),
    "\n",
    include_str!("../../shaders/reflection_probe_blur.wgsl"),
    "\n",
    include_str!("../../shaders/fluid_motion.wgsl"),
    "\n",
    include_str!("../../shaders/fluid_reflection_ray.wgsl"),
    "\n",
    include_str!("../../shaders/screen_march.wgsl"),
    "\n",
);

/// The march's groups: 0 the shared camera group, 1 the face's world
/// material (forward's layout and bind groups, as the overlay binds them),
/// 2 the frame's scene depth, 3 the level's floor layer (the chain, its
/// sampler and the tier's march).
pub(crate) const GLOSSY_FLOOR_MATERIAL_GROUP: u32 = 1;
pub(crate) const GLOSSY_FLOOR_SCENE_GROUP: u32 = 2;
pub(crate) const GLOSSY_FLOOR_LEVEL_GROUP: u32 = 3;

/// The floor layer's target holds premultiplied (c × X, c), as water's
/// reflection target does: a bilinear read interpolates both linearly.
pub(crate) const GLOSSY_FLOOR_LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The tier's march (group 3 binding 2), the shader's `GlossyFloorMarch`
/// member for member.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GlossyFloorMarchUniform {
    pub step_budget: u32,
    pub stride: f32,
    pub thickness: f32,
    /// Padding to the uniform's 16 bytes; carries nothing.
    pub spare: f32,
}

pub(crate) const GLOSSY_FLOOR_MARCH_UNIFORM_BYTES: u64 =
    std::mem::size_of::<GlossyFloorMarchUniform>() as u64;

impl GlossyFloorMarchUniform {
    /// The march a floor tier's `preset` runs at `scene` extent: water's
    /// columns, its stride in full-resolution pixels.
    pub(crate) fn of(preset: FluidReflectionPreset, scene: Extent) -> Self {
        Self {
            step_budget: preset.march.step_budget,
            stride: preset.march.stride((scene.width, scene.height)),
            thickness: preset.march.thickness,
            spare: 0.0,
        }
    }
}

/// Group 2: the frame's full-resolution scene depth, read by `textureLoad`
/// and never attached by this pass.
pub(crate) fn glossy_floor_scene_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }]
}

/// Group 3: the level's scene-colour chain, every level, its trilinear
/// clamped sampler and the tier's march uniform.
pub(crate) fn glossy_floor_level_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
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
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(GLOSSY_FLOOR_MARCH_UNIFORM_BYTES),
            },
            count: None,
        },
    ]
}

/// The march's own depth: the nearest floor owns a target texel, as water's
/// reflection depth orders fluid against fluid; scene depth never occludes.
pub(crate) fn glossy_floor_depth_stencil_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// The march's pipelines, one per `render.reflections.style` and
/// [`GlossyOverlayForm`], all built at full init so a style or tier change
/// compiles nothing.
pub(crate) struct GlossyFloorMarchPipelines {
    /// Indexed `[style][form]`, as the overlay's.
    pipelines: [[wgpu::RenderPipeline; 3]; 2],
}

impl GlossyFloorMarchPipelines {
    pub(super) fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glossy Floor March Shader"),
            source: wgpu::ShaderSource::Wgsl(GLOSSY_FLOOR_MARCH_SHADER_SOURCE.into()),
        });
        Self {
            pipelines: GLOSSY_STYLES.map(|style| {
                GlossyOverlayForm::ALL
                    .map(|form| march_pipeline(device, layout, &shader, form.entry(style)))
            }),
        }
    }

    fn for_style(
        &self,
        style: FluidReflectionStyle,
        form: GlossyOverlayForm,
    ) -> &wgpu::RenderPipeline {
        &self.pipelines[style_index(style)][form.index()]
    }

    /// `pipeline` in every style's and form's place: a test-only variant.
    #[cfg(test)]
    pub(crate) fn every(pipeline: wgpu::RenderPipeline) -> Self {
        Self {
            pipelines: GLOSSY_STYLES.map(|_| GlossyOverlayForm::ALL.map(|_| pipeline.clone())),
        }
    }
}

pub(crate) fn march_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Glossy Floor March Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &glossy_vertex_buffers(),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: glossy_primitive_state(),
        depth_stencil: Some(glossy_floor_depth_stencil_state()),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: GLOSSY_FLOOR_LAYER_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The frame's scene-depth group, built at full init and with the scene
/// targets, never per frame.
pub(super) fn scene_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_depth: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.new_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Glossy Floor Scene Bind Group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(scene_depth),
        }],
    })
}

impl GlossyFloorPass {
    /// Record `targets`' march over the frame's glossy camera ranges in
    /// `style`, the frame's committed reflection style. The caller opens the
    /// pass over the floor layer's target and depth, both cleared, and binds
    /// group 0; this binds 2 and 3, the world vertex and probe streams, and
    /// each bucket's world material through `bind_material`. `glossy` names
    /// the glossy buckets, `form` each bucket's [`GlossyOverlayForm`]; the
    /// vertex stage drops every face but a probe-bound, up-facing one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_march<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        style: FluidReflectionStyle,
        targets: &'a GlossyFloorTargets,
        world_probes: &'a wgpu::Buffer,
        vertex_buffer: &'a wgpu::Buffer,
        index_buffer: &'a wgpu::Buffer,
        cull: &'a ComputeCullPipeline,
        glossy: &dyn Fn(u32) -> bool,
        form: &dyn Fn(u32) -> GlossyOverlayForm,
        bind_material: &SetTextureFn<'a>,
    ) {
        #[cfg(test)]
        self.marches.set(self.marches.get() + 1);
        pass.set_bind_group(GLOSSY_FLOOR_SCENE_GROUP, &self.scene_group, &[]);
        pass.set_bind_group(GLOSSY_FLOOR_LEVEL_GROUP, targets.march_group(), &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, world_probes.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        // One sweep per form, as the overlay's; the march's depth keeps the
        // nearest floor at a texel whatever the order.
        for drawn in GlossyOverlayForm::ALL {
            let keep = |bucket: u32| glossy(bucket) && form(bucket) == drawn;
            if !cull.draws_bucket(keep) {
                continue;
            }
            pass.set_pipeline(self.march.for_style(style, drawn));
            cull.draw_indirect_subset(pass, Some(bind_material), &keep);
        }
    }
}
