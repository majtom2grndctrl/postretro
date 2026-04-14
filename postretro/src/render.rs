// Textured renderer: GPU init, texture upload, pipeline, and draw.
// See: context/lib/rendering_pipeline.md

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::geometry::CellChunkTable;
use crate::texture::{LoadedTexture, TextureSet};
use crate::visibility::{DrawRange, VisibleCells, VisibleFaces};

// --- WGSL Shaders ---

const SHADER_SOURCE: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    ambient_light: vec3<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_tangent: vec3<f32>,
    @location(3) bitangent_sign: f32,
};

// Decode octahedral-encoded u16x2 to unit direction vector.
fn oct_decode(enc: vec2<u32>) -> vec3<f32> {
    let ox = f32(enc.x) / 65535.0 * 2.0 - 1.0;
    let oy = f32(enc.y) / 65535.0 * 2.0 - 1.0;
    let z = 1.0 - abs(ox) - abs(oy);
    var x: f32;
    var y: f32;
    if z < 0.0 {
        x = (1.0 - abs(oy)) * select(-1.0, 1.0, ox >= 0.0);
        y = (1.0 - abs(ox)) * select(-1.0, 1.0, oy >= 0.0);
    } else {
        x = ox;
        y = oy;
    }
    return normalize(vec3<f32>(x, y, z));
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.base_uv;

    // Decode octahedral normal.
    out.world_normal = oct_decode(in.normal_oct);

    // Decode packed tangent: strip sign bit from v-component, remap 15-bit to 16-bit.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    // Flat ambient — normal and tangent are present but unused until Phase 4.
    let rgb = base_color.rgb * uniforms.ambient_light;
    return vec4<f32>(rgb, base_color.a);
}
"#;

// Wireframe overlay shader: culling-delta debug visualization.
// Draws all chunks color-coded by cull status:
//   green = rendered (passed portal vis + frustum)
//   cyan  = portal-culled (cell not in visible set)
//   red   = frustum-culled (cell visible, AABB outside frustum)
//
// The chunk index is passed via instance_index (first_instance in draw call).
// Cull status is read from a storage buffer written by the compute cull shader.
// See: context/lib/rendering_pipeline.md §7.1
const WIREFRAME_SHADER_SOURCE: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    ambient_light: vec3<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var<storage, read> cull_status: array<u32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    @location(3) tangent_packed: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) chunk_idx: u32,
};

@vertex
fn vs_main(in: VertexInput, @builtin(instance_index) instance_idx: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.chunk_idx = instance_idx;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let status = cull_status[in.chunk_idx];
    // 0 = portal-culled, 1 = frustum-culled, 2 = visible
    switch status {
        case 2u: { return vec4<f32>(0.0, 1.0, 0.2, 1.0); }  // green: rendered
        case 1u: { return vec4<f32>(1.0, 0.2, 0.15, 1.0); } // red: frustum-culled
        default: { return vec4<f32>(0.0, 0.9, 0.9, 1.0); }  // cyan: portal-culled
    }
}
"#;

// --- Uniform buffer layout ---

/// Per-frame uniform data: view-projection matrix + ambient light.
/// Layout must match the WGSL Uniforms struct (std140-aligned).
/// mat4x4 = 64 bytes, vec3 = 12 bytes + 4 bytes padding = 80 bytes total.
const UNIFORM_SIZE: usize = 80;

fn build_uniform_data(view_proj: &Mat4, ambient_light: [f32; 3]) -> [u8; UNIFORM_SIZE] {
    let mut bytes = [0u8; UNIFORM_SIZE];
    // 16 mat4 floats, 3 ambient floats, 1 f32 of padding = 80 bytes.
    // std140: vec3 is 16-byte aligned, so the trailing pad (bytes 76..80)
    // stays zero.
    let cols = view_proj.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    for (i, val) in ambient_light.iter().enumerate() {
        let off = 64 + i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    bytes
}

// --- GPU texture ---

/// A GPU-uploaded texture with its bind group for per-texture binding.
struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// Upload a single LoadedTexture to the GPU and create a bind group.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    loaded: &LoadedTexture,
    sampler: &wgpu::Sampler,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
) -> GpuTexture {
    let size = wgpu::Extent3d {
        width: loaded.width,
        height: loaded.height,
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &loaded.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * loaded.width),
            rows_per_image: Some(loaded.height),
        },
        size,
    );

    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("{label} Bind Group")),
        layout: texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });

    GpuTexture { bind_group }
}

