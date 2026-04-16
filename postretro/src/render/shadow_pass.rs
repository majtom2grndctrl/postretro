// Shadow pass GPU resources: textures, pipelines, and per-frame rendering.
// See: context/plans/in-progress/lighting-foundation/5-shadow-maps.md

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::lighting::shadow::{
    self, CSM_CASCADE_COUNT, CSM_RESOLUTION, CSM_TOTAL_LAYERS, CUBE_FACES,
    MAX_POINT_SHADOW_LIGHTS, POINT_SHADOW_RESOLUTION, SHADOW_KIND_CSM,
    SHADOW_KIND_CUBE, SHADOW_KIND_SPOT_2D, SPOT_SHADOW_RESOLUTION,
    MAX_SPOT_SHADOW_LIGHTS, ShadowAssignment, ShadowSlot, ShadowSlotPool,
};
use crate::prl::MapLight;

/// Depth format for all shadow map textures.
const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/shadow_depth.wgsl");
const SHADOW_POINT_SHADER_SOURCE: &str = include_str!("../shaders/shadow_depth_point.wgsl");

/// Total layers in the point shadow 2D array (6 faces per slot).
const POINT_TOTAL_LAYERS: u32 = (MAX_POINT_SHADOW_LIGHTS * CUBE_FACES) as u32;

/// All GPU-side shadow map resources: textures, views, pipelines, uniform buffers.
pub struct ShadowResources {
    // --- Textures ---
    /// CSM cascade depth texture array (layers = CSM_TOTAL_LAYERS).
    #[allow(dead_code)] // Retained so the GPU texture is not dropped.
    csm_texture: wgpu::Texture,
    csm_layer_views: Vec<wgpu::TextureView>,
    pub csm_array_view: wgpu::TextureView,

    /// Point light shadow maps stored as a 2D array (6 layers per slot).
    #[allow(dead_code)] // Retained so the GPU texture is not dropped.
    point_texture: wgpu::Texture,
    /// Per-face views for rendering. Index: slot * 6 + face.
    point_face_views: Vec<wgpu::TextureView>,
    /// Full array view for fragment shader sampling.
    pub point_array_view: wgpu::TextureView,

    /// Spot light shadow map texture array.
    #[allow(dead_code)] // Retained so the GPU texture is not dropped.
    spot_texture: wgpu::Texture,
    spot_layer_views: Vec<wgpu::TextureView>,
    pub spot_array_view: wgpu::TextureView,

    // --- Comparison sampler ---
    pub shadow_sampler: wgpu::Sampler,

    // --- Pipelines ---
    shadow_pipeline: wgpu::RenderPipeline,
    point_shadow_pipeline: wgpu::RenderPipeline,

    // --- Uniform buffers ---
    shadow_uniform_buffer: wgpu::Buffer,
    shadow_uniform_bind_group: wgpu::BindGroup,
    point_params_buffer: wgpu::Buffer,
    point_uniform_bind_group: wgpu::BindGroup,

    // --- Storage buffers for fragment shader ---
    pub csm_vp_buffer: wgpu::Buffer,
    pub spot_vp_buffer: wgpu::Buffer,

    // --- Slot pool ---
    pub slot_pool: ShadowSlotPool,
}

impl ShadowResources {
    /// Create all shadow map textures, pipelines, and buffers at level load.
    pub fn new(
        device: &wgpu::Device,
        world_vertex_stride: u64,
    ) -> Self {
        // --- CSM texture ---
        let csm_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CSM Depth Array"),
            size: wgpu::Extent3d {
                width: CSM_RESOLUTION,
                height: CSM_RESOLUTION,
                depth_or_array_layers: CSM_TOTAL_LAYERS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let csm_layer_views: Vec<wgpu::TextureView> = (0..CSM_TOTAL_LAYERS as u32)
            .map(|layer| {
                csm_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("CSM Layer {layer}")),
                    format: Some(SHADOW_DEPTH_FORMAT),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        let csm_array_view = csm_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("CSM Array View"),
            format: Some(SHADOW_DEPTH_FORMAT),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // --- Point light shadow map: 2D array, 6 layers per slot ---
        let point_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Point Shadow 2D Array"),
            size: wgpu::Extent3d {
                width: POINT_SHADOW_RESOLUTION,
                height: POINT_SHADOW_RESOLUTION,
                depth_or_array_layers: POINT_TOTAL_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let point_face_views: Vec<wgpu::TextureView> = (0..POINT_TOTAL_LAYERS)
            .map(|layer| {
                point_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Point Shadow Face {layer}")),
                    format: Some(SHADOW_DEPTH_FORMAT),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        let point_array_view = point_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Point Shadow Array View"),
            format: Some(SHADOW_DEPTH_FORMAT),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // --- Spot shadow texture ---
        let spot_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Spot Shadow Array"),
            size: wgpu::Extent3d {
                width: SPOT_SHADOW_RESOLUTION,
                height: SPOT_SHADOW_RESOLUTION,
                depth_or_array_layers: MAX_SPOT_SHADOW_LIGHTS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let spot_layer_views: Vec<wgpu::TextureView> = (0..MAX_SPOT_SHADOW_LIGHTS as u32)
            .map(|layer| {
                spot_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("Spot Shadow Layer {layer}")),
                    format: Some(SHADOW_DEPTH_FORMAT),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        let spot_array_view = spot_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Spot Shadow Array View"),
            format: Some(SHADOW_DEPTH_FORMAT),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // --- Comparison sampler ---
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow Comparison Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        // --- Bind group layouts ---
        let shadow_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow Uniform BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let point_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Point Shadow Uniform BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // --- Uniform buffers ---
        let shadow_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shadow Uniform Buffer"),
            contents: &[0u8; 64],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shadow_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shadow Uniform BG"),
            layout: &shadow_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_uniform_buffer.as_entire_binding(),
            }],
        });

        let point_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Point Light Params Buffer"),
            contents: &[0u8; 32],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let point_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Point Shadow Uniform BG"),
            layout: &point_uniform_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: shadow_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: point_params_buffer.as_entire_binding(),
                },
            ],
        });

