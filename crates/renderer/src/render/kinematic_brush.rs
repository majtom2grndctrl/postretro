// Renderer-owned draw path for PRL-loaded kinematic brush movers.
// See: context/lib/rendering_pipeline.md

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use super::*;

const INSTANCE_ENTRY_SIZE: usize = 64;
#[cfg(test)]
const KINEMATIC_BIND_GROUP_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KinematicMoverInstance {
    pub mover_id: u32,
    pub transform: glam::Mat4,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KinematicLightParams {
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    ambient_floor: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterialRange {
    material_index: usize,
    index_start: u32,
    index_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UploadedMoverDraw {
    mover_id: u32,
    material_ranges: Vec<MaterialRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveMoverDraw {
    mover_draw_index: usize,
    instance_index: u32,
}

pub(crate) struct KinematicBrushPass {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    instance_bind_group: wgpu::BindGroup,
    light_bind_group_layout: wgpu::BindGroupLayout,
    light_bind_group: Option<wgpu::BindGroup>,
    light_params_buffer: wgpu::Buffer,
    cube_array_supported: bool,
    movers: Vec<UploadedMoverDraw>,
    mover_lookup: HashMap<u32, usize>,
    active_draws: Vec<ActiveMoverDraw>,
    instance_bytes: Vec<u8>,
}

const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/kinematic_brush.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample.wgsl"),
);

fn shader_source(cube_array_supported: bool) -> std::borrow::Cow<'static, str> {
    if cube_array_supported {
        std::borrow::Cow::Borrowed(SHADER_SOURCE)
    } else {
        std::borrow::Cow::Owned(crate::render::strip_point_shadow_cube(SHADER_SOURCE))
    }
}

fn build_instance_entry(model: glam::Mat4) -> [u8; INSTANCE_ENTRY_SIZE] {
    let mut bytes = [0u8; INSTANCE_ENTRY_SIZE];
    for (i, value) in model.to_cols_array().iter().enumerate() {
        let offset = i * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn build_light_params_bytes(params: KinematicLightParams) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&params.light_count.to_ne_bytes());
    bytes[4..8].copy_from_slice(&params.time.to_ne_bytes());
    bytes[8..12].copy_from_slice(&params.lighting_isolation.to_ne_bytes());
    bytes[12..16].copy_from_slice(&params.ambient_floor.to_ne_bytes());
    bytes
}

fn light_bind_group_layout_entries(cube_array_supported: bool) -> Vec<wgpu::BindGroupLayoutEntry> {
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![
        storage_entry(0),
        storage_entry(1),
        storage_entry(2),
        storage_entry(3),
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(
                    crate::lighting::spot_shadow::LIGHT_SPACE_MATRICES_SIZE,
                ),
            },
            count: None,
        },
    ];
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            },
            count: None,
        });
    }
    entries
}

fn pack_vertex(
    vertex: &postretro_level_format::geometry::Vertex,
) -> postretro_render_data::geometry::WorldVertex {
    postretro_render_data::geometry::WorldVertex {
        position: vertex.position,
        base_uv: vertex.uv,
        normal_oct: vertex.normal_oct,
        tangent_packed: vertex.tangent_packed,
        lightmap_uv: vertex.lightmap_uv,
        lightmap_layer: vertex.lightmap_layer as u32,
    }
}

fn derive_material_ranges(
    indices: &[u32],
    face_meta: &[postretro_level_format::geometry::FaceMeta],
    material_count: usize,
) -> Vec<MaterialRange> {
    let mut ranges = Vec::new();
    let mut offset = 0usize;
    for face in face_meta {
        if offset + 2 >= indices.len() {
            break;
        }
        let face_base = indices[offset];
        let start = offset;
        while offset + 2 < indices.len() && indices[offset] == face_base {
            offset += 3;
        }
        if offset == start {
            break;
        }
        push_or_merge_material_range(
            &mut ranges,
            material_index(face.texture_index, material_count),
            start as u32,
            (offset - start) as u32,
        );
    }
    if offset < indices.len() {
        push_or_merge_material_range(
            &mut ranges,
            0,
            offset as u32,
            (indices.len() - offset) as u32,
        );
    }
    ranges
}

