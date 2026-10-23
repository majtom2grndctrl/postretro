// Kinematic-brush ghost pipelines (a colour and a depth-only draw per mode),
// their instance upload and draw recording over the mover pass's geometry.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_instance::{
    KINEMATIC_GHOST_INSTANCE_SIZE, KinematicGhostInstance, pack_kinematic_ghost_instance,
};
use postretro_render_cpu::ghost_transparency::GhostTransparency;

use super::ghost_order::{GhostDrawStep, mover_ghost_corners};
use super::kinematic_brush::KinematicBrushPass;
use super::mesh_ghost::GhostPipelines;
use super::{GpuTexture, SCENE_COLOR_FORMAT};

/// Unlit mover ghost shader with the shared ghost look helper appended. It
/// binds no lights, shadows or SH.
pub(super) const KINEMATIC_GHOST_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/kinematic_ghost.wgsl"),
    "\n",
    include_str!("../shaders/ghost_look.wgsl"),
);

/// The mover pass's world vertex stream; unlit, so only position and base UV
/// are declared.
static KINEMATIC_GHOST_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
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
];

fn kinematic_ghost_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: postretro_render_data::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &KINEMATIC_GHOST_VERTEX_ATTRIBUTES,
    }
}

/// One packed ghost instance's draw: which mover's geometry, at which slot of
/// the ghost instance buffer, and where its geometry's bounds corners land,
/// the points the frame sorts ghosts by.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GhostDraw {
    mover_id: u32,
    instance_index: u32,
    corners: [glam::Vec3; 8],
}

/// Mover ghosts: their own 80-byte instance list (model plus packed look),
/// never deduplicated by mover id, drawn unlit through the mod's
/// `render.ghosts` mode over the mover pass's vertex and index buffers. Never
/// part of shadow depth, occluder bounds or SH sample regions.
pub(crate) struct KinematicGhostPass {
    pipelines: GhostPipelines,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    /// Instances the buffer holds.
    instance_capacity: usize,
    instance_bind_group: wgpu::BindGroup,
    /// This frame's packed instances, retained scratch.
    instance_bytes: Vec<u8>,
    draws: Vec<GhostDraw>,
}

impl KinematicGhostPass {
    /// Colour layout: group 0 camera, group 1 the world material, group 2 the
    /// ghost instances; the depth-only layout drops the material. No light,
    /// shadow or SH group, so mover ghosts add nothing to any fragment binding
    /// budget. Every pipeline targets the scene colour, so both draws of a
    /// ghost share one pass with the skinned ghosts; the depth-only one writes
    /// no colour.
    pub(crate) fn new(
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Kinematic Ghost Shader"),
            source: wgpu::ShaderSource::Wgsl(KINEMATIC_GHOST_SHADER_SOURCE.into()),
        });
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kinematic Ghost Instance BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Kinematic Ghost Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(&instance_bind_group_layout),
            ],
            immediate_size: 0,
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Kinematic Ghost Depth Pipeline Layout"),
            bind_group_layouts: &[Some(camera_bgl), None, Some(&instance_bind_group_layout)],
            immediate_size: 0,
        });
        let pipelines = GhostPipelines::build("Kinematic", depth_format, |step, state, label| {
            let pipeline_layout = match step {
                GhostDrawStep::Depth => &depth_layout,
                GhostDrawStep::Colour => &layout,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(kinematic_ghost_vertex_layout())],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: state.cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(state.depth),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(state.fragment_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_COLOR_FORMAT,
                        blend: state.blend,
                        write_mask: state.write_mask,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let instance_buffer = create_instance_buffer(device, 1);
        let instance_bind_group =
            create_instance_bind_group(device, &instance_bind_group_layout, &instance_buffer);
        Self {
            pipelines,
            instance_bind_group_layout,
            instance_buffer,
            instance_capacity: 1,
            instance_bind_group,
            instance_bytes: Vec::new(),
            draws: Vec::new(),
        }
    }

    /// Replace this frame's mover ghosts. An instance whose mover this level
    /// did not upload is skipped. No allocation once the buffer and scratch
    /// have grown to the frame's ghost count.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        instances: &[KinematicGhostInstance],
        brush: &KinematicBrushPass,
    ) {
        self.draws.clear();
        self.instance_bytes.clear();
        for instance in instances {
            let Some(local_bounds) = brush.mover_bounds(instance.mover_id) else {
                continue;
            };
            self.draws.push(GhostDraw {
                mover_id: instance.mover_id,
                instance_index: self.draws.len() as u32,
                corners: mover_ghost_corners(instance.transform, local_bounds),
            });
            self.instance_bytes
                .extend_from_slice(&pack_kinematic_ghost_instance(
                    instance.transform,
                    instance.look,
                ));
        }
        if self.draws.is_empty() {
            return;
        }
        if self.draws.len() > self.instance_capacity {
            self.instance_capacity = self.draws.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(device, self.instance_capacity);
            self.instance_bind_group = create_instance_bind_group(
                device,
                &self.instance_bind_group_layout,
                &self.instance_buffer,
            );
        }
        queue.write_buffer(&self.instance_buffer, 0, &self.instance_bytes);
    }

    /// Drop this frame's ghosts: level geometry changed, so mover ids moved.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.instance_bytes.clear();
    }

    pub(crate) fn has_draws(&self) -> bool {
        !self.draws.is_empty()
    }

    /// Each mover ghost's world-space geometry bounds corners, in draw-slot
    /// order.
    pub(super) fn draw_corners(&self) -> impl Iterator<Item = [glam::Vec3; 8]> + '_ {
        self.draws.iter().map(|draw| draw.corners)
    }

    /// Record the mover ghost in slot `draw` as `step` of its draw pair,
    /// through `mode`'s pipelines: one draw per material span, from the mover
    /// pass's shared geometry. Group 0 (camera) must be set by the caller;
    /// this sets the instances and, for the colour step, each span's world
    /// material.
    pub(super) fn record_ghost(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        brush: &KinematicBrushPass,
        materials: &[GpuTexture],
        draw: u32,
        mode: GhostTransparency,
        step: GhostDrawStep,
    ) {
        let Some(draw) = self.draws.get(draw as usize) else {
            return;
        };
        let Some(spans) = brush.material_spans(draw.mover_id) else {
            return;
        };
        pass.set_pipeline(self.pipelines.for_step(mode, step));
        pass.set_bind_group(2, &self.instance_bind_group, &[]);
        pass.set_vertex_buffer(0, brush.shared_vertex_buffer().slice(..));
        pass.set_index_buffer(
            brush.shared_index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        let instances = draw.instance_index..draw.instance_index + 1;
        for (material_index, indices) in spans {
            if step == GhostDrawStep::Colour {
                let material = materials
                    .get(material_index)
                    .or_else(|| materials.first())
                    .expect("renderer always keeps a placeholder material bind group");
                pass.set_bind_group(1, &material.bind_group, &[]);
            }
            pass.draw_indexed(indices, 0, instances.clone());
        }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Kinematic Ghost Instance Buffer"),
        size: (capacity * KINEMATIC_GHOST_INSTANCE_SIZE) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_instance_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Kinematic Ghost Instance Bind Group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}

#[cfg(test)]
#[path = "kinematic_ghost_tests.rs"]
pub(crate) mod tests;