// --- Depth buffer ---

/// Depth format used for the depth buffer.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Create the depth texture and return both the texture and its view
/// (for depth attachment).
fn create_depth_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_texture, view)
}

// --- Geometry data ---

/// Geometry data the renderer needs from a level, including cell chunk table
/// for draw call dispatch.
pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
    pub indices: &'a [u32],
    /// Per-cell draw chunk table. `None` for legacy PRL files without CellChunks.
    pub cell_chunk_table: Option<&'a CellChunkTable>,
}

// --- Renderer ---

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,

    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    depth_view: wgpu::TextureView,

    /// GPU textures indexed by texture index.
    gpu_textures: Vec<GpuTexture>,
    /// Per-cell draw chunk table for draw call dispatch (CPU fallback + wireframe).
    cell_chunk_table: Option<CellChunkTable>,
    /// GPU-driven compute culling pipeline. `Some` when the level has a
    /// CellChunkTable; `None` for legacy PRL files or no-geometry mode.
    compute_cull: Option<ComputeCullPipeline>,

    /// Debug wireframe overlay pipeline (LineList topology, cull-status-driven color).
    wireframe_pipeline: wgpu::RenderPipeline,
    /// Line-list index buffer built from the triangle index buffer at load time.
    /// Layout is 1:1 parallel with the triangle index buffer: each triangle at
    /// triangle-buffer range `[tri_start..tri_end]` (multiple of 3) maps to
    /// line-buffer range `[tri_start*2..tri_end*2]` (6 line indices per 3
    /// triangle indices).
    wireframe_index_buffer: wgpu::Buffer,
    wireframe_index_count: u32,
    /// Bind group layout for the wireframe cull-status storage buffer (group 1).
    wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    /// Whether the culling-delta wireframe overlay is active.
    wireframe_enabled: bool,

    /// Whether the surface is currently configured with vsync on
    /// (`AutoVsync`) or off (`AutoNoVsync`). Toggled by the
    /// `Alt+Shift+V` diagnostic chord so the frametime meter can be
    /// compared against real CPU cost; initialized to match the
    /// `AutoVsync` default chosen in `Renderer::new`.
    vsync_enabled: bool,

    has_geometry: bool,
}


impl Renderer {
    /// Create the renderer, taking ownership of all GPU state.
    ///
    /// `geometry` is `None` when no map file was loaded (renders clear color only).
    /// `texture_set` provides CPU-side textures for GPU upload; `None` for no textures.
    pub fn new(
        window: &Arc<Window>,
        geometry: Option<&LevelGeometry>,
        texture_set: Option<&TextureSet>,
    ) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

        // Probe for multi_draw_indexed_indirect support via downlevel flags.
        // Available on Vulkan, Metal, DX12; absent on WebGL2 (not a target).
        let downlevel = adapter.get_downlevel_capabilities();
        let has_multi_draw_indirect = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        if has_multi_draw_indirect {
            log::info!("[Renderer] Indirect execution supported (multi_draw_indexed_indirect)");
        } else {
            log::info!(
                "[Renderer] Indirect execution not supported — using singular draw_indexed_indirect fallback"
            );
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Postretro Device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .context("failed to create GPU device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        log::info!("[Renderer] vsync on");

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        // Build vertex and index buffers.
        let (vertex_data, index_data, index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let count = geom.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geom.vertices),
                bytemuck_cast_slice_u32(geom.indices),
                count,
            )
        } else {
            (
                vec![0u8; crate::geometry::WorldVertex::STRIDE], // one dummy vertex
                vec![0u8; 4],  // one dummy index
                0u32,
            )
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Index Buffer"),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        // Build a line-list index buffer from the triangle index buffer for the
        // wireframe overlay. Each triangle contributes its three edges as line
        // pairs. Shared edges are duplicated (cheap, and avoids a hash set).
        let (wireframe_index_data, wireframe_index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let line_indices = build_line_indices_from_triangles(geom.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };

        let wireframe_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Wireframe Line Index Buffer"),
            contents: &wireframe_index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        // Uniform buffer (view-projection + ambient light).
        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let uniform_data = build_uniform_data(&view_proj, [1.0, 1.0, 1.0]);

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Bind group layout for group 0: per-frame uniforms.
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Bind group layout for group 1: per-texture.
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Texture Bind Group Layout"),
                entries: &[
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
                ],
            });