fn push_or_merge_material_range(
    ranges: &mut Vec<MaterialRange>,
    material_index: usize,
    index_start: u32,
    index_count: u32,
) {
    if index_count == 0 {
        return;
    }
    if let Some(last) = ranges.last_mut() {
        if last.material_index == material_index
            && last.index_start + last.index_count == index_start
        {
            last.index_count += index_count;
            return;
        }
    }
    ranges.push(MaterialRange {
        material_index,
        index_start,
        index_count,
    });
}

fn material_index(texture_index: u32, material_count: usize) -> usize {
    if texture_index == postretro_level_format::geometry::NO_TEXTURE {
        return 0;
    }
    let index = texture_index as usize;
    if index < material_count { index } else { 0 }
}

impl KinematicBrushPass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        sh_volume_bgl: &wgpu::BindGroupLayout,
        cube_array_supported: bool,
    ) -> Self {
        let source = shader_source(cube_array_supported);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Kinematic Brush Shader"),
            source: wgpu::ShaderSource::Wgsl(source.as_ref().into()),
        });

        let light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kinematic Brush Light BGL (group 2)"),
                entries: &light_bind_group_layout_entries(cube_array_supported),
            });
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kinematic Brush Instance BGL"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Kinematic Brush Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(&light_bind_group_layout),
                Some(&instance_bind_group_layout),
                Some(sh_volume_bgl),
            ],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Kinematic Brush Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
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
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Vertex Buffer"),
            contents: &[0u8; postretro_render_data::geometry::WorldVertex::STRIDE],
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Index Buffer"),
            contents: &[0u8; 4],
            usage: wgpu::BufferUsages::INDEX,
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinematic Brush Instance Buffer"),
            size: INSTANCE_ENTRY_SIZE as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kinematic Brush Instance Bind Group"),
            layout: &instance_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: instance_buffer.as_entire_binding(),
            }],
        });
        let light_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kinematic Brush Light Params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            index_count: 0,
            instance_buffer,
            instance_capacity: 1,
            instance_bind_group_layout,
            instance_bind_group,
            light_bind_group_layout,
            light_bind_group: None,
            light_params_buffer,
            cube_array_supported,
            movers: Vec::new(),
            mover_lookup: HashMap::new(),
            active_draws: Vec::new(),
            instance_bytes: Vec::new(),
        }
    }

    pub fn install_geometry(
        &mut self,
        device: &wgpu::Device,
        geometry: Option<&postretro_level_loader::KinematicGeometry>,
        material_count: usize,
    ) {
        self.movers.clear();
        self.mover_lookup.clear();
        self.active_draws.clear();

        let Some(geometry) = geometry else {
            self.install_empty_geometry(device);
            return;
        };

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for mover in &geometry.movers {
            if mover.vertices.is_empty() || mover.indices.is_empty() {
                continue;
            }

            let vertex_base = vertices.len() as u32;
            let index_base = indices.len() as u32;
            vertices.extend(mover.vertices.iter().map(pack_vertex));
            indices.extend(mover.indices.iter().map(|index| vertex_base + *index));

            let mut ranges =
                derive_material_ranges(&mover.indices, &mover.face_meta, material_count);
            for range in &mut ranges {
                range.index_start += index_base;
            }
            if ranges.is_empty() {
                ranges.push(MaterialRange {
                    material_index: 0,
                    index_start: index_base,
                    index_count: mover.indices.len() as u32,
                });
            }
            let draw_index = self.movers.len();
            self.mover_lookup.insert(mover.mover_id, draw_index);
            self.movers.push(UploadedMoverDraw {
                mover_id: mover.mover_id,
                material_ranges: ranges,
            });
        }

        if vertices.is_empty() || indices.is_empty() {
            self.install_empty_geometry(device);
            return;
        }

        let vertex_bytes = cast_world_vertices_to_bytes(&vertices);
        let index_bytes = bytemuck_cast_slice_u32(&indices);
        self.vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Vertex Buffer"),
            contents: &vertex_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Index Buffer"),
            contents: &index_bytes,
            usage: wgpu::BufferUsages::INDEX,
        });
        self.index_count = indices.len() as u32;
    }

    fn install_empty_geometry(&mut self, device: &wgpu::Device) {
        self.vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Vertex Buffer"),
            contents: &[0u8; postretro_render_data::geometry::WorldVertex::STRIDE],
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Kinematic Brush Index Buffer"),
            contents: &[0u8; 4],
            usage: wgpu::BufferUsages::INDEX,
        });
        self.index_count = 0;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_light_bind_group(
        &mut self,
        device: &wgpu::Device,
        lights: &wgpu::Buffer,
        influence: &wgpu::Buffer,
        scripted_descriptors: &wgpu::Buffer,
        anim_samples: &wgpu::Buffer,
        spot_shadow_depth: &wgpu::TextureView,
        spot_shadow_compare: &wgpu::Sampler,
        light_space_matrices: &wgpu::Buffer,
        point_shadow_cube: Option<&wgpu::TextureView>,
    ) {
        assert_eq!(
            point_shadow_cube.is_some(),
            self.cube_array_supported,
            "kinematic brush group-2 cube view must be Some iff the BGL carries binding 8",
        );
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: influence.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: scripted_descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: self.light_params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(spot_shadow_depth),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(spot_shadow_compare),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: light_space_matrices.as_entire_binding(),
            },
        ];
        if let Some(cube_view) = point_shadow_cube {
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(cube_view),
            });
        }
        self.light_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Kinematic Brush Light Bind Group (group 2)"),
            layout: &self.light_bind_group_layout,
            entries: &entries,
        }));
    }

    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[KinematicMoverInstance],
    ) {
        self.active_draws.clear();
        self.instance_bytes.clear();

        for instance in instances {
            let Some(&mover_draw_index) = self.mover_lookup.get(&instance.mover_id) else {
                continue;
            };
            let instance_index = (self.instance_bytes.len() / INSTANCE_ENTRY_SIZE) as u32;
            self.instance_bytes
                .extend_from_slice(&build_instance_entry(instance.transform));
            self.active_draws.push(ActiveMoverDraw {
                mover_draw_index,
                instance_index,
            });
        }

        if self.instance_bytes.is_empty() {
            return;
        }
        let instance_count = self.instance_bytes.len() / INSTANCE_ENTRY_SIZE;
        if instance_count > self.instance_capacity {
            self.instance_capacity = instance_count.next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Kinematic Brush Instance Buffer"),
                size: (self.instance_capacity * INSTANCE_ENTRY_SIZE) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kinematic Brush Instance Bind Group"),
                layout: &self.instance_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.instance_buffer.as_entire_binding(),
                }],
            });
        }
        queue.write_buffer(&self.instance_buffer, 0, &self.instance_bytes);
    }

    pub fn write_light_params(
        &self,
        queue: &wgpu::Queue,
        light_count: u32,
        time: f32,
        lighting_isolation: u32,
        ambient_floor: f32,
    ) {
        queue.write_buffer(
            &self.light_params_buffer,
            0,
            &build_light_params_bytes(KinematicLightParams {
                light_count,
                time,
                lighting_isolation,
                ambient_floor,
            }),
        );
    }

    pub fn has_draws(&self) -> bool {
        self.index_count > 0 && !self.active_draws.is_empty()
    }

    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, materials: &[GpuTexture]) {
        if !self.has_draws() {
            return;
        }
        let light_bind_group = self
            .light_bind_group
            .as_ref()
            .expect("kinematic brush light bind group must be built before recording draws");
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, light_bind_group, &[]);
        pass.set_bind_group(3, &self.instance_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        for active in &self.active_draws {
            let mover = &self.movers[active.mover_draw_index];
            let instance_range = active.instance_index..active.instance_index + 1;
            for range in &mover.material_ranges {
                if range.index_count == 0 {
                    continue;
                }
                let material = materials
                    .get(range.material_index)
                    .or_else(|| materials.first())
                    .expect("renderer always keeps a placeholder material bind group");
                pass.set_bind_group(1, &material.bind_group, &[]);
                pass.draw_indexed(
                    range.index_start..range.index_start + range.index_count,
                    0,
                    instance_range.clone(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::{FaceMeta, Vertex};

    fn vertex(lightmap_uv: [f32; 2], lightmap_layer: u16) -> Vertex {
        Vertex::new(
            [1.0, 2.0, 3.0],
            [4.0, 5.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            lightmap_uv,
            lightmap_layer,
        )
    }

    #[test]
    fn pack_vertex_consumes_zeroed_lightmap_fields_without_rewriting_them() {
        let packed = pack_vertex(&vertex([0.0, 0.0], 0));
        assert_eq!(packed.position, [1.0, 2.0, 3.0]);
        assert_eq!(packed.base_uv, [4.0, 5.0]);
        assert_eq!(packed.lightmap_uv, [0, 0]);
        assert_eq!(packed.lightmap_layer, 0);
    }

    #[test]
    fn material_ranges_preserve_face_texture_order_and_index_spans() {
        let indices = [0, 1, 2, 0, 2, 3, 4, 5, 6];
        let face_meta = [
            FaceMeta {
                leaf_index: 0,
                texture_index: 2,
            },
            FaceMeta {
                leaf_index: 0,
                texture_index: 1,
            },
        ];
        let ranges = derive_material_ranges(&indices, &face_meta, 4);
        assert_eq!(
            ranges,
            vec![
                MaterialRange {
                    material_index: 2,
                    index_start: 0,
                    index_count: 6,
                },
                MaterialRange {
                    material_index: 1,
                    index_start: 6,
                    index_count: 3,
                },
            ],
        );
    }

    #[test]
    fn material_ranges_merge_adjacent_faces_with_same_material() {
        let indices = [0, 1, 2, 3, 4, 5];
        let face_meta = [
            FaceMeta {
                leaf_index: 0,
                texture_index: 1,
            },
            FaceMeta {
                leaf_index: 0,
                texture_index: 1,
            },
        ];
        assert_eq!(
            derive_material_ranges(&indices, &face_meta, 2),
            vec![MaterialRange {
                material_index: 1,
                index_start: 0,
                index_count: 6,
            }],
        );
    }

    #[test]
    fn material_range_uses_placeholder_for_missing_or_out_of_range_texture() {
        assert_eq!(
            material_index(postretro_level_format::geometry::NO_TEXTURE, 3),
            0
        );
        assert_eq!(material_index(9, 3), 0);
        assert_eq!(material_index(2, 3), 2);
    }

    #[test]
    fn kinematic_pass_uses_existing_bind_group_budget() {
        assert_eq!(KINEMATIC_BIND_GROUP_COUNT, 5);
        assert!(KINEMATIC_BIND_GROUP_COUNT <= 8);
    }

    #[test]
    fn kinematic_light_layout_keeps_storage_fragment_only() {
        let entries = light_bind_group_layout_entries(true);
        let storage_count = entries
            .iter()
            .filter(|entry| {
                entry.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                    && matches!(
                        entry.ty,
                        wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { .. },
                            ..
                        }
                    )
            })
            .count();
        assert_eq!(storage_count, 4);
        assert!(
            entries
                .iter()
                .all(|entry| !entry.visibility.contains(wgpu::ShaderStages::VERTEX)),
            "runtime-light bindings must not spend vertex-stage storage budget",
        );
    }
}
