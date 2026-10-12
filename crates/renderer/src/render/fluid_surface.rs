// Fluid surface pass: environment-volume surface faces drawn translucent,
// two-sided and warped after skinned meshes and before billboards.
// See: context/lib/rendering_pipeline.md §7.4

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_render_cpu::fluid_surface::{
    FLUID_PARAMS_STRIDE, FLUID_PARAMS_WINDOW_BYTES, FLUID_SURFACE_VERTEX_STRIDE, FluidSurfaceDraw,
    FluidSurfaceSlots, FluidSurfaceVolume, pack_fluid_surfaces, slot_window,
};

use super::*;

/// Pipeline group of the per-volume parameter window.
pub(crate) const FLUID_PARAMS_GROUP: u32 = 4;
/// Groups the pipeline layout composes, in order: 0 camera, 1 world material,
/// 2 lighting, 3 SH volume, 4 per-volume parameters.
#[cfg(any(test, debug_assertions))]
pub(crate) const FLUID_BIND_GROUP_COUNT: usize = 5;

const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/fluid_surface.wgsl"),
    "\n",
    include_str!("../shaders/sh_indirection.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/light_falloff.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
);

/// Group 4: one 64 KiB uniform window of per-volume `vec4` rows, selected by a
/// dynamic offset. Textures, samplers and uniforms only: the shared groups
/// already spend the whole FRAGMENT storage budget.
pub(crate) fn fluid_params_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
        },
        count: None,
    }]
}

/// Every BGL entry set the fluid pipeline layout composes, in group order.
/// GPU-free, so the budget tests and the init-time debug assert derive the
/// per-stage inventory from the same builders the layout is created from.
#[cfg(any(test, debug_assertions))]
pub(crate) fn fluid_pipeline_bind_group_layout_entries()
-> [Vec<wgpu::BindGroupLayoutEntry>; FLUID_BIND_GROUP_COUNT] {
    [
        uniform_bind_group_layout_entries().to_vec(),
        material_bind_group_layout_entries().to_vec(),
        lighting_bind_group_layout_entries().to_vec(),
        sh_volume::sh_bind_group_layout_entries(),
        fluid_params_bind_group_layout_entries().to_vec(),
    ]
}

/// Sum one per-stage binding count over the fluid pipeline's groups.
#[cfg(debug_assertions)]
pub(crate) fn fluid_pipeline_stage_count(count: fn(&[wgpu::BindGroupLayoutEntry]) -> u32) -> u32 {
    fluid_pipeline_bind_group_layout_entries()
        .iter()
        .map(|entries| count(entries))
        .sum()
}

/// Straight alpha over the scene at the fluid's surface opacity.
pub(crate) fn fluid_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

/// Depth tested like the billboard pass and never written: a fluid face hides
/// nothing drawn after it, so smoke above the water stays undimmed and fog
/// composites over it.
pub(crate) fn fluid_depth_stencil_state(format: wgpu::TextureFormat) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Two-sided: a fluid face is seen from inside the volume as well as outside.
pub(crate) fn fluid_primitive_state() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: None,
        ..Default::default()
    }
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x3,
    },
    wgpu::VertexAttribute {
        offset: 12,
        shader_location: 1,
        format: wgpu::VertexFormat::Float32x3,
    },
    wgpu::VertexAttribute {
        offset: 24,
        shader_location: 2,
        format: wgpu::VertexFormat::Float32x2,
    },
];

/// One installed level's fluid geometry and per-volume parameter buffer.
struct InstalledFluidSurfaces {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    params_bind_group: wgpu::BindGroup,
    /// Indexed by draw slot.
    volumes: Vec<FluidSurfaceVolume>,
    slots: FluidSurfaceSlots,
}

pub(crate) struct FluidSurfacePass {
    pipeline: wgpu::RenderPipeline,
    params_bind_group_layout: wgpu::BindGroupLayout,
    /// `None` until a level with drawable fluid surfaces installs: a level
    /// without them allocates nothing and never opens the pass.
    installed: Option<InstalledFluidSurfaces>,
}

