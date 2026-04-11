// Textured renderer: GPU init, texture upload, pipeline, and draw.
// See: context/lib/rendering_pipeline.md

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::bsp::TextureSubRange;
use crate::texture::{LoadedTexture, TextureSet};
use crate::visibility::{DrawRange, VisibleFaces};

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
    @location(2) vertex_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) vert_color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.base_uv;
    out.vert_color = in.vertex_color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base_color = textureSample(base_texture, base_sampler, in.uv);
    let rgb = base_color.rgb * uniforms.ambient_light * in.vert_color.rgb;
    let a = base_color.a * in.vert_color.a;
    return vec4<f32>(rgb, a);
}
"#;

// Wireframe overlay shader: shares the uniforms bind group (group 0) and vertex
// format with the textured pipeline, but ignores UV and vertex_color. Emits a flat
// bright-green color for high visibility over textured surfaces. Intended for
// debug use only (toggled at runtime via Shift+\).
const WIREFRAME_SHADER_SOURCE: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    ambient_light: vec3<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) vertex_color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return uniforms.view_proj * vec4<f32>(in.position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.2, 1.0);
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

fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
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
        format: wgpu::TextureFormat::Depth24Plus,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    depth_texture.create_view(&wgpu::TextureViewDescriptor::default())
}

// --- Geometry data ---

/// Geometry data the renderer needs from a BSP level, including texture sub-ranges
/// for draw call batching.
pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::bsp::TexturedVertex],
    pub indices: &'a [u32],
    /// Per-leaf texture sub-ranges for draw call grouping.
    /// Indexed by leaf index; each leaf contains a list of texture sub-ranges.
    pub leaf_texture_sub_ranges: Vec<Vec<TextureSubRange>>,
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

    /// GPU textures indexed by BSP miptexture index.
    gpu_textures: Vec<GpuTexture>,
    /// Per-leaf texture sub-ranges for draw call grouping.
    leaf_texture_sub_ranges: Vec<Vec<TextureSubRange>>,

    /// Debug wireframe overlay pipeline (LineList topology, flat-color shader).
    wireframe_pipeline: wgpu::RenderPipeline,
    /// Line-list index buffer built from the triangle index buffer at load time.
    /// Layout is 1:1 parallel with the triangle index buffer: each triangle at
    /// triangle-buffer range `[tri_start..tri_end]` (multiple of 3) maps to
    /// line-buffer range `[tri_start*2..tri_end*2]` (6 line indices per 3
    /// triangle indices).
    wireframe_index_buffer: wgpu::Buffer,
    wireframe_index_count: u32,
    /// Current wireframe debug overlay mode.
    wireframe_mode: WireframeMode,

    has_geometry: bool,
}

/// Debug wireframe overlay state. Cycled by `Alt+Shift+\` in the engine.
///
/// - `Off`: no overlay.
/// - `Culled`: draws lines only for the sub-ranges the textured renderer
///   actually drew this frame (reuses PVS + frustum + back-face culled set).
/// - `All`: draws lines for the entire index buffer, regardless of visibility.
///
/// Comparing `Culled` against `All` reveals where the renderer is wrongly
/// culling or wrongly including surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireframeMode {
    Off,
    Culled,
    All,
}

