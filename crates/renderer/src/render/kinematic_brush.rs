// Renderer-owned draw path for PRL-loaded kinematic brush movers.
// See: context/lib/rendering_pipeline.md

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use super::*;

const INSTANCE_ENTRY_SIZE: usize = 64;
const KINEMATIC_LIGHT_PARAMS_SIZE: usize = 32;
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
    light_term_mask: u32,
    ambient_floor: f32,
    dynamic_light_count: u32,
    _pad: [u32; 3],
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

/// Full shared-index-buffer span for one uploaded mover. Unlike the material
/// ranges, this is a single draw range suitable for the depth-only path, which
/// does not bind or batch by material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoverIndexRange {
    pub(crate) index_start: u32,
    pub(crate) index_count: u32,
}

/// One active mover's dense transform-buffer index. `mover_draw_index` is the
/// stable key for geometry metadata; it deliberately is not the game-side
/// mover id, which the depth recorder resolves through `KinematicBrushPass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveMoverDraw {
    pub(crate) mover_draw_index: usize,
    pub(crate) instance_index: u32,
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
    /// Every present mover transform, including camera-PVS-culled shadow casters.
    active_draws: Vec<ActiveMoverDraw>,
    active_draw_lookup: HashMap<usize, ActiveMoverDraw>,
    /// Camera-visible subset of `active_draws` for the beauty pass.
    beauty_draws: Vec<ActiveMoverDraw>,
    mover_index_ranges: Vec<MoverIndexRange>,
    instance_bytes: Vec<u8>,
}

const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/kinematic_brush.wgsl"),
    "\n",
    include_str!("../shaders/material_shading.wgsl"),
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
    include_str!("../shaders/shadow_sample.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample_static_cache.wgsl"),
    "\n",
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