        // --- Shadow vertex layout: position only ---
        let shadow_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: world_vertex_stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            }],
        };

        // --- Directional/spot depth-only pipeline ---
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow Depth Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER_SOURCE.into()),
        });

        let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shadow Pipeline Layout"),
            bind_group_layouts: &[Some(&shadow_uniform_bgl)],
            immediate_size: 0,
        });

        // Depth bias: starting values for Depth32Float. Tuned by observation.
        // See sub-plan 5 §Notes for rationale.
        let shadow_bias = wgpu::DepthBiasState {
            constant: 4,
            slope_scale: 2.0,
            clamp: 0.0,
        };

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Shadow Depth Pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                buffers: std::slice::from_ref(&shadow_vertex_layout),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: shadow_bias,
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        // --- Point light pipeline (linear depth fragment shader) ---
        let point_shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Point Shadow Depth Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_POINT_SHADER_SOURCE.into()),
        });

        let point_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Point Shadow Pipeline Layout"),
            bind_group_layouts: &[Some(&point_uniform_bgl)],
            immediate_size: 0,
        });

        let point_shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Point Shadow Pipeline"),
            layout: Some(&point_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &point_shadow_shader,
                entry_point: Some("vs_main"),
                buffers: &[shadow_vertex_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                // Point lights write linear depth via frag_depth, so hardware
                // depth bias has no effect on the stored value. Bias is applied
                // shader-side in sample_point_shadow instead.
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &point_shadow_shader,
                entry_point: Some("fs_main"),
                targets: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // --- Storage buffers for CSM and spot VP matrices ---
        let csm_vp_buf_size = (CSM_TOTAL_LAYERS * 64).max(64);
        let csm_vp_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("CSM ViewProj Storage"),
            contents: &vec![0u8; csm_vp_buf_size],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let spot_vp_buf_size = (MAX_SPOT_SHADOW_LIGHTS * 64).max(64);
        let spot_vp_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spot ViewProj Storage"),
            contents: &vec![0u8; spot_vp_buf_size],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            csm_texture,
            csm_layer_views,
            csm_array_view,
            point_texture,
            point_face_views,
            point_array_view,
            spot_texture,
            spot_layer_views,
            spot_array_view,
            shadow_sampler,
            shadow_pipeline,
            point_shadow_pipeline,
            shadow_uniform_buffer,
            shadow_uniform_bind_group,
            point_params_buffer,
            point_uniform_bind_group,
            csm_vp_buffer,
            spot_vp_buffer,
            slot_pool: ShadowSlotPool::new(),
        }
    }

    /// Encode shadow render passes for the current frame's slot assignment.
    #[allow(clippy::too_many_arguments)]
    pub fn render_shadow_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        assignment: &ShadowAssignment,
        lights: &[MapLight],
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
        camera_view_proj: Mat4,
        camera_near: f32,
        camera_far: f32,
    ) {
        let mut csm_matrices = vec![Mat4::IDENTITY; CSM_TOTAL_LAYERS];
        let mut spot_matrices = vec![Mat4::IDENTITY; MAX_SPOT_SHADOW_LIGHTS];

        for slot in &assignment.slots {
            let light = &lights[slot.light_index as usize];

            match slot.shadow_kind {
                SHADOW_KIND_CSM => {
                    self.render_csm_passes(
                        encoder, queue, slot, light, vertex_buffer, index_buffer,
                        index_count, camera_view_proj, camera_near, camera_far,
                        &mut csm_matrices,
                    );
                }
                SHADOW_KIND_CUBE => {
                    if !slot.cached {
                        self.render_point_shadow_passes(
                            encoder, queue, slot, light, vertex_buffer, index_buffer,
                            index_count,
                        );
                    }
                }
                SHADOW_KIND_SPOT_2D => {
                    let pos = light_pos(light);
                    let dir = light_dir(light);
                    let vp = shadow::spot_light_matrix(
                        pos, dir, light.cone_angle_outer, light.falloff_range,
                    );
                    spot_matrices[slot.pool_slot as usize] = vp;
                    if !slot.cached {
                        self.render_spot_shadow_pass(
                            encoder, queue, slot, light, vertex_buffer, index_buffer,
                            index_count, &vp,
                        );
                    }
                }
                _ => {}
            }
        }

        // Upload VP matrices for fragment shader sampling.
        queue.write_buffer(
            &self.csm_vp_buffer, 0,
            &shadow::pack_csm_view_proj_buffer(&csm_matrices),
        );
        queue.write_buffer(
            &self.spot_vp_buffer, 0,
            &shadow::pack_spot_view_proj_buffer(&spot_matrices),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_csm_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        slot: &ShadowSlot,
        light: &MapLight,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
        camera_view_proj: Mat4,
        camera_near: f32,
        camera_far: f32,
        csm_matrices: &mut [Mat4],
    ) {
        let light_direction = light_dir(light);
        let inv_vp = camera_view_proj.inverse();
        let splits = shadow::compute_cascade_splits(camera_near, camera_far, 0.5);

        for cascade in 0..CSM_CASCADE_COUNT {
            let split_near = if cascade == 0 { camera_near } else { splits[cascade - 1] };
            let split_far = splits[cascade];

            let light_vp = shadow::cascade_ortho_matrix(
                inv_vp, split_near, split_far, camera_near, camera_far, light_direction,
            );

            let layer_idx = slot.pool_slot as usize * CSM_CASCADE_COUNT + cascade;
            csm_matrices[layer_idx] = light_vp;

            write_mat4_to_buffer(queue, &self.shadow_uniform_buffer, &light_vp);

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&format!("CSM light={} cascade={cascade}", slot.light_index)),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.csm_layer_views[layer_idx],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.shadow_uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_point_shadow_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        slot: &ShadowSlot,
        light: &MapLight,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
    ) {
        let pos = light_pos(light);
        let range = light.falloff_range;
        let face_matrices = shadow::point_light_cube_matrices(pos, range);

        // Upload point light params.
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&pos.x.to_ne_bytes());
        params[4..8].copy_from_slice(&pos.y.to_ne_bytes());
        params[8..12].copy_from_slice(&pos.z.to_ne_bytes());
        params[12..16].copy_from_slice(&range.to_ne_bytes());
        queue.write_buffer(&self.point_params_buffer, 0, &params);

        let base_layer = slot.pool_slot as usize * CUBE_FACES;

        for (face, face_vp) in face_matrices.iter().enumerate().take(CUBE_FACES) {
            write_mat4_to_buffer(queue, &self.shadow_uniform_buffer, face_vp);

            let layer_idx = base_layer + face;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&format!("Point light={} face={face}", slot.light_index)),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.point_face_views[layer_idx],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.point_shadow_pipeline);
            pass.set_bind_group(0, &self.point_uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_spot_shadow_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        slot: &ShadowSlot,
        _light: &MapLight,
        vertex_buffer: &wgpu::Buffer,
        index_buffer: &wgpu::Buffer,
        index_count: u32,
        vp: &Mat4,
    ) {
        write_mat4_to_buffer(queue, &self.shadow_uniform_buffer, vp);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&format!("Spot light={}", slot.light_index)),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.spot_layer_views[slot.pool_slot as usize],
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.shadow_uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
}

fn light_pos(light: &MapLight) -> Vec3 {
    Vec3::new(
        light.origin[0] as f32,
        light.origin[1] as f32,
        light.origin[2] as f32,
    )
}

fn light_dir(light: &MapLight) -> Vec3 {
    Vec3::new(
        light.cone_direction[0],
        light.cone_direction[1],
        light.cone_direction[2],
    )
}

fn write_mat4_to_buffer(queue: &wgpu::Queue, buffer: &wgpu::Buffer, mat: &Mat4) {
    let mut bytes = [0u8; 64];
    for (i, &val) in mat.to_cols_array().iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&val.to_ne_bytes());
    }
    queue.write_buffer(buffer, 0, &bytes);
}