impl FluidSurfacePass {
    /// Build the pipeline over the renderer-owned shared layouts: camera
    /// (group 0), world material (1), lighting (2) and SH volume (3).
    pub fn new(
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        lighting_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fluid Surface Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });
        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Fluid Surface Params BGL (group 4)"),
                entries: &fluid_params_bind_group_layout_entries(),
            });
        // Same group order as `fluid_pipeline_bind_group_layout_entries`.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fluid Surface Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(lighting_bgl),
                Some(sh_bgl),
                Some(&params_bind_group_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fluid Surface Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: FLUID_SURFACE_VERTEX_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: fluid_primitive_state(),
            depth_stencil: Some(fluid_depth_stencil_state(depth_format)),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_COLOR_FORMAT,
                    blend: Some(fluid_blend_state()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            params_bind_group_layout,
            installed: None,
        }
    }

    /// Drop the installed level's fluid geometry. Level install and unload
    /// both start here.
    pub fn clear(&mut self) {
        self.installed = None;
    }

    /// Upload every drawable surface face of `volumes`, fan-triangulated into
    /// one shared buffer with per-volume material ranges. UVs normalize by
    /// `texture_dimensions` (indexed by texture index); faces whose texture is
    /// past `material_count` draw with the placeholder material.
    pub fn install(
        &mut self,
        device: &wgpu::Device,
        volumes: &[EnvironmentVolumeRecord],
        texture_dimensions: &[[u32; 2]],
        material_count: usize,
    ) {
        self.installed = None;
        let mesh = pack_fluid_surfaces(volumes, texture_dimensions, material_count);
        if mesh.indices.is_empty() {
            return;
        }
        let slots = FluidSurfaceSlots::new(&mesh.volumes);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // Zeroed at creation; each slot uploads its opacity on first draw.
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Surface Params"),
            size: u64::from(slots.window_count()) * FLUID_PARAMS_WINDOW_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Surface Params Bind Group (group 4)"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &params_buffer,
                    offset: 0,
                    size: NonZeroU64::new(FLUID_PARAMS_WINDOW_BYTES),
                }),
            }],
        });
        log::info!(
            "[Renderer] Fluid surfaces installed: {} volumes, {} triangles",
            mesh.volumes.len(),
            mesh.indices.len() / 3,
        );
        self.installed = Some(InstalledFluidSurfaces {
            vertex_buffer,
            index_buffer,
            params_buffer,
            params_bind_group,
            volumes: mesh.volumes,
            slots,
        });
    }

    /// Replace this frame's draws, uploading only opacity rows that changed.
    pub fn set_draws(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        draws: &[FluidSurfaceDraw],
    ) {
        let Some(installed) = self.installed.as_mut() else {
            return;
        };
        installed.slots.set_draws(draws);
        if let Some((range, rows)) = installed.slots.take_dirty() {
            queue.write_buffer(
                &installed.params_buffer,
                u64::from(range.start) * FLUID_PARAMS_STRIDE as u64,
                bytemuck::cast_slice(rows),
            );
        }
    }

    /// The pass's own gate: any visible fluid surface this frame.
    pub fn has_draws(&self) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|installed| installed.slots.has_draws())
    }

    /// Record this frame's draws. The caller binds groups 0, 2 and 3.
    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, materials: &[GpuTexture]) {
        let Some(installed) = self.installed.as_ref() else {
            return;
        };
        if !installed.slots.has_draws() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, installed.vertex_buffer.slice(..));
        pass.set_index_buffer(installed.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        let mut bound_window = None;
        let mut bound_material = None;
        for &slot in installed.slots.frame_slots() {
            let (window, element) = slot_window(slot);
            if bound_window != Some(window) {
                let offset = u64::from(window) * FLUID_PARAMS_WINDOW_BYTES;
                pass.set_bind_group(
                    FLUID_PARAMS_GROUP,
                    &installed.params_bind_group,
                    &[offset as wgpu::DynamicOffset],
                );
                bound_window = Some(window);
            }
            for range in &installed.volumes[slot as usize].material_ranges {
                if bound_material != Some(range.material_index) {
                    let material = materials
                        .get(range.material_index)
                        .or_else(|| materials.first())
                        .expect("renderer always keeps a placeholder material bind group");
                    pass.set_bind_group(1, &material.bind_group, &[]);
                    bound_material = Some(range.material_index);
                }
                pass.draw_indexed(
                    range.index_start..range.index_start + range.index_count,
                    0,
                    element..element + 1,
                );
            }
        }
    }
}

impl Renderer {
    /// Install the level's fluid surfaces. Call after `install_textures` and
    /// `install_level_geometry` (which drops the previous level's): UVs
    /// normalize by the installed diffuse dimensions, as
    /// `normalize_world_uvs` does for world faces. Volumes without surface
    /// faces allocate nothing; a level without any never opens the pass.
    pub fn install_fluid_surfaces(&mut self, volumes: &[EnvironmentVolumeRecord]) {
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let texture_dimensions =
            super::renderer_models::loaded_texture_dimensions(&full.loaded_textures);
        let material_count = full.gpu_textures.len();
        full.fluid_surface
            .install(device, volumes, &texture_dimensions, material_count);
    }

    /// This frame's visible fluid draws, far to near, from
    /// `FluidSurfaceRenderCollector`. Call before `render_frame_indirect`; an
    /// empty slice closes the pass.
    pub fn set_fluid_surface_draws(&mut self, draws: &[FluidSurfaceDraw]) {
        let Self { queue, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        full.fluid_surface.set_draws(queue, draws);
    }
}

#[cfg(test)]
pub(crate) fn composed_shader_source() -> &'static str {
    SHADER_SOURCE
}

#[cfg(test)]
#[path = "fluid_surface_tests.rs"]
mod tests;