        // Create shared sampler: nearest filtering for retro pixel aesthetic, repeat.
        let base_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Base Texture Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Upload textures to GPU.
        let gpu_textures = if let Some(tex_set) = texture_set {
            tex_set
                .textures
                .iter()
                .enumerate()
                .map(|(idx, loaded)| {
                    let label = format!("Texture {idx}");
                    upload_texture(
                        &device,
                        &queue,
                        loaded,
                        &base_sampler,
                        &texture_bind_group_layout,
                        &label,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        // If we have no textures at all, create a single placeholder so we always
        // have something to bind.
        let gpu_textures = if gpu_textures.is_empty() {
            let placeholder = crate::texture::generate_placeholder();
            vec![upload_texture(
                &device,
                &queue,
                &placeholder,
                &base_sampler,
                &texture_bind_group_layout,
                "Placeholder Texture",
            )]
        } else {
            gpu_textures
        };

        // Store per-cell draw chunk table and create compute cull pipeline.
        let cell_chunk_table = geometry.and_then(|g| g.cell_chunk_table.cloned());
        let compute_cull = cell_chunk_table.as_ref().map(|table| {
            // Count distinct material buckets.
            let max_bucket = table
                .chunks
                .iter()
                .map(|c| c.material_bucket_id)
                .max()
                .unwrap_or(0);
            let num_buckets = max_bucket + 1;
            ComputeCullPipeline::new(&device, table, num_buckets, has_multi_draw_indirect)
        });

        // Depth buffer.
        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);


        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Textured Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Textured Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        // position: vec3<f32> at offset 0
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        // base_uv: vec2<f32> at offset 12
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // normal_oct: u16x2 at offset 20
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // tangent_packed: u16x2 at offset 24
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
                format: DEPTH_FORMAT,
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

        // --- Wireframe overlay pipeline ---
        // Group 0 = uniforms (view_proj), group 1 = cull_status storage buffer.
        // Draws line lists with depth test disabled so edges render on top.
        // Colors are driven by per-chunk cull status from the compute shader.
        let wireframe_cull_status_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Wireframe Cull Status BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Wireframe Pipeline Layout"),
                bind_group_layouts: &[
                    Some(&uniform_bind_group_layout),
                    Some(&wireframe_cull_status_layout),
                ],
                immediate_size: 0,
            });

        let wireframe_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wireframe Shader"),
            source: wgpu::ShaderSource::Wgsl(WIREFRAME_SHADER_SOURCE.into()),
        });

        let wireframe_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
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
                topology: wgpu::PrimitiveTopology::LineList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
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

        if has_geometry {
            let chunk_summary = match &cell_chunk_table {
                Some(t) => format!("{} cells, {} chunks", t.cell_ranges.len(), t.chunks.len()),
                None => "none (legacy)".to_string(),
            };
            log::info!(
                "[Renderer] Textured pipeline ready: {} indices, {} textures, cell_chunks=[{}]",
                index_count,
                gpu_textures.len(),
                chunk_summary,
            );
            log::info!(
                "[Renderer] Wireframe overlay pipeline ready: {} line indices",
                wireframe_index_count,
            );
        } else {
            log::info!("[Renderer] Pipeline ready (no geometry loaded)");
        }

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            is_surface_configured: true,
            pipeline,
            vertex_buffer,
            index_buffer,
            index_count,
            uniform_buffer,
            uniform_bind_group,
            depth_view,
            gpu_textures,
            cell_chunk_table,
            compute_cull,
            wireframe_pipeline,
            wireframe_index_buffer,
            wireframe_index_count,
            wireframe_cull_status_bgl: wireframe_cull_status_layout,
            wireframe_enabled: false,
            vsync_enabled: true,
            has_geometry,
        })
    }

    /// Toggle the culling-delta wireframe debug overlay on/off.
    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }

    /// Flip between `AutoVsync` and `AutoNoVsync`. Rebuilds the swapchain
    /// via `surface.configure`. Returns the new state (`true` = vsync on).
    ///
    /// Diagnostic-only — triggered by the `Alt+Shift+V` chord so the user
    /// can compare vsync-pinned frametimes against real CPU cost.
    pub fn toggle_vsync(&mut self) -> bool {
        self.vsync_enabled = !self.vsync_enabled;
        self.surface_config.present_mode = if self.vsync_enabled {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        self.surface.configure(&self.device, &self.surface_config);
        self.vsync_enabled
    }

    /// Whether the surface is currently configured with vsync on.
    /// Read by the title rewrite so the current state is always visible.
    pub fn vsync_enabled(&self) -> bool {
        self.vsync_enabled
    }

    /// Handle window resize. Reconfigures the surface and recreates the depth buffer.
    /// The caller is responsible for updating the view-projection matrix via
    /// `update_view_projection` after calling this (the camera owns aspect ratio).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (_depth_texture, depth_view) = create_depth_texture(&self.device, width, height);
        self.depth_view = depth_view;
        self.is_surface_configured = true;
    }

    pub fn update_view_projection(&self, view_proj: Mat4) {
        let data = build_uniform_data(&view_proj, [1.0, 1.0, 1.0]);
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Whether the compute cull pipeline is available (level has a CellChunkTable).
    pub fn has_compute_cull(&self) -> bool {
        self.compute_cull.is_some()
    }

    /// GPU-driven render frame: dispatch compute cull shader, then issue
    /// indirect draw calls. This is the primary render path when a
    /// CellChunkTable is available.
    ///
    /// `visible` carries the set of potentially-visible cell IDs from the
    /// CPU-side visibility system (portal traversal, PVS, or fallbacks).
    /// The compute shader performs per-chunk AABB frustum culling and writes
    /// DrawIndexedIndirect commands; the render pass consumes them via
    /// multi_draw_indexed_indirect (or the singular fallback).
    pub fn render_frame_indirect(
        &mut self,
        visible: &VisibleCells,
        view_proj: Mat4,
    ) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        // Collect visible cell IDs for compute dispatch and wireframe overlay.
        let visible_cell_ids: Vec<u32> = match visible {
            VisibleCells::DrawAll => {
                // All cells visible — collect all cell IDs from the table.
                if let Some(table) = &self.cell_chunk_table {
                    table.cell_ranges.iter().map(|cr| cr.cell_id).collect()
                } else {
                    Vec::new()
                }
            }
            VisibleCells::Culled(cells) => cells.clone(),
        };

        // Dispatch compute cull shader. The fixed-slot design writes
        // instance_count = 1 for visible chunks directly in the indirect
        // buffer. No readback or GPU sync needed — the render pass
        // consumes the buffer in the same command buffer submission.
        if let Some(cull) = &mut self.compute_cull {
            cull.dispatch(
                &self.device,
                &self.queue,
                &mut encoder,
                &visible_cell_ids,
                &view_proj,
            );
        }


        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Textured Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    // GPU-driven indirect draw path.
                    let gpu_textures = &self.gpu_textures;
                    cull.draw_indirect(
                        &mut render_pass,
                        &|pass, bucket| {
                            let bind_group = if (bucket as usize) < gpu_textures.len() {
                                &gpu_textures[bucket as usize].bind_group
                            } else {
                                &gpu_textures[0].bind_group
                            };
                            pass.set_bind_group(1, bind_group, &[]);
                        },
                    );
                } else {
                    // Legacy CPU fallback (no CellChunkTable).
                    let bind_group = self.texture_bind_group(0);
                    render_pass.set_bind_group(1, bind_group, &[]);
                    render_pass.draw_indexed(0..self.index_count, 0, 0..1);
                }
            }
        }

        // Culling-delta wireframe overlay: draw ALL chunks color-coded by cull status.
        if self.wireframe_enabled
            && self.has_geometry
            && self.wireframe_index_count > 0
        {
            if let (Some(cull), Some(table)) = (&self.compute_cull, &self.cell_chunk_table) {
                let cull_status_bind_group =
                    self.device
                        .create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Wireframe Cull Status BG"),
                            layout: &self.wireframe_cull_status_bgl,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: cull.cull_status_buffer().as_entire_binding(),
                            }],
                        });

                let mut overlay_pass =
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Wireframe Overlay Pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(
                            wgpu::RenderPassDepthStencilAttachment {
                                view: &self.depth_view,
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            },
                        ),
                        ..Default::default()
                    });

                overlay_pass.set_pipeline(&self.wireframe_pipeline);
                overlay_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                overlay_pass.set_bind_group(1, &cull_status_bind_group, &[]);
                overlay_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                overlay_pass.set_index_buffer(
                    self.wireframe_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                // Draw every chunk with its chunk index as instance_index so
                // the shader can look up the cull status per chunk.
                for (chunk_idx, chunk) in table.chunks.iter().enumerate() {
                    let wire_offset = chunk.index_offset * 2;
                    let wire_count = chunk.index_count * 2;
                    let ci = chunk_idx as u32;
                    overlay_pass.draw_indexed(
                        wire_offset..wire_offset + wire_count,
                        0,
                        ci..ci + 1,
                    );
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Render a frame with visibility-based culling and per-cell chunk draw calls.
    /// Legacy path: CPU-driven per-cell draws. Used when compute cull is not
    /// available or for backward compatibility.
    ///
    /// When `visible` is `VisibleFaces::Culled`, extracts visible cell ids from the
    /// draw ranges and issues per-chunk draw calls for those cells. When `DrawAll`,
    /// draws all chunks.
    pub fn render_frame(&self, visible: &VisibleFaces) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        // Compute visible cell ids once per frame. Both the textured pass and
        // the (optional) wireframe overlay pass consume the same set.
        let visible_cells: Vec<u32> = match visible {
            VisibleFaces::DrawAll => Vec::new(),
            VisibleFaces::Culled(ranges) => self.collect_visible_cell_ids(ranges),
        };

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Textured Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                match visible {
                    VisibleFaces::DrawAll => {
                        self.draw_all_chunks(&mut render_pass);
                    }
                    VisibleFaces::Culled(_) => {
                        self.draw_visible_chunks(&mut render_pass, &visible_cells);
                    }
                }
            }
        }

        // Culling-delta wireframe overlay requires the compute cull pipeline
        // for per-chunk cull status. Not available in the legacy CPU path.
        // (The legacy path is only used when POSTRETRO_FORCE_LEGACY=1 or
        // no CellChunkTable is present.)

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Draw all chunks. Used when visibility data is unavailable (DrawAll path).
    fn draw_all_chunks<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let mut draw_calls = 0u32;

        if let Some(table) = &self.cell_chunk_table {
            let mut last_tex: Option<u32> = None;
            for chunk in &table.chunks {
                let tex_idx = chunk.material_bucket_id;
                if last_tex != Some(tex_idx) {
                    let bind_group = self.texture_bind_group(tex_idx as usize);
                    render_pass.set_bind_group(1, bind_group, &[]);
                    last_tex = Some(tex_idx);
                }
                let start = chunk.index_offset;
                let end = start + chunk.index_count;
                render_pass.draw_indexed(start..end, 0, 0..1);
                draw_calls += 1;
            }
        } else {
            // Legacy fallback: draw the entire index buffer with the first texture.
            let bind_group = self.texture_bind_group(0);
            render_pass.set_bind_group(1, bind_group, &[]);
            render_pass.draw_indexed(0..self.index_count, 0, 0..1);
            draw_calls = 1;
        }

        log::trace!("[Renderer] DrawAll: {draw_calls} draw calls");
    }

    /// Draw chunks for visible cells. Uses the CellChunkTable to look up chunks
    /// per cell_id and issues one draw call per chunk.
    fn draw_visible_chunks<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        visible_cells: &[u32],
    ) {
        let mut draw_calls = 0u32;
        let mut last_tex: Option<u32> = None;

        if let Some(table) = &self.cell_chunk_table {
            for &cell_id in visible_cells {
                for chunk in table.chunks_for_cell(cell_id) {
                    let tex_idx = chunk.material_bucket_id;
                    if last_tex != Some(tex_idx) {
                        let bind_group = self.texture_bind_group(tex_idx as usize);
                        render_pass.set_bind_group(1, bind_group, &[]);
                        last_tex = Some(tex_idx);
                    }
                    let start = chunk.index_offset;
                    let end = start + chunk.index_count;
                    render_pass.draw_indexed(start..end, 0, 0..1);
                    draw_calls += 1;
                }
            }
        } else {
            // Legacy fallback: draw the entire index buffer with the first texture.
            let bind_group = self.texture_bind_group(0);
            render_pass.set_bind_group(1, bind_group, &[]);
            render_pass.draw_indexed(0..self.index_count, 0, 0..1);
            draw_calls = 1;
        }

        log::trace!(
            "[Renderer] Culled: {draw_calls} draw calls, {} visible cells",
            visible_cells.len(),
        );
    }


    /// Look up the texture bind group for a material bucket index. Falls back
    /// to the first texture (placeholder) if the index is out of range.
    fn texture_bind_group(&self, tex_idx: usize) -> &wgpu::BindGroup {
        if tex_idx < self.gpu_textures.len() {
            &self.gpu_textures[tex_idx].bind_group
        } else {
            &self.gpu_textures[0].bind_group
        }
    }

    /// Convert visibility draw ranges to a list of visible cell ids. The
    /// Transitional bridge: converts legacy per-face draw ranges to cell IDs
    /// for the GPU-driven path. O(cells * chunks * ranges) — fine at current
    /// map sizes but should be removed once `determine_visible_cells` fully
    /// replaces `determine_prl_visibility` in the GPU path.
    ///
    /// The visibility system produces per-face draw ranges keyed by leaf; since
    /// cell_id == leaf_index in the current compiler, we extract leaf indices
    /// that overlap any draw range and use them as cell ids.
    fn collect_visible_cell_ids(&self, ranges: &[DrawRange]) -> Vec<u32> {
        let table = match &self.cell_chunk_table {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut visible = Vec::new();
        for cr in &table.cell_ranges {
            let cell_id = cr.cell_id;
            let start = cr.chunk_start as usize;
            let end = start + cr.chunk_count as usize;

            // A cell is visible if any of its chunks overlap any draw range.
            let is_visible = table.chunks[start..end].iter().any(|chunk| {
                let c_start = chunk.index_offset;
                let c_end = chunk.index_offset + chunk.index_count;
                ranges.iter().any(|r| {
                    let r_start = r.index_offset;
                    let r_end = r.index_offset + r.index_count;
                    r_start < c_end && r_end > c_start
                })
            });

            if is_visible {
                visible.push(cell_id);
            }
        }
        visible
    }
}

