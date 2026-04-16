// Textured renderer: GPU init, texture upload, pipeline, and draw.
// See: context/lib/rendering_pipeline.md

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::geometry::BvhTree;
use crate::lighting::influence::{self, LightInfluence};
use crate::lighting::{GPU_LIGHT_SIZE, pack_lights};
use crate::prl::MapLight;
use crate::texture::{LoadedTexture, TextureSet};
use crate::visibility::VisibleCells;

// --- WGSL Shaders ---

const SHADER_SOURCE: &str = include_str!("shaders/forward.wgsl");

// Wireframe overlay: culling-delta debug visualization. See shader header.
const WIREFRAME_SHADER_SOURCE: &str = include_str!("shaders/wireframe.wgsl");

// --- Uniform buffer layout ---

/// Per-frame uniform data: view-projection, camera world-space position,
/// ambient floor, and active light count.
///
/// Layout must match the WGSL `Uniforms` struct in `forward.wgsl` and
/// `wireframe.wgsl` — both shaders bind the same buffer. std140 rules
/// align `vec3<f32>` to 16 bytes, so `camera_position` (vec3) + trailing
/// `ambient_floor` (f32) share one 16-byte slot. `light_count` (u32)
/// starts a new slot and is padded out to a full vec4 slot for alignment.
///
/// Offsets (bytes):
///   0..64   view_proj      (mat4x4<f32>)
///   64..76  camera_position (vec3<f32>)
///   76..80  ambient_floor   (f32)
///   80..84  light_count     (u32)
///   84..96  _padding        (3 × u32)
const UNIFORM_SIZE: usize = 96;

fn build_uniform_data(
    view_proj: &Mat4,
    camera_position: Vec3,
    ambient_floor: f32,
    light_count: u32,
) -> [u8; UNIFORM_SIZE] {
    let mut bytes = [0u8; UNIFORM_SIZE];
    let cols = view_proj.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    bytes[64..68].copy_from_slice(&camera_position.x.to_ne_bytes());
    bytes[68..72].copy_from_slice(&camera_position.y.to_ne_bytes());
    bytes[72..76].copy_from_slice(&camera_position.z.to_ne_bytes());
    bytes[76..80].copy_from_slice(&ambient_floor.to_ne_bytes());
    bytes[80..84].copy_from_slice(&light_count.to_ne_bytes());
    // bytes 84..96 are _padding — left as zero.
    bytes
}

/// Default ambient floor applied when the caller doesn't override it.
/// Provisional value from sub-plan 3; tuned via the ambient-floor slider
/// in the settings menu. The right default is the lowest value where a
/// player can still navigate dark areas.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.05;

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

