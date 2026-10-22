// Kinematic-brush ghost pipelines (blend and screen-door), their instance upload and
// draw recording over the mover pass's shared geometry.
// See: context/lib/rendering_pipeline.md §7.9

use postretro_render_cpu::ghost_instance::{
    KINEMATIC_GHOST_INSTANCE_SIZE, KinematicGhostInstance, pack_kinematic_ghost_instance,
};
use postretro_render_cpu::ghost_transparency::GhostTransparency;

use super::kinematic_brush::KinematicBrushPass;
use super::mesh_ghost::ghost_pipeline_state;
use super::{GpuTexture, SCENE_COLOR_FORMAT};

/// Unlit mover ghost shader with the shared ghost look helper appended. It
/// binds no lights, shadows or SH.
pub(super) const KINEMATIC_GHOST_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/kinematic_ghost.wgsl"),
    "\n",
    include_str!("../shaders/ghost_look.wgsl"),
);

/// One packed ghost instance's draw: which mover's geometry, at which slot of
/// the ghost instance buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GhostDraw {
    mover_id: u32,
    instance_index: u32,
}

/// Mover ghosts: their own 80-byte instance list (model plus packed look),
/// never deduplicated by mover id, drawn unlit through the mod's
/// `render.ghosts` mode over the mover pass's vertex and index buffers. Never
/// part of shadow depth, occluder bounds or SH sample regions.
pub(crate) struct KinematicGhostPass {
    blend: wgpu::RenderPipeline,
    dither: wgpu::RenderPipeline,
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
    /// Layout: group 0 camera, group 1 the world material, group 2 the ghost
    /// instances. No light, shadow or SH group, so mover ghosts add nothing to
    /// any fragment binding budget.
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
        let build = |mode: GhostTransparency, label: &str| {
            let state = ghost_pipeline_state(mode, depth_format);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    // The mover pass's world vertex stream; unlit, so only
                    // position and base UV are declared.
                    buffers: &[Some(wgpu::VertexBufferLayout {
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
                        ],
                    })],
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
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let instance_buffer = create_instance_buffer(device, 1);
        let instance_bind_group =
            create_instance_bind_group(device, &instance_bind_group_layout, &instance_buffer);
        Self {
            blend: build(GhostTransparency::Blend, "Kinematic Ghost Blend Pipeline"),
            dither: build(GhostTransparency::Dither, "Kinematic Ghost Dither Pipeline"),
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
            if brush.material_spans(instance.mover_id).is_none() {
                continue;
            }
            self.draws.push(GhostDraw {
                mover_id: instance.mover_id,
                instance_index: self.draws.len() as u32,
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

    /// Record every mover ghost through `mode`'s pipeline. Group 0 (camera)
    /// must be set by the caller. One instanced draw per material span per
    /// ghost, from the mover pass's shared geometry.
    pub(crate) fn record_draws(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        brush: &KinematicBrushPass,
        materials: &[GpuTexture],
        mode: GhostTransparency,
    ) {
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(match mode {
            GhostTransparency::Blend => &self.blend,
            GhostTransparency::Dither => &self.dither,
        });
        pass.set_bind_group(2, &self.instance_bind_group, &[]);
        pass.set_vertex_buffer(0, brush.shared_vertex_buffer().slice(..));
        pass.set_index_buffer(
            brush.shared_index_buffer().slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for draw in &self.draws {
            let Some(spans) = brush.material_spans(draw.mover_id) else {
                continue;
            };
            let instances = draw.instance_index..draw.instance_index + 1;
            for (material_index, indices) in spans {
                let material = materials
                    .get(material_index)
                    .or_else(|| materials.first())
                    .expect("renderer always keeps a placeholder material bind group");
                pass.set_bind_group(1, &material.bind_group, &[]);
                pass.draw_indexed(indices, 0, instances.clone());
            }
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