/// Serialize the 32-byte uniform row shared with `KinematicLightParams` in
/// `kinematic_brush.wgsl`. The dynamic-tier count sits at byte 16; the tail
/// stays explicit padding so the next uniform row remains 16-byte aligned.
fn build_light_params_bytes(params: KinematicLightParams) -> [u8; KINEMATIC_LIGHT_PARAMS_SIZE] {
    let mut bytes = [0u8; KINEMATIC_LIGHT_PARAMS_SIZE];
    bytes[0..4].copy_from_slice(&params.light_count.to_ne_bytes());
    bytes[4..8].copy_from_slice(&params.time.to_ne_bytes());
    bytes[8..12].copy_from_slice(&params.light_term_mask.to_ne_bytes());
    bytes[12..16].copy_from_slice(&params.ambient_floor.to_ne_bytes());
    bytes[16..20].copy_from_slice(&params.dynamic_light_count.to_ne_bytes());
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
                min_binding_size: std::num::NonZeroU64::new(KINEMATIC_LIGHT_PARAMS_SIZE as u64),
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
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 9,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 10,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
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
                    format: SCENE_COLOR_FORMAT,
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
            size: KINEMATIC_LIGHT_PARAMS_SIZE as wgpu::BufferAddress,
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
            active_draw_lookup: HashMap::new(),
            beauty_draws: Vec::new(),
            mover_index_ranges: Vec::new(),
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
        self.active_draw_lookup.clear();
        self.beauty_draws.clear();
        self.mover_index_ranges.clear();

        let Some(geometry) = geometry else {
            self.install_empty_geometry(device);
            return;
        };
        // This map is rebuilt every render collection; reserve at level install
        // so shadow recording does not trigger a per-frame reallocation.
        self.active_draw_lookup.reserve(geometry.movers.len());
        self.beauty_draws.reserve(geometry.movers.len());

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
            self.mover_index_ranges.push(MoverIndexRange {
                index_start: index_base,
                index_count: mover.indices.len() as u32,
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

    /// Shared position-bearing vertex buffer. The rigid depth path consumes
    /// only location 0 while retaining this buffer's world-vertex stride.
    pub(crate) fn shared_vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    /// Shared geometry index buffer used by both mover beauty and depth draws.
    pub(crate) fn shared_index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }

    /// Layout for the uploaded per-instance model transforms. The rigid depth
    /// pipeline keeps it at group 1, separate from the beauty path's group 3.
    pub(crate) fn instance_transform_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.instance_bind_group_layout
    }

    /// Existing per-instance model-transform binding populated by
    /// [`Self::upload_instances`] before shadow recording.
    pub(crate) fn instance_transform_bind_group(&self) -> &wgpu::BindGroup {
        &self.instance_bind_group
    }

    /// Dense all-present mover instances, keyed by `mover_draw_index` rather
    /// than the game-side mover id. Shadow depth reads this list; beauty draws
    /// use the camera-visible subset held separately.
    pub(crate) fn active_draws(&self) -> &[ActiveMoverDraw] {
        &self.active_draws
    }

    /// One full index span per uploaded mover, keyed by `mover_draw_index`.
    pub(crate) fn mover_index_ranges(&self) -> &[MoverIndexRange] {
        &self.mover_index_ranges
    }

    /// Resolve a game-side mover id through this pass's uploaded mover list to
    /// the draw index that keys active instances and full index spans.
    pub(crate) fn mover_draw_index_for_mover_id(&self, mover_id: u32) -> Option<usize> {
        self.mover_lookup.get(&mover_id).copied()
    }

    /// Resolve an active transform-buffer record by its geometry draw index.
    /// External mover ids must first pass through [`Self::mover_draw_index_for_mover_id`].
    pub(crate) fn active_draw_for_mover_draw_index(
        &self,
        mover_draw_index: usize,
    ) -> Option<ActiveMoverDraw> {
        self.active_draw_lookup.get(&mover_draw_index).copied()
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
        promoted_spot_cache: &wgpu::TextureView,
        promoted_cube_cache: Option<&wgpu::TextureView>,
    ) {
        assert_eq!(
            point_shadow_cube.is_some(),
            self.cube_array_supported,
            "kinematic brush group-2 cube view must be Some iff the BGL carries binding 8",
        );
        assert_eq!(
            promoted_cube_cache.is_some(),
            self.cube_array_supported,
            "kinematic brush group-2 promoted cube cache must be Some iff the BGL carries binding 10",
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
        entries.push(wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(promoted_spot_cache),
        });
        if let Some(cube_view) = promoted_cube_cache {
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
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
        beauty_instances: &[KinematicMoverInstance],
        shadow_instances: &[KinematicMoverInstance],
    ) {
        self.active_draws.clear();
        self.active_draw_lookup.clear();
        self.beauty_draws.clear();
        self.instance_bytes.clear();

        for instance in shadow_instances {
            let Some(&mover_draw_index) = self.mover_lookup.get(&instance.mover_id) else {
                continue;
            };
            let instance_index = (self.instance_bytes.len() / INSTANCE_ENTRY_SIZE) as u32;
            self.instance_bytes
                .extend_from_slice(&build_instance_entry(instance.transform));
            let active_draw = ActiveMoverDraw {
                mover_draw_index,
                instance_index,
            };
            self.active_draws.push(active_draw);
            // A mover id has one external AABB. Keep its first packed instance
            // so the depth recorder preserves the former `iter().find()` behavior
            // if malformed input ever duplicates that id.
            self.active_draw_lookup
                .entry(mover_draw_index)
                .or_insert(active_draw);
        }

        for instance in beauty_instances {
            let Some(&mover_draw_index) = self.mover_lookup.get(&instance.mover_id) else {
                continue;
            };
            let Some(active_draw) = self.active_draw_lookup.get(&mover_draw_index) else {
                continue;
            };
            self.beauty_draws.push(*active_draw);
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
        dynamic_light_count: u32,
        time: f32,
        light_term_mask: u32,
        ambient_floor: f32,
    ) {
        queue.write_buffer(
            &self.light_params_buffer,
            0,
            &build_light_params_bytes(KinematicLightParams {
                light_count,
                dynamic_light_count,
                time,
                light_term_mask,
                ambient_floor,
                _pad: [0; 3],
            }),
        );
    }

    pub fn has_draws(&self) -> bool {
        self.index_count > 0 && !self.beauty_draws.is_empty()
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

        for active in &self.beauty_draws {
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

    const POST_RETRO_SAMPLING_CALLS: &[&str] = &[
        "textureDimensions",
        "floor",
        "max",
        "fwidth",
        "clamp",
        "textureSampleGrad",
    ];

    fn extract_wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = format!("fn {name}");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("shader should declare fn {name}"));
        let body_start = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("fn {name} should have a body"));
        let mut depth = 0i32;
        for (offset, ch) in source[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..body_start + offset + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("fn {name} should close its body");
    }

    fn wgsl_call_fingerprint(source: &str, keep: &[&str]) -> Vec<String> {
        let source = strip_wgsl_comments(source);
        let mut calls = Vec::new();
        for (start, ident) in wgsl_identifiers(&source) {
            let after_ident = &source[start + ident.len()..];
            if !after_ident.trim_start().starts_with('(') {
                continue;
            }
            if keep.contains(&ident) {
                calls.push(ident.to_owned());
            }
        }
        calls
    }

    fn strip_wgsl_comments(source: &str) -> String {
        let mut stripped = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '/' {
                stripped.push(ch);
                continue;
            }
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for comment_ch in chars.by_ref() {
                        if comment_ch == '\n' {
                            stripped.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for comment_ch in chars.by_ref() {
                        if comment_ch == '\n' {
                            stripped.push('\n');
                        }
                        if prev == '*' && comment_ch == '/' {
                            break;
                        }
                        prev = comment_ch;
                    }
                }
                _ => stripped.push(ch),
            }
        }
        stripped
    }

    fn wgsl_identifiers(source: &str) -> Vec<(usize, &str)> {
        let mut idents = Vec::new();
        let mut iter = source.char_indices().peekable();
        while let Some((start, ch)) = iter.next() {
            if !is_wgsl_ident_start(ch) {
                continue;
            }
            let mut end = start + ch.len_utf8();
            while let Some(&(next, next_ch)) = iter.peek() {
                if !is_wgsl_ident_continue(next_ch) {
                    break;
                }
                iter.next();
                end = next + next_ch.len_utf8();
            }
            idents.push((start, &source[start..end]));
        }
        idents
    }

    fn is_wgsl_ident_start(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphabetic()
    }

    fn is_wgsl_ident_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }

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
    fn kinematic_brush_shader_matches_forward_post_retro_sampling() {
        let forward = include_str!("../shaders/forward.wgsl");
        let kinematic = include_str!("../shaders/kinematic_brush.wgsl");

        assert_eq!(
            wgsl_call_fingerprint(
                extract_wgsl_fn(kinematic, "sample_post_retro"),
                POST_RETRO_SAMPLING_CALLS,
            ),
            wgsl_call_fingerprint(
                extract_wgsl_fn(forward, "sample_post_retro"),
                POST_RETRO_SAMPLING_CALLS,
            ),
            "kinematic brush movers must keep the same Post Retro sampling operations as static world geometry",
        );

        let fs_calls = wgsl_call_fingerprint(
            extract_wgsl_fn(kinematic, "fs_main"),
            &["dpdx", "dpdy", "sample_post_retro", "textureSample"],
        );
        let ddx_index = fs_calls
            .iter()
            .position(|call| call == "dpdx")
            .expect("kinematic brush fragment shader must compute a UV ddx");
        let ddy_index = fs_calls
            .iter()
            .position(|call| call == "dpdy")
            .expect("kinematic brush fragment shader must compute a UV ddy");
        let sample_index = fs_calls
            .iter()
            .position(|call| call == "sample_post_retro")
            .expect("kinematic brush base texture must sample through the Post Retro helper");
        assert!(
            ddx_index < sample_index && ddy_index < sample_index,
            "kinematic brush fragment shader must compute UV derivatives before sampling",
        );
        assert!(
            !fs_calls.iter().any(|call| call == "textureSample"),
            "kinematic brush fragment shader must not bypass the Post Retro helper",
        );
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
        const { assert!(KINEMATIC_BIND_GROUP_COUNT <= 8) };
    }

    #[test]
    fn kinematic_pipeline_fragment_texture_budget_includes_emissive() {
        let total = |cube_array_supported| {
            let per_group = [
                fragment_sampled_textures(&uniform_bind_group_layout_entries()),
                fragment_sampled_textures(&material_bind_group_layout_entries()),
                fragment_sampled_textures(&light_bind_group_layout_entries(cube_array_supported)),
                0, // group 3 instance storage buffer
                fragment_sampled_textures(&sh_volume::sh_bind_group_layout_entries()),
            ];
            (per_group, per_group.iter().sum::<u32>())
        };

        let (cube_groups, cube_total) = total(true);
        assert_eq!(cube_groups, [0, 4, 4, 0, 3]);
        assert_eq!(cube_total, 11);
        assert!(cube_total <= 16);

        let (no_cube_groups, no_cube_total) = total(false);
        assert_eq!(no_cube_groups, [0, 4, 2, 0, 3]);
        assert_eq!(no_cube_total, 9);
        assert!(no_cube_total <= 16);
    }

    #[test]
    fn kinematic_material_uniform_mirrors_emissive_layout() {
        let shader = include_str!("../shaders/kinematic_brush.wgsl");
        assert!(shader.contains("shininess: f32,"));
        assert!(shader.contains("emissive_strength: f32,"));
        assert!(shader.contains("emissive_texture"));
        assert!(
            shader.contains("base_color.rgb * lighting + emissive * material.emissive_strength")
        );
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

    #[test]
    fn kinematic_light_layout_requires_32_byte_params_uniform() {
        let entries = light_bind_group_layout_entries(true);
        let params_entry = entries
            .iter()
            .find(|entry| entry.binding == 4)
            .expect("kinematic light layout must bind params at binding 4");
        let wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset,
            min_binding_size,
        } = params_entry.ty
        else {
            panic!("kinematic light binding 4 must be a uniform buffer");
        };

        assert_eq!(ty, wgpu::BufferBindingType::Uniform);
        assert!(!has_dynamic_offset);
        assert_eq!(
            min_binding_size.map(std::num::NonZeroU64::get),
            Some(KINEMATIC_LIGHT_PARAMS_SIZE as u64),
        );
    }

    #[test]
    fn kinematic_light_params_uploads_dynamic_tier_count_in_second_uniform_row() {
        let bytes = build_light_params_bytes(KinematicLightParams {
            light_count: 11,
            time: 1.5,
            light_term_mask: 0x7F,
            ambient_floor: 0.375,
            dynamic_light_count: 7,
            _pad: [0; 3],
        });

        assert_eq!(bytes.len(), KINEMATIC_LIGHT_PARAMS_SIZE);
        assert_eq!(bytes[0..4], 11u32.to_ne_bytes());
        assert_eq!(bytes[4..8], 1.5f32.to_ne_bytes());
        assert_eq!(bytes[8..12], 0x7Fu32.to_ne_bytes());
        assert_eq!(bytes[12..16], 0.375f32.to_ne_bytes());
        assert_eq!(bytes[16..20], 7u32.to_ne_bytes());
        assert_eq!(bytes[20..], [0; 12]);
    }

    #[test]
    fn kinematic_light_params_wgsl_layout_matches_rust_upload() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("composed kinematic brush shader should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("composed kinematic brush shader should pass Naga validation");

        let (span, members) = module
            .types
            .iter()
            .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
                (Some(name), naga::TypeInner::Struct { span, members, .. })
                    if name == "KinematicLightParams" =>
                {
                    Some((*span, members))
                }
                _ => None,
            })
            .expect("kinematic brush shader should declare KinematicLightParams");

        assert_eq!(span as usize, KINEMATIC_LIGHT_PARAMS_SIZE);
        assert_eq!(
            members
                .iter()
                .map(|member| member.name.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("light_count"),
                Some("time"),
                Some("light_term_mask"),
                Some("ambient_floor"),
                Some("dynamic_light_count"),
                Some("_pad0"),
                Some("_pad1"),
                Some("_pad2"),
            ],
        );
        assert_eq!(
            members
                .iter()
                .map(|member| member.offset)
                .collect::<Vec<_>>(),
            vec![0, 4, 8, 12, 16, 20, 24, 28],
        );
    }

    #[test]
    fn kinematic_shader_source_validates_without_cube_arrays() {
        let source = shader_source(false);
        for declaration in [
            "@group(2) @binding(8) var point_shadow_cube",
            "@group(2) @binding(10) var promoted_cube_depth_cache",
        ] {
            assert!(
                !source.contains(declaration),
                "no-cube kinematic source must strip cube binding: {declaration}",
            );
        }
        let module = naga::front::wgsl::parse_str(&source)
            .expect("no-cube kinematic brush shader should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("no-cube kinematic brush shader should pass Naga validation");
    }

    #[test]
    fn kinematic_light_bgl_matches_both_cube_variants() {
        let no_cube_bindings: Vec<u32> = light_bind_group_layout_entries(false)
            .iter()
            .map(|entry| entry.binding)
            .collect();
        assert_eq!(no_cube_bindings, vec![0, 1, 2, 3, 4, 5, 6, 7, 9]);

        let cube = light_bind_group_layout_entries(true);
        let cube_bindings: Vec<u32> = cube.iter().map(|entry| entry.binding).collect();
        assert_eq!(cube_bindings, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(matches!(
            cube[9].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            }
        ));
        assert!(matches!(
            cube[10].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            }
        ));
    }

    #[test]
    fn kinematic_shader_uses_shared_material_helpers_for_promoted_static_specular() {
        assert!(
            SHADER_SOURCE.contains("fn blinn_phong(")
                && SHADER_SOURCE.contains("fn sample_normal(")
                && SHADER_SOURCE.contains("fn reconstruct_tbn_normal("),
            "the composed mover shader must append the shared material shading snippet",
        );

        let dynamic_loop = extract_wgsl_fn(
            include_str!("../shaders/kinematic_brush.wgsl"),
            "accumulate_dynamic_direct",
        );
        assert!(
            dynamic_loop.contains(
                "if use_specular && i >= kinematic_light_params.dynamic_light_count && n_dot_l > 0.0"
            ),
            "only front-lit promoted records with LightTermMask specular enabled may add mover specular",
        );
        let mover_src = include_str!("../shaders/kinematic_brush.wgsl");
        assert!(
            mover_src.contains("let use_dynamic = (light_terms & 0x20u) != 0u;")
                && mover_src.contains("let use_specular = (light_terms & 0x40u) != 0u;"),
            "mover dynamic and specular gates must derive independently from LightTermMask bits 5 and 6",
        );
        assert!(
            dynamic_loop.contains(
                "blinn_phong(L, V, n, effective_color, spec_exp, spec_int) * attenuation"
            ),
            "promoted mover specular must retain the runtime attenuation, cone, shadow, and promotion color factors",
        );
    }

    #[test]
    fn kinematic_animated_descriptors_are_limited_to_dynamic_prefix() {
        let dynamic_loop = extract_wgsl_fn(
            include_str!("../shaders/kinematic_brush.wgsl"),
            "accumulate_dynamic_direct",
        );
        assert!(
            dynamic_loop.contains("if i < kinematic_light_params.dynamic_light_count {")
                && dynamic_loop.contains("let scripted_desc = scripted_light_descriptors[i];"),
            "promoted static records append after the descriptor-upload prefix and must not read stale descriptor tail bytes",
        );
    }
}