/// Geometry data the renderer needs from a level, including the BVH used to
/// build the GPU-driven indirect draw pipeline and the map's light list.
pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
    pub indices: &'a [u32],
    /// Global BVH loaded from the `Bvh` section. Always present for valid
    /// PRL levels — pre-BVH maps fail earlier in the loader.
    pub bvh: &'a BvhTree,
    /// Direct lights parsed from the AlphaLights PRL section. May be empty
    /// on maps compiled before the Lighting Foundation milestone.
    pub lights: &'a [MapLight],
    /// Per-light influence volumes from the LightInfluence PRL section.
    /// Same length as `lights` when present; empty if absent.
    pub light_influences: &'a [LightInfluence],
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

    /// Group 2 — storage buffer of packed `GpuLight` records, uploaded
    /// once at level load. Always bound; maps with zero lights get a
    /// 1-element dummy buffer so the binding is never empty (wgpu
    /// rejects zero-sized storage buffer bindings).
    lighting_bind_group: wgpu::BindGroup,
    /// Number of real (non-dummy) lights uploaded — this is the loop
    /// bound the fragment shader reads from `uniforms.light_count`.
    light_count: u32,
    /// Ambient floor applied before albedo multiply. Player-facing setting
    /// (0.0–1.0 slider); default is `DEFAULT_AMBIENT_FLOOR`. A scalar
    /// brightness only — no color.
    ambient_floor: f32,

    /// Per-light influence volumes for CPU-side frustum test. Stored at
    /// load time; consumed once per frame by `visible_lights` to determine
    /// which shadow-casting lights need active shadow-map slots.
    light_influences: Vec<LightInfluence>,
    /// Indices of lights whose influence volumes intersect the camera
    /// frustum this frame. Populated by `update_visible_lights` and
    /// consumed by sub-plan 5 (shadow-slot allocation).
    visible_light_indices: Vec<u32>,

    depth_view: wgpu::TextureView,

    /// GPU textures indexed by texture index.
    gpu_textures: Vec<GpuTexture>,
    /// Cached BVH leaves, used by the wireframe overlay to size per-leaf
    /// draw ranges. The renderer no longer consults this for the textured
    /// pass — that flows entirely through the compute shader / indirect
    /// buffer path.
    bvh_leaves: Vec<crate::geometry::BvhLeaf>,
    /// GPU-driven compute culling pipeline. `Some` when the level has a
    /// non-empty BVH; `None` for no-geometry mode.
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
                vec![0u8; 4],                                    // one dummy index
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

        // Uniform buffer (view-projection + camera position + ambient floor
        // + light count). Initial value is the hardcoded default view until
        // `update_per_frame_uniforms` is called from the main loop.
        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let light_count = geometry.map(|g| g.lights.len() as u32).unwrap_or(0);
        let ambient_floor = DEFAULT_AMBIENT_FLOOR;
        let uniform_data = build_uniform_data(&view_proj, Vec3::ZERO, ambient_floor, light_count);

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

        // Bind group layout for group 2: direct lighting storage buffers.
        // Binding 0 = GpuLight array, binding 1 = influence volumes.
        // Uploaded once at level load; animation and gameplay lights are
        // follow-up work (Milestone 6+).
        let lighting_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lighting Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Pack the map's lights into GPU bytes and create the storage
        // buffer. wgpu rejects a zero-size storage buffer, so we pad to a
        // single dummy record when there are no lights at all; the
        // shader's `light_count` loop bound stays at 0 so the dummy is
        // never read.
        let lights_data = match geometry {
            Some(g) if !g.lights.is_empty() => pack_lights(g.lights),
            _ => vec![0u8; GPU_LIGHT_SIZE],
        };
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Influence volume buffer (binding 1). Same dummy strategy as lights.
        let influence_data = match geometry {
            Some(g) if !g.light_influences.is_empty() => {
                influence::pack_influence(g.light_influences)
            }
            _ => vec![0u8; 16], // one dummy vec4<f32>
        };
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lighting Bind Group"),
            layout: &lighting_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: influence_buffer.as_entire_binding(),
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

        // Store the BVH leaves (for the wireframe overlay) and create the
        // compute cull pipeline off the loaded BVH. Empty-BVH levels skip
        // the pipeline entirely.
        let bvh_leaves: Vec<crate::geometry::BvhLeaf> =
            geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
        let compute_cull = geometry
            .filter(|g| !g.bvh.leaves.is_empty())
            .map(|g| ComputeCullPipeline::new(&device, g.bvh, has_multi_draw_indirect));

        // Depth buffer.
        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);

        // Pipeline layout. Group 2 is the direct-lighting storage buffer
        // introduced in sub-plan 3 of the lighting foundation.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&lighting_bind_group_layout),
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
            log::info!(
                "[Renderer] Textured pipeline ready: {} indices, {} textures, bvh_leaves={}",
                index_count,
                gpu_textures.len(),
                bvh_leaves.len(),
            );
            log::info!(
                "[Renderer] Wireframe overlay pipeline ready: {} line indices",
                wireframe_index_count,
            );
        } else {
            log::info!("[Renderer] Pipeline ready (no geometry loaded)");
        }

        let light_influences: Vec<LightInfluence> = geometry
            .map(|g| g.light_influences.to_vec())
            .unwrap_or_default();

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
            lighting_bind_group,
            light_count,
            ambient_floor,
            light_influences,
            visible_light_indices: Vec::new(),
            depth_view,
            gpu_textures,
            bvh_leaves,
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
    /// `update_per_frame_uniforms` after calling this (the camera owns aspect ratio).
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

    /// Upload the per-frame uniform buffer (view-projection matrix, camera
    /// world position, ambient floor, and light count). The ambient floor
    /// and light count are pulled from renderer state — they only change
    /// when the player adjusts the setting or a new level loads, but
    /// including them in every write keeps the upload path simple.
    pub fn update_per_frame_uniforms(&self, view_proj: Mat4, camera_position: Vec3) {
        let data = build_uniform_data(
            &view_proj,
            camera_position,
            self.ambient_floor,
            self.light_count,
        );
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
    }

    /// Current ambient floor value (0.0–1.0). Read by the diagnostic
    /// `Alt+Shift+{` / `Alt+Shift+}` chords so each press steps from the
    /// current value rather than a stored target. Will move to the
    /// settings menu when one exists.
    pub fn ambient_floor(&self) -> f32 {
        self.ambient_floor
    }

    /// Update the ambient floor, clamped to [0.0, 1.0]. Takes effect on
    /// the next `update_per_frame_uniforms` upload. Player-facing entry
    /// point is currently the `Alt+Shift+{` / `Alt+Shift+}` diagnostic
    /// chords; will move to the settings menu when one exists.
    pub fn set_ambient_floor(&mut self, value: f32) {
        self.ambient_floor = value.clamp(0.0, 1.0);
    }

    /// Run the per-frame sphere-vs-frustum test on all light influence
    /// volumes. Stashes the result for sub-plan 5 (shadow-slot allocation).
    /// Call once per frame, passing the same `Frustum` already produced by
    /// `extract_frustum_planes` for portal traversal.
    pub fn update_visible_lights(&mut self, frustum: &crate::visibility::Frustum) {
        self.visible_light_indices =
            influence::visible_lights(&self.light_influences, frustum);
        log::debug!(
            "[Renderer] visible_lights: {}/{}",
            self.visible_light_indices.len(),
            self.light_influences.len(),
        );
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Whether the compute cull pipeline is available (level has a non-empty BVH).
    #[allow(dead_code)]
    pub fn has_compute_cull(&self) -> bool {
        self.compute_cull.is_some()
    }

    /// GPU-driven render frame: dispatch the BVH traversal compute shader,
    /// then issue indirect draw calls. This is the only render path.
    ///
    /// `visible` carries the set of potentially-visible cells from the
    /// CPU-side visibility system (portal traversal, PVS, or fallbacks).
    /// The compute shader walks the BVH, frustum-culls each surviving leaf,
    /// checks its cell id against the visible-cell bitmask, and writes one
    /// `DrawIndexedIndirect` per surviving leaf. The render pass consumes
    /// them via `multi_draw_indexed_indirect` (or the singular fallback).
    pub fn render_frame_indirect(&mut self, visible: &VisibleCells, view_proj: Mat4) -> Result<()> {
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

        // Dispatch the BVH traversal compute shader. Portal DFS already
        // produced the visible-cell set on the CPU; the shader writes
        // per-leaf `DrawIndexedIndirect` commands into the indirect buffer
        // in the same command submission — no readback or GPU sync needed.
        if let Some(cull) = &mut self.compute_cull {
            cull.dispatch(&self.device, &self.queue, &mut encoder, visible, &view_proj);
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
                render_pass.set_bind_group(2, &self.lighting_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    // GPU-driven indirect draw path — the only path.
                    let gpu_textures = &self.gpu_textures;
                    cull.draw_indirect(&mut render_pass, &|pass, bucket| {
                        let bind_group = if (bucket as usize) < gpu_textures.len() {
                            &gpu_textures[bucket as usize].bind_group
                        } else {
                            &gpu_textures[0].bind_group
                        };
                        pass.set_bind_group(1, bind_group, &[]);
                    });
                }
            }
        }

        // Culling-delta wireframe overlay: draw ALL BVH leaves color-coded by cull status.
        if self.wireframe_enabled
            && self.has_geometry
            && self.wireframe_index_count > 0
            && !self.bvh_leaves.is_empty()
        {
            if let Some(cull) = &self.compute_cull {
                let cull_status_bind_group =
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Wireframe Cull Status BG"),
                        layout: &self.wireframe_cull_status_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: cull.cull_status_buffer().as_entire_binding(),
                        }],
                    });

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
                overlay_pass.set_bind_group(1, &cull_status_bind_group, &[]);
                overlay_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                overlay_pass.set_index_buffer(
                    self.wireframe_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                // Draw every BVH leaf with its leaf index as instance_index
                // so the shader can look up the per-leaf cull status.
                for (leaf_idx, leaf) in self.bvh_leaves.iter().enumerate() {
                    let wire_offset = leaf.index_offset * 2;
                    let wire_count = leaf.index_count * 2;
                    let li = leaf_idx as u32;
                    overlay_pass.draw_indexed(wire_offset..wire_offset + wire_count, 0, li..li + 1);
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
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
        let data = build_uniform_data(&vp, Vec3::ZERO, 0.05, 0);
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

    /// Regression: both the CPU-side `build_uniform_data` packer and the
    /// CPU-side `pack_light` packer must match the WGSL struct layouts
    /// that the fragment shader compiles against. Parsing the live
    /// shader source with naga catches drift before it reaches a GPU
    /// round-trip (see the similar test in `compute_cull.rs`).
    #[test]
    fn forward_wgsl_struct_strides_match_cpu_layout() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("forward shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "forward.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );

        let light_span = seen
            .get("GpuLight")
            .copied()
            .expect("forward shader should declare struct GpuLight");
        assert_eq!(
            light_span as usize,
            crate::lighting::GPU_LIGHT_SIZE,
            "forward.wgsl GpuLight stride ({light_span}) must match GPU_LIGHT_SIZE ({})",
            crate::lighting::GPU_LIGHT_SIZE,
        );
    }

    /// Ensure the wireframe shader's `Uniforms` struct stays in sync with
    /// the forward shader's — they share a single uniform buffer binding.
    #[test]
    fn wireframe_wgsl_uniforms_match_forward_layout() {
        let module = naga::front::wgsl::parse_str(WIREFRAME_SHADER_SOURCE)
            .expect("wireframe shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("wireframe shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "wireframe.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );
    }

    #[test]
    fn uniform_data_encodes_view_proj_camera_and_lighting_fields() {
        let vp = Mat4::IDENTITY;
        let camera = Vec3::new(10.0, 20.0, 30.0);
        let ambient_floor = 0.125_f32;
        let light_count = 7_u32;
        let data = build_uniform_data(&vp, camera, ambient_floor, light_count);

        // view_proj: first 64 bytes = 16 f32 identity columns.
        let mut floats = Vec::new();
        for chunk in data.chunks_exact(4).take(16) {
            floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
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

        // camera_position at bytes 64..76.
        let cx = f32::from_ne_bytes(data[64..68].try_into().unwrap());
        let cy = f32::from_ne_bytes(data[68..72].try_into().unwrap());
        let cz = f32::from_ne_bytes(data[72..76].try_into().unwrap());
        assert_eq!(cx, 10.0);
        assert_eq!(cy, 20.0);
        assert_eq!(cz, 30.0);

        // ambient_floor at bytes 76..80.
        let af = f32::from_ne_bytes(data[76..80].try_into().unwrap());
        assert!((af - ambient_floor).abs() < 1e-6);

        // light_count at bytes 80..84.
        let lc = u32::from_ne_bytes(data[80..84].try_into().unwrap());
        assert_eq!(lc, light_count);

        // Trailing pad (84..96) zero.
        for &b in &data[84..96] {
            assert_eq!(b, 0);
        }
    }
}