// --- Hardcoded view-projection ---

/// Camera at (0, 200, 500) looking at origin.
fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2, // 90 degree FOV
        aspect,
        0.1,
        4096.0,
    );

    projection * view
}

// --- Byte casting helpers ---

fn cast_world_vertices_to_bytes(data: &[crate::geometry::WorldVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::geometry::WorldVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.normal_oct {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.tangent_packed {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
    }
    bytes
}

/// Build a line-list index buffer from a triangle-list index buffer.
/// Each triangle `[a, b, c]` contributes three line-list edges
/// `[a, b, b, c, c, a]`. Shared edges across triangles are emitted multiple
/// times; this is cheap and fine for a debug overlay. Incomplete trailing
/// indices (not a full triangle) are ignored.
fn build_line_indices_from_triangles(tri_indices: &[u32]) -> Vec<u32> {
    let tri_count = tri_indices.len() / 3;
    let mut lines = Vec::with_capacity(tri_count * 6);
    for tri in tri_indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        lines.push(a);
        lines.push(b);
        lines.push(b);
        lines.push(c);
        lines.push(c);
        lines.push(a);
    }
    lines
}

fn bytemuck_cast_slice_u32(data: &[u32]) -> Vec<u8> {
    let byte_len = std::mem::size_of_val(data);
    let mut bytes = Vec::with_capacity(byte_len);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_projection_is_finite() {
        let vp = build_default_view_projection(16.0 / 9.0);
        let cols = vp.to_cols_array();
        for (i, val) in cols.iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn cast_world_vertices_roundtrips() {
        let input = vec![
            crate::geometry::WorldVertex {
                position: [1.0, 2.0, 3.0],
                base_uv: [0.5, 0.75],
                normal_oct: [32768, 32768],
                tangent_packed: [65535, 32768],
            },
            crate::geometry::WorldVertex {
                position: [4.0, 5.0, 6.0],
                base_uv: [0.25, 0.125],
                normal_oct: [0, 32768],
                tangent_packed: [32768, 0],
            },
        ];
        let bytes = cast_world_vertices_to_bytes(&input);
        // 2 vertices * 28 bytes = 56 bytes
        assert_eq!(bytes.len(), 56);

        // Read back first vertex: 3 f32 pos + 2 f32 uv + 2 u16 normal + 2 u16 tangent = 28 bytes
        let pos_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let pos_y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let pos_z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let uv_u = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let uv_v = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        let n_u = u16::from_ne_bytes(bytes[20..22].try_into().unwrap());
        let n_v = u16::from_ne_bytes(bytes[22..24].try_into().unwrap());
        let t_u = u16::from_ne_bytes(bytes[24..26].try_into().unwrap());
        let t_v = u16::from_ne_bytes(bytes[26..28].try_into().unwrap());

        assert_eq!([pos_x, pos_y, pos_z], [1.0, 2.0, 3.0]);
        assert_eq!([uv_u, uv_v], [0.5, 0.75]);
        assert_eq!([n_u, n_v], [32768, 32768]);
        assert_eq!([t_u, t_v], [65535, 32768]);
    }

    #[test]
    fn byte_cast_u32_roundtrips() {
        let input = vec![100u32, 200, 300];
        let bytes = bytemuck_cast_slice_u32(&input);
        assert_eq!(bytes.len(), 12);

        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(output, vec![100, 200, 300]);
    }

    #[test]
    fn uniform_data_has_correct_size() {
        let vp = Mat4::IDENTITY;
        let data = build_uniform_data(&vp, [1.0, 1.0, 1.0]);
        assert_eq!(data.len(), UNIFORM_SIZE);
    }

    #[test]
    fn line_indices_from_single_triangle_produces_three_edges() {
        let tri = vec![0u32, 1, 2];
        let lines = build_line_indices_from_triangles(&tri);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn line_indices_from_two_triangles_produces_twelve_indices() {
        let tris = vec![0u32, 1, 2, 3, 4, 5];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines.len(), 12);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0, 3, 4, 4, 5, 5, 3]);
    }

    #[test]
    fn line_indices_from_empty_input_is_empty() {
        let lines = build_line_indices_from_triangles(&[]);
        assert!(lines.is_empty());
    }

    #[test]
    fn line_indices_ignores_incomplete_trailing_triangle() {
        // 4 indices = 1 full triangle + 1 dangling index.
        let tris = vec![0u32, 1, 2, 3];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn uniform_data_encodes_view_proj_and_ambient() {
        let vp = Mat4::IDENTITY;
        let ambient = [0.5, 0.7, 0.9];
        let data = build_uniform_data(&vp, ambient);

        // Read back the view-proj matrix (first 64 bytes = 16 floats).
        let mut floats = Vec::new();
        for chunk in data.chunks_exact(4) {
            floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }

        // Identity matrix columns.
        let identity = Mat4::IDENTITY.to_cols_array();
        for i in 0..16 {
            let epsilon = 1e-6;
            assert!(
                (floats[i] - identity[i]).abs() < epsilon,
                "view_proj[{i}] mismatch: expected {}, got {}",
                identity[i],
                floats[i],
            );
        }

        // Ambient light at floats[16..19].
        let epsilon = 1e-6;
        assert!((floats[16] - 0.5).abs() < epsilon);
        assert!((floats[17] - 0.7).abs() < epsilon);
        assert!((floats[18] - 0.9).abs() < epsilon);
    }
}
