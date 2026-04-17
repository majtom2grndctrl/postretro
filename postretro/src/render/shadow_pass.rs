// Shadow pass GPU resources: textures, pipelines, and per-frame rendering.
// See: context/plans/in-progress/lighting-foundation/5-shadow-maps.md
//
// Only CSM (directional / sun) shadows live here. Point and spot lights
// receive their shadow contribution from sub-plan 9 (SDF sphere-trace); no
// per-light shadow maps are allocated for them.

use std::num::NonZeroU64;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::lighting::shadow::{
    self, CSM_CASCADE_COUNT, CSM_RESOLUTION, CSM_TOTAL_LAYERS, SHADOW_KIND_CSM,
    ShadowAssignment, ShadowSlot, ShadowSlotPool,
};
use crate::prl::MapLight;

/// Depth format for all shadow map textures.
const SHADOW_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/shadow_depth.wgsl");

/// Byte size of one mat4x4<f32> (shadow view-projection).
const MAT4_SIZE: u64 = 64;

/// All GPU-side shadow map resources: textures, views, pipelines, uniform buffers.
pub struct ShadowResources {
    // --- Textures ---
    /// CSM cascade depth texture array (layers = CSM_TOTAL_LAYERS).
    #[allow(dead_code)] // Retained so the GPU texture is not dropped.
    csm_texture: wgpu::Texture,
    csm_layer_views: Vec<wgpu::TextureView>,
    pub csm_array_view: wgpu::TextureView,

    // --- Comparison sampler ---
    pub shadow_sampler: wgpu::Sampler,

    // --- Pipelines ---
    shadow_pipeline: wgpu::RenderPipeline,

    // --- Uniform buffers ---
    shadow_uniform_buffer: wgpu::Buffer,
    shadow_uniform_bind_group: wgpu::BindGroup,

    // --- Storage buffers for fragment shader ---
    pub csm_vp_buffer: wgpu::Buffer,

    // --- Slot pool ---
    pub slot_pool: ShadowSlotPool,

    // --- Dynamic-offset stride (padded to device alignment) ---
    uniform_stride: u32,
}

/// Compute offset index (0-based pass slot) for a CSM cascade layer.
#[inline]
fn csm_uniform_slot(layer: usize) -> u32 {
    layer as u32
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

        // --- Dynamic-offset alignment ---
        // Each cascade pass needs its own matrix region in a single wide
        // uniform buffer, addressed via dynamic offset. The per-slot stride
        // must be a multiple of the device's min uniform buffer alignment
        // (256 on most desktop backends).
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let uniform_stride = align.max(MAT4_SIZE);

        // --- Bind group layout ---
        let shadow_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow Uniform BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(MAT4_SIZE),
                },
                count: None,
            }],
        });

        // --- Uniform buffer ---
        // The shadow uniform buffer is wide enough to hold one mat4 per CSM
        // cascade slot, each region padded to `uniform_stride`. Each cascade
        // pass binds its own dynamic offset, so writes do not trample each
        // other before `submit()` flushes.
        let shadow_uniform_size = uniform_stride * CSM_TOTAL_LAYERS as u64;
        let shadow_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shadow Uniform Buffer"),
            contents: &vec![0u8; shadow_uniform_size as usize],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shadow_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shadow Uniform BG"),
            layout: &shadow_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_uniform_buffer,
                    offset: 0,
                    size: NonZeroU64::new(MAT4_SIZE),
                }),
            }],
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

        // --- CSM depth-only pipeline ---
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

        // --- Storage buffer for CSM VP matrices (fragment-shader read) ---
        let csm_vp_buf_size = (CSM_TOTAL_LAYERS * 64).max(64);
        let csm_vp_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("CSM ViewProj Storage"),
            contents: &vec![0u8; csm_vp_buf_size],
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            csm_texture,
            csm_layer_views,
            csm_array_view,
            shadow_sampler,
            shadow_pipeline,
            shadow_uniform_buffer,
            shadow_uniform_bind_group,
            csm_vp_buffer,
            slot_pool: ShadowSlotPool::new(),
            uniform_stride: uniform_stride as u32,
        }
    }

    /// Encode shadow render passes for the current frame's slot assignment.
    /// Returns the CSM view-projection matrices built this frame (indexed by
    /// layer = pool_slot * CSM_CASCADE_COUNT + cascade), for delta logging.
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
    ) -> Vec<Mat4> {
        let mut csm_matrices = vec![Mat4::IDENTITY; CSM_TOTAL_LAYERS];

        for slot in &assignment.slots {
            let light = &lights[slot.light_index as usize];

            if slot.shadow_kind == SHADOW_KIND_CSM {
                self.render_csm_passes(
                    encoder, queue, slot, light, vertex_buffer, index_buffer,
                    index_count, camera_view_proj, camera_near, camera_far,
                    &mut csm_matrices,
                );
            }
            // Any other shadow_kind is not handled by the CSM pipeline.
            // In particular, shadow_kind == 2 is reserved for sub-plan 9's
            // SDF sphere-trace, which has no rasterized shadow pass.
        }

        // Upload VP matrices for fragment shader sampling.
        queue.write_buffer(
            &self.csm_vp_buffer, 0,
            &shadow::pack_csm_view_proj_buffer(&csm_matrices),
        );

        csm_matrices
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

            let dyn_offset = csm_uniform_slot(layer_idx) * self.uniform_stride;
            write_mat4_to_buffer(
                queue, &self.shadow_uniform_buffer, dyn_offset as u64, &light_vp,
            );

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
            pass.set_bind_group(0, &self.shadow_uniform_bind_group, &[dyn_offset]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..index_count, 0, 0..1);
        }
    }
}

fn light_dir(light: &MapLight) -> Vec3 {
    Vec3::new(
        light.cone_direction[0],
        light.cone_direction[1],
        light.cone_direction[2],
    )
}

fn write_mat4_to_buffer(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, mat: &Mat4) {
    let mut bytes = [0u8; 64];
    for (i, &val) in mat.to_cols_array().iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&val.to_ne_bytes());
    }
    queue.write_buffer(buffer, offset, &bytes);
}