impl WireframeMode {
    /// Next state in the `Off → Culled → All → Off` cycle.
    fn next(self) -> Self {
        match self {
            WireframeMode::Off => WireframeMode::Culled,
            WireframeMode::Culled => WireframeMode::All,
            WireframeMode::All => WireframeMode::Off,
        }
    }
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

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        // Build vertex and index buffers.
        let (vertex_data, index_data, index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let count = geom.indices.len() as u32;
            (
                cast_textured_vertices_to_bytes(geom.vertices),
                bytemuck_cast_slice_u32(geom.indices),
                count,
            )
        } else {
            (
                vec![0u8; 36], // one dummy vertex (9 floats)
                vec![0u8; 4],  // one dummy index
                0u32,
            )
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP Index Buffer"),
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

        let wireframe_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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

        // Store per-leaf texture sub-ranges.
        let leaf_texture_sub_ranges = geometry
            .map(|g| g.leaf_texture_sub_ranges.clone())
            .unwrap_or_default();

        // Depth buffer.
        let depth_view = create_depth_texture(&device, surface_config.width, surface_config.height);

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
                    array_stride: crate::bsp::TexturedVertex::STRIDE as wgpu::BufferAddress,
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
                        // vertex_color: vec4<f32> at offset 20
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
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
                format: wgpu::TextureFormat::Depth24Plus,
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
        // Uses only the uniform bind group (group 0), matches the textured vertex
        // buffer layout, draws line lists, and disables depth write + uses
        // `CompareFunction::Always` so edges render on top of textured surfaces.
        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Wireframe Pipeline Layout"),
                bind_group_layouts: &[Some(&uniform_bind_group_layout)],
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
                    array_stride: crate::bsp::TexturedVertex::STRIDE as wgpu::BufferAddress,
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
                            format: wgpu::VertexFormat::Float32x4,
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
                format: wgpu::TextureFormat::Depth24Plus,
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
            log::info!(
                "[Renderer] Textured pipeline ready: {} indices, {} textures, {} leaf sub-range groups",
                index_count,
                gpu_textures.len(),
                leaf_texture_sub_ranges.len(),
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
            leaf_texture_sub_ranges,
            wireframe_pipeline,
            wireframe_index_buffer,
            wireframe_index_count,
            wireframe_mode: WireframeMode::Off,
            has_geometry,
        })
    }

    /// Advance the wireframe debug overlay to the next mode in the
    /// `Off → Culled → All → Off` cycle. Returns the new mode.
    pub fn cycle_wireframe_mode(&mut self) -> WireframeMode {
        self.wireframe_mode = self.wireframe_mode.next();
        log::info!("[Renderer] Wireframe overlay: {:?}", self.wireframe_mode);
        self.wireframe_mode
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
        self.depth_view = create_depth_texture(&self.device, width, height);
        self.is_surface_configured = true;
    }

    pub fn update_view_projection(&self, view_proj: Mat4) {
        let data = build_uniform_data(&view_proj, [1.0, 1.0, 1.0]);
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Render a frame with visibility-based culling and textured draw calls.
    ///
    /// When `visible` is `VisibleFaces::Culled`, issues draw calls per (leaf, texture) pair
    /// using pre-computed texture sub-ranges. When `DrawAll`, draws everything.
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

        // Compute the visible-leaf set once per frame when culling is active.
        // Both the textured pass and the (optional) wireframe overlay pass
        // consume the same set; computing it here avoids a redundant scan
        // when the wireframe overlay is enabled.
        let visible_leaves: Vec<usize> = match visible {
            VisibleFaces::DrawAll => Vec::new(),
            VisibleFaces::Culled(ranges) => self.collect_visible_leaf_indices(ranges),
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
                        self.draw_all_textured(&mut render_pass);
                    }
                    VisibleFaces::Culled(_) => {
                        self.draw_visible_textured(&mut render_pass, &visible_leaves);
                    }
                }
            }
        }

        // Debug wireframe overlay pass. Loads the existing color target and depth
        // buffer (no clear) and draws edges on top with depth test disabled.
        //
        // `Culled` reuses the same visible-leaf set the textured pass drew so
        // the wireframe matches the ground truth of what was rendered. `All`
        // draws every line in the buffer regardless of visibility; comparing
        // the two reveals culling bugs.
        if self.wireframe_mode != WireframeMode::Off
            && self.has_geometry
            && self.wireframe_index_count > 0
        {
            let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            overlay_pass.set_pipeline(&self.wireframe_pipeline);
            overlay_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            overlay_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            overlay_pass.set_index_buffer(
                self.wireframe_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );

            match self.wireframe_mode {
                WireframeMode::Off => {}
                WireframeMode::All => {
                    overlay_pass.draw_indexed(0..self.wireframe_index_count, 0, 0..1);
                }
                WireframeMode::Culled => {
                    self.draw_visible_wireframe(&mut overlay_pass, visible, &visible_leaves);
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Draw all geometry with texture sub-ranges. Used when PVS is unavailable.
    fn draw_all_textured<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let mut draw_calls = 0u32;
        for leaf_sub_ranges in &self.leaf_texture_sub_ranges {
            for sub_range in leaf_sub_ranges {
                let tex_idx = sub_range.texture_index as usize;
                let bind_group = if tex_idx < self.gpu_textures.len() {
                    &self.gpu_textures[tex_idx].bind_group
                } else {
                    // Fallback to first texture (placeholder).
                    &self.gpu_textures[0].bind_group
                };
                render_pass.set_bind_group(1, bind_group, &[]);
                let start = sub_range.index_offset;
                let end = start + sub_range.index_count;
                render_pass.draw_indexed(start..end, 0, 0..1);
                draw_calls += 1;
            }
        }
        log::trace!("[Renderer] DrawAll: {draw_calls} draw calls");
    }

    /// Draw visible faces using texture sub-ranges per leaf.
    /// `visible_leaves` is the pre-computed set of visible leaf indices
    /// (see `collect_visible_leaf_indices`), lifted to the caller so the
    /// wireframe overlay pass can reuse it without rescanning.
    ///
    /// Deduplicates `set_bind_group` calls: per-leaf sub-ranges are already
    /// sorted by `texture_index` (see `prl.rs` `build_leaf_texture_sub_ranges`),
    /// so tracking the last-set texture across the outer leaf loop lets
    /// cross-leaf transitions skip redundant binds when leaf N's final
    /// texture matches leaf N+1's first texture.
    fn draw_visible_textured<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        visible_leaves: &[usize],
    ) {
        let mut draw_calls = 0u32;
        let mut last_tex: Option<usize> = None;
        for leaf_idx in visible_leaves {
            if let Some(sub_ranges) = self.leaf_texture_sub_ranges.get(*leaf_idx) {
                for sub_range in sub_ranges {
                    let tex_idx = sub_range.texture_index as usize;
                    if last_tex != Some(tex_idx) {
                        let bind_group = if tex_idx < self.gpu_textures.len() {
                            &self.gpu_textures[tex_idx].bind_group
                        } else {
                            &self.gpu_textures[0].bind_group
                        };
                        render_pass.set_bind_group(1, bind_group, &[]);
                        last_tex = Some(tex_idx);
                    }
                    let start = sub_range.index_offset;
                    let end = start + sub_range.index_count;
                    render_pass.draw_indexed(start..end, 0, 0..1);
                    draw_calls += 1;
                }
            }
        }
        log::trace!(
            "[Renderer] Culled: {draw_calls} draw calls, {} visible leaves",
            visible_leaves.len(),
        );
    }

    /// Draw the debug wireframe overlay for the same visible sub-ranges the
    /// textured pass drew this frame. Reuses the visibility determination
    /// (`VisibleFaces`) produced upstream — no separate culling pass — so the
    /// "Culled" wireframe is guaranteed to match the textured output exactly.
    ///
    /// The line index buffer is 1:1 parallel with the triangle index buffer
    /// (6 line indices per 3 triangle indices), so each textured sub-range
    /// `[start..end]` becomes the line sub-range `[start*2..end*2]`.
    fn draw_visible_wireframe<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        visible: &VisibleFaces,
        visible_leaves: &[usize],
    ) {
        match visible {
            VisibleFaces::DrawAll => {
                // Visibility system said "draw everything", so the culled
                // overlay matches the all overlay: draw the whole line buffer.
                render_pass.draw_indexed(0..self.wireframe_index_count, 0, 0..1);
            }
            VisibleFaces::Culled(_) => {
                let mut draw_calls = 0u32;
                for leaf_idx in visible_leaves {
                    if let Some(sub_ranges) = self.leaf_texture_sub_ranges.get(*leaf_idx) {
                        for sub_range in sub_ranges {
                            let start = sub_range.index_offset * 2;
                            let end = (sub_range.index_offset + sub_range.index_count) * 2;
                            render_pass.draw_indexed(start..end, 0, 0..1);
                            draw_calls += 1;
                        }
                    }
                }
                log::trace!(
                    "[Renderer] Wireframe Culled: {draw_calls} line draw calls, {} visible leaves",
                    visible_leaves.len(),
                );
            }
        }
    }

    /// Identify which leaf indices are visible given a set of `DrawRange`s
    /// produced by the visibility system. A leaf is visible iff any of its
    /// texture sub-ranges overlap any visible draw range in index space.
    ///
    /// Shared by both the textured and wireframe-culled draw paths so they
    /// can never disagree on "which leaves are visible this frame".
    fn collect_visible_leaf_indices(&self, ranges: &[DrawRange]) -> Vec<usize> {
        let mut visible_leaves = Vec::new();
        for (leaf_idx, sub_ranges) in self.leaf_texture_sub_ranges.iter().enumerate() {
            if sub_ranges.is_empty() {
                continue;
            }
            // Leaves have their sub-ranges sorted by index_offset, so the
            // first/last entries bound the leaf's span in the index buffer.
            let leaf_start = sub_ranges.first().map(|sr| sr.index_offset).unwrap_or(0);
            let leaf_end = sub_ranges
                .last()
                .map(|sr| sr.index_offset + sr.index_count)
                .unwrap_or(0);

            let is_visible = ranges.iter().any(|r| {
                let r_start = r.index_offset;
                let r_end = r.index_offset + r.index_count;
                // Overlap test.
                r_start < leaf_end && r_end > leaf_start
            });

            if is_visible {
                visible_leaves.push(leaf_idx);
            }
        }
        visible_leaves
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

fn cast_textured_vertices_to_bytes(data: &[crate::bsp::TexturedVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::bsp::TexturedVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.vertex_color {
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
    fn cast_textured_vertices_roundtrips() {
        let input = vec![
            crate::bsp::TexturedVertex {
                position: [1.0, 2.0, 3.0],
                base_uv: [0.5, 0.75],
                vertex_color: [1.0, 1.0, 1.0, 1.0],
            },
            crate::bsp::TexturedVertex {
                position: [4.0, 5.0, 6.0],
                base_uv: [0.25, 0.125],
                vertex_color: [0.5, 0.5, 0.5, 1.0],
            },
        ];
        let bytes = cast_textured_vertices_to_bytes(&input);
        // 2 vertices * 9 floats * 4 bytes = 72 bytes
        assert_eq!(bytes.len(), 72);

        // Read back.
        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(
            output,
            vec![
                1.0, 2.0, 3.0, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 4.0, 5.0, 6.0, 0.25, 0.125, 0.5, 0.5,
                0.5, 1.0
            ]
        );
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
